//! **Myco's own sockets on the mesh**, with no VPN.
//!
//! The node's packets used to reach Myco's servers and clients through the
//! kernel: the `VpnService` handed them to a TUN, the kernel's TCP terminated
//! them, and ordinary sockets on `[::]:4870` or to `[fd00::peer]:24243` did
//! the rest. That held the user's one VPN slot for traffic that never leaves
//! the app. Now a userspace TCP/IP stack
//! ([`fips_host::netstack::Netstack`]) on the node's own address terminates
//! them in process, and the VPN is only for letting *other* apps on the mesh
//! (see `tun_bridge`).
//!
//! This module is the one seam between that stack and Myco:
//!
//! - [`init`] starts it, once per process, on the node's address.
//! - [`listen`] gives a server a [`MeshListener`] beside its kernel one.
//! - [`dial`] opens a TCP connection to a mesh address — on the stack when
//!   there is one, through the kernel otherwise (the host, where tests and
//!   desktop builds have a system TUN or none at all).
//! - [`proxy`] points an HTTP client at a loopback SOCKS5 bridge into
//!   [`dial`], because reqwest offers no way to hand it a socket of our own.
//!   The bridge is guarded by a per-process secret, so no other app on the
//!   phone can use it to reach the mesh as this device.
//!
//! Keeping every dial and listen behind this module is also what lets a
//! FIPS node run by another app replace the embedded one later: only this
//! file would change.

use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};
use std::time::Duration;

use fips_host::netstack::{Config, Netstack, TcpListener, TcpStream};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

static STACK: OnceLock<Netstack> = OnceLock::new();

/// The runtime the stack runs on, which the SOCKS bridge shares.
static RUNTIME: OnceLock<tokio::runtime::Handle> = OnceLock::new();

/// Start the mesh stack on `addr`, the node's mesh address, once per process.
/// Its packets go to whichever node is installed in `tun_bridge` at the time,
/// so it outlives a node rebuilt on a BLE off-and-on. It sends packets of at
/// most 1280 bytes, which FIPS carries over any path whatever the node's MTU,
/// so it needs nothing from the node beyond the address.
pub(crate) fn init(handle: &tokio::runtime::Handle, addr: Ipv6Addr) {
    let _ = RUNTIME.set(handle.clone());
    if STACK.get().is_some() {
        return;
    }
    let stack = Netstack::start(
        Config::new(addr),
        Box::new(|packet| {
            crate::tun_bridge::to_mesh(packet);
        }),
        handle,
    );
    if STACK.set(stack).is_ok() {
        tracing::info!(%addr, "mesh stack started");
    }
}

/// The mesh stack, when this process runs one.
pub(crate) fn stack() -> Option<&'static Netstack> {
    STACK.get()
}

/// A listener on the mesh stack's `port`, or `None` with no stack.
pub(crate) fn listen(port: u16) -> Option<io::Result<MeshListener>> {
    STACK.get().map(|stack| stack.bind(port).map(MeshListener))
}

/// A server's listener on the mesh stack. Hands axum each peer's mesh
/// address, as a kernel listener would, so the Circle gate reads the same.
pub(crate) struct MeshListener(TcpListener);

