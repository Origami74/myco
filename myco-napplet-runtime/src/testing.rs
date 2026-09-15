//! A signed fixture napplet, and the knobs to break it one way at a time.
//!
//! Hand-rolled in Rust rather than produced by `@napplet/vite-plugin`: the
//! aggregate has to match either way, and a fixture that needs a JS toolchain to
//! regenerate is a fixture that rots. The plugin's real output gets checked
//! against this when the render path lands — that is a conformance question, not
//! a verification one.
//!
//! Mirrors `nsite_deck::testing::build_test_site`, and behind the same
//! `testing` feature so no generator ever ships in a release build.

use nostr::{Event, EventBuilder, Keys, Kind, PublicKey, Tag};
use nsite_deck::aggregate::{compute_aggregate_hash, PathEntry};
use nsite_deck::sync::sha256_hex;

use crate::manifest::{KIND_NAMED, KIND_ROOT, KIND_SNAPSHOT};

/// A single-file napplet that completes the NAP-SHELL handshake and says so.
/// Everything is inline, which is what "single-file" means: an opaque origin
/// has nowhere to resolve a relative subresource to.
pub const FIXTURE_INDEX_HTML: &str = r#"<!doctype html>
<meta charset="utf-8">
<title>Fixture Napplet</title>
<style>body{font:16px system-ui;margin:2rem}</style>
<h1>Fixture Napplet</h1>
<p id="status">waiting for shell…</p>
<script>
  window.addEventListener('message', (event) => {
    if (event.data && event.data.action === 'shell.init') {
      document.getElementById('status').textContent = 'ready';
    }
  });
  parent.postMessage({ domain: 'shell', action: 'shell.ready' }, '*');
</script>
"#;

/// A generated napplet: the signed manifest and the blob bytes it references.
pub struct TestNapplet {
    pub author: PublicKey,
    pub manifest: Event,
    /// `(sha256 hex, bytes)` for every file the manifest lists.
    pub blobs: Vec<(String, Vec<u8>)>,
}

impl TestNapplet {
    /// The bytes of the first (usually only) blob.
    pub fn index_bytes(&self) -> &[u8] {
        &self.blobs[0].1
    }
}

/// What aggregate `x` tag the generated manifest carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FixtureAggregate {
    /// The correct aggregate over the manifest's `path` tags.
    #[default]
    Valid,
    /// A well-formed aggregate over a different file set.
    Corrupt,
    /// No aggregate tag. Fatal for a napplet — the aggregate is its identity.
    Omitted,
}

/// Builds signed NIP-5D fixtures. Defaults to a valid, single-file, named
/// napplet; each method breaks exactly one thing, so a test that rejects proves
/// *which* guard rejected.
pub struct NappletBuilder {
    keys: Keys,
    files: Vec<(String, Vec<u8>)>,
    kind: u16,
    d_tag: Option<String>,
    title: Option<String>,
    requires: Vec<String>,
    archetypes: Vec<(String, String)>,
    config: Option<String>,
    servers: Vec<String>,
    aggregate: FixtureAggregate,
    break_signature: bool,
}

impl Default for NappletBuilder {
    fn default() -> Self {
        Self {
            keys: Keys::generate(),
            files: vec![("/index.html".into(), FIXTURE_INDEX_HTML.as_bytes().to_vec())],
            kind: KIND_NAMED,
            d_tag: Some("fixture".into()),
            title: Some("Fixture Napplet".into()),
            requires: vec!["shell".into(), "relay".into()],
            archetypes: Vec::new(),
            config: None,
            servers: Vec::new(),
            aggregate: FixtureAggregate::Valid,
            break_signature: false,
        }
    }
}

