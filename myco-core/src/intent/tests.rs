//! NAP-INTENT on the host: resolution through a live host, delivery timing,
//! the Library catalog and the nsite built-in.

use super::*;
use myco_napplet_runtime::nap::intent::{
    IntentCatalogSnapshot, NO_HANDLER, UNSUPPORTED_ACTION, UNSUPPORTED_CONVENTION,
};
use myco_napplet_runtime::testing::NappletBuilder;
use nostr::nips::nip19::ToBech32;
use nsite_deck::seams::{BlobStore, RelayBackend};
use nsite_deck::testing::{MemBlobs, MemRelay};
use serde_json::json;

/// A catalog a test can change under the host.
struct TestIntents(Mutex<IntentCatalogSnapshot>);

#[async_trait::async_trait]
impl IntentCatalog for TestIntents {
    async fn snapshot(&self) -> IntentCatalogSnapshot {
        self.0.lock().unwrap().clone()
    }
}

struct Fixture {
    host: NappletHost,
    intents: Arc<TestIntents>,
    feed: NappletAddr,
    profiles: NappletAddr,
    cards: NappletAddr,
}

fn key_of(addr: &NappletAddr) -> String {
    handler_key(&addr.author.to_bech32().unwrap(), addr.d_tag.as_deref())
}

fn handler_for(addr: &NappletAddr, title: &str) -> IntentHandler {
    IntentHandler {
        key: key_of(addr),
        d_tag: addr.d_tag.clone().unwrap_or_default(),
        title: Some(title.to_string()),
        pointer: key_of(addr),
        archetypes: vec![Archetype {
            slug: "profile".into(),
            convention: "napplet:profile/open".into(),
        }],
    }
}

impl Fixture {
    /// Only `profiles` handles `profile` (plus Myco's built-ins).
    fn only_profiles(&self) {
        let mut handlers = vec![handler_for(&self.profiles, "Profiles")];
        handlers.extend(builtin_handlers());
        *self.intents.0.lock().unwrap() = IntentCatalogSnapshot {
            handlers,
            defaults: BTreeMap::new(),
        };
    }

    /// `profiles` and `cards` both handle `profile`; `default` names one.
    fn both(&self, default: Option<&NappletAddr>) {
        let mut handlers = vec![
            handler_for(&self.profiles, "Profiles"),
            handler_for(&self.cards, "Cards"),
        ];
        handlers.extend(builtin_handlers());
        *self.intents.0.lock().unwrap() = IntentCatalogSnapshot {
            handlers,
            defaults: default
                .map(|a| BTreeMap::from([("profile".to_string(), key_of(a))]))
                .unwrap_or_default(),
        };
    }

    async fn open(&self, addr: &NappletAddr) -> String {
        let opened = self
            .host
            .open(addr, Some(crate::napplet::effective_grants(&[])))
            .await
            .unwrap();
        self.host
            .frame(
                &opened.session_id,
                r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#,
            )
            .await;
        opened.session_id
    }

    async fn send(&self, session: &str, message: Value) -> Vec<ToShell> {
        let frame = json!({"channel": "napplet", "message": message}).to_string();
        self.host.frame(session, &frame).await
    }

    async fn invoke(&self, session: &str, request: Value) -> Vec<ToShell> {
        // Clear the per-window cooldown: these tests invoke in quick
        // succession on purpose. `cooldown_limits_a_window` tests it.
        self.host
            .sweep_intents(Instant::now() + INVOKE_COOLDOWN + Duration::from_millis(1));
        self.send(
            session,
            json!({"type": "intent.invoke", "id": "i1", "request": request}),
        )
        .await
    }

    async fn subscribe(&self, session: &str, topic: &str) -> Vec<ToShell> {
        self.send(
            session,
            json!({"type": "inc.subscribe", "id": "s1", "topic": topic}),
        )
        .await
    }

