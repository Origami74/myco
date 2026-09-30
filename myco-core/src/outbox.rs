//! NAP-OUTBOX's two seams over Myco's world: relay plans from NIP-65 lists in
//! the local store, and lane I/O over the store, the Circle relay pool and the
//! internet.
//!
//! The three lanes are what make the outbox model work with no internet
//! (design §7.4): a kind 10002 can name `ws://<npub>.fips:4870` beside `wss://`
//! relays, and a napplet written for the open web reads a Circle member's
//! notes from their phone across the room by the same NIP-65 logic it would
//! use anywhere. A mesh lane is a directed connection to that one relay — the
//! relay model — never the Circle flood, which is NAP-MESH's.
//!
//! Policy, in one place:
//!
//! - A mesh relay is reachable only if its npub is a Circle member. Anyone
//!   else's `.fips` URL in a relay list is dropped from the plan.
//! - Our own mesh relay is never a lane: the local relay *is* it.
//! - Internet relays are dropped when "offline only" is on.
//! - An author with no relay list gets the configured relays, and the plan
//!   says so (`missing_authors`, `source: fallback`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::join_all;
use nostr::{Event, Filter, Kind, PublicKey};
use nsite_deck::seams::RelayBackend;

use myco_napplet_runtime::seams::{
    remote_lanes, Direction, EarlyAnswer, LaneTransport, OutboxResolver, PlanSource, RelayLane,
    RelayPlan, WorkScope,
};

use crate::content::Content;
use crate::mesh_relay::RelayHub;

/// How long a one-shot pull (a lane past the stream bound, a mesh lane's
/// re-pull) waits for its lanes.
const PULL_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a plan waits for missing relay lists before falling back. Paid
/// once per plan that names authors with no list here — all of them are
/// looked up in the same round. The same bound `AuthorOutbox` uses.
const LIST_FETCH_TIMEOUT: Duration = crate::ip_source::LIST_FETCH_TIMEOUT;

/// A subscription keeps at most this many relay connections open per
/// napplet, and the whole app at most [`MAX_STREAMS`]. Lanes past either
/// bound get a one-shot pull instead: they backfill, and live events reach
/// the napplet through the lanes that do stream, the Circle flood, and its
/// own next subscription.
const MAX_STREAMS_PER_NAPPLET: usize = 16;
const MAX_STREAMS: usize = 64;

/// A stream that dropped is re-opened after this, doubling per quick
/// failure up to [`STREAM_BACKOFF_MAX`], with jitter so a relay restart does
/// not see every phone come back in the same second. One that stayed up for
/// [`STREAM_HEALTHY_AFTER`] starts over at the first step.
#[cfg(not(test))]
const STREAM_BACKOFF_FIRST: Duration = Duration::from_secs(5);
#[cfg(test)]
const STREAM_BACKOFF_FIRST: Duration = Duration::from_millis(200);
const STREAM_BACKOFF_MAX: Duration = Duration::from_secs(5 * 60);
const STREAM_HEALTHY_AFTER: Duration = Duration::from_secs(60);

/// A mesh lane has no REQ of its own to hold open — the Circle pool keeps
/// one connection per peer and answers requests over it — so a
/// subscription re-asks it this often, with up to a third again as jitter.
/// Events a Circle member publishes reach this device through the flood
/// anyway; this catches what arrived at their relay from elsewhere.
const MESH_REPULL_EVERY: Duration = Duration::from_secs(45);

/// On reconnecting, ask only for what is newer than this far before the
/// last connection went down — clocks disagree, and a replayed event is
/// deduplicated by id.
const RECONNECT_OVERLAP_SECS: u64 = 60;

/// A stored relay list published less than this long ago is fresh.
const LIST_FRESH_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// A stored relay list older than [`LIST_FRESH_FOR`] is used now and
/// re-checked behind the answer — unless this device asked the pool about
/// its author less than this long ago. Staleness is "when did we last
/// look", not "when was it published": NIP-65 lists change rarely, and most
/// real ones were published months ago, so judging by `created_at` alone
/// re-checked nearly every author on every plan.
const LIST_RECHECK_AFTER: Duration = Duration::from_secs(6 * 60 * 60);

pub struct OutboxService {
    store: Arc<dyn RelayBackend>,
    hub: Arc<Mutex<Option<Arc<RelayHub>>>>,
    content: Arc<Content>,
    /// This device's mesh npub, so its own `.fips` relay is recognised and
    /// never dialled.
    own_npub: String,
    /// The internet relays used as fallback and searched for relay lists.
    /// The defaults, unless a test says otherwise.
    configured: Vec<String>,
    /// Stored relay lists, the process-wide memory of authors with none, and
    /// the indexer relays — shared with the manifest lookups
    /// (`ip_source`), so both ask the same places and remember the same
    /// misses.
    lists: Arc<crate::ip_source::AuthorOutbox>,
    /// Authors whose stale list is being refreshed right now, so a burst of
    /// calls spawns one fetch rather than one per call.
    refreshing: Arc<Mutex<std::collections::HashSet<PublicKey>>>,
    /// When the pool was last asked about each author's relay list, found or
    /// not. In memory: after a restart every stored list is checked once.
    lists_checked: Arc<Mutex<std::collections::HashMap<PublicKey, std::time::Instant>>>,
    /// Relay-list fetches under way, by author: a second plan naming an
    /// author already being looked up waits for that lookup instead of
    /// starting its own. The flag turns true when the lookup is over.
    lists_in_flight:
        Arc<Mutex<std::collections::HashMap<PublicKey, tokio::sync::watch::Receiver<bool>>>>,
    /// Background work per napplet session: its bound and its tasks. See
    /// [`SessionWork`].
    work: Arc<Mutex<std::collections::HashMap<u64, Arc<SessionWork>>>>,
    /// Internet relays with a stream open right now, app-wide, by
    /// `lane_key`, with how many. A plan prefers a relay already connected.
    open_streams: Arc<Mutex<std::collections::HashMap<String, usize>>>,
    /// Whether an Internet lane is resolved and refused when its name points
    /// at a private address (see [`dials_public`]). Always on, except in
    /// host tests that dial a mock relay on `127.0.0.1` as an Internet lane.
    guard_private_dials: bool,
}

/// What a lookup found, and how fresh it is.
enum Listed {
    /// Stored and fresh, or fetched just now.
    Fresh(Vec<RelayLane>),
    /// Stored but old; served now, refreshed behind the answer.
    Stale(Vec<RelayLane>),
    /// Not stored, not found, or a recent miss.
    Missing,
}

impl OutboxService {
    pub fn new(
        store: Arc<dyn RelayBackend>,
        hub: Arc<Mutex<Option<Arc<RelayHub>>>>,
        content: Arc<Content>,
        own_npub: String,
    ) -> Self {
        Self {
            store: store.clone(),
            hub,
            content,
            own_npub,
            configured: crate::ip_source::default_relays(),
            lists: Arc::new(crate::ip_source::AuthorOutbox::new(store.clone())),
            refreshing: Arc::new(Mutex::new(std::collections::HashSet::new())),
            lists_checked: Arc::new(Mutex::new(std::collections::HashMap::new())),
            lists_in_flight: Arc::new(Mutex::new(std::collections::HashMap::new())),
            work: Arc::new(Mutex::new(std::collections::HashMap::new())),
            open_streams: Arc::new(Mutex::new(std::collections::HashMap::new())),
            guard_private_dials: true,
        }
    }

    /// Use `relays` instead of the default public set — for tests, which
    /// must never reach the internet.
    #[cfg(test)]
    pub fn with_configured_relays(mut self, relays: Vec<String>) -> Self {
        self.configured = relays;
        // And no public indexers either.
        self.with_indexers(Vec::new())
    }

    /// Look relay lists up on `indexers` instead of the public ones — for
    /// tests.
    #[cfg(test)]
    pub fn with_indexers(mut self, indexers: Vec<String>) -> Self {
        self.lists = Arc::new(
            crate::ip_source::AuthorOutbox::new(self.store.clone())
                .with_indexers(indexers)
                .allowing_private_dials(),
        );
        self
    }

    /// Dial Internet lanes that resolve to a private address — for tests
    /// whose "internet relay" is a mock on `127.0.0.1`.
    #[cfg(test)]
    pub fn allowing_private_dials(mut self) -> Self {
        self.guard_private_dials = false;
        self
    }

    /// A clone that owns its handles — for work spawned past a `&self`. The
    /// caches are shared, not copied.
    fn detached(&self) -> Arc<Self> {
        Arc::new(Self {
            store: self.store.clone(),
            hub: self.hub.clone(),
            content: self.content.clone(),
            own_npub: self.own_npub.clone(),
            configured: self.configured.clone(),
            lists: self.lists.clone(),
            refreshing: self.refreshing.clone(),
            lists_checked: self.lists_checked.clone(),
            lists_in_flight: self.lists_in_flight.clone(),
            work: self.work.clone(),
            open_streams: self.open_streams.clone(),
            guard_private_dials: self.guard_private_dials,
        })
    }

    /// Whether an Internet lane may be dialled: the guard is off, or its
    /// name resolves to public addresses only.
    async fn may_dial(&self, url: &str) -> bool {
        if !self.guard_private_dials {
            return true;
        }
        if dials_public(url).await {
            return true;
        }
        tracing::debug!(
            url,
            "outbox: internet lane resolves to a private address; not dialled"
        );
        false
    }

    /// Look up `authors`' relay lists in the pool and store what is found;
    /// an author still without one afterwards is remembered as a miss.
    ///
    /// One round, one `REQ` per relay, all the authors in it (chunked, so a
    /// huge set does not make one huge filter). An author another plan is
    /// already looking up is not asked for twice: this call waits for that
    /// lookup instead. Bounded by [`LIST_FETCH_TIMEOUT`] either way.
    async fn fetch_relay_lists(&self, authors: &[PublicKey], hints: &[RelayLane]) {
        let (tx, _) = tokio::sync::watch::channel(false);
        let mut mine: Vec<PublicKey> = Vec::new();
        let mut theirs: Vec<tokio::sync::watch::Receiver<bool>> = Vec::new();
        {
            let mut in_flight = self.lists_in_flight.lock().unwrap();
            for author in authors {
                match in_flight.get(author) {
                    Some(rx) => theirs.push(rx.clone()),
                    None if !mine.contains(author) => {
                        in_flight.insert(*author, tx.subscribe());
                        mine.push(*author);
                    }
                    None => {}
                }
            }
        }
        // Clears this call's entries and wakes the waiters however it ends —
        // answered, timed out, or dropped with the task that ran it.
        let _done = ListsInFlight {
            map: self.lists_in_flight.clone(),
            authors: mine.clone(),
            tx,
        };
        let fetch = async {
            if !mine.is_empty() {
                self.fetch_relay_lists_now(&mine, hints).await;
            }
        };
        let wait = async {
            for mut rx in theirs {
                let _ = rx.wait_for(|done| *done).await;
            }
        };
        let _ = tokio::time::timeout(
            LIST_FETCH_TIMEOUT + Duration::from_secs(1),
            futures_util::future::join(fetch, wait),
        )
        .await;
    }

    /// The round behind [`Self::fetch_relay_lists`], for authors nobody else
    /// is looking up. Asked, all at once:
    ///
    /// - the configured relays and the indexers, through [`AuthorOutbox`] —
    ///   unless offline only or the internet looks down;
    /// - the relays the napplet named (`hints`), through the lanes, so a
    ///   name that resolves to a private address is refused as always;
    /// - every Circle member's mesh relay: the people around the user are
    ///   exactly who would have seen a friend's list.
    ///
    /// An author still without a list is remembered as a miss only when the
    /// napplet named no relays (they may be the only place it is) and every
    /// relay asked answered to the end — a timeout is not a "no".
    ///
    /// [`AuthorOutbox`]: crate::ip_source::AuthorOutbox
    async fn fetch_relay_lists_now(&self, authors: &[PublicKey], hints: &[RelayLane]) {
        let public: Vec<String> =
            if self.content.is_offline_only() || self.content.internet_looks_down() {
                Vec::new()
            } else {
                let mut out: Vec<String> = Vec::new();
                for url in self.configured.iter().chain(self.lists.indexers()) {
                    if !out.iter().any(|r| crate::ip_source::same_relay(r, url)) {
                        out.push(url.clone());
                    }
                }
                out
            };
        let mut lanes: Vec<RelayLane> = hints.to_vec();
        for npub in self.content.circle_npubs() {
            if npub != self.own_npub {
                lanes.push(RelayLane::Mesh {
                    url: crate::ip_source::mesh_relay_url(&npub),
                });
            }
        }
        let lanes = remote_lanes(lanes);
        let filters: Vec<Filter> = authors
            .chunks(crate::ip_source::LIST_AUTHORS_PER_FILTER)
            .map(|chunk| {
                Filter::new()
                    .kind(Kind::RelayList)
                    .authors(chunk.iter().copied())
            })
            .collect();
        let lanes_round = async {
            if lanes.is_empty() {
                Vec::new()
            } else {
                self.query(&lanes, &filters, LIST_FETCH_TIMEOUT).await
            }
        };
        let (public_complete, answers) =
            futures_util::future::join(self.lists.fetch_lists_from(authors, &public), lanes_round)
                .await;
        let lanes_complete = answers.iter().all(|(_, events)| events.is_some());
        for event in answers.into_iter().filter_map(|(_, e)| e).flatten() {
            if event.kind == Kind::RelayList && authors.contains(&event.pubkey) {
                // The local relay is the cache: a replaceable kind keeps the
                // newest.
                self.lists.remember(event).await;
            }
        }
        // Stamped only now, after what was found is stored: stamped sooner, a
        // plan racing the round would take the old stored list as checked
        // and fresh.
        {
            let now = std::time::Instant::now();
            let mut checked = self.lists_checked.lock().unwrap();
            for author in authors {
                checked.insert(*author, now);
            }
        }
        let asked_anyone = !public.is_empty() || !lanes.is_empty();
        if !asked_anyone {
            return;
        }
        if hints.is_empty() && public_complete && lanes_complete {
            self.lists.note_misses_among(authors).await;
        } else {
            // Not a "no" — someone did not answer, or the napplet named
            // relays — but not worth asking again on the next call either.
            self.lists.note_soft_misses_among(authors).await;
        }
    }

    /// The newest relay list the local store holds for each of `authors`.
    async fn stored_lists(
        &self,
        authors: &[PublicKey],
    ) -> std::collections::HashMap<PublicKey, Event> {
        let filter = Filter::new()
            .kind(Kind::RelayList)
            .authors(authors.iter().copied());
        let mut out: std::collections::HashMap<PublicKey, Event> = std::collections::HashMap::new();
        for event in self.store.query(&[filter]).await.unwrap_or_default() {
            let newer = out
                .get(&event.pubkey)
                .is_none_or(|held| event.created_at > held.created_at);
            if newer {
                out.insert(event.pubkey, event);
            }
        }
        out
    }

    /// The configured relays as lanes, or nothing when offline only.
    fn fallback_lanes(&self) -> Vec<RelayLane> {
        if self.content.is_offline_only() {
            return Vec::new();
        }
        self.configured
            .iter()
            .map(|url| RelayLane::Internet { url: url.clone() })
            .collect()
    }

    /// Whether a round over this lane would really try the internet: an
    /// allowed internet lane whose relay is not on the skip list. A round of
    /// nothing but skipped relays has not tried the internet, and must not
    /// trip the breaker that would then hold back the good ones.
    fn tries_internet(&self, lane: &RelayLane) -> bool {
        match lane {
            RelayLane::Internet { url } => {
                self.allowed(lane) && !crate::relay_health::is_skipped(url)
            }
            _ => false,
        }
    }

    /// Whether a lane may be used from this device, per the policy above.
    fn allowed(&self, lane: &RelayLane) -> bool {
        match lane {
            RelayLane::Local => true,
            RelayLane::Internet { .. } => !self.content.internet_looks_down(),
            RelayLane::Mesh { url } => match mesh_relay_npub(url) {
                Some(npub) => npub != self.own_npub && self.content.circle_npubs().contains(&npub),
                None => false,
            },
        }
    }

