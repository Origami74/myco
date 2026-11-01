//! A userspace TCP/IP stack on the node's own mesh address.
//!
//! A FIPS node with an app-owned TUN (`fips::Node::enable_app_owned_tun`)
//! exchanges IPv6 packets with its embedder over two channels and never
//! touches a system TUN. Something still has to turn those packets into TCP
//! connections. On Android that used to be the kernel, reached by handing the
//! packets to a `VpnService` — which costs the user their one VPN slot, for
//! traffic that never leaves the app.
//!
//! [`Netstack`] is that something, in process: a [smoltcp] interface holding
//! the node's `fd00::` address, fed the node's inbound packets with
//! [`Netstack::offer`] and writing its own to an [`Egress`] the embedder points
//! at the node. On top of it, [`Netstack::bind`] and [`Netstack::connect`] give
//! a [`TcpListener`] and [`TcpStream`]s with tokio's `AsyncRead`/`AsyncWrite`,
//! which is all an HTTP server, an HTTP client or a WebSocket needs.
//!
//! ## Sharing the node with a VPN
//!
//! The stack does not have to own every packet. [`Netstack::claims`] says which
//! inbound ones are its own: TCP to this address on a port it listens on, or on
//! one in its ephemeral range ([`Config::ephemeral`]). Anything else is the
//! embedder's to hand on — to a TUN when other apps are allowed on the mesh, or
//! nowhere. The default ephemeral range sits above Linux's (and Android's)
//! `ip_local_port_range` of 32768–60999, so the kernel's own mesh connections
//! never land in it.
//!
//! ## What it is not
//!
//! TCP only, over IPv6 only — the only thing FIPS's IPv6 shim carries that
//! an app's own services use. No UDP, no ICMP (an ICMPv6 error from the node is
//! not claimed, so a dial to an unreachable peer ends at its timeout rather
//! than at once). TIME-WAIT is not kept: a closed socket is freed as soon as
//! its close completes, as the ephemeral ports rotate through a range of
//! several thousand before one is reused.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::poll_fn;
use std::io;
use std::net::{Ipv6Addr, SocketAddrV6};
use std::ops::RangeInclusive;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use smoltcp::iface::{Config as IfaceConfig, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::time::Instant as SmolInstant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, IpEndpoint};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::Notify;

/// Where the stack's outbound packets go: in an embedded node, its app-owned
/// TUN's outbound channel. Called with the stack's lock released; must not
/// block. A packet it cannot take is dropped, and TCP sends it again.
pub type Egress = Box<dyn Fn(Vec<u8>) + Send + Sync>;

/// How long a socket whose stream was dropped may take to finish closing
/// before it is reset. A peer that vanished never acknowledges the FIN.
const CLOSE_LINGER: Duration = Duration::from_secs(30);

/// Longest the driver sleeps without looking at the sockets, whatever
/// smoltcp's next timer says.
const MAX_IDLE: Duration = Duration::from_secs(1);

/// The stack's shape. [`Config::new`] fills in the defaults.
#[derive(Clone, Debug)]
pub struct Config {
    /// This node's mesh address: the stack's only address.
    pub addr: Ipv6Addr,
    /// The largest IPv6 packet the node carries — FIPS's effective IPv6 MTU.
    /// TCP's MSS follows from it, so no segment needs clamping.
    pub mtu: usize,
    /// Local ports outgoing connections are given.
    pub ephemeral: RangeInclusive<u16>,
    /// Receive and send buffer per connection, each, in bytes.
    pub buffer: usize,
    /// Most sockets at once — listening, connecting, open and closing alike.
    pub max_sockets: usize,
    /// Handshakes a listener can have in flight at once.
    pub backlog: usize,
    /// Inbound packets waiting for the driver; more are dropped.
    pub rx_queue: usize,
}

impl Config {
    /// The defaults, for a node at `addr` carrying `mtu`-byte packets.
    pub fn new(addr: Ipv6Addr, mtu: usize) -> Self {
        Self {
            addr,
            mtu,
            ephemeral: 61000..=65535,
            buffer: 64 * 1024,
            max_sockets: 256,
            backlog: 8,
            rx_queue: 1024,
        }
    }
}

