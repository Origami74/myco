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
use std::time::Duration;

use tokio::sync::mpsc;

use nostr::nips::nip19::FromBech32;
use nostr::PublicKey;
use nsite_deck::seams::{newest_in_slot, BlobStore, PeerSource, RelayBackend};

use myco_napplet_runtime::artifact::{assemble, Injection, SrcdocArtifact};
use myco_napplet_runtime::dispatch::{dispatch, NapContext};
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

    /// The public source for this napplet: its pointer's relay hints, then
    /// the defaults, asking for the napplet kind. Untrusted — every byte is
    /// hashed and the signature checked before anything is kept.
    ///
    /// Somebody is usually watching a spinner, so one relay answering in a
    /// few hundred milliseconds is not held up by another that sits on the
    /// connection until the timeout.
    pub fn public_source(&self) -> crate::ip_source::IpPeerSource {
        crate::ip_source::IpPeerSource::new(
            self.search_relays(),
            crate::ip_source::default_blossom_servers(),
        )
        .with_kind(self.kind())
        .with_first_answer_grace(Duration::from_millis(600))
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
    /// The window's session, behind an async lock so concurrent frames **queue**
    /// rather than race.
    ///
    /// They must queue and not be dropped. A napplet's shim sends several
    /// messages as it starts, so anything that discards a frame under
    /// contention will sooner or later discard `shell.ready` — and then the
    /// session never establishes and every capability call afterwards is
    /// refused with "session not established", long after the message that
    /// went missing.
    session: Arc<tokio::sync::Mutex<Session>>,
    artifact: SrcdocArtifact,
    /// Frames the runtime wants to send this window without being asked —
    /// subscription deliveries, and later anything else the shell must be told.
    ///
    /// Queued rather than pushed directly because the FFI only runs when
    /// called. The Activity drains this on a long poll, which is the same shape
    /// the BLE and TUN bridges already use.
    outbox: mpsc::UnboundedSender<ToShell>,
    /// The draining end. Behind a lock because one window has one drainer, and
    /// two would split its frames between them.
    drain: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<ToShell>>>,
}

/// What the Activity needs to put a napplet on screen.
#[derive(Debug, Clone)]
pub struct OpenedNapplet {
    /// Opaque per-window session id, passed back on every frame.
    pub session_id: String,
    /// The origin the shell is served at, `<label>.napplet.localhost`.
    pub shell_host: String,
    pub title: Option<String>,
    /// What the session was actually opened with: the stored grants, widened
    /// by any domain the napplet declared that this build implements, an
    /// earlier one did not, and the user has not switched off. See
    /// [`NappletHost::open_with`].
    pub grants: crate::content::NappletGrants,
}

impl OpenedNapplet {
    /// The domains the session may use — a view over [`OpenedNapplet::grants`].
    pub fn granted(&self) -> &[String] {
        &self.grants.granted
    }
}

/// Where a napplet's **served** manifest comes from, and how a version is
/// pinned once its bytes are here.
///
/// The relay keeps the newest manifest per slot, and a newer one can arrive
/// with no blob behind it — pulled by a subscription, flooded by a peer. Served
/// straight from the relay, that napplet stops opening until the blob turns up.
/// So, as with nsites (`nsite-updates.md` §1), what is served is the version
/// that was **pinned** when its blob landed, and the pin moves only when the
/// next version's blob has landed too.
#[async_trait::async_trait]
pub trait ManifestStore: Send + Sync {
    /// The manifest to serve for a slot: the pinned version when there is one,
    /// else the newest the relay holds.
    async fn current(
        &self,
        kind: u16,
        author: &PublicKey,
        d_tag: Option<&str>,
    ) -> anyhow::Result<Option<nostr::Event>>;

    /// Pin `manifest` as the version to serve for its slot. Called only once
    /// its index blob is in the local store.
    fn pin(&self, manifest: &nostr::Event);
}

/// A [`ManifestStore`] with no pin: always the relay's newest. For tests, and
/// for a host stood up over bare seams.
pub struct NewestInSlot(pub Arc<dyn RelayBackend>);

#[async_trait::async_trait]
impl ManifestStore for NewestInSlot {
    async fn current(
        &self,
        kind: u16,
        author: &PublicKey,
        d_tag: Option<&str>,
    ) -> anyhow::Result<Option<nostr::Event>> {
        newest_in_slot(self.0.as_ref(), kind, author, d_tag).await
    }

    fn pin(&self, _manifest: &nostr::Event) {}
}

/// The device's live napplet sessions.
pub struct NappletHost {
    relay: Arc<dyn RelayBackend>,
    blobs: Arc<dyn BlobStore>,
    /// Which version of each napplet is served. See [`ManifestStore`].
    manifests: Arc<dyn ManifestStore>,
    /// What the capabilities reach the world through. One per device — the
    /// seams are not per napplet; the session is.
    ctx: NapContext,
    sessions: Mutex<HashMap<String, LiveNapplet>>,
    next_id: Mutex<u64>,
}

impl NappletHost {
    /// Stand up the host over a wired set of seams. The relay and blob store
    /// the host resolves napplets from are the ones the capabilities use.
    pub fn new(ctx: NapContext) -> Self {
        Self {
            relay: ctx.relay.clone(),
            blobs: ctx.blobs.clone(),
            manifests: Arc::new(NewestInSlot(ctx.relay.clone())),
            ctx,
            sessions: Mutex::new(HashMap::new()),
            next_id: Mutex::new(1),
        }
    }

    /// Serve versions through `manifests` — on the device, the content layer's
    /// active-version pins — instead of the relay's newest.
    pub fn with_manifests(mut self, manifests: Arc<dyn ManifestStore>) -> Self {
        self.manifests = manifests;
        self
    }

    /// As [`NappletHost::open_with`], for a napplet with nothing switched off:
    /// `granted` is what the user approved, or `None` for one not installed.
    pub async fn open(
        &self,
        addr: &NappletAddr,
        granted: Option<Vec<String>>,
    ) -> anyhow::Result<OpenedNapplet> {
        self.open_with(
            addr,
            granted.map(|granted| crate::content::NappletGrants {
                granted,
                denied: Vec::new(),
            }),
        )
        .await
    }

