//! Wiring `myco-napplet-runtime` to this device: the relay its manifests live
//! in, the Blossom store its files live in, and the live sessions the WebView
//! talks to.
//!
//! The runtime crate names no relay, no blob store and no WebView — that is
//! what makes it testable with no phone. This module is where those seams meet
//! the real ones, and it is deliberately thin: no verification happens here, no
//! policy is decided here. It resolves, hands the bytes to the runtime, and
//! carries frames.
//!
//! ## One session per window
//!
//! A session is created when a napplet window opens and dropped when it closes.
//! Sessions are keyed by an opaque id handed to the Activity, not by the
//! napplet's identity: the same napplet open in two windows is two sessions
//! with two handshakes, and neither can see the other's.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use nostr::nips::nip19::FromBech32;
use nostr::PublicKey;
use nsite_deck::seams::{newest_in_slot, BlobStore, PeerSource, RelayBackend};

use myco_napplet_runtime::artifact::{assemble, Injection, SrcdocArtifact};
use myco_napplet_runtime::dispatch::dispatch;
use myco_napplet_runtime::manifest::{KIND_NAMED, KIND_ROOT, KIND_SNAPSHOT};
use myco_napplet_runtime::prelude::render_for;
use myco_napplet_runtime::resolve::resolve;
use myco_napplet_runtime::session::{NappletIdentity, Session};
use myco_napplet_runtime::shell_link::{ShellAction, ToRuntime, ToShell};

/// Where a napplet manifest lives: an author, a `d` tag for a named one, and
/// the relays the pointer itself named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NappletAddr {
    pub author: PublicKey,
    /// `None` for a root (`15129`) napplet.
    pub d_tag: Option<String>,
    /// Relay hints carried in the `naddr`.
    ///
    /// Not decoration. A napplet lives wherever its author published it, which
    /// is frequently nowhere near the popular aggregators — of Myco's five
    /// default relays, one carried the napplet this was first tested against,
    /// while both relays the pointer named did. Dropping the hints turns "the
    /// author told us where this is" into "we guessed and missed", which the
    /// user sees as a napplet that does not exist.
    pub relays: Vec<String>,
}

impl NappletAddr {
    /// Parse a napplet pointer.
    ///
    /// Accepts the official `napplet:` scheme — `napplet://<naddr>` or
    /// `napplet:<naddr>` — as well as a bare `naddr1…`, a `nostr:` URI, and the
    /// `<npub>:<dtag>` / `<npub>` shorthand the Library already uses for nsites.
    ///
    /// The scheme is stripped rather than interpreted: what identifies a napplet
    /// is the `naddr` inside it, and a `napplet:` URI wrapping something that is
    /// not one is not a napplet however it is spelled.
    pub fn parse(pointer: &str) -> anyhow::Result<Self> {
        let pointer = Self::strip_scheme(pointer.trim());
        if pointer.starts_with("naddr1") {
            let coordinate = nostr::nips::nip19::Nip19Coordinate::from_bech32(pointer)
                .map_err(|e| anyhow::anyhow!("not a valid naddr: {e}"))?;
            let kind = coordinate.coordinate.kind.as_u16();
            anyhow::ensure!(
                kind == KIND_NAMED || kind == KIND_ROOT || kind == KIND_SNAPSHOT,
                "naddr points at kind {kind}, which is not a napplet manifest"
            );
            let identifier = coordinate.coordinate.identifier.clone();
            return Ok(Self {
                author: coordinate.coordinate.public_key,
                d_tag: (!identifier.is_empty()).then_some(identifier),
                relays: coordinate.relays.iter().map(|r| r.to_string()).collect(),
            });
        }

        let (npub, d_tag) = match pointer.split_once(':') {
            Some((npub, d)) => (npub, (!d.is_empty()).then(|| d.to_string())),
            None => (pointer, None),
        };
        let author = PublicKey::from_bech32(npub)
            .map_err(|e| anyhow::anyhow!("not a valid npub or naddr: {e}"))?;
        Ok(Self {
            author,
            d_tag,
            relays: Vec::new(),
        })
    }

