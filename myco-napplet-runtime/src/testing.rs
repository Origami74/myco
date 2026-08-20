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
    let signer = std::sync::Arc::new(TestSigner::new());
    let ctx = crate::dispatch::NapContext {
        signer: signer.clone(),
        relay: std::sync::Arc::new(nsite_deck::testing::MemRelay::new()),
    };
    (ctx, signer)
}
