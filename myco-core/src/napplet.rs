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
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::sync::mpsc;

use nostr::nips::nip19::FromBech32;
use nostr::PublicKey;
use nsite_deck::seams::{newest_in_slot, BlobStore, PeerSource, RelayBackend};

use myco_napplet_runtime::artifact::{assemble, Injection, SrcdocArtifact};
use myco_napplet_runtime::delivered::{Delivered, Ledger};
use myco_napplet_runtime::dispatch::{dispatch, NapContext, Outcome};
use myco_napplet_runtime::manifest::{KIND_NAMED, KIND_ROOT, KIND_SNAPSHOT};
use myco_napplet_runtime::nap::link::{LinkTarget, BLOCKED_BY_POLICY, INVALID_URL};
use myco_napplet_runtime::prelude::render_for;
use myco_napplet_runtime::resolve::resolve;
use myco_napplet_runtime::session::{Appearance, NappletIdentity, Session};
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
    ///
    /// An alphanumeric-mode QR code yields `NOSTR:NADDR1…`. Bech32 is valid
    /// all-upper or all-lower (mixed case is not), so an upper-case `naddr`
    /// or `npub` is lowered before decoding; the `d` tag of the shorthand is
    /// never touched — it is a name, not an encoding.
    pub fn parse(pointer: &str) -> anyhow::Result<Self> {
        let pointer = Self::strip_scheme(pointer.trim());
        // `get`, not a byte slice: the prefix test must not cut a multibyte
        // character (H1's rule).
        if pointer
            .get(..6)
            .is_some_and(|p| p.eq_ignore_ascii_case("naddr1"))
        {
            let lowered = pointer.to_ascii_lowercase();
            let coordinate = nostr::nips::nip19::Nip19Coordinate::from_bech32(&lowered)
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
        let author = PublicKey::from_bech32(&npub.to_ascii_lowercase())
            .map_err(|e| anyhow::anyhow!("not a valid npub or naddr: {e}"))?;
        Ok(Self {
            author,
            d_tag,
            relays: Vec::new(),
        })
    }

    /// Strip a `napplet:` or `nostr:` scheme, with or without `//`, and any
    /// trailing slash the OS may have added.
    ///
    /// Never index a `&str` by a length derived from another string: the
    /// pointer comes from a QR code, an NFC tap or a share link, and a
    /// multibyte character straddling the cut is a panic that unwinds through
    /// the JNI boundary and aborts the app. `get` refuses a non-boundary cut
    /// with `None`; the slice below is only taken once the prefix is known to
    /// be ASCII, so the boundary is safe.
    fn strip_scheme(pointer: &str) -> &str {
        let mut rest = pointer;
        for scheme in ["napplet://", "napplet:", "nostr://", "nostr:"] {
            if rest
                .get(..scheme.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(scheme))
            {
                rest = &rest[scheme.len()..];
                break;
            }
        }
        rest.trim_end_matches('/')
    }

    /// The public source for this napplet: its pointer's relay hints, then
    /// the author's NIP-65 write relays (read from and cached in `store`),
    /// then the defaults, asking for the napplet kind. Untrusted — every byte
    /// is hashed and the signature checked before anything is kept.
    ///
    /// Somebody is usually watching a spinner, so one relay answering in a
    /// few hundred milliseconds is not held up by another that sits on the
    /// connection until the timeout — and the author's relay list is looked
    /// up alongside the defaults, never in front of them
    /// ([`crate::ip_source::IpPeerSource::with_author_outbox`]).
    pub fn public_source(&self, store: Arc<dyn RelayBackend>) -> crate::ip_source::IpPeerSource {
        crate::ip_source::IpPeerSource::new(
            crate::ip_source::default_relays(),
            crate::ip_source::default_blossom_servers(),
        )
        .with_relay_hints(self.relays.clone())
        .with_author_outbox(Arc::new(crate::ip_source::AuthorOutbox::new(store)))
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
    /// The version this window opened. See [`NappletHost::newer_version`].
    opened: OpenedVersion,
    /// The secret in this window's blob URLs (`/_blob/<token>/<sha256>`).
    /// Only the shell learns it, from the load command; the napplet's frame
    /// never does, so it cannot ask the window host for blobs itself. See
    /// [`NappletHost::blob`].
    blob_token: String,
}

/// A fresh blob-URL secret: 128 bits from the OS.
fn new_blob_token() -> String {
    use chacha20poly1305::aead::rand_core::RngCore as _;
    let mut bytes = [0u8; 16];
    chacha20poly1305::aead::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Which version of which napplet a window's session pinned at open — kept to
/// tell, later, whether the served version has moved on without it.
struct OpenedVersion {
    addr: NappletAddr,
    created_at: nostr::Timestamp,
    aggregate: String,
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
    /// by any **reviewed** domain the napplet declared that this build
    /// implements, an earlier one did not, and the user has not switched off.
    /// See [`NappletHost::open_with`].
    pub grants: crate::content::NappletGrants,
    /// What the served manifest declares with `requires` — the list a review
    /// sheet for this version would show.
    pub requires: Vec<String>,
    /// Declared domains this build implements that the user has never seen:
    /// not on the reviewed list, not granted, not denied. Never granted here;
    /// the caller routes a non-empty list back through the review sheet.
    pub unreviewed: Vec<String>,
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

/// The install-review slot the app's review sheet is drawn from — shared with
/// `AppRuntime`, which fills it on `FetchNapplet`.
pub type ReviewSlot = Arc<Mutex<Option<NappletReview>>>;

/// How soon after one admitted napplet link another may open a review.
///
/// Covers the gap between answering `link.open` and the window host
/// dispatching the fetch that fills the review slot — without it, a napplet
/// firing a burst would have every one admitted before the first sheet
/// showed — and stops a napplet reopening the sheet the moment the user
/// dismisses it.
pub const REVIEW_LINK_COOLDOWN: Duration = Duration::from_secs(5);

/// How soon after one admitted web link another may open the browser.
pub const WEB_LINK_COOLDOWN: Duration = Duration::from_secs(2);

/// `error` for a napplet link refused because the review sheet this napplet
/// opened is still up over its window. NAP-LINK leaves the code open; this
/// one says what to do: answer the sheet first.
pub const LINK_BUSY_REVIEW_OPEN: &str = "busy: another review is open";

/// `error` for a napplet link refused while another app is being added: the
/// download it started must not lose its sheet.
pub const LINK_BUSY_ADDING: &str = "busy: another app is being added";

/// `error` for a napplet link inside [`REVIEW_LINK_COOLDOWN`] of the last one.
pub const LINK_BUSY_TRY_AGAIN: &str = "busy: try again in a moment";

/// NAP-LINK admission — the host's half of `link.open`.
///
/// The runtime crate classifies a link; whether it is admitted *now* depends on
/// what is on screen, which only the host knows. The rate limits are
/// device-wide, not per window: two napplets taking turns must not get twice
/// the rate. What the review slot holds is judged per window: only a review
/// this window opened is on screen over it.
#[derive(Default)]
struct LinkGate {
    /// When set, the review slot the app's sheets are drawn from.
    review: Option<ReviewSlot>,
    /// The session and pointer of the last napplet link admitted: the review
    /// that window's sheet shows while the slot still names that pointer.
    review_owner: Mutex<Option<(String, String)>>,
    last_review: Mutex<Option<std::time::Instant>>,
    last_web: Mutex<Option<std::time::Instant>>,
}

impl LinkGate {
    /// Admit `target` for the window of `session` at `now`, returning the
    /// command for the window host, or the `error` code to deny it with.
    fn admit(
        &self,
        session: &str,
        target: &LinkTarget,
        now: std::time::Instant,
    ) -> Result<ToShell, &'static str> {
        let within = |last: &Mutex<Option<std::time::Instant>>, cooldown: Duration| {
            let mut last = last.lock().unwrap();
            if last.is_some_and(|t| now.saturating_duration_since(t) < cooldown) {
                return true;
            }
            *last = Some(now);
            false
        };
        match target {
            LinkTarget::Web(url) => {
                if within(&self.last_web, WEB_LINK_COOLDOWN) {
                    return Err(BLOCKED_BY_POLICY);
                }
                Ok(ToShell::OpenExternal { url: url.clone() })
            }
            LinkTarget::Napplet(pointer) => {
                // The runtime already decoded it; the host's own parser is the
                // one `FetchNapplet` will use, so it gets the last word.
                if NappletAddr::parse(pointer).is_err() {
                    return Err(INVALID_URL);
                }
                // One question at a time, over this window: the review this
                // window opened — loading or waiting for an answer — is on
                // screen above the napplet, and a napplet's say-so never
                // replaces it. One queued anywhere else (the Apps screen,
                // another napplet's window) is not in front of the user, so
                // it does not refuse this link; the new review replaces it.
                // An install already downloading does, wherever it started:
                // its sheet has to be there to say how the download went. An
                // added review is a confirmation, not a question, and never
                // refuses.
                if let Some(slot) = &self.review {
                    if let Some(open) = slot.lock().unwrap().as_ref().filter(|r| !r.added) {
                        if open.installing {
                            return Err(LINK_BUSY_ADDING);
                        }
                        let owner = self.review_owner.lock().unwrap();
                        if owner
                            .as_ref()
                            .is_some_and(|(s, p)| s == session && *p == open.pointer)
                        {
                            return Err(LINK_BUSY_REVIEW_OPEN);
                        }
                    }
                }
                if within(&self.last_review, REVIEW_LINK_COOLDOWN) {
                    return Err(LINK_BUSY_TRY_AGAIN);
                }
                *self.review_owner.lock().unwrap() = Some((session.to_string(), pointer.clone()));
                Ok(ToShell::ReviewNapplet {
                    pointer: pointer.clone(),
                })
            }
        }
    }
}

/// A napplet's ledger key: its author and `d` tag.
type LedgerKey = (nostr::PublicKey, Option<String>);

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
    /// NAP-LINK admission. See [`LinkGate`].
    links: LinkGate,
    /// What each napplet has been delivered (NAP-LOCAL), shared by its open
    /// windows. Held weakly: the sessions own it, so it goes when the
    /// napplet's last window closes.
    ledgers: Mutex<HashMap<LedgerKey, Weak<std::sync::Mutex<Delivered>>>>,
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
            links: LinkGate::default(),
            ledgers: Mutex::new(HashMap::new()),
        }
    }

    /// The delivered-ids ledger for `addr`: the one its open windows share,
    /// or a fresh one when none is open.
    fn ledger_for(&self, addr: &NappletAddr) -> Ledger {
        let mut ledgers = self.ledgers.lock().unwrap();
        ledgers.retain(|_, weak| weak.strong_count() > 0);
        let key = (addr.author, addr.d_tag.clone());
        if let Some(live) = ledgers.get(&key).and_then(Weak::upgrade) {
            return live;
        }
        let fresh = Ledger::default();
        ledgers.insert(key, Arc::downgrade(&fresh));
        fresh
    }

    /// Refuse NAP-LINK reviews while `slot` holds one — the slot the app's
    /// review sheet is drawn from.
    pub fn with_review_slot(mut self, slot: ReviewSlot) -> Self {
        self.links.review = Some(slot);
        self
    }

    /// Resolve and install napplets from `blobs` rather than the capabilities'
    /// store. On the device the capabilities write what a napplet fetches to
    /// the shell cache, while an installed napplet's own files must be
    /// **kept**, as an nsite's are — never evicted, never cleared with the
    /// cache.
    pub fn with_kept_blobs(mut self, blobs: Arc<dyn BlobStore>) -> Self {
        self.blobs = blobs;
        self
    }

    /// Serve versions through `manifests` — on the device, the content layer's
    /// active-version pins — instead of the relay's newest.
    pub fn with_manifests(mut self, manifests: Arc<dyn ManifestStore>) -> Self {
        self.manifests = manifests;
        self
    }

    /// As [`NappletHost::open_with`], for a napplet with nothing switched off
    /// whose review showed exactly what the served manifest declares:
    /// `granted` is what the user approved, or `None` for one not installed.
    /// A test convenience — the device always goes through `open_with` with
    /// what the Library recorded.
    pub async fn open(
        &self,
        addr: &NappletAddr,
        granted: Option<Vec<String>>,
    ) -> anyhow::Result<OpenedNapplet> {
        self.open_inner(
            addr,
            granted.map(|granted| crate::content::NappletGrants {
                granted,
                denied: Vec::new(),
                reviewed: Vec::new(),
            }),
            true,
        )
        .await
    }

    /// Resolve a napplet from the local stores and open a session for it.
    ///
    /// `grants` is what the Library records — what the user allowed, what
    /// they switched off, and what the review sheet showed them — or `None`
    /// for a napplet that is not installed, which opens with nothing but the
    /// handshake. A napplet that fails verification never gets a session: the
    /// error propagates and no window opens.
    pub async fn open_with(
        &self,
        addr: &NappletAddr,
        grants: Option<crate::content::NappletGrants>,
    ) -> anyhow::Result<OpenedNapplet> {
        self.open_inner(addr, grants, false).await
    }

    /// The open behind both entry points. `reviewed_is_declared` stands in
    /// for a reviewed list equal to the served manifest's `requires` — what
    /// [`NappletHost::open`] promises.
    async fn open_inner(
        &self,
        addr: &NappletAddr,
        grants: Option<crate::content::NappletGrants>,
        reviewed_is_declared: bool,
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
        // the user agreed to was the list the review sheet showed them, plus
        // the defaults — the **reviewed** list, recorded in the Library. This
        // build implementing more of *that* list does not change the
        // agreement, so those domains are granted at open.
        //
        // The served manifest is not the agreement. It is whatever version
        // the update check pinned, and an update check shows no screen: a v2
        // that declares more than the v1 the user reviewed does not get the
        // extra on the strength of having been pinned. Those domains are
        // returned as `unreviewed`, ungranted, and the caller puts them in
        // front of the user; installing from that sheet records the new
        // reviewed list. An entry with an empty reviewed list — written by
        // this branch before the list existed, never released — widens over
        // the defaults only, and anything more it declares is reviewed once.
        //
        // And what the user switched off stays off. A domain in `denied` is
        // a decision, and a launch does not get to overrule it — that is the
        // difference between "never decided" and "said no", and why the two
        // are stored apart. Nothing undeclared is added, nothing is added to a
        // napplet that was never installed, and the long-press sheet shows
        // the result.
        let requires = resolved.manifest.requires.clone();
        let mut unreviewed: Vec<String> = Vec::new();
        let grants = match grants {
            None => crate::content::NappletGrants::default(),
            Some(mut grants) => {
                let reviewed = if reviewed_is_declared {
                    effective_grants(&requires)
                } else {
                    effective_grants(&grants.reviewed)
                };
                for domain in effective_grants(&requires) {
                    if grants.granted.contains(&domain) || grants.denied.contains(&domain) {
                        continue;
                    }
                    if !reviewed.contains(&domain) {
                        tracing::info!(
                            napplet = %addr.d_tag.as_deref().unwrap_or("<root>"),
                            %domain,
                            "the served manifest declares a capability the user never reviewed; not granted"
                        );
                        unreviewed.push(domain);
                        continue;
                    }
                    tracing::info!(
                        napplet = %addr.d_tag.as_deref().unwrap_or("<root>"),
                        %domain,
                        "granting a reviewed capability this build newly implements"
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
            ?unreviewed,
            "opening napplet"
        );
        let session = Session::new(
            NappletIdentity::from(&resolved),
            grants.granted.iter().cloned(),
        )
        .with_ledger(self.ledger_for(addr));
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
                opened: OpenedVersion {
                    addr: addr.clone(),
                    created_at: event.created_at,
                    aggregate: resolved.aggregate.clone(),
                },
                blob_token: new_blob_token(),
            },
        );

        Ok(OpenedNapplet {
            session_id,
            shell_host,
            title,
            grants,
            requires,
            unreviewed,
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
                Some(live) => vec![ToShell::load(
                    &live.artifact,
                    &format!("/_blob/{}/", live.blob_token),
                )],
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

        // NAP-LINK: the runtime classified the link, the host decides. An
        // admitted link goes to the window host as a command *and* is
        // answered `opened`; the command comes first so the window acts on
        // it before the napplet hears back.
        if let Outcome::Link(request) = &out {
            return match self
                .links
                .admit(session_id, &request.target, std::time::Instant::now())
            {
                Ok(command) => {
                    tracing::info!(session = session_id, ?command, "napplet link admitted");
                    vec![command, ToShell::to_napplet(request.opened())]
                }
                Err(error) => {
                    tracing::info!(
                        session = session_id,
                        target = ?request.target,
                        error,
                        "napplet link refused"
                    );
                    vec![ToShell::to_napplet(request.denied(error))]
                }
            };
        }

        out.envelopes()
            .iter()
            .cloned()
            .map(ToShell::to_napplet)
            .collect()
    }

    /// Record the app's light/dark appearance for one window, and push
    /// NAP-THEME's `theme.changed` if it changed and the napplet may hear it.
    ///
    /// The window host calls this right after open (before the handshake, so
    /// the first `theme.get` is already right) and again on every
    /// configuration change.
    pub async fn set_appearance(&self, session_id: &str, appearance: Appearance) {
        let (session, outbox) = {
            let sessions = self.sessions.lock().unwrap();
            match sessions.get(session_id) {
                Some(live) => (live.session.clone(), live.outbox.clone()),
                None => return,
            }
        };
        let mut session = session.lock().await;
        if session.set_appearance(appearance) {
            if let Some(push) = myco_napplet_runtime::nap::theme::changed_frame(&session) {
                let _ = outbox.send(ToShell::to_napplet(push));
            }
        }
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

    /// Tell every open napplet granted `identity` that the user changed:
    /// NAP-IDENTITY's `identity.changed`, a hex pubkey on login and `""` on
    /// logout. A napplet re-runs its identity-dependent work on it instead of
    /// holding the old answer until it is reopened.
    pub async fn identity_changed(&self, pubkey_hex: &str) {
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
            if session.lock().await.may_service("identity") {
                let _ = outbox.send(ToShell::to_napplet(
                    myco_napplet_runtime::nap::identity::changed(pubkey_hex),
                ));
            }
        }
    }

    /// The bytes of a blob this window's napplet was delivered, for the
    /// window host to serve at `/_blob/<token>/<sha256>` — how NAP-RESOURCE
    /// bytes reach the shell without riding the JSON channel.
    ///
    /// `None` unless the token is this window's, the napplet still holds
    /// `resource`, and the blob was delivered to it (it is then in the
    /// store; one evicted since is `None` too, and the shell says so).
    pub async fn blob(&self, session_id: &str, token: &str, sha256_hex: &str) -> Option<Vec<u8>> {
        let key = myco_napplet_runtime::delivered::parse_hex32(sha256_hex)?;
        let session = {
            let sessions = self.sessions.lock().unwrap();
            let live = sessions.get(session_id)?;
            if live.blob_token.is_empty() || live.blob_token != token {
                return None;
            }
            live.session.clone()
        };
        {
            let session = session.lock().await;
            if !session.is_granted("resource") || !session.was_delivered_blob(&key) {
                return None;
            }
        }
        self.ctx
            .blobs
            .get(&sha256_hex.to_ascii_lowercase())
            .await
            .ok()?
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

    /// The aggregate of the version now served for this window's napplet, when
    /// it is newer than the one the window opened — or `None` while the window
    /// is current, and for a session that is not open.
    ///
    /// A window keeps the version it opened for its whole life (design doc
    /// §7.2); an update check or a Circle push moves the pin under it. The
    /// Activity asks this when the window comes back to the foreground, to
    /// offer a restart onto the new version. The served version is the pinned
    /// one, and a pin moves only once its bytes are here — so a restart this
    /// answers "yes" for opens the new version, offline or not.
    ///
    /// Newer means a later-or-equal `created_at` **and** a different
    /// aggregate: a re-signed manifest over the same bytes is not an update
    /// worth restarting for, and a pin never moves back, so an older served
    /// version is not expected — and would not be offered if it were.
    pub async fn newer_version(&self, session_id: &str) -> Option<String> {
        let (addr, created_at, aggregate) = {
            let sessions = self.sessions.lock().unwrap();
            let opened = &sessions.get(session_id)?.opened;
            (
                opened.addr.clone(),
                opened.created_at,
                opened.aggregate.clone(),
            )
        };
        let served = self
            .manifests
            .current(addr.kind(), &addr.author, addr.d_tag.as_deref())
            .await
            .ok()
            .flatten()?;
        if served.created_at < created_at {
            return None;
        }
        let served = myco_napplet_runtime::manifest::NappletManifest::from_event(served).ok()?;
        (served.aggregate != aggregate).then_some(served.aggregate)
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
        ingest_into(
            self.relay.as_ref(),
            self.blobs.as_ref(),
            self.manifests.as_ref(),
            addr,
            source,
        )
        .await
    }

    /// Bring in the napplet a manifest describes: fetch its bytes from
    /// `source`, verify them against the manifest, and keep both. For install,
    /// after review ran on the manifest alone ([`fetch_manifest`]).
    pub async fn ingest_event(
        &self,
        event: nostr::Event,
        source: &dyn PeerSource,
    ) -> anyhow::Result<IngestedNapplet> {
        ingest_event_into(
            self.relay.as_ref(),
            self.blobs.as_ref(),
            self.manifests.as_ref(),
            event,
            source,
        )
        .await
    }
}

/// Fetch a napplet's manifest only — no bytes — and check what can be
/// checked without them: the signature, that it is the author and address
/// asked for, and that it parses as a NIP-5D manifest. What install review
/// needs; nothing is downloaded or kept until the user says yes.
pub async fn fetch_manifest(
    addr: &NappletAddr,
    source: &dyn PeerSource,
) -> anyhow::Result<(nostr::Event, IngestedNapplet)> {
    let event = source
        .fetch_manifest(&addr.author, addr.d_tag.as_deref())
        .await?
        .ok_or_else(|| anyhow::anyhow!("no napplet manifest at that address"))?;
    event
        .verify()
        .map_err(|e| anyhow::anyhow!("manifest signature: {e}"))?;
    anyhow::ensure!(
        event.pubkey == addr.author,
        "the manifest is not by the napplet's author"
    );
    let m = myco_napplet_runtime::manifest::NappletManifest::from_event(event.clone())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    anyhow::ensure!(
        m.d_tag.as_deref() == addr.d_tag.as_deref(),
        "the manifest is for a different napplet"
    );
    let found = IngestedNapplet {
        requires: m.requires,
        title: m.title,
        description: m.description,
        d_tag: m.d_tag.unwrap_or_default(),
        aggregate: m.aggregate,
    };
    Ok((event, found))
}

/// The same untrusted-source, verify-before-keep path as
/// [`NappletHost::ingest`], for a caller that has the three seams but no host.
///
/// The first-run seed is that caller: it runs before any napplet has opened,
/// and standing up a host through `AppRuntime::napplet_context` would
/// generate the user key, which D3 reserves for first napplet use. `source`
/// is not trusted — every byte it returns is hashed and the signature and
/// aggregate checked before any of it is kept.
pub async fn ingest_into(
    relay: &dyn RelayBackend,
    blobs: &dyn BlobStore,
    manifests: &dyn ManifestStore,
    addr: &NappletAddr,
    source: &dyn PeerSource,
) -> anyhow::Result<IngestedNapplet> {
    let event = source
        .fetch_manifest(&addr.author, addr.d_tag.as_deref())
        .await?
        .ok_or_else(|| anyhow::anyhow!("no napplet manifest at that address"))?;
    ingest_event_into(relay, blobs, manifests, event, source).await
}

/// The second half of [`ingest_into`]: the bytes for a manifest already in
/// hand. `source` is not trusted; `resolve` verifies the signature and every
/// byte before anything is kept. The manifest's author and address are the
/// caller's to have checked — install hands in one [`fetch_manifest`]
/// accepted.
pub async fn ingest_event_into(
    relay: &dyn RelayBackend,
    blobs: &dyn BlobStore,
    manifests: &dyn ManifestStore,
    event: nostr::Event,
    source: &dyn PeerSource,
) -> anyhow::Result<IngestedNapplet> {
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
    blobs.put(resolved.index_html.as_bytes()).await?;
    relay.publish(event.clone()).await?;
    manifests.pin(&event);

    Ok(IngestedNapplet {
        requires: resolved.manifest.requires.clone(),
        title: resolved.manifest.title.clone(),
        description: resolved.manifest.description.clone(),
        d_tag: resolved.d_tag.clone(),
        aggregate: resolved.aggregate.clone(),
    })
}

impl NappletHost {
    /// Fetch whatever `source` has for `addr` and, if it is a newer version
    /// than the one served, bring it in — bytes first, so the served version
    /// moves only when the new one can open. Returns the manifest now served
    /// when it moved, so the caller can pass it on to the Circle.
    ///
    /// The update path for napplets: the same [`NappletHost::ingest`] the first
    /// fetch used, gated on version. A source with nothing newer, or nothing at
    /// all, leaves the served version alone.
    pub async fn refresh(
        &self,
        addr: &NappletAddr,
        source: &dyn PeerSource,
    ) -> anyhow::Result<Option<nostr::Event>> {
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
            return Ok(None);
        }
        // The source is untrusted and its answer is about to be passed on to
        // the Circle: it must be this napplet, not whatever was asked for.
        anyhow::ensure!(
            offered.pubkey == addr.author
                && offered.kind.as_u16() == addr.kind()
                && offered.tags.identifier() == addr.d_tag.as_deref(),
            "the source offered a manifest for another napplet"
        );
        // What was checked is what is kept: `ingest` would ask the source
        // again, and a source answering differently the second time must not
        // slip in a version the newer-than check never saw.
        self.ingest_event(offered.clone(), source).await?;
        Ok(Some(offered))
    }
}

/// Refresh every installed napplet from the public relays, in parallel.
/// Returns the manifests that moved — for the update-check toast, and for the
/// caller to pass on to the Circle as it does an nsite update — and how many
/// were checked.
///
/// Public relays only — the hints, the author's NIP-65 relays and the
/// defaults: a napplet's author publishes there, and the holder who shared
/// it is not recorded. Offline-only skips the lot — `checked`
/// still counts them, so the toast says they were not updated rather than
/// that there were none.
pub async fn refresh_all(host: &NappletHost, addrs: &[NappletAddr]) -> (Vec<nostr::Event>, usize) {
    let checks = addrs.iter().map(|addr| async move {
        match host
            .refresh(addr, &addr.public_source(host.relay.clone()))
            .await
        {
            Ok(moved) => moved,
            Err(e) => {
                tracing::debug!(
                    napplet = %addr.d_tag.as_deref().unwrap_or("<root>"),
                    error = %e,
                    "napplet update check: no newer version reachable"
                );
                None
            }
        }
    });
    let results = futures_util::future::join_all(checks).await;
    (results.into_iter().flatten().collect(), addrs.len())
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

/// Frames for a window, one compact JSON object per line — how they cross
/// the FFI. Compact JSON never holds a raw newline (one inside a string is
/// written `\n`), so the Kotlin side splits on it and hands each frame on
/// as it is. A JSON array made that side parse every frame, pictures as
/// megabytes of base64 included, only to write it back out.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn frames_as_lines(frames: &[ToShell]) -> String {
    frames
        .iter()
        .filter_map(|f| serde_json::to_string(f).ok())
        .collect::<Vec<_>>()
        .join("\n")
}

/// NAP-RESOURCE's fetcher: where a blob this device does not hold is looked
/// for — the Circle's Blossom stores over the mesh and, when the internet is
/// allowed, the servers the URL named, the author's servers (kind 10063, from
/// what this device holds) and the public defaults, in that order.
///
/// Only ever reached on a local miss; the handler asks the store first and
/// keeps whatever this returns. Mesh and internet are asked **at once** and
/// the first to bring the bytes wins: a feed of pictures must not wait out a
/// mesh timeout per picture before the internet is tried. A picture one
/// phone in the room fetched is still one the others get over the mesh — it
/// is simply no longer made to wait for it.
///
/// A blob nobody had is remembered as missing for [`MISS_REMEMBERED`], so a
/// feed scrolling past a dead link does not ask the whole world again on
/// every scroll.
pub struct BlossomFetcher {
    content: Arc<crate::content::Content>,
    /// The public Blossom servers. The defaults, unless a test says otherwise.
    public_servers: Vec<String>,
    /// One client for every internet fetch, so its connections are reused.
    http: reqwest::Client,
    /// Blobs nobody had, by sha256, with when that was found.
    misses: Mutex<HashMap<String, std::time::Instant>>,
}

/// How long one mesh peer gets.
const MESH_BLOB_TIMEOUT: Duration = Duration::from_secs(8);
/// How long the internet servers get, all together.
/// Long enough for a resource at the cap (64 MiB) on a slow mobile link; a
/// dead or silent server is cut off far sooner by the connect timeout and
/// [`INTERNET_READ_TIMEOUT`].
const INTERNET_BLOB_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a download may go without a byte before it is given up on.
const INTERNET_READ_TIMEOUT: Duration = Duration::from_secs(15);
/// How long one internet server gets to accept the connection, so a dead
/// hint does not eat the whole budget before the defaults are asked.
const INTERNET_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a blob nobody had is not asked for again.
const MISS_REMEMBERED: Duration = Duration::from_secs(10 * 60);
/// The most misses remembered; past it the expired go, then all of them.
const MAX_MISSES: usize = 4096;
/// The most authors whose server lists one fetch looks up.
const MAX_HINT_AUTHORS: usize = 4;

impl BlossomFetcher {
    pub fn new(content: Arc<crate::content::Content>) -> Self {
        Self {
            content,
            public_servers: crate::ip_source::default_blossom_servers(),
            http: reqwest::Client::builder()
                .connect_timeout(INTERNET_CONNECT_TIMEOUT)
                .timeout(INTERNET_BLOB_TIMEOUT)
                .read_timeout(INTERNET_READ_TIMEOUT)
                .build()
                .unwrap_or_default(),
            misses: Mutex::new(HashMap::new()),
        }
    }

    fn recently_missed(&self, sha256_hex: &str) -> bool {
        let mut misses = self.misses.lock().unwrap();
        match misses.get(sha256_hex) {
            Some(at) if at.elapsed() < MISS_REMEMBERED => true,
            Some(_) => {
                misses.remove(sha256_hex);
                false
            }
            None => false,
        }
    }

    fn remember_miss(&self, sha256_hex: &str) {
        let mut misses = self.misses.lock().unwrap();
        if misses.len() >= MAX_MISSES {
            misses.retain(|_, at| at.elapsed() < MISS_REMEMBERED);
            if misses.len() >= MAX_MISSES {
                misses.clear();
            }
        }
        misses.insert(sha256_hex.to_string(), std::time::Instant::now());
    }

    /// Every reachable Circle member at once; the first to bring it wins. A
    /// peer that does not hold it answers quickly with nothing, and a peer
    /// that is gone hits the bound.
    async fn ask_mesh(&self, sha256_hex: &str, max_bytes: usize) -> Option<Vec<u8>> {
        use futures_util::StreamExt as _;
        let pool = self.content.peer_relays();
        let mut asks: futures_util::stream::FuturesUnordered<_> = self
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
            })
            .collect();
        while let Some(found) = asks.next().await {
            if found.is_some() {
                return found;
            }
        }
        None
    }

    /// The servers the authors named in their kind 10063 lists, as held
    /// here. Nothing is fetched for this: a list not held costs no wait.
    async fn author_servers(&self, authors: &[String]) -> Vec<String> {
        let authors: Vec<PublicKey> = authors
            .iter()
            .take(MAX_HINT_AUTHORS)
            .filter_map(|a| PublicKey::from_hex(a).ok())
            .collect();
        if authors.is_empty() {
            return Vec::new();
        }
        let filter = nostr::Filter::new()
            .kind(nostr::Kind::from(10_063u16))
            .authors(authors);
        let Ok(lists) = self.content.relay().query(&[filter]).await else {
            return Vec::new();
        };
        let mut servers = Vec::new();
        for list in &lists {
            for tag in list.tags.iter() {
                let tag = tag.as_slice();
                if tag.first().map(String::as_str) == Some("server") {
                    if let Some(url) = tag.get(1).filter(|u| u.starts_with("https://")) {
                        let url = url.trim_end_matches('/').to_string();
                        if !servers.contains(&url) {
                            servers.push(url);
                        }
                    }
                }
            }
        }
        servers
    }

    /// The internet's answer: `None` when it was not asked (it looks down).
    async fn ask_internet(
        &self,
        sha256_hex: &str,
        max_bytes: usize,
        hints: &myco_napplet_runtime::BlobHints,
    ) -> Option<anyhow::Result<Option<Vec<u8>>>> {
        if self.content.internet_looks_down() {
            return None;
        }
        let mut servers = hints.servers.clone();
        for server in self.author_servers(&hints.authors).await {
            if !servers.contains(&server) {
                servers.push(server);
            }
        }
        let public = crate::ip_source::IpPeerSource::new(Vec::new(), self.public_servers.clone())
            .with_http_client(self.http.clone())
            .with_max_blob_bytes(max_bytes);
        Some(
            match tokio::time::timeout(
                INTERNET_BLOB_TIMEOUT,
                public.fetch_blob(sha256_hex, &servers),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Ok(None),
            },
        )
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
    async fn fetch(
        &self,
        sha256_hex: &str,
        max_bytes: usize,
        hints: &myco_napplet_runtime::BlobHints,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        if self.recently_missed(sha256_hex) {
            return Ok(None);
        }
        let mesh = self.ask_mesh(sha256_hex, max_bytes);
        let internet = self.ask_internet(sha256_hex, max_bytes, hints);
        tokio::pin!(mesh, internet);
        let (mut mesh_done, mut internet_done) = (false, false);
        let mut internet_said = None;
        while !(mesh_done && internet_done) {
            tokio::select! {
                found = &mut mesh, if !mesh_done => {
                    mesh_done = true;
                    if found.is_some() {
                        return Ok(found);
                    }
                }
                answer = &mut internet, if !internet_done => {
                    internet_done = true;
                    if let Some(Ok(Some(bytes))) = answer {
                        return Ok(Some(bytes));
                    }
                    internet_said = answer;
                }
            }
        }
        match internet_said {
            // Only a miss the internet confirmed is remembered: with it
            // down, the mesh alone saying no says little, and a peer may
            // join in a minute.
            Some(Ok(None)) => {
                self.remember_miss(sha256_hex);
                Ok(None)
            }
            Some(Err(e)) => Err(e),
            _ => Ok(None),
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

    /// Kept and flooded again even when already seen — the hub's seen-set
    /// would otherwise stop a rebroadcast at this phone. Peers' own seen-sets
    /// still end the flood.
    async fn rebroadcast(&self, event: nostr::Event, ttl: u8) -> anyhow::Result<()> {
        // Only what the push plane floods at all: not manifests (their own
        // interest-aware path) and not gift wraps (addressed to one phone).
        let kind = event.kind.as_u16();
        if !crate::gossip::is_gossip_eligible(kind) || kind == 1059 {
            anyhow::bail!("kind {kind} is not passed on over the mesh");
        }
        let ttl = ttl.min(self.limits.read().unwrap().publish_ttl);
        let hub = self.hub.lock().unwrap().clone();
        match hub {
            Some(hub) => hub.rebroadcast_local(event, ttl).await,
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
                match hub.accept_pulled(event).await {
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
    /// The user said yes and the app's bytes are downloading. Review runs on
    /// the manifest alone — nothing is downloaded before the answer — so this
    /// is the wait between "Add" and the app landing on the grid.
    #[serde(default)]
    pub installing: bool,
    /// The download landed and the answer was recorded: the sheet stays up
    /// saying so, and offers to open the app, until the user closes it
    /// (`DismissNappletReview`). The other fields describe the review as it
    /// was answered — `installed` true with nothing `unreviewed` means this
    /// was a "Download again", not a first add. Unlike a review still asking,
    /// it does not hold the slot: a napplet's `link.open` to another napplet
    /// is admitted and its review replaces this one.
    #[serde(default)]
    pub added: bool,
    /// This napplet — the same author and `d` tag — is already in the
    /// Library. With nothing in [`Self::unreviewed`], adding it again would
    /// change nothing, so the sheet says it is installed instead of offering
    /// Add.
    #[serde(default)]
    pub installed: bool,
    /// Installed, and its files are on this phone (the tile reads "Ready").
    /// An installed napplet that is not — seeded offline, or its blobs gone —
    /// comes back through this sheet to be downloaded again, so the sheet
    /// offers that instead of "Already installed".
    #[serde(default)]
    pub ready: bool,
    /// For an installed napplet: what this manifest would grant that the user
    /// never reviewed and never decided on — an update declaring more. The
    /// sheet still asks about these; answering records the new reviewed list.
    /// Empty when the napplet is not installed, where everything is new.
    #[serde(default)]
    pub unreviewed: Vec<String>,
    /// The fetched manifest, kept so install downloads exactly what was
    /// reviewed rather than whatever the relays hold by then. Never sent to
    /// Kotlin.
    #[serde(skip)]
    pub manifest: Option<nostr::Event>,
    /// Set when the fetch failed; the screen shows this instead of asking.
    pub error: String,
    /// The peer who shared it, when it arrived by a tap or a scan — kept so a
    /// retry tries their phone first again, exactly as the first attempt did.
    /// Without it a retry in a room with no internet would search blind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder: Option<String>,
}

/// Where a napplet under review stands against the Library: whether it is
/// installed, and if so what the manifest declaring `requires` would grant
/// that the installed copy never reviewed ([`unreviewed_domains`]).
pub fn library_standing(
    content: &crate::content::Content,
    addr: &NappletAddr,
    requires: &[String],
) -> (bool, Vec<String>) {
    use nostr::nips::nip19::ToBech32;
    let npub = addr.author.to_bech32().unwrap_or_default();
    match library_grants(content, &npub, addr.d_tag.as_deref()) {
        None => (false, Vec::new()),
        Some(grants) => (true, unreviewed_domains(&grants, requires)),
    }
}

/// What the Library records for a napplet, as an open or a review should
/// read it: for a preinstalled default still on its seeded entry, the
/// permissions Myco vetted it for count as reviewed on top of whatever the
/// entry's own list says. `None` for a napplet that is not installed.
///
/// Evaluated here, at every read, rather than only written at seed time: a
/// review answered since may have recorded a shorter list, and an expected
/// set widened by a later release reaches the entries seeded before it. It
/// widens the *reviewed* list only — nothing is granted unless a served
/// version declares it, and a domain in `denied` stays off (`open_with`).
pub fn library_grants(
    content: &crate::content::Content,
    npub: &str,
    d_tag: Option<&str>,
) -> Option<crate::content::NappletGrants> {
    library_grants_with(
        content,
        npub,
        d_tag,
        crate::runtime::default_napplet_expected,
    )
}

/// [`library_grants`], with the table of defaults' expected permissions
/// passed in — the real one names addresses a test cannot sign for.
fn library_grants_with(
    content: &crate::content::Content,
    npub: &str,
    d_tag: Option<&str>,
    expected_for: impl Fn(&str, Option<&str>) -> Option<Vec<String>>,
) -> Option<crate::content::NappletGrants> {
    let mut grants = content.napplet_grants(npub, d_tag)?;
    if content.napplet_is_preinstalled(npub, d_tag) {
        for domain in expected_for(npub, d_tag).unwrap_or_default() {
            if !grants.reviewed.contains(&domain) {
                grants.reviewed.push(domain);
            }
        }
    }
    Some(grants)
}

/// Whether the installed napplet at `addr` can open: its tile status is
/// `ready` (the served manifest and its index blob are both here).
pub fn is_ready_here(content: &crate::content::Content, addr: &NappletAddr) -> bool {
    let host =
        myco_napplet_runtime::host::shell_host(&addr.author.to_bytes(), addr.d_tag.as_deref());
    content
        .napplet_status_snapshot()
        .into_iter()
        .any(|s| s.host == host && s.state == "ready")
}

/// What a manifest declaring `requires` would grant that an installed
/// napplet's user was never asked about: not on the reviewed list, and neither
/// granted nor switched off. The same rule [`NappletHost::open_with`] applies
/// to the served version at open.
pub fn unreviewed_domains(
    grants: &crate::content::NappletGrants,
    requires: &[String],
) -> Vec<String> {
    let reviewed = effective_grants(&grants.reviewed);
    effective_grants(requires)
        .into_iter()
        .filter(|d| !grants.granted.contains(d) && !grants.denied.contains(d))
        .filter(|d| !reviewed.contains(d))
        .collect()
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

    /// A napplet's windows share one ledger of what it was delivered, and it
    /// goes with the last of them.
    #[test]
    fn the_ledger_is_shared_by_windows_and_dropped_with_the_last() {
        let host = NappletHost::new(test_ctx(
            Arc::new(MemRelay::new()),
            Arc::new(nsite_deck::testing::MemBlobs::new()),
        ));
        let keys = nostr::Keys::generate();
        let addr = NappletAddr {
            author: keys.public_key(),
            d_tag: Some("chat".to_string()),
            relays: Vec::new(),
        };
        let other = NappletAddr {
            d_tag: Some("other".to_string()),
            ..addr.clone()
        };
        let first = host.ledger_for(&addr);
        let second = host.ledger_for(&addr);
        assert!(Arc::ptr_eq(&first, &second), "two windows, two ledgers");
        assert!(!Arc::ptr_eq(&first, &host.ledger_for(&other)));
        first.lock().unwrap().record_event(&[7u8; 32]);
        drop(first);
        assert!(second.lock().unwrap().has_event(&[7u8; 32]));
        drop(second);
        let reopened = host.ledger_for(&addr);
        assert!(
            !reopened.lock().unwrap().has_event(&[7u8; 32]),
            "the ledger outlived the napplet's windows"
        );
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
            blobs: blobs.clone(),
            fetcher: Arc::new(myco_napplet_runtime::seams::NoFetcher),
            kept_blobs: blobs,
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
        host_with(NappletBuilder::new()).await
    }

    /// A host over in-memory seams serving the napplet `builder` makes, which
    /// must keep the fixture's `d` tag.
    async fn host_with(builder: NappletBuilder) -> (NappletHost, NappletAddr) {
        let napplet = builder.build();
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

    /// What one window of a napplet was shown, another window of the same
    /// napplet may keep — through the host's snapshot path, not the ledger
    /// alone.
    #[tokio::test]
    async fn a_second_window_keeps_what_the_first_was_shown() {
        let (host, addr) = host_with_fixture().await;
        let granted = Some(vec!["relay".to_string(), "local".to_string()]);
        let mut windows = Vec::new();
        for _ in 0..2 {
            let opened = host.open(&addr, granted.clone()).await.unwrap();
            host.frame(
                &opened.session_id,
                r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#,
            )
            .await;
            windows.push(opened.session_id);
        }
        let note = nostr::EventBuilder::text_note("shown in one window")
            .sign_with_keys(&nostr::Keys::generate())
            .unwrap();
        host.ctx.relay.publish(note.clone()).await.unwrap();
        let keep = serde_json::json!({
            "channel": "napplet",
            "message": {"type": "local.publish", "id": "k", "event": note},
        })
        .to_string();
        let kept = |replies: Vec<ToShell>| {
            replies.into_iter().find_map(|r| match r {
                ToShell::Napplet { message } if message.msg_type == "local.publish.result" => {
                    message.field("ok").cloned()
                }
                _ => None,
            })
        };

        assert_eq!(
            kept(host.frame(&windows[1], &keep).await),
            Some(serde_json::json!(false)),
            "kept before either window was shown it"
        );
        // Shown through a subscription's backlog, which reads this relay.
        let subscribe = serde_json::json!({
            "channel": "napplet",
            "message": {"type": "relay.subscribe", "id": "s", "subId": "s",
                        "filters": [{"ids": [note.id.to_hex()]}]},
        })
        .to_string();
        let shown = host.frame(&windows[0], &subscribe).await;
        assert!(
            format!("{shown:?}").contains(&note.id.to_hex()),
            "the backlog did not carry the note"
        );
        assert_eq!(
            kept(host.frame(&windows[1], &keep).await),
            Some(serde_json::json!(true))
        );
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

    /// A napplet naddr of the named kind, for NAP-LINK tests.
    fn napplet_naddr() -> String {
        use nostr::nips::nip01::Coordinate;
        use nostr::nips::nip19::Nip19Coordinate;
        let coordinate = Coordinate::new(
            nostr::Kind::from(KIND_NAMED),
            nostr::Keys::generate().public_key(),
        )
        .identifier("dingdong");
        Nip19Coordinate::new(coordinate, Vec::<nostr::RelayUrl>::new())
            .to_bech32()
            .unwrap()
    }

    fn link_frame(url: &str) -> String {
        serde_json::json!({
            "channel": "napplet",
            "message": {"type": "link.open", "id": "l1", "url": url}
        })
        .to_string()
    }

    /// An established session granted `granted`, on a host whose review
    /// sheet is drawn from `slot`.
    async fn linked_host(granted: &[&str], slot: ReviewSlot) -> (NappletHost, String) {
        let (host, addr) = host_with_fixture().await;
        let host = host.with_review_slot(slot);
        let opened = host
            .open(&addr, Some(granted.iter().map(|g| g.to_string()).collect()))
            .await
            .unwrap();
        host.frame(
            &opened.session_id,
            r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#,
        )
        .await;
        (host, opened.session_id)
    }

    fn link_result(frame: &ToShell) -> (String, Option<String>) {
        let ToShell::Napplet { message } = frame else {
            panic!("expected a napplet reply, got {frame:?}");
        };
        assert_eq!(message.msg_type, "link.open.result");
        (
            message
                .field("status")
                .unwrap()
                .as_str()
                .unwrap()
                .to_string(),
            message
                .field("error")
                .and_then(|e| e.as_str())
                .map(str::to_string),
        )
    }

    /// NAP-LINK to another napplet: the window is told to open the review
    /// for it — the same `FetchNapplet` path a scanned code takes — and the
    /// napplet hears `opened`. Nothing here, or anywhere on this path, can
    /// install: the frame carries a pointer and nothing else.
    #[tokio::test]
    async fn a_napplet_link_asks_the_window_for_a_review() {
        let slot: ReviewSlot = Arc::new(Mutex::new(None));
        let (host, session) = linked_host(&["link"], slot.clone()).await;
        let pointer = napplet_naddr();

        let out = host
            .frame(&session, &link_frame(&format!("nostr:{pointer}")))
            .await;
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(
            out[0],
            ToShell::ReviewNapplet {
                pointer: pointer.clone()
            }
        );
        assert_eq!(link_result(&out[1]), ("opened".into(), None));

        // While the review it opened is showing over this window, another
        // link from it is refused, and the napplet is told why.
        *slot.lock().unwrap() = Some(NappletReview {
            pointer: pointer.clone(),
            loading: true,
            title: String::new(),
            description: String::new(),
            requires: Vec::new(),
            grants: Vec::new(),
            installing: false,
            added: false,
            installed: false,
            ready: false,
            unreviewed: Vec::new(),
            manifest: None,
            error: String::new(),
            holder: None,
        });
        let out = host
            .frame(&session, &link_frame(&format!("nostr:{}", napplet_naddr())))
            .await;
        assert_eq!(out.len(), 1);
        assert_eq!(
            link_result(&out[0]),
            ("denied".into(), Some(LINK_BUSY_REVIEW_OPEN.into()))
        );
    }

    /// An added review has been answered: it is a confirmation, not a
    /// question, so a napplet link is admitted over it (and its fetch then
    /// replaces it in the slot).
    #[tokio::test]
    async fn an_added_review_does_not_block_a_napplet_link() {
        let pointer = napplet_naddr();
        let slot: ReviewSlot = Arc::new(Mutex::new(Some(NappletReview {
            pointer: pointer.clone(),
            loading: false,
            title: "Chat".into(),
            description: String::new(),
            requires: Vec::new(),
            grants: Vec::new(),
            installing: false,
            added: true,
            installed: false,
            ready: false,
            unreviewed: Vec::new(),
            manifest: None,
            error: String::new(),
            holder: None,
        })));
        let (host, session) = linked_host(&["link"], slot.clone()).await;
        let next = napplet_naddr();

        let out = host
            .frame(&session, &link_frame(&format!("nostr:{next}")))
            .await;
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out[0], ToShell::ReviewNapplet { pointer: next });
        assert_eq!(link_result(&out[1]), ("opened".into(), None));
    }

    /// A burst of review links admits one: the rest land inside the cooldown,
    /// before the first fetch has even filled the slot.
    #[tokio::test]
    async fn a_napplet_cannot_spam_review_sheets() {
        let (host, session) = linked_host(&["link"], Arc::new(Mutex::new(None))).await;
        let mut admitted = 0;
        for _ in 0..5 {
            let out = host
                .frame(&session, &link_frame(&format!("nostr:{}", napplet_naddr())))
                .await;
            if out
                .iter()
                .any(|f| matches!(f, ToShell::ReviewNapplet { .. }))
            {
                admitted += 1;
            } else {
                assert_eq!(
                    link_result(&out[0]),
                    ("denied".into(), Some(LINK_BUSY_TRY_AGAIN.into()))
                );
            }
        }
        assert_eq!(admitted, 1);
    }

    #[test]
    fn link_cooldowns_expire() {
        let gate = LinkGate::default();
        let t0 = std::time::Instant::now();
        let web = LinkTarget::Web("https://example.com".into());
        assert!(gate.admit("s1", &web, t0).is_ok());
        assert_eq!(gate.admit("s1", &web, t0), Err(BLOCKED_BY_POLICY));
        assert!(gate.admit("s1", &web, t0 + WEB_LINK_COOLDOWN).is_ok());

        let napplet = LinkTarget::Napplet(napplet_naddr());
        assert!(gate.admit("s1", &napplet, t0).is_ok());
        assert_eq!(
            gate.admit("s1", &napplet, t0 + REVIEW_LINK_COOLDOWN / 2),
            Err(LINK_BUSY_TRY_AGAIN)
        );
        assert!(gate
            .admit("s1", &napplet, t0 + REVIEW_LINK_COOLDOWN)
            .is_ok());
    }

    fn review_of(pointer: &str) -> NappletReview {
        NappletReview {
            pointer: pointer.to_string(),
            loading: false,
            title: "Update".into(),
            description: String::new(),
            requires: vec!["outbox".into()],
            grants: vec!["outbox".into()],
            installing: false,
            added: false,
            installed: true,
            ready: true,
            unreviewed: vec!["outbox".into()],
            manifest: None,
            error: String::new(),
            holder: None,
        }
    }

    /// The bug seen on a first run: AppStore's update review sat in the
    /// shared slot, on the Apps screen, while the user was in AppStore — and
    /// every Install tap there was refused. A review this window did not open
    /// is not on screen over it, so it refuses nothing; the new review
    /// replaces it.
    #[test]
    fn a_review_queued_elsewhere_does_not_block_a_napplet_link() {
        let slot: ReviewSlot = Arc::new(Mutex::new(Some(review_of(&napplet_naddr()))));
        let gate = LinkGate {
            review: Some(slot),
            ..Default::default()
        };
        let target = LinkTarget::Napplet(napplet_naddr());
        assert!(
            gate.admit("napplet-1", &target, std::time::Instant::now())
                .is_ok(),
            "a queued review blocked a link from a window it is not over"
        );
    }

    /// The review a window's link opened is on screen over that window: a
    /// second link from it is refused, saying why. Another window's link is
    /// not — its review replaces the one it cannot see.
    #[test]
    fn a_review_open_over_this_window_blocks_its_next_link() {
        let slot: ReviewSlot = Arc::new(Mutex::new(None));
        let gate = LinkGate {
            review: Some(slot.clone()),
            ..Default::default()
        };
        let t0 = std::time::Instant::now();
        let first = napplet_naddr();
        assert!(gate
            .admit("napplet-1", &LinkTarget::Napplet(first.clone()), t0)
            .is_ok());
        // The window's fetch fills the slot.
        *slot.lock().unwrap() = Some(review_of(&first));

        let later = t0 + REVIEW_LINK_COOLDOWN;
        let next = LinkTarget::Napplet(napplet_naddr());
        assert_eq!(
            gate.admit("napplet-1", &next, later),
            Err(LINK_BUSY_REVIEW_OPEN)
        );
        assert!(gate.admit("napplet-2", &next, later).is_ok());
    }

    /// A download under way keeps its sheet, whichever window asks.
    #[test]
    fn an_install_under_way_blocks_every_napplet_link() {
        let mut installing = review_of(&napplet_naddr());
        installing.installing = true;
        let gate = LinkGate {
            review: Some(Arc::new(Mutex::new(Some(installing)))),
            ..Default::default()
        };
        assert_eq!(
            gate.admit(
                "napplet-1",
                &LinkTarget::Napplet(napplet_naddr()),
                std::time::Instant::now()
            ),
            Err(LINK_BUSY_ADDING)
        );
    }

    #[tokio::test]
    async fn a_web_link_goes_to_the_browser_and_others_are_denied() {
        let (host, session) = linked_host(&["link"], Arc::new(Mutex::new(None))).await;
        let out = host
            .frame(&session, &link_frame("https://example.com/x"))
            .await;
        assert_eq!(
            out[0],
            ToShell::OpenExternal {
                url: "https://example.com/x".into()
            }
        );
        assert_eq!(link_result(&out[1]), ("opened".into(), None));

        let out = host.frame(&session, &link_frame("tel:+15550100")).await;
        assert_eq!(out.len(), 1);
        assert_eq!(
            link_result(&out[0]),
            ("denied".into(), Some("unsupported-scheme".into()))
        );
        let out = host.frame(&session, &link_frame("https://")).await;
        assert_eq!(
            link_result(&out[0]),
            ("denied".into(), Some("invalid-url".into()))
        );
    }

    /// A napplet whose `link` grant was switched off gets a refusal and no
    /// command reaches the window.
    #[tokio::test]
    async fn an_ungranted_link_reaches_no_window() {
        let (host, addr) = host_with_fixture().await;
        // Switched off on the sheet: `denied`, which a launch never widens
        // over — `link` being a default does not bring it back.
        let opened = host
            .open_with(
                &addr,
                Some(crate::content::NappletGrants {
                    granted: vec!["relay".into()],
                    denied: vec!["link".into()],
                    reviewed: Vec::new(),
                }),
            )
            .await
            .unwrap();
        let session = opened.session_id;
        host.frame(
            &session,
            r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#,
        )
        .await;
        let out = host
            .frame(&session, &link_frame(&format!("nostr:{}", napplet_naddr())))
            .await;
        assert_eq!(out.len(), 1);
        assert!(
            matches!(&out[0], ToShell::Napplet { message } if message.field("error").is_some())
        );
    }

    /// NAP-THEME: the window reports dark mode, `theme.get` answers AMOLED,
    /// and a later switch is pushed to the napplet.
    #[tokio::test]
    async fn the_theme_follows_the_windows_appearance() {
        let (host, addr) = host_with_fixture().await;
        let opened = host.open(&addr, Some(vec!["theme".into()])).await.unwrap();
        // Reported before the handshake: nothing is pushed, but the answer is
        // already right.
        host.set_appearance(&opened.session_id, Appearance::Dark)
            .await;
        host.frame(
            &opened.session_id,
            r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#,
        )
        .await;
        let out = host
            .frame(
                &opened.session_id,
                r#"{"channel":"napplet","message":{"type":"theme.get","id":"t1"}}"#,
            )
            .await;
        let ToShell::Napplet { message } = &out[0] else {
            panic!("expected a reply");
        };
        assert_eq!(message.field("theme").unwrap()["title"], "Myco AMOLED");
        assert!(host
            .next_frames(&opened.session_id, Duration::from_millis(10))
            .await
            .is_empty());

        // Switched to light while open: pushed once, not again for the same.
        host.set_appearance(&opened.session_id, Appearance::Light)
            .await;
        host.set_appearance(&opened.session_id, Appearance::Light)
            .await;
        let pushed = host
            .next_frames(&opened.session_id, Duration::from_millis(50))
            .await;
        assert_eq!(pushed.len(), 1);
        let ToShell::Napplet { message } = &pushed[0] else {
            panic!("expected a push");
        };
        assert_eq!(message.msg_type, "theme.changed");
        assert_eq!(message.field("theme").unwrap()["title"], "Myco Light");
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
    /// implements gets that domain if it declared it **and the user reviewed
    /// it** — and a napplet that was never installed gets nothing whatever it
    /// declares.
    #[tokio::test]
    async fn an_installed_napplets_grants_widen_to_what_it_declared() {
        let (host, addr) = host_with_fixture().await; // declares shell, relay
        let stored = crate::content::NappletGrants {
            granted: vec![],
            denied: vec![],
            reviewed: vec!["shell".into(), "relay".into()],
        };
        let opened = host.open_with(&addr, Some(stored)).await.unwrap();
        assert!(opened.unreviewed.is_empty(), "{:?}", opened.unreviewed);
        let mut granted = opened.granted().to_vec();
        granted.sort();
        let mut expected: Vec<String> = effective_grants(&["shell".into(), "relay".into()]);
        expected.sort();
        assert_eq!(granted, expected);
        assert!(granted.contains(&"relay".to_string()));
        assert!(
            !granted.contains(&"outbox".to_string()),
            "undeclared, non-default: not granted"
        );

        let stranger = host.open(&addr, None).await.unwrap();
        assert!(
            stranger.granted().is_empty(),
            "an uninstalled napplet was granted something"
        );
        assert!(
            stranger.unreviewed.is_empty(),
            "an uninstalled napplet has nothing to review at open"
        );
    }

    /// `mesh` became a default after napplets were already installed. One the
    /// user never decided on picks it up at its next open, silently — there
    /// is no sheet for a default. One where the user switched `mesh` off keeps
    /// it off: a default does not overrule a decision.
    #[tokio::test]
    async fn a_new_mesh_default_does_not_overrule_a_switched_off_mesh() {
        let (host, addr) = host_with_fixture().await; // declares shell, relay

        let never_decided = crate::content::NappletGrants {
            granted: vec!["shell".into(), "relay".into()],
            denied: vec![],
            reviewed: vec!["shell".into(), "relay".into()],
        };
        let opened = host.open_with(&addr, Some(never_decided)).await.unwrap();
        assert!(opened.granted().contains(&"mesh".to_string()));
        assert!(opened.unreviewed.is_empty(), "{:?}", opened.unreviewed);

        let switched_off = crate::content::NappletGrants {
            granted: vec!["shell".into(), "relay".into()],
            denied: vec!["mesh".into()],
            reviewed: vec!["shell".into(), "relay".into()],
        };
        let opened = host.open_with(&addr, Some(switched_off)).await.unwrap();
        assert!(
            !opened.granted().contains(&"mesh".to_string()),
            "the new default overruled a mesh the user switched off"
        );
        assert!(opened.granted().contains(&"relay".to_string()));
        assert!(opened.unreviewed.is_empty(), "{:?}", opened.unreviewed);
    }

    /// A pinned update that declares more than the version the user reviewed
    /// does not get the extra at open: the update check showed no screen. The
    /// domain comes back as unreviewed for the sheet, and is granted only
    /// once a review that showed it has been recorded.
    #[tokio::test]
    async fn an_update_that_declares_more_is_not_granted_until_reviewed() {
        let (host, addr) = host_with(NappletBuilder::new().requires(&["relay", "outbox"])).await;

        // v1 was reviewed with `relay`; the served v2 now also declares `outbox`.
        let stored = crate::content::NappletGrants {
            granted: vec![],
            denied: vec![],
            reviewed: vec!["relay".into()],
        };
        let opened = host.open_with(&addr, Some(stored)).await.unwrap();
        assert!(
            opened.granted().contains(&"relay".to_string()),
            "a reviewed, declared domain was not granted"
        );
        assert!(
            !opened.granted().contains(&"outbox".to_string()),
            "an update granted itself a domain nobody reviewed"
        );
        assert_eq!(opened.unreviewed, vec!["outbox".to_string()]);
        assert_eq!(
            opened.requires,
            vec!["relay".to_string(), "outbox".to_string()]
        );

        // Reviewed again, with the new list: now it is granted.
        let reviewed = crate::content::NappletGrants {
            granted: vec![],
            denied: vec![],
            reviewed: vec!["relay".into(), "outbox".into()],
        };
        let opened = host.open_with(&addr, Some(reviewed)).await.unwrap();
        assert!(opened.granted().contains(&"outbox".to_string()));
        assert!(opened.unreviewed.is_empty());

        // And a decision already made is not "unreviewed", whatever the list
        // says: an entry that predates the reviewed list keeps its grants and
        // is not asked about a domain it already switched off.
        let decided = crate::content::NappletGrants {
            granted: vec!["relay".into()],
            denied: vec!["outbox".into()],
            reviewed: vec![],
        };
        let opened = host.open_with(&addr, Some(decided)).await.unwrap();
        assert!(opened.unreviewed.is_empty(), "{:?}", opened.unreviewed);
        assert!(!opened.granted().contains(&"outbox".to_string()));
    }

    /// The shape the first-run seed writes — the defaults granted, nothing
    /// reviewed — pins to "asks at first open": every declared, non-default
    /// domain comes back as unreviewed for the sheet, and the window opens
    /// with the defaults and nothing more.
    #[tokio::test]
    async fn a_seeded_napplet_reviews_what_it_declares_on_first_open() {
        let (host, addr) = host_with(NappletBuilder::new().requires(&["relay", "outbox"])).await;

        let seeded = crate::content::NappletGrants {
            granted: effective_grants(&[]),
            denied: vec![],
            reviewed: vec![],
        };
        let opened = host.open_with(&addr, Some(seeded)).await.unwrap();
        assert_eq!(opened.unreviewed, vec!["outbox".to_string()]);
        let mut granted = opened.granted().to_vec();
        granted.sort();
        assert_eq!(granted, effective_grants(&[]));
        assert!(
            !granted.contains(&"outbox".to_string()),
            "a seed granted a domain nobody reviewed"
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
        // Wired as on the device: capabilities fetch into the cache, the host
        // installs into the kept store.
        let host = NappletHost::new(test_ctx(content.relay(), content.cache_blobs()))
            .with_manifests(content.clone())
            .with_kept_blobs(content.blobs());

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
        assert!(
            content.blobs_local().unwrap().has(&v1.blobs[0].0).await,
            "an installed napplet's file was not kept"
        );
        assert!(!content.blob_cache().contains(&v1.blobs[0].0));

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
        let moved = host.refresh(&addr, &source_for(&v2).await).await.unwrap();
        assert_eq!(moved.map(|m| m.id), Some(v2.manifest.id));
        let opened = host.open(&addr, None).await.unwrap();
        assert_eq!(opened.title.as_deref(), Some("Version two"));
        assert!(host
            .refresh(&addr, &source_for(&v2).await)
            .await
            .unwrap()
            .is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A phone for the Circle-update tests: a real content layer (so pins,
    /// the Library and the relay's slot rules are the device's) and a host
    /// serving through its pins. The data dir goes when the phone does.
    struct Phone {
        dir: std::path::PathBuf,
        content: Arc<crate::content::Content>,
        host: NappletHost,
    }

    impl Drop for Phone {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn phone(tag: &str) -> Phone {
        let dir = std::env::temp_dir().join(format!(
            "myco-napplet-push-{tag}-{}-{}",
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
        Phone { dir, content, host }
    }

    /// One version of the `fixture` napplet by `keys`, with bytes of its own.
    fn version(
        keys: &nostr::Keys,
        at: u64,
        title: &str,
        requires: &[&str],
    ) -> myco_napplet_runtime::testing::TestNapplet {
        let html = format!("<!doctype html><title>{title}</title>");
        NappletBuilder::new()
            .keys(keys.clone())
            .created_at(at)
            .title(title)
            .requires(requires)
            .files(&[("/index.html", html.as_bytes())])
            .build()
    }

    /// A peer that holds `napplet` whole.
    async fn holder_of(
        napplet: &myco_napplet_runtime::testing::TestNapplet,
    ) -> Arc<dyn PeerSource> {
        let relay = MemRelay::new();
        let blobs = MemBlobs::new();
        for (_, bytes) in &napplet.blobs {
            blobs.put(bytes).await.unwrap();
        }
        relay.publish(napplet.manifest.clone()).await.unwrap();
        Arc::new(FakeSource {
            relay,
            blobs,
            kind: napplet.manifest.kind.as_u16(),
        })
    }

    fn addr_of(napplet: &myco_napplet_runtime::testing::TestNapplet) -> NappletAddr {
        NappletAddr {
            author: napplet.author,
            d_tag: Some("fixture".to_string()),
            relays: Vec::new(),
        }
    }

    /// Install `napplet` the way an answered review does: bytes in and
    /// pinned, then a Library entry recording what the sheet showed.
    async fn install(
        phone: &Phone,
        napplet: &myco_napplet_runtime::testing::TestNapplet,
        reviewed: &[&str],
    ) {
        let source = holder_of(napplet).await;
        phone
            .host
            .ingest_event(napplet.manifest.clone(), source.as_ref())
            .await
            .unwrap();
        let reviewed: Vec<String> = reviewed.iter().map(|d| d.to_string()).collect();
        phone.content.add_napplet_to_library(
            &napplet.author.to_bech32().unwrap(),
            Some("fixture"),
            Some("Fixture"),
            "fixture.napplet.localhost",
            effective_grants(&reviewed),
            reviewed,
            "",
            0,
        );
    }

    /// Put `napplet` on `phone` the way the first-run seed does: bytes in,
    /// the defaults granted, `expected` recorded as reviewed, and the entry
    /// marked preinstalled.
    async fn seed(
        phone: &Phone,
        napplet: &myco_napplet_runtime::testing::TestNapplet,
        expected: &[&str],
    ) {
        let source = holder_of(napplet).await;
        phone
            .host
            .ingest_event(napplet.manifest.clone(), source.as_ref())
            .await
            .unwrap();
        let npub = napplet.author.to_bech32().unwrap();
        let expected: Vec<String> = expected.iter().map(|d| d.to_string()).collect();
        phone.content.add_napplet_to_library(
            &npub,
            Some("fixture"),
            Some("Fixture"),
            "fixture.napplet.localhost",
            effective_grants(&[]),
            expected.clone(),
            "",
            0,
        );
        phone
            .content
            .mark_napplet_preinstalled(&npub, Some("fixture"), &expected);
    }

    /// Bring `napplet` in as the update check does: bytes first, then pinned.
    async fn update_to(phone: &Phone, napplet: &myco_napplet_runtime::testing::TestNapplet) {
        let source = holder_of(napplet).await;
        phone
            .host
            .ingest_event(napplet.manifest.clone(), source.as_ref())
            .await
            .unwrap();
    }

    /// The defaults table as a test sees it: `author`'s `fixture` napplet is
    /// a default expected to use `expected`, and nothing else is.
    fn defaults_table(
        author: &PublicKey,
        expected: &'static [&'static str],
    ) -> impl Fn(&str, Option<&str>) -> Option<Vec<String>> {
        let npub = author.to_bech32().unwrap();
        move |who: &str, d: Option<&str>| {
            (who == npub && d == Some("fixture"))
                .then(|| expected.iter().map(|d| d.to_string()).collect())
        }
    }

    /// Open the served version of `addr` with what the Library records,
    /// read as a device open reads it.
    async fn open_as_device(
        phone: &Phone,
        addr: &NappletAddr,
        table: impl Fn(&str, Option<&str>) -> Option<Vec<String>>,
    ) -> OpenedNapplet {
        let npub = addr.author.to_bech32().unwrap();
        let grants = library_grants_with(&phone.content, &npub, addr.d_tag.as_deref(), table);
        phone.host.open_with(addr, grants).await.unwrap()
    }

    /// The first-run bug: AppStore, preinstalled, is updated to a version
    /// that declares `outbox`. Within what Myco vetted it for, so it opens
    /// with `outbox` granted and nothing to review.
    #[tokio::test]
    async fn a_preinstalled_default_updated_within_its_expected_set_is_granted() {
        let phone = phone("preinstalled-within");
        let keys = nostr::Keys::generate();
        let v1 = version(&keys, 1_000, "v1", &["theme"]);
        let v2 = version(&keys, 2_000, "v2", &["outbox", "theme"]);
        seed(&phone, &v1, &["mesh", "outbox", "theme"]).await;
        update_to(&phone, &v2).await;

        let table = defaults_table(&keys.public_key(), &["mesh", "outbox", "theme"]);
        let opened = open_as_device(&phone, &addr_of(&v2), table).await;
        assert!(opened.unreviewed.is_empty(), "{:?}", opened.unreviewed);
        assert!(opened.granted().contains(&"outbox".to_string()));
    }

    /// An entry seeded before the reviewed list recorded the expected set —
    /// reviewed empty — still gets it: the table is read at open, for any
    /// entry marked preinstalled.
    #[tokio::test]
    async fn the_expected_set_is_read_at_open_not_only_seeded() {
        let phone = phone("preinstalled-at-open");
        let keys = nostr::Keys::generate();
        let v1 = version(&keys, 1_000, "v1", &["outbox", "theme"]);
        seed(&phone, &v1, &[]).await;

        let table = defaults_table(&keys.public_key(), &["mesh", "outbox", "theme"]);
        let opened = open_as_device(&phone, &addr_of(&v1), table).await;
        assert!(opened.unreviewed.is_empty(), "{:?}", opened.unreviewed);
        assert!(opened.granted().contains(&"outbox".to_string()));
    }

    /// An update declaring beyond the expected set still goes through the
    /// review sheet for the extra.
    #[tokio::test]
    async fn a_preinstalled_default_asking_beyond_its_expected_set_is_reviewed() {
        let phone = phone("preinstalled-beyond");
        let keys = nostr::Keys::generate();
        let v1 = version(&keys, 1_000, "v1", &[]);
        let v2 = version(&keys, 2_000, "v2", &["outbox"]);
        seed(&phone, &v1, &["mesh"]).await;
        update_to(&phone, &v2).await;

        let table = defaults_table(&keys.public_key(), &["mesh"]);
        let opened = open_as_device(&phone, &addr_of(&v2), table).await;
        assert_eq!(opened.unreviewed, vec!["outbox".to_string()]);
        assert!(!opened.granted().contains(&"outbox".to_string()));
    }

    /// Someone else's napplet under the same `d` tag is not the default,
    /// even on an entry marked preinstalled: the table is keyed on the
    /// author too.
    #[tokio::test]
    async fn another_authors_napplet_at_the_same_d_tag_is_reviewed() {
        let phone = phone("preinstalled-stranger");
        let vetted = nostr::Keys::generate();
        let stranger = nostr::Keys::generate();
        let v1 = version(&stranger, 1_000, "v1", &["theme"]);
        let v2 = version(&stranger, 2_000, "v2", &["outbox", "theme"]);
        seed(&phone, &v1, &[]).await;
        update_to(&phone, &v2).await;

        let table = defaults_table(&vetted.public_key(), &["mesh", "outbox", "theme"]);
        let opened = open_as_device(&phone, &addr_of(&v2), table).await;
        assert_eq!(opened.unreviewed, vec!["outbox".to_string()]);
        assert!(!opened.granted().contains(&"outbox".to_string()));
    }

    /// The same author's napplet installed by the user, not seeded, gets no
    /// expected set: what the user reviewed is what it has.
    #[tokio::test]
    async fn a_napplet_the_user_installed_is_not_widened_by_the_defaults_table() {
        let phone = phone("preinstalled-own");
        let keys = nostr::Keys::generate();
        let v1 = version(&keys, 1_000, "v1", &["theme"]);
        let v2 = version(&keys, 2_000, "v2", &["outbox", "theme"]);
        install(&phone, &v1, &["theme"]).await;
        update_to(&phone, &v2).await;

        let table = defaults_table(&keys.public_key(), &["mesh", "outbox", "theme"]);
        let opened = open_as_device(&phone, &addr_of(&v2), table).await;
        assert_eq!(opened.unreviewed, vec!["outbox".to_string()]);
    }

    /// What the user switched off stays off, expected or not — and is not
    /// asked about again either.
    #[tokio::test]
    async fn a_switched_off_expected_domain_stays_off() {
        let phone = phone("preinstalled-denied");
        let keys = nostr::Keys::generate();
        let v1 = version(&keys, 1_000, "v1", &["outbox", "theme"]);
        seed(&phone, &v1, &["mesh", "outbox", "theme"]).await;
        let npub = keys.public_key().to_bech32().unwrap();
        let mut grants = phone
            .content
            .napplet_grants(&npub, Some("fixture"))
            .unwrap();
        grants.set("outbox", false);
        phone
            .content
            .set_napplet_grants(&npub, Some("fixture"), grants);

        let table = defaults_table(&keys.public_key(), &["mesh", "outbox", "theme"]);
        let opened = open_as_device(&phone, &addr_of(&v1), table).await;
        assert!(!opened.granted().contains(&"outbox".to_string()));
        assert!(opened.unreviewed.is_empty(), "{:?}", opened.unreviewed);
    }

    /// A push from a mesh peer with `ttl` hops left.
    fn mesh(ttl: u8) -> crate::mesh_relay::Inbound {
        crate::mesh_relay::Inbound {
            origin: crate::mesh_relay::Origin::Mesh,
            event_ttl: Some(ttl),
            sender: None,
        }
    }

    /// Push `napplet`'s manifest at `phone` as the relay hub does: stored
    /// first, then handed to the policy. Returns the outcome and every
    /// forward made, each with the version pinned at the moment it went out.
    async fn push(
        phone: &Phone,
        napplet: &myco_napplet_runtime::testing::TestNapplet,
        inbound: crate::mesh_relay::Inbound,
        sources: &[Arc<dyn PeerSource>],
    ) -> (
        crate::content::NappletPush,
        Vec<(u8, Option<nostr::EventId>)>,
    ) {
        let event = napplet.manifest.clone();
        phone.content.relay().publish(event.clone()).await.unwrap();
        let forwarded = Mutex::new(Vec::new());
        let content = phone.content.clone();
        let outcome = phone
            .content
            .handle_napplet_manifest(event, &inbound, sources, |m, ttl| {
                let pinned = content
                    .pinned_manifest(m.kind.as_u16(), &m.pubkey, Some("fixture"))
                    .map(|e| e.id);
                forwarded.lock().unwrap().push((ttl, pinned));
            })
            .await;
        (outcome, forwarded.into_inner().unwrap())
    }

    /// An installed napplet that hears of a newer version from a Circle peer
    /// fetches its bytes, verifies and pins them, and only then passes the
    /// manifest on — so the next phone can fetch the bytes from this one.
    /// The next launch opens the new version.
    #[tokio::test]
    async fn an_installed_napplet_downloads_a_pushed_update_before_passing_it_on() {
        let phone = phone("update");
        let keys = nostr::Keys::generate();
        let v1 = version(&keys, 1_000, "Version one", &["relay"]);
        let v2 = version(&keys, 2_000, "Version two", &["relay"]);
        install(&phone, &v1, &["relay"]).await;

        let (outcome, forwarded) = push(&phone, &v2, mesh(2), &[holder_of(&v2).await]).await;
        assert_eq!(outcome, crate::content::NappletPush::Updated);
        assert_eq!(
            forwarded,
            vec![(1, Some(v2.manifest.id))],
            "passed on before the new version was here, or with the wrong budget"
        );
        assert!(phone.content.blobs().has(&v2.blobs[0].0).await);
        let opened = phone.host.open(&addr_of(&v2), None).await.unwrap();
        assert_eq!(opened.title.as_deref(), Some("Version two"));
    }

    /// A phone that does not have the napplet is a pure relay for it, as for
    /// an nsite it does not run: the manifest goes on at once and nothing is
    /// downloaded. With no hops left it goes nowhere.
    #[tokio::test]
    async fn a_napplet_not_installed_here_is_passed_on_and_not_fetched() {
        let phone = phone("relay");
        let keys = nostr::Keys::generate();
        let v1 = version(&keys, 1_000, "Version one", &["relay"]);

        let (outcome, forwarded) = push(&phone, &v1, mesh(2), &[holder_of(&v1).await]).await;
        assert_eq!(outcome, crate::content::NappletPush::Relayed);
        assert_eq!(forwarded, vec![(1, None)]);
        assert!(!phone.content.blobs().has(&v1.blobs[0].0).await);

        let v2 = version(&keys, 2_000, "Version two", &["relay"]);
        let (outcome, forwarded) = push(&phone, &v2, mesh(0), &[holder_of(&v2).await]).await;
        assert_eq!(outcome, crate::content::NappletPush::Relayed);
        assert!(forwarded.is_empty(), "a spent budget was forwarded");

        // Published here: the default budget, or the one a napplet chose
        // through NAP-MESH — where 0 means this phone only.
        let local = |ttl| crate::mesh_relay::Inbound {
            origin: crate::mesh_relay::Origin::Local,
            event_ttl: ttl,
            sender: None,
        };
        let v3 = version(&keys, 3_000, "Version three", &["relay"]);
        let (_, forwarded) = push(&phone, &v3, local(None), &[]).await;
        assert_eq!(
            forwarded,
            vec![(crate::mesh_wire::EVENT_TTL - 1, None)],
            "a local publish did not originate at the default"
        );
        let v4 = version(&keys, 4_000, "Version four", &["relay"]);
        let (_, forwarded) = push(&phone, &v4, local(Some(0)), &[]).await;
        assert!(forwarded.is_empty(), "a NAP-MESH ttl of 0 left the phone");
    }

    /// An older version is a downgrade or a replay: it is not fetched, not
    /// pinned, and goes no further. A newer napplet by another author under
    /// the same `d` tag is a different napplet — relayed like any other this
    /// phone lacks, and never a way to replace the installed one.
    #[tokio::test]
    async fn an_older_version_or_another_authors_napplet_leaves_the_installed_one_alone() {
        let phone = phone("stale");
        let keys = nostr::Keys::generate();
        let v1 = version(&keys, 1_000, "Version one", &["relay"]);
        let v2 = version(&keys, 2_000, "Version two", &["relay"]);
        install(&phone, &v2, &["relay"]).await;

        let (outcome, forwarded) = push(&phone, &v1, mesh(2), &[holder_of(&v1).await]).await;
        assert_eq!(outcome, crate::content::NappletPush::Dropped);
        assert!(forwarded.is_empty(), "a downgrade was passed on");
        assert!(!phone.content.blobs().has(&v1.blobs[0].0).await);

        // The installed version itself, pushed again: nothing new to fetch
        // and nothing to spread.
        let (outcome, forwarded) = push(&phone, &v2, mesh(2), &[holder_of(&v2).await]).await;
        assert_eq!(outcome, crate::content::NappletPush::Dropped);
        assert!(forwarded.is_empty(), "a replay was passed on");

        let impostor = version(&nostr::Keys::generate(), 3_000, "Impostor", &["relay"]);
        let (outcome, _) = push(&phone, &impostor, mesh(2), &[holder_of(&impostor).await]).await;
        assert_eq!(outcome, crate::content::NappletPush::Relayed);
        assert!(!phone.content.blobs().has(&impostor.blobs[0].0).await);

        let opened = phone.host.open(&addr_of(&v2), None).await.unwrap();
        assert_eq!(opened.title.as_deref(), Some("Version two"));
    }

    /// A manifest that fails the napplet's own checks — a forged signature,
    /// an aggregate that does not cover its files — is stopped here, however
    /// new it claims to be.
    #[tokio::test]
    async fn an_invalid_napplet_manifest_goes_no_further() {
        let phone = phone("invalid");
        let keys = nostr::Keys::generate();
        let v1 = version(&keys, 1_000, "Version one", &["relay"]);
        install(&phone, &v1, &["relay"]).await;

        let corrupt = NappletBuilder::new()
            .keys(keys.clone())
            .created_at(2_000)
            .aggregate(myco_napplet_runtime::testing::FixtureAggregate::Corrupt)
            .build();
        let forged = NappletBuilder::new()
            .keys(keys.clone())
            .created_at(3_000)
            .break_signature()
            .build();
        for bad in [&corrupt, &forged] {
            let (outcome, forwarded) = push(&phone, bad, mesh(2), &[holder_of(bad).await]).await;
            assert_eq!(outcome, crate::content::NappletPush::Dropped);
            assert!(forwarded.is_empty(), "an invalid manifest was passed on");
        }
        assert_eq!(
            phone
                .content
                .pinned_manifest(KIND_NAMED, &keys.public_key(), Some("fixture"))
                .map(|e| e.id),
            Some(v1.manifest.id)
        );
    }

    /// No source with the bytes: the installed version keeps serving, and the
    /// manifest still goes on so the wave does not stall on this phone.
    #[tokio::test]
    async fn a_failed_download_keeps_the_served_version_and_still_passes_it_on() {
        let phone = phone("nobytes");
        let keys = nostr::Keys::generate();
        let v1 = version(&keys, 1_000, "Version one", &["relay"]);
        let v2 = version(&keys, 2_000, "Version two", &["relay"]);
        install(&phone, &v1, &["relay"]).await;

        // The only source has v1's bytes, not v2's.
        let (outcome, forwarded) = push(&phone, &v2, mesh(2), &[holder_of(&v1).await]).await;
        assert_eq!(outcome, crate::content::NappletPush::NotDownloaded);
        assert_eq!(forwarded, vec![(1, Some(v1.manifest.id))]);
        let opened = phone.host.open(&addr_of(&v1), None).await.unwrap();
        assert_eq!(opened.title.as_deref(), Some("Version one"));
    }

    /// A napplet's pin never moves back. An older version is refused; the
    /// same version, or another signed in the same second, is accepted —
    /// the re-pins that happen in practice (open, "Download again") are of
    /// the version already served.
    #[tokio::test]
    async fn a_napplet_pin_never_moves_back() {
        let phone = phone("monotonic");
        let keys = nostr::Keys::generate();
        let v1 = version(&keys, 1_000, "Version one", &["relay"]);
        let v2 = version(&keys, 2_000, "Version two", &["relay"]);
        let v2_twin = version(&keys, 2_000, "Version two, again", &["relay"]);
        let pinned = || {
            phone
                .content
                .pinned_manifest(KIND_NAMED, &keys.public_key(), Some("fixture"))
                .map(|e| e.id)
        };

        ManifestStore::pin(phone.content.as_ref(), &v2.manifest);
        ManifestStore::pin(phone.content.as_ref(), &v1.manifest);
        assert_eq!(pinned(), Some(v2.manifest.id), "the pin moved back");
        ManifestStore::pin(phone.content.as_ref(), &v2.manifest);
        assert_eq!(pinned(), Some(v2.manifest.id));
        ManifestStore::pin(phone.content.as_ref(), &v2_twin.manifest);
        assert_eq!(
            pinned(),
            Some(v2_twin.manifest.id),
            "an equal version was refused"
        );
    }

    /// Nothing after a pushed update takes it back: not a window open on the
    /// old version (which re-pins what it resolves), and not an older
    /// version's download finishing late.
    #[tokio::test]
    async fn neither_an_open_window_nor_a_late_download_undoes_a_pushed_update() {
        let phone = phone("nodowngrade");
        let keys = nostr::Keys::generate();
        let v1 = version(&keys, 1_000, "Version one", &["relay"]);
        let v2 = version(&keys, 2_000, "Version two", &["relay"]);
        install(&phone, &v1, &["relay"]).await;
        let window = phone.host.open(&addr_of(&v1), None).await.unwrap();
        assert_eq!(window.title.as_deref(), Some("Version one"));

        let (outcome, _) = push(&phone, &v2, mesh(2), &[holder_of(&v2).await]).await;
        assert_eq!(outcome, crate::content::NappletPush::Updated);

        // v1's bytes arriving now — a slow install, a refresh that started
        // earlier — are kept, but do not move the pin.
        phone
            .host
            .ingest_event(v1.manifest.clone(), holder_of(&v1).await.as_ref())
            .await
            .unwrap();
        for _ in 0..2 {
            let opened = phone.host.open(&addr_of(&v2), None).await.unwrap();
            assert_eq!(opened.title.as_deref(), Some("Version two"));
        }
        assert_eq!(
            phone
                .content
                .pinned_manifest(KIND_NAMED, &keys.public_key(), Some("fixture"))
                .map(|e| e.id),
            Some(v2.manifest.id)
        );
    }

    /// A window open on v1 is told once the served version has moved to v2 —
    /// by a Circle push here, the update check the same way — and a window
    /// opened after that is current. A session that is gone has nothing to say.
    #[tokio::test]
    async fn an_open_window_learns_the_served_version_moved_on() {
        let phone = phone("stale");
        let keys = nostr::Keys::generate();
        let v1 = version(&keys, 1_000, "Version one", &["relay"]);
        let v2 = version(&keys, 2_000, "Version two", &["relay"]);
        install(&phone, &v1, &["relay"]).await;
        let old = phone.host.open(&addr_of(&v1), None).await.unwrap();
        assert_eq!(
            phone.host.newer_version(&old.session_id).await,
            None,
            "a window on the served version is current"
        );

        let (outcome, _) = push(&phone, &v2, mesh(2), &[holder_of(&v2).await]).await;
        assert_eq!(outcome, crate::content::NappletPush::Updated);
        let v2_aggregate =
            myco_napplet_runtime::manifest::NappletManifest::from_event(v2.manifest.clone())
                .unwrap()
                .aggregate;
        assert_eq!(
            phone.host.newer_version(&old.session_id).await,
            Some(v2_aggregate)
        );

        let fresh = phone.host.open(&addr_of(&v2), None).await.unwrap();
        assert_eq!(phone.host.newer_version(&fresh.session_id).await, None);

        phone.host.close(&old.session_id);
        assert_eq!(phone.host.newer_version(&old.session_id).await, None);
        assert_eq!(phone.host.newer_version("napplet-999").await, None);
    }

    /// A newer manifest with no bytes behind it is not served, so an open
    /// window is not asked to restart onto it — the restart would open the
    /// same version again.
    #[tokio::test]
    async fn a_manifest_without_its_bytes_does_not_make_a_window_stale() {
        let phone = phone("stale-noblob");
        let keys = nostr::Keys::generate();
        let v1 = version(&keys, 1_000, "Version one", &["relay"]);
        let v2 = version(&keys, 2_000, "Version two", &["relay"]);
        install(&phone, &v1, &["relay"]).await;
        let window = phone.host.open(&addr_of(&v1), None).await.unwrap();

        phone
            .content
            .relay()
            .publish(v2.manifest.clone())
            .await
            .unwrap();
        assert_eq!(phone.host.newer_version(&window.session_id).await, None);
    }

    /// A source that answers every manifest query with the same event,
    /// whatever was asked — what a hostile or confused relay can do.
    struct LyingSource {
        manifest: nostr::Event,
        blobs: MemBlobs,
    }

    #[async_trait::async_trait]
    impl PeerSource for LyingSource {
        async fn fetch_manifest(
            &self,
            _author: &PublicKey,
            _d_tag: Option<&str>,
        ) -> anyhow::Result<Option<nostr::Event>> {
            Ok(Some(self.manifest.clone()))
        }

        async fn fetch_blob(
            &self,
            sha256_hex: &str,
            _servers: &[String],
        ) -> anyhow::Result<Option<Vec<u8>>> {
            self.blobs.get(sha256_hex).await
        }
    }

    /// The update check passes what it brings in on to the Circle, so it
    /// must be the napplet asked for: a newer manifest for another author or
    /// another `d` tag is refused, and the served version stays.
    #[tokio::test]
    async fn the_update_check_refuses_a_manifest_for_another_napplet() {
        let phone = phone("refresh-foreign");
        let keys = nostr::Keys::generate();
        let v1 = version(&keys, 1_000, "Version one", &["relay"]);
        install(&phone, &v1, &["relay"]).await;

        let other_author = version(&nostr::Keys::generate(), 5_000, "Impostor", &["relay"]);
        let other_d = NappletBuilder::new()
            .keys(keys.clone())
            .created_at(5_000)
            .d_tag(Some("elsewhere"))
            .build();
        for offered in [&other_author, &other_d] {
            let blobs = MemBlobs::new();
            for (_, bytes) in &offered.blobs {
                blobs.put(bytes).await.unwrap();
            }
            let source = LyingSource {
                manifest: offered.manifest.clone(),
                blobs,
            };
            assert!(phone.host.refresh(&addr_of(&v1), &source).await.is_err());
            assert_eq!(
                phone
                    .content
                    .pinned_manifest(KIND_NAMED, &keys.public_key(), Some("fixture"))
                    .map(|e| e.id),
                Some(v1.manifest.id)
            );
        }
    }

    /// An update from the Circle never widens what the napplet may do. The new
    /// version declaring a domain the user never reviewed opens without it,
    /// and hands it back as unreviewed for the review sheet.
    #[tokio::test]
    async fn a_pushed_update_that_declares_more_is_not_granted_until_reviewed() {
        let phone = phone("grants");
        let keys = nostr::Keys::generate();
        let v1 = version(&keys, 1_000, "Version one", &["relay"]);
        let v2 = version(&keys, 2_000, "Version two", &["relay", "outbox"]);
        install(&phone, &v1, &["relay"]).await;

        let (outcome, _) = push(&phone, &v2, mesh(2), &[holder_of(&v2).await]).await;
        assert_eq!(outcome, crate::content::NappletPush::Updated);

        let npub = keys.public_key().to_bech32().unwrap();
        let grants = phone.content.napplet_grants(&npub, Some("fixture"));
        let opened = phone.host.open_with(&addr_of(&v2), grants).await.unwrap();
        assert_eq!(opened.title.as_deref(), Some("Version two"));
        assert!(
            !opened.granted().contains(&"outbox".to_string()),
            "an update from the Circle granted itself a domain nobody reviewed"
        );
        assert_eq!(opened.unreviewed, vec!["outbox".to_string()]);
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
            reviewed: vec!["shell".into(), "relay".into()],
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

        // `mesh` is a default grant (for now), so switch it off here first.
        host.apply_grants(&addr.author, Some("fixture"), vec![])
            .await;
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

        // `mesh` is a default grant (for now): switched off on the sheet, it
        // is refused at once.
        host.apply_grants(&addr.author, Some("fixture"), vec![])
            .await;
        let out = host.frame(&opened.session_id, ask).await;
        let ToShell::Napplet { message } = &out[0] else {
            panic!("not a napplet frame")
        };
        assert!(
            message.field("error").is_some(),
            "mesh was served after being switched off"
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

    /// A delivered blob is served to the holder of the window's token, and
    /// nothing is served on a wrong token or for a blob never delivered.
    #[tokio::test]
    async fn a_delivered_blob_is_served_only_on_the_window_token() {
        let (host, addr) = host_with_fixture().await;
        let opened = host
            .open(&addr, Some(vec!["resource".into()]))
            .await
            .unwrap();
        let out = host
            .frame(
                &opened.session_id,
                r#"{"channel":"shell","action":"mounted"}"#,
            )
            .await;
        let ToShell::Shell { blobs, .. } = &out[0] else {
            panic!("expected a load command")
        };
        let token = blobs
            .strip_prefix("/_blob/")
            .and_then(|t| t.strip_suffix('/'))
            .unwrap()
            .to_string();
        assert_eq!(token.len(), 32);

        let shown = host.ctx.blobs.put(b"\x89PNG\r\n\x1a\nshown").await.unwrap();
        let hidden = host.ctx.blobs.put(b"never shown").await.unwrap();
        host.frame(
            &opened.session_id,
            r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#,
        )
        .await;
        let ask = format!(
            r#"{{"channel":"napplet","message":{{"type":"resource.bytes","id":"b1","url":"blossom:{shown}"}}}}"#
        );
        let reply = host.frame(&opened.session_id, &ask).await;
        let ToShell::Napplet { message } = &reply[0] else {
            panic!("not a napplet frame")
        };
        assert_eq!(
            message.field("blobRef").and_then(|v| v.as_str()),
            Some(shown.as_str()),
            "{message:?}"
        );

        assert!(host
            .blob(&opened.session_id, &token, &shown)
            .await
            .is_some());
        assert!(host
            .blob(&opened.session_id, "0".repeat(32).as_str(), &shown)
            .await
            .is_none());
        assert!(host
            .blob(&opened.session_id, &token, &hidden)
            .await
            .is_none());
        assert!(host.blob("napplet-none", &token, &shown).await.is_none());
    }

    /// One frame per line, whatever the frames carry, and each line a frame
    /// whose tag comes first — the window spots a napplet frame by it.
    #[test]
    fn frames_cross_the_ffi_one_per_line() {
        let message = myco_napplet_runtime::seams::Envelope::new("relay.event")
            .with_field("content", "two\nlines\r\nand \u{2028}");
        let frames = vec![
            ToShell::Napplet {
                message: message.clone(),
            },
            ToShell::Relaunch,
            ToShell::Napplet { message },
        ];
        let lines = frames_as_lines(&frames);
        let split: Vec<&str> = lines.split('\n').collect();
        assert_eq!(split.len(), 3);
        assert!(
            split[0].starts_with(r#"{"channel":"napplet""#),
            "{}",
            split[0]
        );
        for (line, frame) in split.iter().zip(&frames) {
            assert_eq!(&serde_json::from_str::<ToShell>(line).unwrap(), frame);
        }
        assert_eq!(frames_as_lines(&[]), "");
    }

    /// The fetcher reaches the public servers when the store misses, and not
    /// at all when offline only.
    #[tokio::test]
    async fn the_blossom_fetcher_uses_the_public_servers_unless_offline() {
        use myco_napplet_runtime::seams::BlobFetcher as _;
        let none = myco_napplet_runtime::BlobHints::default();

        let dir = std::env::temp_dir().join(format!("myco-fetcher-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(crate::content::Content::open(&dir).unwrap());
        let bytes = b"a picture, allegedly".to_vec();
        let sha = nsite_deck::sync::sha256_hex(&bytes);
        let server =
            crate::ip_source::tests::mock_blossom(vec![(sha.clone(), bytes.clone())]).await;
        let fetcher = BlossomFetcher::new(content.clone()).with_public_servers(vec![server]);

        assert_eq!(
            fetcher.fetch(&sha, 1 << 20, &none).await.unwrap(),
            Some(bytes.clone())
        );
        let missing = "00".repeat(32);
        assert_eq!(fetcher.fetch(&missing, 1 << 20, &none).await.unwrap(), None);
        assert!(
            fetcher.recently_missed(&missing),
            "a miss was not remembered"
        );
        // A cap below the blob's size is enforced by the download, not after it.
        assert_eq!(
            fetcher.fetch(&sha, bytes.len() - 1, &none).await.unwrap(),
            None,
            "an oversized blob came back anyway"
        );

        content.set_offline_only(true);
        let fresh = BlossomFetcher::new(content.clone());
        assert_eq!(
            fresh.fetch(&sha, 1 << 20, &none).await.unwrap(),
            None,
            "offline only reached the internet"
        );
        assert!(
            !fresh.recently_missed(&sha),
            "a miss the internet never confirmed was remembered"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A server the URL named is asked, even when it is none of the defaults.
    #[tokio::test]
    async fn the_blossom_fetcher_asks_the_hinted_server() {
        use myco_napplet_runtime::seams::BlobFetcher as _;

        let dir = std::env::temp_dir().join(format!("myco-fetcher-hint-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(crate::content::Content::open(&dir).unwrap());
        let bytes = b"only over there".to_vec();
        let sha = nsite_deck::sync::sha256_hex(&bytes);
        let server =
            crate::ip_source::tests::mock_blossom(vec![(sha.clone(), bytes.clone())]).await;
        let fetcher = BlossomFetcher::new(content.clone()).with_public_servers(Vec::new());
        let hints = myco_napplet_runtime::BlobHints {
            servers: vec![server],
            authors: Vec::new(),
        };
        assert_eq!(
            fetcher.fetch(&sha, 1 << 20, &hints).await.unwrap(),
            Some(bytes)
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

    /// Review runs on the manifest alone: finding a napplet fetches no bytes
    /// and keeps nothing. The bytes come only with install, verified against
    /// the manifest that was reviewed.
    #[tokio::test]
    async fn finding_a_napplet_downloads_nothing_until_install() {
        struct Counting(FakeSource, std::sync::atomic::AtomicUsize);
        #[async_trait::async_trait]
        impl PeerSource for Counting {
            async fn fetch_manifest(
                &self,
                author: &PublicKey,
                d_tag: Option<&str>,
            ) -> anyhow::Result<Option<nostr::Event>> {
                self.0.fetch_manifest(author, d_tag).await
            }
            async fn fetch_blob(
                &self,
                sha256_hex: &str,
                servers: &[String],
            ) -> anyhow::Result<Option<Vec<u8>>> {
                self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                self.0.fetch_blob(sha256_hex, servers).await
            }
        }

        let napplet = NappletBuilder::new().build();
        let source = Counting(
            FakeSource {
                relay: MemRelay::new(),
                blobs: MemBlobs::new(),
                kind: KIND_NAMED,
            },
            std::sync::atomic::AtomicUsize::new(0),
        );
        for (_, bytes) in &napplet.blobs {
            source.0.blobs.put(bytes).await.unwrap();
        }
        source
            .0
            .relay
            .publish(napplet.manifest.clone())
            .await
            .unwrap();
        let addr = NappletAddr {
            author: napplet.author,
            d_tag: Some("fixture".to_string()),
            relays: Vec::new(),
        };

        let (event, found) = fetch_manifest(&addr, &source).await.unwrap();
        let declared =
            myco_napplet_runtime::manifest::NappletManifest::from_event(napplet.manifest.clone())
                .unwrap();
        assert_eq!(found.requires, declared.requires);
        assert_eq!(found.title, declared.title);
        assert_eq!(
            source.1.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "finding a napplet downloaded it"
        );

        let local_relay = Arc::new(MemRelay::new());
        let host = NappletHost::new(test_ctx(local_relay.clone(), Arc::new(MemBlobs::new())));
        assert!(local_relay.is_empty(), "finding a napplet kept something");
        host.ingest_event(event, &source).await.unwrap();
        assert!(source.1.load(std::sync::atomic::Ordering::SeqCst) > 0);
        assert!(host.open(&addr, Some(vec!["relay".into()])).await.is_ok());
    }

    /// A forged manifest never reaches the review screen.
    #[tokio::test]
    async fn a_manifest_with_a_bad_signature_is_not_found() {
        let napplet = NappletBuilder::new().break_signature().build();
        let source = FakeSource {
            relay: MemRelay::new(),
            blobs: MemBlobs::new(),
            kind: KIND_NAMED,
        };
        source
            .relay
            .publish(napplet.manifest.clone())
            .await
            .unwrap();
        let addr = NappletAddr {
            author: napplet.author,
            d_tag: Some("fixture".to_string()),
            relays: Vec::new(),
        };
        assert!(fetch_manifest(&addr, &source).await.is_err());
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

    /// A pointer whose bytes cut a multibyte character where a scheme would
    /// end is an error, not a panic. The pointer arrives from a peer (QR,
    /// NFC, share link) and a panic here unwinds through JNI and kills the
    /// app — H1 of the PR #52 review.
    #[test]
    fn a_non_ascii_pointer_is_refused_not_a_panic() {
        for pointer in [
            "nostré",
            "naddr1€€",
            "napplet:€",
            "nostr:€x",
            "é",
            "napplet://é",
        ] {
            assert!(
                NappletAddr::parse(pointer).is_err(),
                "{pointer:?} should be refused"
            );
        }
    }

    /// An alphanumeric-mode QR code carries the pointer upper-cased. Bech32
    /// decodes either case, and Kotlin already routes `NOSTR:NADDR1…` here —
    /// L4 of the PR #52 review, where Rust then refused it.
    #[test]
    fn an_upper_case_naddr_from_an_alphanumeric_qr_parses() {
        let naddr = "naddr1qvzqqqyf8ypzpwa4mkswz4t8j70s2s6q00wzqv7k7zamxrmj2y4fs88aktcfuf68qyxhwumn8ghj7mn0wvhxcmmvqy2hwumn8ghj7un9d3shjtnyd968gmewwp6kyqqgv35kuemydahxwmmmsd2";
        let lower = NappletAddr::parse(naddr).unwrap();
        let upper = naddr.to_uppercase();
        assert_eq!(NappletAddr::parse(&upper).unwrap(), lower);
        assert_eq!(
            NappletAddr::parse(&format!("NOSTR:{upper}")).unwrap(),
            lower
        );
        assert_eq!(
            NappletAddr::parse(&format!("NAPPLET://{upper}")).unwrap(),
            lower
        );

        // The shorthand: the npub is lowered, the d tag is kept as given.
        let keys = nostr::Keys::generate();
        let npub = keys.public_key().to_bech32().unwrap().to_uppercase();
        let named = NappletAddr::parse(&format!("{npub}:MixedCase")).unwrap();
        assert_eq!(named.author, keys.public_key());
        assert_eq!(named.d_tag.as_deref(), Some("MixedCase"));
        let root = NappletAddr::parse(&npub).unwrap();
        assert_eq!(root.author, keys.public_key());
        assert_eq!(root.d_tag, None);
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
        let source = addr.public_source(Arc::new(MemRelay::new()));

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

    /// The Minesweeper napplet lives only on its author's `relay.ditto.pub`,
    /// and the AppStore's `naddr` for it carries no hints. With a default
    /// relay that does not have it, it is found only through the author's
    /// kind 10002 — which is also stored for next time.
    ///
    /// `cargo test -p myco-core --lib live_fetch::minesweeper -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn minesweeper_is_found_through_the_authors_relay_list() {
        let author =
            PublicKey::from_hex("266815e0c9210dfa324c6cba3573b14bee49da4209a9456f9484e5106cd408a5")
                .unwrap();
        let store = Arc::new(MemRelay::new());
        let source = crate::ip_source::IpPeerSource::new(
            vec!["wss://relay.damus.io".to_string()],
            Vec::new(),
        )
        .with_author_outbox(Arc::new(crate::ip_source::AuthorOutbox::new(store.clone())))
        .with_kind(KIND_NAMED)
        .with_first_answer_grace(std::time::Duration::from_millis(600));
        let started = std::time::Instant::now();
        let found = source
            .fetch_manifest(&author, Some("minesweeper"))
            .await
            .unwrap();
        println!("lookup took {:.2?}", started.elapsed());
        assert!(found.is_some(), "minesweeper not found");
        let outbox = crate::ip_source::AuthorOutbox::new(store);
        assert!(
            outbox.stored_list(&author).await.is_some(),
            "list not stored"
        );
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