    /// Relays to search, the pointer's own hints first.
    ///
    /// The author's hints lead because they are the only ones that know where
    /// the napplet actually is; the defaults follow as a fallback for a pointer
    /// that carried none.
    pub fn search_relays(&self) -> Vec<String> {
        let mut out = self.relays.clone();
        for relay in crate::ip_source::default_relays() {
            if !out
                .iter()
                .any(|r| r.trim_end_matches('/') == relay.trim_end_matches('/'))
            {
                out.push(relay);
            }
        }
        out
    }

    /// Strip a `napplet:` or `nostr:` scheme, with or without `//`, and any
    /// trailing slash the OS may have added.
    fn strip_scheme(pointer: &str) -> &str {
        let mut rest = pointer;
        for scheme in ["napplet://", "napplet:", "nostr://", "nostr:"] {
            if rest.len() >= scheme.len() && rest[..scheme.len()].eq_ignore_ascii_case(scheme) {
                rest = &rest[scheme.len()..];
                break;
            }
        }
        rest.trim_end_matches('/')
    }

    /// The manifest kind this address resolves in.
    pub fn kind(&self) -> u16 {
        match self.d_tag {
            Some(_) => KIND_NAMED,
            None => KIND_ROOT,
        }
    }
}

/// One open napplet window.
struct LiveNapplet {
    session: Session,
    artifact: SrcdocArtifact,
}

/// What the Activity needs to put a napplet on screen.
#[derive(Debug, Clone)]
pub struct OpenedNapplet {
    /// Opaque per-window session id, passed back on every frame.
    pub session_id: String,
    /// The origin the shell is served at, `<label>.napplet.localhost`.
    pub shell_host: String,
    pub title: Option<String>,
}

/// The device's live napplet sessions.
pub struct NappletHost {
    relay: Arc<dyn RelayBackend>,
    blobs: Arc<dyn BlobStore>,
    sessions: Mutex<HashMap<String, LiveNapplet>>,
    next_id: Mutex<u64>,
}

impl NappletHost {
    pub fn new(relay: Arc<dyn RelayBackend>, blobs: Arc<dyn BlobStore>) -> Self {
        Self {
            relay,
            blobs,
            sessions: Mutex::new(HashMap::new()),
            next_id: Mutex::new(1),
        }
    }

    /// Resolve a napplet from the local stores and open a session for it.
    ///
    /// `granted` is what the user approved at install review. Nothing here
    /// widens it, and a napplet that fails verification never gets a session —
    /// the error propagates and no window opens.
    pub async fn open(
        &self,
        addr: &NappletAddr,
        granted: Vec<String>,
    ) -> anyhow::Result<OpenedNapplet> {
        let event = newest_in_slot(
            self.relay.as_ref(),
            addr.kind(),
            &addr.author,
            addr.d_tag.as_deref(),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("no napplet manifest for this address"))?;

        // Every check lives in the runtime crate; a failure here means no
        // session and no window.
        let resolved = resolve(event, self.blobs.as_ref())
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let session = Session::new(NappletIdentity::from(&resolved), granted);
        let prelude = render_for(&session);
        let artifact = assemble(
            &resolved.index_html,
            &Injection {
                prelude_js: Some(&prelude),
                ..Default::default()
            },
        );

        let shell_host =
            myco_napplet_runtime::host::shell_host(&addr.author.to_bytes(), addr.d_tag.as_deref());
        let title = resolved.manifest.title.clone();

        let session_id = {
            let mut next = self.next_id.lock().unwrap();
            let id = format!("napplet-{}", *next);
            *next += 1;
            id
        };
        self.sessions
            .lock()
            .unwrap()
            .insert(session_id.clone(), LiveNapplet { session, artifact });

        Ok(OpenedNapplet {
            session_id,
            shell_host,
            title,
        })
    }

    /// Carry one frame from a window's shell, and return what to send back.
    ///
    /// An unparseable frame yields nothing: the shell is trusted to tag frames,
    /// but what it relays came from the napplet and may be anything at all.
    pub fn frame(&self, session_id: &str, frame_json: &str) -> Vec<ToShell> {
        let Ok(frame) = serde_json::from_str::<ToRuntime>(frame_json) else {
            return Vec::new();
        };

        let mut sessions = self.sessions.lock().unwrap();
        let Some(live) = sessions.get_mut(session_id) else {
            return Vec::new();
        };

        match frame {
            ToRuntime::Shell {
                action: ShellAction::Mounted,
            } => vec![ToShell::load(&live.artifact)],
            ToRuntime::Napplet { message } => dispatch(&mut live.session, &message)
                .envelopes()
                .iter()
                .cloned()
                .map(ToShell::to_napplet)
                .collect(),
        }
    }

