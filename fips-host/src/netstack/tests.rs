//! Two stacks wired back to back, standing in for two nodes and the mesh
//! between them.

use super::*;
use std::sync::atomic::AtomicU64;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const A: Ipv6Addr = Ipv6Addr::new(0xfd01, 0, 0, 0, 0, 0, 0, 1);
const B: Ipv6Addr = Ipv6Addr::new(0xfd02, 0, 0, 0, 0, 0, 0, 2);

/// FIPS's effective IPv6 MTU on a 1280-byte transport.
const MTU: usize = 1203;

/// A stack whose packets go into a channel, and that channel.
fn stack(addr: Ipv6Addr) -> (Netstack, tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let stack = Netstack::start(
        Config::new(addr, MTU),
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
    let (a, from_a) = stack(A);
    let (b, from_b) = stack(B);
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
        a.bind(61000).unwrap_err().kind(),
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