impl NappletBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Use a caller-supplied key, so an author's npub is stable across calls.
    pub fn keys(mut self, keys: Keys) -> Self {
        self.keys = keys;
        self
    }

    /// Replace the file set outright.
    pub fn files(mut self, files: &[(&str, &[u8])]) -> Self {
        self.files = files
            .iter()
            .map(|(p, b)| ((*p).to_string(), b.to_vec()))
            .collect();
        self
    }

    /// Add a file — the way to build the multi-file bundle a napplet may not be.
    pub fn file(mut self, path: &str, bytes: &[u8]) -> Self {
        self.files.push((path.to_string(), bytes.to_vec()));
        self
    }

    /// A root (`15129`) or snapshot (`5129`) manifest instead of a named one.
    /// Both drop the `d` tag, since only the addressable kind carries one.
    pub fn kind(mut self, kind: u16) -> Self {
        self.kind = kind;
        if kind == KIND_ROOT || kind == KIND_SNAPSHOT {
            self.d_tag = None;
        }
        self
    }

    pub fn d_tag(mut self, d_tag: Option<&str>) -> Self {
        self.d_tag = d_tag.map(str::to_string);
        self
    }

    pub fn requires(mut self, domains: &[&str]) -> Self {
        self.requires = domains.iter().map(|d| (*d).to_string()).collect();
        self
    }

    pub fn archetype(mut self, slug: &str, convention: &str) -> Self {
        self.archetypes
            .push((slug.to_string(), convention.to_string()));
        self
    }

    /// The raw `config` tag value — raw so a test can supply malformed JSON.
    pub fn config(mut self, schema_json: &str) -> Self {
        self.config = Some(schema_json.to_string());
        self
    }

    pub fn server(mut self, url: &str) -> Self {
        self.servers.push(url.to_string());
        self
    }

    pub fn aggregate(mut self, aggregate: FixtureAggregate) -> Self {
        self.aggregate = aggregate;
        self
    }

    /// Tamper with the event after signing, so its id and signature no longer
    /// cover its contents.
    pub fn break_signature(mut self) -> Self {
        self.break_signature = true;
        self
    }

    pub fn build(self) -> TestNapplet {
        let mut tags: Vec<Tag> = Vec::new();
        if let Some(d) = &self.d_tag {
            tags.push(Tag::identifier(d.clone()));
        }

        let mut blobs = Vec::new();
        let mut entries = Vec::new();
        for (path, bytes) in &self.files {
            let hash = sha256_hex(bytes);
            tags.push(Tag::parse(["path", path, hash.as_str()]).expect("path tag"));
            entries.push(PathEntry {
                path: path.clone(),
                sha256: hash.clone(),
            });
            blobs.push((hash, bytes.clone()));
        }

        for domain in &self.requires {
            tags.push(Tag::parse(["requires", domain]).expect("requires tag"));
        }
        for (slug, convention) in &self.archetypes {
            tags.push(Tag::parse(["archetype", slug, convention]).expect("archetype tag"));
        }
        if let Some(schema) = &self.config {
            tags.push(Tag::parse(["config", schema]).expect("config tag"));
        }
        for url in &self.servers {
            tags.push(Tag::parse(["server", url]).expect("server tag"));
        }
        if let Some(title) = &self.title {
            tags.push(Tag::parse(["title", title]).expect("title tag"));
        }

        match self.aggregate {
            FixtureAggregate::Omitted => {}
            FixtureAggregate::Valid => {
                let hash = compute_aggregate_hash(&entries);
                tags.push(Tag::parse(["x", hash.as_str(), "aggregate"]).expect("aggregate tag"));
            }
            FixtureAggregate::Corrupt => {
                // The aggregate of a file set with one more file in it — what a
                // re-signing intermediary that dropped a file leaves behind.
                let mut other = entries.clone();
                other.push(PathEntry {
                    path: "/ghost.js".into(),
                    sha256: sha256_hex(b"a file the manifest does not list"),
                });
                let hash = compute_aggregate_hash(&other);
                tags.push(Tag::parse(["x", hash.as_str(), "aggregate"]).expect("aggregate tag"));
            }
        }

        let manifest = EventBuilder::new(Kind::from(self.kind), "")
            .tags(tags)
            .sign_with_keys(&self.keys)
            .expect("sign manifest");

        let manifest = if self.break_signature {
            tamper(&manifest)
        } else {
            manifest
        };

        TestNapplet {
            author: self.keys.public_key(),
            manifest,
            blobs,
        }
    }
}

/// Rewrite a signed event's content, leaving its id and signature behind. The
/// signature is over the original, so `verify()` fails — the same shape as a
/// manifest altered in transit.
fn tamper(event: &Event) -> Event {
    let mut json: serde_json::Value = serde_json::to_value(event).expect("event to json");
    json["content"] = serde_json::Value::String("tampered".into());
    serde_json::from_value(json).expect("json to event")
}