    /// Drop a window's session. Every later frame for it is ignored.
    pub fn close(&self, session_id: &str) {
        self.sessions.lock().unwrap().remove(session_id);
    }

    /// How many sessions are open — for state reporting and tests.
    pub fn open_count(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }

    /// Fetch a napplet from somewhere else, verify it, and store it locally.
    ///
    /// This is D9's acquisition path: online once when added by `naddr`, local
    /// and mesh-replicable from then on. `source` is whatever can reach it — a
    /// public-relay source or a mesh peer's — and it is **not trusted**: every
    /// byte it returns is hashed and the signature and aggregate checked before
    /// any of it is kept.
    pub async fn ingest(
        &self,
        addr: &NappletAddr,
        source: &dyn PeerSource,
    ) -> anyhow::Result<IngestedNapplet> {
        let event = source
            .fetch_manifest(&addr.author, addr.d_tag.as_deref())
            .await?
            .ok_or_else(|| anyhow::anyhow!("no napplet manifest at that address"))?;

        // Resolving against a view onto the *source* means the bytes are
        // verified where they arrive, before anything is written here.
        let view = SourceBlobs {
            source,
            servers: servers_from(&event),
        };
        let resolved = resolve(event.clone(), &view)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        // Blob first, manifest last: a half-written napplet is then one the
        // local relay has no manifest for, rather than a manifest whose bytes
        // are missing.
        let index = resolved
            .manifest
            .index_entry()
            .ok_or_else(|| anyhow::anyhow!("verified napplet has no index entry"))?;
        let bytes = view
            .get(&index.sha256)
            .await?
            .ok_or_else(|| anyhow::anyhow!("source served no bytes for {}", index.sha256))?;
        self.blobs.put(&bytes).await?;
        self.relay.publish(event).await?;

        Ok(IngestedNapplet {
            requires: resolved.manifest.requires.clone(),
            title: resolved.manifest.title.clone(),
            description: resolved.manifest.description.clone(),
            d_tag: resolved.d_tag.clone(),
            aggregate: resolved.aggregate.clone(),
        })
    }
}

/// The manifest's `["server", …]` Blossom hints.
fn servers_from(event: &nostr::Event) -> Vec<String> {
    event
        .tags
        .iter()
        .filter_map(|t| {
            let s = t.as_slice();
            (s.first().map(String::as_str) == Some("server")).then(|| s.get(1).cloned())?
        })
        .collect()
}

/// A read-only [`BlobStore`] view onto a [`PeerSource`], so [`resolve`] can
/// verify bytes where they arrive rather than after they are stored.
///
/// Writes are refused rather than silently dropped: nothing should be trying to
/// write into a remote source, and a no-op `put` would hide the mistake.
struct SourceBlobs<'a> {
    source: &'a dyn PeerSource,
    servers: Vec<String>,
}

#[async_trait::async_trait]
impl BlobStore for SourceBlobs<'_> {
    async fn has(&self, sha256_hex: &str) -> bool {
        matches!(self.get(sha256_hex).await, Ok(Some(_)))
    }

    async fn get(&self, sha256_hex: &str) -> anyhow::Result<Option<Vec<u8>>> {
        self.source.fetch_blob(sha256_hex, &self.servers).await
    }

    async fn put(&self, _bytes: &[u8]) -> anyhow::Result<String> {
        anyhow::bail!("a remote napplet source is read-only")
    }

    async fn wipe(&self) -> anyhow::Result<()> {
        anyhow::bail!("a remote napplet source is read-only")
    }
}

/// A fetched, verified napplet awaiting the user's answer on install review.
///
/// Carries what the napplet asked for, never what it was given. A grant exists
/// only once the user answers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NappletReview {
    /// How to open it again: `naddr…` or `<npub>:<dtag>`.
    pub pointer: String,
    /// The fetch is still running. Set the moment the user asks, so the screen
    /// opens immediately and says so, rather than leaving them watching a grid
    /// that has not changed while several relays are tried.
    pub loading: bool,
    pub title: String,
    pub description: String,
    /// The capability domains it declared with `requires` tags. What the review
    /// screen must put in front of the user, in words a person understands.
    pub requires: Vec<String>,
    /// Set when the fetch failed; the screen shows this instead of asking.
    pub error: String,
}