    async fn drain(&self, session: &str) -> Vec<ToShell> {
        self.host
            .next_frames(session, Duration::from_millis(30))
            .await
    }
}

async fn fixture() -> Fixture {
    let relay = Arc::new(MemRelay::new());
    let blobs = Arc::new(MemBlobs::new());
    let mut addrs = Vec::new();
    for (d, title) in [
        ("feed", "Feed"),
        ("profiles", "Profiles"),
        ("cards", "Cards"),
    ] {
        let napplet = NappletBuilder::new()
            .d_tag(Some(d))
            .title(title)
            .archetype("profile", "napplet:profile/open")
            .build();
        for (_, bytes) in &napplet.blobs {
            blobs.put(bytes).await.unwrap();
        }
        relay.publish(napplet.manifest.clone()).await.unwrap();
        addrs.push(NappletAddr {
            author: napplet.author,
            d_tag: Some(d.to_string()),
            relays: Vec::new(),
        });
    }
    let intents = Arc::new(TestIntents(Mutex::new(IntentCatalogSnapshot::default())));
    let (mut ctx, _signer) = myco_napplet_runtime::testing::test_context();
    ctx.relay = relay;
    ctx.blobs = blobs.clone();
    ctx.kept_blobs = blobs;
    ctx.intents = intents.clone();
    let mut addrs = addrs.into_iter();
    let fixture = Fixture {
        host: NappletHost::new(ctx),
        intents,
        feed: addrs.next().unwrap(),
        profiles: addrs.next().unwrap(),
        cards: addrs.next().unwrap(),
    };
    fixture.only_profiles();
    fixture
}

/// The napplet messages among `frames`, as JSON.
fn messages(frames: &[ToShell]) -> Vec<Value> {
    frames
        .iter()
        .filter_map(|f| match f {
            ToShell::Napplet { message } => Some(serde_json::to_value(message).unwrap()),
            _ => None,
        })
        .collect()
}

fn of_type(frames: &[ToShell], msg_type: &str) -> Vec<Value> {
    messages(frames)
        .into_iter()
        .filter(|m| m["type"] == msg_type)
        .collect()
}

fn open_command(frames: &[ToShell]) -> Option<(String, String)> {
    frames.iter().find_map(|f| match f {
        ToShell::OpenNapplet { pointer, token, .. } => Some((pointer.clone(), token.clone())),
        _ => None,
    })
}

const PROFILE: &str = "napplet:profile/open";

/// The whole path, in the order a device takes it: the payload waits while
/// the handler's window opens and its napplet boots, the caller hears back
/// once the window exists, and the payload is handed over exactly once, after
/// the handler subscribes.
#[tokio::test]
async fn a_sole_handler_opens_and_hears_the_payload_after_it_subscribes() {
    let f = fixture().await;
    let feed = f.open(&f.feed).await;

    let out = f
        .invoke(
            &feed,
            json!({"archetype": "profile", "payload": {"pubkey": "abc"}}),
        )
        .await;
    let (pointer, token) = open_command(&out).expect("the handler's window is opened");
    assert_eq!(pointer, key_of(&f.profiles));
    assert!(
        of_type(&out, "intent.invoke.result").is_empty(),
        "answered before the window exists"
    );

    // The window opens and binds the token; its napplet has not subscribed.
    let profiles = f.open(&f.profiles).await;
    assert!(f.host.bind_intent(&profiles, &token).await);
    let results = of_type(&f.drain(&feed).await, "intent.invoke.result");
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0]["result"],
        json!({"ok": true, "archetype": "profile", "action": "open", "handled": true,
               "handler": "profiles", "windowId": profiles, "convention": PROFILE})
    );
    assert!(
        of_type(&f.drain(&profiles).await, "inc.event").is_empty(),
        "delivered before the napplet listened"
    );

    // It subscribes: the result, then the payload, from the caller.
    let out = f.subscribe(&profiles, PROFILE).await;
    let msgs = messages(&out);
    assert_eq!(msgs[0]["type"], "inc.subscribe.result");
    assert_eq!(
        msgs[1],
        json!({"type": "inc.event", "topic": PROFILE, "sender": "feed",
               "payload": {"pubkey": "abc"}})
    );
    // Once.
    assert!(of_type(&f.subscribe(&profiles, PROFILE).await, "inc.event").is_empty());
    assert!(of_type(&f.drain(&profiles).await, "inc.event").is_empty());
    assert_eq!(f.host.pending_intent_count(), 0);
}