    /// Resolve a napplet from the local stores and open a session for it.
    ///
    /// `grants` is what the Library records — what the user allowed and what
    /// they switched off — or `None` for a napplet that is not installed,
    /// which opens with nothing but the handshake. A napplet that fails
    /// verification never gets a session: the error propagates and no window
    /// opens.
    pub async fn open_with(
        &self,
        addr: &NappletAddr,
        grants: Option<crate::content::NappletGrants>,
    ) -> anyhow::Result<OpenedNapplet> {
        let event = self
            .manifests
            .current(addr.kind(), &addr.author, addr.d_tag.as_deref())
            .await?
            .ok_or_else(|| anyhow::anyhow!("no napplet manifest for this address"))?;

        // Every check lives in the runtime crate; a failure here means no
        // session and no window.
        let resolved = resolve(event.clone(), self.blobs.as_ref())
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        // It opened, so its blob is here: this is the version to keep serving
        // until the next one's blob is too. Covers napplets ingested before
        // pinning existed, and costs nothing when already pinned.
        self.manifests.pin(&event);

        // The grants were narrowed at install to what *that* build could do
        // (`effective_grants`), so a napplet installed before a domain existed
        // here has no grant for it however plainly it declared the need — and
        // fails on the first call, with no screen ever having said no. What
        // the user agreed to was "what it declares, plus the defaults"; this
        // build implementing more of that list does not change the agreement,
        // so the session is opened with the declared set as of now.
        //
        // Except what the user switched off. A domain in `denied` is a
        // decision, and a launch does not get to overrule it — that is the
        // difference between "never decided" and "said no", and why the two
        // are stored apart. Nothing undeclared is added, nothing is added to a
        // napplet that was never installed, and the long-press sheet shows
        // the result.
        let grants = match grants {
            None => crate::content::NappletGrants::default(),
            Some(mut grants) => {
                for domain in effective_grants(&resolved.manifest.requires) {
                    if grants.granted.contains(&domain) || grants.denied.contains(&domain) {
                        continue;
                    }
                    tracing::info!(
                        napplet = %addr.d_tag.as_deref().unwrap_or("<root>"),
                        %domain,
                        "granting a declared capability this build newly implements"
                    );
                    grants.granted.push(domain);
                }
                grants
            }
        };

        tracing::info!(
            napplet = %addr.d_tag.as_deref().unwrap_or("<root>"),
            granted = ?grants.granted,
            denied = ?grants.denied,
            "opening napplet"
        );
        let session = Session::new(
            NappletIdentity::from(&resolved),
            grants.granted.iter().cloned(),
        );
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
        let (outbox, drain) = mpsc::unbounded_channel();
        self.sessions.lock().unwrap().insert(
            session_id.clone(),
            LiveNapplet {
                session: Arc::new(tokio::sync::Mutex::new(session)),
                artifact,
                outbox,
                drain: Arc::new(tokio::sync::Mutex::new(drain)),
            },
        );

        Ok(OpenedNapplet {
            session_id,
            shell_host,
            title,
            grants,
        })
    }

    /// Push changed grants into every open window of the napplet at
    /// `(author, d_tag)`, so a switch flipped on the sheet is obeyed by the
    /// next call rather than the next launch.
    ///
    /// Matched on the author as well as the `d_tag`. Two authors may publish
    /// napplets under the same `d`, and a grant given to one must not reach
    /// the other's open window even for the moment before it relaunches —
    /// that moment is exactly long enough to make a call.
    ///
    /// Each window touched is also told to relaunch (see
    /// [`ToShell::Relaunch`]): the grant is live for the next call, but the
    /// napplet made its startup calls — its subscriptions — under the old
    /// grants, and a refused subscribe is not retried.
    pub async fn apply_grants(
        &self,
        author: &PublicKey,
        d_tag: Option<&str>,
        granted: Vec<String>,
    ) {
        let wanted = d_tag.unwrap_or("");
        let author = author.to_hex();
        let live: Vec<(
            Arc<tokio::sync::Mutex<Session>>,
            mpsc::UnboundedSender<ToShell>,
        )> = {
            let sessions = self.sessions.lock().unwrap();
            sessions
                .values()
                .map(|l| (l.session.clone(), l.outbox.clone()))
                .collect()
        };
        for (session, outbox) in live {
            // A session mid-call is updated when the call ends; the grant is
            // checked per call anyway.
            let mut s = session.lock().await;
            if s.identity().d_tag == wanted && s.identity().author == author {
                s.set_granted(granted.clone());
                let _ = outbox.send(ToShell::Relaunch);
            }
        }
    }

    /// Carry one frame from a window's shell, and return what to send back.
    ///
    /// An unparseable frame yields nothing: the shell is trusted to tag frames,
    /// but what it relays came from the napplet and may be anything at all.
    pub async fn frame(&self, session_id: &str, frame_json: &str) -> Vec<ToShell> {
        let Ok(frame) = serde_json::from_str::<ToRuntime>(frame_json) else {
            return Vec::new();
        };

        // The mount reply needs no capability work, so it is answered without
        // taking the session across an await point.
        if let ToRuntime::Shell {
            action: ShellAction::Mounted,
        } = frame
        {
            let sessions = self.sessions.lock().unwrap();
            return match sessions.get(session_id) {
                Some(live) => vec![ToShell::load(&live.artifact)],
                None => Vec::new(),
            };
        }

        let ToRuntime::Napplet { message } = frame else {
            return Vec::new();
        };

        // The session handle is cloned out and the map's lock released before
        // awaiting, so a slow capability call never blocks another window.
        let session = {
            let sessions = self.sessions.lock().unwrap();
            match sessions.get(session_id) {
                Some(live) => live.session.clone(),
                None => return Vec::new(),
            }
        };

        // The session is held only for what changes it — the handshake, a
        // subscription opening or closing. A read runs against a snapshot
        // with the lock released: a relay query waits on the network for
        // seconds, and holding the session across it would queue every other
        // call from this window behind it, until the napplet's own timeout
        // fired on a call that had not even started. The gate (established,
        // granted) is checked on the snapshot, which is as current as the
        // moment the call arrived.
        let out = if myco_napplet_runtime::needs_session(&message) {
            let mut session = session.lock().await;
            dispatch(&self.ctx, &mut session, &message).await
        } else {
            let mut snapshot = session.lock().await.clone();
            dispatch(&self.ctx, &mut snapshot, &message).await
        };

        out.envelopes()
            .iter()
            .cloned()
            .map(ToShell::to_napplet)
            .collect()
    }