impl axum::serve::Listener for MeshListener {
    type Io = TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.0.accept().await {
                Ok((stream, peer)) => return (stream, SocketAddr::V6(peer)),
                // Only a stopped stack refuses, and it stops with the process;
                // don't spin on it.
                Err(e) => {
                    tracing::warn!(port = self.0.port(), error = %e, "mesh accept failed");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(SocketAddr::V6(self.0.local_addr()))
    }
}

/// Whether `ip` is a mesh address (`fd00::/8`).
fn is_mesh(ip: &IpAddr) -> bool {
    matches!(ip, IpAddr::V6(v6) if v6.octets()[0] == 0xfd)
}

/// A TCP connection to `addr`: on the mesh stack for a mesh address when
/// there is one, through the kernel for anything else.
pub(crate) async fn dial(addr: SocketAddr) -> io::Result<MeshStream> {
    match (addr, STACK.get()) {
        (SocketAddr::V6(v6), Some(stack)) if is_mesh(&addr.ip()) => {
            Ok(MeshStream::Stack(stack.connect(v6).await?))
        }
        _ => {
            let stream = tokio::net::TcpStream::connect(addr).await?;
            let _ = stream.set_nodelay(true);
            Ok(MeshStream::Kernel(stream))
        }
    }
}

/// A WebSocket to `url`, whose host must be an address literal (a mesh name
/// is resolved before it gets here; see `dns_intercept::mesh_literal_url`).
// The same error `tokio_tungstenite::connect_async` returns, so callers that
// classify its faults (`peer_relay`) need not change.
#[allow(clippy::result_large_err)]
pub(crate) async fn connect_ws(
    url: &str,
) -> Result<tokio_tungstenite::WebSocketStream<MeshStream>, tokio_tungstenite::tungstenite::Error> {
    use tokio_tungstenite::tungstenite::{error::UrlError, Error};
    let unusable = || Error::Url(UrlError::UnableToConnect(url.to_string()));
    let parsed = reqwest::Url::parse(url).map_err(|_| unusable())?;
    let ip: IpAddr = parsed
        .host_str()
        .map(|h| h.trim_start_matches('[').trim_end_matches(']'))
        .and_then(|h| h.parse().ok())
        .ok_or_else(unusable)?;
    let port = parsed.port_or_known_default().ok_or_else(unusable)?;
    let addr = SocketAddr::new(ip, port);
    let stream = dial(addr).await.map_err(Error::Io)?;
    let (ws, _) = tokio_tungstenite::client_async(url, stream).await?;
    Ok(ws)
}

/// A connection from [`dial`]: on the mesh stack, or a kernel socket.
pub(crate) enum MeshStream {
    Stack(TcpStream),
    Kernel(tokio::net::TcpStream),
}

impl AsyncRead for MeshStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Stack(s) => Pin::new(s).poll_read(cx, buf),
            Self::Kernel(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MeshStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Stack(s) => Pin::new(s).poll_write(cx, data),
            Self::Kernel(s) => Pin::new(s).poll_write(cx, data),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Stack(s) => Pin::new(s).poll_flush(cx),
            Self::Kernel(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Stack(s) => Pin::new(s).poll_shutdown(cx),
            Self::Kernel(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

// ── The SOCKS5 bridge ───────────────────────────────────────────────────────

/// Where the bridge listens and the credentials it asks for.
struct Bridge {
    port: u16,
    user: String,
    pass: String,
}

static BRIDGE: OnceLock<Option<Bridge>> = OnceLock::new();

/// How long a client has to finish the SOCKS handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the bridge waits on a dial — the stack's own connect timeout
/// bounds it too; this covers the kernel path.
const DIAL_TIMEOUT: Duration = Duration::from_secs(20);

/// A proxy that carries an HTTP client's mesh connections onto the mesh
/// stack, or `None` when there is no stack to carry them to (the kernel
/// reaches the mesh itself then, if it can). Always on in tests, so the
/// bridge is exercised by every mesh-client test.
pub(crate) fn proxy() -> Option<reqwest::Proxy> {
    if STACK.get().is_none() && !cfg!(test) {
        return None;
    }
    let bridge = BRIDGE.get_or_init(start_bridge).as_ref()?;
    // `socks5`, not `socks5h`: the client resolves the name itself, through
    // its guarded resolver (`http`), and hands the bridge an address.
    reqwest::Proxy::all(format!("socks5://127.0.0.1:{}", bridge.port))
        .map(|p| p.basic_auth(&bridge.user, &bridge.pass))
        .inspect_err(|e| tracing::warn!(error = %e, "mesh proxy unusable"))
        .ok()
}

/// A random hex secret: the CSPRNG the file transfer uses.
fn secret() -> String {
    use chacha20poly1305::aead::{KeyInit, OsRng};
    hex::encode(chacha20poly1305::XChaCha20Poly1305::generate_key(
        &mut OsRng,
    ))
}

/// Bind the bridge on loopback and serve it on the stack's runtime — or, in a
/// process with no stack (the host, tests), on a thread of its own, so it
/// needs no runtime from whoever builds the first mesh client.
fn start_bridge() -> Option<Bridge> {
    let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(error = %e, "mesh proxy could not bind; mesh HTTP is refused");
            return None;
        }
    };
    let port = listener.local_addr().ok()?.port();
    let (user, pass) = (secret(), secret());
    let creds = (user.clone(), pass.clone());
    if let Some(handle) = RUNTIME.get() {
        handle.spawn(serve_bridge(listener, creds));
        return Some(Bridge { port, user, pass });
    }
    let spawned = std::thread::Builder::new()
        .name("myco-mesh-proxy".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::warn!(error = %e, "mesh proxy runtime failed");
                    return;
                }
            };
            rt.block_on(serve_bridge(listener, creds));
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "mesh proxy thread failed");
        return None;
    }
    Some(Bridge { port, user, pass })
}