/// A handler already running and listening hears the payload the moment its
/// window binds the token.
#[tokio::test]
async fn a_listening_handler_hears_it_at_once() {
    let f = fixture().await;
    let feed = f.open(&f.feed).await;
    let profiles = f.open(&f.profiles).await;
    f.subscribe(&profiles, PROFILE).await;

    let out = f
        .invoke(&feed, json!({"archetype": "profile", "payload": {"n": 1}}))
        .await;
    let (_, token) = open_command(&out).unwrap();
    assert!(f.host.bind_intent(&profiles, &token).await);
    let events = of_type(&f.drain(&profiles).await, "inc.event");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["payload"], json!({"n": 1}));
}

/// A token names one napplet. Offered to another's window it binds nothing,
/// and that napplet never hears the payload, listening or not.
#[tokio::test]
async fn only_the_resolved_handler_ever_gets_the_payload() {
    let f = fixture().await;
    let feed = f.open(&f.feed).await;
    let cards = f.open(&f.cards).await;
    f.subscribe(&cards, PROFILE).await;

    let out = f
        .invoke(
            &feed,
            json!({"archetype": "profile", "payload": {"secret": 1}}),
        )
        .await;
    let (_, token) = open_command(&out).unwrap();
    assert!(!f.host.bind_intent(&cards, &token).await, "another napplet");
    assert!(
        !f.host.bind_intent(&feed, &token).await,
        "the caller itself"
    );
    assert!(of_type(&f.drain(&cards).await, "inc.event").is_empty());

    let profiles = f.open(&f.profiles).await;
    assert!(f.host.bind_intent(&profiles, &token).await);
    assert!(!f.host.bind_intent(&profiles, &token).await, "bound once");
    assert_eq!(
        of_type(&f.subscribe(&profiles, PROFILE).await, "inc.event").len(),
        1
    );
    assert!(of_type(&f.drain(&cards).await, "inc.event").is_empty());
    assert!(!f.host.bind_intent(&cards, "not-a-token").await);
}

/// Several handlers and no default: the calling window is asked to show the
/// chooser, and the pick is opened for the caller.
#[tokio::test]
async fn several_handlers_ask_the_user_and_the_pick_is_opened() {
    let f = fixture().await;
    f.both(None);
    let feed = f.open(&f.feed).await;

    let out = f.invoke(&feed, json!({"archetype": "profile"})).await;
    let Some(ToShell::ChooseIntentHandler {
        token,
        archetype,
        candidates,
        ..
    }) = out.first().cloned()
    else {
        panic!("expected the chooser, got {out:?}");
    };
    assert_eq!(archetype, "profile");
    let keys: Vec<_> = candidates.iter().map(|c| c.key.clone()).collect();
    assert_eq!(keys, vec![key_of(&f.profiles), key_of(&f.cards)]);
    assert_eq!(
        f.host.intent_chooser_archetype(&token).as_deref(),
        Some("profile")
    );

    // A second invoke while the question is up is refused.
    let busy = f.invoke(&feed, json!({"archetype": "profile"})).await;
    assert_eq!(
        of_type(&busy, "intent.invoke.result")[0]["result"]["error"],
        INTENT_BUSY
    );

    assert!(f
        .host
        .answer_intent_chooser(&token, Some(&key_of(&f.cards))));
    let (pointer, _) = open_command(&f.drain(&feed).await).expect("the pick is opened");
    assert_eq!(pointer, key_of(&f.cards));
    assert!(!f.host.answer_intent_chooser(&token, None), "answered once");
}