    /// Each author's NIP-65 relays for `direction`, in the order given: from
    /// the local store when it has a list, from the pool when it does not —
    /// stored on the way in, so the second call is local. Every author with
    /// no list here is looked up in the same round.
    ///
    /// `hints` are relays the napplet named: asked for lists too, and an
    /// author remembered as having none is asked again when there are some.
    async fn listed_for(
        &self,
        authors: &[PublicKey],
        direction: Direction,
        hints: &[RelayLane],
    ) -> Vec<Listed> {
        let mut stored = self.stored_lists(authors).await;
        let now_secs = nostr::Timestamp::now().as_secs();
        let unknown: Vec<PublicKey> = authors
            .iter()
            .filter(|a| {
                !stored.contains_key(a) && (!hints.is_empty() || !self.lists.recently_missed(a))
            })
            .copied()
            .collect();
        if !unknown.is_empty() {
            self.fetch_relay_lists(&unknown, hints).await;
            stored.extend(self.stored_lists(&unknown).await);
        }

        let mut stale: Vec<PublicKey> = Vec::new();
        let listed = authors
            .iter()
            .map(|author| {
                let Some(list) = stored.get(author) else {
                    return Listed::Missing;
                };
                let lanes = relay_list_lanes(list, direction);
                let age = Duration::from_secs(now_secs.saturating_sub(list.created_at.as_secs()));
                let checked_lately = self
                    .lists_checked
                    .lock()
                    .unwrap()
                    .get(author)
                    .is_some_and(|at| at.elapsed() < LIST_RECHECK_AFTER);
                if age <= LIST_FRESH_FOR || checked_lately || unknown.contains(author) {
                    return Listed::Fresh(lanes);
                }
                // Old enough to check, not too old to use. The napplet gets
                // the stored plan now; the next call gets whatever the
                // refresh found.
                if self.refreshing.lock().unwrap().insert(*author) {
                    stale.push(*author);
                }
                Listed::Stale(lanes)
            })
            .collect();
        if !stale.is_empty() {
            // One round for every stale author of this plan, not one each.
            let this = self.detached();
            tokio::spawn(async move {
                this.fetch_relay_lists(&stale, &[]).await;
                let mut refreshing = this.refreshing.lock().unwrap();
                for author in &stale {
                    refreshing.remove(author);
                }
            });
        }
        listed
    }

    /// Query one lane, verifying what comes back. `None` when the lane could
    /// not be reached or is not allowed.
    async fn query_lane(
        &self,
        lane: &RelayLane,
        filters: &[Filter],
        timeout: Duration,
    ) -> Option<Vec<Event>> {
        if !self.allowed(lane) {
            return None;
        }
        match lane {
            RelayLane::Local => self.store.query(filters).await.ok(),
            RelayLane::Mesh { url } => {
                // The pool dials the URL rebuilt from the npub, never the
                // lane's string: a mesh URL is an address *by name*, and the
                // name is all that is taken from it.
                let npub = mesh_relay_npub(url)?;
                let raw: Vec<serde_json::Value> = filters
                    .iter()
                    .filter_map(|f| serde_json::to_value(f).ok())
                    .collect();
                let events = self
                    .content
                    .peer_relays()
                    .request(
                        &npub,
                        &crate::ip_source::mesh_relay_url(&npub),
                        raw,
                        timeout,
                    )
                    .await;
                // The pool returns events as received; the caller verifies.
                Some(events.into_iter().filter(|e| e.verify().is_ok()).collect())
            }
            RelayLane::Internet { url } => {
                // On the skip list: finished, nothing, at once — not even
                // resolved. See `relay_health`.
                if crate::relay_health::is_skipped(url) {
                    return None;
                }
                // One connection, one REQ carrying every filter; verified at
                // ingress by `query_relay_filters`. The resolve-and-refuse
                // guard runs inside the same timeout, so a slow resolver
                // cannot hold the round past what the caller allowed.
                let values: Vec<serde_json::Value> = filters
                    .iter()
                    .filter_map(|f| serde_json::to_value(f).ok())
                    .collect();
                let dial = async {
                    if !self.may_dial(url).await {
                        return None;
                    }
                    Some(crate::ip_source::query_relay_filters(url, values).await)
                };
                match crate::relay_health::timeout(url, timeout, dial).await {
                    Ok(None) => None,
                    Ok(Some(Ok(events))) => Some(events),
                    Ok(Some(Err(e))) => {
                        tracing::debug!(url, error = %e, "outbox: relay query failed");
                        None
                    }
                    Err(_) => {
                        tracing::debug!(url, "outbox: relay query timed out");
                        None
                    }
                }
            }
        }
    }

    async fn publish_lane(&self, lane: &RelayLane, event: &Event, timeout: Duration) -> bool {
        if !self.allowed(lane) {
            return false;
        }
        match lane {
            RelayLane::Local => self.accept_local(event.clone()).await.is_ok(),
            RelayLane::Mesh { url } => {
                let Some(npub) = mesh_relay_npub(url) else {
                    return false;
                };
                let pool = self.content.peer_relays();
                // The push plane is fire-and-forget over the pooled connection,
                // so "accepted" can only honestly mean "there is a connection to
                // put it on". A peer in dial backoff gets a false, not a queue.
                if !pool.connected_npubs().contains(&npub) {
                    return false;
                }
                let Ok(frame) = serde_json::to_string(&serde_json::json!(["EVENT", event])) else {
                    return false;
                };
                pool.send(&npub, &crate::ip_source::mesh_relay_url(&npub), frame);
                true
            }
            RelayLane::Internet { url } => {
                if crate::relay_health::is_skipped(url) {
                    return false;
                }
                let dial = async {
                    if !self.may_dial(url).await {
                        return false;
                    }
                    matches!(
                        crate::ip_source::publish_to_relay(url, event).await,
                        Ok(true)
                    )
                };
                matches!(
                    crate::relay_health::timeout(url, timeout, dial).await,
                    Ok(true)
                )
            }
        }
    }
}

/// Whether `url` names only public addresses right now — the dial-site half
/// of the private-host refusal.
///
/// `validate_relay_url` judges the host as written; a public *name* can
/// still resolve to `127.0.0.1` (the ungated loopback relay) or a LAN
/// address, by a hostile zone or a rebinding trick. So the name is resolved
/// here, with the port the URL names or the scheme's default, and the lane
/// is refused if the lookup fails or **any** answer is
/// [`myco_napplet_runtime::nap::outbox::is_private_ip`]. Userinfo is
/// refused again for the same reason it is refused there.
///
/// Lives in this crate because the resolve is async and the runtime crate
/// has no tokio. Known gap: `connect_async` resolves the name again, so an
/// answer that changes between the two is not caught (TOCTOU). Accepted for
/// now; the follow-up is to connect to the checked address ourselves and
/// hand the stream to `client_async_tls_with_config`.
pub(crate) async fn dials_public(url: &str) -> bool {
    let Ok(parsed) = nostr::Url::parse(url) else {
        return false;
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let port = match parsed.port_or_known_default() {
        Some(port) => port,
        None => match parsed.scheme() {
            "wss" => 443,
            _ => 80,
        },
    };
    // `lookup_host` wants a bare host; the parser keeps the brackets on a v6
    // literal.
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let Ok(addrs) = tokio::net::lookup_host((host, port)).await else {
        // The name does not resolve: no dial would get anywhere either.
        crate::relay_health::record_dns_failure(url);
        return false;
    };
    let mut any = false;
    for addr in addrs {
        any = true;
        if myco_napplet_runtime::nap::outbox::is_private_ip(addr.ip()) {
            return false;
        }
    }
    any
}

impl OutboxService {
    /// Accept an event into this device's relay **unforwarded**: stored, shown
    /// to live subscriptions here (the WebView's, other napplets'), handed to
    /// no gossiper. Returns whether it was the first sighting.
    ///
    /// Falls back to storing when no hub is up — the host-build and pre-start
    /// case, not a silent downgrade in the field: on a phone the hub is stood
    /// up with the content layer, before any napplet can open.
    async fn accept_local(&self, event: Event) -> anyhow::Result<bool> {
        // Cloned out rather than held: the lock must not span the await.
        let hub = self.hub.lock().unwrap().clone();
        match hub {
            Some(hub) => hub.accept_unforwarded(event).await,
            None => {
                self.store.publish(event).await?;
                Ok(true)
            }
        }
    }
}

/// Whether every filter asks only for replaceable or addressable kinds — a
/// read whose late lanes can only bring newer versions of what was answered.
fn only_replaceable(filters: &[Filter]) -> bool {
    !filters.is_empty()
        && filters.iter().all(|f| {
            f.kinds.as_ref().is_some_and(|kinds| {
                !kinds.is_empty()
                    && kinds
                        .iter()
                        .all(|k| k.is_replaceable() || k.is_addressable())
            })
        })
}

/// How long one internet relay gets to say `OK` before a pool publish moves on.
const POOL_PUBLISH_TIMEOUT: Duration = Duration::from_secs(10);

/// NAP-RELAY's `relay.publish` lands here: the shell's **relay pool** — this
/// device's own relay, and the configured internet relays when reachable.
///
/// Not the mesh. NAP-RELAY says relays, and the Circle flood is NAP-MESH's,
/// with a hop budget the user caps — a `relay` grant must not be a back door
/// to it. So the event is accepted unforwarded here and fanned out to the
/// internet lanes through the same [`LaneTransport`] NAP-OUTBOX uses, which
/// is what applies offline-only and feeds the internet breaker.
///
/// The internet half is spawned and best-effort: a public relay is seconds
/// away on a good day and unreachable on the day this app is for, and the
/// napplet's result must not wait on either. NAP-RELAY asks for the signed
/// event back, not a per-relay tally — that is NAP-OUTBOX's `publish`.
#[async_trait::async_trait]
impl myco_napplet_runtime::seams::EventSink for OutboxService {
    async fn accept(&self, event: Event) -> anyhow::Result<()> {
        // A repeat was already sent on the first sighting; a relay that has
        // it answers a duplicate with the same OK and nothing is gained.
        if !self.accept_local(event.clone()).await? {
            return Ok(());
        }
        self.push_to_fallback_lanes(event);
        Ok(())
    }

    /// NAP-LOCAL's `local.publish`: kept here and shown to this device's
    /// subscriptions, sent nowhere.
    async fn keep(&self, event: Event) -> anyhow::Result<()> {
        self.accept_local(event).await.map(|_| ())
    }

    /// A delivered event published as it is: kept, and pushed to the
    /// internet lanes whether or not this device has seen it — having seen it
    /// is how the napplet came to have it.
    async fn rebroadcast(&self, event: Event) -> anyhow::Result<()> {
        let hub = self.hub.lock().unwrap().clone();
        if let Some(hub) = hub {
            hub.claim_pass_on(&event, "relays")?;
        }
        self.accept_local(event.clone()).await?;
        self.push_to_fallback_lanes(event);
        Ok(())
    }
}

impl OutboxService {
    /// Push `event` to the internet fallback lanes, spawned and best-effort.
    fn push_to_fallback_lanes(&self, event: Event) {
        let lanes = self.fallback_lanes();
        if lanes.is_empty() {
            return;
        }
        let this = self.detached();
        tokio::spawn(async move {
            let results = this.publish(&lanes, &event, POOL_PUBLISH_TIMEOUT).await;
            let accepted = results.iter().filter(|(_, ok)| *ok).count();
            tracing::info!(
                event = %event.id,
                accepted,
                total = results.len(),
                "napplet publish reached the internet pool"
            );
        });
    }
}

#[async_trait::async_trait]
impl OutboxResolver for OutboxService {
    async fn plan(&self, direction: Direction, authors: &[PublicKey]) -> RelayPlan {
        self.plan_hinted(direction, authors, &[]).await
    }

    async fn plan_hinted(
        &self,
        direction: Direction,
        authors: &[PublicKey],
        hints: &[RelayLane],
    ) -> RelayPlan {
        if authors.is_empty() {
            let mut lanes = vec![RelayLane::Local];
            lanes.extend(self.fallback_lanes());
            return RelayPlan {
                lanes,
                source: PlanSource::Policy,
                missing_authors: Vec::new(),
            };
        }

        let mut lanes: Vec<RelayLane> = vec![RelayLane::Local];
        let mut missing = Vec::new();
        let mut any_stale = false;
        // Each listed author's usable internet relays, for selection below.
        let mut author_relays: Vec<Vec<String>> = Vec::new();
        // Every author at once: the lists not stored here are looked up in
        // one round, not one author after another.
        let listed = self.listed_for(authors, direction, hints).await;
        for (author, listed) in authors.iter().zip(listed) {
            let listed = match listed {
                Listed::Fresh(listed) => listed,
                Listed::Stale(listed) => {
                    any_stale = true;
                    listed
                }
                Listed::Missing => {
                    missing.push(*author);
                    lanes.extend(self.fallback_lanes());
                    continue;
                }
            };
            let mut internet = Vec::new();
            for lane in listed.into_iter().filter(|l| self.allowed(l)) {
                match lane {
                    // Mesh lanes are the Circle's, one pooled connection per
                    // peer: always kept.
                    RelayLane::Internet { url } if direction == Direction::Read => {
                        internet.push(url)
                    }
                    other => lanes.push(other),
                }
            }
            author_relays.push(internet);
        }
        if direction == Direction::Read {
            // Reading: enough relays to find every author twice, not every
            // relay every author names. (Writing to inboxes is a delivery
            // contract — every relay — so it is not narrowed.)
            let candidates = {
                let mut keys = std::collections::HashSet::new();
                author_relays
                    .iter()
                    .flatten()
                    .filter(|u| keys.insert(u.trim_end_matches('/').to_string()))
                    .count()
            };
            let open = self.open_streams.lock().unwrap().clone();
            let chosen = select_relays(
                &author_relays,
                crate::relay_health::is_skipped,
                |url| open.contains_key(url.trim_end_matches('/')),
                RELAYS_PER_AUTHOR,
                MAX_SELECTED_RELAYS,
            );
            tracing::debug!(
                "plan: {} authors -> {} relays (from {} candidates)",
                author_relays.len(),
                chosen.len(),
                candidates
            );
            lanes.extend(chosen.into_iter().map(|url| RelayLane::Internet { url }));
        }
        let mut seen = std::collections::HashSet::new();
        lanes.retain(|l| seen.insert(myco_napplet_runtime::seams::lane_key(l)));
        RelayPlan {
            lanes,
            // Fallback outranks cache: a plan with one author's list missing
            // is a fallback plan whatever the other lists' age.
            source: if !missing.is_empty() {
                PlanSource::Fallback
            } else if any_stale {
                PlanSource::Cache
            } else {
                PlanSource::Nip65
            },
            missing_authors: missing,
        }
    }
}

/// How many of an author's relays a read plan aims to include: two, so one
/// relay down or behind does not lose the author (NDK and welshman aim for
/// the same).
const RELAYS_PER_AUTHOR: usize = 2;

/// The most relays a read plan selects to cover its authors. Popular relays
/// cover most authors between them, so eight covers a large follow list
/// twice over in practice; and eight, beside the three or four fallback
/// relays and a few napplet-named ones, stays inside one napplet's
/// [`MAX_STREAMS_PER_NAPPLET`], where a plan that unioned every author's
/// relays reached 20 and 39 lanes for one subscription.
const MAX_SELECTED_RELAYS: usize = 8;

/// Choose which of the authors' relays a read goes to: a greedy set cover,
/// as NDK and welshman do.
///
/// `author_relays` holds each author's write relays, in list order. Picked
/// first is the relay covering the most authors that still need one — in
/// two passes, so every author is covered once before any is covered twice
/// — up to `per_author` each (or all of theirs, if fewer), and at most `cap`
/// relays in all. A relay `skipped` (the skip list) is chosen only for an
/// author nothing else covers. Ties go to a relay with a stream `open`
/// already, then to the relay more authors list, then to the one seen
/// first — so the same inputs always give the same plan.
fn select_relays(
    author_relays: &[Vec<String>],
    skipped: impl Fn(&str) -> bool,
    open: impl Fn(&str) -> bool,
    per_author: usize,
    cap: usize,
) -> Vec<String> {
    // Candidates by normalised URL, first spelling and first-seen order kept.
    let mut urls: Vec<String> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut covers: Vec<Vec<usize>> = Vec::new();
    for (author, relays) in author_relays.iter().enumerate() {
        for url in relays {
            let k = url.trim_end_matches('/').to_string();
            let i = *index.entry(k).or_insert_with(|| {
                urls.push(url.clone());
                covers.push(Vec::new());
                urls.len() - 1
            });
            if !covers[i].contains(&author) {
                covers[i].push(author);
            }
        }
    }
    let skip: Vec<bool> = urls.iter().map(|u| skipped(u)).collect();
    let is_open: Vec<bool> = urls.iter().map(|u| open(u)).collect();
    let usable_count = |author: usize| {
        (0..urls.len())
            .filter(|&i| !skip[i] && covers[i].contains(&author))
            .count()
    };
    let wanted: Vec<usize> = (0..author_relays.len())
        .map(|a| per_author.min(usable_count(a)))
        .collect();

    let mut chosen: Vec<usize> = Vec::new();
    let mut have = vec![0usize; author_relays.len()];
    // Pass 1: everyone once; pass 2: everyone `per_author` times; pass 3: an
    // author with only skipped relays gets one of those.
    for (target, allow_skipped) in [(1, false), (per_author, false), (1, true)] {
        loop {
            if chosen.len() >= cap {
                break;
            }
            let need = |a: usize| {
                let goal = if allow_skipped {
                    usize::from(wanted[a] == 0)
                } else {
                    target.min(wanted[a])
                };
                have[a] < goal
            };
            let best = (0..urls.len())
                .filter(|i| !chosen.contains(i) && skip[*i] == allow_skipped)
                .map(|i| {
                    let gain = covers[i].iter().filter(|&&a| need(a)).count();
                    (i, gain)
                })
                .filter(|(_, gain)| *gain > 0)
                .max_by(|(a, ga), (b, gb)| {
                    ga.cmp(gb)
                        .then(is_open[*a].cmp(&is_open[*b]))
                        .then(covers[*a].len().cmp(&covers[*b].len()))
                        // Earlier seen wins: reverse the index order.
                        .then(b.cmp(a))
                });
            let Some((i, _)) = best else { break };
            chosen.push(i);
            for &a in &covers[i] {
                have[a] += 1;
            }
        }
    }
    chosen.into_iter().map(|i| urls[i].clone()).collect()
}

/// Counts one open stream to a relay in [`OutboxService::open_streams`] for
/// as long as it lives.
struct OpenStream {
    map: Arc<Mutex<std::collections::HashMap<String, usize>>>,
    key: String,
}

impl OpenStream {
    fn new(map: Arc<Mutex<std::collections::HashMap<String, usize>>>, url: &str) -> Self {
        let key = url.trim_end_matches('/').to_string();
        *map.lock().unwrap().entry(key.clone()).or_default() += 1;
        Self { map, key }
    }
}

impl Drop for OpenStream {
    fn drop(&mut self) {
        let mut map = self.map.lock().unwrap();
        if let Some(n) = map.get_mut(&self.key) {
            *n -= 1;
            if *n == 0 {
                map.remove(&self.key);
            }
        }
    }
}

/// One lane's answer, as a query round hands it on.
type LaneAnswer = (RelayLane, Option<Vec<Event>>);

/// Where a round's answers go.
#[derive(Clone, Copy)]
enum Deliver<'a> {
    /// To the caller, at the end of the round: [`LaneTransport::query`].
    Caller,
    /// To a caller listening lane by lane, while it listens. A lane that
    /// finishes after it stopped — the answer has gone — is kept in the
    /// local relay instead: [`LaneTransport::query_early`].
    Listener(&'a tokio::sync::mpsc::UnboundedSender<LaneAnswer>),
    /// Into the local relay, as each lane finishes: a subscription's pull.
    Store,
}

/// The lanes a query answered without keep going behind the answer, this
/// many rounds per napplet at a time; past that they are dropped (see
/// [`LaneTransport::query_early`] on [`OutboxService`]).
const MAX_BACKGROUND_PER_NAPPLET: usize = 4;

/// One napplet session's background work: its query leftovers (at most
/// [`MAX_BACKGROUND_PER_NAPPLET`]), its open relay streams (at most
/// [`MAX_STREAMS_PER_NAPPLET`]), and every task, aborted when the session
/// closes.
struct SessionWork {
    permits: Arc<tokio::sync::Semaphore>,
    streams: Arc<tokio::sync::Semaphore>,
    tasks: Mutex<Vec<tokio::task::AbortHandle>>,
}

/// The app-wide bound on open relay streams.
fn all_streams() -> Arc<tokio::sync::Semaphore> {
    static STREAMS: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> = std::sync::OnceLock::new();
    STREAMS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(MAX_STREAMS)))
        .clone()
}

