//! The trait seams the runtime reaches the world through. Nothing in this crate
//! names a concrete relay, blob store, key store, radio, or WebView — which is
//! what keeps every layer above testable off-device, with no phone in the loop.
//!
//! Two seams are not ours: [`RelayBackend`] and [`BlobStore`] come from
//! `nsite-deck`, because a napplet's manifests live in the same relay and its
//! files in the same Blossom store as an nsite's. Re-exported here so a consumer
//! wires one crate rather than two.
//!
//! The rest are napplet-only:
//!
//! - [`Signer`] — the user key, which never leaves Rust. A napplet asks for a
//!   signature; it never sees a key, and there is no seam through which it
//!   could.
//! - [`OutboxResolver`] — which relays an event should reach, across the three
//!   lanes (local, mesh, internet).
//! - [`MeshSink`] — hop-limited publish and pull over the device's mesh,
//!   behind NAP-MESH. The runtime never sees a radio or a peer address; it
//!   hands over an event and a hop budget, and asks for backlog with one.
//! - [`NapTransport`] — the shell ↔ Rust channel. A seam so that dispatch,
//!   policy and capabilities never learn whether they are talking over
//!   `addWebMessageListener`, a `WebMessagePort`, or the desktop harness's
//!   WebSocket.

use async_trait::async_trait;
use nostr::{Event, PublicKey, UnsignedEvent};

pub use nsite_deck::seams::{BlobStore, RelayBackend};

/// Signs on the user's behalf. Implemented over the napplet **user key**, which
/// is separate from the mesh device key (D3).
///
/// Mediated signing is the whole point: the napplet describes an event, the
/// runtime decides whether the grant covers it and signs. No capability hands
/// out key material.
#[async_trait]
pub trait Signer: Send + Sync {
    /// The public key events will be signed with.
    async fn public_key(&self) -> anyhow::Result<PublicKey>;

    /// Sign an event the runtime has already authorised.
    async fn sign(&self, unsigned: UnsignedEvent) -> anyhow::Result<Event>;
}

/// One relay a napplet's traffic can travel over.
///
/// The three lanes exist because "offline" is the wrong frame in a mesh: a
/// NIP-65 relay list can name `ws://<npub>.fips:4870` beside `wss://` internet
/// relays, and the outbox model works unmodified (design §7.4). A napplet
/// written for the open web works in a room with no internet.
///
/// A mesh lane is a *directed* connection to one peer's relay — the relay
/// model, not the flood. Flooding the Circle with a hop budget is NAP-MESH's
/// ([`MeshSink`]), behind its own grant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RelayLane {
    /// The device's own embedded relay.
    Local,
    /// A mesh peer's relay, addressed `ws://<npub>.fips:4870` and resolved over
    /// FIPS on the Rust side — napplets never open sockets.
    Mesh { url: String },
    /// An internet relay, when one is reachable.
    Internet { url: String },
}

impl RelayLane {
    /// Classify a relay URL the way a NIP-65 list would be read: a `.fips`
    /// host is a mesh peer, anything else is the internet. Local is never a
    /// URL — it is this device, and nothing outside it can name it.
    pub fn from_url(url: &str) -> Self {
        if is_mesh_relay_url(url) {
            Self::Mesh {
                url: url.to_string(),
            }
        } else {
            Self::Internet {
                url: url.to_string(),
            }
        }
    }

    /// The URL a napplet may be shown for this lane. `None` for the local
    /// relay, which has no address anyone else could use.
    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Local => None,
            Self::Mesh { url } | Self::Internet { url } => Some(url),
        }
    }
}

/// Whether `url` names a mesh peer's relay: a `ws://` URL whose host ends in
/// `.fips`.
///
/// Deliberately lenient — this is the *classifier*, not the gate. A malformed
/// `.fips` URL must land in the Mesh lane and be refused there by
/// [`mesh_relay_npub`]; if it fell through to Internet, the runtime would dial
/// whatever the URL's userinfo trick actually names.
pub fn is_mesh_relay_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("ws://") else {
        return false;
    };
    let host = rest.split(['/', ':', '?', '#']).next().unwrap_or("");
    host.ends_with(".fips")
}

/// The mesh relay port every peer serves on.
pub const MESH_RELAY_PORT: u16 = 4870;

/// The one way a peer's mesh relay is addressed: `ws://<npub>.fips:4870`.
///
/// Byte-identical to the builder the core uses to dial — a mesh URL from
/// outside (a napplet's `options.relays`, a kind 10002) is never dialled as
/// given; its npub is taken by [`mesh_relay_npub`] and the URL rebuilt here.
pub fn mesh_relay_url(npub: &str) -> String {
    format!("ws://{npub}.fips:{MESH_RELAY_PORT}")
}