    /// Deliver an accepted event to whichever open napplets subscribed to it.
    ///
    /// Called for every event this device accepts — its own publishes and
    /// anything a peer sent — so a subscription behaves the same whichever side
    /// of the mesh an event came from. A napplet that never subscribed, or was
    /// not granted `relay`, matches nothing and costs one filter check.
    pub async fn on_event(&self, event: nostr::Event) {
        // The handles are cloned out and the map's lock released before any
        // await: a slow window must not hold up delivery to the others.
        let live: Vec<(
            Arc<tokio::sync::Mutex<Session>>,
            mpsc::UnboundedSender<ToShell>,
        )> = {
            let sessions = self.sessions.lock().unwrap();
            sessions
                .values()
                .map(|l| (l.session.clone(), l.outbox.clone()))
                .collect()
        };

        for (session, outbox) in live {
            let frames = {
                let session = session.lock().await;
                myco_napplet_runtime::deliveries_for(&session, &event)
            };
            for frame in frames {
                // A closed window's receiver is gone; its frames go nowhere,
                // which is what closing means.
                let _ = outbox.send(ToShell::to_napplet(frame));
            }
        }
    }

    /// Wait for frames this window should be sent unprompted, up to `timeout`.
    ///
    /// Blocks rather than returning immediately so the caller can long-poll
    /// instead of spinning — the same shape the BLE and TUN bridges use. An
    /// empty result means the wait expired, not that the window is gone.
    pub async fn next_frames(&self, session_id: &str, timeout: Duration) -> Vec<ToShell> {
        let drain = {
            let sessions = self.sessions.lock().unwrap();
            match sessions.get(session_id) {
                Some(live) => live.drain.clone(),
                None => return Vec::new(),
            }
        };

        let mut drain = drain.lock().await;
        let mut out = Vec::new();
        // One blocking wait, then everything else already queued behind it, so
        // a burst crosses the FFI in one call rather than one per frame.
        if let Ok(Some(first)) = tokio::time::timeout(timeout, drain.recv()).await {
            out.push(first);
            while let Ok(next) = drain.try_recv() {
                out.push(next);
            }
        }
        out
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

        // Blob first, manifest last, pin last of all: a half-written napplet
        // is then one the local relay has no manifest for, rather than a
        // manifest whose bytes are missing — and the served version moves only
        // once the new bytes are here. The bytes are the ones `resolve` already
        // fetched and verified: `index_html` decoded as UTF-8 without loss, so
        // re-encoding it is the original blob, and the source — a peer over
        // BLE, often — is not asked for it twice.
        self.blobs.put(resolved.index_html.as_bytes()).await?;
        self.relay.publish(event.clone()).await?;
        self.manifests.pin(&event);

        Ok(IngestedNapplet {
            requires: resolved.manifest.requires.clone(),
            title: resolved.manifest.title.clone(),
            description: resolved.manifest.description.clone(),
            d_tag: resolved.d_tag.clone(),
            aggregate: resolved.aggregate.clone(),
        })
    }
}

impl NappletHost {
    /// Fetch whatever `source` has for `addr` and, if it is a newer version
    /// than the one served, bring it in — bytes first, so the served version
    /// moves only when the new one can open. Returns whether it moved.
    ///
    /// The update path for napplets: the same [`NappletHost::ingest`] the first
    /// fetch used, gated on version. A source with nothing newer, or nothing at
    /// all, leaves the served version alone.
    pub async fn refresh(
        &self,
        addr: &NappletAddr,
        source: &dyn PeerSource,
    ) -> anyhow::Result<bool> {
        let served = self
            .manifests
            .current(addr.kind(), &addr.author, addr.d_tag.as_deref())
            .await?;
        let offered = source
            .fetch_manifest(&addr.author, addr.d_tag.as_deref())
            .await?
            .ok_or_else(|| anyhow::anyhow!("no napplet manifest at that address"))?;
        let newer = match &served {
            Some(served) => offered.created_at > served.created_at && offered.id != served.id,
            None => true,
        };
        if !newer {
            return Ok(false);
        }
        self.ingest(addr, source).await?;
        Ok(true)
    }
}