/// The default fixture: a valid, signed, single-file named napplet.
pub fn build_test_napplet() -> TestNapplet {
    NappletBuilder::new().build()
}

// --- capability seams -----------------------------------------------------

/// A [`Signer`](crate::seams::Signer) over a throwaway key.
///
/// Real in the way that matters: it signs with a key the test can check
/// against, so "the runtime signed this" is a verifiable claim rather than a
/// stub returning a fixed value.
pub struct TestSigner {
    keys: Keys,
}

impl TestSigner {
    pub fn new() -> Self {
        Self {
            keys: Keys::generate(),
        }
    }

    pub fn with_keys(keys: Keys) -> Self {
        Self { keys }
    }

    pub fn public_key(&self) -> PublicKey {
        self.keys.public_key()
    }
}

impl Default for TestSigner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl crate::seams::Signer for TestSigner {
    async fn public_key(&self) -> anyhow::Result<PublicKey> {
        Ok(self.keys.public_key())
    }

    async fn sign(&self, unsigned: nostr::UnsignedEvent) -> anyhow::Result<Event> {
        unsigned
            .sign_with_keys(&self.keys)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }
}

/// A [`Signer`](crate::seams::Signer) with no key behind it — what a device
/// that has never run a napplet looks like.
pub struct AbsentSigner;

#[async_trait::async_trait]
impl crate::seams::Signer for AbsentSigner {
    async fn public_key(&self) -> anyhow::Result<PublicKey> {
        anyhow::bail!("no user key on this device yet")
    }

    async fn sign(&self, _unsigned: nostr::UnsignedEvent) -> anyhow::Result<Event> {
        anyhow::bail!("no user key on this device yet")
    }
}

/// A context over in-memory seams, for driving capabilities in tests.
pub fn test_context() -> (crate::dispatch::NapContext, std::sync::Arc<TestSigner>) {
    let (ctx, _mesh, signer) = test_context_with_mesh(crate::seams::MeshLimits {
        publish_ttl: 3,
        subscribe_ttl: 2,
    });
    (ctx, signer)
}

/// As [`test_context`], with the [`MemMesh`] handed back so a test can assert
/// what reached the mesh, and with the user's caps set to `limits`.
pub fn test_context_with_mesh(
    limits: crate::seams::MeshLimits,
) -> (
    crate::dispatch::NapContext,
    std::sync::Arc<MemMesh>,
    std::sync::Arc<TestSigner>,
) {
    let signer = std::sync::Arc::new(TestSigner::new());
    let relay: std::sync::Arc<dyn crate::seams::RelayBackend> =
        std::sync::Arc::new(nsite_deck::testing::MemRelay::new());
    let mesh = std::sync::Arc::new(MemMesh::new(relay.clone(), limits));
    let outbox = std::sync::Arc::new(OutboxFixture::new(relay.clone()));
    let ctx = crate::dispatch::NapContext {
        signer: signer.clone(),
        relay: relay.clone(),
        sink: std::sync::Arc::new(crate::seams::StoreOnlySink(relay)),
        mesh: mesh.clone(),
        outbox: outbox.clone(),
        lanes: outbox,
        blobs: std::sync::Arc::new(nsite_deck::testing::MemBlobs::new()),
        fetcher: std::sync::Arc::new(crate::seams::NoFetcher),
    };
    (ctx, mesh, signer)
}

/// As [`test_context`], with the [`OutboxFixture`] handed back so a test can
/// stage relay lists and relays, and assert what was asked of them.
pub fn test_context_with_outbox() -> (
    crate::dispatch::NapContext,
    std::sync::Arc<OutboxFixture>,
    std::sync::Arc<TestSigner>,
) {
    let signer = std::sync::Arc::new(TestSigner::new());
    let relay: std::sync::Arc<dyn crate::seams::RelayBackend> =
        std::sync::Arc::new(nsite_deck::testing::MemRelay::new());
    let mesh = std::sync::Arc::new(MemMesh::new(
        relay.clone(),
        crate::seams::MeshLimits {
            publish_ttl: 3,
            subscribe_ttl: 2,
        },
    ));
    let outbox = std::sync::Arc::new(OutboxFixture::new(relay.clone()));
    let ctx = crate::dispatch::NapContext {
        signer: signer.clone(),
        relay: relay.clone(),
        sink: std::sync::Arc::new(crate::seams::StoreOnlySink(relay)),
        mesh,
        outbox: outbox.clone(),
        lanes: outbox.clone(),
        blobs: std::sync::Arc::new(nsite_deck::testing::MemBlobs::new()),
        fetcher: std::sync::Arc::new(crate::seams::NoFetcher),
    };
    (ctx, outbox, signer)
}