/// What a fetched, verified napplet declares — the input to install review.
///
/// [`IngestedNapplet::requires`] is what the review screen must show, in words a
/// person understands: these are the capabilities the user is being asked to
/// grant, and a granted `relay` covers publishing with no per-event prompt.
#[derive(Debug, Clone)]
pub struct IngestedNapplet {
    pub requires: Vec<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub d_tag: String,
    pub aggregate: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use myco_napplet_runtime::testing::NappletBuilder;
    use nostr::nips::nip19::ToBech32;
    use nsite_deck::testing::{MemBlobs, MemRelay};

    /// A [`PeerSource`] over in-memory stores — "somewhere else", with no
    /// network. Mirrors what `IpPeerSource` does over public relays.
    struct FakeSource {
        relay: MemRelay,
        blobs: MemBlobs,
        kind: u16,
    }

    #[async_trait::async_trait]
    impl PeerSource for FakeSource {
        async fn fetch_manifest(
            &self,
            author: &PublicKey,
            d_tag: Option<&str>,
        ) -> anyhow::Result<Option<nostr::Event>> {
            newest_in_slot(&self.relay, self.kind, author, d_tag).await
        }

        async fn fetch_blob(
            &self,
            sha256_hex: &str,
            _servers: &[String],
        ) -> anyhow::Result<Option<Vec<u8>>> {
            self.blobs.get(sha256_hex).await
        }
    }

    async fn host_with_fixture() -> (NappletHost, NappletAddr) {
        let napplet = NappletBuilder::new().build();
        let relay = Arc::new(MemRelay::new());
        let blobs = Arc::new(MemBlobs::new());
        for (_, bytes) in &napplet.blobs {
            blobs.put(bytes).await.unwrap();
        }
        relay.publish(napplet.manifest.clone()).await.unwrap();

        let addr = NappletAddr {
            author: napplet.author,
            d_tag: Some("fixture".to_string()),
            relays: Vec::new(),
        };
        (NappletHost::new(relay, blobs), addr)
    }

    #[tokio::test]
    async fn opening_resolves_and_hands_back_a_shell_origin() {
        let (host, addr) = host_with_fixture().await;
        let opened = host.open(&addr, vec!["shell".into()]).await.unwrap();

        assert!(opened.shell_host.ends_with(".napplet.localhost"));
        assert_eq!(opened.title.as_deref(), Some("Fixture Napplet"));
        assert_eq!(host.open_count(), 1);
    }

    #[tokio::test]
    async fn the_mount_frame_returns_the_verified_bytes() {
        let (host, addr) = host_with_fixture().await;
        let opened = host.open(&addr, vec![]).await.unwrap();

        let out = host.frame(
            &opened.session_id,
            r#"{"channel":"shell","action":"mounted"}"#,
        );
        assert_eq!(out.len(), 1);
        let ToShell::Shell {
            artifact, sandbox, ..
        } = &out[0]
        else {
            panic!("expected a load command");
        };
        assert!(artifact.contains("Fixture Napplet"));
        assert_eq!(sandbox, SrcdocArtifact::SANDBOX);
    }

    #[tokio::test]
    async fn the_handshake_runs_over_the_frame_channel() {
        let (host, addr) = host_with_fixture().await;
        let opened = host.open(&addr, vec![]).await.unwrap();

        let out = host.frame(
            &opened.session_id,
            r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#,
        );
        assert_eq!(out.len(), 1);
        let ToShell::Napplet { message } = &out[0] else {
            panic!("expected a relayed reply");
        };
        assert_eq!(message.msg_type, "shell.init");

        // Exactly once, however many times it is sent.
        assert!(host
            .frame(
                &opened.session_id,
                r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#
            )
            .is_empty());
    }