async fn serve_bridge(listener: std::net::TcpListener, creds: (String, String)) {
    let listener = match listener
        .set_nonblocking(true)
        .and_then(|()| tokio::net::TcpListener::from_std(listener))
    {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(error = %e, "mesh proxy listener failed");
            return;
        }
    };
    let creds = std::sync::Arc::new(creds);
    loop {
        let Ok((client, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let creds = creds.clone();
        tokio::spawn(async move {
            if let Err(e) = relay(client, &creds).await {
                tracing::debug!(error = %e, "mesh proxy connection ended");
            }
        });
    }
}

/// SOCKS5 reply codes (RFC 1928 §6).
const REP_OK: u8 = 0x00;
const REP_FAILURE: u8 = 0x01;
const REP_NOT_ALLOWED: u8 = 0x02;
const REP_UNREACHABLE: u8 = 0x04;
const REP_REFUSED: u8 = 0x05;
const REP_COMMAND: u8 = 0x07;
const REP_ADDRESS_TYPE: u8 = 0x08;

/// One SOCKS5 client: username/password auth (RFC 1929), a `CONNECT` to an
/// address literal, then bytes both ways.
async fn relay(mut client: tokio::net::TcpStream, creds: &(String, String)) -> io::Result<()> {
    let target = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(&mut client, creds))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "SOCKS handshake"))??;
    let Some(target) = target else {
        return Ok(());
    };
    // The client gives up on its own timeout by closing its socket; stop
    // dialling then, rather than hold a socket for a fetch nobody awaits.
    let dialled = tokio::select! {
        dialled = tokio::time::timeout(DIAL_TIMEOUT, dial(target)) => dialled
            .unwrap_or_else(|_| Err(io::Error::new(io::ErrorKind::TimedOut, "dial timed out"))),
        () = hung_up(&client) => {
            return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "client left mid-dial"));
        }
    };
    let mut upstream = match dialled {
        Ok(s) => s,
        Err(e) => {
            let rep = match e.kind() {
                io::ErrorKind::ConnectionRefused => REP_REFUSED,
                io::ErrorKind::NetworkUnreachable
                | io::ErrorKind::HostUnreachable
                | io::ErrorKind::TimedOut => REP_UNREACHABLE,
                _ => REP_FAILURE,
            };
            reply(&mut client, rep).await?;
            return Err(e);
        }
    };
    reply(&mut client, REP_OK).await?;
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

/// Resolves once the client has closed its end. It sends nothing between its
/// `CONNECT` and our reply, so anything readable then is the close.
async fn hung_up(client: &tokio::net::TcpStream) {
    let mut probe = [0u8; 1];
    loop {
        match client.peek(&mut probe).await {
            Ok(0) | Err(_) => return,
            // Early bytes are not a close: wait out the dial.
            Ok(_) => std::future::pending::<()>().await,
        }
    }
}

/// The SOCKS handshake, ending at the target to dial — or `None` once a
/// refusal has been sent.
async fn handshake(
    client: &mut tokio::net::TcpStream,
    (user, pass): &(String, String),
) -> io::Result<Option<SocketAddr>> {
    // Greeting: version, the methods offered. Only username/password (0x02)
    // is accepted.
    let mut head = [0u8; 2];
    client.read_exact(&mut head).await?;
    if head[0] != 5 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not SOCKS5"));
    }
    let mut methods = vec![0u8; head[1] as usize];
    client.read_exact(&mut methods).await?;
    if !methods.contains(&0x02) {
        client.write_all(&[5, 0xff]).await?;
        return Ok(None);
    }
    client.write_all(&[5, 0x02]).await?;

    // Username/password.
    let mut ver = [0u8; 2];
    client.read_exact(&mut ver).await?;
    let mut got_user = vec![0u8; ver[1] as usize];
    client.read_exact(&mut got_user).await?;
    let mut plen = [0u8; 1];
    client.read_exact(&mut plen).await?;
    let mut got_pass = vec![0u8; plen[0] as usize];
    client.read_exact(&mut got_pass).await?;
    if ver[0] != 1 || got_user != user.as_bytes() || got_pass != pass.as_bytes() {
        client.write_all(&[1, 1]).await?;
        return Ok(None);
    }
    client.write_all(&[1, 0]).await?;

    // The request: version, command, reserved, address type.
    let mut req = [0u8; 4];
    client.read_exact(&mut req).await?;
    if req[1] != 0x01 {
        reply(client, REP_COMMAND).await?;
        return Ok(None);
    }
    let ip: IpAddr = match req[3] {
        0x01 => {
            let mut a = [0u8; 4];
            client.read_exact(&mut a).await?;
            IpAddr::from(a)
        }
        0x04 => {
            let mut a = [0u8; 16];
            client.read_exact(&mut a).await?;
            IpAddr::from(a)
        }
        // A "name" that is really an address literal: hyper-util before
        // 0.1.21 sends an IPv6 destination this way, brackets and all. A real
        // name would be resolved here, out of the client's guard's sight, so
        // anything else is refused.
        0x03 => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            client.read_exact(&mut name).await?;
            let literal = std::str::from_utf8(&name)
                .ok()
                .map(|n| n.trim_start_matches('[').trim_end_matches(']'))
                .and_then(|n| n.parse::<IpAddr>().ok());
            match literal {
                Some(ip) => ip,
                None => {
                    reply(client, REP_ADDRESS_TYPE).await?;
                    return Ok(None);
                }
            }
        }
        _ => {
            reply(client, REP_ADDRESS_TYPE).await?;
            return Ok(None);
        }
    };
    let mut port = [0u8; 2];
    client.read_exact(&mut port).await?;
    // The mesh only. Loopback too in tests, whose servers live there.
    if !(is_mesh(&ip) || (cfg!(test) && ip.is_loopback())) {
        reply(client, REP_NOT_ALLOWED).await?;
        return Ok(None);
    }
    Ok(Some(SocketAddr::new(ip, u16::from_be_bytes(port))))
}