#[tokio::test]
async fn a_cancelled_chooser_says_so() {
    let f = fixture().await;
    f.both(None);
    let feed = f.open(&f.feed).await;
    let out = f.invoke(&feed, json!({"archetype": "profile"})).await;
    let Some(ToShell::ChooseIntentHandler { token, .. }) = out.first().cloned() else {
        panic!("expected the chooser");
    };
    assert!(f.host.answer_intent_chooser(&token, None));
    let results = of_type(&f.drain(&feed).await, "intent.invoke.result");
    assert_eq!(results[0]["result"]["ok"], false);
    assert_eq!(results[0]["result"]["error"], USER_CANCELLED);

    // A pick that was not offered is a cancel too, never an open.
    let out = f.invoke(&feed, json!({"archetype": "profile"})).await;
    let Some(ToShell::ChooseIntentHandler { token, .. }) = out.first().cloned() else {
        panic!("expected the chooser");
    };
    assert!(f
        .host
        .answer_intent_chooser(&token, Some("npub1someoneelse:x")));
    let drained = f.drain(&feed).await;
    assert!(open_command(&drained).is_none());
    assert_eq!(
        of_type(&drained, "intent.invoke.result")[0]["result"]["error"],
        USER_CANCELLED
    );
}

/// The user's default is opened without asking; `handler: "choose"` and a
/// named handler still ask.
#[tokio::test]
async fn the_default_is_opened_and_choose_still_asks() {
    let f = fixture().await;
    f.both(Some(&f.cards));
    let feed = f.open(&f.feed).await;

    let (pointer, _) =
        open_command(&f.invoke(&feed, json!({"archetype": "profile"})).await).unwrap();
    assert_eq!(pointer, key_of(&f.cards));

    for handler in ["choose", "profiles"] {
        let out = f
            .invoke(&feed, json!({"archetype": "profile", "handler": handler}))
            .await;
        let Some(ToShell::ChooseIntentHandler { token, .. }) = out.first().cloned() else {
            panic!("{handler}: expected the chooser, got {out:?}");
        };
        f.host.answer_intent_chooser(&token, None);
        f.drain(&feed).await;
    }
}

#[tokio::test]
async fn no_handler_and_unsupported_requests_fail_at_once() {
    let f = fixture().await;
    let feed = f.open(&f.feed).await;
    for (request, error) in [
        (json!({"archetype": "pet"}), NO_HANDLER),
        (
            json!({"archetype": "profile", "action": "share"}),
            UNSUPPORTED_ACTION,
        ),
        (
            json!({"archetype": "profile", "convention": "napplet:person/open"}),
            UNSUPPORTED_CONVENTION,
        ),
    ] {
        let out = f.invoke(&feed, request.clone()).await;
        assert!(open_command(&out).is_none(), "{request}");
        let results = of_type(&out, "intent.invoke.result");
        assert_eq!(results[0]["result"]["error"], error, "{request}");
        assert_eq!(results[0]["result"]["handled"], false);
    }
    assert_eq!(f.host.pending_intent_count(), 0);
}

/// A window that never binds fails the caller when the payload expires; a
/// bound payload whose napplet never listens is dropped then, silently.
#[tokio::test]
async fn unclaimed_payloads_expire() {
    let f = fixture().await;
    let feed = f.open(&f.feed).await;

    let out = f.invoke(&feed, json!({"archetype": "profile"})).await;
    let (_, token) = open_command(&out).unwrap();
    f.host
        .sweep_intents(Instant::now() + PENDING_INTENT_TTL + Duration::from_secs(1));
    let results = of_type(&f.drain(&feed).await, "intent.invoke.result");
    assert_eq!(results[0]["result"]["error"], INVOKE_FAILED);
    let profiles = f.open(&f.profiles).await;
    assert!(!f.host.bind_intent(&profiles, &token).await, "expired");

    let out = f.invoke(&feed, json!({"archetype": "profile"})).await;
    let (_, token) = open_command(&out).unwrap();
    assert!(f.host.bind_intent(&profiles, &token).await);
    f.drain(&feed).await;
    f.host
        .sweep_intents(Instant::now() + PENDING_INTENT_TTL + Duration::from_secs(1));
    assert_eq!(f.host.pending_intent_count(), 0);
    assert!(of_type(&f.subscribe(&profiles, PROFILE).await, "inc.event").is_empty());
    assert!(
        of_type(&f.drain(&feed).await, "intent.invoke.result").is_empty(),
        "the caller was already answered"
    );
}

