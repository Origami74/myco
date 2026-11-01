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
//! ## Holding up against peers
//!
//! Any node on the mesh can send this stack packets, so nothing a peer does
//! may hold a socket for good, or take the sockets everyone else needs:
//!
//! - **Every socket has a lifetime.** A handshake that stalls is dropped after
//!   [`Config::handshake_timeout`], a dial after [`Config::connect_timeout`],
//!   and an open connection whose peer stops answering its keep-alives after
//!   [`Config::dead_peer_timeout`].
//! - **One peer gets a share, not the lot.** A peer holding
//!   [`Config::per_peer`] sockets has its next connection attempt dropped.
//! - **Dials keep a reserve.** Listeners never take the last
//!   [`Config::dial_reserve`] sockets, so a flood of inbound connections
//!   cannot stop this node reaching out.
//!
//! ## What it is not
//!
//! TCP only, over IPv6 only — the only thing FIPS's IPv6 shim carries that an
//! app's own services use. Of ICMPv6 it reads only "destination unreachable"
//! for its own dials, which ends the dial at once
//! ([`io::ErrorKind::HostUnreachable`]). No path-MTU discovery: the stack sends
//! at most [`IPV6_MIN_MTU`]-byte packets, which FIPS carries over any path
//! (fragmenting where a path is narrower). TIME-WAIT is not kept: a closed
//! socket is freed as soon as its close completes, as the ephemeral ports
//! rotate through a range of several thousand before one is reused.

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

/// IPv6's minimum link MTU: the largest packet FIPS carries over every path.
pub const IPV6_MIN_MTU: usize = 1280;

/// The stack's shape. [`Config::new`] fills in the defaults.
#[derive(Clone, Debug)]
pub struct Config {
    /// This node's mesh address: the stack's only address.
    pub addr: Ipv6Addr,
    /// The largest IPv6 packet the stack sends. TCP's MSS follows from it, so
    /// no segment needs clamping. Fixed for the stack's life: smoltcp reads it
    /// once. [`IPV6_MIN_MTU`] by default, which FIPS carries over any path.
    pub mtu: usize,
    /// Local ports outgoing connections are given. The default starts above
    /// 61000, the top of the kernel's range on older Linux too.
    pub ephemeral: RangeInclusive<u16>,
    /// Receive and send buffer per connection, each, in bytes.
    pub buffer: usize,
    /// Most sockets at once — listening, connecting, open and closing alike.
    pub max_sockets: usize,
    /// Of [`Self::max_sockets`], how many listeners may never take: kept for
    /// this node's own dials.
    pub dial_reserve: usize,
    /// Most sockets one remote address may hold at once, in any state. A
    /// connection attempt past it is dropped.
    pub per_peer: usize,
    /// Handshakes a listener can have in flight at once.
    pub backlog: usize,
    /// Inbound packets waiting for the driver; more are dropped.
    pub rx_queue: usize,
    /// How long an inbound handshake may stall before its socket is dropped
    /// and a fresh one listens in its place.
    pub handshake_timeout: Duration,
    /// How long a dial waits for the peer to answer.
    pub connect_timeout: Duration,
    /// How often an idle open connection checks that its peer is there.
    pub keep_alive: Duration,
    /// How long an open connection's peer may go without a sound — keep-alive
    /// answers included — before the connection is reset.
    pub dead_peer_timeout: Duration,
}