/// Refresh every installed napplet from the public relays, in parallel.
/// Returns `(updated, checked)` for the update-check toast.
///
/// Public relays only: a napplet's author publishes there, and the holder
/// who shared it is not recorded. Offline-only skips the lot — `checked`
/// still counts them, so the toast says they were not updated rather than
/// that there were none.
pub async fn refresh_all(host: &NappletHost, addrs: &[NappletAddr]) -> (usize, usize) {
    let checks = addrs.iter().map(|addr| async move {
        match host.refresh(addr, &addr.public_source()).await {
            Ok(moved) => moved,
            Err(e) => {
                tracing::debug!(
                    napplet = %addr.d_tag.as_deref().unwrap_or("<root>"),
                    error = %e,
                    "napplet update check: no newer version reachable"
                );
                false
            }
        }
    });
    let results = futures_util::future::join_all(checks).await;
    (results.iter().filter(|m| **m).count(), addrs.len())
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

/// NAP-RESOURCE's fetcher: where a blob this device does not hold is looked
/// for, in the order Myco prefers — the Circle's Blossom stores over the mesh
/// first, in parallel, then the public servers when the internet is allowed.
///
/// Only ever reached on a local miss; the handler asks the store first and
/// keeps whatever this returns. The mesh goes first because it is what this
/// app is for: a picture one phone in the room fetched once is a picture
/// nobody else in the room needs the internet for.
pub struct BlossomFetcher {
    content: Arc<crate::content::Content>,
    /// The public Blossom servers. The defaults, unless a test says otherwise.
    public_servers: Vec<String>,
}

/// How long one mesh peer gets before the public servers are tried.
const MESH_BLOB_TIMEOUT: Duration = Duration::from_secs(8);
/// How long the public servers get, all together.
const INTERNET_BLOB_TIMEOUT: Duration = Duration::from_secs(20);

impl BlossomFetcher {
    pub fn new(content: Arc<crate::content::Content>) -> Self {
        Self {
            content,
            public_servers: crate::ip_source::default_blossom_servers(),
        }
    }

    /// Use `servers` instead of the public defaults — for tests, which must
    /// never reach the internet.
    #[cfg(test)]
    pub fn with_public_servers(mut self, servers: Vec<String>) -> Self {
        self.public_servers = servers;
        self
    }
}

#[async_trait::async_trait]
impl myco_napplet_runtime::seams::BlobFetcher for BlossomFetcher {
    async fn fetch(&self, sha256_hex: &str, max_bytes: usize) -> anyhow::Result<Option<Vec<u8>>> {
        // Every reachable Circle member at once; the first to answer wins.
        // A peer that does not hold it answers quickly with nothing, and a
        // peer that is gone hits the bound — either way the others are not
        // waited on serially.
        let pool = self.content.peer_relays();
        let asks = self
            .content
            .reachable_npubs()
            .into_iter()
            .filter_map(|npub| crate::ip_source::mesh_source_for(pool.clone(), &npub).ok())
            .map(|source| source.with_max_blob_bytes(max_bytes))
            .map(|source| async move {
                match tokio::time::timeout(MESH_BLOB_TIMEOUT, source.fetch_blob(sha256_hex, &[]))
                    .await
                {
                    Ok(Ok(Some(bytes))) => Some(bytes),
                    _ => None,
                }
            });
        if let Some(found) = futures_util::future::join_all(asks)
            .await
            .into_iter()
            .flatten()
            .next()
        {
            return Ok(Some(found));
        }

        if self.content.internet_looks_down() {
            return Ok(None);
        }
        let public = crate::ip_source::IpPeerSource::new(Vec::new(), self.public_servers.clone())
            .with_max_blob_bytes(max_bytes);
        match tokio::time::timeout(INTERNET_BLOB_TIMEOUT, public.fetch_blob(sha256_hex, &[])).await
        {
            Ok(Ok(bytes)) => Ok(bytes),
            Ok(Err(e)) => Err(e),
            Err(_) => Ok(None),
        }
    }
}

/// NAP-MESH's seam over the device's mesh: hop-limited publish through the
/// [`RelayHub`](crate::mesh_relay::RelayHub), backlog pull through the Circle
/// relay pool, and the user's caps from settings.
///
/// The caps are read per call from a lock the settings action writes, so a
/// user lowering "how far apps reach" is obeyed by the very next publish —
/// there is no per-napplet copy to go stale.
pub struct NappletMeshSink {
    hub: Arc<Mutex<Option<Arc<crate::mesh_relay::RelayHub>>>>,
    content: Arc<crate::content::Content>,
    limits: Arc<std::sync::RwLock<myco_napplet_runtime::MeshLimits>>,
    node_live: Arc<std::sync::atomic::AtomicBool>,
}

impl NappletMeshSink {
    pub fn new(
        hub: Arc<Mutex<Option<Arc<crate::mesh_relay::RelayHub>>>>,
        content: Arc<crate::content::Content>,
        limits: Arc<std::sync::RwLock<myco_napplet_runtime::MeshLimits>>,
        node_live: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            hub,
            content,
            limits,
            node_live,
        }
    }
}

#[async_trait::async_trait]
impl myco_napplet_runtime::seams::MeshSink for NappletMeshSink {
    async fn limits(&self) -> myco_napplet_runtime::MeshLimits {
        *self.limits.read().unwrap()
    }

    async fn reach(&self) -> anyhow::Result<myco_napplet_runtime::MeshReach> {
        Ok(myco_napplet_runtime::MeshReach {
            online: self.node_live.load(std::sync::atomic::Ordering::Relaxed),
            peers: self.content.reachable_npubs().len(),
        })
    }

    async fn publish(&self, event: nostr::Event, ttl: u8) -> anyhow::Result<()> {
        // Clamped again here, whatever the caller did: the seam is the last
        // place a budget passes before it reaches the mesh, and the cap is the
        // user's promise, not the runtime crate's.
        let ttl = ttl.min(self.limits.read().unwrap().publish_ttl);
        let hub = self.hub.lock().unwrap().clone();
        match hub {
            Some(hub) => {
                hub.accept_local_with_ttl(event, Some(ttl)).await?;
                Ok(())
            }
            // No hub is the host-build and pre-start case; the event is
            // stored and goes no further, which is what `ttl` 0 means anyway.
            None => self.content.relay().publish(event).await,
        }
    }

    async fn pull(&self, filters: Vec<serde_json::Value>, ttl: u8) -> anyhow::Result<()> {
        let ttl = ttl.min(self.limits.read().unwrap().subscribe_ttl);
        if ttl == 0 {
            return Ok(());
        }
        let hub = self.hub.lock().unwrap().clone();
        let Some(hub) = hub else {
            // Nothing to pull through and nowhere to deliver to.
            return Ok(());
        };
        let content = self.content.clone();
        // Spawned: a peer two hops out answers in seconds, and the napplet's
        // next call must not queue behind it. What arrives is accepted into
        // the hub, which is what delivers it to the napplet's live
        // subscription — and to every other subscriber on this device.
        tokio::spawn(async move {
            // `ttl` counts rings of peers beyond this device, as a publish's
            // budget does. The envelope carries the budget the *receiver* may
            // spend, so the first ring is asked with one less: 1 asks direct
            // peers and stops, 2 lets them ask theirs.
            let meta = crate::mesh_wire::MeshMeta::pull(
                ttl - 1,
                crate::mesh_wire::new_query_id(),
                crate::content::PULL_BUDGET_MS,
            );
            let events = content.pull_from_peers(filters, meta, None).await;
            let mut fresh = 0usize;
            for event in events {
                match hub.accept_unforwarded(event).await {
                    Ok(true) => fresh += 1,
                    Ok(false) => {}
                    Err(e) => tracing::debug!(error = %e, "napplet mesh pull: could not store"),
                }
            }
            tracing::debug!(ttl, fresh, "napplet mesh pull finished");
        });
        Ok(())
    }
}