/// The npub a mesh relay URL names, if the URL is exactly the shape
/// [`mesh_relay_url`] produces — `ws://<npub>.fips`, optionally `:4870`,
/// optionally a bare `/`.
///
/// Strict on purpose. A URL is parsed by the WebSocket client, not by us, and
/// `ws://npub1peer.fips:4870@evil.example/` parses as *userinfo* on
/// `evil.example`: whoever got that string into the pool would own the
/// peer's connection. So userinfo, any other port, any path, query or
/// fragment, and any host label that is not `npub1[a-z0-9]*` are `None` —
/// refused, never dialled.
pub fn mesh_relay_npub(url: &str) -> Option<String> {
    use nostr::types::url::Host;
    let parsed = nostr::Url::parse(url).ok()?;
    if parsed.scheme() != "ws" || !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    let Some(Host::Domain(host)) = parsed.host() else {
        return None;
    };
    let npub = host.strip_suffix(".fips")?;
    let is_npub_label = npub.starts_with("npub1")
        && npub
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    if !is_npub_label {
        return None;
    }
    if !matches!(parsed.port(), None | Some(MESH_RELAY_PORT)) {
        return None;
    }
    if !matches!(parsed.path(), "" | "/") || parsed.query().is_some() || parsed.fragment().is_some()
    {
        return None;
    }
    Some(npub.to_string())
}

/// Which way a relay plan is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Where an author's events can be read from (their NIP-65 write relays).
    Read,
    /// Where events *for* an author should be sent (their NIP-65 read relays).
    Write,
}

/// Where a relay plan came from — NAP-OUTBOX's `OutboxRelayPlan.source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanSource {
    /// Every author's NIP-65 list was found.
    Nip65,
    /// Served from a cached relay list.
    Cache,
    /// Shell policy — for Myco, a Circle member's mesh relay.
    Policy,
    /// Configured relays, because NIP-65 data was absent.
    Fallback,
}

impl PlanSource {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nip65 => "nip65",
            Self::Cache => "cache",
            Self::Policy => "policy",
            Self::Fallback => "fallback",
        }
    }
}

/// The relays a read or write should use, and how sure the shell is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayPlan {
    /// Deduplicated lanes, in the order the shell would try them.
    pub lanes: Vec<RelayLane>,
    pub source: PlanSource,
    /// Authors whose relay lists could not be resolved. Non-empty means the
    /// plan is a fallback for at least one of them.
    pub missing_authors: Vec<PublicKey>,
}

impl RelayPlan {
    /// A plan that reaches only this device.
    pub fn local_only(source: PlanSource) -> Self {
        Self {
            lanes: vec![RelayLane::Local],
            source,
            missing_authors: Vec::new(),
        }
    }
}

/// Resolves which relays to read from and write to (NIP-65) — the seam behind
/// NAP-OUTBOX's routing.
///
/// The shell owns relay discovery, fallback and policy; a napplet only ever
/// sees the resulting plan. What "policy" means here is Myco's: a Circle
/// member is reachable at their mesh relay whether or not they have published
/// a relay list, and a device with no internet has no fallback relays to offer.
#[async_trait]
pub trait OutboxResolver: Send + Sync {
    /// The plan for `authors` in `direction`. An empty `authors` asks for the
    /// shell-user's own plan — where they publish (`Write`) or, for a read,
    /// the shell's policy relays.
    async fn plan(&self, direction: Direction, authors: &[PublicKey]) -> RelayPlan;

    /// As [`OutboxResolver::plan`], knowing the relays the napplet named
    /// (`options.relays`): a resolver may also look for relay lists there,
    /// and should not conclude an author has none while those are unasked.
    /// The hints are not added to the plan — the caller adds them. The
    /// default ignores them.
    async fn plan_hinted(
        &self,
        direction: Direction,
        authors: &[PublicKey],
        hints: &[RelayLane],
    ) -> RelayPlan {
        let _ = hints;
        self.plan(direction, authors).await
    }