/// As [`test_context`], with the [`MemFetcher`] handed back so a test can
/// stage what "somewhere else" holds and assert what was asked for.
pub fn test_context_with_fetcher() -> (crate::dispatch::NapContext, std::sync::Arc<MemFetcher>) {
    let (mut ctx, _signer) = test_context();
    let fetcher = std::sync::Arc::new(MemFetcher::default());
    ctx.fetcher = fetcher.clone();
    (ctx, fetcher)
}

/// A [`BlobFetcher`](crate::seams::BlobFetcher) over a map: what it holds by
/// sha256, plus anything a test told it to lie about, and every sha it was
/// asked for.
#[derive(Default)]
pub struct MemFetcher {
    held: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
    asked: std::sync::Mutex<Vec<String>>,
}

impl MemFetcher {
    /// Hold `bytes` under their real sha256.
    pub fn hold(&self, bytes: &[u8]) -> String {
        let sha = nsite_deck::sync::sha256_hex(bytes);
        self.held
            .lock()
            .unwrap()
            .insert(sha.clone(), bytes.to_vec());
        sha
    }

    /// Answer `sha` with `bytes` that do not hash to it — a bad server.
    pub fn lie(&self, sha: &str, bytes: &[u8]) {
        self.held
            .lock()
            .unwrap()
            .insert(sha.to_string(), bytes.to_vec());
    }