/// A userspace TCP/IP stack on one mesh address. Cheap to clone; every clone
/// is the same stack, and it stops when the last clone is dropped (open
/// streams then fail).
#[derive(Clone)]
pub struct Netstack {
    shared: Arc<Shared>,
    _owner: Arc<Owner>,
}

impl std::fmt::Debug for Netstack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Netstack")
            .field("addr", &self.shared.addr)
            .finish_non_exhaustive()
    }
}

struct Shared {
    addr: Ipv6Addr,
    ephemeral: RangeInclusive<u16>,
    /// The ports listeners hold, readable without the stack's lock — it is
    /// asked once per inbound packet.
    listening: RwLock<HashSet<u16>>,
    state: Mutex<State>,
    /// Wakes the driver: a packet arrived, or a socket has something to send.
    wake: Notify,
    egress: Egress,
    /// Inbound packets dropped because the queue was full.
    dropped: AtomicUsize,
}

struct State {
    iface: Interface,
    device: QueueDevice,
    sockets: SocketSet<'static>,
    count: usize,
    listeners: HashMap<u16, Listening>,
    /// The ephemeral port each outgoing socket holds.
    ports: HashMap<SocketHandle, u16>,
    used_ports: HashSet<u16>,
    next_port: u16,
    /// Sockets whose stream was dropped, finishing their close.
    closing: Vec<(SocketHandle, std::time::Instant)>,
    buffer: usize,
    max_sockets: usize,
    backlog: usize,
    rx_queue: usize,
    shut: bool,
}

#[derive(Default)]
struct Listening {
    /// Sockets listening on the port, or mid-handshake.
    backlog: Vec<SocketHandle>,
    /// Connections that completed their handshake, waiting for `accept`.
    ready: VecDeque<SocketHandle>,
    waker: Option<Waker>,
}

/// Stops the driver when the last [`Netstack`] clone goes.
struct Owner(Weak<Shared>);