#[tokio::test]
async fn closing_the_handler_window_drops_its_payload() {
    let f = fixture().await;
    let feed = f.open(&f.feed).await;
    let out = f.invoke(&feed, json!({"archetype": "profile"})).await;
    let (_, token) = open_command(&out).unwrap();
    let profiles = f.open(&f.profiles).await;
    f.host.bind_intent(&profiles, &token).await;
    f.host.close(&profiles);
    assert_eq!(f.host.pending_intent_count(), 0);
}

/// The window could not open the handler, or the user declined it.
#[tokio::test]
async fn a_window_that_fails_to_open_answers_the_caller() {
    let f = fixture().await;
    let feed = f.open(&f.feed).await;
    let out = f.invoke(&feed, json!({"archetype": "profile"})).await;
    let (_, token) = open_command(&out).unwrap();
    f.host.fail_intent(&token, USER_CANCELLED);
    let results = of_type(&f.drain(&feed).await, "intent.invoke.result");
    assert_eq!(results[0]["result"]["error"], USER_CANCELLED);
    assert_eq!(f.host.pending_intent_count(), 0);
}

#[tokio::test]
async fn a_window_may_not_invoke_in_a_burst() {
    let f = fixture().await;
    let feed = f.open(&f.feed).await;
    let first = f.invoke(&feed, json!({"archetype": "profile"})).await;
    assert!(open_command(&first).is_some());
    let second = f
        .send(
            &feed,
            json!({"type": "intent.invoke", "id": "i2", "request": {"archetype": "profile"}}),
        )
        .await;
    assert_eq!(
        of_type(&second, "intent.invoke.result")[0]["result"]["error"],
        INTENT_BUSY
    );
}

/// `napplet:nsite/open` goes to Myco's own nsite opener, whichever way the
/// payload names the site.
#[tokio::test]
async fn an_nsite_opens_in_myco() {
    let f = fixture().await;
    let feed = f.open(&f.feed).await;
    let keys = nostr::Keys::generate();
    let site = nsite_deck::SiteAddr {
        author: keys.public_key(),
        d_tag: Some("blog".into()),
    };
    let naddr = nostr::nips::nip19::Nip19Coordinate::new(
        nostr::nips::nip01::Coordinate::new(nostr::Kind::from(35128u16), keys.public_key())
            .identifier("blog"),
        Vec::<nostr::RelayUrl>::new(),
    )
    .to_bech32()
    .unwrap();

    let out = f
        .invoke(
            &feed,
            json!({"archetype": "nsite", "convention": NSITE_OPEN,
                   "payload": {"naddr": naddr, "kind": 35128}}),
        )
        .await;
    assert_eq!(
        out.first(),
        Some(&ToShell::OpenNsite {
            host: site.host_label()
        })
    );
    let results = of_type(&out, "intent.invoke.result");
    assert_eq!(results[0]["result"]["handler"], MYCO_HANDLER_KEY);
    assert_eq!(results[0]["result"]["convention"], NSITE_OPEN);

    let bad = f
        .invoke(
            &feed,
            json!({"archetype": "nsite", "payload": {"naddr": "nope"}}),
        )
        .await;
    assert_eq!(
        of_type(&bad, "intent.invoke.result")[0]["result"]["error"],
        INVOKE_FAILED
    );
}