/// Events a relay stream may have in hand before it waits for the store.
const STREAM_QUEUE: usize = 256;

/// How often an open stream checks whether "offline only" was switched on.
const OFFLINE_CHECK_EVERY: Duration = Duration::from_secs(3);

/// Whether `event` is one `filters` asked for.
fn matches_any(filters: &[Filter], event: &Event) -> bool {
    filters
        .iter()
        .any(|f| f.match_event(event, nostr::filter::MatchEventOptions::new()))
}

/// Up to `max` of jitter, from the OS's randomness.
fn jitter(max: Duration) -> Duration {
    let bytes = crate::ip_source::random_bytes(4);
    let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    max.mul_f64(f64::from(n) / f64::from(u32::MAX))
}

/// `filters`, each asking for nothing older than `since` (a filter's own
/// later `since` is kept).
fn filters_since(filters: &[Filter], since: Option<nostr::Timestamp>) -> Vec<Filter> {
    filters
        .iter()
        .map(|f| {
            let mut f = f.clone();
            if let Some(since) = since {
                f.since = Some(f.since.map_or(since, |own| own.max(since)));
            }
            f
        })
        .collect()
}

/// Clears a relay-list lookup's in-flight entries and wakes whoever waits on
/// them, when the lookup is over or dropped.
struct ListsInFlight {
    map: Arc<Mutex<std::collections::HashMap<PublicKey, tokio::sync::watch::Receiver<bool>>>>,
    authors: Vec<PublicKey>,
    tx: tokio::sync::watch::Sender<bool>,
}

impl Drop for ListsInFlight {
    fn drop(&mut self) {
        let mut map = self.map.lock().unwrap();
        for author in &self.authors {
            map.remove(author);
        }
        let _ = self.tx.send(true);
    }
}

impl OutboxService {
    /// One query round over `lanes`, every lane at once, each bounded by
    /// `timeout`, answered as `deliver` says. The whole round is returned at
    /// the end, after the breaker and the log have seen it.
    ///
    /// What another relay answered with is kept here one way or the other,
    /// never both: a lane whose answer was **stored** (a pull, or a lane that
    /// finished after an early answer) went through the hub into the cache,
    /// waking live subscriptions; a lane whose answer the caller **took** is
    /// remembered quietly — kept kinds to the local relay, the rest to the
    /// cache.
    async fn run_round(
        &self,
        lanes: &[RelayLane],
        filters: &[Filter],
        timeout: Duration,
        deliver: Deliver<'_>,
    ) -> Vec<LaneAnswer> {
        let started = std::time::Instant::now();
        // A round bounded by less than a counted timeout — a napplet's short
        // `timeoutMs` — says nothing about the internet either way.
        let tried_internet = timeout >= crate::relay_health::COUNTED_TIMEOUT_MIN
            && lanes.iter().any(|l| self.tries_internet(l));
        let rounds: Vec<(LaneAnswer, bool)> = join_all(lanes.iter().map(|lane| async move {
            // Only what the filters asked for: a relay answering with
            // anything else is not stored, delivered, or allowed to end an
            // early answer's round.
            let events = self.query_lane(lane, filters, timeout).await.map(|events| {
                events
                    .into_iter()
                    .filter(|e| matches_any(filters, e))
                    .collect::<Vec<_>>()
            });
            let answer = (lane.clone(), events);
            let stored = match deliver {
                Deliver::Caller => false,
                // Closed by the listener when it answered: this one is late.
                Deliver::Listener(tx) => tx.send(answer.clone()).is_err(),
                Deliver::Store => true,
            };
            if stored && *lane != RelayLane::Local {
                self.keep_pulled(answer.1.as_deref().unwrap_or_default())
                    .await;
            }
            (answer, stored)
        }))
        .await;
        let out: Vec<(RelayLane, Option<Vec<Event>>)> =
            rounds.iter().map(|(answer, _)| answer.clone()).collect();
        let any_ok = out
            .iter()
            .any(|(l, r)| matches!(l, RelayLane::Internet { .. }) && r.is_some());
        self.content
            .note_internet_round(any_ok, tried_internet, started);
        // Profiles, relay lists and manifests another relay answered with are
        // kept here, and the rest cached, behind the answer, so the next ask
        // is local and works offline. Already verified by the lane; the local
        // lane's own answer is not offered back to it, nor a lane already
        // stored whole.
        self.content.remember(
            rounds
                .iter()
                .filter(|((lane, _), stored)| *lane != RelayLane::Local && !stored)
                .filter_map(|((_, events), _)| events.as_ref())
                .flatten(),
        );
        // One line per round: which lanes answered and with how much. This
        // is the first thing to look at when a napplet says "not found".
        let summary: Vec<String> = out
            .iter()
            .map(|(lane, r)| {
                let name = lane.url().unwrap_or("local");
                match r {
                    Some(events) => format!("{name}={}", events.len()),
                    None => format!("{name}=unreachable"),
                }
            })
            .collect();
        tracing::info!(
            filters = filters.len(),
            lanes = %summary.join(" "),
            "outbox query round"
        );
        out
    }

    /// Accept events another relay returned, unforwarded, into the shell
    /// **cache** (the kinds kept as seen go to the local relay): stored, and
    /// delivered to live subscriptions here. The hub dedupes by id, so an
    /// event two lanes return is delivered once. With no hub (host builds,
    /// before start) it is stored only.
    async fn keep_pulled(&self, events: &[Event]) -> usize {
        let hub = self.hub.lock().unwrap().clone();
        let cache = self.content.cache_relay();
        let mut fresh = 0usize;
        for event in events {
            let first = match &hub {
                Some(hub) => hub.accept_pulled(event.clone()).await,
                None => cache.publish(event.clone()).await.map(|()| true),
            };
            if let Ok(true) = first {
                fresh += 1;
            }
        }
        fresh
    }

    /// The session's background bookkeeping, made on first use and dropped —
    /// its tasks aborted — when the session's scope is cancelled. `None` for
    /// work with no napplet behind it, which is not bounded here.
    fn session_work(&self, scope: &WorkScope) -> Option<Arc<SessionWork>> {
        let id = scope.id()?;
        let (work, fresh) = {
            let mut map = self.work.lock().unwrap();
            match map.get(&id) {
                Some(work) => (work.clone(), false),
                None => {
                    let work = Arc::new(SessionWork {
                        permits: Arc::new(tokio::sync::Semaphore::new(MAX_BACKGROUND_PER_NAPPLET)),
                        streams: Arc::new(tokio::sync::Semaphore::new(MAX_STREAMS_PER_NAPPLET)),
                        tasks: Mutex::new(Vec::new()),
                    });
                    map.insert(id, work.clone());
                    (work, true)
                }
            }
        };
        if fresh {
            let map = self.work.clone();
            scope.session().on_cancel(move || {
                if let Some(work) = map.lock().unwrap().remove(&id) {
                    for task in work.tasks.lock().unwrap().drain(..) {
                        task.abort();
                    }
                }
            });
        }
        Some(work)
    }

    /// Tie a spawned task to `scope`: aborted when its subscription closes
    /// (if it is a subscription's), or when the session does.
    fn track(&self, scope: &WorkScope, task: tokio::task::AbortHandle) {
        if let Some(work) = self.session_work(scope) {
            let mut tasks = work.tasks.lock().unwrap();
            tasks.retain(|t| !t.is_finished());
            tasks.push(task.clone());
        }
        if let Some(sub) = scope.subscription() {
            let task = task.clone();
            sub.on_cancel(move || task.abort());
        }
        if scope.is_cancelled() {
            // Closed while this was being set up: the on-cancel hooks have
            // already run, so stop this one here.
            task.abort();
        }
    }

    /// Start a subscription's remote half on `lanes` (planned first, if
    /// `plan` says so) within `scope`, for as long as the subscription
    /// lives.
    ///
    /// Each lane gets a stream while the napplet and the app have one free
    /// ([`MAX_STREAMS_PER_NAPPLET`], [`MAX_STREAMS`]): a `REQ` held open that
    /// delivers the stored events and then every new one, re-opened with
    /// backoff when it drops. A lane past the bound gets one pull. All of it
    /// stops when the subscription closes or the window does — the task
    /// holding the streams is aborted with its scope.
    fn spawn_pull(
        &self,
        scope: &WorkScope,
        plan: Option<(Arc<dyn OutboxResolver>, Vec<PublicKey>)>,
        lanes: Vec<RelayLane>,
        filters: Vec<Filter>,
    ) {
        let this = self.detached();
        let session = self.session_work(scope);
        let task = tokio::spawn(async move {
            let lanes = match plan {
                Some((resolver, authors)) => {
                    let plan = resolver
                        .plan_hinted(Direction::Read, &authors, &lanes)
                        .await;
                    remote_lanes(plan.lanes.into_iter().chain(lanes))
                }
                None => remote_lanes(lanes),
            };
            if lanes.is_empty() {
                return;
            }
            let mut streams = tokio::task::JoinSet::new();
            let mut once = Vec::new();
            for lane in lanes {
                // A lane on the skip list, or not usable now, takes no
                // stream slot: it would hold one to do nothing. It gets the
                // one pull, which costs it nothing either.
                let usable = this.allowed(&lane)
                    && lane
                        .url()
                        .is_none_or(|url| !crate::relay_health::is_skipped(url));
                if !usable {
                    once.push(lane);
                    continue;
                }
                let napplet = match &session {
                    Some(work) => match work.streams.clone().try_acquire_owned() {
                        Ok(permit) => Some(permit),
                        Err(_) => {
                            once.push(lane);
                            continue;
                        }
                    },
                    None => None,
                };
                let Ok(app) = all_streams().try_acquire_owned() else {
                    once.push(lane);
                    continue;
                };
                let (this, filters) = (this.clone(), filters.clone());
                streams.spawn(async move {
                    let _permits = (napplet, app);
                    this.stream_lane(lane, filters).await;
                });
            }
            if !once.is_empty() {
                tracing::debug!(
                    lanes = once.len(),
                    "outbox: stream bound reached; pulling once"
                );
                this.run_round(&once, &filters, PULL_TIMEOUT, Deliver::Store)
                    .await;
            }
            // Held until aborted: dropping the set would close the streams.
            while streams.join_next().await.is_some() {}
        });
        self.track(scope, task.abort_handle());
    }

    /// Keep one lane feeding the local relay for as long as the task runs.
    /// Never returns on its own.
    async fn stream_lane(self: Arc<Self>, lane: RelayLane, filters: Vec<Filter>) {
        let mut backoff = STREAM_BACKOFF_FIRST;
        let mut since: Option<nostr::Timestamp> = None;
        loop {
            let opened = tokio::time::Instant::now();
            let asked = filters_since(&filters, since);
            // `since` moves only once everything stored up to now has been
            // heard — EOSE on a stream, an answer from a mesh re-pull. A
            // connection that failed, or dropped before EOSE, leaves it
            // where it was, so nothing stored meanwhile is skipped.
            let heard_all_from =
                nostr::Timestamp::now() - Duration::from_secs(RECONNECT_OVERLAP_SECS);
            match &lane {
                RelayLane::Mesh { .. } => {
                    let answers = self
                        .run_round(
                            std::slice::from_ref(&lane),
                            &asked,
                            PULL_TIMEOUT,
                            Deliver::Store,
                        )
                        .await;
                    if answers.first().is_some_and(|(_, events)| events.is_some()) {
                        since = Some(heard_all_from);
                    }
                    tokio::time::sleep(MESH_REPULL_EVERY + jitter(MESH_REPULL_EVERY / 3)).await;
                    continue;
                }
                RelayLane::Internet { url } if self.allowed(&lane) => {
                    // `since` moves only once a connection saw EOSE; a
                    // connection that ended sooner falls through to the
                    // backoff below.
                    let saw_eose = self.stream_internet(url, &asked).await;
                    if saw_eose {
                        since = Some(heard_all_from);
                    }
                }
                // Not allowed right now (offline only, the internet breaker):
                // wait and look again, without counting it against the relay
                // — the backoff stays where it was.
                _ => {
                    tokio::time::sleep(STREAM_BACKOFF_FIRST + jitter(STREAM_BACKOFF_FIRST)).await;
                    continue;
                }
            }
            backoff = if opened.elapsed() >= STREAM_HEALTHY_AFTER {
                STREAM_BACKOFF_FIRST
            } else {
                (backoff * 2).min(STREAM_BACKOFF_MAX)
            };
            tokio::time::sleep(backoff + jitter(backoff / 2)).await;
        }
    }