impl Drop for Owner {
    fn drop(&mut self) {
        if let Some(shared) = self.0.upgrade() {
            shared.lock().shut = true;
            shared.wake.notify_one();
        }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic under the lock leaves smoltcp's state as consistent as
        // smoltcp left it; carrying on beats taking the whole mesh down.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Netstack {
    /// Start a stack on `handle`'s runtime, sending its packets to `egress`.
    pub fn start(config: Config, egress: Egress, handle: &tokio::runtime::Handle) -> Self {
        let mut device = QueueDevice {
            rx: VecDeque::new(),
            tx: Vec::new(),
            mtu: config.mtu,
        };
        let mut iface_config = IfaceConfig::new(HardwareAddress::Ip);
        iface_config.random_seed = seed(&config.addr);
        let mut iface = Interface::new(iface_config, &mut device, SmolInstant::now());
        // The whole of fd00::/8 is on-link: every mesh address is one hop
        // away as far as IP can tell, and FIPS does the routing.
        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::new(IpAddress::Ipv6(config.addr), 8));
        });
        let start = *config.ephemeral.start();
        let shared = Arc::new(Shared {
            addr: config.addr,
            ephemeral: config.ephemeral.clone(),
            listening: RwLock::new(HashSet::new()),
            state: Mutex::new(State {
                iface,
                device,
                sockets: SocketSet::new(Vec::new()),
                count: 0,
                listeners: HashMap::new(),
                ports: HashMap::new(),
                used_ports: HashSet::new(),
                next_port: start,
                closing: Vec::new(),
                buffer: config.buffer,
                max_sockets: config.max_sockets,
                backlog: config.backlog.max(1),
                rx_queue: config.rx_queue,
                shut: false,
            }),
            wake: Notify::new(),
            egress,
            dropped: AtomicUsize::new(0),
        });
        handle.spawn(drive(shared.clone()));
        Self {
            _owner: Arc::new(Owner(Arc::downgrade(&shared))),
            shared,
        }
    }

    /// The stack's address.
    pub fn addr(&self) -> Ipv6Addr {
        self.shared.addr
    }

    /// Follow a change in the node's MTU. Connections already open keep the
    /// MSS they agreed.
    pub fn set_mtu(&self, mtu: usize) {
        self.shared.lock().device.mtu = mtu;
    }

    /// Inbound packets dropped so far because the stack was not keeping up.
    pub fn dropped(&self) -> usize {
        self.shared.dropped.load(Ordering::Relaxed)
    }

    /// Whether an inbound IPv6 `packet` is this stack's: TCP, to its address,
    /// on a port it listens on or one in its ephemeral range.
    pub fn claims(&self, packet: &[u8]) -> bool {
        // 40-byte fixed header, then (with no extension headers) TCP's ports.
        if packet.len() < 44 || packet[0] >> 4 != 6 || packet[6] != 6 {
            return false;
        }
        if packet[24..40] != self.shared.addr.octets() {
            return false;
        }
        let port = u16::from_be_bytes([packet[42], packet[43]]);
        self.shared.ephemeral.contains(&port)
            || self
                .shared
                .listening
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&port)
    }

    /// Take an inbound packet if it is [claimed](Self::claims); hand it back if
    /// not, for the caller to route elsewhere.
    pub fn offer(&self, packet: Vec<u8>) -> Result<(), Vec<u8>> {
        if !self.claims(&packet) {
            return Err(packet);
        }
        {
            let mut state = self.shared.lock();
            if state.device.rx.len() >= state.rx_queue {
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
            state.device.rx.push_back(packet);
        }
        self.shared.wake.notify_one();
        Ok(())
    }

    /// Listen on `port`. One listener per port; the ephemeral range is not
    /// for listening.
    pub fn bind(&self, port: u16) -> io::Result<TcpListener> {
        if port == 0 || self.shared.ephemeral.contains(&port) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("port {port} is not one a listener may take"),
            ));
        }
        {
            let mut state = self.shared.lock();
            if state.shut {
                return Err(stopped());
            }
            if state.listeners.contains_key(&port) {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("port {port} already has a listener"),
                ));
            }
            state.listeners.insert(port, Listening::default());
            state.refill(port);
            self.shared
                .listening
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(port);
        }
        self.shared.wake.notify_one();
        Ok(TcpListener {
            shared: self.shared.clone(),
            port,
        })
    }

    /// Open a connection to `remote`. Fails when the peer refuses; a peer that
    /// never answers is left to the caller's timeout.
    pub async fn connect(&self, remote: SocketAddrV6) -> io::Result<TcpStream> {
        let handle = {
            let mut state = self.shared.lock();
            if state.shut {
                return Err(stopped());
            }
            let port = state.alloc_port(&self.shared).ok_or_else(|| {
                io::Error::new(io::ErrorKind::AddrNotAvailable, "no free local port")
            })?;
            let Some(handle) = state.new_socket() else {
                state.used_ports.remove(&port);
                return Err(io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    "too many open connections",
                ));
            };
            state.ports.insert(handle, port);
            let endpoint = IpEndpoint::new(IpAddress::Ipv6(*remote.ip()), remote.port());
            let dialled = {
                let State { iface, sockets, .. } = &mut *state;
                sockets
                    .get_mut::<tcp::Socket>(handle)
                    .connect(iface.context(), endpoint, port)
            };
            if let Err(e) = dialled {
                state.forget(handle);
                return Err(io::Error::new(io::ErrorKind::InvalidInput, e.to_string()));
            }
            handle
        };
        self.shared.wake.notify_one();
        // From here the stream owns the socket: dropping this future mid-dial
        // drops the stream, which closes it.
        let stream = TcpStream {
            shared: self.shared.clone(),
            handle,
        };
        poll_fn(|cx| {
            let mut state = self.shared.lock();
            let socket = state.sockets.get_mut::<tcp::Socket>(handle);
            match socket.state() {
                tcp::State::SynSent | tcp::State::SynReceived => {
                    socket.register_send_waker(cx.waker());
                    Poll::Pending
                }
                tcp::State::Closed | tcp::State::TimeWait => Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    format!("{remote} refused the connection"),
                ))),
                _ => Poll::Ready(Ok(())),
            }
        })
        .await?;
        Ok(stream)
    }

    /// Sockets the stack holds right now, of every kind.
    #[doc(hidden)]
    pub fn socket_count(&self) -> usize {
        self.shared.lock().count
    }
}