    /// Two windows on one napplet are two sessions. Neither handshake
    /// establishes the other, or closing one window would silently disarm the
    /// other's session.
    #[tokio::test]
    async fn two_windows_are_two_independent_sessions() {
        let (host, addr) = host_with_fixture().await;
        let a = host.open(&addr, vec![]).await.unwrap();
        let b = host.open(&addr, vec![]).await.unwrap();
        assert_ne!(a.session_id, b.session_id);
        assert_eq!(a.shell_host, b.shell_host, "same napplet, same origin");

        let ready = r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#;
        assert_eq!(host.frame(&a.session_id, ready).len(), 1);
        assert_eq!(
            host.frame(&b.session_id, ready).len(),
            1,
            "the second window needs its own handshake"
        );

        host.close(&a.session_id);
        assert_eq!(host.open_count(), 1);
        assert!(host.frame(&a.session_id, ready).is_empty());
    }

    /// A napplet that fails verification opens no window at all.
    #[tokio::test]
    async fn a_tampered_napplet_never_opens() {
        let napplet = NappletBuilder::new().break_signature().build();
        let relay = Arc::new(MemRelay::new());
        let blobs = Arc::new(MemBlobs::new());
        for (_, bytes) in &napplet.blobs {
            blobs.put(bytes).await.unwrap();
        }
        relay.publish(napplet.manifest.clone()).await.unwrap();

        let host = NappletHost::new(relay, blobs);
        let addr = NappletAddr {
            author: napplet.author,
            d_tag: Some("fixture".to_string()),
            relays: Vec::new(),
        };
        assert!(host.open(&addr, vec![]).await.is_err());
        assert_eq!(host.open_count(), 0);
    }

    #[tokio::test]
    async fn an_unknown_napplet_is_an_error_not_an_empty_window() {
        let (host, _) = host_with_fixture().await;
        let stranger = NappletAddr {
            author: nostr::Keys::generate().public_key(),
            d_tag: Some("nope".to_string()),
            relays: Vec::new(),
        };
        assert!(host.open(&stranger, vec![]).await.is_err());
    }

    /// D9's acquisition path: fetched from somewhere else, verified against the
    /// fetched bytes, then stored — after which it opens from the local stores
    /// with the source gone.
    #[tokio::test]
    async fn ingest_verifies_then_stores_and_the_napplet_opens_locally() {
        let napplet = NappletBuilder::new()
            .requires(&["relay", "identity"])
            .build();

        // Somewhere else entirely.
        let source = FakeSource {
            relay: MemRelay::new(),
            blobs: MemBlobs::new(),
            kind: KIND_NAMED,
        };
        for (_, bytes) in &napplet.blobs {
            source.blobs.put(bytes).await.unwrap();
        }
        source
            .relay
            .publish(napplet.manifest.clone())
            .await
            .unwrap();

        // This device, empty.
        let host = NappletHost::new(Arc::new(MemRelay::new()), Arc::new(MemBlobs::new()));
        let addr = NappletAddr {
            author: napplet.author,
            d_tag: Some("fixture".to_string()),
            relays: Vec::new(),
        };

        let ingested = host.ingest(&addr, &source).await.unwrap();
        // What install review has to put in front of the user.
        assert_eq!(ingested.requires, vec!["relay", "identity"]);
        assert_eq!(ingested.title.as_deref(), Some("Fixture Napplet"));

        // Now local: opens with no source in reach.
        let opened = host.open(&addr, vec!["relay".into()]).await.unwrap();
        assert!(opened.shell_host.ends_with(".napplet.localhost"));
    }

    /// A napplet that fails verification leaves nothing behind. Storing first
    /// and checking later would leave bytes a later open could pick up.
    #[tokio::test]
    async fn a_failed_ingest_stores_nothing() {
        let napplet = NappletBuilder::new().break_signature().build();
        let source = FakeSource {
            relay: MemRelay::new(),
            blobs: MemBlobs::new(),
            kind: KIND_NAMED,
        };
        for (_, bytes) in &napplet.blobs {
            source.blobs.put(bytes).await.unwrap();
        }
        source
            .relay
            .publish(napplet.manifest.clone())
            .await
            .unwrap();

        let local_relay = Arc::new(MemRelay::new());
        let local_blobs = Arc::new(MemBlobs::new());
        let host = NappletHost::new(local_relay.clone(), local_blobs.clone());
        let addr = NappletAddr {
            author: napplet.author,
            d_tag: Some("fixture".to_string()),
            relays: Vec::new(),
        };

        assert!(host.ingest(&addr, &source).await.is_err());
        assert!(
            local_relay.is_empty(),
            "a rejected napplet left a manifest behind"
        );
        assert!(
            local_blobs.is_empty(),
            "a rejected napplet left bytes behind"
        );
    }

