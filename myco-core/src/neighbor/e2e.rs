//! Two devices' neighborhoods, linked through a real link service on
//! loopback: what one publishes reaches the other, and a pull brings it
//! back. Host-only stand-in for the two-phone test.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use myco_napplet_runtime::nap::neighbor::{CreateRequest, NeighborData, NeighborHost};
use myco_napplet_runtime::NappletIdentity;
use nostr::{EventBuilder, Filter, Keys, Kind};
use tower::ServiceExt as _;

use super::host::{Ask, Asker, Neighbors, PeerDirectory, PeerView, Seams};
use super::link::{Links, SetupResponse, Shared};
use super::service::{routes, LinkService};
use super::store::Record;

struct Yes;
#[async_trait::async_trait]
impl Asker for Yes {
    async fn confirm(&self, _: Ask) -> bool {
        true
    }
}

struct Nobody;
impl PeerDirectory for Nobody {
    fn peers(&self, _: &Record) -> Vec<PeerView> {
        Vec::new()
    }
}

/// One device: its links, its neighborhoods, and its link service served on
/// a loopback port.
struct Device {
    svc: LinkService,
    neighbors: Arc<Neighbors>,
}

async fn device() -> Device {
    let d = std::env::temp_dir().join(format!("myco-e2e-{}", super::id::token()));
    std::fs::create_dir_all(&d).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let links = Arc::new(Links::open(&d));
    let neighbors = Arc::new(Neighbors::open(
        &d,
        Seams {
            peers: Arc::new(Nobody),
            asker: Arc::new(Yes),
            scoped_store: Arc::new(nsite_deck::testing::MemRelay::new()),
            scoped_blobs: Arc::new(nsite_deck::testing::MemBlobs::new()),
            links: Some(links.clone()),
        },
    ));
    let svc = LinkService {
        links,
        neighbors: Arc::default(),
        bases: vec![base],
        relay_url: String::new(),
        blossom_url: String::new(),
    };
    let _ = svc.neighbors.set(neighbors.clone());
    let app = routes(svc.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Device { svc, neighbors }
}

fn app() -> NappletIdentity {
    NappletIdentity::new("chat", "hash")
}

/// Alice starts "Club" and invites; Bob joins carrying it.
async fn linked_club() -> (Device, Device, String, String) {
    let (alice, bob) = (device().await, device().await);
    let club = alice
        .neighbors
        .create(
            &app(),
            CreateRequest {
                name: "Club".into(),
                auto_accept: false,
            },
        )
        .await
        .unwrap();
    let id = alice
        .neighbors
        .records()
        .iter()
        .find(|r| r.name == "Club")
        .unwrap()
        .id;
    let offer = alice.svc.invite(
        vec![Shared {
            id,
            name: "Club".into(),
        }],
        600,
    );
    let url = offer.link[0].clone();
    let bob_svc = bob.svc.clone();
    let (request, joining) =
        Links::join(&offer, &url, vec![id], &|pk| bob_svc.offer(pk), None).unwrap();
    let path = url
        .split_once("/nh/")
        .map(|(_, p)| format!("/nh/{p}/setup"))
        .unwrap();
    let resp = routes(alice.svc.clone())
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let response: SetupResponse = serde_json::from_slice(&bytes).unwrap();
    bob.svc.links.finish(joining, &response).unwrap();
    bob.neighbors.adopt(&Shared {
        id,
        name: "Club".into(),
    });
    let bob_addr = bob
        .neighbors
        .list(&app())
        .await
        .into_iter()
        .find(|n| n.name == "Club")
        .unwrap()
        .address
        .unwrap();
    (alice, bob, club.address.unwrap(), bob_addr)
}

fn note(text: &str) -> nostr::Event {
    EventBuilder::text_note(text)
        .sign_with_keys(&Keys::generate())
        .unwrap()
}

async fn arrived(n: &Neighbors, id: super::id::NeighborhoodId, text: &str) -> bool {
    for _ in 0..100 {
        if n.link_backlog(id, &kind1())
            .await
            .iter()
            .any(|e| e.content == text)
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

fn kind1() -> Vec<Filter> {
    vec![Filter::new().kind(Kind::TextNote)]
}

#[tokio::test(flavor = "multi_thread")]
async fn a_publish_crosses_the_link_and_a_pull_brings_it_back() {
    let (alice, bob, alice_addr, bob_addr) = linked_club().await;
    let id = alice
        .neighbors
        .records()
        .iter()
        .find(|r| r.name == "Club")
        .unwrap()
        .id;

    // Bob publishes; it arrives in Alice's Club.
    bob.neighbors
        .publish(&app(), &bob_addr, note("hi club"))
        .await
        .unwrap();
    assert!(
        arrived(&alice.neighbors, id, "hi club").await,
        "Bob's event reached Alice over the link"
    );

    // Alice posts while Bob isn't asking; Bob's subscription pulls it.
    alice
        .neighbors
        .publish(&app(), &alice_addr, note("from alice"))
        .await
        .unwrap();
    let b = bob.neighbors.clone();
    let mut feed = b.subscribe(&app(), &bob_addr, kind1()).await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(10), async {
        use futures_util::StreamExt as _;
        loop {
            if let Some(e) = feed.live.next().await {
                if e.content == "from alice" {
                    return true;
                }
            }
        }
    })
    .await
    .unwrap_or(false)
        || feed.backlog.iter().any(|e| e.content == "from alice");
    assert!(got, "Alice's event reached Bob");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_not_carrying_the_neighborhood_is_refused() {
    let (alice, bob, _, _) = linked_club().await;
    let other = super::id::NeighborhoodId::random();
    let link = bob.svc.links.all().remove(0);
    let sent = super::wire::send(&link, other, 0, &note("sneaky")).await;
    assert!(sent.is_err() || !sent.unwrap(), "refused");
    assert!(alice
        .neighbors
        .link_backlog(other, &kind1())
        .await
        .is_empty());
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(bytes))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blob_kept_in_the_neighborhood_is_fetched_over_the_link() {
    let (alice, bob, alice_addr, bob_addr) = linked_club().await;
    let bytes = b"club poster".to_vec();
    let sha = sha256_hex(&bytes);
    alice
        .neighbors
        .upload(&app(), &alice_addr, bytes.clone(), &sha)
        .await
        .unwrap();

    let got = bob.neighbors.blob(&app(), &bob_addr, &sha).await.unwrap();
    assert_eq!(got, Some(bytes.clone()), "Bob fetched it from Alice");

    // Now kept in Bob's Club too: served even with Alice gone.
    let id = bob.neighbors.records()[0].id;
    assert_eq!(bob.neighbors.link_blob(id, &sha).await, Some(bytes));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blob_outside_the_neighborhood_is_not_served() {
    let (alice, bob, _, bob_addr) = linked_club().await;
    // Alice holds it, but in another neighborhood Bob isn't in.
    let other = alice
        .neighbors
        .create(
            &app(),
            CreateRequest {
                name: "Private".into(),
                auto_accept: false,
            },
        )
        .await
        .unwrap();
    let bytes = b"not for the club".to_vec();
    let sha = sha256_hex(&bytes);
    alice
        .neighbors
        .upload(&app(), other.address.as_ref().unwrap(), bytes, &sha)
        .await
        .unwrap();

    assert_eq!(
        bob.neighbors.blob(&app(), &bob_addr, &sha).await.unwrap(),
        None,
        "only the Club's own blobs cross the Club's link"
    );
}

/// Fetch the peer's current offer onto `dev`'s only link, as presence does.
async fn refresh_offer(dev: &Device) -> super::link::Link {
    let link = dev.svc.links.all().remove(0);
    let resp = super::service::call(&link, reqwest::Method::GET, "offer", None)
        .await
        .unwrap();
    let offer: nostr::Event = serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    dev.svc.links.update_offer(&link.id, offer);
    dev.svc.links.all().remove(0)
}

#[tokio::test(flavor = "multi_thread")]
async fn extensions_are_shared_when_on_at_both_ends() {
    use super::extensions::{shared, NAPPLET_UPDATES, NSITE_UPDATES};
    let (alice, bob, _, _) = linked_club().await;
    let id = bob.neighbors.records()[0].id;

    let link = refresh_offer(&bob).await;
    let club = bob.neighbors.records()[0].clone();
    assert!(shared(&club, &link, id, NAPPLET_UPDATES), "on by default");
    assert!(shared(&club, &link, id, NSITE_UPDATES));

    // Alice turns nsite updates off in the Club; Bob learns it from her offer.
    alice.neighbors.set_extension(id, NSITE_UPDATES, false);
    let link = refresh_offer(&bob).await;
    assert!(!shared(&club, &link, id, NSITE_UPDATES), "off at her end");
    assert!(
        shared(&club, &link, id, NAPPLET_UPDATES),
        "the rest stay on"
    );

    // Bob's own switch counts as much as hers.
    bob.neighbors.set_extension(id, NAPPLET_UPDATES, false);
    let club = bob.neighbors.records()[0].clone();
    assert!(!shared(&club, &link, id, NAPPLET_UPDATES), "off at his end");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_napplet_update_is_pushed_and_its_files_fetched_over_the_link() {
    use super::extensions::NAPPLET_UPDATES;
    use nsite_deck::{BlobStore as _, PeerSource as _};
    let (alice, bob, _, _) = linked_club().await;
    // Both learn the other's extensions, as presence would.
    refresh_offer(&alice).await;
    refresh_offer(&bob).await;
    let id = bob.neighbors.records()[0].id;

    // Alice holds the new version's file in her own store, not the
    // neighborhood's.
    let kept = Arc::new(nsite_deck::testing::MemBlobs::new());
    let file = b"<html>v2</html>".to_vec();
    let sha = kept.put(&file).await.unwrap();
    alice.neighbors.set_kept_blobs(kept);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    bob.neighbors.on_update(Arc::new(move |n, ev| {
        let _ = tx.send((n, ev));
    }));

    let manifest = EventBuilder::new(Kind::from(myco_napplet_runtime::KIND_NAMED), "")
        .tags([nostr::Tag::identifier("chat")])
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let sent = alice
        .neighbors
        .share_update(NAPPLET_UPDATES, manifest.clone(), &[sha.clone()])
        .await;
    assert_eq!(sent, 1, "pushed to Bob's link");

    let (n, got) = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("Bob was told")
        .unwrap();
    assert_eq!((n, got.id), (id, manifest.id));

    let source = super::updates::NeighborhoodSource {
        neighbors: bob.neighbors.clone(),
        n,
    };
    assert_eq!(
        source.fetch_blob(&sha, &[]).await.unwrap(),
        Some(file),
        "the version's file came from Alice through the Club"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_update_is_pushed_where_the_peer_has_the_extension_off() {
    use super::extensions::NAPPLET_UPDATES;
    let (alice, bob, _, _) = linked_club().await;
    let id = bob.neighbors.records()[0].id;
    bob.neighbors.set_extension(id, NAPPLET_UPDATES, false);
    refresh_offer(&alice).await;
    let manifest = EventBuilder::new(Kind::from(myco_napplet_runtime::KIND_NAMED), "")
        .tags([nostr::Tag::identifier("chat")])
        .sign_with_keys(&Keys::generate())
        .unwrap();
    assert_eq!(
        alice
            .neighbors
            .share_update(NAPPLET_UPDATES, manifest, &[])
            .await,
        0
    );
}