    /// As [`OutboxResolver::plan_hinted`], from what the resolver holds
    /// **now**: it never waits on the network. An author whose relay list
    /// is not held gets the fallback relays and is named in
    /// `missing_authors`; a resolver may look the list up behind the answer,
    /// so a later `plan_hinted` has it. What a read uses to open its lanes
    /// at once — a stream forwards what it can reach now and widens when
    /// more is known, rather than waiting to know everything. The default
    /// is `plan_hinted`, for a resolver with nothing to wait on.
    async fn plan_stored(
        &self,
        direction: Direction,
        authors: &[PublicKey],
        hints: &[RelayLane],
    ) -> RelayPlan {
        self.plan_hinted(direction, authors, hints).await
    }
}

/// Carries NIP-01 traffic over lanes — the seam behind NAP-OUTBOX's I/O.
///
/// Nothing here names a socket, a pool or a radio: a lane is a value, and the
/// implementation decides what carrying it means. That is what keeps the
/// handler testable with an in-memory relay per URL.
#[async_trait]
pub trait LaneTransport: Send + Sync {
    /// Query every lane in parallel, each bounded by `timeout`. A lane that
    /// could not be reached yields `None`, so the caller can say `incomplete`
    /// rather than pass an empty answer off as a complete one. Events are
    /// signature-verified before they are returned.
    async fn query(
        &self,
        lanes: &[RelayLane],
        filters: &[nostr::Filter],
        timeout: std::time::Duration,
    ) -> Vec<(RelayLane, Option<Vec<Event>>)>;

    /// As [`LaneTransport::query`], but may answer before every lane has,
    /// per `early` (see [`EarlyAnswer`]): shortly after the first **remote**
    /// lane returns events, at `early.local_cap` if only the local lane has
    /// by then, at once when every lane has finished, and never later than
    /// `timeout`.
    ///
    /// For one-shot reads (`relay.query`, `outbox.query`, `outbox.getEvent`),
    /// whose wire has one result frame and no way to add to it. A lane not
    /// heard from by the answer is `None`, as an unreachable one is, so the
    /// caller says `incomplete`. Where the transport can leave work running
    /// it keeps going behind the answer, within `scope`, and what it finds
    /// is kept locally for the next read.
    ///
    /// The default waits for every lane — right for a transport that cannot.
    async fn query_early(
        &self,
        lanes: &[RelayLane],
        filters: &[nostr::Filter],
        timeout: std::time::Duration,
        early: EarlyAnswer,
        scope: &WorkScope,
    ) -> Vec<(RelayLane, Option<Vec<Event>>)> {
        let _ = (early, scope);
        self.query(lanes, filters, timeout).await
    }

    /// Publish to every lane in parallel, each bounded by `timeout`, and say
    /// which accepted. The local lane is accepted *unforwarded*: stored and
    /// shown to this device's live subscriptions, handed to no flood.
    async fn publish(
        &self,
        lanes: &[RelayLane],
        event: &Event,
        timeout: std::time::Duration,
    ) -> Vec<(RelayLane, bool)>;

    /// As [`LaneTransport::publish`], but may answer early: once the local
    /// lane has answered and `quorum` of the other lanes (or all of them, if
    /// there are fewer) have accepted. Lanes still going keep going and are
    /// left out of the answer. The default waits for every lane — right for
    /// a transport that cannot leave work running.
    async fn publish_quorum(
        &self,
        lanes: &[RelayLane],
        event: &Event,
        timeout: std::time::Duration,
        quorum: usize,
    ) -> Vec<(RelayLane, bool)> {
        let _ = quorum;
        self.publish(lanes, event, timeout).await
    }

    /// Query the lanes for `filters` and accept what comes back into the
    /// local relay — unforwarded — so it reaches live subscriptions here.
    ///
    /// Returns once the work is under way, not once it is done: this is the
    /// remote half of an outbox subscription, and a napplet's other calls
    /// must not queue behind a slow relay.
    ///
    /// Each lane's events are accepted as that lane delivers them, not once
    /// the slowest has, and — where the transport can — for as long as
    /// `scope` lives: a subscription is a stream, and a relay that receives
    /// a matching event an hour from now should deliver it then.
    ///
    /// The work belongs to `scope`: it is bounded per napplet and stops when
    /// the subscription or the window closes.
    async fn pull_into_local(
        &self,
        lanes: &[RelayLane],
        filters: &[nostr::Filter],
        scope: &WorkScope,
    ) -> anyhow::Result<()>;

