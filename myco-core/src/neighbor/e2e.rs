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

use super::host::{Ask, Asker, Neighbors, Seams};
use super::link::{Links, SetupResponse, Shared};
use super::nearby::{DirectPeer, DirectPeers, Nearby, Reach};
use super::service::{routes, LinkService};

struct Yes;
#[async_trait::async_trait]
impl Asker for Yes {
    async fn confirm(&self, _: Ask) -> bool {
        true
    }
}

/// One device: its links, its neighborhoods, and its link service served on
/// a loopback port.
struct Device {
    svc: LinkService,
    neighbors: Arc<Neighbors>,
}

async fn device() -> Device {
    let dev = device_with(Arc::new(Fake::default()), false).await;
    dev.neighbors.set_nearby(false);
    dev
}

/// Who a device's fake radio sees in reach. Requests arrive from loopback,
/// which no FIPS address maps to, so the fake maps loopback to the first
/// peer it sees.
#[derive(Default)]
struct Fake(std::sync::Mutex<Vec<DirectPeer>>);

impl Fake {
    fn see(&self, peers: Vec<DirectPeer>) {
        *self.0.lock().unwrap() = peers;
    }
}

impl DirectPeers for Fake {
    fn direct(&self) -> Vec<DirectPeer> {
        self.0.lock().unwrap().clone()
    }
    fn who(&self, source: std::net::IpAddr) -> Option<DirectPeer> {
        if source.is_loopback() {
            self.direct().into_iter().next()
        } else {
            None
        }
    }
}

/// One device whose link service sees connection sources, as the auth
/// plane does; `direct` is its radio.
async fn device_with(direct: Arc<dyn DirectPeers>, connect_info: bool) -> Device {
    let d = std::env::temp_dir().join(format!("myco-e2e-{}", super::id::token()));
    std::fs::create_dir_all(&d).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let links = Arc::new(Links::open(&d));
    let neighbors = Arc::new(Neighbors::open(
        &d,
        Seams {
            peers: Arc::new(super::LinkPeers(Some(links.clone()))),
            asker: Arc::new(Yes),
            scoped_stores: Arc::new(crate::neighbor::data::MemStores::default()),
            scoped_blobs: Arc::new(nsite_deck::testing::MemBlobs::new()),
            links: Some(links.clone()),
        },
    ));
    let svc = LinkService {
        links,
        neighbors: Arc::default(),
        bases: vec![base],
        nearby: Arc::new(Nearby::with(direct)),
    };
    let _ = svc.neighbors.set(neighbors.clone());
    let app = routes(svc.clone());
    tokio::spawn(async move {
        if connect_info {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        } else {
            axum::serve(listener, app).await.unwrap();
        }
    });
    Device { svc, neighbors }
}

fn app() -> NappletIdentity {
    NappletIdentity::new("chat", "hash")
}

/// Alice starts "Club" and invites; Bob joins carrying it.
async fn linked_club() -> (Device, Device, String, String) {
    let (alice, bob) = (device().await, device().await);
    let club = club(&alice).await;
    let bob_addr = join_club(&alice, &bob).await;
    (alice, bob, club, bob_addr)
}

/// Alice's Club, by its address.
async fn club(alice: &Device) -> String {
    alice
        .neighbors
        .create(
            &app(),
            CreateRequest {
                name: "Club".into(),
                auto_accept: false,
            },
        )
        .await
        .unwrap()
        .address
        .unwrap()
}

/// Bob joins Alice's Club over a link; Bob's address for it.
async fn join_club(alice: &Device, bob: &Device) -> String {
    let id = club_id(alice);
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
    bob.neighbors
        .list(&app())
        .await
        .into_iter()
        .find(|n| n.name == "Club")
        .unwrap()
        .address
        .unwrap()
}