#[test]
fn an_nsite_is_found_by_host_naddr_or_url() {
    let keys = nostr::Keys::generate();
    let root = nsite_deck::SiteAddr {
        author: keys.public_key(),
        d_tag: None,
    };
    let label = root.host_label();
    assert_eq!(
        nsite_host(Some(&json!({"host": label}))),
        Some(label.clone())
    );
    assert_eq!(
        nsite_host(Some(&json!({"url": format!("http://{label}.localhost/x")}))),
        Some(label.clone())
    );
    let naddr = nostr::nips::nip19::Nip19Coordinate::new(
        nostr::nips::nip01::Coordinate::new(nostr::Kind::from(15128u16), keys.public_key()),
        Vec::<nostr::RelayUrl>::new(),
    )
    .to_bech32()
    .unwrap();
    assert_eq!(
        nsite_host(Some(&json!({"naddr": format!("nostr:{naddr}")}))),
        Some(label.clone())
    );
    // A napplet's naddr is not an nsite.
    let napplet = nostr::nips::nip19::Nip19Coordinate::new(
        nostr::nips::nip01::Coordinate::new(nostr::Kind::from(35129u16), keys.public_key())
            .identifier("x"),
        Vec::<nostr::RelayUrl>::new(),
    )
    .to_bech32()
    .unwrap();
    assert_eq!(nsite_host(Some(&json!({"naddr": napplet}))), None);
    assert_eq!(nsite_host(Some(&json!({"host": "not a label"}))), None);
    assert_eq!(nsite_host(Some(&json!("just a string"))), None);
    assert_eq!(nsite_host(None), None);
}