    /// As [`LaneTransport::pull_into_local`], with the lanes still to be
    /// planned: `resolver`'s read plan for `authors`, plus `extra`, less the
    /// local lane.
    ///
    /// A plan can wait seconds on a relay list nobody has cached, and an
    /// outbox subscription holds the napplet's session while it opens. So
    /// the plan belongs behind the answer too — the local backlog goes out
    /// first. The default plans before returning, for a transport that
    /// cannot leave work running.
    async fn pull_plan_into_local(
        &self,
        resolver: std::sync::Arc<dyn OutboxResolver>,
        authors: Vec<PublicKey>,
        extra: Vec<RelayLane>,
        filters: Vec<nostr::Filter>,
        scope: &WorkScope,
    ) -> anyhow::Result<()> {
        let plan = resolver
            .plan_hinted(Direction::Read, &authors, &extra)
            .await;
        let lanes = remote_lanes(plan.lanes.into_iter().chain(extra));
        if lanes.is_empty() {
            return Ok(());
        }
        self.pull_into_local(&lanes, &filters, scope).await
    }
}

/// When a one-shot read may answer before every lane has. See
/// [`LaneTransport::query_early`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EarlyAnswer {
    /// How long the other lanes get once the first **remote** lane has
    /// returned events.
    pub grace: std::time::Duration,
    /// How long, from the start, a round whose only events so far are this
    /// device's own waits for a remote lane to return some. The local lane
    /// answers in a millisecond and may hold a stale replaceable; a cold TLS
    /// dial on a phone does not fit in a grace counted from *that*.
    pub local_cap: std::time::Duration,
}

/// Work a napplet started that may outlive the call that started it — a
/// subscription's remote pull, the lanes a query answered without.
///
/// A handle, passed to the seams: the transport asks it for an
/// [`WorkScope::id`] to bound work per napplet by, and registers with
/// [`WorkScope::on_cancel`] what stops its work. It is cancelled when the
/// [`ScopeOwner`] behind it is dropped — the session's when the window
/// closes, a subscription's when it is closed or replaced. No runtime here:
/// what "stop" means (aborting a task) is the transport's.
#[derive(Clone)]
pub struct WorkScope {
    /// The session's scope first, then a subscription's if any. Cancelled
    /// when any of them is.
    scopes: Vec<std::sync::Arc<ScopeInner>>,
}

struct ScopeInner {
    id: u64,
    cancelled: std::sync::atomic::AtomicBool,
    on_cancel: std::sync::Mutex<Vec<Stop>>,
}

/// What stops a scope's work.
type Stop = Box<dyn FnOnce() + Send>;
/// A [`Stop`] shared by every scope it was registered with, run by the first.
type StopOnce = std::sync::Arc<std::sync::Mutex<Option<Stop>>>;

impl ScopeInner {
    fn new() -> std::sync::Arc<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        std::sync::Arc::new(Self {
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            cancelled: std::sync::atomic::AtomicBool::new(false),
            on_cancel: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let callbacks = std::mem::take(&mut *self.on_cancel.lock().unwrap());
        for f in callbacks {
            f();
        }
    }
}

impl WorkScope {
    /// A scope nothing owns: never cancelled, and not bounded by
    /// [`WorkScope::id`] (it is `None`). For work with no napplet behind it,
    /// and for tests.
    pub fn detached() -> Self {
        // Id 0: not a napplet's, so a transport does not bound it per napplet.
        Self {
            scopes: vec![std::sync::Arc::new(ScopeInner {
                id: 0,
                cancelled: std::sync::atomic::AtomicBool::new(false),
                on_cancel: std::sync::Mutex::new(Vec::new()),
            })],
        }
    }

    /// The napplet session this work belongs to, to bound work per napplet
    /// by. `None` for a [`WorkScope::detached`] scope.
    pub fn id(&self) -> Option<u64> {
        self.scopes.first().map(|s| s.id).filter(|id| *id != 0)
    }

    /// Whether the scope — or the session it is in — was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.scopes
            .iter()
            .any(|s| s.cancelled.load(std::sync::atomic::Ordering::SeqCst))
    }

