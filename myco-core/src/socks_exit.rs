//! **Full-tunnel exit over SOCKS5**: every TCP connection on the phone, not just
//! the web traffic that honours a system HTTP proxy, carried to a SOCKS5 proxy
//! on a mesh exit node.
//!
//! In this mode the VpnService claims the default routes, so every app's
//! packets reach [`crate::tun_bridge::send_packet`]. Mesh-bound (`fd00::/8`)
//! packets still go to FIPS; anything else is [`offer`]ed here, to a userspace
//! TCP/IP stack ([`ipstack`]) that terminates each TCP flow and re-opens it as
//! a SOCKS5 `CONNECT` to the exit. Packets the stack sends back go to the TUN
//! through [`crate::tun_bridge::push_local`].
//!
//! The upstream connection to the proxy is an ordinary socket in this process,
//! to a mesh address — so it rides the tunnel's `fd00::/8` route into FIPS, and
//! never back into this stack. No `protect()` needed, no loop.
//!
//! Only TCP is carried. UDP (QUIC included) is dropped, and apps fall back to
//! TCP. DNS is not lost with it: queries go to the sentinel resolver, and
//! [`crate::dns_intercept`] relays non-`.fips` ones over TCP through
//! [`dns_over_tcp`] while this mode is on.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use ipstack::{IpStack, IpStackConfig, IpStackStream};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

/// The resolver non-`.fips` DNS is asked through the proxy. The phone's own
/// resolvers are on the network underneath, which the exit usually cannot
/// reach, so use one every exit can.
const DNS_VIA_EXIT: &str = "1.1.1.1:53";

/// How long a SOCKS handshake or DNS exchange may take before it is dropped.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

/// A running stack: where its proxy is, how to feed it, and the task that owns
/// it (aborting the task drops the [`IpStack`], which stops it).
struct Running {
    proxy: String,
    tx: UnboundedSender<Vec<u8>>,
    task: JoinHandle<()>,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

/// The exit's own runtime. `None` if it could not be built — the callers sit
/// under JNI, where a panic would take the process down.
fn rt() -> Option<&'static tokio::runtime::Runtime> {
    static RT: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("myco-socks")
            .enable_all()
            .build()
            .map_err(|e| tracing::warn!("socks exit runtime failed: {e}"))
            .ok()
    })
    .as_ref()
}

/// Turn the exit on (`Some("host:port")`) or off (`None` or empty). Restarts
/// the stack only when the proxy changes.
pub fn set_proxy(proxy: Option<String>) {
    let proxy = proxy
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty());
    let mut running = RUNNING.lock().unwrap();
    if running.as_ref().map(|r| &r.proxy) == proxy.as_ref() {
        return;
    }
    if let Some(old) = running.take() {
        old.task.abort();
    }
    let Some(proxy) = proxy else {
        tracing::info!("socks exit off");
        return;
    };
    let Some(rt) = rt() else {
        return;
    };
    tracing::info!("socks exit on via {proxy}");
    let (tx, rx) = unbounded_channel();
    let task = rt.spawn(serve(proxy.clone(), rx));
    *running = Some(Running { proxy, tx, task });
}

/// The proxy in use, if the exit is on.
pub fn proxy() -> Option<String> {
    RUNNING.lock().unwrap().as_ref().map(|r| r.proxy.clone())
}

/// Hand a non-mesh packet from the TUN to the stack. `false` if the exit is off.
pub fn offer(packet: Vec<u8>) -> bool {
    match RUNNING.lock().unwrap().as_ref() {
        Some(r) => r.tx.send(packet).is_ok(),
        None => false,
    }
}

async fn serve(proxy: String, rx: UnboundedReceiver<Vec<u8>>) {
    let mut config = IpStackConfig::default();
    config.mtu_unchecked(1280);
    let mut stack = IpStack::new(config, ChanDevice { rx });
    while let Ok(stream) = stack.accept().await {
        // UDP and anything else is dropped (see the module docs).
        if let IpStackStream::Tcp(mut tcp) = stream {
            let proxy = proxy.clone();
            tokio::spawn(async move {
                let dst = tcp.peer_addr();
                match connect(&proxy, dst).await {
                    Ok(mut up) => {
                        let _ = tokio::io::copy_bidirectional(&mut tcp, &mut up).await;
                    }
                    Err(e) => tracing::debug!("socks exit: {dst} via {proxy}: {e}"),
                }
            });
        }
    }
}

/// `<npub>.fips:port` as the mesh socket address it names, computed from the
/// key: the system resolver fails the name from inside this process (seen on
/// device), and the address needs no lookup anyway. Also warms the route, which a
/// non-neighbour exit needs before its address is routable (see
/// [`crate::dns_intercept::warm_route`]). `None` for any other host.
fn mesh_addr(proxy: &str) -> Option<String> {
    let (host, port) = proxy.rsplit_once(':')?;
    let npub = host.strip_suffix(".fips")?;
    let ip = fips::PeerIdentity::from_npub(npub)
        .ok()?
        .address()
        .to_ipv6();
    crate::dns_intercept::warm_route(npub);
    Some(format!("[{ip}]:{port}"))
}