    #[test]
    fn pointers_parse_in_the_shapes_the_library_already_uses() {
        let keys = nostr::Keys::generate();
        let npub = keys.public_key().to_bech32().unwrap();

        let named = NappletAddr::parse(&format!("{npub}:chat")).unwrap();
        assert_eq!(named.author, keys.public_key());
        assert_eq!(named.d_tag.as_deref(), Some("chat"));
        assert_eq!(named.kind(), KIND_NAMED);

        let root = NappletAddr::parse(&npub).unwrap();
        assert_eq!(root.d_tag, None);
        assert_eq!(root.kind(), KIND_ROOT);

        assert!(NappletAddr::parse("not-a-pointer").is_err());
    }

    /// The official scheme, in the spellings the OS and other apps produce.
    #[test]
    fn the_napplet_scheme_is_accepted_however_it_is_spelled() {
        let naddr = "naddr1qvzqqqyf8ypzpwa4mkswz4t8j70s2s6q00wzqv7k7zamxrmj2y4fs88aktcfuf68qyxhwumn8ghj7mn0wvhxcmmvqy2hwumn8ghj7un9d3shjtnyd968gmewwp6kyqqgv35kuemydahxwmmmsd2";
        let bare = NappletAddr::parse(naddr).unwrap();

        for spelling in [
            format!("napplet://{naddr}"),
            format!("napplet:{naddr}"),
            format!("NAPPLET://{naddr}"),
            format!("napplet://{naddr}/"),
            format!("nostr:{naddr}"),
            format!("  napplet://{naddr}  "),
        ] {
            let parsed = NappletAddr::parse(&spelling)
                .unwrap_or_else(|e| panic!("{spelling} did not parse: {e}"));
            assert_eq!(parsed, bare, "{spelling} decoded differently");
        }

        // The scheme is stripped, not trusted: it does not make a non-napplet
        // pointer into one.
        assert!(NappletAddr::parse("napplet://nonsense").is_err());
    }

    /// An naddr naming an nsite is not a napplet. Distinct kinds are what keep
    /// the two resolution paths apart, so the pointer has to respect them.
    #[test]
    fn an_naddr_for_the_nsite_kind_is_refused() {
        let keys = nostr::Keys::generate();
        let coordinate = nostr::nips::nip19::Nip19Coordinate {
            coordinate: nostr::nips::nip01::Coordinate {
                kind: nostr::Kind::from(35128u16),
                public_key: keys.public_key(),
                identifier: "chat".to_string(),
            },
            relays: Vec::new(),
        };
        let naddr = coordinate.to_bech32().unwrap();
        let err = NappletAddr::parse(&naddr).unwrap_err();
        assert!(err.to_string().contains("35128"), "unexpected error: {err}");
    }
}

#[cfg(test)]
mod real_naddr {
    use super::*;

    /// A real napplet `naddr` from the ecosystem, relay hints and all.
    ///
    /// Hand-built pointers prove the parser agrees with itself; this one proves
    /// it agrees with what napplet tooling actually emits — including the relay
    /// TLVs, which a naive decoder trips over.
    #[test]
    fn decodes_a_real_napplet_naddr() {
        let naddr = "naddr1qvzqqqyf8ypzpwa4mkswz4t8j70s2s6q00wzqv7k7zamxrmj2y4fs88aktcfuf68qyxhwumn8ghj7mn0wvhxcmmvqy2hwumn8ghj7un9d3shjtnyd968gmewwp6kyqqgv35kuemydahxwmmmsd2";
        let addr = NappletAddr::parse(naddr).expect("a real napplet naddr must parse");

        assert_eq!(
            addr.author.to_hex(),
            "bbb5dda0e15567979f0543407bdc2033d6f0bbb30f72512a981cfdb2f09e2747"
        );
        assert_eq!(addr.d_tag.as_deref(), Some("dingdong"));
        assert_eq!(addr.kind(), KIND_NAMED);
    }
}