fn stopped() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "the mesh stack has stopped")
}

/// A seed that differs per address and per start, for smoltcp's initial
/// sequence numbers and port choices. Need not be cryptographic.
fn seed(addr: &Ipv6Addr) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let bits = u128::from(*addr);
    nanos ^ (bits as u64) ^ ((bits >> 64) as u64).rotate_left(17)
}

impl State {
    fn new_socket(&mut self) -> Option<SocketHandle> {
        if self.count >= self.max_sockets {
            return None;
        }
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; self.buffer]),
            tcp::SocketBuffer::new(vec![0; self.buffer]),
        );
        socket.set_congestion_control(tcp::CongestionControl::Cubic);
        // Request/response traffic (HTTP, WebSocket frames) waits on every
        // small write; Nagle would hold each for an ACK.
        socket.set_nagle_enabled(false);
        self.count += 1;
        Some(self.sockets.add(socket))
    }

    /// Drop a socket and whatever port it held.
    fn forget(&mut self, handle: SocketHandle) {
        self.sockets.remove(handle);
        self.count -= 1;
        if let Some(port) = self.ports.remove(&handle) {
            self.used_ports.remove(&port);
        }
    }

    fn alloc_port(&mut self, shared: &Shared) -> Option<u16> {
        let (start, end) = (*shared.ephemeral.start(), *shared.ephemeral.end());
        for _ in 0..=(end - start) {
            let port = self.next_port;
            self.next_port = if port >= end { start } else { port + 1 };
            if self.used_ports.insert(port) {
                return Some(port);
            }
        }
        None
    }

    /// Top a listener's backlog back up to full with listening sockets.
    fn refill(&mut self, port: u16) {
        let want = self.backlog;
        while self
            .listeners
            .get(&port)
            .is_some_and(|l| l.backlog.len() < want)
        {
            let Some(handle) = self.new_socket() else {
                return;
            };
            if self
                .sockets
                .get_mut::<tcp::Socket>(handle)
                .listen(port)
                .is_err()
            {
                self.forget(handle);
                return;
            }
            if let Some(listening) = self.listeners.get_mut(&port) {
                listening.backlog.push(handle);
            }
        }
    }

    /// Move each listener's finished handshakes to its accept queue.
    fn service_listeners(&mut self) {
        let ports: Vec<u16> = self.listeners.keys().copied().collect();
        for port in ports {
            let mut gone = Vec::new();
            {
                let Self {
                    sockets, listeners, ..
                } = self;
                let Some(listening) = listeners.get_mut(&port) else {
                    continue;
                };
                let mut promoted = false;
                listening.backlog.retain(|&handle| {
                    match sockets.get::<tcp::Socket>(handle).state() {
                        tcp::State::Listen | tcp::State::SynReceived => true,
                        tcp::State::Established | tcp::State::CloseWait => {
                            listening.ready.push_back(handle);
                            promoted = true;
                            false
                        }
                        _ => {
                            gone.push(handle);
                            false
                        }
                    }
                });
                if promoted {
                    if let Some(waker) = listening.waker.take() {
                        waker.wake();
                    }
                }
            }
            for handle in gone {
                self.forget(handle);
            }
            self.refill(port);
        }
    }

    /// Free sockets that finished closing, and reset those taking too long.
    fn reap(&mut self) {
        let now = std::time::Instant::now();
        let mut done = Vec::new();
        let Self {
            sockets, closing, ..
        } = self;
        closing.retain(|&(handle, since)| {
            let socket = sockets.get_mut::<tcp::Socket>(handle);
            match socket.state() {
                tcp::State::Closed | tcp::State::TimeWait => {
                    done.push(handle);
                    false
                }
                _ => {
                    if now.duration_since(since) > CLOSE_LINGER {
                        // Sends a reset on the next poll, then reads Closed.
                        socket.abort();
                    }
                    true
                }
            }
        });
        for handle in done {
            self.forget(handle);
        }
    }

    /// A stream is done with its socket: close it gracefully if it is still
    /// open, and free it once the close completes.
    fn release(&mut self, handle: SocketHandle) {
        let socket = self.sockets.get_mut::<tcp::Socket>(handle);
        match socket.state() {
            tcp::State::Closed | tcp::State::TimeWait => self.forget(handle),
            _ => {
                socket.close();
                self.closing.push((handle, std::time::Instant::now()));
            }
        }
    }
}