async fn reply(client: &mut tokio::net::TcpStream, rep: u8) -> io::Result<()> {
    client.write_all(&[5, rep, 0, 0x01, 0, 0, 0, 0, 0, 0]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_fd00_is_mesh() {
        assert!(is_mesh(&"fd12::1".parse().unwrap()));
        assert!(!is_mesh(&"fe80::1".parse().unwrap()));
        assert!(!is_mesh(&"2001:db8::1".parse().unwrap()));
        assert!(!is_mesh(&"10.0.0.1".parse().unwrap()));
    }

    async fn echo_server() -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (mut r, mut w) = s.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });
        addr
    }

    /// Speak SOCKS5 to the bridge by hand, as `user`/`pass`, asking for
    /// `target`. Returns the stream and the request's reply code, or `None`
    /// when auth was refused.
    async fn socks(
        user: &str,
        pass: &str,
        target: SocketAddr,
    ) -> (tokio::net::TcpStream, Option<u8>) {
        assert!(proxy().is_some());
        let port = BRIDGE.get().unwrap().as_ref().unwrap().port;
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        s.write_all(&[5, 1, 2]).await.unwrap();
        let mut m = [0u8; 2];
        s.read_exact(&mut m).await.unwrap();
        assert_eq!(m, [5, 2]);
        let mut auth = vec![1, user.len() as u8];
        auth.extend(user.as_bytes());
        auth.push(pass.len() as u8);
        auth.extend(pass.as_bytes());
        s.write_all(&auth).await.unwrap();
        let mut a = [0u8; 2];
        s.read_exact(&mut a).await.unwrap();
        if a[1] != 0 {
            return (s, None);
        }
        let mut req = vec![5, 1, 0];
        match target.ip() {
            IpAddr::V4(ip) => {
                req.push(1);
                req.extend(ip.octets());
            }
            IpAddr::V6(ip) => {
                req.push(4);
                req.extend(ip.octets());
            }
        }
        req.extend(target.port().to_be_bytes());
        s.write_all(&req).await.unwrap();
        let mut rep = [0u8; 10];
        s.read_exact(&mut rep).await.unwrap();
        (s, Some(rep[1]))
    }

    fn creds() -> (String, String) {
        let _ = proxy();
        let b = BRIDGE.get().unwrap().as_ref().unwrap();
        (b.user.clone(), b.pass.clone())
    }

    #[tokio::test]
    async fn bridge_carries_an_authenticated_connect() {
        let echo = echo_server().await;
        let (user, pass) = creds();
        let (mut s, rep) = socks(&user, &pass, echo).await;
        assert_eq!(rep, Some(REP_OK));
        s.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
    }

    /// A mesh HTTP client — reqwest, through the bridge — reaches an IPv6
    /// server, the way a pairing POST reaches a peer's `[fd00::…]:4873`.
    #[tokio::test]
    async fn a_mesh_client_posts_through_the_bridge_over_ipv6() {
        use axum::routing::post;
        let listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().route("/pair", post(|body: String| async move { body }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = crate::http::HttpClient::mesh(reqwest::Client::builder());
        let reply = client
            .post(&format!("http://[::1]:{}/pair", addr.port()))
            .unwrap()
            .body("hello")
            .send()
            .await
            .unwrap();
        assert!(reply.status().is_success());
        assert_eq!(reply.text().await.unwrap(), "hello");
    }

    /// What hyper-util before 0.1.21 sends for an IPv6 destination: the
    /// literal, bracketed, as a domain name. Carried; a real name is not.
    #[tokio::test]
    async fn bridge_reads_a_bracketed_literal_sent_as_a_name() {
        let echo = echo_server().await;
        let (user, pass) = creds();
        for (name, want) in [
            (format!("[{}]", echo.ip()), REP_OK),
            (echo.ip().to_string(), REP_OK),
            ("example.com".to_string(), REP_ADDRESS_TYPE),
        ] {
            let port = BRIDGE.get().unwrap().as_ref().unwrap().port;
            let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            s.write_all(&[5, 1, 2]).await.unwrap();
            let mut m = [0u8; 2];
            s.read_exact(&mut m).await.unwrap();
            let mut auth = vec![1, user.len() as u8];
            auth.extend(user.as_bytes());
            auth.push(pass.len() as u8);
            auth.extend(pass.as_bytes());
            s.write_all(&auth).await.unwrap();
            s.read_exact(&mut m).await.unwrap();
            let mut req = vec![5, 1, 0, 3, name.len() as u8];
            req.extend(name.as_bytes());
            req.extend(echo.port().to_be_bytes());
            s.write_all(&req).await.unwrap();
            let mut rep = [0u8; 10];
            s.read_exact(&mut rep).await.unwrap();
            assert_eq!(rep[1], want, "{name}");
        }
    }

    /// Two mesh stacks, each one's packets offered to the other.
    fn stack_pair() -> (Netstack, Netstack) {
        let addr = |last| Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, last);
        let handle = tokio::runtime::Handle::current();
        let (to_b, mut for_b) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let (to_a, mut for_a) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let a = Netstack::start(
            Config::new(addr(1)),
            Box::new(move |p| {
                let _ = to_b.send(p);
            }),
            &handle,
        );
        let b = Netstack::start(
            Config::new(addr(2)),
            Box::new(move |p| {
                let _ = to_a.send(p);
            }),
            &handle,
        );
        let (a2, b2) = (a.clone(), b.clone());
        tokio::spawn(async move {
            while let Some(p) = for_b.recv().await {
                let _ = b2.offer(p);
            }
        });
        tokio::spawn(async move {
            while let Some(p) = for_a.recv().await {
                let _ = a2.offer(p);
            }
        });
        (a, b)
    }

    /// A server on the mesh stack sees each peer by its mesh address, so the
    /// Circle gate judges the right one — here Blossom's, which refuses a
    /// stranger.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_gate_sees_a_stack_peer_by_its_mesh_address() {
        let (a, b) = stack_pair();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = seen.clone();
        let access: myco_blossom::server::AccessFn = std::sync::Arc::new(move |ip, _| {
            record.lock().unwrap().push(ip);
            false
        });
        let listener = MeshListener(b.bind(24243).unwrap());
        tokio::spawn(myco_blossom::server::serve_on_guarded(
            std::sync::Arc::new(nsite_deck::testing::MemBlobs::new()),
            listener,
            access,
            None,
        ));
        let remote = std::net::SocketAddrV6::new(b.addr(), 24243, 0, 0);
        let mut stream = a.connect(remote).await.unwrap();
        let sha = "0".repeat(64);
        stream
            .write_all(format!("HEAD /{sha} HTTP/1.1\r\nHost: peer\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut head = [0u8; 12];
        stream.read_exact(&mut head).await.unwrap();
        assert!(
            head.starts_with(b"HTTP/1.1 4"),
            "{:?}",
            String::from_utf8_lossy(&head)
        );
        assert_eq!(*seen.lock().unwrap(), vec![IpAddr::V6(a.addr())]);
    }

    /// A client that hangs up mid-dial is noticed, so the bridge stops
    /// dialling for it.
    #[tokio::test]
    async fn the_bridge_notices_a_client_hanging_up() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (served, _) = listener.accept().await.unwrap();
        let waiting = tokio::spawn(async move { hung_up(&served).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiting.is_finished(), "still connected");
        drop(client);
        tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .expect("the hang-up should be seen")
            .unwrap();
    }

    #[tokio::test]
    async fn bridge_refuses_a_wrong_secret() {
        let echo = echo_server().await;
        let (user, _) = creds();
        let (_, rep) = socks(&user, "not-the-secret", echo).await;
        assert_eq!(rep, None);
    }

    #[tokio::test]
    async fn bridge_refuses_anything_off_the_mesh() {
        let (user, pass) = creds();
        let (_, rep) = socks(&user, &pass, "93.184.216.34:80".parse().unwrap()).await;
        assert_eq!(rep, Some(REP_NOT_ALLOWED));
    }
}