#[cfg(test)]
mod live_fetch {
    use super::*;
    use nsite_deck::testing::{MemBlobs, MemRelay};

    /// Fetch the real napplet from the real internet, end to end.
    ///
    /// `#[ignore]`d because it needs the network and depends on someone else's
    /// relays staying up — but it is the only test that answers "is this
    /// napplet actually reachable from the relays Myco asks", which is
    /// indistinguishable, from inside the app, from a bug in our own code.
    ///
    /// `cargo test -p myco-core --lib live_fetch -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn fetches_the_dingdong_napplet_from_public_relays() {
        let naddr = "naddr1qvzqqqyf8ypzpwa4mkswz4t8j70s2s6q00wzqv7k7zamxrmj2y4fs88aktcfuf68qyxhwumn8ghj7mn0wvhxcmmvqy2hwumn8ghj7un9d3shjtnyd968gmewwp6kyqqgv35kuemydahxwmmmsd2";
        let addr = NappletAddr::parse(naddr).unwrap();
        println!("looking for kind {} d={:?}", addr.kind(), addr.d_tag);

        // Exactly what the app builds, so the timing here is the timing a
        // person sees.
        let source = crate::ip_source::IpPeerSource::new(
            addr.search_relays(),
            crate::ip_source::default_blossom_servers(),
        )
        .with_kind(addr.kind())
        .with_first_answer_grace(std::time::Duration::from_millis(600));

        // The manifest first, on its own, so a missing manifest is told apart
        // from a manifest whose blobs are missing.
        match source
            .fetch_manifest(&addr.author, addr.d_tag.as_deref())
            .await
        {
            Ok(Some(event)) => {
                println!(
                    "manifest found: kind={} id={}",
                    event.kind.as_u16(),
                    event.id
                );
                for tag in event.tags.iter() {
                    println!("  tag {:?}", tag.as_slice());
                }
            }
            Ok(None) => println!("NO MANIFEST on any default relay"),
            Err(e) => println!("manifest fetch errored: {e}"),
        }

        // Then the whole ingest, which is what the app actually runs.
        let host = NappletHost::new(Arc::new(MemRelay::new()), Arc::new(MemBlobs::new()));
        let started = std::time::Instant::now();
        match host.ingest(&addr, &source).await {
            Ok(ingested) => println!(
                "INGEST OK in {:.2?}: title={:?} requires={:?}",
                started.elapsed(),
                ingested.title,
                ingested.requires
            ),
            Err(e) => println!("INGEST FAILED in {:.2?}: {e}", started.elapsed()),
        }
    }
}

#[cfg(test)]
mod relay_probe {
    use super::*;

    const NADDR: &str = "naddr1qvzqqqyf8ypzpwa4mkswz4t8j70s2s6q00wzqv7k7zamxrmj2y4fs88aktcfuf68qyxhwumn8ghj7mn0wvhxcmmvqy2hwumn8ghj7un9d3shjtnyd968gmewwp6kyqqgv35kuemydahxwmmmsd2";

    /// Which relays actually carry this napplet, and how fast — plus the relay
    /// hints the `naddr` itself names, which the pointer parser currently drops.
    ///
    /// `cargo test -p myco-core --lib relay_probe -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn which_relays_have_it() {
        use nostr::nips::nip19::FromBech32;
        let coordinate = nostr::nips::nip19::Nip19Coordinate::from_bech32(NADDR).unwrap();
        println!("naddr relay hints: {:?}", coordinate.relays);

        let addr = NappletAddr::parse(NADDR).unwrap();

        let mut candidates: Vec<String> = crate::ip_source::default_relays();
        for relay in &coordinate.relays {
            candidates.push(relay.to_string());
        }

        for relay in candidates {
            let source = crate::ip_source::IpPeerSource::new(vec![relay.clone()], Vec::new())
                .with_kind(addr.kind());
            let started = std::time::Instant::now();
            let found = source
                .fetch_manifest(&addr.author, addr.d_tag.as_deref())
                .await;
            let elapsed = started.elapsed();
            let verdict = match found {
                Ok(Some(_)) => "HAS IT",
                Ok(None) => "nothing",
                Err(_) => "error",
            };
            println!("{elapsed:>8.2?}  {verdict:<8} {relay}");
        }
    }
}