    /// Run `stop` once, when the scope is cancelled — at once if it already
    /// is.
    pub fn on_cancel(&self, stop: impl FnOnce() + Send + 'static) {
        let once: StopOnce = std::sync::Arc::new(std::sync::Mutex::new(Some(Box::new(stop))));
        let fire = |once: &StopOnce| {
            if let Some(f) = once.lock().unwrap().take() {
                f();
            }
        };
        for scope in &self.scopes {
            let registered = {
                let mut callbacks = scope.on_cancel.lock().unwrap();
                if scope.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                    false
                } else {
                    let once = once.clone();
                    callbacks.push(Box::new(move || fire(&once)));
                    true
                }
            };
            if !registered {
                fire(&once);
                return;
            }
        }
    }

    /// Just the session's scope, without any subscription's — for work
    /// bookkeeping that should last as long as the session.
    pub fn session(&self) -> Self {
        Self {
            scopes: self.scopes.iter().take(1).cloned().collect(),
        }
    }

    /// Just the subscription's scope, if this is one.
    pub fn subscription(&self) -> Option<Self> {
        (self.scopes.len() > 1).then(|| Self {
            scopes: self.scopes.iter().skip(1).cloned().collect(),
        })
    }

    /// This scope narrowed to `sub`: cancelled when either is.
    pub fn with(&self, sub: &ScopeOwner) -> Self {
        let mut scopes = self.scopes.clone();
        scopes.push(sub.0.clone());
        Self { scopes }
    }
}

/// Owns a [`WorkScope`]: dropping it cancels the scope. Held by a
/// [`crate::session::Session`] for itself and for each live subscription.
pub struct ScopeOwner(std::sync::Arc<ScopeInner>);

impl ScopeOwner {
    pub fn new() -> Self {
        Self(ScopeInner::new())
    }

    /// The scope this owns.
    pub fn scope(&self) -> WorkScope {
        WorkScope {
            scopes: vec![self.0.clone()],
        }
    }
}

impl Default for ScopeOwner {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ScopeOwner {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl std::fmt::Debug for ScopeOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ScopeOwner").field(&self.0.id).finish()
    }
}

impl std::fmt::Debug for WorkScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.scopes.iter().map(|s| s.id))
            .finish()
    }
}

/// What two lanes must share to be the same relay: the local lane is
/// itself; a relay is its URL but for a trailing slash, which relay lists
/// spell both ways (the rule `ip_source::same_relay` applies in myco-core).
pub fn lane_key(lane: &RelayLane) -> String {
    match lane.url() {
        Some(url) => url.trim_end_matches('/').to_string(),
        None => String::new(),
    }
}

/// `lanes` deduplicated in order, without the local lane — what a pull
/// behind a local backlog should dial.
pub fn remote_lanes(lanes: impl IntoIterator<Item = RelayLane>) -> Vec<RelayLane> {
    // By `lane_key`: relay lists spell `wss://relay.damus.io` with and
    // without a trailing slash, and each spelling would be a socket.
    let mut seen = std::collections::HashSet::new();
    lanes
        .into_iter()
        .filter(|l| *l != RelayLane::Local && seen.insert(lane_key(l)))
        .collect()
}

/// Fetches a blob this device does not hold — the seam behind NAP-RESOURCE's
/// `blossom:` scheme.
///
/// The local [`BlobStore`] is always asked first, by the handler; this is
/// only ever reached on a miss. An implementation goes to the Circle's
/// Blossom stores over the mesh and to the public servers when reachable,
/// and returns the bytes only if they hash to `sha256_hex`. What it returns
/// is stored locally by the handler before delivery, so the next ask is
/// local — "anything queried is saved".
#[async_trait]
pub trait BlobFetcher: Send + Sync {
    /// The bytes named by `sha256_hex`, verified, or `None` when nobody
    /// reachable had them — or when what they had was over `max_bytes`, which
    /// a fetcher must enforce **while downloading**, not after: a cap checked
    /// on the finished body has already paid for the body. `Err` is for a
    /// fetch that could not even start.
    ///
    /// `hints` are where the URL said to look (BUD-10 `xs` and `as`). They
    /// only add places to ask; the hash still decides what is accepted.
    async fn fetch(
        &self,
        sha256_hex: &str,
        max_bytes: usize,
        hints: &BlobHints,
    ) -> anyhow::Result<Option<Vec<u8>>>;
}

/// Where a `blossom:` URL says its blob may be found (BUD-10): `xs` servers
/// that hold it, and `as` authors whose server list (kind 10063) may.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlobHints {
    /// Server base URLs, `https://` added where the URL gave a bare domain.
    pub servers: Vec<String>,
    /// Author pubkeys, lowercase hex.
    pub authors: Vec<String>,
}

/// A [`BlobFetcher`] with nowhere to fetch from — the honest default for a
/// runtime with no network behind it, and what tests use when fetching is not
/// the point.
pub struct NoFetcher;

