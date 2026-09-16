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
    Direction, LaneTransport, OutboxResolver, PlanSource, RelayLane, RelayPlan,
};

use crate::content::Content;
use crate::mesh_relay::RelayHub;

/// How long a subscription's remote pull waits for its lanes.
const PULL_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a plan waits for a missing relay list before falling back. Paid
/// once per unknown author, on the first call that names them.
const LIST_FETCH_TIMEOUT: Duration = Duration::from_secs(4);

/// A stored relay list older than this is used now and refreshed behind the
/// answer. NIP-65 lists change rarely; a day keeps a phone that was offline
/// for a week from serving a week-old plan forever.
const LIST_FRESH_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a miss is remembered. An author with no list anywhere is not
/// asked about again on every call, and is asked again soon enough that a
/// list published today is found today.
const MISS_REMEMBERED_FOR: Duration = Duration::from_secs(10 * 60);

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
    /// Authors asked about and not found, with when. The local relay is the
    /// positive cache; this is the negative one.
    misses: Arc<Mutex<std::collections::HashMap<PublicKey, std::time::Instant>>>,
    /// Authors whose stale list is being refreshed right now, so a burst of
    /// calls spawns one fetch rather than one per call.
    refreshing: Arc<Mutex<std::collections::HashSet<PublicKey>>>,
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
            store,
            hub,
            content,
            own_npub,
            configured: crate::ip_source::default_relays(),
            misses: Arc::new(Mutex::new(std::collections::HashMap::new())),
            refreshing: Arc::new(Mutex::new(std::collections::HashSet::new())),
        }
    }

    /// Use `relays` instead of the default public set — for tests, which
    /// must never reach the internet.
    #[cfg(test)]
    pub fn with_configured_relays(mut self, relays: Vec<String>) -> Self {
        self.configured = relays;
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
            misses: self.misses.clone(),
            refreshing: self.refreshing.clone(),
        })
    }

    /// Where a relay list might be found: the configured relays unless
    /// offline-only, and every Circle member's mesh relay — the people around
    /// the user are exactly who would have seen a friend's list.
    fn list_lanes(&self) -> Vec<RelayLane> {
        let mut lanes = self.fallback_lanes();
        for npub in self.content.circle_npubs() {
            if npub != self.own_npub {
                lanes.push(RelayLane::Mesh {
                    url: crate::ip_source::mesh_relay_url(&npub),
                });
            }
        }
        lanes
    }

    /// Ask the pool for `author`'s newest relay list and store it. Returns
    /// the list, or `None` when nobody had one within the bound.
    async fn fetch_relay_list(&self, author: &PublicKey) -> Option<Event> {
        let lanes = self.list_lanes();
        if lanes.is_empty() {
            return None;
        }
        let filter = Filter::new().kind(Kind::RelayList).author(*author).limit(1);
        let answers = self
            .query(&lanes, std::slice::from_ref(&filter), LIST_FETCH_TIMEOUT)
            .await;
        let newest = answers
            .into_iter()
            .filter_map(|(_, events)| events)
            .flatten()
            .filter(|e| e.kind == Kind::RelayList && e.pubkey == *author)
            .max_by_key(|e| e.created_at)?;
        // The local relay is the cache: a replaceable kind keeps the newest.
        if let Err(e) = self.store.publish(newest.clone()).await {
            tracing::debug!(error = %e, "outbox: could not cache a relay list");
        }
        Some(newest)
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

    /// The author's NIP-65 relays for `direction`: from the local store when
    /// it has a list, from the pool when it does not — stored on the way in,
    /// so the second call is local.
    async fn nip65_lanes(&self, author: &PublicKey, direction: Direction) -> Listed {
        let filter = Filter::new().kind(Kind::RelayList).author(*author).limit(1);
        let stored = self
            .store
            .query(&[filter])
            .await
            .ok()
            .and_then(|events| events.into_iter().max_by_key(|e| e.created_at));

        if let Some(list) = stored {
            let age = Duration::from_secs(
                nostr::Timestamp::now()
                    .as_secs()
                    .saturating_sub(list.created_at.as_secs()),
            );
            let lanes = relay_list_lanes(&list, direction);
            if age <= LIST_FRESH_FOR {
                return Listed::Fresh(lanes);
            }
            // Old enough to check, not too old to use. The napplet gets the
            // stored plan now; the next call gets whatever the refresh found.
            if self.refreshing.lock().unwrap().insert(*author) {
                let this = self.detached();
                let author = *author;
                tokio::spawn(async move {
                    let _ = this.fetch_relay_list(&author).await;
                    this.refreshing.lock().unwrap().remove(&author);
                });
            }
            return Listed::Stale(lanes);
        }

        let recently_missed = self
            .misses
            .lock()
            .unwrap()
            .get(author)
            .is_some_and(|at| at.elapsed() < MISS_REMEMBERED_FOR);
        if recently_missed {
            return Listed::Missing;
        }
        match self.fetch_relay_list(author).await {
            Some(list) => Listed::Fresh(relay_list_lanes(&list, direction)),
            None => {
                self.misses
                    .lock()
                    .unwrap()
                    .insert(*author, std::time::Instant::now());
                Listed::Missing
            }
        }
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
                // One connection, one REQ carrying every filter; verified at
                // ingress by `query_relay_filters`.
                let values: Vec<serde_json::Value> = filters
                    .iter()
                    .filter_map(|f| serde_json::to_value(f).ok())
                    .collect();
                match tokio::time::timeout(
                    timeout,
                    crate::ip_source::query_relay_filters(url, values),
                )
                .await
                {
                    Ok(Ok(events)) => Some(events),
                    Ok(Err(e)) => {
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
            RelayLane::Internet { url } => matches!(
                tokio::time::timeout(timeout, crate::ip_source::publish_to_relay(url, event)).await,
                Ok(Ok(true))
            ),
        }
    }
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
        let lanes = self.fallback_lanes();
        if lanes.is_empty() {
            return Ok(());
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
        Ok(())
    }
}

#[async_trait::async_trait]
impl OutboxResolver for OutboxService {
    async fn plan(&self, direction: Direction, authors: &[PublicKey]) -> RelayPlan {
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
        for author in authors {
            match self.nip65_lanes(author, direction).await {
                Listed::Fresh(listed) => {
                    lanes.extend(listed.into_iter().filter(|l| self.allowed(l)))
                }
                Listed::Stale(listed) => {
                    any_stale = true;
                    lanes.extend(listed.into_iter().filter(|l| self.allowed(l)))
                }
                Listed::Missing => {
                    missing.push(*author);
                    lanes.extend(self.fallback_lanes());
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        lanes.retain(|l| seen.insert(l.clone()));
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

#[async_trait::async_trait]
impl LaneTransport for OutboxService {
    async fn query(
        &self,
        lanes: &[RelayLane],
        filters: &[Filter],
        timeout: Duration,
    ) -> Vec<(RelayLane, Option<Vec<Event>>)> {
        let tried_internet = lanes
            .iter()
            .any(|l| matches!(l, RelayLane::Internet { .. }) && self.allowed(l));
        let out: Vec<(RelayLane, Option<Vec<Event>>)> =
            join_all(lanes.iter().map(|lane| async move {
                (lane.clone(), self.query_lane(lane, filters, timeout).await)
            }))
            .await;
        let any_ok = out
            .iter()
            .any(|(l, r)| matches!(l, RelayLane::Internet { .. }) && r.is_some());
        self.content.note_internet_round(any_ok, tried_internet);
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

    async fn publish(
        &self,
        lanes: &[RelayLane],
        event: &Event,
        timeout: Duration,
    ) -> Vec<(RelayLane, bool)> {
        let tried_internet = lanes
            .iter()
            .any(|l| matches!(l, RelayLane::Internet { .. }) && self.allowed(l));
        let out: Vec<(RelayLane, bool)> = join_all(lanes.iter().map(|lane| async move {
            (lane.clone(), self.publish_lane(lane, event, timeout).await)
        }))
        .await;
        let any_ok = out
            .iter()
            .any(|(l, ok)| matches!(l, RelayLane::Internet { .. }) && *ok);
        self.content.note_internet_round(any_ok, tried_internet);
        out
    }

    async fn pull_into_local(&self, lanes: &[RelayLane], filters: &[Filter]) -> anyhow::Result<()> {
        let hub = self.hub.lock().unwrap().clone();
        let Some(hub) = hub else {
            // Nothing to deliver through.
            return Ok(());
        };
        let this = self.detached();
        let lanes = lanes.to_vec();
        let filters = filters.to_vec();
        tokio::spawn(async move {
            let answers = this.query(&lanes, &filters, PULL_TIMEOUT).await;
            let mut fresh = 0usize;
            for (_, events) in answers {
                for event in events.unwrap_or_default() {
                    if let Ok(true) = hub.accept_unforwarded(event).await {
                        fresh += 1;
                    }
                }
            }
            tracing::debug!(fresh, "outbox pull finished");
        });
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
            match myco_napplet_runtime::nap::outbox::validate_relay_url(url) {
                Ok(lane) => Some(lane),
                Err(reason) => {
                    tracing::debug!(url, reason, "outbox: relay list names a URL we refuse");
                    None
                }
            }
        })
        .collect()
}

/// The user's own kind 10002: the configured internet relays, so the user's
/// own outbox plan resolves as NIP-65 rather than fallback.
///
/// Deliberately **not** this device's mesh relay. `ws://<device-npub>.fips`
/// names the device key, and this event is signed by the user key: putting the
/// one inside the other would publish the link D3 exists to avoid — the social
/// identity tied to the hardware, in a signed event anyone could keep. The
/// people who can reach this device over the mesh are Circle members, and they
/// already reach its relay by policy (`OutboxService::allowed`), which needs no
/// tag to say so.
pub fn own_relay_list(keys: &nostr::Keys) -> anyhow::Result<Event> {
    let mut tags = Vec::new();
    for url in crate::ip_source::default_relays() {
        tags.push(nostr::Tag::parse(["r".to_string(), url])?);
    }
    Ok(nostr::EventBuilder::new(Kind::RelayList, "")
        .tags(tags)
        .sign_with_keys(keys)?)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        );

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
        assert!(svc
            .misses
            .lock()
            .unwrap()
            .contains_key(&nobody.public_key()));
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
        );

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

    /// One round where every internet lane failed trips a breaker: the next
    /// round skips the internet at once instead of paying the timeouts
    /// again, and reports the lane as unreached so the answer says
    /// `incomplete`.
    #[tokio::test]
    async fn a_dead_internet_trips_the_breaker_for_the_next_round() {
        let dir = std::env::temp_dir().join(format!("myco-outbox-breaker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());
        let svc = OutboxService::new(
            content.relay(),
            Arc::new(Mutex::new(None)),
            content.clone(),
            "npub1me".to_string(),
        );
        let dead = RelayLane::Internet {
            url: "ws://127.0.0.1:1".to_string(),
        };
        let filters = [Filter::new().kind(Kind::TextNote)];

        let first = svc
            .query(
                std::slice::from_ref(&dead),
                &filters,
                Duration::from_secs(2),
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
}