/// Open a SOCKS5 `CONNECT` (no auth) to `dst` through `proxy`.
async fn connect(proxy: &str, dst: SocketAddr) -> io::Result<TcpStream> {
    tokio::time::timeout(EXIT_TIMEOUT, async {
        let mut s = TcpStream::connect(mesh_addr(proxy).as_deref().unwrap_or(proxy)).await?;
        s.set_nodelay(true)?;
        s.write_all(&[5, 1, 0]).await?;
        let mut hello = [0u8; 2];
        s.read_exact(&mut hello).await?;
        if hello != [5, 0] {
            return Err(io::Error::other("proxy wants auth we don't offer"));
        }
        let mut req = vec![5, 1, 0];
        match dst {
            SocketAddr::V4(a) => {
                req.push(1);
                req.extend_from_slice(&a.ip().octets());
            }
            SocketAddr::V6(a) => {
                req.push(4);
                req.extend_from_slice(&a.ip().octets());
            }
        }
        req.extend_from_slice(&dst.port().to_be_bytes());
        s.write_all(&req).await?;
        let mut head = [0u8; 4];
        s.read_exact(&mut head).await?;
        if head[1] != 0 {
            return Err(io::Error::other(format!("proxy refused: code {}", head[1])));
        }
        // Skip the bound address the reply carries.
        let skip = match head[3] {
            1 => 4 + 2,
            4 => 16 + 2,
            3 => s.read_u8().await? as usize + 2,
            _ => return Err(io::Error::other("bad proxy reply")),
        };
        let mut rest = vec![0u8; skip];
        s.read_exact(&mut rest).await?;
        Ok(s)
    })
    .await
    .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
}

/// Resolve one DNS query (wire format) over TCP through the exit. Blocking;
/// [`crate::dns_intercept`] calls it from its per-query thread.
pub fn dns_over_tcp(proxy: &str, query: &[u8]) -> Option<Vec<u8>> {
    let proxy = proxy.to_string();
    let query = query.to_vec();
    let result = rt()?.block_on(async move {
        tokio::time::timeout(EXIT_TIMEOUT, async {
            let dns: SocketAddr = DNS_VIA_EXIT.parse().map_err(io::Error::other)?;
            let mut s = connect(&proxy, dns).await?;
            let mut msg = (query.len() as u16).to_be_bytes().to_vec();
            msg.extend_from_slice(&query);
            s.write_all(&msg).await?;
            let n = s.read_u16().await? as usize;
            let mut reply = vec![0u8; n];
            s.read_exact(&mut reply).await?;
            Ok::<_, io::Error>(reply)
        })
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
    });
    result
        .map_err(|e| tracing::debug!("socks exit: dns via {DNS_VIA_EXIT}: {e}"))
        .ok()
}

/// The stack's "TUN device": one packet per read from the TUN pump, one packet
/// per write back to it (ipstack writes each packet with a single `write_all`).
struct ChanDevice {
    rx: UnboundedReceiver<Vec<u8>>,
}

impl AsyncRead for ChanDevice {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.rx.poll_recv(cx) {
            Poll::Ready(Some(pkt)) => {
                let n = pkt.len().min(buf.remaining());
                buf.put_slice(&pkt[..n]);
                Poll::Ready(Ok(()))
            }
            // Closed: the exit was turned off and this task is being aborted.
            Poll::Ready(None) => Poll::Pending,
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for ChanDevice {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        crate::tun_bridge::push_local(buf.to_vec());
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// A one-shot SOCKS5 proxy: checks the no-auth greeting and the CONNECT
    /// request for `want`, replies success, then echoes.
    async fn fake_proxy(want: SocketAddr) -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut hello = [0u8; 3];
            s.read_exact(&mut hello).await.unwrap();
            assert_eq!(hello, [5, 1, 0]);
            s.write_all(&[5, 0]).await.unwrap();
            let mut req = [0u8; 10];
            s.read_exact(&mut req).await.unwrap();
            assert_eq!(req[..4], [5, 1, 0, 1]);
            let SocketAddr::V4(w) = want else { panic!() };
            assert_eq!(req[4..8], w.ip().octets());
            assert_eq!(req[8..], w.port().to_be_bytes());
            s.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.unwrap();
            let mut buf = [0u8; 64];
            let n = s.read(&mut buf).await.unwrap();
            s.write_all(&buf[..n]).await.unwrap();
        });
        addr
    }

    #[test]
    fn a_fips_proxy_host_becomes_its_mesh_address() {
        let npub = "npub1l0k4hf0a505h06622addqq7g8t96anm0j2uq8yrqjr08422zkqxskjqn0l";
        assert_eq!(
            mesh_addr(&format!("{npub}.fips:1080")).as_deref(),
            Some("[fdb5:12f7:9554:f160:527b:b19a:7fd2:d734]:1080")
        );
        // Literals and ordinary names are left to the system.
        assert_eq!(mesh_addr("[fd00::1]:1080"), None);
        assert_eq!(mesh_addr("proxy.example:1080"), None);
        assert_eq!(mesh_addr("notakey.fips:1080"), None);
    }

    #[tokio::test]
    async fn connect_speaks_socks5_then_carries_bytes() {
        let dst: SocketAddr = "93.184.216.34:443".parse().unwrap();
        let proxy = fake_proxy(dst).await;
        let mut s = connect(&proxy, dst).await.unwrap();
        s.write_all(b"ping").await.unwrap();
        let mut back = [0u8; 4];
        s.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"ping");
    }
}
