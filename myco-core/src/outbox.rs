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

pub struct OutboxService {
    store: Arc<dyn RelayBackend>,
    hub: Arc<Mutex<Option<Arc<RelayHub>>>>,
    content: Arc<Content>,
    /// This device's mesh npub, so its own `.fips` relay is recognised and
    /// never dialled.
    own_npub: String,
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
        }
    }

    /// The configured relays as lanes, or nothing when offline only.
    fn fallback_lanes(&self) -> Vec<RelayLane> {
        if self.content.is_offline_only() {
            return Vec::new();
        }
        crate::ip_source::default_relays()
            .into_iter()
            .map(|url| RelayLane::Internet { url })
            .collect()
    }

    /// Whether a lane may be used from this device, per the policy above.
    fn allowed(&self, lane: &RelayLane) -> bool {
        match lane {
            RelayLane::Local => true,
            RelayLane::Internet { .. } => !self.content.is_offline_only(),
            RelayLane::Mesh { url } => match mesh_relay_npub(url) {
                Some(npub) => npub != self.own_npub && self.content.circle_npubs().contains(&npub),
                None => false,
            },
        }
    }

    /// The author's NIP-65 relays for `direction`, if they have published a
    /// list this device has stored.
    async fn nip65_lanes(
        &self,
        author: &PublicKey,
        direction: Direction,
    ) -> Option<Vec<RelayLane>> {
        let filter = Filter::new().kind(Kind::RelayList).author(*author).limit(1);
        let newest = self
            .store
            .query(&[filter])
            .await
            .ok()?
            .into_iter()
            .max_by_key(|e| e.created_at)?;
        Some(relay_list_lanes(&newest, direction))
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
                let npub = mesh_relay_npub(url)?;
                let raw: Vec<serde_json::Value> = filters
                    .iter()
                    .filter_map(|f| serde_json::to_value(f).ok())
                    .collect();
                let events = self
                    .content
                    .peer_relays()
                    .request(&npub, url, raw, timeout)
                    .await;
                // The pool returns events as received; the caller verifies.
                Some(events.into_iter().filter(|e| e.verify().is_ok()).collect())
            }
            RelayLane::Internet { url } => {
                // `query_relay` takes one filter and verifies at ingress.
                let mut out = Vec::new();
                for filter in filters {
                    let value = serde_json::to_value(filter).ok()?;
                    match tokio::time::timeout(timeout, crate::ip_source::query_relay(url, value))
                        .await
                    {
                        Ok(Ok(events)) => out.extend(events),
                        Ok(Err(e)) => {
                            tracing::debug!(url, error = %e, "outbox: relay query failed");
                            return None;
                        }
                        Err(_) => {
                            tracing::debug!(url, "outbox: relay query timed out");
                            return None;
                        }
                    }
                }
                Some(out)
            }
        }
    }

    async fn publish_lane(&self, lane: &RelayLane, event: &Event, timeout: Duration) -> bool {
        if !self.allowed(lane) {
            return false;
        }
        match lane {
            RelayLane::Local => {
                // Cloned out rather than held: the lock must not span the await.
                let hub = self.hub.lock().unwrap().clone();
                match hub {
                    Some(hub) => hub.accept_unforwarded(event.clone()).await.is_ok(),
                    None => self.store.publish(event.clone()).await.is_ok(),
                }
            }
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
                pool.send(&npub, url, frame);
                true
            }
            RelayLane::Internet { url } => matches!(
                tokio::time::timeout(timeout, crate::ip_source::publish_to_relay(url, event)).await,
                Ok(Ok(true))
            ),
        }
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
        for author in authors {
            match self.nip65_lanes(author, direction).await {
                Some(listed) => lanes.extend(listed.into_iter().filter(|l| self.allowed(l))),
                None => {
                    missing.push(*author);
                    lanes.extend(self.fallback_lanes());
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        lanes.retain(|l| seen.insert(l.clone()));
        RelayPlan {
            lanes,
            source: if missing.is_empty() {
                PlanSource::Nip65
            } else {
                PlanSource::Fallback
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
        join_all(lanes.iter().map(|lane| async move {
            (lane.clone(), self.query_lane(lane, filters, timeout).await)
        }))
        .await
    }

    async fn publish(
        &self,
        lanes: &[RelayLane],
        event: &Event,
        timeout: Duration,
    ) -> Vec<(RelayLane, bool)> {
        join_all(lanes.iter().map(|lane| async move {
            (lane.clone(), self.publish_lane(lane, event, timeout).await)
        }))
        .await
    }

    async fn pull_into_local(&self, lanes: &[RelayLane], filters: &[Filter]) -> anyhow::Result<()> {
        let hub = self.hub.lock().unwrap().clone();
        let Some(hub) = hub else {
            // Nothing to deliver through.
            return Ok(());
        };
        let this = Arc::new(Self {
            store: self.store.clone(),
            hub: self.hub.clone(),
            content: self.content.clone(),
            own_npub: self.own_npub.clone(),
        });
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

/// The npub in a mesh relay URL, `ws://<npub>.fips:4870`.
pub(crate) fn mesh_relay_npub(url: &str) -> Option<String> {
    let rest = url.strip_prefix("ws://")?;
    let host = rest.split(['/', ':']).next()?;
    let npub = host.strip_suffix(".fips")?;
    if npub.starts_with("npub1") {
        Some(npub.to_string())
    } else {
        None
    }
}

/// The lanes a kind 10002 names for `direction`: a `["r", url]` tag with no
/// marker is both, `read` and `write` markers are one each. NIP-65's marker
/// is from the author's point of view, so what *we* read from is what they
/// marked `write`, and vice versa.
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
                None => Some(RelayLane::from_url(url)),
                Some(marker) if marker == wanted => Some(RelayLane::from_url(url)),
                Some(_) => None,
            }
        })
        .collect()
}

/// The user's own kind 10002: their mesh relay for the people around them,
/// and the configured relays for everyone else. Published with the guest
/// profile on first napplet use, so peers can route back (design §7.4) and
/// the user's own outbox plan resolves as NIP-65 rather than fallback.
pub fn own_relay_list(keys: &nostr::Keys, own_npub: &str) -> anyhow::Result<Event> {
    let mut tags = vec![nostr::Tag::parse([
        "r".to_string(),
        crate::ip_source::mesh_relay_url(own_npub),
    ])?];
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

    #[test]
    fn the_own_relay_list_names_the_mesh_relay_first() {
        let keys = Keys::generate();
        let list = own_relay_list(&keys, "npub1me").unwrap();
        assert_eq!(list.kind, Kind::RelayList);
        assert!(list.verify().is_ok());
        let lanes = relay_list_lanes(&list, Direction::Read);
        assert_eq!(
            lanes[0],
            RelayLane::Mesh {
                url: "ws://npub1me.fips:4870".into()
            }
        );
        assert!(lanes.len() > 1);
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
        let svc = OutboxService::new(
            store.clone(),
            Arc::new(Mutex::new(None)),
            content.clone(),
            "npub1me".to_string(),
        );

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
        assert!(
            plan.lanes.len() > 1,
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
}