    /// One connection's life on an internet lane: its events go into the
    /// local relay as they come — the ones the filters asked for; anything
    /// else a relay sends is dropped here. Ends early when "offline only" is
    /// switched on. Returns whether the relay got as far as EOSE.
    async fn stream_internet(&self, url: &str, filters: &[Filter]) -> bool {
        if crate::relay_health::is_skipped(url) || !self.may_dial(url).await {
            return false;
        }
        let values: Vec<serde_json::Value> = filters
            .iter()
            .filter_map(|f| serde_json::to_value(f).ok())
            .collect();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Event>(STREAM_QUEUE);
        let saw_eose = std::sync::atomic::AtomicBool::new(false);
        let stream = crate::ip_source::stream_relay_filters(url, values, tx, &saw_eose);
        let _open = OpenStream::new(self.open_streams.clone(), url);
        let deliver = async {
            let mut fresh = 0usize;
            while let Some(event) = rx.recv().await {
                if matches_any(filters, &event) {
                    fresh += self.keep_pulled(std::slice::from_ref(&event)).await;
                }
            }
            fresh
        };
        let offline_only = async {
            loop {
                tokio::time::sleep(OFFLINE_CHECK_EVERY).await;
                if self.content.is_offline_only() {
                    return;
                }
            }
        };
        tokio::select! {
            (ended, fresh) = futures_util::future::join(stream, deliver) => {
                tracing::debug!(url, fresh, ended = ?ended.err(), "outbox stream closed");
            }
            () = offline_only => {
                tracing::debug!(url, "outbox stream closed: offline only");
            }
        }
        saw_eose.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl LaneTransport for OutboxService {
    async fn query(
        &self,
        lanes: &[RelayLane],
        filters: &[Filter],
        timeout: Duration,
    ) -> Vec<(RelayLane, Option<Vec<Event>>)> {
        self.run_round(lanes, filters, timeout, Deliver::Caller)
            .await
    }

    /// The round runs on its own task. When the answer goes out the lanes
    /// still running keep going — if the napplet has a background slot free
    /// — and what they find is kept in the local relay, so the next read (or
    /// a live subscription) has it. With no slot free they are dropped: the
    /// napplet has its answer, and a napplet firing queries faster than
    /// relays answer must not grow a pile of sockets behind it.
    async fn query_early(
        &self,
        lanes: &[RelayLane],
        filters: &[Filter],
        timeout: Duration,
        early: EarlyAnswer,
        scope: &WorkScope,
    ) -> Vec<(RelayLane, Option<Vec<Event>>)> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<LaneAnswer>();
        let this = self.detached();
        let (round_lanes, round_filters) = (lanes.to_vec(), filters.to_vec());
        let round = tokio::spawn(async move {
            this.run_round(
                &round_lanes,
                &round_filters,
                timeout,
                Deliver::Listener(&tx),
            )
            .await;
        });

        let started = tokio::time::Instant::now();
        let mut deadline = started + timeout;
        let mut heard: Vec<LaneAnswer> = Vec::with_capacity(lanes.len());
        while heard.len() < lanes.len() {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(answer)) => {
                    if answer.1.as_ref().is_some_and(|e| !e.is_empty()) {
                        deadline = deadline.min(if answer.0 == RelayLane::Local {
                            // This device's events: a floor, not the answer —
                            // a relay may have a newer replaceable.
                            started + early.local_cap
                        } else {
                            tokio::time::Instant::now() + early.grace
                        });
                    }
                    heard.push(answer);
                }
                // Round over (every sender gone) or out of time.
                Ok(None) | Err(_) => break,
            }
        }
        // From here a lane's send fails and it keeps its answer itself; one
        // already sent is still ours.
        rx.close();
        while let Ok(answer) = rx.try_recv() {
            heard.push(answer);
        }
        if heard.len() < lanes.len() {
            let slot = self
                .session_work(scope)
                .map(|w| w.permits.clone().try_acquire_owned());
            match slot {
                // A read of replaceable things only — profiles, follow and
                // relay lists, manifests — always finishes: a newer version
                // is the whole point of the lanes still out, each author
                // costs one event, and what they bring is kept.
                Some(Err(_)) if !only_replaceable(filters) => {
                    round.abort();
                    tracing::debug!("outbox query: no background slot; late lanes dropped");
                }
                slot => {
                    let permit = slot.and_then(Result::ok);
                    self.track(scope, round.abort_handle());
                    tokio::spawn(async move {
                        let _ = round.await;
                        drop(permit);
                    });
                }
            }
            tracing::debug!(
                heard = heard.len(),
                lanes = lanes.len(),
                waited_ms = started.elapsed().as_millis() as u64,
                "outbox query answered early"
            );
        }
        // In the order asked, a lane not heard from as `None`.
        lanes
            .iter()
            .map(|lane| {
                let answer = heard
                    .iter()
                    .position(|(l, _)| l == lane)
                    .and_then(|i| heard.swap_remove(i).1);
                (lane.clone(), answer)
            })
            .collect()
    }

    async fn publish(
        &self,
        lanes: &[RelayLane],
        event: &Event,
        timeout: Duration,
    ) -> Vec<(RelayLane, bool)> {
        let started = std::time::Instant::now();
        let tried_internet = timeout >= crate::relay_health::COUNTED_TIMEOUT_MIN
            && lanes.iter().any(|l| self.tries_internet(l));
        let out: Vec<(RelayLane, bool)> = join_all(lanes.iter().map(|lane| async move {
            (lane.clone(), self.publish_lane(lane, event, timeout).await)
        }))
        .await;
        let any_ok = out
            .iter()
            .any(|(l, ok)| matches!(l, RelayLane::Internet { .. }) && *ok);
        self.content
            .note_internet_round(any_ok, tried_internet, started);
        out
    }

    /// Every lane starts at once on a detached task, so the lanes the answer
    /// does not wait for still finish — and still count toward the internet
    /// breaker — after the napplet has its answer.
    async fn publish_quorum(
        &self,
        lanes: &[RelayLane],
        event: &Event,
        timeout: Duration,
        quorum: usize,
    ) -> Vec<(RelayLane, bool)> {
        let remote_total = lanes.iter().filter(|l| l.url().is_some()).count();
        let enough = quorum.min(remote_total);
        let has_local = lanes.iter().any(|l| l.url().is_none());

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let this = self.detached();
        let lanes = lanes.to_vec();
        let event = event.clone();
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            let tried_internet = timeout >= crate::relay_health::COUNTED_TIMEOUT_MIN
                && lanes.iter().any(|l| this.tries_internet(l));
            let out: Vec<(RelayLane, bool)> = join_all(lanes.iter().map(|lane| {
                let (this, event, tx) = (&this, &event, tx.clone());
                async move {
                    let ok = this.publish_lane(lane, event, timeout).await;
                    let _ = tx.send((lane.clone(), ok));
                    (lane.clone(), ok)
                }
            }))
            .await;
            let any_ok = out
                .iter()
                .any(|(l, ok)| matches!(l, RelayLane::Internet { .. }) && *ok);
            this.content
                .note_internet_round(any_ok, tried_internet, started);
            // The whole round, stragglers included — the answer may have
            // gone out before some of these came in.
            tracing::debug!(
                accepted = out.iter().filter(|(_, ok)| *ok).count(),
                total = out.len(),
                "publish round finished"
            );
        });

        let mut out = Vec::new();
        let (mut local_done, mut accepted, mut remote_done) = (!has_local, 0, 0);
        while let Some((lane, ok)) = rx.recv().await {
            if lane.url().is_none() {
                local_done = true;
                if !ok {
                    // Not stored here: the publish has failed, whatever
                    // the relays say.
                    out.push((lane, ok));
                    break;
                }
            } else {
                remote_done += 1;
                accepted += usize::from(ok);
            }
            out.push((lane, ok));
            if local_done && (accepted >= enough || remote_done == remote_total) {
                break;
            }
        }
        out
    }

    async fn pull_into_local(
        &self,
        lanes: &[RelayLane],
        filters: &[Filter],
        scope: &WorkScope,
    ) -> anyhow::Result<()> {
        if self.hub.lock().unwrap().is_none() {
            // Nothing to deliver through.
            return Ok(());
        }
        self.spawn_pull(scope, None, lanes.to_vec(), filters.to_vec());
        Ok(())
    }

    /// Planned and pulled on a detached task: the subscription's answer (the
    /// local backlog) does not wait for a relay list to be fetched.
    async fn pull_plan_into_local(
        &self,
        resolver: Arc<dyn OutboxResolver>,
        authors: Vec<PublicKey>,
        extra: Vec<RelayLane>,
        filters: Vec<Filter>,
        scope: &WorkScope,
    ) -> anyhow::Result<()> {
        if self.hub.lock().unwrap().is_none() {
            return Ok(());
        }
        self.spawn_pull(scope, Some((resolver, authors)), extra, filters);
        Ok(())
    }
}

/// The npub in a mesh relay URL, `ws://<npub>.fips:4870` — and `None` for
/// anything that is not exactly that shape. One strict parser, shared with
/// the runtime crate, so the lane a napplet names and the URL the pool dials
/// cannot disagree.
pub(crate) fn mesh_relay_npub(url: &str) -> Option<String> {
    myco_napplet_runtime::seams::mesh_relay_npub(url)
}

/// The lanes a kind 10002 names for `direction`: a `["r", url]` tag with no
/// marker is both, `read` and `write` markers are one each. NIP-65's marker
/// is from the author's point of view, so what *we* read from is what they
/// marked `write`, and vice versa.
///
/// Every URL goes through the same gate a napplet's `options.relays` does:
/// a relay list is signed by its author, not trusted, and a hostile one
/// naming `ws://npub1peer.fips:4870@evil.example/` or a loopback address
/// must mint no lane at all.
pub(crate) fn relay_list_lanes(list: &Event, direction: Direction) -> Vec<RelayLane> {
    relay_list_urls(list, direction)
        .into_iter()
        .filter_map(
            |url| match myco_napplet_runtime::nap::outbox::validate_relay_url(url) {
                Ok(lane) => Some(lane),
                Err(reason) => {
                    tracing::debug!(url, reason, "outbox: relay list names a URL we refuse");
                    None
                }
            },
        )
        .collect()
}

/// The raw URLs a kind 10002 names for `direction`, trimmed and without a
/// trailing slash, in the order the list gives them. The marker rule is
/// [`relay_list_lanes`]'s. **Not validated**: every caller gates each URL
/// before dialling it.
pub(crate) fn relay_list_urls(list: &Event, direction: Direction) -> Vec<&str> {
    let wanted = match direction {
        Direction::Read => "write",
        Direction::Write => "read",
    };
    list.tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            if parts.first().map(String::as_str) != Some("r") {
                return None;
            }
            let url = parts.get(1)?.trim().trim_end_matches('/');
            if url.is_empty() {
                return None;
            }
            match parts.get(2).map(|m| m.as_str()) {
                None => {}
                Some(marker) if marker == wanted => {}
                Some(_) => return None,
            }
            Some(url)
        })
        .collect()
}

/// Where a new guest publishes (NIP-65 `write`): its outbox. Its own list,
/// not [`crate::ip_source::default_relays`] — those are where Myco *looks
/// things up*, and suit that rather than being anyone's home. `nos.lol` was the obvious third pick but did not answer
/// when this list was chosen; `relay.ditto.pub` did.
pub const USER_OUTBOX_RELAYS: [&str; 3] = [
    "wss://relay.damus.io",
    "wss://relay.ditto.pub",
    "wss://relay.primal.net",
];

/// Where others write to a new guest (NIP-65 `read`), and where its NIP-17
/// direct messages go (kind 10050).
pub const USER_INBOX_RELAYS: [&str; 3] = [
    "wss://relay.damus.io",
    "wss://relay.ditto.pub",
    "wss://relay.primal.net",
];

/// NIP-65 `r` tags for an outbox and an inbox: a relay in both is one
/// unmarked tag, a relay in one only is marked `write` or `read`. Outbox
/// order first, then the inbox-only relays.
fn relay_list_tags(outbox: &[&str], inbox: &[&str]) -> anyhow::Result<Vec<nostr::Tag>> {
    let mut tags = Vec::new();
    for url in outbox {
        let mut tag = vec!["r", *url];
        if !inbox.contains(url) {
            tag.push("write");
        }
        tags.push(nostr::Tag::parse(tag)?);
    }
    for url in inbox.iter().filter(|url| !outbox.contains(url)) {
        tags.push(nostr::Tag::parse(["r", *url, "read"])?);
    }
    Ok(tags)
}

/// A new guest's kind 10002: [`USER_OUTBOX_RELAYS`] and
/// [`USER_INBOX_RELAYS`], so the user's own outbox plan resolves as NIP-65
/// rather than fallback. Signed once, when a guest is created; an imported or signer-app
/// account keeps whatever list it has, and gets none from Myco.
///
/// Deliberately **not** this device's mesh relay. `ws://<device-npub>.fips`
/// names the device key, and this event is signed by the user key: putting the
/// one inside the other would publish the link D3 exists to avoid — the social
/// identity tied to the hardware, in a signed event anyone could keep. The
/// people who can reach this device over the mesh are Circle members, and they
/// already reach its relay by policy (`OutboxService::allowed`), which needs no
/// tag to say so.
pub fn own_relay_list(keys: &nostr::Keys) -> anyhow::Result<Event> {
    let tags = relay_list_tags(&USER_OUTBOX_RELAYS, &USER_INBOX_RELAYS)?;
    Ok(nostr::EventBuilder::new(Kind::RelayList, "")
        .tags(tags)
        .sign_with_keys(keys)?)
}