fn club_id(dev: &Device) -> super::id::NeighborhoodId {
    dev.neighbors
        .records()
        .iter()
        .find(|r| r.name == "Club")
        .unwrap()
        .id
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

/// Whether an event with `text` comes on `feed` within `secs`.
async fn next(feed: &mut myco_napplet_runtime::nap::neighbor::Feed, text: &str, secs: u64) -> bool {
    use futures_util::StreamExt as _;
    tokio::time::timeout(Duration::from_secs(secs), async {
        while let Some(e) = feed.live.next().await {
            if e.content == text {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_open_subscription_follows_a_peer_who_joins_later_until_the_link_ends() {
    let (alice, bob) = (device().await, device().await);
    let addr = club(&alice).await;
    // Alice listens before anyone else is in the Club.
    let a = alice.neighbors.clone();
    let mut feed = a.subscribe(&app(), &addr, kind1()).await.unwrap();
    // Bob joins holding an event he never published to Alice.
    join_club(&alice, &bob).await;
    let id = club_id(&bob);
    bob.neighbors
        .scoped
        .add_shared(id, note("held by bob"))
        .await
        .unwrap();
    assert!(
        next(&mut feed, "held by bob", 10).await,
        "Bob's backlog reached the open subscription"
    );

    // What Bob keeps later follows on its own.
    bob.neighbors
        .scoped
        .add_shared(id, note("later"))
        .await
        .unwrap();
    assert!(
        next(&mut feed, "later", 10).await,
        "a later event followed live"
    );

    // The link ends: nothing more comes from Bob.
    let link = alice.svc.links.carrying(id)[0].id.clone();
    alice.svc.links.end(&link);
    tokio::time::sleep(Duration::from_millis(300)).await;
    bob.neighbors
        .scoped
        .add_shared(id, note("after the end"))
        .await
        .unwrap();
    assert!(
        !next(&mut feed, "after the end", 2).await,
        "the subscription on Bob closed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_event_already_kept_comes_again_from_a_peer_in_reach() {
    let (alice, bob) = (device().await, device().await);
    let addr = club(&alice).await;
    join_club(&alice, &bob).await;
    let id = club_id(&bob);
    bob.neighbors
        .scoped
        .add_shared(id, note("held by bob"))
        .await
        .unwrap();
    let a = alice.neighbors.clone();
    let mut first = a.subscribe(&app(), &addr, kind1()).await.unwrap();
    assert!(next(&mut first, "held by bob", 10).await, "kept by Alice");

    // A later subscription has it from Alice's store, and once more from
    // Bob: a sign someone in reach can serve it, without saying who.
    let mut again = a.subscribe(&app(), &addr, kind1()).await.unwrap();
    assert!(again.backlog.iter().any(|e| e.content == "held by bob"));
    assert!(
        next(&mut again, "held by bob", 10).await,
        "Bob sent it again"
    );
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

#[tokio::test(flavor = "multi_thread")]
async fn sharing_and_getting_are_each_sides_own_choice() {
    use super::extensions::NAPPLET_UPDATES;
    let (alice, bob, _, _) = linked_club().await;
    let id = bob.neighbors.records()[0].id;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    bob.neighbors.on_update(Arc::new(move |_n, _ext, ev| {
        let _ = tx.send(ev.id);
    }));
    let update = || {
        EventBuilder::new(Kind::from(myco_napplet_runtime::KIND_NAMED), "")
            .tags([nostr::Tag::identifier("chat")])
            .sign_with_keys(&Keys::generate())
            .unwrap()
    };

    // Alice doesn't share: nothing leaves her phone.
    alice
        .neighbors
        .set_extension(id, NAPPLET_UPDATES, true, false);
    assert_eq!(
        alice
            .neighbors
            .share_update(NAPPLET_UPDATES, update(), &[])
            .await,
        0
    );

    // Alice shares, Bob doesn't get: it reaches his Club, but he doesn't act.
    alice
        .neighbors
        .set_extension(id, NAPPLET_UPDATES, true, true);
    bob.neighbors
        .set_extension(id, NAPPLET_UPDATES, false, false);
    let ignored = update();
    alice
        .neighbors
        .share_update(NAPPLET_UPDATES, ignored.clone(), &[])
        .await;
    assert!(
        arrived_event(&bob.neighbors, id, &ignored).await,
        "kept in the neighborhood"
    );
    assert!(rx.try_recv().is_err(), "but not applied");

    // Bob gets again, without sharing himself: one-way is fine.
    bob.neighbors
        .set_extension(id, NAPPLET_UPDATES, false, true);
    bob.neighbors
        .set_extension(id, NAPPLET_UPDATES, true, false);
    let taken = update();
    alice
        .neighbors
        .share_update(NAPPLET_UPDATES, taken.clone(), &[])
        .await;
    let got = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("Bob was told")
        .unwrap();
    assert_eq!(got, taken.id);
}

async fn arrived_event(n: &Neighbors, id: super::id::NeighborhoodId, event: &nostr::Event) -> bool {
    let filter = vec![Filter::new().id(event.id)];
    for _ in 0..100 {
        if !n.link_backlog(id, &filter).await.is_empty() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread")]
async fn a_napplet_update_is_pushed_and_its_files_fetched_over_the_link() {
    use super::extensions::NAPPLET_UPDATES;
    use nsite_deck::{BlobStore as _, PeerSource as _};
    let (alice, bob, _, _) = linked_club().await;
    let id = bob.neighbors.records()[0].id;

    // Alice holds the new version's file in her own store, not the
    // neighborhood's.
    let kept = Arc::new(nsite_deck::testing::MemBlobs::new());
    let file = b"<html>v2</html>".to_vec();
    let sha = kept.put(&file).await.unwrap();
    alice.neighbors.set_kept_blobs(kept);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    bob.neighbors.on_update(Arc::new(move |n, _ext, ev| {
        let _ = tx.send((n, ev));
    }));

    let manifest = EventBuilder::new(Kind::from(myco_napplet_runtime::KIND_NAMED), "")
        .tags([nostr::Tag::identifier("chat")])
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let sent = alice
        .neighbors
        .share_update(
            NAPPLET_UPDATES,
            manifest.clone(),
            std::slice::from_ref(&sha),
        )
        .await;
    assert_eq!(sent, 1, "published in the Club");

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
async fn an_nsite_update_travels_under_nsite_updates() {
    use super::extensions::NSITE_UPDATES;
    let (alice, bob, _, _) = linked_club().await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    bob.neighbors.on_update(Arc::new(move |_n, ext, ev| {
        let _ = tx.send((ext, ev.id));
    }));
    let manifest = EventBuilder::new(Kind::from(nsite_deck::KIND_NAMED), "")
        .tags([nostr::Tag::identifier("blog")])
        .sign_with_keys(&Keys::generate())
        .unwrap();
    assert_eq!(
        alice
            .neighbors
            .share_update(NSITE_UPDATES, manifest.clone(), &[])
            .await,
        1
    );
    let got = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("Bob was told")
        .unwrap();
    assert_eq!(got, (NSITE_UPDATES, manifest.id));
}

/// An app this phone doesn't have yet is found in the neighborhoods that
/// carry its kind: the manifest asked of the links, then its file.
#[tokio::test(flavor = "multi_thread")]
async fn an_app_not_here_is_found_through_the_neighborhoods() {
    use super::extensions::NSITE_UPDATES;
    use nsite_deck::PeerSource as _;
    let (alice, bob, _, _) = linked_club().await;
    let id = alice.neighbors.records()[0].id;
    bob.neighbors.set_extension(id, NSITE_UPDATES, false, true);

    // Alice holds a site in the Club; Bob has never heard of it.
    let kept = Arc::new(nsite_deck::testing::MemBlobs::new());
    let file = b"<h1>blog</h1>".to_vec();
    let sha = nsite_deck::BlobStore::put(&*kept, &file).await.unwrap();
    alice.neighbors.set_kept_blobs(kept);
    let author = Keys::generate();
    let manifest = EventBuilder::new(Kind::from(nsite_deck::KIND_NAMED), "")
        .tags([nostr::Tag::identifier("blog")])
        .sign_with_keys(&author)
        .unwrap();
    alice
        .neighbors
        .scoped
        .share_kept(id, std::slice::from_ref(&sha))
        .unwrap();
    alice
        .neighbors
        .scoped
        .add_shared(id, manifest.clone())
        .await
        .unwrap();

    let source = super::updates::ExtensionSource {
        neighbors: Arc::downgrade(&bob.neighbors),
        extension: NSITE_UPDATES,
    };
    let found = source
        .fetch_manifest(&author.public_key(), Some("blog"))
        .await
        .unwrap();
    assert_eq!(found.map(|e| e.id), Some(manifest.id));
    assert_eq!(source.fetch_blob(&sha, &[]).await.unwrap(), Some(file));

    // A neighborhood Bob doesn't get nsites from is not asked.
    bob.neighbors.set_extension(id, NSITE_UPDATES, false, false);
    assert!(source
        .fetch_manifest(&author.public_key(), Some("blog"))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_napplet_held_but_never_pushed_is_exposed_where_updates_are_sent() {
    use super::extensions::NAPPLET_UPDATES;
    use nsite_deck::{BlobStore as _, RelayBackend as _};
    let (alice, bob, _, bob_addr) = linked_club().await;
    let id = bob.neighbors.records()[0].id;

    // Alice holds a napplet (its manifest and file), never shared.
    let held = myco_napplet_runtime::testing::build_test_napplet();
    let relay = Arc::new(nsite_deck::testing::MemRelay::new());
    let blobs = Arc::new(nsite_deck::testing::MemBlobs::new());
    relay.publish(held.manifest.clone()).await.unwrap();
    for (_, bytes) in &held.blobs {
        blobs.put(bytes).await.unwrap();
    }
    // A manifest whose file is not here is not exposed.
    let missing = myco_napplet_runtime::testing::NappletBuilder::new()
        .d_tag(Some("elsewhere"))
        .files(&[("/index.html", b"<!doctype html><p>not here</p>".as_slice())])
        .build();
    relay.publish(missing.manifest.clone()).await.unwrap();
    alice.neighbors.set_held(super::updates::HeldNapplets {
        relay,
        blobs,
        installed: installed(&[&held.manifest, &missing.manifest]),
    });

    let napplets = vec![Filter::new().kinds([
        Kind::from(myco_napplet_runtime::KIND_ROOT),
        Kind::from(myco_napplet_runtime::KIND_NAMED),
    ])];
    // Held open while it is followed: dropping it stops the pull.
    let _feed = bob
        .neighbors
        .subscribe(&app(), &bob_addr, napplets.clone())
        .await
        .unwrap();
    let mut got = Vec::new();
    for _ in 0..100 {
        got = bob.neighbors.kept(id, &napplets).await;
        if got.iter().any(|e| e.id == held.manifest.id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        got.iter().any(|e| e.id == held.manifest.id),
        "Bob's pull brought it"
    );
    assert!(
        !got.iter().any(|e| e.id == missing.manifest.id),
        "files not here: not exposed"
    );

    let (sha, bytes) = held.blobs[0].clone();
    assert_eq!(
        bob.neighbors.blob(&app(), &bob_addr, &sha).await.unwrap(),
        Some(bytes),
        "and its file"
    );

    // With sending off, Alice exposes it no more.
    alice
        .neighbors
        .set_extension(id, NAPPLET_UPDATES, true, false);
    assert!(!alice
        .neighbors
        .link_backlog(id, &napplets)
        .await
        .iter()
        .any(|e| e.id == held.manifest.id));
    assert_eq!(alice.neighbors.link_blob(id, &sha).await, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn whether_a_neighbor_has_a_blob_is_asked_without_fetching_it() {
    let (alice, bob, alice_addr, bob_addr) = linked_club().await;
    let bytes = b"poster".to_vec();
    let sha = sha256_hex(&bytes);
    alice
        .neighbors
        .upload(&app(), &alice_addr, bytes, &sha)
        .await
        .unwrap();

    assert_eq!(
        bob.neighbors.has(&app(), &bob_addr, &sha).await.unwrap(),
        Some(true),
        "Alice has it"
    );
    let id = bob.neighbors.records()[0].id;
    assert!(
        !bob.neighbors.link_has(id, &sha).await,
        "and asking did not fetch it"
    );
    assert_eq!(
        bob.neighbors
            .has(&app(), &bob_addr, &sha256_hex(b"nobody"))
            .await
            .unwrap(),
        Some(false),
        "nobody has this one"
    );
}

// --- Nearby -------------------------------------------------------------------

/// Two phones in reach of each other, Nearby on, with their fake radios;
/// `alice` has the lower device npub, so she dials.
struct Pair {
    alice: Device,
    bob: Device,
    alice_radio: Arc<Fake>,
    bob_radio: Arc<Fake>,
}

const ALICE: &str = "npub1alice";
const BOB: &str = "npub1bob";

async fn pair() -> Pair {
    let (alice_radio, bob_radio) = (Arc::new(Fake::default()), Arc::new(Fake::default()));
    let alice = device_with(alice_radio.clone(), true).await;
    let bob = device_with(bob_radio.clone(), true).await;
    Pair {
        alice,
        bob,
        alice_radio,
        bob_radio,
    }
}

fn direct(npub: &str) -> DirectPeer {
    DirectPeer {
        npub: npub.into(),
        reach: Reach::Radio,
    }
}

impl Pair {
    /// Each radio sees the other.
    fn in_reach(&self) {
        self.alice_radio.see(vec![direct(BOB)]);
        self.bob_radio.see(vec![direct(ALICE)]);
        self.hear_adverts();
    }

    /// Each hears the other's current nearby secret, as from a BLE advert.
    fn hear_adverts(&self) {
        let now = std::time::Instant::now();
        let (a, b) = (&self.alice.svc.nearby, &self.bob.svc.nearby);
        a.hear(b.secret(now), now);
        b.hear(a.secret(now), now);
    }

    /// Alice's tick, dialing Bob's loopback service.
    async fn alice_tick(&self) {
        let base = self.bob.svc.bases[0].clone();
        super::nearby::tick(
            &self.alice.svc,
            ALICE,
            &move |_| base.clone(),
            std::time::Instant::now(),
        )
        .await;
    }

    async fn bob_tick(&self) {
        let base = self.alice.svc.bases[0].clone();
        super::nearby::tick(
            &self.bob.svc,
            BOB,
            &move |_| base.clone(),
            std::time::Instant::now(),
        )
        .await;
    }
}

fn join(d: &Device, name: &str, auto_accept: bool) -> super::id::NeighborhoodId {
    d.neighbors.create_local(name, auto_accept).id
}

#[tokio::test(flavor = "multi_thread")]
async fn devices_in_reach_link_and_carry_only_the_auto_accepted_neighborhoods_both_have() {
    let p = pair().await;
    let lowlands = join(&p.alice, "Lowlands", true);
    join(&p.alice, "Club", false);
    join(&p.bob, "Lowlands", true);
    join(&p.bob, "Other", true);
    p.in_reach();

    // Bob has the higher npub: he waits for Alice.
    p.bob_tick().await;
    assert!(
        p.bob.svc.links.all().is_empty(),
        "the higher npub never dials"
    );

    p.alice_tick().await;
    let (a, b) = (p.alice.svc.links.all(), p.bob.svc.links.all());
    assert_eq!((a.len(), b.len()), (1, 1), "one link, both sides");
    let both = std::collections::BTreeSet::from([super::nearby::id(), lowlands]);
    assert_eq!(a[0].neighborhoods, both);
    assert_eq!(b[0].neighborhoods, both, "not Club, not Other");
    assert_eq!(a[0].ephemeral.as_ref().unwrap().device, BOB);
    assert_eq!(b[0].ephemeral.as_ref().unwrap().device, ALICE);
    assert_eq!(a[0].name, None, "nearby offers carry no name");
    assert_eq!(b[0].name, None);

    // A second tick links nobody twice.
    p.alice_tick().await;
    p.bob_tick().await;
    assert_eq!(p.alice.svc.links.all().len(), 1);
    assert_eq!(p.bob.svc.links.all().len(), 1);

    // Napplets see Bob in nearby, on a proximity path.
    let nearby = p
        .alice
        .neighbors
        .list(&app())
        .await
        .into_iter()
        .find(|n| n.name == "nearby")
        .unwrap();
    let peers = p.alice.neighbors.peers(&app(), &nearby.id).await.unwrap();
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].name, None, "nameless");
    assert_eq!(
        peers[0].link,
        Some(myco_napplet_runtime::nap::neighbor::LinkClass::Proximity)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_nearby_setup_from_a_device_not_in_reach_is_refused() {
    let p = pair().await;
    // Alice's radio sees Bob; Bob's sees nobody, so Alice isn't direct to him.
    p.alice_radio.see(vec![direct(BOB)]);
    p.alice_tick().await;
    assert!(p.alice.svc.links.all().is_empty());
    assert!(p.bob.svc.links.all().is_empty(), "refused");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_nearby_setup_without_the_source_address_is_refused() {
    // Served without connect info, Bob can't tell where a request came from.
    let alice_radio = Arc::new(Fake::default());
    let alice = device_with(alice_radio.clone(), true).await;
    let bob_radio = Arc::new(Fake::default());
    let bob = device_with(bob_radio.clone(), false).await;
    alice_radio.see(vec![direct(BOB)]);
    bob_radio.see(vec![direct(ALICE)]);
    let base = bob.svc.bases[0].clone();
    super::nearby::tick(
        &alice.svc,
        ALICE,
        &move |_| base.clone(),
        std::time::Instant::now(),
    )
    .await;
    assert!(bob.svc.links.all().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_auto_accepted_neighborhood_joined_later_is_added_to_the_link() {
    let p = pair().await;
    p.in_reach();
    p.alice_tick().await;
    assert_eq!(p.alice.svc.links.all()[0].neighborhoods.len(), 1);

    let fest = join(&p.alice, "Festival", true);
    p.alice_tick().await;
    assert!(
        !p.alice.svc.links.all()[0].neighborhoods.contains(&fest),
        "Bob doesn't carry it yet"
    );
    // Bob joins it too; his tick offers it, and Alice takes it.
    join(&p.bob, "Festival", true);
    p.bob_tick().await;
    assert!(p.bob.svc.links.all()[0].neighborhoods.contains(&fest));
    assert!(p.alice.svc.links.all()[0].neighborhoods.contains(&fest));

    // A neighborhood Bob doesn't carry is never taken, even when offered.
    let mine = join(&p.alice, "Mine only", true);
    p.alice_tick().await;
    assert!(!p.bob.svc.links.all()[0].neighborhoods.contains(&mine));
}

#[tokio::test(flavor = "multi_thread")]
async fn turning_nearby_off_removes_the_neighborhood_and_ends_its_links() {
    let p = pair().await;
    p.in_reach();
    p.alice_tick().await;
    assert_eq!(p.bob.svc.links.all().len(), 1);

    p.bob.neighbors.set_nearby(false);
    p.bob_tick().await;
    assert!(p.bob.svc.links.all().is_empty(), "its links ended");
    assert!(!p
        .bob
        .neighbors
        .records()
        .iter()
        .any(|r| r.id == super::nearby::id()));
    // And while off, Bob refuses new ones.
    for l in p.alice.svc.links.all() {
        p.alice.svc.links.end(&l.id);
    }
    p.alice_tick().await;
    assert!(p.alice.svc.links.all().is_empty());
    assert!(p.bob.svc.links.all().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_nearby_peer_gone_past_the_grace_period_is_dropped_on_both_sides() {
    let p = pair().await;
    p.in_reach();
    p.alice_tick().await;
    p.alice_radio.see(Vec::new());
    p.bob_radio.see(Vec::new());
    let later = std::time::Instant::now() + super::nearby::GRACE + Duration::from_secs(1);
    let base = p.bob.svc.bases[0].clone();
    super::nearby::tick(&p.alice.svc, ALICE, &move |_| base.clone(), later).await;
    let base = p.alice.svc.bases[0].clone();
    super::nearby::tick(&p.bob.svc, BOB, &move |_| base.clone(), later).await;
    assert!(p.alice.svc.links.all().is_empty());
    assert!(p.bob.svc.links.all().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn without_hearing_their_secret_nobody_links() {
    let p = pair().await;
    p.alice_radio.see(vec![direct(BOB)]);
    p.bob_radio.see(vec![direct(ALICE)]);
    p.alice_tick().await;
    assert!(p.alice.svc.links.all().is_empty());
    assert!(p.bob.svc.links.all().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn told_a_secret_was_used_the_dialer_tries_again_with_the_next() {
    let p = pair().await;
    p.in_reach();
    // Someone else got in with the secret Alice heard: Bob has a new one,
    // which Alice hears a moment after her first try.
    p.bob.svc.nearby.renew(std::time::Instant::now());
    let (a, b) = (p.alice.svc.nearby.clone(), p.bob.svc.nearby.clone());
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let now = std::time::Instant::now();
        a.hear(b.secret(now), now);
    });
    p.alice_tick().await;
    assert_eq!(
        p.alice.svc.links.ephemeral().len(),
        1,
        "linked on the next secret"
    );
    assert_eq!(p.bob.svc.links.ephemeral().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn with_nobody_in_reach_nearby_forgets_at_once_and_meets_again_as_strangers() {
    let p = pair().await;
    p.in_reach();
    p.alice_tick().await;
    let first = p.alice.svc.links.ephemeral()[0].clone();

    // Nobody in reach: no grace period, everything forgotten.
    p.alice_radio.see(Vec::new());
    p.alice_tick().await;
    assert!(p.alice.svc.links.ephemeral().is_empty());
    assert!(!p.alice.svc.links.reachable(&first.id));

    // Back in reach: a new link on new keys, with no name either way.
    p.in_reach();
    p.alice_tick().await;
    let again = p.alice.svc.links.ephemeral();
    assert_eq!(again.len(), 1);
    assert_ne!(again[0].id, first.id);
    assert_ne!(again[0].keys().public_key(), first.keys().public_key());
    assert_eq!(again[0].name, None);
}

/// The Pixel/A52 case: an update shared while nobody was in reach reaches
/// a peer met afterwards, through the standing update subscription.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_shared_while_nobody_was_in_reach_arrives_when_they_meet() {
    use super::extensions::NAPPLET_UPDATES;
    let p = pair().await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    p.bob.neighbors.on_update(Arc::new(move |_n, ext, ev| {
        let _ = tx.send((ext, ev.id));
    }));
    let manifest = EventBuilder::new(Kind::from(myco_napplet_runtime::KIND_NAMED), "")
        .tags([nostr::Tag::identifier("chat")])
        .sign_with_keys(&Keys::generate())
        .unwrap();
    // Nobody around: kept in Alice's nearby neighborhood, sent to no link.
    p.alice
        .neighbors
        .share_update(NAPPLET_UPDATES, manifest.clone(), &[])
        .await;
    assert!(p.alice.svc.links.all().is_empty());

    p.in_reach();
    p.alice_tick().await;
    let got = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("Bob was told once they met")
        .unwrap();
    assert_eq!(got, (NAPPLET_UPDATES, manifest.id));
}

/// An update a napplet's own subscription brought in is applied too: every
/// way an update lands here ends at the same hand-off.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_pulled_for_a_napplet_subscription_is_offered() {
    let p = pair().await;
    let held = hold_napplet(&p.alice).await;
    let addr = nearby_addr(&p.bob).await;
    let mut feed = p
        .bob
        .neighbors
        .subscribe(&app(), &addr, napplet_kinds())
        .await
        .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    p.bob.neighbors.on_update(Arc::new(move |_n, _ext, ev| {
        let _ = tx.send(ev.id);
    }));
    p.in_reach();
    p.alice_tick().await;
    assert!(arrives(&mut feed, held.id).await, "the napplet saw it");
    let got = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("and the runtime was offered it")
        .unwrap();
    assert_eq!(got, held.id);
}

/// Bob holds a napplet he never shared, as `HeldNapplets` exposes it.
async fn hold_napplet(dev: &Device) -> nostr::Event {
    use nsite_deck::{BlobStore as _, RelayBackend as _};
    let held = myco_napplet_runtime::testing::build_test_napplet();
    let relay = Arc::new(nsite_deck::testing::MemRelay::new());
    let blobs = Arc::new(nsite_deck::testing::MemBlobs::new());
    relay.publish(held.manifest.clone()).await.unwrap();
    for (_, bytes) in &held.blobs {
        blobs.put(bytes).await.unwrap();
    }
    dev.neighbors.set_held(super::updates::HeldNapplets {
        relay,
        blobs,
        installed: installed(&[&held.manifest]),
    });
    held.manifest
}

fn napplet_kinds() -> Vec<Filter> {
    vec![Filter::new().kinds([
        Kind::from(myco_napplet_runtime::KIND_ROOT),
        Kind::from(myco_napplet_runtime::KIND_NAMED),
    ])]
}

async fn nearby_addr(dev: &Device) -> String {
    dev.neighbors
        .list(&app())
        .await
        .into_iter()
        .find(|n| n.name == "nearby")
        .unwrap()
        .address
        .unwrap()
}

async fn arrives(feed: &mut myco_napplet_runtime::nap::neighbor::Feed, id: nostr::EventId) -> bool {
    use futures_util::StreamExt as _;
    feed.backlog.iter().any(|e| e.id == id)
        || tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(e) = feed.live.next().await {
                if e.id == id {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
}

#[tokio::test(flavor = "multi_thread")]
async fn around_you_open_before_a_peer_comes_in_reach_sees_its_napplets() {
    // The one who dials listens.
    let p = pair().await;
    let held = hold_napplet(&p.bob).await;
    let addr = nearby_addr(&p.alice).await;
    let mut feed = p
        .alice
        .neighbors
        .subscribe(&app(), &addr, napplet_kinds())
        .await
        .unwrap();
    p.in_reach();
    p.alice_tick().await;
    assert!(
        arrives(&mut feed, held.id).await,
        "dialer sees answerer's napplet"
    );

    // The one who answers listens.
    let p = pair().await;
    let held = hold_napplet(&p.alice).await;
    let addr = nearby_addr(&p.bob).await;
    let mut feed = p
        .bob
        .neighbors
        .subscribe(&app(), &addr, napplet_kinds())
        .await
        .unwrap();
    p.in_reach();
    p.alice_tick().await;
    assert!(
        arrives(&mut feed, held.id).await,
        "answerer sees dialer's napplet"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_links_relay_sockets_are_capped_and_close_when_the_link_ends() {
    use futures_util::StreamExt as _;
    let (alice, bob, _, _) = linked_club().await;
    let id = club_id(&alice);
    let link = bob.svc.links.all().remove(0);
    let url = super::wire::scoped(&super::wire::relay_urls(&link)[0], id, 0);
    let mut sockets = Vec::new();
    for _ in 0..super::link::MAX_RELAY_SOCKETS {
        sockets.push(super::wire::open(&link, &url).await.unwrap());
    }
    assert!(
        super::wire::open(&link, &url).await.is_err(),
        "one socket too many is refused"
    );
    // Alice ends the link on her side: every socket closes, without waiting
    // for the peer to say anything.
    let theirs = alice.svc.links.all().remove(0);
    assert!(alice.svc.links.end(&theirs.id));
    for mut ws in sockets {
        let closed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match ws.next().await {
                    None | Some(Err(_)) => return,
                    Some(Ok(m)) if m.is_close() => return,
                    Some(Ok(_)) => {}
                }
            }
        })
        .await;
        assert!(closed.is_ok(), "the socket closed with the link");
    }
}

// --- What a peer is given -------------------------------------------------------

/// The napplet slots of `manifests`, as the Library would list them.
fn installed(manifests: &[&nostr::Event]) -> super::updates::held::Installed {
    let slots: Vec<(nostr::PublicKey, Option<String>)> = manifests
        .iter()
        .map(|m| {
            let parsed = myco_napplet_runtime::NappletManifest::from_event((*m).clone()).unwrap();
            (parsed.author, parsed.d_tag)
        })
        .collect();
    Arc::new(move || slots.clone())
}

/// Alice — Bob — Carol in one Club: Bob linked to both, Alice and Carol not
/// to each other. Alice's address and Carol's.
async fn chain() -> (Device, Device, Device, String, String) {
    let (alice, bob, carol) = (device().await, device().await, device().await);
    let a = club(&alice).await;
    join_club(&alice, &bob).await;
    let c = join_club(&bob, &carol).await;
    (alice, bob, carol, a, c)
}

/// Whether `text` is kept in `n`'s own view on this device, within a few
/// seconds.
async fn kept_here(n: &Neighbors, id: super::id::NeighborhoodId, text: &str) -> bool {
    for _ in 0..100 {
        if n.kept(id, &kind1()).await.iter().any(|e| e.content == text) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// The texts `feed` brings until `stop` comes (or 10 s pass).
async fn until(feed: &mut myco_napplet_runtime::nap::neighbor::Feed, stop: &str) -> Vec<String> {
    use futures_util::StreamExt as _;
    let mut seen: Vec<String> = feed.backlog.iter().map(|e| e.content.clone()).collect();
    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        while !seen.iter().any(|s| s == stop) {
            match feed.live.next().await {
                Some(e) => seen.push(e.content),
                None => return,
            }
        }
    })
    .await;
    seen
}

/// Bob holds `marker` as his own; Carol's subscription on Bob is open once
/// it has come. Returns her feed.
async fn carol_listens(
    bob: &Device,
    carol: &Device,
    carol_addr: &str,
) -> myco_napplet_runtime::nap::neighbor::Feed {
    let id = club_id(bob);
    bob.neighbors
        .scoped
        .add_shared(id, note("marker"))
        .await
        .unwrap();
    let mut feed = carol
        .neighbors
        .subscribe(&app(), carol_addr, kind1())
        .await
        .unwrap();
    assert!(
        until(&mut feed, "marker")
            .await
            .iter()
            .any(|s| s == "marker"),
        "Carol follows Bob"
    );
    feed
}

/// Close (one hop, nothing passed on): Alice's post reaches Bob, and Bob
/// gives it to nobody — not in a backlog, not following an open `REQ`.
#[tokio::test(flavor = "multi_thread")]
async fn close_a_post_stops_at_the_next_device() {
    let (alice, bob, carol, alice_addr, carol_addr) = chain().await;
    let id = club_id(&alice);
    for d in [&alice, &bob, &carol] {
        d.neighbors.set_policy(id, 1, 1, false);
    }
    let mut feed = carol_listens(&bob, &carol, &carol_addr).await;

    alice
        .neighbors
        .publish(&app(), &alice_addr, note("from alice"))
        .await
        .unwrap();
    assert!(
        kept_here(&bob.neighbors, id, "from alice").await,
        "Bob has it"
    );
    // Bob's own post after it, on the same open REQ: once it is in, Alice's
    // would have been too.
    bob.neighbors
        .scoped
        .add_shared(id, note("after"))
        .await
        .unwrap();
    let seen = until(&mut feed, "after").await;
    assert!(seen.iter().any(|s| s == "after"), "{seen:?}");
    assert!(
        !seen.iter().any(|s| s == "from alice"),
        "not live through Bob: {seen:?}"
    );
    assert!(
        !bob.neighbors
            .link_backlog(id, &kind1())
            .await
            .iter()
            .any(|e| e.content == "from alice"),
        "not in Bob's backlog"
    );
    assert!(!carol
        .neighbors
        .kept(id, &kind1())
        .await
        .iter()
        .any(|e| e.content == "from alice"));
}

/// Wide (the default: three hops, passed on): Alice's post reaches Carol
/// through Bob, and Bob serves it to anyone asking. What Carol only pulled
/// she passes on to nobody.
#[tokio::test(flavor = "multi_thread")]
async fn wide_a_post_travels_on_within_its_hops() {
    let (alice, bob, carol, alice_addr, carol_addr) = chain().await;
    let id = club_id(&alice);
    let mut feed = carol_listens(&bob, &carol, &carol_addr).await;
    alice
        .neighbors
        .publish(&app(), &alice_addr, note("from alice"))
        .await
        .unwrap();
    let seen = until(&mut feed, "from alice").await;
    assert!(seen.iter().any(|s| s == "from alice"), "{seen:?}");
    assert!(
        bob.neighbors
            .link_backlog(id, &kind1())
            .await
            .iter()
            .any(|e| e.content == "from alice"),
        "in Bob's backlog"
    );
    let pulled = carol
        .neighbors
        .kept(id, &kind1())
        .await
        .into_iter()
        .find(|e| e.content == "marker")
        .unwrap();
    assert!(!carol
        .neighbors
        .link_backlog(id, &kind1())
        .await
        .iter()
        .any(|e| e.id == pulled.id));
}

/// With an extension's sharing off, its events leave this device by no path.
#[tokio::test(flavor = "multi_thread")]
async fn an_extension_not_shared_is_not_forwarded_followed_or_served() {
    use super::extensions::NSITE_UPDATES;
    let (alice, bob, alice_addr, bob_addr) = linked_club().await;
    let id = club_id(&alice);
    alice
        .neighbors
        .set_extension(id, NSITE_UPDATES, true, false);
    let site = EventBuilder::new(Kind::from(nsite_deck::KIND_NAMED), "")
        .tags([nostr::Tag::identifier("blog")])
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let both = vec![
        Filter::new().kind(Kind::TextNote),
        Filter::new().kind(Kind::from(nsite_deck::KIND_NAMED)),
    ];
    alice
        .neighbors
        .scoped
        .add_shared(id, note("marker"))
        .await
        .unwrap();
    let mut feed = bob
        .neighbors
        .subscribe(&app(), &bob_addr, both.clone())
        .await
        .unwrap();
    assert!(until(&mut feed, "marker")
        .await
        .iter()
        .any(|s| s == "marker"));

    // Published (forwarded, and live on Bob's REQ), then a note after it on
    // the same REQ.
    alice
        .neighbors
        .publish(&app(), &alice_addr, site.clone())
        .await
        .unwrap();
    alice
        .neighbors
        .scoped
        .add_shared(id, note("after"))
        .await
        .unwrap();
    assert!(until(&mut feed, "after").await.iter().any(|s| s == "after"));
    assert!(bob
        .neighbors
        .kept(id, &both)
        .await
        .iter()
        .all(|e| e.id != site.id));
    assert!(alice
        .neighbors
        .link_backlog(id, &both)
        .await
        .iter()
        .all(|e| e.id != site.id));
    // Sharing on again: served.
    alice.neighbors.set_extension(id, NSITE_UPDATES, true, true);
    assert!(alice
        .neighbors
        .link_backlog(id, &both)
        .await
        .iter()
        .any(|e| e.id == site.id));
}

/// An offer without relay reads refuses a peer's `REQ`; one without relay
/// writes refuses its `EVENT`.
#[tokio::test(flavor = "multi_thread")]
async fn the_offer_decides_whether_peers_read_and_write() {
    use myco_napplet_runtime::nap::neighbor::Offer;
    let (alice, bob, _, _) = linked_club().await;
    let id = club_id(&alice);
    alice
        .neighbors
        .scoped
        .add_shared(id, note("held"))
        .await
        .unwrap();
    assert_eq!(bob.neighbors.ask_links(id, &kind1()).await.len(), 1);
    let link = bob.svc.links.carrying(id).remove(0);
    let token = alice.neighbors.list(&app()).await.remove(0).id;

    let writes_only = Offer {
        relay_write: true,
        ..Offer::default()
    };
    alice
        .neighbors
        .set_offer(&app(), &token, writes_only)
        .await
        .unwrap();
    assert!(
        bob.neighbors.ask_links(id, &kind1()).await.is_empty(),
        "no reads"
    );
    assert!(super::wire::send(&link, id, 0, &note("in")).await.unwrap());

    let reads_only = Offer {
        relay_read: true,
        ..Offer::default()
    };
    alice
        .neighbors
        .set_offer(&app(), &token, reads_only)
        .await
        .unwrap();
    assert!(
        !super::wire::send(&link, id, 0, &note("refused"))
            .await
            .unwrap(),
        "no writes"
    );
    assert!(!alice
        .neighbors
        .kept(id, &kind1())
        .await
        .iter()
        .any(|e| e.content == "refused"));
    assert!(!bob.neighbors.ask_links(id, &kind1()).await.is_empty());
}

/// A napplet only cached here, not in the Library, is not exposed.
#[tokio::test(flavor = "multi_thread")]
async fn a_napplet_not_installed_is_not_exposed() {
    use nsite_deck::{BlobStore as _, RelayBackend as _};
    let (alice, _bob, _, _) = linked_club().await;
    let id = club_id(&alice);
    let held = myco_napplet_runtime::testing::build_test_napplet();
    let relay = Arc::new(nsite_deck::testing::MemRelay::new());
    let blobs = Arc::new(nsite_deck::testing::MemBlobs::new());
    relay.publish(held.manifest.clone()).await.unwrap();
    for (_, bytes) in &held.blobs {
        blobs.put(bytes).await.unwrap();
    }
    alice.neighbors.set_held(super::updates::HeldNapplets {
        relay,
        blobs,
        installed: installed(&[]),
    });
    assert!(alice
        .neighbors
        .link_backlog(id, &napplet_kinds())
        .await
        .is_empty());
    let (sha, _) = held.blobs[0].clone();
    assert_eq!(alice.neighbors.link_blob(id, &sha).await, None);
}
