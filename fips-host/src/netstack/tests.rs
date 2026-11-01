//! Two stacks wired back to back, standing in for two nodes and the mesh
//! between them.

use super::*;
use std::sync::atomic::AtomicU64;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const A: Ipv6Addr = Ipv6Addr::new(0xfd01, 0, 0, 0, 0, 0, 0, 1);
const B: Ipv6Addr = Ipv6Addr::new(0xfd02, 0, 0, 0, 0, 0, 0, 2);

/// FIPS's effective IPv6 MTU on a 1280-byte transport.
const MTU: usize = 1203;

/// The test stacks' shape: the defaults, at [`MTU`].
fn config(addr: Ipv6Addr) -> Config {
    Config {
        mtu: MTU,
        ..Config::new(addr)
    }
}

/// A stack whose packets go into a channel, and that channel.
fn stack(addr: Ipv6Addr) -> (Netstack, tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) {
    stack_with(config(addr))
}

fn stack_with(config: Config) -> (Netstack, tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let stack = Netstack::start(
        config,
        Box::new(move |p| {
            let _ = tx.send(p);
        }),
        &tokio::runtime::Handle::current(),
    );
    (stack, rx)
}

/// Two stacks, each one's packets offered to the other. `lose` decides, by
/// packet number across both directions, which never arrive.
fn pair(lose: fn(u64) -> bool) -> (Netstack, Netstack) {
    pair_with(config(A), config(B), lose)
}

fn pair_with(a: Config, b: Config, lose: fn(u64) -> bool) -> (Netstack, Netstack) {
    let (a, from_a) = stack_with(a);
    let (b, from_b) = stack_with(b);
    let sent = Arc::new(AtomicU64::new(0));
    for (mut from, to) in [(from_a, b.clone()), (from_b, a.clone())] {
        let sent = sent.clone();
        tokio::spawn(async move {
            while let Some(packet) = from.recv().await {
                if !lose(sent.fetch_add(1, Ordering::Relaxed)) {
                    let _ = to.offer(packet);
                }
            }
        });
    }
    (a, b)
}

fn lossless(_: u64) -> bool {
    false
}

fn at(addr: Ipv6Addr, port: u16) -> SocketAddrV6 {
    SocketAddrV6::new(addr, port, 0, 0)
}

const WAIT: Duration = Duration::from_secs(10);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_accept_and_talk_both_ways() {
    let (a, b) = pair(lossless);
    let listener = b.bind(4870).unwrap();
    let (client, accepted) = tokio::join!(
        tokio::time::timeout(WAIT, a.connect(at(B, 4870))),
        tokio::time::timeout(WAIT, listener.accept()),
    );
    let mut client = client.unwrap().unwrap();
    let (mut server, peer) = accepted.unwrap().unwrap();
    assert_eq!(*peer.ip(), A);
    assert!(a.config_ephemeral().contains(&peer.port()));
    assert_eq!(client.peer_addr(), Some(at(B, 4870)));

    client.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    server.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"ping");

    server.write_all(b"pong").await.unwrap();
    client.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"pong");
}