    pub fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl crate::seams::BlobFetcher for MemFetcher {
    async fn fetch(&self, sha256_hex: &str) -> anyhow::Result<Option<Vec<u8>>> {
        self.asked.lock().unwrap().push(sha256_hex.to_string());
        Ok(self.held.lock().unwrap().get(sha256_hex).cloned())
    }
}

/// Staged relay lists: `(author, direction)` to the URLs and their source.
type StagedPlans = std::collections::HashMap<
    (PublicKey, crate::seams::Direction),
    (Vec<String>, crate::seams::PlanSource),
>;

/// An in-memory world for NAP-OUTBOX: relay lists a test stages, and one
/// [`MemRelay`](nsite_deck::testing::MemRelay) per URL that a test can
/// pre-load, kill, or make refuse publishes. Implements both seams.
pub struct OutboxFixture {
    local: std::sync::Arc<dyn crate::seams::RelayBackend>,
    plans: std::sync::Mutex<StagedPlans>,
    fallback: std::sync::Mutex<Vec<String>>,
    relays: std::sync::Mutex<
        std::collections::HashMap<String, std::sync::Arc<nsite_deck::testing::MemRelay>>,
    >,
    dead: std::sync::Mutex<std::collections::HashSet<String>>,
    refusing: std::sync::Mutex<std::collections::HashSet<String>>,
    queried: std::sync::Mutex<Vec<crate::seams::RelayLane>>,
    published: std::sync::Mutex<Vec<(crate::seams::RelayLane, Event)>>,
    pulled: std::sync::Mutex<Vec<(Vec<crate::seams::RelayLane>, Vec<nostr::Filter>)>>,
}

impl OutboxFixture {
    pub fn new(local: std::sync::Arc<dyn crate::seams::RelayBackend>) -> Self {
        Self {
            local,
            plans: std::sync::Mutex::new(std::collections::HashMap::new()),
            fallback: std::sync::Mutex::new(Vec::new()),
            relays: std::sync::Mutex::new(std::collections::HashMap::new()),
            dead: std::sync::Mutex::new(std::collections::HashSet::new()),
            refusing: std::sync::Mutex::new(std::collections::HashSet::new()),
            queried: std::sync::Mutex::new(Vec::new()),
            published: std::sync::Mutex::new(Vec::new()),
            pulled: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Stage an author's relays for one direction.
    pub fn set_plan(
        &self,
        author: PublicKey,
        direction: crate::seams::Direction,
        urls: &[&str],
        source: crate::seams::PlanSource,
    ) {
        self.plans.lock().unwrap().insert(
            (author, direction),
            (urls.iter().map(|u| u.to_string()).collect(), source),
        );
    }

    /// The relays used when an author has no list, and for policy reads.
    pub fn set_fallback(&self, urls: &[&str]) {
        *self.fallback.lock().unwrap() = urls.iter().map(|u| u.to_string()).collect();
    }

    /// The in-memory relay at `url`, created on first use.
    pub fn relay(&self, url: &str) -> std::sync::Arc<nsite_deck::testing::MemRelay> {
        self.relays
            .lock()
            .unwrap()
            .entry(url.to_string())
            .or_insert_with(|| std::sync::Arc::new(nsite_deck::testing::MemRelay::new()))
            .clone()
    }

    /// Make `url` unreachable: queries yield `None`, publishes `false`.
    pub fn mark_dead(&self, url: &str) {
        self.dead.lock().unwrap().insert(url.to_string());
    }

    /// Make `url` answer publishes with `OK false`.
    pub fn mark_refusing(&self, url: &str) {
        self.refusing.lock().unwrap().insert(url.to_string());
    }

    pub fn queried(&self) -> Vec<crate::seams::RelayLane> {
        self.queried.lock().unwrap().clone()
    }

    pub fn published(&self) -> Vec<(crate::seams::RelayLane, Event)> {
        self.published.lock().unwrap().clone()
    }

    pub fn pulled(&self) -> Vec<(Vec<crate::seams::RelayLane>, Vec<nostr::Filter>)> {
        self.pulled.lock().unwrap().clone()
    }

    fn lanes_of(urls: &[String]) -> Vec<crate::seams::RelayLane> {
        urls.iter()
            .map(|u| crate::seams::RelayLane::from_url(u))
            .collect()
    }
}

#[async_trait::async_trait]
impl crate::seams::OutboxResolver for OutboxFixture {
    async fn plan(
        &self,
        direction: crate::seams::Direction,
        authors: &[PublicKey],
    ) -> crate::seams::RelayPlan {
        use crate::seams::{PlanSource, RelayPlan};
        let fallback = self.fallback.lock().unwrap().clone();
        if authors.is_empty() {
            return RelayPlan {
                lanes: Self::lanes_of(&fallback),
                source: PlanSource::Policy,
                missing_authors: Vec::new(),
            };
        }
        let plans = self.plans.lock().unwrap();
        let mut lanes = Vec::new();
        let mut missing = Vec::new();
        let mut source = PlanSource::Nip65;
        for author in authors {
            match plans.get(&(*author, direction)) {
                Some((urls, src)) => {
                    lanes.extend(Self::lanes_of(urls));
                    if *src != PlanSource::Nip65 {
                        source = *src;
                    }
                }
                None => {
                    missing.push(*author);
                    lanes.extend(Self::lanes_of(&fallback));
                    source = PlanSource::Fallback;
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        lanes.retain(|l| seen.insert(l.clone()));
        RelayPlan {
            lanes,
            source,
            missing_authors: missing,
        }
    }
}

#[async_trait::async_trait]
impl crate::seams::LaneTransport for OutboxFixture {
    async fn query(
        &self,
        lanes: &[crate::seams::RelayLane],
        filters: &[nostr::Filter],
        _timeout: std::time::Duration,
    ) -> Vec<(crate::seams::RelayLane, Option<Vec<Event>>)> {
        self.queried.lock().unwrap().extend(lanes.iter().cloned());
        let mut out = Vec::new();
        for lane in lanes {
            let answer = match lane.url() {
                None => self.local.query(filters).await.ok(),
                Some(url) if self.dead.lock().unwrap().contains(url) => None,
                Some(url) => {
                    let relay: std::sync::Arc<dyn crate::seams::RelayBackend> = self.relay(url);
                    relay.query(filters).await.ok()
                }
            };
            out.push((lane.clone(), answer));
        }
        out
    }

    async fn publish(
        &self,
        lanes: &[crate::seams::RelayLane],
        event: &Event,
        _timeout: std::time::Duration,
    ) -> Vec<(crate::seams::RelayLane, bool)> {
        let mut out = Vec::new();
        for lane in lanes {
            let ok = match lane.url() {
                None => self.local.publish(event.clone()).await.is_ok(),
                Some(url)
                    if self.dead.lock().unwrap().contains(url)
                        || self.refusing.lock().unwrap().contains(url) =>
                {
                    false
                }
                Some(url) => {
                    let relay: std::sync::Arc<dyn crate::seams::RelayBackend> = self.relay(url);
                    relay.publish(event.clone()).await.is_ok()
                }
            };
            if ok {
                self.published
                    .lock()
                    .unwrap()
                    .push((lane.clone(), event.clone()));
            }
            out.push((lane.clone(), ok));
        }
        out
    }

    async fn pull_into_local(
        &self,
        lanes: &[crate::seams::RelayLane],
        filters: &[nostr::Filter],
    ) -> anyhow::Result<()> {
        self.pulled
            .lock()
            .unwrap()
            .push((lanes.to_vec(), filters.to_vec()));
        Ok(())
    }
}

/// A [`MeshSink`](crate::seams::MeshSink) with no mesh behind it: stores a
/// publish locally and records the hop budget it came with, records every
/// pull, and reports whatever reach a test sets.
pub struct MemMesh {
    store: std::sync::Arc<dyn crate::seams::RelayBackend>,
    limits: std::sync::Mutex<crate::seams::MeshLimits>,
    reach: std::sync::Mutex<crate::seams::MeshReach>,
    published: std::sync::Mutex<Vec<(Event, u8)>>,
    pulled: std::sync::Mutex<Vec<(Vec<serde_json::Value>, u8)>>,
}

impl MemMesh {
    pub fn new(
        store: std::sync::Arc<dyn crate::seams::RelayBackend>,
        limits: crate::seams::MeshLimits,
    ) -> Self {
        Self {
            store,
            limits: std::sync::Mutex::new(limits),
            reach: std::sync::Mutex::new(crate::seams::MeshReach::default()),
            published: std::sync::Mutex::new(Vec::new()),
            pulled: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Every event published, with the hop budget it was given.
    pub fn published(&self) -> Vec<(Event, u8)> {
        self.published.lock().unwrap().clone()
    }

    /// Every pull requested: the raw filters and the hop budget.
    pub fn pulled(&self) -> Vec<(Vec<serde_json::Value>, u8)> {
        self.pulled.lock().unwrap().clone()
    }

    /// Change the caps under a running context, as a settings change would.
    pub fn set_limits(&self, limits: crate::seams::MeshLimits) {
        *self.limits.lock().unwrap() = limits;
    }

    pub fn set_reach(&self, online: bool, peers: usize) {
        *self.reach.lock().unwrap() = crate::seams::MeshReach { online, peers };
    }
}

#[async_trait::async_trait]
impl crate::seams::MeshSink for MemMesh {
    async fn limits(&self) -> crate::seams::MeshLimits {
        *self.limits.lock().unwrap()
    }

    async fn reach(&self) -> anyhow::Result<crate::seams::MeshReach> {
        Ok(*self.reach.lock().unwrap())
    }

    async fn publish(&self, event: Event, ttl: u8) -> anyhow::Result<()> {
        self.store.publish(event.clone()).await?;
        self.published.lock().unwrap().push((event, ttl));
        Ok(())
    }

    async fn pull(&self, filters: Vec<serde_json::Value>, ttl: u8) -> anyhow::Result<()> {
        self.pulled.lock().unwrap().push((filters, ttl));
        Ok(())
    }
}

/// An [`EventSink`](crate::seams::EventSink) that records what it accepted, so
/// a test can assert an event was handed on rather than only written.
#[derive(Default)]
pub struct RecordingSink {
    accepted: std::sync::Mutex<Vec<Event>>,
}

impl RecordingSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn accepted(&self) -> Vec<Event> {
        self.accepted.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl crate::seams::EventSink for RecordingSink {
    async fn accept(&self, event: Event) -> anyhow::Result<()> {
        self.accepted.lock().unwrap().push(event);
        Ok(())
    }
}