/// The driver: poll smoltcp, hand its packets to the egress, sleep until a
/// packet arrives, a socket wants attention, or smoltcp's next timer.
async fn drive(shared: Arc<Shared>) {
    loop {
        let (out, delay) = {
            let mut guard = shared.lock();
            let state = &mut *guard;
            if state.shut {
                break;
            }
            let now = SmolInstant::now();
            state.iface.poll(now, &mut state.device, &mut state.sockets);
            state.service_listeners();
            state.reap();
            let out = std::mem::take(&mut state.device.tx);
            let delay = state.iface.poll_delay(now, &state.sockets);
            (out, delay)
        };
        for packet in out {
            (shared.egress)(packet);
        }
        let wait = delay
            .map(|d| Duration::from_micros(d.total_micros()))
            .unwrap_or(MAX_IDLE)
            .min(MAX_IDLE);
        if wait.is_zero() {
            tokio::task::yield_now().await;
            continue;
        }
        tokio::select! {
            _ = shared.wake.notified() => {}
            _ = tokio::time::sleep(wait) => {}
        }
    }
    // Stopped: wake everything still waiting so it sees the end.
    let mut state = shared.lock();
    let handles: Vec<SocketHandle> = state.sockets.iter().map(|(h, _)| h).collect();
    for handle in handles {
        state.sockets.get_mut::<tcp::Socket>(handle).abort();
    }
    for listening in state.listeners.values_mut() {
        if let Some(waker) = listening.waker.take() {
            waker.wake();
        }
    }
}

/// A listener on one port of the stack. Dropping it stops listening and
/// resets connections it accepted the handshake for but never handed out.
pub struct TcpListener {
    shared: Arc<Shared>,
    port: u16,
}

impl TcpListener {
    /// The port it holds.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The stack's address and this listener's port.
    pub fn local_addr(&self) -> SocketAddrV6 {
        SocketAddrV6::new(self.shared.addr, self.port, 0, 0)
    }

    /// The next connection, and the peer it is from.
    pub async fn accept(&self) -> io::Result<(TcpStream, SocketAddrV6)> {
        poll_fn(|cx| {
            let mut state = self.shared.lock();
            if state.shut {
                return Poll::Ready(Err(stopped()));
            }
            let State {
                sockets, listeners, ..
            } = &mut *state;
            let Some(listening) = listeners.get_mut(&self.port) else {
                return Poll::Ready(Err(stopped()));
            };
            match listening.ready.pop_front() {
                Some(handle) => {
                    let peer = match sockets.get::<tcp::Socket>(handle).remote_endpoint() {
                        Some(IpEndpoint {
                            addr: IpAddress::Ipv6(ip),
                            port,
                        }) => SocketAddrV6::new(ip, port, 0, 0),
                        // Reset between the handshake and now; the stream
                        // reads as closed at once.
                        _ => SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0),
                    };
                    Poll::Ready(Ok((
                        TcpStream {
                            shared: self.shared.clone(),
                            handle,
                        },
                        peer,
                    )))
                }
                None => {
                    listening.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        })
        .await
    }
}

impl std::fmt::Debug for TcpListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpListener")
            .field("addr", &self.local_addr())
            .finish()
    }
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        self.shared
            .listening
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.port);
        let mut state = self.shared.lock();
        if let Some(listening) = state.listeners.remove(&self.port) {
            for handle in listening.backlog.into_iter().chain(listening.ready) {
                let socket = state.sockets.get_mut::<tcp::Socket>(handle);
                if socket.state() == tcp::State::Listen {
                    state.forget(handle);
                } else {
                    socket.abort();
                    state.closing.push((handle, std::time::Instant::now()));
                }
            }
        }
        drop(state);
        self.shared.wake.notify_one();
    }
}