#[async_trait]
impl BlobFetcher for NoFetcher {
    async fn fetch(
        &self,
        _sha256_hex: &str,
        _max_bytes: usize,
        _hints: &BlobHints,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
}

/// Where a napplet's `relay.publish` goes: the shell's **relay pool**.
///
/// Separate from [`RelayBackend`] because storing and *accepting* are different
/// acts. A store is where an event rests; accepting is what also wakes this
/// device's live subscriptions and hands the event to the other relays in the
/// pool. A napplet that only stored would have its event signed, saved, and
/// invisible — nothing would redraw here and no relay would ever hear it.
///
/// Relays, not the mesh. NAP-RELAY says "relay pool"; flooding the people
/// nearby is [`MeshSink`]'s, behind its own grant and a hop budget the user
/// caps. An implementation that fanned a relay publish out to the Circle would
/// hand every `relay`-granted napplet the mesh without the review screen ever
/// saying so.
#[async_trait]
pub trait EventSink: Send + Sync {
    /// Take a signed event and do everything accepting it implies.
    async fn accept(&self, event: Event) -> anyhow::Result<()>;

    /// Keep a signed event on this device — stored in the local relay and
    /// shown to this device's subscriptions — and send it nowhere
    /// (NAP-LOCAL's `local.publish`). The default accepts it, which is right
    /// for a sink with nothing to fan out to.
    async fn keep(&self, event: Event) -> anyhow::Result<()> {
        self.accept(event).await
    }

    /// Accept an event the napplet was delivered and publishes **as it is** —
    /// kept here and sent on to relays again, even though this device has
    /// seen it before (a plain [`EventSink::accept`] of a known event goes no
    /// further than the store). The default accepts it.
    async fn rebroadcast(&self, event: Event) -> anyhow::Result<()> {
        self.accept(event).await
    }
}

/// An [`EventSink`] that only stores — the honest default for a runtime with
/// nothing to fan out to, and what tests use when distribution is not the point.
pub struct StoreOnlySink(pub std::sync::Arc<dyn RelayBackend>);

#[async_trait]
impl EventSink for StoreOnlySink {
    async fn accept(&self, event: Event) -> anyhow::Result<()> {
        self.0.publish(event).await
    }
}

/// How far a napplet may reach over the mesh: the hop budgets the user has
/// capped publishes and subscribes at. See NAP-MESH (`docs/design/napplet/NAP-MESH.md`).
///
/// Two numbers rather than one because a flooded read costs more than a
/// flooded write — every hop answers as well as forwards — so the user is
/// given a separate, lower default for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshLimits {
    /// Most hops a `mesh.publish` may ask for.
    pub publish_ttl: u8,
    /// Most hops a `mesh.subscribe` backlog pull may ask for.
    pub subscribe_ttl: u8,
}

impl MeshLimits {
    /// The hop budget a publish actually gets: what was asked, or the cap when
    /// nothing was, and never more than the cap.
    pub fn clamp_publish(&self, requested: Option<u8>) -> u8 {
        requested.unwrap_or(self.publish_ttl).min(self.publish_ttl)
    }

    /// The hop budget a subscribe pull actually gets, by the same rule.
    pub fn clamp_subscribe(&self, requested: Option<u8>) -> u8 {
        requested
            .unwrap_or(self.subscribe_ttl)
            .min(self.subscribe_ttl)
    }
}

/// Where a napplet's mesh traffic goes — the seam behind NAP-MESH.
///
/// Separate from [`EventSink`] because the two mean different things. A relay
/// publish is "put this on my relays"; a mesh publish is "flood this to the
/// people around me, this far". The hop budget is the whole difference, and it
/// is the one thing a napplet may choose here that it may not choose anywhere
/// else — within the user's cap, which the implementation enforces, not the
/// napplet.
#[async_trait]
pub trait MeshSink: Send + Sync {
    /// The user's current caps. Read per call, so a changed setting takes
    /// effect on the next call rather than the next launch.
    async fn limits(&self) -> MeshLimits;

    /// Whether the mesh is up at all, and how many Circle peers are reachable
    /// right now. Reported, not promised: a peer can leave between the answer
    /// and the next publish.
    async fn reach(&self) -> anyhow::Result<MeshReach>;

    /// Store a signed event locally and flood it to Circle peers with `ttl`
    /// hops of budget. `0` means store only. The caller has already clamped
    /// `ttl` to [`MeshSink::limits`]; an implementation may clamp again but
    /// must never raise it.
    async fn publish(&self, event: Event, ttl: u8) -> anyhow::Result<()>;

    /// As [`MeshSink::publish`], for an event the napplet was delivered and
    /// publishes **as it is**: kept here and flooded again with `ttl` hops,
    /// even though this device has seen (and may already have flooded) it.
    /// The default publishes it.
    async fn rebroadcast(&self, event: Event, ttl: u8) -> anyhow::Result<()> {
        self.publish(event, ttl).await
    }