/// What installing a napplet would grant it: what it declared it needs, plus
/// the defaults every napplet gets, narrowed to what this build can actually
/// do.
///
/// Narrowing matters: offering a capability Myco has not implemented would put
/// a promise on the review screen that no call could keep.
pub fn effective_grants(requires: &[String]) -> Vec<String> {
    use myco_napplet_runtime::session::{DEFAULT_GRANTS, IMPLEMENTED_DOMAINS, MANDATORY_DOMAINS};

    let mut out: Vec<String> = Vec::new();
    let mut add = |domain: &str| {
        if IMPLEMENTED_DOMAINS.contains(&domain)
            && !MANDATORY_DOMAINS.contains(&domain)
            && !out.iter().any(|d| d == domain)
        {
            out.push(domain.to_string());
        }
    };
    for domain in DEFAULT_GRANTS {
        add(domain);
    }
    for domain in requires {
        add(domain);
    }
    out.sort();
    out
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
    /// The capability domains it declared with `requires` tags — a statement of
    /// what it needs, not what it gets.
    pub requires: Vec<String>,
    /// What installing it would actually grant: its declared `requires`
    /// together with the defaults every napplet receives, narrowed to what this
    /// build implements.
    ///
    /// This — not `requires` — is what the review screen must put in front of
    /// the user in words, because this is what they are agreeing to. A default
    /// that was not shown would be a grant nobody made.
    pub grants: Vec<String>,
    /// Set when the fetch failed; the screen shows this instead of asking.
    pub error: String,
    /// The peer who shared it, when it arrived by a tap or a scan — kept so a
    /// retry tries their phone first again, exactly as the first attempt did.
    /// Without it a retry in a room with no internet would search blind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder: Option<String>,
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

    /// A context over in-memory seams for `relay` and `blobs`, with nothing
    /// behind the mesh, the outbox or the fetcher.
    pub(super) fn test_ctx(relay: Arc<dyn RelayBackend>, blobs: Arc<dyn BlobStore>) -> NapContext {
        NapContext {
            signer: Arc::new(myco_napplet_runtime::testing::TestSigner::new()),
            relay: relay.clone(),
            sink: Arc::new(myco_napplet_runtime::seams::StoreOnlySink(Arc::new(
                MemRelay::new(),
            ))),
            mesh: test_mesh(),
            outbox: test_outbox(),
            lanes: test_outbox(),
            blobs,
            fetcher: Arc::new(myco_napplet_runtime::seams::NoFetcher),
        }
    }

    /// An outbox with nothing staged, for tests that are not about it.
    pub(super) fn test_outbox() -> Arc<myco_napplet_runtime::testing::OutboxFixture> {
        Arc::new(myco_napplet_runtime::testing::OutboxFixture::new(Arc::new(
            MemRelay::new(),
        )))
    }

    /// A mesh with nothing behind it, for tests that are not about the mesh.
    pub(super) fn test_mesh() -> Arc<myco_napplet_runtime::testing::MemMesh> {
        Arc::new(myco_napplet_runtime::testing::MemMesh::new(
            Arc::new(MemRelay::new()),
            myco_napplet_runtime::MeshLimits {
                publish_ttl: 3,
                subscribe_ttl: 2,
            },
        ))
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
        (NappletHost::new(test_ctx(relay, blobs)), addr)
    }

    #[tokio::test]
    async fn opening_resolves_and_hands_back_a_shell_origin() {
        let (host, addr) = host_with_fixture().await;
        let opened = host.open(&addr, Some(vec!["shell".into()])).await.unwrap();

        assert!(opened.shell_host.ends_with(".napplet.localhost"));
        assert_eq!(opened.title.as_deref(), Some("Fixture Napplet"));
        assert_eq!(host.open_count(), 1);
    }

    #[tokio::test]
    async fn the_mount_frame_returns_the_verified_bytes() {
        let (host, addr) = host_with_fixture().await;
        let opened = host.open(&addr, None).await.unwrap();

        let out = host
            .frame(
                &opened.session_id,
                r#"{"channel":"shell","action":"mounted"}"#,
            )
            .await;
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
        let opened = host.open(&addr, None).await.unwrap();

        let out = host
            .frame(
                &opened.session_id,
                r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#,
            )
            .await;
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
            .await
            .is_empty());
    }

    /// Frames that overlap must queue, never be dropped.
    ///
    /// This is the bug that made a napplet report "session not established"
    /// long after it had sent `shell.ready`: an earlier design lifted the
    /// session out for the duration of a call, so a frame arriving meanwhile
    /// found nothing to dispatch to and was discarded. A napplet's shim sends
    /// several messages as it starts, so the discarded one was eventually the
    /// handshake — and then every capability call afterwards was refused, with
    /// nothing to show that a message had gone missing.
    #[tokio::test]
    async fn overlapping_frames_queue_rather_than_vanish() {
        let (host, addr) = host_with_fixture().await;
        let opened = host
            .open(&addr, Some(vec!["identity".into()]))
            .await
            .unwrap();

        let ready = r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#;
        let query = r#"{"channel":"napplet","message":{"type":"identity.getPublicKey","id":"i1"}}"#;

        // Fired together, as a shim starting up does.
        let (a, b) = tokio::join!(
            host.frame(&opened.session_id, ready),
            host.frame(&opened.session_id, query),
        );

        // Whichever order they land in, the handshake is not lost: exactly one
        // frame answers with shell.init.
        let inits = [&a, &b]
            .iter()
            .flat_map(|out| out.iter())
            .filter(
                |f| matches!(f, ToShell::Napplet { message } if message.msg_type == "shell.init"),
            )
            .count();
        assert_eq!(inits, 1, "the handshake was dropped under contention");

        // And the session really is established afterwards — a later call is
        // serviced rather than refused.
        let out = host.frame(&opened.session_id, query).await;
        let ToShell::Napplet { message } = &out[0] else {
            panic!("expected a reply");
        };
        assert_eq!(message.msg_type, "identity.getPublicKey.result");
        assert!(
            message.field("error").is_none(),
            "still refused after the handshake: {:?}",
            message.field("error")
        );
    }

    /// The whole point of a subscription: an event arriving **after** it was
    /// made is delivered.
    ///
    /// Before this, `relay.subscribe` answered with what was already stored and
    /// registered nothing, so a later event had nowhere to go — a query wearing
    /// a subscription's name. A doorbell rung on another phone could never
    /// reach the napplet waiting for it, however well the mesh carried it.
    #[tokio::test]
    async fn an_event_arriving_after_subscribe_is_delivered() {
        let (host, addr) = host_with_fixture().await;
        let opened = host.open(&addr, Some(vec!["relay".into()])).await.unwrap();

        host.frame(
            &opened.session_id,
            r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#,
        )
        .await;

        let out = host
            .frame(
                &opened.session_id,
                r#"{"channel":"napplet","message":{"type":"relay.subscribe","id":"a1","subId":"sub-1","filters":[{"kinds":[20666]}]}}"#,
            )
            .await;
        // Nothing stored yet, so the subscription opens straight to EOSE.
        assert_eq!(out.len(), 1);
        let ToShell::Napplet { message } = &out[0] else {
            panic!("expected a relayed reply");
        };
        assert_eq!(message.msg_type, "relay.eose");

        // Now an event turns up — a peer's doorbell, as far as this device is
        // concerned.
        let ringer = nostr::Keys::generate();
        let ring = nostr::EventBuilder::new(nostr::Kind::from(20666u16), "ding")
            .sign_with_keys(&ringer)
            .unwrap();
        host.on_event(ring.clone()).await;

        // It reaches the napplet unprompted.
        let pushed = host
            .next_frames(&opened.session_id, Duration::from_secs(2))
            .await;
        assert_eq!(pushed.len(), 1, "the subscription delivered nothing");
        let ToShell::Napplet { message } = &pushed[0] else {
            panic!("expected a napplet frame");
        };
        assert_eq!(message.msg_type, "relay.event");
        assert_eq!(message.field("subId").unwrap(), "sub-1");
        assert_eq!(message.field("result").unwrap()["event"]["content"], "ding");
    }

    /// An event nothing asked for is not delivered, and a closed subscription
    /// stops delivering.
    #[tokio::test]
    async fn only_matching_live_subscriptions_are_delivered() {
        let (host, addr) = host_with_fixture().await;
        let opened = host.open(&addr, Some(vec!["relay".into()])).await.unwrap();
        host.frame(
            &opened.session_id,
            r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#,
        )
        .await;
        host.frame(
            &opened.session_id,
            r#"{"channel":"napplet","message":{"type":"relay.subscribe","id":"a1","subId":"sub-1","filters":[{"kinds":[20666]}]}}"#,
        )
        .await;

        // A kind nobody subscribed to.
        let other = nostr::EventBuilder::text_note("not for you")
            .sign_with_keys(&nostr::Keys::generate())
            .unwrap();
        host.on_event(other).await;
        assert!(
            host.next_frames(&opened.session_id, Duration::from_millis(200))
                .await
                .is_empty(),
            "delivered an event nothing subscribed to"
        );

        // Closed, so the matching kind stops arriving too.
        host.frame(
            &opened.session_id,
            r#"{"channel":"napplet","message":{"type":"relay.close","id":"a2","subId":"sub-1"}}"#,
        )
        .await;
        let ring = nostr::EventBuilder::new(nostr::Kind::from(20666u16), "ding")
            .sign_with_keys(&nostr::Keys::generate())
            .unwrap();
        host.on_event(ring).await;
        assert!(
            host.next_frames(&opened.session_id, Duration::from_millis(200))
                .await
                .is_empty(),
            "a closed subscription kept delivering"
        );
    }

    /// An installed napplet whose stored grants predate a domain this build
    /// implements gets that domain if it declared it — and a napplet that was
    /// never installed gets nothing whatever it declares.
    #[tokio::test]
    async fn an_installed_napplets_grants_widen_to_what_it_declared() {
        let (host, addr) = host_with_fixture().await; // declares shell, relay
        let opened = host.open(&addr, Some(vec![])).await.unwrap();
        let mut granted = opened.granted().to_vec();
        granted.sort();
        let mut expected: Vec<String> = effective_grants(&["shell".into(), "relay".into()]);
        expected.sort();
        assert_eq!(granted, expected);
        assert!(granted.contains(&"relay".to_string()));
        assert!(
            !granted.contains(&"mesh".to_string()),
            "undeclared, non-default: not granted"
        );

        let stranger = host.open(&addr, None).await.unwrap();
        assert!(
            stranger.granted().is_empty(),
            "an uninstalled napplet was granted something"
        );
    }

    /// The version served is the one whose bytes are here. A newer manifest
    /// landing in the relay with no blob behind it — pulled by a subscription,
    /// flooded by a peer — must not take the napplet off the air; and once
    /// the newer version's bytes arrive, it is served. The nsite rule
    /// (`nsite-updates.md` §1), for napplets.
    #[tokio::test]
    async fn a_newer_manifest_without_its_blob_does_not_displace_the_served_version() {
        let dir = std::env::temp_dir().join(format!(
            "myco-napplet-pin-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(crate::content::Content::open(&dir).unwrap());
        let host = NappletHost::new(test_ctx(content.relay(), content.blobs()))
            .with_manifests(content.clone());

        let keys = nostr::Keys::generate();
        let v1 = NappletBuilder::new()
            .keys(keys.clone())
            .created_at(1_000)
            .title("Version one")
            .files(&[("/index.html", b"<!doctype html><title>one</title>")])
            .build();
        let v2 = NappletBuilder::new()
            .keys(keys.clone())
            .created_at(2_000)
            .title("Version two")
            .files(&[("/index.html", b"<!doctype html><title>two</title>")])
            .build();
        let addr = NappletAddr {
            author: keys.public_key(),
            d_tag: Some("fixture".to_string()),
            relays: Vec::new(),
        };
        async fn source_for(napplet: &myco_napplet_runtime::testing::TestNapplet) -> FakeSource {
            let relay = MemRelay::new();
            let blobs = MemBlobs::new();
            for (_, bytes) in &napplet.blobs {
                blobs.put(bytes).await.unwrap();
            }
            relay.publish(napplet.manifest.clone()).await.unwrap();
            FakeSource {
                relay,
                blobs,
                kind: KIND_NAMED,
            }
        }

        // v1 arrives whole and opens.
        host.ingest(&addr, &source_for(&v1).await).await.unwrap();
        let opened = host.open(&addr, None).await.unwrap();
        assert_eq!(opened.title.as_deref(), Some("Version one"));

        // v2's manifest lands in the relay by some other route — no blob.
        content.relay().publish(v2.manifest.clone()).await.unwrap();
        let opened = host.open(&addr, None).await.unwrap();
        assert_eq!(
            opened.title.as_deref(),
            Some("Version one"),
            "a manifest with no bytes behind it was served"
        );

        // A refresh from a source that has v2 whole moves the served version;
        // one that has nothing newer does not.
        assert!(host.refresh(&addr, &source_for(&v2).await).await.unwrap());
        let opened = host.open(&addr, None).await.unwrap();
        assert_eq!(opened.title.as_deref(), Some("Version two"));
        assert!(!host.refresh(&addr, &source_for(&v2).await).await.unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A domain the user switched off on the sheet stays off at the next
    /// launch, however plainly the napplet declares it and whatever the
    /// defaults say. This is the bug where the switch flipped itself back on:
    /// "not granted" and "said no" were the same empty slot, so the widening
    /// that grants newly implemented declared domains re-granted the refusal.
    #[tokio::test]
    async fn a_domain_switched_off_stays_off_at_the_next_launch() {
        let (host, addr) = host_with_fixture().await; // declares shell, relay
        let stored = crate::content::NappletGrants {
            granted: vec!["identity".into()],
            denied: vec!["relay".into(), "resource".into()],
        };
        let opened = host.open_with(&addr, Some(stored.clone())).await.unwrap();
        assert!(
            !opened.granted().contains(&"relay".to_string()),
            "a declared domain the user switched off was granted at launch"
        );
        assert!(
            !opened.granted().contains(&"resource".to_string()),
            "a default domain the user switched off was granted at launch"
        );
        assert_eq!(opened.grants.denied, stored.denied, "the refusal was lost");
        assert!(opened.granted().contains(&"identity".to_string()));

        // And the refusal holds on the wire, not only in the record.
        host.frame(
            &opened.session_id,
            r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#,
        )
        .await;
        let out = host
            .frame(
                &opened.session_id,
                r#"{"channel":"napplet","message":{"type":"relay.query","id":"q1","filters":{"kinds":[1]}}}"#,
            )
            .await;
        let ToShell::Napplet { message } = &out[0] else {
            panic!("not a napplet frame")
        };
        assert!(
            message.field("error").is_some(),
            "relay was served after being switched off"
        );
    }

    /// A grant given to one author's napplet must not reach another author's
    /// napplet that happens to share the `d` tag — not even for the moment
    /// before the other window relaunches.
    #[tokio::test]
    async fn a_grant_change_is_scoped_to_the_author() {
        let (host, addr) = host_with_fixture().await;
        let opened = host.open(&addr, Some(vec![])).await.unwrap();
        host.frame(
            &opened.session_id,
            r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#,
        )
        .await;
        let ask = r#"{"channel":"napplet","message":{"type":"mesh.info","id":"m1"}}"#;

        let other_author = nostr::Keys::generate().public_key();
        host.apply_grants(&other_author, Some("fixture"), vec!["mesh".into()])
            .await;
        let out = host.frame(&opened.session_id, ask).await;
        let ToShell::Napplet { message } = &out[0] else {
            panic!("not a napplet frame")
        };
        assert!(
            message.field("error").is_some(),
            "another author's grant reached this napplet"
        );
    }

    /// A grant flipped on the sheet reaches an open window: refused before,
    /// served after, with no reload — and withdrawn the same way.
    #[tokio::test]
    async fn a_grant_changed_on_the_sheet_is_live() {
        let (host, addr) = host_with_fixture().await;
        let opened = host.open(&addr, Some(vec![])).await.unwrap();
        host.frame(
            &opened.session_id,
            r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#,
        )
        .await;
        let ask = r#"{"channel":"napplet","message":{"type":"mesh.info","id":"m1"}}"#;

        let out = host.frame(&opened.session_id, ask).await;
        let ToShell::Napplet { message } = &out[0] else {
            panic!("not a napplet frame")
        };
        assert!(
            message.field("error").is_some(),
            "mesh was granted without asking"
        );

        host.apply_grants(&addr.author, Some("fixture"), vec!["mesh".into()])
            .await;
        let out = host.frame(&opened.session_id, ask).await;
        let ToShell::Napplet { message } = &out[0] else {
            panic!("not a napplet frame")
        };
        assert!(
            message.field("error").is_none(),
            "the switch did not reach the window"
        );
        assert!(message.field("limits").is_some());

        host.apply_grants(&addr.author, Some("fixture"), vec![])
            .await;
        let out = host.frame(&opened.session_id, ask).await;
        let ToShell::Napplet { message } = &out[0] else {
            panic!("not a napplet frame")
        };
        assert!(
            message.field("error").is_some(),
            "withdrawing did not reach the window"
        );
    }

    /// A napplet without the grant receives nothing, even if it managed to
    /// register a subscription — the check is on delivery, so revoking a grant
    /// stops the next event rather than the next launch.
    #[tokio::test]
    async fn an_ungranted_napplet_receives_no_deliveries() {
        let (host, addr) = host_with_fixture().await;
        let opened = host.open(&addr, None).await.unwrap();
        host.frame(
            &opened.session_id,
            r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#,
        )
        .await;
        host.frame(
            &opened.session_id,
            r#"{"channel":"napplet","message":{"type":"relay.subscribe","id":"a1","subId":"sub-1","filters":[{"kinds":[20666]}]}}"#,
        )
        .await;

        let ring = nostr::EventBuilder::new(nostr::Kind::from(20666u16), "ding")
            .sign_with_keys(&nostr::Keys::generate())
            .unwrap();
        host.on_event(ring).await;
        assert!(
            host.next_frames(&opened.session_id, Duration::from_millis(200))
                .await
                .is_empty(),
            "delivered to a napplet that was never granted relay"
        );
    }

    /// The fetcher reaches the public servers when the store misses, and not
    /// at all when offline only.
    #[tokio::test]
    async fn the_blossom_fetcher_uses_the_public_servers_unless_offline() {
        use myco_napplet_runtime::seams::BlobFetcher as _;

        let dir = std::env::temp_dir().join(format!("myco-fetcher-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(crate::content::Content::open(&dir).unwrap());
        let bytes = b"a picture, allegedly".to_vec();
        let sha = nsite_deck::sync::sha256_hex(&bytes);
        let server =
            crate::ip_source::tests::mock_blossom(vec![(sha.clone(), bytes.clone())]).await;
        let fetcher = BlossomFetcher::new(content.clone()).with_public_servers(vec![server]);

        assert_eq!(
            fetcher.fetch(&sha, 1 << 20).await.unwrap(),
            Some(bytes.clone())
        );
        assert_eq!(
            fetcher.fetch(&"00".repeat(32), 1 << 20).await.unwrap(),
            None
        );
        // A cap below the blob's size is enforced by the download, not after it.
        assert_eq!(
            fetcher.fetch(&sha, bytes.len() - 1).await.unwrap(),
            None,
            "an oversized blob came back anyway"
        );

        content.set_offline_only(true);
        assert_eq!(
            fetcher.fetch(&sha, 1 << 20).await.unwrap(),
            None,
            "offline only reached the internet"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two windows on one napplet are two sessions. Neither handshake
    /// establishes the other, or closing one window would silently disarm the
    /// other's session.
    #[tokio::test]
    async fn two_windows_are_two_independent_sessions() {
        let (host, addr) = host_with_fixture().await;
        let a = host.open(&addr, None).await.unwrap();
        let b = host.open(&addr, None).await.unwrap();
        assert_ne!(a.session_id, b.session_id);
        assert_eq!(a.shell_host, b.shell_host, "same napplet, same origin");

        let ready = r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#;
        assert_eq!(host.frame(&a.session_id, ready).await.len(), 1);
        assert_eq!(
            host.frame(&b.session_id, ready).await.len(),
            1,
            "the second window needs its own handshake"
        );

        host.close(&a.session_id);
        assert_eq!(host.open_count(), 1);
        assert!(host.frame(&a.session_id, ready).await.is_empty());
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

        let host = NappletHost::new(test_ctx(relay, blobs));
        let addr = NappletAddr {
            author: napplet.author,
            d_tag: Some("fixture".to_string()),
            relays: Vec::new(),
        };
        assert!(host.open(&addr, None).await.is_err());
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
        assert!(host.open(&stranger, None).await.is_err());
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
        let host = NappletHost::new(test_ctx(
            Arc::new(MemRelay::new()),
            Arc::new(MemBlobs::new()),
        ));
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
        let opened = host.open(&addr, Some(vec!["relay".into()])).await.unwrap();
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
        let host = NappletHost::new(test_ctx(local_relay.clone(), local_blobs.clone()));
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
    use super::tests::test_ctx;
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
        let host = NappletHost::new(test_ctx(
            Arc::new(MemRelay::new()),
            Arc::new(MemBlobs::new()),
        ));
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

#[cfg(test)]
mod grants {
    use super::*;

    /// A napplet is useful only if it can do something, and a manifest's
    /// `requires` cannot be relied on to say what — the napplet this was first
    /// tested against declares nothing at all, because its toolchain dropped
    /// the tags. Defaults are what stop that being an app that can never be
    /// granted anything.
    #[test]
    fn a_napplet_that_declares_nothing_still_gets_the_defaults() {
        let grants = effective_grants(&[]);
        assert!(grants.contains(&"relay".to_string()));
        assert!(grants.contains(&"identity".to_string()));
    }

    #[test]
    fn what_it_declares_is_added_to_the_defaults() {
        let grants = effective_grants(&["identity".to_string()]);
        assert!(grants.contains(&"identity".to_string()));
        assert!(grants.contains(&"relay".to_string()));
        // Declared twice over is still granted once.
        assert_eq!(
            grants.iter().filter(|d| *d == "identity").count(),
            1,
            "a domain was granted twice"
        );
    }

    /// Offering a capability Myco has not built would put a promise on the
    /// review screen that no call could keep.
    #[test]
    fn a_capability_this_build_lacks_is_never_offered() {
        let grants = effective_grants(&["storage".to_string(), "notify".to_string()]);
        assert!(!grants.contains(&"storage".to_string()));
        assert!(!grants.contains(&"notify".to_string()));
    }

    /// `shell` is the handshake, not a permission. Listing it would ask someone
    /// to agree to the app starting up.
    #[test]
    fn the_handshake_is_not_offered_as_a_permission() {
        assert!(!effective_grants(&["shell".to_string()]).contains(&"shell".to_string()));
    }
}