/// Bulk data in one direction, in segments the MTU bounds, arrives whole and
/// in order — and still does when one packet in a hundred is lost.
///
/// Loss is slow to recover from: smoltcp has no SACK and a 1 s floor on its
/// retransmission timeout, so 1 MB takes ~3 s at 1% loss and ~70 s at 5%
/// (against ~60 ms with none).
async fn transfer(lose: fn(u64) -> bool) {
    let (a, b) = pair(lose);
    let listener = b.bind(24243).unwrap();
    let data: Vec<u8> = (0..1_000_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let expected = data.clone();
    let reader = tokio::spawn(async move {
        let (mut server, _) = listener.accept().await.unwrap();
        let mut got = Vec::new();
        server.read_to_end(&mut got).await.unwrap();
        got
    });
    let mut client = tokio::time::timeout(WAIT, a.connect(at(B, 24243)))
        .await
        .unwrap()
        .unwrap();
    client.write_all(&data).await.unwrap();
    client.shutdown().await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(60), reader)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.len(), expected.len());
    assert!(got == expected, "bytes differ");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_transfer_arrives_whole() {
    transfer(lossless).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_transfer_survives_loss() {
    transfer(|n| n % 100 == 7).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_reads_as_end_of_stream_and_frees_the_sockets() {
    let (a, b) = pair(lossless);
    let listener = b.bind(4873).unwrap();
    let (a_base, b_base) = (a.socket_count(), b.socket_count());
    let (client, accepted) = tokio::join!(a.connect(at(B, 4873)), listener.accept());
    let mut client = client.unwrap();
    let (mut server, _) = accepted.unwrap();

    client.write_all(b"bye").await.unwrap();
    drop(client);
    let mut got = Vec::new();
    tokio::time::timeout(WAIT, server.read_to_end(&mut got))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, b"bye");
    drop(server);

    // Both ends finish the close and give their sockets back; the listener
    // keeps its backlog topped up to where it was.
    tokio::time::timeout(WAIT, async {
        while a.socket_count() != a_base || b.socket_count() != b_base {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("sockets were not freed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn many_connections_at_once() {
    let (a, b) = pair(lossless);
    let listener = b.bind(4870).unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = [0u8; 8];
                stream.read_exact(&mut buf).await.unwrap();
                stream.write_all(&buf).await.unwrap();
            });
        }
    });
    let clients = (0..32u64).map(|i| {
        let a = a.clone();
        tokio::spawn(async move {
            let mut stream = a.connect(at(B, 4870)).await.unwrap();
            stream.write_all(&i.to_be_bytes()).await.unwrap();
            let mut buf = [0u8; 8];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(u64::from_be_bytes(buf), i);
        })
    });
    for client in clients {
        tokio::time::timeout(WAIT, client).await.unwrap().unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_port_nobody_listens_on_is_not_claimed() {
    let (a, b) = pair(lossless);
    // B claims nothing on 9999, so A's SYN is handed back to the embedder
    // (and here, dropped): the dial is left to its timeout.
    let dial = tokio::time::timeout(Duration::from_millis(300), a.connect(at(B, 9999))).await;
    assert!(dial.is_err(), "dial should not have finished");
    let _ = b;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_listener_per_port_until_dropped() {
    let (a, _b) = pair(lossless);
    let first = a.bind(4870).unwrap();
    assert_eq!(a.bind(4870).unwrap_err().kind(), io::ErrorKind::AddrInUse);
    assert_eq!(first.local_addr(), at(A, 4870));
    drop(first);
    a.bind(4870).unwrap();
    // The ephemeral range is the dialler's.
    assert_eq!(
        a.bind(61001).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claims_only_its_own_tcp() {
    let (a, _b) = pair(lossless);
    let _listener = a.bind(4870).unwrap();
    let packet = |dst: Ipv6Addr, next_header: u8, port: u16| {
        let mut p = vec![0u8; 60];
        p[0] = 0x60;
        p[6] = next_header;
        p[24..40].copy_from_slice(&dst.octets());
        p[42..44].copy_from_slice(&port.to_be_bytes());
        p
    };
    assert!(a.claims(&packet(A, 6, 4870)));
    assert!(a.claims(&packet(A, 6, 61234)));
    assert!(!a.claims(&packet(A, 6, 8080)), "port nobody holds");
    assert!(!a.claims(&packet(B, 6, 4870)), "someone else's address");
    assert!(!a.claims(&packet(A, 17, 4870)), "UDP");
    assert!(!a.claims(&packet(A, 6, 4870)[..43]), "truncated");
    assert_eq!(a.offer(packet(A, 17, 4870)).unwrap_err().len(), 60);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_stack_ends_its_streams() {
    let (a, b) = pair(lossless);
    let listener = b.bind(4870).unwrap();
    let (client, accepted) = tokio::join!(a.connect(at(B, 4870)), listener.accept());
    let mut client = client.unwrap();
    let _server = accepted.unwrap();
    // The forwarders hold clones of both stacks, so dropping ours would not
    // stop A; stop it the way its last owner would.
    a.shared.lock().shut = true;
    a.shared.wake.notify_one();
    let mut buf = [0u8; 1];
    let read = tokio::time::timeout(WAIT, client.read(&mut buf))
        .await
        .unwrap();
    assert!(read.is_err());
}

impl Netstack {
    fn config_ephemeral(&self) -> RangeInclusive<u16> {
        self.shared.ephemeral.clone()
    }
}

// --- holding up against peers ---

const X: Ipv6Addr = Ipv6Addr::new(0xfd03, 0, 0, 0, 0, 0, 0, 3);

/// B listens; A and X reach it, and B's replies reach A only: X sends its
/// first `syns` packets (its SYNs) and is never heard from again.
fn with_a_vanishing_peer(b: Config, syns: u64) -> (Netstack, Netstack, Netstack) {
    let (a, mut from_a) = stack(A);
    let (b, mut from_b) = stack_with(b);
    let (x, mut from_x) = stack(X);
    let to_b = b.clone();
    tokio::spawn(async move {
        while let Some(p) = from_a.recv().await {
            let _ = to_b.offer(p);
        }
    });
    let to_b = b.clone();
    tokio::spawn(async move {
        let mut n = 0;
        while let Some(p) = from_x.recv().await {
            n += 1;
            if n <= syns {
                let _ = to_b.offer(p);
            }
        }
    });
    let to_a = a.clone();
    tokio::spawn(async move {
        while let Some(p) = from_b.recv().await {
            if p[24..40] == A.octets() {
                let _ = to_a.offer(p);
            }
        }
    });
    (a, b, x)
}

/// Eight handshakes left hanging fill a listener's backlog; they time out and
/// the listener takes connections again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_handshakes_time_out() {
    let b = Config {
        handshake_timeout: Duration::from_millis(500),
        per_peer: 100,
        ..config(B)
    };
    let (a, b, x) = with_a_vanishing_peer(b, 8);
    let listener = b.bind(4873).unwrap();
    tokio::spawn(async move { while listener.accept().await.is_ok() {} });
    for _ in 0..8 {
        let x = x.clone();
        tokio::spawn(async move {
            let _ = x.connect(at(B, 4873)).await;
        });
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    // Full of X's half-open handshakes: A is turned away for now…
    let early = a.connect(at(B, 4873)).await;
    assert_eq!(early.unwrap_err().kind(), io::ErrorKind::ConnectionRefused);
    // …but not for good.
    tokio::time::sleep(Duration::from_secs(2)).await;
    tokio::time::timeout(WAIT, a.connect(at(B, 4873)))
        .await
        .unwrap()
        .expect("the listener should accept again");
}

/// A peer may hold only its share: its handshakes past it are dropped, so
/// the backlog stays open to everyone else.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_peer_cannot_fill_a_backlog() {
    let b = Config {
        per_peer: 3,
        ..config(B)
    };
    let (a, b, x) = with_a_vanishing_peer(b, 8);
    let listener = b.bind(4873).unwrap();
    tokio::spawn(async move { while listener.accept().await.is_ok() {} });
    for _ in 0..8 {
        let x = x.clone();
        tokio::spawn(async move {
            let _ = x.connect(at(B, 4873)).await;
        });
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    tokio::time::timeout(WAIT, a.connect(at(B, 4873)))
        .await
        .unwrap()
        .expect("A should still get in");
}

/// One peer opening connection after connection stops at its share, and the
/// stack can still dial out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_peer_cannot_take_every_socket() {
    let b = Config {
        per_peer: 16,
        connect_timeout: Duration::from_millis(500),
        ..config(B)
    };
    let (a, b) = pair_with(config(A), b, lossless);
    let listener = b.bind(4870).unwrap();
    let held = Arc::new(Mutex::new(Vec::new()));
    let keep = held.clone();
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            keep.lock().unwrap().push(s);
        }
    });
    let mut mine = Vec::new();
    for _ in 0..64 {
        match a.connect(at(B, 4870)).await {
            Ok(s) => mine.push(s),
            Err(_) => break,
        }
    }
    assert_eq!(mine.len(), 16, "A's share");
    let _back = a.bind(9).unwrap();
    tokio::time::timeout(WAIT, b.connect(at(A, 9)))
        .await
        .unwrap()
        .expect("B can still dial out");
}

/// Listeners never take the sockets kept for dials, however many peers
/// connect.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listeners_leave_the_dial_reserve() {
    let b = Config {
        max_sockets: 20,
        dial_reserve: 4,
        per_peer: 100,
        ..config(B)
    };
    let (a, b) = pair_with(config(A), b, lossless);
    let listener = b.bind(4870).unwrap();
    let held = Arc::new(Mutex::new(Vec::new()));
    let keep = held.clone();
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            keep.lock().unwrap().push(s);
        }
    });
    let mut mine = Vec::new();
    while let Ok(Ok(s)) =
        tokio::time::timeout(Duration::from_millis(500), a.connect(at(B, 4870))).await
    {
        mine.push(s);
    }
    assert!(
        b.socket_count() <= 16,
        "listeners held {}",
        b.socket_count()
    );
    let _back = a.bind(9).unwrap();
    tokio::time::timeout(WAIT, b.connect(at(A, 9)))
        .await
        .unwrap()
        .expect("the reserve is there for B's own dial");
}