    /// Ask Circle peers, `ttl` hops out, for stored events matching `filters`.
    /// `0` means ask nobody.
    ///
    /// Returns once the request is *under way*, not once peers have answered:
    /// a peer two hops out may take seconds, and a napplet's other calls must
    /// not queue behind it. What comes back is accepted into the local relay
    /// by the implementation, which is what delivers it to the napplet's live
    /// subscription — the same path a freshly published event takes.
    async fn pull(&self, filters: Vec<serde_json::Value>, ttl: u8) -> anyhow::Result<()>;
}

/// What [`MeshSink::reach`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MeshReach {
    /// The mesh node is running.
    pub online: bool,
    /// Circle peers reachable right now.
    pub peers: usize,
}

/// One message across the shell ↔ Rust channel: a capability call, its result,
/// or a pushed subscription event.
///
/// The wire format is NIP-5D's, and it is **flat** — the payload's fields sit
/// beside `type` and `id`, not nested under a `payload` key:
///
/// ```text
/// -> { "type": "relay.publish", "id": "a1", "event": { … } }
/// <- { "type": "relay.publish.result", "id": "a1", "ok": true }
/// ```
///
/// `type` is `domain.action` — the NAP addressing scheme, where the domain
/// (`relay`, `shell`, `identity`) names the capability and surfaces to the
/// napplet as `window.napplet.<domain>`. A result echoes the request's `type`
/// with `.result` appended.
///
/// `id` correlates a result with its call. It is absent on unsolicited pushes,
/// and absent on both handshake messages, which occur exactly once per napplet
/// and so have nothing to correlate.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Envelope {
    /// `domain.action`, or `domain.action.result` for a result.
    #[serde(rename = "type")]
    pub msg_type: String,
    /// Correlation id, echoed on the result. `None` for pushes and handshakes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Every other top-level field. Flattened, because the payload *is* the
    /// top level on this wire — nesting it under a key would be a different
    /// protocol that no conformant napplet speaks.
    #[serde(flatten)]
    pub fields: serde_json::Map<String, serde_json::Value>,
}

impl Envelope {
    /// A message with no payload fields.
    pub fn new(msg_type: impl Into<String>) -> Self {
        Self {
            msg_type: msg_type.into(),
            id: None,
            fields: serde_json::Map::new(),
        }
    }

    /// Set the correlation id.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Add one top-level field.
    pub fn with_field(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }

    /// The capability domain — everything before the first `.`.
    pub fn domain(&self) -> &str {
        self.msg_type.split('.').next().unwrap_or("")
    }

    /// The action within the domain: the segment after the domain, with any
    /// trailing `.result` removed.
    pub fn action(&self) -> &str {
        let rest = match self.msg_type.split_once('.') {
            Some((_, rest)) => rest,
            None => return "",
        };
        rest.strip_suffix(".result").unwrap_or(rest)
    }

    /// Whether this is a result rather than a call.
    pub fn is_result(&self) -> bool {
        self.msg_type.ends_with(".result")
    }

    /// The result envelope for this call: the same `type` with `.result`
    /// appended and the same `id`. Result fields are the individual NAP's
    /// business — `ok` belongs to `relay.publish`, not to every message — so
    /// the caller adds them with [`Envelope::with_field`].
    ///
    /// Calling this on a message that is already a result would produce
    /// `x.result.result`, so the suffix is only ever appended once.
    pub fn to_result(&self) -> Self {
        let msg_type = if self.is_result() {
            self.msg_type.clone()
        } else {
            format!("{}.result", self.msg_type)
        };
        Self {
            msg_type,
            id: self.id.clone(),
            fields: serde_json::Map::new(),
        }
    }

    /// The failure result for this call. Per the registry's error model, a
    /// result carrying `error` leaves every other result field undefined — so
    /// this deliberately carries nothing else.
    pub fn to_error(&self, error: impl Into<String>) -> Self {
        let mut out = self.to_result();
        out.fields
            .insert("error".to_string(), serde_json::Value::String(error.into()));
        out
    }

    /// Read one top-level field.
    pub fn field(&self, key: &str) -> Option<&serde_json::Value> {
        self.fields.get(key)
    }
}