/// A catalog change is pushed to every open napplet granted `intent` — once
/// per archetype that changed.
#[tokio::test]
async fn catalog_changes_are_pushed_as_intent_changed() {
    let f = fixture().await;
    let feed = f.open(&f.feed).await;
    f.drain(&feed).await;

    f.both(None);
    f.host.intents_changed().await;
    let pushed = of_type(&f.drain(&feed).await, "intent.changed");
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0]["availability"]["archetype"], "profile");
    assert_eq!(
        pushed[0]["availability"]["candidates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // Nothing changed: nothing pushed.
    f.host.intents_changed().await;
    assert!(of_type(&f.drain(&feed).await, "intent.changed").is_empty());

    // A default is a change.
    f.both(Some(&f.profiles));
    f.host.intents_changed().await;
    let pushed = of_type(&f.drain(&feed).await, "intent.changed");
    assert_eq!(pushed[0]["availability"]["hasDefault"], true);
}

fn library_item(npub: &str, d: &str, archetypes: Option<Vec<LibraryArchetype>>) -> LibraryItem {
    LibraryItem {
        author_npub: npub.to_string(),
        d_tag: Some(d.to_string()),
        title: String::new(),
        url_host: String::new(),
        pinned: true,
        added_at: 0,
        kind: LibraryKind::Napplet,
        granted: Vec::new(),
        denied: Vec::new(),
        pointer: String::new(),
        reviewed: Vec::new(),
        preinstalled: false,
        archetypes,
    }
}

#[test]
fn the_catalog_is_the_librarys_declared_roles_and_myco() {
    let npub = nostr::Keys::generate().public_key().to_bech32().unwrap();
    let profile = LibraryArchetype {
        slug: "profile".into(),
        convention: PROFILE.into(),
    };
    let mut titled = library_item(&npub, "profiles", Some(vec![profile.clone()]));
    titled.title = "Profiles".into();
    titled.pointer = "naddr1whatever".into();
    let library = vec![
        titled,
        library_item(&npub, "cards", Some(vec![profile])),
        library_item(&npub, "none", Some(Vec::new())),
        library_item(&npub, "unknown", None),
    ];
    let catalog = catalog_from(
        &library,
        &BTreeMap::from([("profile".to_string(), format!("{npub}:cards"))]),
    );
    let keys: Vec<_> = catalog.handlers.iter().map(|h| h.key.clone()).collect();
    assert_eq!(
        keys,
        vec![
            format!("{npub}:profiles"),
            format!("{npub}:cards"),
            MYCO_HANDLER_KEY.to_string()
        ]
    );
    assert_eq!(catalog.handlers[0].title.as_deref(), Some("Profiles"));
    assert_eq!(catalog.handlers[0].pointer, "naddr1whatever");
    assert_eq!(catalog.handlers[1].title.as_deref(), Some("cards"));
    assert_eq!(catalog.handlers[1].pointer, format!("{npub}:cards"));

    let views = archetype_views(&library, &catalog.defaults);
    let names: Vec<_> = views.iter().map(|v| v.archetype.clone()).collect();
    assert_eq!(names, vec!["nsite", "profile"]);
    assert_eq!(views[1].default_key, format!("{npub}:cards"));
    assert_eq!(views[1].candidates.len(), 2);
    assert_eq!(views[0].candidates[0].title, "Myco");
}

fn tmp(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "myco-intent-test-{}-{}-{}",
        std::process::id(),
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

/// An entry installed before NAP-INTENT has no recorded roles: the catalog
/// fills them from the served manifest the first time it is read, and a
/// served version that moves rewrites them.
#[tokio::test]
async fn archetypes_are_backfilled_and_follow_the_served_version() {
    let dir = tmp("backfill");
    let content = Arc::new(Content::open(&dir).unwrap());
    let keys = nostr::Keys::generate();
    let npub = keys.public_key().to_bech32().unwrap();
    let v1 = NappletBuilder::new()
        .keys(keys.clone())
        .d_tag(Some("profiles"))
        .archetype("profile", PROFILE)
        .created_at(1_000)
        .build();
    content.relay().publish(v1.manifest.clone()).await.unwrap();
    content.add_napplet_to_library(
        &npub,
        Some("profiles"),
        Some("Profiles"),
        "host",
        Vec::new(),
        Vec::new(),
        "",
        0,
    );
    let entry = |content: &Content| {
        content
            .library_snapshot()
            .into_iter()
            .find(|i| i.d_tag.as_deref() == Some("profiles"))
            .unwrap()
    };
    assert_eq!(entry(&content).archetypes, None);

    let intents = LibraryIntents::new(content.clone(), Default::default());
    let catalog = intents.snapshot().await;
    assert!(
        catalog
            .handlers
            .iter()
            .any(|h| h.key == format!("{npub}:profiles")),
        "the backfilled napplet is a candidate"
    );
    assert_eq!(
        entry(&content).archetypes,
        Some(vec![LibraryArchetype {
            slug: "profile".into(),
            convention: PROFILE.into()
        }])
    );

    // v2 declares another role; pinning it rewrites the entry.
    let v2 = NappletBuilder::new()
        .keys(keys)
        .d_tag(Some("profiles"))
        .archetype("profile", PROFILE)
        .archetype("dm", "napplet:dm/open")
        .created_at(2_000)
        .build();
    content.pin(&v2.manifest);
    assert_eq!(entry(&content).archetypes.unwrap().len(), 2);
    // An older one never moves it back.
    content.pin(&v1.manifest);
    assert_eq!(entry(&content).archetypes.unwrap().len(), 2);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Every Library write wakes the `intent.changed` watcher.
#[tokio::test]
async fn a_library_write_wakes_the_watcher() {
    let dir = tmp("notify");
    let content = Content::open(&dir).unwrap();
    let changed = content.library_changed();
    content.add_napplet_to_library(
        "npub1x",
        Some("d"),
        None,
        "host",
        Vec::new(),
        Vec::new(),
        "",
        0,
    );
    tokio::time::timeout(Duration::from_secs(1), changed.notified())
        .await
        .expect("the write left a permit");
    let _ = std::fs::remove_dir_all(&dir);
}