/// A dial nobody answers ends at the connect timeout and frees its socket.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dial_nobody_answers_times_out() {
    let (a, _nowhere) = stack_with(Config {
        connect_timeout: Duration::from_millis(300),
        ..config(A)
    });
    let err = tokio::time::timeout(WAIT, a.connect(at(B, 80)))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    tokio::time::timeout(WAIT, async {
        while a.socket_count() != 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the dial's socket was not freed");
}

/// The node's "destination unreachable" ends a dial at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_route_ends_a_dial_at_once() {
    let (a, mut out) = stack(A);
    let dial = tokio::spawn({
        let a = a.clone();
        async move { a.connect(at(B, 4870)).await }
    });
    let syn = out.recv().await.unwrap();
    // What the node sends back: ICMPv6 type 1, from its own address, quoting
    // the SYN.
    let mut icmp = vec![0u8; 48];
    icmp[0] = 0x60;
    let len = (8 + syn.len()) as u16;
    icmp[4..6].copy_from_slice(&len.to_be_bytes());
    icmp[6] = 58;
    icmp[7] = 64;
    icmp[8..24].copy_from_slice(&A.octets());
    icmp[24..40].copy_from_slice(&A.octets());
    icmp[40] = 1;
    icmp.extend_from_slice(&syn);
    assert!(a.claims(&icmp));
    a.offer(icmp).unwrap();
    let err = tokio::time::timeout(Duration::from_secs(1), dial)
        .await
        .expect("the dial should end at once")
        .unwrap()
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::HostUnreachable);
}

static CUT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// An open connection whose peer goes silent is reset, and its socket freed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_silent_peer_is_let_go() {
    let short = |c: Config| Config {
        keep_alive: Duration::from_millis(200),
        dead_peer_timeout: Duration::from_secs(1),
        ..c
    };
    let (a, b) = pair_with(short(config(A)), short(config(B)), |_| {
        CUT.load(Ordering::Relaxed)
    });
    let listener = b.bind(4870).unwrap();
    let (client, accepted) = tokio::join!(a.connect(at(B, 4870)), listener.accept());
    let mut client = client.unwrap();
    let (_server, _) = accepted.unwrap();
    let before = a.socket_count();
    CUT.store(true, Ordering::Relaxed);
    let mut buf = [0u8; 1];
    let read = tokio::time::timeout(Duration::from_secs(5), client.read(&mut buf))
        .await
        .expect("the connection should be reset");
    assert!(matches!(read, Ok(0) | Err(_)));
    drop(client);
    tokio::time::timeout(WAIT, async {
        while a.socket_count() >= before {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the socket was not freed");
}