/// One TCP connection on the stack.
///
/// Reading and writing may happen from two tasks at once (as a split
/// WebSocket does): each direction has its own waker. A reset reads as end of
/// stream — smoltcp does not tell the two apart.
pub struct TcpStream {
    shared: Arc<Shared>,
    handle: SocketHandle,
}

impl TcpStream {
    /// The peer's address.
    pub fn peer_addr(&self) -> Option<SocketAddrV6> {
        let state = self.shared.lock();
        match state
            .sockets
            .get::<tcp::Socket>(self.handle)
            .remote_endpoint()
        {
            Some(IpEndpoint {
                addr: IpAddress::Ipv6(ip),
                port,
            }) => Some(SocketAddrV6::new(ip, port, 0, 0)),
            _ => None,
        }
    }
}

impl std::fmt::Debug for TcpStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpStream")
            .field("peer", &self.peer_addr())
            .finish()
    }
}

impl AsyncRead for TcpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let mut state = self.shared.lock();
        if state.shut {
            return Poll::Ready(Err(stopped()));
        }
        let socket = state.sockets.get_mut::<tcp::Socket>(self.handle);
        if socket.can_recv() {
            let read = socket.recv_slice(buf.initialize_unfilled());
            drop(state);
            return match read {
                Ok(n) => {
                    buf.advance(n);
                    // The window opened: let the driver tell the peer.
                    self.shared.wake.notify_one();
                    Poll::Ready(Ok(()))
                }
                Err(e) => Poll::Ready(Err(io::Error::other(e.to_string()))),
            };
        }
        if !socket.may_recv() {
            return Poll::Ready(Ok(()));
        }
        socket.register_recv_waker(cx.waker());
        Poll::Pending
    }
}

impl AsyncWrite for TcpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut state = self.shared.lock();
        if state.shut {
            return Poll::Ready(Err(stopped()));
        }
        let socket = state.sockets.get_mut::<tcp::Socket>(self.handle);
        if !socket.may_send() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "connection closed",
            )));
        }
        if !socket.can_send() {
            socket.register_send_waker(cx.waker());
            return Poll::Pending;
        }
        let sent = socket.send_slice(data);
        drop(state);
        match sent {
            Ok(n) => {
                self.shared.wake.notify_one();
                Poll::Ready(Ok(n))
            }
            Err(e) => Poll::Ready(Err(io::Error::other(e.to_string()))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Written bytes are the driver's to send as soon as it is woken, which
        // every write does.
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.shared
            .lock()
            .sockets
            .get_mut::<tcp::Socket>(self.handle)
            .close();
        self.shared.wake.notify_one();
        Poll::Ready(Ok(()))
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        self.shared.lock().release(self.handle);
        self.shared.wake.notify_one();
    }
}

/// The interface's "device": two queues. Inbound packets wait in `rx` for
/// the driver's poll; outbound ones collect in `tx` until it hands them on.
struct QueueDevice {
    rx: VecDeque<Vec<u8>>,
    tx: Vec<Vec<u8>>,
    mtu: usize,
}

impl Device for QueueDevice {
    type RxToken<'a> = Rx;
    type TxToken<'a> = Tx<'a>;

    fn receive(&mut self, _now: SmolInstant) -> Option<(Rx, Tx<'_>)> {
        let packet = self.rx.pop_front()?;
        Some((Rx(packet), Tx(&mut self.tx)))
    }

    fn transmit(&mut self, _now: SmolInstant) -> Option<Tx<'_>> {
        Some(Tx(&mut self.tx))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = self.mtu;
        caps
    }
}

struct Rx(Vec<u8>);

impl RxToken for Rx {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.0)
    }
}

struct Tx<'a>(&'a mut Vec<Vec<u8>>);

impl TxToken for Tx<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut packet = vec![0; len];
        let result = f(&mut packet);
        self.0.push(packet);
        result
    }
}

#[cfg(test)]
mod tests;