impl Config {
    /// The defaults, for a node at `addr`.
    pub fn new(addr: Ipv6Addr) -> Self {
        Self {
            addr,
            mtu: IPV6_MIN_MTU,
            ephemeral: 61001..=65535,
            buffer: 64 * 1024,
            max_sockets: 256,
            dial_reserve: 64,
            per_peer: 32,
            backlog: 8,
            rx_queue: 1024,
            handshake_timeout: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(15),
            keep_alive: Duration::from_secs(30),
            dead_peer_timeout: Duration::from_secs(120),
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
    /// Dials ended by an ICMPv6 "destination unreachable".
    unreachable: HashSet<SocketHandle>,
    limits: Limits,
    shut: bool,
}

/// The parts of [`Config`] the stack consults as it runs.
#[derive(Clone, Copy)]
struct Limits {
    buffer: usize,
    max_sockets: usize,
    dial_reserve: usize,
    per_peer: usize,
    backlog: usize,
    rx_queue: usize,
    handshake_timeout: smoltcp::time::Duration,
    connect_timeout: Duration,
    keep_alive: smoltcp::time::Duration,
    dead_peer_timeout: smoltcp::time::Duration,
}

fn smol(d: Duration) -> smoltcp::time::Duration {
    smoltcp::time::Duration::from_micros(d.as_micros() as u64)
}

/// The fields of a TCP segment `offer` looks at, from a packet `claims` passed.
fn tcp_syn_from(packet: &[u8]) -> Option<Ipv6Addr> {
    // Flags at TCP offset 13: SYN set, ACK clear — a new connection attempt.
    let flags = *packet.get(40 + 13)?;
    if packet[6] == 6 && flags & 0x02 != 0 && flags & 0x10 == 0 {
        let src: [u8; 16] = packet[8..24].try_into().ok()?;
        Some(Ipv6Addr::from(src))
    } else {
        None
    }
}

/// For an ICMPv6 "destination unreachable" about one of `ours` TCP segments:
/// the dial it refers to, as (local port, remote address).
fn unreachable_dial(packet: &[u8], ours: &Ipv6Addr) -> Option<(u16, Ipv6Addr)> {
    // ICMPv6 header (8 bytes), then the offending packet: its IPv6 header
    // (40) and at least the TCP ports (4).
    const INNER: usize = 40 + 8;
    if packet.len() < INNER + 44 || packet[6] != 58 || packet[40] != 1 {
        return None;
    }
    let inner = &packet[INNER..];
    if inner[0] >> 4 != 6 || inner[6] != 6 || inner[8..24] != ours.octets() {
        return None;
    }
    let dst: [u8; 16] = inner[24..40].try_into().ok()?;
    let port = u16::from_be_bytes([inner[40], inner[41]]);
    Some((port, Ipv6Addr::from(dst)))
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
        let limits = Limits {
            buffer: config.buffer,
            max_sockets: config.max_sockets,
            dial_reserve: config.dial_reserve.min(config.max_sockets),
            per_peer: config.per_peer.max(1),
            backlog: config.backlog.max(1),
            rx_queue: config.rx_queue,
            handshake_timeout: smol(config.handshake_timeout),
            connect_timeout: config.connect_timeout,
            keep_alive: smol(config.keep_alive),
            dead_peer_timeout: smol(config.dead_peer_timeout),
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
                unreachable: HashSet::new(),
                limits,
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

    /// Inbound packets dropped so far because the stack was not keeping up.
    pub fn dropped(&self) -> usize {
        self.shared.dropped.load(Ordering::Relaxed)
    }

    /// Whether an inbound IPv6 `packet` is this stack's: TCP, to its address,
    /// on a port it listens on or one in its ephemeral range — or an ICMPv6
    /// "destination unreachable" about one of its own dials.
    pub fn claims(&self, packet: &[u8]) -> bool {
        if packet.len() < 44 || packet[0] >> 4 != 6 {
            return false;
        }
        if packet[24..40] != self.shared.addr.octets() {
            return false;
        }
        if let Some((port, _)) = unreachable_dial(packet, &self.shared.addr) {
            return self.shared.ephemeral.contains(&port);
        }
        // 40-byte fixed header, then (with no extension headers) TCP's ports.
        if packet[6] != 6 {
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
            if let Some((port, remote)) = unreachable_dial(&packet, &self.shared.addr) {
                state.abort_dial(port, remote);
                drop(state);
                self.shared.wake.notify_one();
                return Ok(());
            }
            if let Some(src) = tcp_syn_from(&packet) {
                if state.held_by(src) >= state.limits.per_peer {
                    self.shared.dropped.fetch_add(1, Ordering::Relaxed);
                    return Ok(());
                }
            }
            if state.device.rx.len() >= state.limits.rx_queue {
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

    /// Open a connection to `remote`. Fails when the peer refuses
    /// ([`io::ErrorKind::ConnectionRefused`]), when the mesh has no route to it
    /// ([`io::ErrorKind::HostUnreachable`]), or when it does not answer within
    /// [`Config::connect_timeout`] ([`io::ErrorKind::TimedOut`]).
    pub async fn connect(&self, remote: SocketAddrV6) -> io::Result<TcpStream> {
        let handle = {
            let mut state = self.shared.lock();
            if state.shut {
                return Err(stopped());
            }
            let port = state.alloc_port(&self.shared).ok_or_else(|| {
                io::Error::new(io::ErrorKind::AddrNotAvailable, "no free local port")
            })?;
            let cap = state.limits.max_sockets;
            let Some(handle) = state.new_socket(cap) else {
                state.used_ports.remove(&port);
                return Err(io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    "too many open connections",
                ));
            };
            state.ports.insert(handle, port);
            let endpoint = IpEndpoint::new(IpAddress::Ipv6(*remote.ip()), remote.port());
            // Twice the dial's own timeout, so that one decides — and reports
            // `TimedOut` — whenever this future is polled to it.
            let backstop = smol(state.limits.connect_timeout * 2);
            let dialled = {
                let State { iface, sockets, .. } = &mut *state;
                let socket = sockets.get_mut::<tcp::Socket>(handle);
                // The socket gives up on its own too, should this future
                // stop being polled.
                socket.set_timeout(Some(backstop));
                socket.connect(iface.context(), endpoint, port)
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
        let timeout = self.shared.lock().limits.connect_timeout;
        let established = poll_fn(|cx| {
            let mut state = self.shared.lock();
            let limits = state.limits;
            if state.unreachable.remove(&handle) {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::HostUnreachable,
                    format!("{remote}: no route to host"),
                )));
            }
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
                _ => {
                    live(socket, &limits);
                    Poll::Ready(Ok(()))
                }
            }
        });
        match tokio::time::timeout(timeout, established).await {
            Ok(result) => result.map(|()| stream),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{remote} did not answer"),
            )),
        }
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

/// An open connection's timers: keep-alives, and a reset once the peer has
/// been silent too long.
fn live(socket: &mut tcp::Socket, limits: &Limits) {
    socket.set_keep_alive(Some(limits.keep_alive));
    socket.set_timeout(Some(limits.dead_peer_timeout));
}

impl State {
    /// A fresh socket, if fewer than `cap` are held.
    fn new_socket(&mut self, cap: usize) -> Option<SocketHandle> {
        if self.count >= cap {
            return None;
        }
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; self.limits.buffer]),
            tcp::SocketBuffer::new(vec![0; self.limits.buffer]),
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
        self.unreachable.remove(&handle);
        if let Some(port) = self.ports.remove(&handle) {
            self.used_ports.remove(&port);
        }
    }

    /// Sockets `peer` holds, in any state — counting its connection attempts
    /// still queued for the driver, which will each take one.
    fn held_by(&self, peer: Ipv6Addr) -> usize {
        let queued = self
            .device
            .rx
            .iter()
            .filter(|p| tcp_syn_from(p) == Some(peer))
            .count();
        queued
            + self
                .sockets
                .iter()
                .filter_map(|(_, s)| match s {
                    smoltcp::socket::Socket::Tcp(t) => t.remote_endpoint(),
                    #[allow(unreachable_patterns)] // only TCP sockets are ever added
                    _ => None,
                })
                .filter(|e| e.addr == IpAddress::Ipv6(peer))
                .count()
    }

    /// End the dial from `port` to `remote`, which the mesh says it cannot
    /// reach. Only a dial still waiting for its answer: an open connection
    /// rides out a route that comes and goes, as a kernel's would.
    fn abort_dial(&mut self, port: u16, remote: Ipv6Addr) {
        let found = self.ports.iter().find_map(|(&handle, &p)| {
            let socket = self.sockets.get::<tcp::Socket>(handle);
            (p == port
                && socket.state() == tcp::State::SynSent
                && socket.remote_endpoint().map(|e| e.addr) == Some(IpAddress::Ipv6(remote)))
            .then_some(handle)
        });
        if let Some(handle) = found {
            self.unreachable.insert(handle);
            self.sockets.get_mut::<tcp::Socket>(handle).abort();
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

    /// Top a listener's backlog back up to full with listening sockets,
    /// leaving [`Limits::dial_reserve`] untouched.
    fn refill(&mut self, port: u16) {
        let want = self.limits.backlog;
        let cap = self.limits.max_sockets - self.limits.dial_reserve;
        while self
            .listeners
            .get(&port)
            .is_some_and(|l| l.backlog.len() < want)
        {
            let Some(handle) = self.new_socket(cap) else {
                return;
            };
            let handshake = self.limits.handshake_timeout;
            let socket = self.sockets.get_mut::<tcp::Socket>(handle);
            // Counts only once a SYN has arrived: a stalled handshake then
            // closes, and is replaced, instead of holding the slot.
            socket.set_timeout(Some(handshake));
            if socket.listen(port).is_err() {
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
        let limits = self.limits;
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
                    let socket = sockets.get_mut::<tcp::Socket>(handle);
                    match socket.state() {
                        tcp::State::Listen | tcp::State::SynReceived => true,
                        tcp::State::Established | tcp::State::CloseWait => {
                            // Off the handshake timer, onto keep-alives.
                            live(socket, &limits);
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