/// The shell ↔ Rust channel.
///
/// On device this is `addWebMessageListener`, scoped to this window's shell
/// origin — never a wildcard, and never `addJavascriptInterface`, which injects
/// into every frame including the napplet's own and would hand the sandboxed
/// napplet the bridge directly. The desktop harness implements the same trait
/// over a loopback WebSocket so the shell can be driven from a browser against
/// a host build.
#[async_trait]
pub trait NapTransport: Send + Sync {
    /// Await the next inbound envelope. `None` means the channel closed.
    async fn recv(&self) -> anyhow::Result<Option<Envelope>>;

    /// Send an envelope to the shell.
    async fn send(&self, envelope: Envelope) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A relay spelled with and without a trailing slash is one lane — one
    /// socket, one stream permit.
    #[test]
    fn a_trailing_slash_does_not_make_a_second_lane() {
        let lanes = remote_lanes([
            RelayLane::Local,
            RelayLane::Internet {
                url: "wss://relay.damus.io".into(),
            },
            RelayLane::Internet {
                url: "wss://relay.damus.io/".into(),
            },
            RelayLane::Internet {
                url: "wss://nos.lol".into(),
            },
        ]);
        assert_eq!(lanes.len(), 2);
    }
    use serde_json::json;

    /// The examples are copied from NAP-SHELL and the NIP-5D web projection. A
    /// napplet is built against those bytes, so this pins the shape rather than
    /// merely round-tripping our own struct through itself.
    #[test]
    fn the_wire_format_is_flat() {
        let call: Envelope = serde_json::from_value(
            json!({"type": "relay.publish", "id": "a1", "event": {"kind": 1}}),
        )
        .unwrap();
        assert_eq!(call.msg_type, "relay.publish");
        assert_eq!(call.id.as_deref(), Some("a1"));
        assert_eq!(call.domain(), "relay");
        assert_eq!(call.action(), "publish");
        assert!(!call.is_result());
        // The payload sits at the top level, not under a `payload` key.
        assert_eq!(call.field("event").unwrap()["kind"], 1);

        assert_eq!(
            serde_json::to_value(&call).unwrap(),
            json!({"type": "relay.publish", "id": "a1", "event": {"kind": 1}})
        );
    }

    #[test]
    fn a_result_echoes_the_type_and_id() {
        let call = Envelope::new("relay.publish").with_id("a1");
        assert_eq!(
            serde_json::to_value(call.to_result().with_field("ok", true)).unwrap(),
            json!({"type": "relay.publish.result", "id": "a1", "ok": true})
        );
    }

    /// The registry's error model: a result carrying `error` leaves every other
    /// result field undefined, so a failure never also looks like a success.
    #[test]
    fn an_error_result_carries_only_the_error() {
        let call = Envelope::new("theme.get").with_id("t1");
        assert_eq!(
            serde_json::to_value(call.to_error("no active theme")).unwrap(),
            json!({"type": "theme.get.result", "id": "t1", "error": "no active theme"})
        );
    }

    /// `.result` is appended once. Deriving a result from a result would
    /// produce `relay.publish.result.result`, which nothing answers to.
    #[test]
    fn results_do_not_stack() {
        let result = Envelope::new("relay.publish").with_id("a1").to_result();
        assert_eq!(result.to_result().msg_type, "relay.publish.result");
        assert_eq!(result.domain(), "relay");
        assert_eq!(result.action(), "publish");
    }

    /// Both handshake messages occur exactly once per napplet lifecycle, so
    /// neither carries a correlation id — and `id` must not appear in the JSON
    /// at all rather than appear as null.
    #[test]
    fn the_handshake_carries_no_id() {
        let ready: Envelope = serde_json::from_value(json!({"type": "shell.ready"})).unwrap();
        assert_eq!(ready.id, None);
        assert_eq!(ready.domain(), "shell");
        assert_eq!(ready.action(), "ready");
        assert!(ready.fields.is_empty(), "shell.ready carries no payload");
        assert_eq!(
            serde_json::to_value(&ready).unwrap(),
            json!({"type": "shell.ready"})
        );

        let init = Envelope::new("shell.init")
            .with_field("capabilities", json!({"domains": ["relay", "identity"]}))
            .with_field("services", json!([]));
        assert_eq!(
            serde_json::to_value(&init).unwrap(),
            json!({
                "type": "shell.init",
                "capabilities": {"domains": ["relay", "identity"]},
                "services": []
            })
        );
    }

    #[test]
    fn a_malformed_type_does_not_panic() {
        for msg_type in ["", "shell", "...", ".result"] {
            let envelope = Envelope::new(msg_type);
            let _ = envelope.domain();
            let _ = envelope.action();
        }
    }
}