/// A new guest's kind 10050 (NIP-17 DM inbox relays): one `["relay", url]`
/// tag per [`USER_INBOX_RELAYS`]. No mesh relay, for the same reason as in
/// [`own_relay_list`].
pub fn own_dm_relay_list(keys: &nostr::Keys) -> anyhow::Result<Event> {
    let tags = USER_INBOX_RELAYS
        .iter()
        .map(|url| nostr::Tag::parse(["relay", *url]))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(nostr::EventBuilder::new(Kind::InboxRelays, "")
        .tags(tags)
        .sign_with_keys(keys)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myco_napplet_runtime::seams::LaneTransport;
    use nostr::{EventBuilder, Keys, Tag};

    #[test]
    fn a_mesh_relay_url_names_its_npub() {
        assert_eq!(
            mesh_relay_npub("ws://npub1abc.fips:4870").as_deref(),
            Some("npub1abc")
        );
        assert_eq!(
            mesh_relay_npub("ws://npub1abc.fips").as_deref(),
            Some("npub1abc")
        );
        assert_eq!(mesh_relay_npub("wss://relay.damus.io"), None);
        assert_eq!(mesh_relay_npub("ws://evil.fips:4870"), None);
    }

    /// A relay list is signed, not trusted. A `.fips` URL with userinfo is
    /// userinfo on an internet host to the WebSocket client, and a wrong port
    /// is the peer's Blossom: neither may become a lane the pool would dial
    /// as the peer's relay (H2 of the PR #52 review).
    #[test]
    fn a_hostile_relay_list_cannot_name_a_userinfo_mesh_url() {
        let keys = Keys::generate();
        let hostile = EventBuilder::new(Kind::RelayList, "")
            .tags([
                Tag::parse(["r", "ws://npub1peer.fips:4870@evil.example/"]).unwrap(),
                Tag::parse(["r", "ws://npub1peer.fips:24243"]).unwrap(),
                Tag::parse(["r", "ws://npub1peer.fips:4870/path"]).unwrap(),
                Tag::parse(["r", "ws://127.0.0.1:4870"]).unwrap(),
            ])
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(relay_list_lanes(&hostile, Direction::Read), Vec::new());
        assert_eq!(relay_list_lanes(&hostile, Direction::Write), Vec::new());

        let honest = EventBuilder::new(Kind::RelayList, "")
            .tags([Tag::parse(["r", "ws://npub1peer.fips"]).unwrap()])
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(
            relay_list_lanes(&honest, Direction::Read),
            vec![RelayLane::Mesh {
                url: crate::ip_source::mesh_relay_url("npub1peer")
            }],
            "a bare .fips host is rebuilt as the canonical URL"
        );
    }

    /// The runtime crate's parser and the core's builder are the two halves
    /// of one address; if either drifts, a lane the napplet named would not
    /// be the URL the pool dials.
    #[test]
    fn mesh_url_round_trips_between_crates() {
        use myco_napplet_runtime::seams;
        assert_eq!(
            seams::mesh_relay_npub(&crate::ip_source::mesh_relay_url("npub1x")).as_deref(),
            Some("npub1x")
        );
        assert_eq!(
            seams::mesh_relay_url("npub1x"),
            crate::ip_source::mesh_relay_url("npub1x")
        );
        assert_eq!(
            mesh_relay_npub("ws://npub1peer.fips:4870@evil.example/"),
            None
        );
    }

    /// NIP-65 markers are the author's; ours are the reverse.
    #[test]
    fn relay_list_markers_are_read_from_the_authors_side() {
        let keys = Keys::generate();
        let list = EventBuilder::new(Kind::RelayList, "")
            .tags([
                Tag::parse(["r", "wss://both.example"]).unwrap(),
                Tag::parse(["r", "wss://they-write.example", "write"]).unwrap(),
                Tag::parse(["r", "wss://they-read.example/", "read"]).unwrap(),
                Tag::parse(["r", "ws://npub1peer.fips:4870"]).unwrap(),
            ])
            .sign_with_keys(&keys)
            .unwrap();

        let read = relay_list_lanes(&list, Direction::Read);
        assert_eq!(
            read,
            vec![
                RelayLane::Internet {
                    url: "wss://both.example".into()
                },
                RelayLane::Internet {
                    url: "wss://they-write.example".into()
                },
                RelayLane::Mesh {
                    url: "ws://npub1peer.fips:4870".into()
                },
            ]
        );
        let write = relay_list_lanes(&list, Direction::Write);
        assert_eq!(
            write,
            vec![
                RelayLane::Internet {
                    url: "wss://both.example".into()
                },
                RelayLane::Internet {
                    url: "wss://they-read.example".into()
                },
                RelayLane::Mesh {
                    url: "ws://npub1peer.fips:4870".into()
                },
            ]
        );
    }

    /// The user key's relay list must not carry the device key. A `.fips`
    /// relay URL *is* the device npub, and a signed event naming both is a
    /// permanent public link between the person and the hardware.
    #[test]
    fn the_own_relay_list_never_names_this_device() {
        let keys = Keys::generate();
        let list = own_relay_list(&keys).unwrap();
        assert_eq!(list.kind, Kind::RelayList);
        assert!(list.verify().is_ok());
        let lanes = relay_list_lanes(&list, Direction::Read);
        assert!(!lanes.is_empty(), "the configured relays should be listed");
        assert!(
            lanes
                .iter()
                .all(|lane| matches!(lane, RelayLane::Internet { .. })),
            "a mesh relay URL leaked the device npub into a user-key event: {lanes:?}"
        );
        assert!(
            !nostr::JsonUtil::as_json(&list).contains(".fips"),
            "no .fips host anywhere in the event"
        );
    }

    fn tag_rows<'a>(tags: impl IntoIterator<Item = &'a Tag>) -> Vec<Vec<String>> {
        tags.into_iter().map(|t| t.as_slice().to_vec()).collect()
    }

    /// A relay in both lists is one unmarked tag; one in a single list is
    /// marked from the author's side.
    #[test]
    fn a_relay_in_both_lists_is_unmarked_and_the_rest_are_marked() {
        let tags = relay_list_tags(
            &["wss://both.example", "wss://out.example"],
            &["wss://in.example", "wss://both.example"],
        )
        .unwrap();
        assert_eq!(
            tag_rows(&tags),
            vec![
                vec!["r", "wss://both.example"],
                vec!["r", "wss://out.example", "write"],
                vec!["r", "wss://in.example", "read"],
            ]
        );
    }

    /// The guest list is the user relays, not the lookup defaults: no
    /// indexer, no nostr.band.
    #[test]
    fn the_own_relay_list_is_the_user_relays() {
        let list = own_relay_list(&Keys::generate()).unwrap();
        assert_eq!(list.kind.as_u16(), 10002);
        // The outbox and inbox are the same three, so each is one unmarked tag.
        assert_eq!(
            tag_rows(list.tags.iter()),
            [
                ["r", "wss://relay.damus.io"],
                ["r", "wss://relay.ditto.pub"],
                ["r", "wss://relay.primal.net"],
            ]
        );
        let json = nostr::JsonUtil::as_json(&list);
        assert!(!json.contains("purplepag.es") && !json.contains("nostr.band"));
    }

    #[test]
    fn the_own_dm_relay_list_is_the_inbox_relays() {
        let keys = Keys::generate();
        let list = own_dm_relay_list(&keys).unwrap();
        assert_eq!(list.kind.as_u16(), 10050);
        assert_eq!(list.pubkey, keys.public_key());
        assert!(list.verify().is_ok());
        assert_eq!(
            tag_rows(list.tags.iter()),
            [
                ["relay", "wss://relay.damus.io"],
                ["relay", "wss://relay.ditto.pub"],
                ["relay", "wss://relay.primal.net"],
            ]
        );
        assert!(!nostr::JsonUtil::as_json(&list).contains(".fips"));
    }

    /// A mock internet relay: the embedded store served over a socket.
    async fn mock_relay() -> (Arc<myco_relay::RelayStore>, String) {
        let remote = Arc::new(myco_relay::RelayStore::in_memory());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        tokio::spawn(crate::mesh_relay::serve_on(remote.clone(), listener));
        (remote, url)
    }

    fn scratch_content(tag: &str) -> Arc<Content> {
        let dir = std::env::temp_dir().join(format!(
            "myco-outbox-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        Arc::new(Content::open(&dir).unwrap())
    }

    /// NAP-RELAY: a relay publish goes to the relay pool, not the Circle. The
    /// event is stored, this phone's live subscriptions hear it, and the
    /// gossiper is never handed it — a `relay` grant is not a back door to
    /// the mesh flood that NAP-MESH gates behind the user's cap.
    #[tokio::test]
    async fn a_relay_publish_is_stored_and_shown_here_but_never_flooded() {
        use crate::mesh_relay::{Gossiper, Inbound};
        use myco_napplet_runtime::seams::EventSink;

        struct Count(std::sync::Mutex<usize>);
        #[async_trait::async_trait]
        impl Gossiper for Count {
            async fn on_event(&self, _event: Event, _inbound: Inbound) {
                *self.0.lock().unwrap() += 1;
            }
        }

        let content = scratch_content("sink-local");
        content.set_offline_only(true);
        let store = content.relay();
        let count = Arc::new(Count(std::sync::Mutex::new(0)));
        let hub = RelayHub::new(store.clone(), Some(count.clone()));
        let mut live = hub.live_events();
        let svc = OutboxService::new(
            store.clone(),
            Arc::new(Mutex::new(Some(hub))),
            content,
            "npub1me".to_string(),
        )
        .allowing_private_dials();

        let keys = Keys::generate();
        let event = EventBuilder::text_note("to my relays")
            .sign_with_keys(&keys)
            .unwrap();
        svc.accept(event.clone()).await.unwrap();

        assert_eq!(live.recv().await.unwrap().id, event.id);
        assert_eq!(
            store
                .query(&[Filter::new().id(event.id)])
                .await
                .unwrap()
                .len(),
            1
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            *count.0.lock().unwrap(),
            0,
            "a relay publish reached the gossiper"
        );
    }

    /// NAP-LOCAL: `keep` stores here and sends nowhere; `rebroadcast` sends
    /// an event this phone has already seen to the relay pool anyway — once
    /// per cooldown.
    #[tokio::test]
    async fn keep_sends_nowhere_and_rebroadcast_sends_a_seen_event() {
        use myco_napplet_runtime::seams::EventSink;

        let (remote, url) = mock_relay().await;
        let content = scratch_content("keep-rebroadcast");
        let store = content.relay();
        let hub = RelayHub::new(store.clone(), None);
        let svc = OutboxService::new(
            store.clone(),
            Arc::new(Mutex::new(Some(hub.clone()))),
            content,
            "npub1me".to_string(),
        )
        .with_configured_relays(vec![url])
        .allowing_private_dials();

        let keys = Keys::generate();
        let kept = EventBuilder::text_note("mine alone")
            .sign_with_keys(&keys)
            .unwrap();
        svc.keep(kept.clone()).await.unwrap();
        assert_eq!(
            store
                .query(&[Filter::new().id(kept.id)])
                .await
                .unwrap()
                .len(),
            1
        );

        let seen = EventBuilder::text_note("seen here already")
            .sign_with_keys(&keys)
            .unwrap();
        hub.accept_unforwarded(seen.clone()).await.unwrap();
        svc.rebroadcast(seen.clone()).await.unwrap();
        let mut reached = false;
        for _ in 0..100 {
            if !remote
                .query(&[Filter::new().id(seen.id)])
                .await
                .unwrap()
                .is_empty()
            {
                reached = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            reached,
            "a rebroadcast of a seen event never reached the pool"
        );
        assert!(
            remote
                .query(&[Filter::new().id(kept.id)])
                .await
                .unwrap()
                .is_empty(),
            "a kept event was sent to the pool"
        );
        assert!(
            svc.rebroadcast(seen).await.is_err(),
            "passed on again inside the cooldown"
        );
    }

    /// A relay that never answers does not hold up the answer once two
    /// others have the event.
    #[tokio::test]
    async fn a_quorum_publish_answers_without_the_silent_relay() {
        use myco_napplet_runtime::seams::LaneTransport;

        let (a, url_a) = mock_relay().await;
        let (b, url_b) = mock_relay().await;
        // Accepts the TCP connection and says nothing, like a relay that
        // hangs in its handshake.
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let silent_url = format!("ws://{}", silent.local_addr().unwrap());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((sock, _)) = silent.accept().await {
                held.push(sock);
            }
        });

        let content = scratch_content("quorum");
        let svc = OutboxService::new(
            content.relay(),
            Arc::new(Mutex::new(None)),
            content,
            "npub1me".to_string(),
        )
        .allowing_private_dials();
        let event = EventBuilder::text_note("gg")
            .sign_with_keys(&Keys::generate())
            .unwrap();
        let lanes = vec![
            RelayLane::Local,
            RelayLane::Internet { url: url_a },
            RelayLane::Internet {
                url: silent_url.clone(),
            },
            RelayLane::Internet { url: url_b },
        ];

        let started = std::time::Instant::now();
        let out = svc
            .publish_quorum(&lanes, &event, Duration::from_secs(5), 2)
            .await;
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "waited for the silent relay: {:?}",
            started.elapsed()
        );
        assert!(out.contains(&(RelayLane::Local, true)));
        assert_eq!(
            out.iter()
                .filter(|(l, ok)| l.url().is_some() && *ok)
                .count(),
            2
        );
        assert!(!out
            .iter()
            .any(|(l, _)| l.url() == Some(silent_url.as_str())));
        assert_eq!((a.count(), b.count()), (1, 1));
    }

    /// Someone with a single outbox relay: that relay is the quorum, and the
    /// answer waits for it rather than returning on the local store alone.
    #[tokio::test]
    async fn a_quorum_publish_with_one_relay_waits_for_it() {
        use myco_napplet_runtime::seams::LaneTransport;

        // A relay that answers, but only after a pause.
        let (remote, url) = mock_relay().await;
        let slow = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let slow_url = format!("ws://{}", slow.local_addr().unwrap());
        let target = url.trim_start_matches("ws://").to_string();
        tokio::spawn(async move {
            while let Ok((mut inbound, _)) = slow.accept().await {
                let target = target.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    if let Ok(mut outbound) = tokio::net::TcpStream::connect(target).await {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                    }
                });
            }
        });

        let content = scratch_content("quorum-one");
        let svc = OutboxService::new(
            content.relay(),
            Arc::new(Mutex::new(None)),
            content,
            "npub1me".to_string(),
        )
        .allowing_private_dials();
        let event = EventBuilder::text_note("gg")
            .sign_with_keys(&Keys::generate())
            .unwrap();
        let lanes = vec![
            RelayLane::Local,
            RelayLane::Internet {
                url: slow_url.clone(),
            },
        ];
        let out = svc
            .publish_quorum(&lanes, &event, Duration::from_secs(5), 2)
            .await;
        assert!(out.contains(&(RelayLane::Local, true)));
        assert!(
            out.contains(&(RelayLane::Internet { url: slow_url }, true)),
            "answered before the only relay did: {out:?}"
        );
        assert_eq!(remote.count(), 1);
    }

    /// The internet half of the pool: the event reaches a configured relay
    /// over plain NIP-01, after the napplet already has its answer — and not
    /// at all when offline only.
    #[tokio::test]
    async fn a_relay_publish_fans_out_to_the_internet_pool() {
        use myco_napplet_runtime::seams::EventSink;

        let (remote, url) = mock_relay().await;
        let content = scratch_content("sink-pool");
        let svc = OutboxService::new(
            content.relay(),
            Arc::new(Mutex::new(None)),
            content.clone(),
            "npub1me".to_string(),
        )
        .allowing_private_dials()
        .with_configured_relays(vec![url.clone()]);
        let keys = Keys::generate();
        let event = EventBuilder::text_note("hello internet")
            .sign_with_keys(&keys)
            .unwrap();
        svc.accept(event.clone()).await.unwrap();

        let arrived = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if remote.count() == 1 {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or(false);
        assert!(arrived, "the event never reached the relay");

        // Offline only: stored here, sent nowhere.
        content.set_offline_only(true);
        let second = EventBuilder::text_note("stays home")
            .sign_with_keys(&keys)
            .unwrap();
        svc.accept(second).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(remote.count(), 1, "offline-only reached the internet");

        // An unreachable relay is a log line, not an error the napplet sees.
        content.set_offline_only(false);
        let dead = OutboxService::new(
            content.relay(),
            Arc::new(Mutex::new(None)),
            content,
            "npub1me".to_string(),
        )
        .allowing_private_dials()
        .with_configured_relays(vec!["ws://127.0.0.1:1".to_string()]);
        dead.accept(event).await.unwrap();
    }

    /// The plan follows NIP-65 when a list is stored, drops what policy
    /// forbids (a stranger's mesh relay, our own), and falls back — saying so
    /// — when it is not.
    #[tokio::test]
    async fn plans_follow_nip65_and_policy() {
        let dir = std::env::temp_dir().join(format!("myco-outbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());
        let store = content.relay();
        let (_remote, url) = mock_relay().await;
        let svc = OutboxService::new(
            store.clone(),
            Arc::new(Mutex::new(None)),
            content.clone(),
            "npub1me".to_string(),
        )
        .allowing_private_dials()
        .with_configured_relays(vec![url.clone()]);

        let alice = Keys::generate();
        let list = EventBuilder::new(Kind::RelayList, "")
            .tags([
                Tag::parse(["r", "wss://alice.example"]).unwrap(),
                Tag::parse(["r", "ws://npub1stranger.fips:4870"]).unwrap(),
                Tag::parse(["r", "ws://npub1me.fips:4870"]).unwrap(),
            ])
            .sign_with_keys(&alice)
            .unwrap();
        store.publish(list).await.unwrap();

        let plan = svc.plan(Direction::Read, &[alice.public_key()]).await;
        assert_eq!(plan.source, PlanSource::Nip65);
        assert!(plan.missing_authors.is_empty());
        assert_eq!(
            plan.lanes,
            vec![
                RelayLane::Local,
                RelayLane::Internet {
                    url: "wss://alice.example".into()
                }
            ],
            "a stranger's mesh relay and our own must be dropped"
        );

        let nobody = Keys::generate();
        let plan = svc.plan(Direction::Read, &[nobody.public_key()]).await;
        assert_eq!(plan.source, PlanSource::Fallback);
        assert_eq!(plan.missing_authors, vec![nobody.public_key()]);
        assert_eq!(
            plan.lanes,
            vec![RelayLane::Local, RelayLane::Internet { url: url.clone() }],
            "fallback offers the configured relays"
        );

        content.set_offline_only(true);
        let plan = svc.plan(Direction::Read, &[nobody.public_key()]).await;
        assert_eq!(
            plan.lanes,
            vec![RelayLane::Local],
            "offline only: nothing to fall back to"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A relay list the store does not have is fetched from the pool, cached
    /// in the store, and served from there afterwards; an author nobody has a
    /// list for is remembered as a miss rather than asked about every call.
    #[tokio::test]
    async fn a_missing_relay_list_is_fetched_once_and_cached() {
        let dir = std::env::temp_dir().join(format!("myco-outbox-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());
        let store = content.relay();
        let (remote, url) = mock_relay().await;
        let svc = OutboxService::new(
            store.clone(),
            Arc::new(Mutex::new(None)),
            content.clone(),
            "npub1me".to_string(),
        )
        .allowing_private_dials()
        .with_configured_relays(vec![url.clone()]);

        // Alice's list lives only on the internet relay.
        let alice = Keys::generate();
        let list = EventBuilder::new(Kind::RelayList, "")
            .tags([Tag::parse(["r", "wss://alice.example"]).unwrap()])
            .sign_with_keys(&alice)
            .unwrap();
        remote.admit_event(list.clone()).await.unwrap();

        let plan = svc.plan(Direction::Read, &[alice.public_key()]).await;
        assert_eq!(
            plan.source,
            PlanSource::Nip65,
            "the fetched list should count as NIP-65"
        );
        assert!(plan.lanes.contains(&RelayLane::Internet {
            url: "wss://alice.example".into()
        }));
        // Cached: the store has it now.
        let cached = store
            .query(&[Filter::new()
                .kind(Kind::RelayList)
                .author(alice.public_key())])
            .await
            .unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].id, list.id);

        // Served from the cache even when the relay is gone.
        content.set_offline_only(true);
        let plan = svc.plan(Direction::Read, &[alice.public_key()]).await;
        assert_eq!(plan.source, PlanSource::Nip65);
        content.set_offline_only(false);

        // A miss is remembered: the second ask does not touch the relay.
        let nobody = Keys::generate();
        let plan = svc.plan(Direction::Read, &[nobody.public_key()]).await;
        assert_eq!(plan.source, PlanSource::Fallback);
        assert!(svc.lists.recently_missed(&nobody.public_key()));
        drop(remote);
        let plan = svc.plan(Direction::Read, &[nobody.public_key()]).await;
        assert_eq!(plan.missing_authors, vec![nobody.public_key()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stored list older than a day is served as `cache` and refreshed
    /// behind the answer.
    #[tokio::test]
    async fn a_stale_list_is_served_as_cache_and_refreshed() {
        let dir = std::env::temp_dir().join(format!("myco-outbox-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());
        let store = content.relay();
        let (remote, url) = mock_relay().await;
        let svc = OutboxService::new(
            store.clone(),
            Arc::new(Mutex::new(None)),
            content.clone(),
            "npub1me".to_string(),
        )
        .allowing_private_dials()
        .with_configured_relays(vec![url.clone()]);

        let alice = Keys::generate();
        let old = EventBuilder::new(Kind::RelayList, "")
            .tags([Tag::parse(["r", "wss://old.example"]).unwrap()])
            .custom_created_at(nostr::Timestamp::from_secs(
                nostr::Timestamp::now().as_secs() - 3 * 24 * 60 * 60,
            ))
            .sign_with_keys(&alice)
            .unwrap();
        store.publish(old).await.unwrap();
        let newer = EventBuilder::new(Kind::RelayList, "")
            .tags([Tag::parse(["r", "wss://new.example"]).unwrap()])
            .sign_with_keys(&alice)
            .unwrap();
        remote.admit_event(newer.clone()).await.unwrap();

        let plan = svc.plan(Direction::Read, &[alice.public_key()]).await;
        assert_eq!(plan.source, PlanSource::Cache);
        assert!(plan.lanes.contains(&RelayLane::Internet {
            url: "wss://old.example".into()
        }));

        // The refresh lands; the next plan is fresh and new.
        let refreshed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let plan = svc.plan(Direction::Read, &[alice.public_key()]).await;
                if plan.source == PlanSource::Nip65 {
                    return plan;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the stale list was never refreshed");
        assert!(refreshed.lanes.contains(&RelayLane::Internet {
            url: "wss://new.example".into()
        }));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The lanes carry NIP-01: a publish lands on the local relay and on an
    /// internet relay and says so per lane; a query reads both back; a relay
    /// that is not there is `None`, never an empty answer.
    #[tokio::test]
    async fn lanes_carry_publish_and_query_and_report_a_dead_relay() {
        let dir = std::env::temp_dir().join(format!("myco-outbox-lanes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());
        let store = content.relay();
        let hub = RelayHub::new(store.clone(), None);
        let mut live = hub.live_events();
        let svc = OutboxService::new(
            store.clone(),
            Arc::new(Mutex::new(Some(hub))),
            content,
            "npub1me".to_string(),
        )
        .allowing_private_dials();

        let remote = Arc::new(myco_relay::RelayStore::in_memory());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        tokio::spawn(crate::mesh_relay::serve_on(remote.clone(), listener));

        let keys = Keys::generate();
        let note = EventBuilder::text_note("over the lanes")
            .sign_with_keys(&keys)
            .unwrap();
        let lanes = vec![
            RelayLane::Local,
            RelayLane::Internet { url: url.clone() },
            RelayLane::Internet {
                url: "ws://127.0.0.1:1".to_string(),
            },
        ];
        let verdicts = svc.publish(&lanes, &note, Duration::from_secs(5)).await;
        assert_eq!(verdicts[0], (RelayLane::Local, true));
        assert_eq!(
            verdicts[1],
            (RelayLane::Internet { url: url.clone() }, true)
        );
        assert!(!verdicts[2].1, "a dead relay accepted a publish");
        // The local lane is accepted unforwarded: live subscribers hear it.
        assert_eq!(live.recv().await.unwrap().id, note.id);
        assert_eq!(remote.count(), 1);

        let answers = svc
            .query(
                &lanes,
                &[Filter::new().kind(Kind::TextNote)],
                Duration::from_secs(5),
            )
            .await;
        assert_eq!(answers[0].1.as_ref().map(|e| e.len()), Some(1));
        assert_eq!(answers[1].1.as_ref().map(|e| e.len()), Some(1));
        assert!(answers[2].1.is_none(), "a dead relay answered");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One round where every internet lane failed — and nothing on the
    /// internet was heard, as with no signal: names do not resolve — trips a
    /// breaker: the next round skips the internet at once instead of paying
    /// the timeouts again, and reports the lane as unreached so the answer
    /// says `incomplete`. Only a round with a counted timeout (8 s or more)
    /// is judged.
    #[tokio::test]
    async fn a_dead_internet_trips_the_breaker_for_the_next_round() {
        crate::relay_health::reset();
        let dir = std::env::temp_dir().join(format!("myco-outbox-breaker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());
        let svc = OutboxService::new(
            content.relay(),
            Arc::new(Mutex::new(None)),
            content.clone(),
            "npub1me".to_string(),
        )
        .allowing_private_dials();
        let dead = RelayLane::Internet {
            url: "ws://myco-breaker-test.invalid".to_string(),
        };
        let filters = [Filter::new().kind(Kind::TextNote)];

        let first = svc
            .query(
                std::slice::from_ref(&dead),
                &filters,
                Duration::from_secs(8),
            )
            .await;
        assert!(first[0].1.is_none());
        assert!(
            content.internet_looks_down(),
            "one failed round should trip the breaker"
        );

        let started = std::time::Instant::now();
        let second = svc
            .query(
                std::slice::from_ref(&dead),
                &filters,
                Duration::from_secs(2),
            )
            .await;
        assert!(second[0].1.is_none());
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "the tripped breaker still waited on the internet"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A service over a fresh store and hub, dialling mock relays on
    /// loopback, with `configured` as its fallback relays.
    fn streaming_service(
        tag: &str,
        configured: Vec<String>,
    ) -> (OutboxService, Arc<dyn RelayBackend>, Arc<RelayHub>) {
        let content = scratch_content(tag);
        let store = content.relay();
        let hub = RelayHub::new(store.clone(), None);
        let svc = OutboxService::new(
            store.clone(),
            Arc::new(Mutex::new(Some(hub.clone()))),
            content,
            "npub1me".to_string(),
        )
        .with_configured_relays(configured)
        .allowing_private_dials();
        (svc, store, hub)
    }

    fn note(text: &str) -> Event {
        EventBuilder::text_note(text)
            .sign_with_keys(&Keys::generate())
            .unwrap()
    }

    fn early(grace_ms: u64, local_cap_ms: u64) -> EarlyAnswer {
        EarlyAnswer {
            grace: Duration::from_millis(grace_ms),
            local_cap: Duration::from_millis(local_cap_ms),
        }
    }

    /// NAP-level `QUERY_EARLY`, restated: the runtime crate keeps it private.
    const NAP_EARLY: EarlyAnswer = EarlyAnswer {
        grace: Duration::ZERO,
        local_cap: Duration::ZERO,
    };

    /// Poll the store until `id` is in it, or give up.
    async fn lands(store: &Arc<dyn RelayBackend>, id: nostr::EventId, within: Duration) -> bool {
        let until = std::time::Instant::now() + within;
        while std::time::Instant::now() < until {
            if !store
                .query(&[Filter::new().id(id)])
                .await
                .unwrap()
                .is_empty()
            {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    /// The device observed: `local=6` and every internet lane slow or dead.
    /// A one-shot read answers with what this device holds once the local
    /// cap is up — not at the local lane's first millisecond, and not at the
    /// slow relay's timeout — and the relay not heard from reads as `None`.
    #[tokio::test]
    async fn a_query_answers_from_local_at_the_cap_while_a_relay_hangs() {
        let held = note("held here");
        let (slow, _) = crate::ip_source::tests::mock_relay_delayed(
            vec![note("far away")],
            Duration::from_secs(5),
        )
        .await;
        let (svc, store, _hub) = streaming_service("early-local", Vec::new());
        store.publish(held.clone()).await.unwrap();

        let started = std::time::Instant::now();
        let answers = svc
            .query_early(
                &[RelayLane::Local, RelayLane::Internet { url: slow.clone() }],
                &[Filter::new().kind(Kind::TextNote)],
                Duration::from_secs(8),
                early(300, 600),
                &WorkScope::detached(),
            )
            .await;
        let waited = started.elapsed();

        assert!(
            waited >= Duration::from_millis(500),
            "answered from local before the cap: {waited:?}"
        );
        assert!(
            waited < Duration::from_millis(1500),
            "the answer waited {waited:?} on a relay that hangs"
        );
        assert_eq!(answers[0].1.as_ref().map(|e| e[0].id), Some(held.id));
        assert_eq!(answers[1].0, RelayLane::Internet { url: slow });
        assert!(answers[1].1.is_none(), "an unheard lane must read as None");
    }

    /// This device holds an old profile, a relay has the new one and answers
    /// at 800 ms — a cold dial on a phone. The answer does not wait for it:
    /// the profile held here goes out at once, and the relay's newer one
    /// lands in the local store behind the answer, for the next read and any
    /// live subscription.
    #[tokio::test]
    async fn the_held_version_answers_at_once_and_a_newer_one_lands_after() {
        let keys = Keys::generate();
        let old = EventBuilder::metadata(&nostr::Metadata::new().name("old"))
            .custom_created_at(nostr::Timestamp::from(
                nostr::Timestamp::now().as_secs() - 3600,
            ))
            .sign_with_keys(&keys)
            .unwrap();
        let new = EventBuilder::metadata(&nostr::Metadata::new().name("new"))
            .sign_with_keys(&keys)
            .unwrap();
        let (relay, _) = crate::ip_source::tests::mock_relay_delayed(
            vec![new.clone()],
            Duration::from_millis(800),
        )
        .await;
        let (svc, store, _hub) = streaming_service("early-stale", Vec::new());
        store.publish(old.clone()).await.unwrap();

        let started = std::time::Instant::now();
        let answers = svc
            .query_early(
                &[RelayLane::Local, RelayLane::Internet { url: relay }],
                &[Filter::new().kind(Kind::Metadata).author(keys.public_key())],
                Duration::from_secs(5),
                NAP_EARLY,
                &WorkScope::detached(),
            )
            .await;
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "the answer waited on the relay: {:?}",
            started.elapsed()
        );
        assert_eq!(answers[0].1.as_ref().map(|e| e[0].id), Some(old.id));
        assert!(answers[1].1.is_none(), "the slow relay was waited for");
        assert!(
            lands(&store, new.id, Duration::from_secs(5)).await,
            "the relay's newer profile never reached the local store"
        );
        let now = store
            .query(&[Filter::new().kind(Kind::Metadata).author(keys.public_key())])
            .await
            .unwrap();
        assert_eq!(
            now.iter().map(|e| e.id).collect::<Vec<_>>(),
            [new.id],
            "the store kept both"
        );
    }

    /// Reads of replaceable kinds only are the ones whose late lanes must
    /// always finish; anything else may be cut when the napplet is busy.
    #[test]
    fn only_replaceable_reads_are_marked_to_finish() {
        assert!(only_replaceable(&[
            Filter::new().kinds([Kind::Metadata, Kind::ContactList])
        ]));
        assert!(only_replaceable(&[
            Filter::new().kind(Kind::from(30_023u16))
        ]));
        assert!(!only_replaceable(&[Filter::new().kind(Kind::TextNote)]));
        assert!(!only_replaceable(&[
            Filter::new().author(Keys::generate().public_key())
        ]));
        assert!(!only_replaceable(&[]));
    }

    /// A second relay answering inside the grace that the first remote
    /// answer started is in the answer.
    #[tokio::test]
    async fn a_query_takes_a_relay_that_answers_inside_the_grace() {
        let first = note("from the quick relay");
        let second = note("from the next relay");
        let (quick, _) = crate::ip_source::tests::mock_relay_holding(vec![first.clone()]).await;
        let (next, _) = crate::ip_source::tests::mock_relay_delayed(
            vec![second.clone()],
            Duration::from_millis(300),
        )
        .await;
        let (svc, _store, _hub) = streaming_service("early-grace", Vec::new());

        let answers = svc
            .query_early(
                &[
                    RelayLane::Local,
                    RelayLane::Internet { url: quick },
                    RelayLane::Internet { url: next },
                ],
                &[Filter::new().kind(Kind::TextNote)],
                Duration::from_secs(8),
                early(2000, 1500),
                &WorkScope::detached(),
            )
            .await;
        assert_eq!(answers[1].1.as_ref().map(|e| e[0].id), Some(first.id));
        assert_eq!(answers[2].1.as_ref().map(|e| e[0].id), Some(second.id));
    }

    /// Every lane finished and nobody had anything: the answer goes then,
    /// not after the grace and not after the timeout.
    #[tokio::test]
    async fn a_query_answers_at_once_when_every_lane_has_finished() {
        let (empty, _) = crate::ip_source::tests::mock_relay_holding(Vec::new()).await;
        let (svc, _store, _hub) = streaming_service("early-done", Vec::new());

        let started = std::time::Instant::now();
        let answers = svc
            .query_early(
                &[RelayLane::Local, RelayLane::Internet { url: empty }],
                &[Filter::new().kind(Kind::TextNote)],
                Duration::from_secs(8),
                early(5000, 5000),
                &WorkScope::detached(),
            )
            .await;
        assert!(started.elapsed() < Duration::from_millis(1500));
        assert_eq!(answers[0].1.as_ref().map(Vec::len), Some(0));
        assert_eq!(answers[1].1.as_ref().map(Vec::len), Some(0));
    }

    /// Nothing anywhere and one relay hanging: the timeout still bounds it.
    #[tokio::test]
    async fn a_query_with_nothing_found_still_ends_at_the_timeout() {
        let (hung, _) =
            crate::ip_source::tests::mock_relay_delayed(Vec::new(), Duration::from_secs(10)).await;
        let (svc, _store, _hub) = streaming_service("early-timeout", Vec::new());

        let started = std::time::Instant::now();
        let answers = svc
            .query_early(
                &[RelayLane::Local, RelayLane::Internet { url: hung }],
                &[Filter::new().kind(Kind::TextNote)],
                Duration::from_millis(800),
                early(300, 300),
                &WorkScope::detached(),
            )
            .await;
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_millis(700),
            "answered before the timeout: {waited:?}"
        );
        assert!(
            waited < Duration::from_millis(2000),
            "overran the timeout: {waited:?}"
        );
        assert!(answers[1].1.is_none());
    }

    /// A relay that answers after the query has is not wasted: what it
    /// found lands in the local relay — any kind, not just the kept ones —
    /// so the next read has it.
    #[tokio::test]
    async fn a_lane_that_misses_the_answer_is_kept_here() {
        let late = note("from the slow relay");
        let (slow, _) = crate::ip_source::tests::mock_relay_delayed(
            vec![late.clone()],
            Duration::from_millis(700),
        )
        .await;
        let (svc, store, _hub) = streaming_service("early-late", Vec::new());
        store.publish(note("held here")).await.unwrap();

        let answers = svc
            .query_early(
                &[RelayLane::Local, RelayLane::Internet { url: slow }],
                &[Filter::new().kind(Kind::TextNote)],
                Duration::from_secs(5),
                early(100, 200),
                &WorkScope::detached(),
            )
            .await;
        assert!(answers[1].1.is_none(), "the slow relay made the answer");
        assert!(
            lands(&store, late.id, Duration::from_secs(3)).await,
            "the late relay's event was dropped"
        );
    }

    /// A subscription's pull is a stream: the quick relay's event is
    /// delivered while the slow one is still thinking, an event two relays
    /// both hold is delivered once, and the slow relay's event still lands
    /// when it comes. Starting the pull returns at once, so the napplet's
    /// EOSE (sent after the local backlog) never waits on a lane.
    #[tokio::test]
    async fn a_pull_delivers_each_relay_as_it_answers_and_once_per_event() {
        let shared = note("on two relays");
        let late = note("from the slow relay");
        let (quick_a, _) = crate::ip_source::tests::mock_relay_holding(vec![shared.clone()]).await;
        let (quick_b, _) = crate::ip_source::tests::mock_relay_holding(vec![shared.clone()]).await;
        let (slow, _) =
            crate::ip_source::tests::mock_relay_delayed(vec![late.clone()], Duration::from_secs(3))
                .await;
        let (svc, _store, hub) = streaming_service("pull-stream", Vec::new());
        let mut live = hub.live_events();

        let started = std::time::Instant::now();
        svc.pull_into_local(
            &[
                RelayLane::Internet { url: quick_a },
                RelayLane::Internet { url: quick_b },
                RelayLane::Internet { url: slow },
            ],
            &[Filter::new().kind(Kind::TextNote)],
            &WorkScope::detached(),
        )
        .await
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "starting a pull waited on its lanes"
        );

        let first = tokio::time::timeout(Duration::from_millis(1500), live.recv())
            .await
            .expect("the quick relays' event waited on the slow relay")
            .unwrap();
        assert_eq!(first.id, shared.id);

        let next = tokio::time::timeout(Duration::from_secs(6), live.recv())
            .await
            .expect("the slow relay's event never landed")
            .unwrap();
        assert_eq!(
            next.id, late.id,
            "an event two relays hold was delivered twice"
        );
        assert!(
            started.elapsed() >= Duration::from_secs(2),
            "the slow relay was not slow"
        );
    }

    /// An outbox subscription's plan is made behind its answer: an author
    /// nobody has a relay list for costs the napplet nothing up front, and
    /// the planned lanes are still pulled once the lookup gives up.
    #[tokio::test]
    async fn a_planned_pull_returns_before_the_plan_is_made() {
        let author = Keys::generate();
        let theirs = EventBuilder::text_note("from the fallback relay")
            .sign_with_keys(&author)
            .unwrap();
        // The only configured relay: searched for the list (it has none, and
        // takes its time saying so), then used as the fallback lane.
        let (slow, _) = crate::ip_source::tests::mock_relay_delayed(
            vec![theirs.clone()],
            Duration::from_millis(1500),
        )
        .await;
        let (svc, _store, hub) = streaming_service("pull-plan", vec![slow]);
        let svc = Arc::new(svc);
        let mut live = hub.live_events();

        let started = std::time::Instant::now();
        svc.pull_plan_into_local(
            svc.clone(),
            vec![author.public_key()],
            Vec::new(),
            vec![Filter::new()
                .kind(Kind::TextNote)
                .author(author.public_key())],
            &WorkScope::detached(),
        )
        .await
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "the subscription waited on its relay plan"
        );

        let got = tokio::time::timeout(Duration::from_secs(8), live.recv())
            .await
            .expect("the planned lane was never pulled")
            .unwrap();
        assert_eq!(got.id, theirs.id);
    }

    fn relay_list_of(keys: &Keys, url: &str) -> Event {
        EventBuilder::new(Kind::RelayList, "")
            .tags([Tag::parse(["r", url]).unwrap()])
            .sign_with_keys(keys)
            .unwrap()
    }

    /// Two authors with no list here cost one `REQ` per relay, not one per
    /// author, and both plans come back from their lists.
    #[tokio::test]
    async fn unknown_authors_are_looked_up_in_one_request() {
        let (a, b) = (Keys::generate(), Keys::generate());
        let (index, reqs) = crate::ip_source::tests::mock_relay_holding(vec![
            relay_list_of(&a, "wss://a.example"),
            relay_list_of(&b, "wss://b.example"),
        ])
        .await;
        let (svc, _store, _hub) = streaming_service("plan-batch", vec![index]);

        let plan = svc
            .plan(Direction::Read, &[a.public_key(), b.public_key()])
            .await;
        assert_eq!(reqs.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(plan.missing_authors.is_empty());
        for url in ["wss://a.example", "wss://b.example"] {
            assert!(plan.lanes.contains(&RelayLane::Internet {
                url: url.to_string()
            }));
        }
    }

    /// Two plans at once for the same unknown author share one lookup.
    #[tokio::test]
    async fn concurrent_plans_share_a_lookup() {
        let a = Keys::generate();
        let (index, reqs) = crate::ip_source::tests::mock_relay_delayed(
            vec![relay_list_of(&a, "wss://a.example")],
            Duration::from_millis(400),
        )
        .await;
        let (svc, _store, _hub) = streaming_service("plan-share", vec![index]);

        let who = [a.public_key()];
        let (one, two) = tokio::join!(
            svc.plan(Direction::Read, &who),
            svc.plan(Direction::Read, &who),
        );
        assert_eq!(reqs.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(one.missing_authors.is_empty() && two.missing_authors.is_empty());
    }

    /// Closing the window stops its pulls: the slow relay's event never
    /// lands. Closing one subscription stops that one's.
    #[tokio::test]
    async fn closing_a_session_or_a_subscription_stops_its_pull() {
        use myco_napplet_runtime::seams::ScopeOwner;

        let (svc, store, _hub) = streaming_service("pull-cancel", Vec::new());
        let lanes_for = |url: String| vec![RelayLane::Internet { url }];

        // The session closes.
        let session_note = note("for a closed window");
        let (slow, _) = crate::ip_source::tests::mock_relay_delayed(
            vec![session_note.clone()],
            Duration::from_millis(500),
        )
        .await;
        let session = ScopeOwner::new();
        svc.pull_into_local(
            &lanes_for(slow),
            &[Filter::new().kind(Kind::TextNote)],
            &session.scope(),
        )
        .await
        .unwrap();
        drop(session);

        // A subscription closes; its session stays open.
        let sub_note = note("for a closed subscription");
        let (slow, _) = crate::ip_source::tests::mock_relay_delayed(
            vec![sub_note.clone()],
            Duration::from_millis(500),
        )
        .await;
        let session = ScopeOwner::new();
        let sub = ScopeOwner::new();
        svc.pull_into_local(
            &lanes_for(slow),
            &[Filter::new().kind(Kind::TextNote)],
            &session.scope().with(&sub),
        )
        .await
        .unwrap();
        drop(sub);

        assert!(!lands(&store, session_note.id, Duration::from_millis(1500)).await);
        assert!(!lands(&store, sub_note.id, Duration::from_millis(100)).await);
        drop(session);
    }

    /// A napplet's background work is bounded: past four rounds left
    /// running behind answers, the next query's leftovers are dropped
    /// rather than piled up.
    #[tokio::test]
    async fn a_napplets_background_rounds_are_bounded() {
        use myco_napplet_runtime::seams::ScopeOwner;

        let (hung, _) =
            crate::ip_source::tests::mock_relay_delayed(Vec::new(), Duration::from_secs(10)).await;
        let (svc, store, _hub) = streaming_service("bounded", Vec::new());
        store.publish(note("held here")).await.unwrap();
        let session = ScopeOwner::new();
        let scope = session.scope();

        for _ in 0..MAX_BACKGROUND_PER_NAPPLET + 2 {
            svc.query_early(
                &[RelayLane::Local, RelayLane::Internet { url: hung.clone() }],
                &[Filter::new().kind(Kind::TextNote)],
                Duration::from_secs(8),
                early(50, 50),
                &scope,
            )
            .await;
        }
        let work = svc.session_work(&scope).unwrap();
        assert_eq!(work.permits.available_permits(), 0);
        let running = work
            .tasks
            .lock()
            .unwrap()
            .iter()
            .filter(|t| !t.is_finished())
            .count();
        assert_eq!(running, MAX_BACKGROUND_PER_NAPPLET);

        // And closing the window stops them.
        drop(session);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(svc.work.lock().unwrap().is_empty());
    }

    /// A subscription is a stream for as long as it is open: a note a
    /// remote relay receives 15 s after the napplet subscribed still reaches
    /// this device — no one-shot pull would have seen it — and once the
    /// subscription closes, the next one does not.
    #[tokio::test]
    async fn a_subscription_hears_a_relay_for_its_whole_life_and_not_after() {
        use myco_napplet_runtime::seams::ScopeOwner;

        let (_remote, url) = mock_relay().await;
        let (svc, store, hub) = streaming_service("stream-live", Vec::new());
        let mut live = hub.live_events();
        let session = ScopeOwner::new();
        let sub = ScopeOwner::new();
        svc.pull_into_local(
            &[RelayLane::Internet { url: url.clone() }],
            &[Filter::new().kind(Kind::TextNote)],
            &session.scope().with(&sub),
        )
        .await
        .unwrap();

        tokio::time::sleep(Duration::from_secs(15)).await;
        let later = note("published a while after the napplet subscribed");
        assert!(crate::ip_source::publish_to_relay(&url, &later)
            .await
            .unwrap());
        let got = tokio::time::timeout(Duration::from_secs(3), live.recv())
            .await
            .expect("the open subscription missed a live event")
            .unwrap();
        assert_eq!(got.id, later.id);

        // Close: the relay's REQ goes with it.
        drop(sub);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let after = note("published after the subscription closed");
        assert!(crate::ip_source::publish_to_relay(&url, &after)
            .await
            .unwrap());
        assert!(
            !lands(&store, after.id, Duration::from_millis(1500)).await,
            "a closed subscription kept its relay stream"
        );
        drop(session);
    }

    /// An author whose relay list is only on an indexer — not on any
    /// configured relay, not in the Circle — is planned onto the write
    /// relays that list names.
    #[tokio::test]
    async fn a_list_found_only_on_an_indexer_plans_the_authors_relays() {
        let author = Keys::generate();
        let list = EventBuilder::new(Kind::RelayList, "")
            .tags([
                Tag::parse(["r", "wss://nostr.wine"]).unwrap(),
                Tag::parse(["r", "wss://pyramid.fiatjaf.com", "write"]).unwrap(),
            ])
            .sign_with_keys(&author)
            .unwrap();
        let (configured, _) = crate::ip_source::tests::mock_relay_holding(Vec::new()).await;
        let (indexer, _) = crate::ip_source::tests::mock_relay_holding(vec![list]).await;
        let (svc, _store, _hub) = streaming_service("plan-indexer", vec![configured]);
        let svc = svc.with_indexers(vec![indexer]);

        let plan = svc.plan(Direction::Read, &[author.public_key()]).await;
        assert!(plan.missing_authors.is_empty());
        assert_eq!(plan.source, PlanSource::Nip65);
        for url in ["wss://nostr.wine", "wss://pyramid.fiatjaf.com"] {
            assert!(
                plan.lanes
                    .contains(&RelayLane::Internet { url: url.into() }),
                "{url} missing from {:?}",
                plan.lanes
            );
        }
    }

    /// A list found only on a relay the napplet named is used; and with
    /// relays named, an author found nowhere is not remembered as having no
    /// list — nor when a relay asked timed out rather than answering. Those
    /// are soft misses: not asked again for a minute, not ten.
    #[tokio::test]
    async fn misses_are_remembered_only_when_every_relay_said_no_and_none_were_named() {
        let (empty, _) = crate::ip_source::tests::mock_relay_holding(Vec::new()).await;
        let (hung, _) =
            crate::ip_source::tests::mock_relay_delayed(Vec::new(), Duration::from_secs(10)).await;

        // A napplet-named relay holds the list.
        let named = Keys::generate();
        let (hint, _) = crate::ip_source::tests::mock_relay_holding(vec![EventBuilder::new(
            Kind::RelayList,
            "",
        )
        .tags([Tag::parse(["r", "wss://named.example"]).unwrap()])
        .sign_with_keys(&named)
        .unwrap()])
        .await;
        let (svc, _store, _hub) = streaming_service("plan-hints", vec![empty.clone()]);
        let hints = [RelayLane::Internet { url: hint }];
        let plan = svc
            .plan_hinted(Direction::Read, &[named.public_key()], &hints)
            .await;
        assert!(plan.lanes.contains(&RelayLane::Internet {
            url: "wss://named.example".into()
        }));

        let soft = crate::ip_source::LIST_SOFT_MISS_FOR;

        // Named relays, found nowhere: a soft miss only.
        let nobody = Keys::generate();
        svc.plan_hinted(Direction::Read, &[nobody.public_key()], &hints)
            .await;
        assert!(svc.lists.miss_left(&nobody.public_key()).unwrap() <= soft);

        // A relay that timed out: a soft miss only.
        let (svc, _store, _hub) = streaming_service("plan-timeout", vec![empty.clone(), hung]);
        let unsure = Keys::generate();
        svc.plan(Direction::Read, &[unsure.public_key()]).await;
        assert!(svc.lists.miss_left(&unsure.public_key()).unwrap() <= soft);

        // Every relay said no: the full ten minutes.
        let (svc, _store, _hub) = streaming_service("plan-clean-no", vec![empty]);
        let absent = Keys::generate();
        svc.plan(Direction::Read, &[absent.public_key()]).await;
        assert!(svc.lists.miss_left(&absent.public_key()).unwrap() > soft);
    }

    /// Device bug: lists published months ago were "stale" on every plan,
    /// and each author got a lookup round of its own. Now the stale authors
    /// of a plan share one round, and an author checked lately is fresh
    /// whatever its list's age.
    #[tokio::test]
    async fn stale_lists_are_rechecked_together_and_then_left_alone() {
        let (index, reqs) = crate::ip_source::tests::mock_relay_holding(Vec::new()).await;
        let (svc, store, _hub) = streaming_service("stale-batch", vec![index]);
        let old = nostr::Timestamp::from(nostr::Timestamp::now().as_secs() - 90 * 24 * 3600);
        let authors: Vec<Keys> = (0..3).map(|_| Keys::generate()).collect();
        for keys in &authors {
            let list = EventBuilder::new(Kind::RelayList, "")
                .tags([Tag::parse(["r", "wss://old.example"]).unwrap()])
                .custom_created_at(old)
                .sign_with_keys(keys)
                .unwrap();
            store.publish(list).await.unwrap();
        }
        let pks: Vec<PublicKey> = authors.iter().map(|k| k.public_key()).collect();

        let plan = svc.plan(Direction::Read, &pks).await;
        assert_eq!(plan.source, PlanSource::Cache);
        for _ in 0..100 {
            if reqs.load(std::sync::atomic::Ordering::SeqCst) > 0
                && svc.refreshing.lock().unwrap().is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            reqs.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "one round for all three"
        );

        let plan = svc.plan(Direction::Read, &pks).await;
        assert_eq!(plan.source, PlanSource::Nip65, "checked lately is fresh");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(reqs.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// A relay that answers with events nobody asked for gets none of them
    /// stored or delivered, and they do not count as an answer.
    #[tokio::test]
    async fn events_outside_the_filters_are_dropped() {
        // The mock matches on kind and author only; the filter also wants a
        // tag the note does not have.
        let (relay, _) =
            crate::ip_source::tests::mock_relay_holding(vec![note("no tag here")]).await;
        let (svc, store, _hub) = streaming_service("unasked", Vec::new());
        let filter = Filter::new().kind(Kind::TextNote).custom_tag(
            nostr::SingleLetterTag::lowercase(nostr::Alphabet::T),
            "wanted",
        );
        let answers = svc
            .query_early(
                &[RelayLane::Internet { url: relay.clone() }],
                std::slice::from_ref(&filter),
                Duration::from_secs(3),
                early(0, 0),
                &WorkScope::detached(),
            )
            .await;
        assert_eq!(answers[0].1.as_ref().map(Vec::len), Some(0));

        svc.pull_into_local(
            &[RelayLane::Internet { url: relay }],
            &[filter],
            &WorkScope::detached(),
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(store.query(&[Filter::new()]).await.unwrap().is_empty());
    }

    /// A stream whose first connect fails must not move `since` past what
    /// the relay holds: when the relay comes up, its stored (older) note
    /// still lands.
    #[tokio::test]
    async fn a_failed_connect_does_not_skip_what_the_relay_holds() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let url = format!("ws://127.0.0.1:{port}");
        let (svc, store, _hub) = streaming_service("since-failed", Vec::new());
        svc.pull_into_local(
            &[RelayLane::Internet { url }],
            &[Filter::new().kind(Kind::TextNote)],
            &WorkScope::detached(),
        )
        .await
        .unwrap();
        // Let a connect or two fail.
        tokio::time::sleep(Duration::from_millis(400)).await;

        let old = EventBuilder::text_note("stored an hour ago")
            .custom_created_at(nostr::Timestamp::from(
                nostr::Timestamp::now().as_secs() - 3600,
            ))
            .sign_with_keys(&Keys::generate())
            .unwrap();
        let remote = Arc::new(myco_relay::RelayStore::in_memory());
        remote.admit_event(old.clone()).await.unwrap();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap();
        tokio::spawn(crate::mesh_relay::serve_on(remote, listener));

        assert!(
            lands(&store, old.id, Duration::from_secs(5)).await,
            "the stored note was skipped by a `since` moved on a failed connect"
        );
    }

    /// A relay on the skip list costs a round nothing: it is not dialled,
    /// reads as finished-with-nothing at once, and a round of only skipped
    /// relays does not trip the internet breaker that would hold back the
    /// relays that work.
    #[tokio::test]
    async fn a_skipped_relay_finishes_at_once_and_trips_no_breaker() {
        use myco_napplet_runtime::seams::LaneTransport;

        let (hung, served) =
            crate::ip_source::tests::mock_relay_delayed(Vec::new(), Duration::from_secs(10)).await;
        crate::relay_health::current()
            .failed(&hung, crate::relay_health::Failure::Refused("HTTP 403"));
        let content = scratch_content("skipped-lane");
        let svc = OutboxService::new(
            content.relay(),
            Arc::new(Mutex::new(None)),
            content.clone(),
            "npub1me".to_string(),
        )
        .allowing_private_dials();

        let started = std::time::Instant::now();
        let answers = svc
            .query(
                &[RelayLane::Local, RelayLane::Internet { url: hung }],
                &[Filter::new().kind(Kind::TextNote)],
                Duration::from_secs(5),
            )
            .await;
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(answers[1].1.is_none());
        assert_eq!(
            served.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a skipped relay was dialled"
        );
        assert!(
            !content.internet_looks_down(),
            "a round of skipped relays tripped the breaker"
        );
    }

    /// Device bug A: one relay answering 502 in a round of its own tripped
    /// the internet breaker for everyone while other relays were answering.
    /// An HTTP answer is the internet working; the breaker stays open. And a
    /// breaker that did trip clears the moment a stream connects.
    #[tokio::test]
    async fn a_relays_502_does_not_trip_the_breaker_and_a_connect_clears_it() {
        crate::relay_health::reset();
        let (addr, _) = crate::relay_health::tests::answering(502).await;
        let content = scratch_content("breaker-502");
        let svc = OutboxService::new(
            content.relay(),
            Arc::new(Mutex::new(None)),
            content.clone(),
            "npub1me".to_string(),
        )
        .allowing_private_dials();
        let answers = svc
            .query(
                &[RelayLane::Internet {
                    url: format!("ws://{addr}"),
                }],
                &[Filter::new()],
                Duration::from_secs(10),
            )
            .await;
        assert!(answers[0].1.is_none());
        assert!(!content.internet_looks_down(), "a 502 tripped the breaker");

        // Tripped for real: nothing heard since the round began.
        content.note_internet_round(false, true, std::time::Instant::now());
        assert!(content.internet_looks_down());
        let (_remote, url) = mock_relay().await;
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let flag = std::sync::atomic::AtomicBool::new(false);
        let _ = tokio::time::timeout(
            Duration::from_millis(500),
            crate::ip_source::stream_relay_filters(&url, vec![serde_json::json!({})], tx, &flag),
        )
        .await;
        assert!(
            !content.internet_looks_down(),
            "a stream connected and the breaker stayed tripped"
        );
    }

    fn relays(list: &[&str]) -> Vec<String> {
        list.iter().map(|u| format!("wss://{u}")).collect()
    }

    fn select(author_relays: &[Vec<String>], skipped: &[&str], open: &[&str]) -> Vec<String> {
        let skipped: Vec<String> = relays(skipped);
        let open: Vec<String> = relays(open);
        select_relays(
            author_relays,
            |u| skipped.iter().any(|s| s == u),
            |u| open.iter().any(|o| o == u),
            RELAYS_PER_AUTHOR,
            MAX_SELECTED_RELAYS,
        )
    }

    /// Every author twice where their lists allow, with the relays most
    /// authors share, not the union of everyone's.
    #[test]
    fn selection_covers_each_author_twice_with_shared_relays() {
        let lists = vec![
            relays(&["damus", "primal", "a-own"]),
            relays(&["damus", "primal", "b-own"]),
            relays(&["damus", "nos", "c-own"]),
            relays(&["primal", "nos"]),
        ];
        let chosen = select(&lists, &[], &[]);
        for (i, list) in lists.iter().enumerate() {
            let n = list.iter().filter(|u| chosen.contains(u)).count();
            assert!(n >= 2, "author {i} covered {n} times by {chosen:?}");
        }
        assert!(chosen.len() <= 3, "{chosen:?}");
        assert!(!chosen.iter().any(|u| u.ends_with("-own")));
    }

    /// Forty authors on forty relays of their own: the plan stops at the
    /// cap, and covers as many authors once as it can before any twice.
    #[test]
    fn selection_respects_the_cap() {
        let lists: Vec<Vec<String>> = (0..40)
            .map(|i| relays(&[&format!("own{i}"), &format!("alt{i}")]))
            .collect();
        let chosen = select(&lists, &[], &[]);
        assert_eq!(chosen.len(), MAX_SELECTED_RELAYS);
        let covered = lists
            .iter()
            .filter(|l| l.iter().any(|u| chosen.contains(u)))
            .count();
        assert_eq!(covered, MAX_SELECTED_RELAYS, "once each before twice");
    }

    /// A skip-listed relay is avoided when another covers the author, and
    /// still chosen when nothing else does; an author with one relay gets it.
    #[test]
    fn selection_avoids_skipped_relays_but_never_drops_an_author() {
        let lists = vec![
            relays(&["broken", "good"]),
            relays(&["broken", "good2"]),
            relays(&["only-broken"]),
            relays(&["lonely"]),
        ];
        let chosen = select(&lists, &["broken", "only-broken"], &[]);
        assert!(!chosen.contains(&"wss://broken".to_string()));
        assert!(chosen.contains(&"wss://only-broken".to_string()));
        assert!(chosen.contains(&"wss://lonely".to_string()));
        assert!(chosen.contains(&"wss://good".to_string()));
        assert!(chosen.contains(&"wss://good2".to_string()));
    }

    /// Ties go to a relay already streaming; and the same lists always give
    /// the same plan.
    #[test]
    fn selection_prefers_open_streams_and_is_deterministic() {
        let lists = vec![relays(&["x", "y", "z"])];
        let chosen = select(&lists, &[], &["z"]);
        assert_eq!(chosen[0], "wss://z");
        let lists = vec![relays(&["a", "b", "c"]), relays(&["c", "b", "a"])];
        let first = select(&lists, &[], &[]);
        for _ in 0..10 {
            assert_eq!(select(&lists, &[], &[]), first);
        }
        assert_eq!(first, relays(&["a", "b"]));
    }

    /// Through the plan: authors' relays are narrowed, while fallback lanes
    /// for an author with no list, and mesh lanes, stay; a write plan
    /// (inbox delivery) is not narrowed at all.
    #[tokio::test]
    async fn a_read_plan_narrows_author_relays_and_keeps_the_rest() {
        let (fallback, _) = crate::ip_source::tests::mock_relay_holding(Vec::new()).await;
        let (svc, store, _hub) = streaming_service("plan-select", vec![fallback.clone()]);
        let mut authors = Vec::new();
        for i in 0..12 {
            let keys = Keys::generate();
            let list = EventBuilder::new(Kind::RelayList, "")
                .tags([
                    Tag::parse(["r", "wss://shared.example"]).unwrap(),
                    Tag::parse(["r", "wss://shared2.example"]).unwrap(),
                    Tag::parse(["r", &format!("wss://own{i}.example")]).unwrap(),
                ])
                .sign_with_keys(&keys)
                .unwrap();
            store.publish(list).await.unwrap();
            authors.push(keys.public_key());
        }
        let nobody = Keys::generate().public_key();
        let mut all = authors.clone();
        all.push(nobody);

        let plan = svc.plan(Direction::Read, &all).await;
        let internet: Vec<&str> = plan.lanes.iter().filter_map(|l| l.url()).collect();
        assert!(internet.contains(&"wss://shared.example"));
        assert!(internet.contains(&"wss://shared2.example"));
        assert!(
            internet.contains(&fallback.as_str()),
            "the fallback was dropped"
        );
        assert_eq!(internet.len(), 3, "{internet:?}");
        assert_eq!(plan.missing_authors, vec![nobody]);

        let write = svc.plan(Direction::Write, &authors).await;
        assert_eq!(write.lanes.len(), 1 + 2 + 12, "inbox delivery was narrowed");
    }

    /// An Internet lane whose name resolves to a private address is not
    /// dialled: `validate_relay_url` judges the host as written, and a public
    /// name pointing at `127.0.0.1` — the ungated loopback relay — or the LAN
    /// is caught here, at the dial. Tests that mean to dial a mock on
    /// loopback opt out with `allowing_private_dials`.
    #[tokio::test]
    async fn an_internet_lane_that_resolves_private_is_not_dialled() {
        let content = scratch_content("private-dial");
        let svc = OutboxService::new(
            content.relay(),
            Arc::new(Mutex::new(None)),
            content,
            "npub1me".to_string(),
        );
        let keys = Keys::generate();
        let note = EventBuilder::text_note("stays home")
            .sign_with_keys(&keys)
            .unwrap();
        let filters = [Filter::new().kind(Kind::TextNote)];

        for url in ["ws://localhost:1", "ws://127.0.0.1:1", "ws://[::1]:1"] {
            let lane = RelayLane::Internet {
                url: url.to_string(),
            };
            let started = std::time::Instant::now();
            assert!(
                svc.query_lane(&lane, &filters, Duration::from_secs(10))
                    .await
                    .is_none(),
                "{url} was queried"
            );
            assert!(
                !svc.publish_lane(&lane, &note, Duration::from_secs(10))
                    .await,
                "{url} was published to"
            );
            // Refused at the resolve, not by a connect that failed or timed
            // out: nothing near the lane timeout was spent.
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "{url} took {:?} — it was dialled",
                started.elapsed()
            );
        }

        // The predicate itself, on what a lookup hands back.
        assert!(!dials_public("ws://user@relay.example").await);
        assert!(!dials_public("not a url").await);
        assert!(!dials_public("ws://[::ffff:127.0.0.1]:4870").await);
        assert!(!dials_public("ws://100.64.0.1").await);
    }

    /// A napplet's query through another relay keeps the profiles in the
    /// answer here — and only those: a note is answered and cached, not kept, and a
    /// forged profile is dropped where it came in and never reaches the store.
    #[tokio::test]
    async fn a_query_keeps_verified_profiles_and_nothing_else() {
        use myco_napplet_runtime::seams::LaneTransport;

        let keys = Keys::generate();
        let profile = EventBuilder::metadata(&nostr::Metadata::new().name("alice"))
            .sign_with_keys(&keys)
            .unwrap();
        let note = EventBuilder::text_note("cached, not kept")
            .sign_with_keys(&keys)
            .unwrap();
        let mut forged = serde_json::to_value(
            EventBuilder::metadata(&nostr::Metadata::new().name("real"))
                .sign_with_keys(&Keys::generate())
                .unwrap(),
        )
        .unwrap();
        forged["content"] = serde_json::json!(r#"{"name":"forged"}"#);
        let forged: Event = serde_json::from_value(forged).unwrap();
        assert!(forged.verify().is_err());

        let (url, _) = crate::ip_source::tests::mock_relay_holding(vec![
            profile.clone(),
            note.clone(),
            forged,
        ])
        .await;
        let content = scratch_content("keep-seen");
        let store = content.relay();
        let relay = content.relay_store().unwrap();
        let cache = content.event_cache();
        let svc = OutboxService::new(
            store.clone(),
            Arc::new(Mutex::new(None)),
            content,
            "npub1me".to_string(),
        )
        .allowing_private_dials();

        let answers = svc
            .query(
                &[RelayLane::Local, RelayLane::Internet { url }],
                &[Filter::new()],
                Duration::from_secs(5),
            )
            .await;
        let answered: usize = answers
            .iter()
            .filter_map(|(_, events)| events.as_ref())
            .map(Vec::len)
            .sum();
        assert_eq!(answered, 2, "the profile and the note, not the forgery");

        // Kept behind the answer, so wait for the write to land.
        for _ in 0..100 {
            if relay.count() > 0 && cache.contains(&note.id.to_bytes()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let kept = relay.query(&[Filter::new()]).await.unwrap();
        assert_eq!(
            kept.iter().map(|e| e.id).collect::<Vec<_>>(),
            [profile.id],
            "only the verified profile is kept"
        );
        // The note is remembered in the cache; the forgery nowhere.
        let seen = store.query(&[Filter::new()]).await.unwrap();
        assert_eq!(
            seen.len(),
            2,
            "the merged view is not the profile and the note"
        );
        assert!(cache.contains(&note.id.to_bytes()));
    }
}
