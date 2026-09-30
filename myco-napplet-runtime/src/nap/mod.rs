//! One module per NAP capability domain.
//!
//! A NAP is one capability contract: `NAP-SHELL` is the handshake, `NAP-RELAY`
//! proxies relay reads and writes, `NAP-INTENT` opens another napplet by role.
//! Each is transport-neutral in the registry; what lands here is the runtime
//! half of the web projection, reached through [`crate::dispatch`].
//!
//! `NAP-MESH` is Myco's own — the one capability with no standard equivalent,
//! specified in the registry's form so it can be proposed there.

pub mod identity;
pub mod inc;
pub mod intent;
pub mod link;
pub mod local;
pub mod mesh;
pub mod outbox;
pub mod relay;
pub mod resource;
pub mod shell;
pub mod theme;

use nostr::{Event, Filter};

use crate::dispatch::NapContext;
use crate::seams::Envelope;
use crate::session::Session;

/// When `relay.query` and `outbox.query` answer: **with the first events
/// anyone has**, never later. They have one result frame, so whatever is not
/// in it is not in it — but a napplet waiting on a relay to maybe bring a
/// newer version is a napplet showing nothing. See
/// [`crate::seams::EarlyAnswer`].
///
/// - `local_cap` zero: when this device holds events, they are the answer,
///   at once. The answer says `incomplete` where the wire can
///   (`outbox.query`).
/// - `grace` zero: when this device has nothing, the first relay to return
///   events answers.
///
/// The lanes left out keep going behind the answer, and what they find —
/// a newer profile or follow list, say — is kept here, where the store keeps
/// the newest per slot. The next read has it, and a live subscription is
/// shown it as it lands (see `LaneTransport::query_early`). A napplet that
/// wants the newer version in the same view subscribes rather than queries.
pub(crate) const QUERY_EARLY: crate::seams::EarlyAnswer = crate::seams::EarlyAnswer {
    grace: std::time::Duration::ZERO,
    local_cap: std::time::Duration::ZERO,
};

/// `events` with only the newest of each replaceable (per kind and author)
/// and addressable (per kind, author and `d`) — what one NIP-01 relay would
/// return. Lanes answer from different stores, and this device's may hold a
/// profile the relay has since replaced; a napplet shown both would have to
/// know to pick. Regular events pass through, one per id. Newest first.
pub(crate) fn newest_per_slot(events: impl IntoIterator<Item = Event>) -> Vec<Event> {
    use std::collections::HashMap;
    let mut slots: HashMap<(u16, nostr::PublicKey, String), Event> = HashMap::new();
    let mut regular: HashMap<nostr::EventId, Event> = HashMap::new();
    for event in events {
        let d = if event.kind.is_addressable() {
            Some(event.tags.identifier().unwrap_or_default().to_string())
        } else if event.kind.is_replaceable() {
            Some(String::new())
        } else {
            None
        };
        match d {
            Some(d) => {
                let key = (event.kind.as_u16(), event.pubkey, d);
                let newer = slots.get(&key).is_none_or(|held| {
                    (event.created_at, std::cmp::Reverse(event.id))
                        > (held.created_at, std::cmp::Reverse(held.id))
                });
                if newer {
                    slots.insert(key, event);
                }
            }
            None => {
                regular.entry(event.id).or_insert(event);
            }
        }
    }
    let mut out: Vec<Event> = slots.into_values().chain(regular.into_values()).collect();
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
    out
}

/// The domains that keep live subscriptions, each delivering `<domain>.event`.
const SUBSCRIBING_DOMAINS: [&str; 3] = ["relay", "mesh", "outbox"];

/// Every frame a session should receive for an arriving event, across the
/// domains that subscribe: `relay.event`, `mesh.event` and `outbox.event`.
///
/// Called for every event this device accepts — its own publishes and anything
/// carried here from a peer — so a subscription behaves the same whichever
/// side of the mesh an event came from. Empty when nothing matches, which is
/// the common case and deliberately cheap.
pub fn deliveries_for(session: &Session, event: &Event) -> Vec<Envelope> {
    let frames: Vec<Envelope> = SUBSCRIBING_DOMAINS
        .iter()
        .flat_map(|domain| deliveries_in(session, domain, event))
        .collect();
    if !frames.is_empty() {
        session
            .ledger()
            .lock()
            .unwrap()
            .record_event(&event.id.to_bytes());
    }
    frames
}

/// The `<domain>.event` frames a session should receive for `event` in one
/// domain: one per matching subscription, gated on the session being
/// established and still granted the domain (see
/// [`Session::matching_subscriptions_in`]).
pub(crate) fn deliveries_in(session: &Session, domain: &str, event: &Event) -> Vec<Envelope> {
    session
        .matching_subscriptions_in(domain, event)
        .into_iter()
        .map(|sub_id| event_frame(domain, sub_id, event))
        .collect()
}

/// One `<domain>.event` frame: the spec's `RelayEventResult`, `{ event }`.
pub(crate) fn event_frame(domain: &str, sub_id: impl Into<String>, event: &Event) -> Envelope {
    Envelope::new(format!("{domain}.event"))
        .with_field("subId", sub_id.into())
        .with_field("result", relay::result_of(event))
}

/// The first half every subscribing domain shares: register the filters,
/// then answer what the local relay already holds.
///
/// Registered **before** the read, so an event landing between the two is
/// delivered by the live path rather than falling through the gap. A napplet
/// may see it twice; Nostr subscriptions are at-least-once and a duplicate id
/// is something every client already handles, whereas a missed event is
/// invisible. Returns the backlog frames, or the reason the subscription
/// closed before it started — each domain wraps that in its own `.closed`.
///
/// A subscription that closes before it started is not left registered: on
/// a failed backlog the filters are removed again, so the napplet — which
/// drops the id on `.closed` — is not matched and emitted for by a runtime
/// that kept it. The session's cap ([`crate::session::MAX_SUBSCRIPTIONS`])
/// is a reason too.
///
/// What follows differs per domain — which relays are pulled behind the
/// backlog, and whether an `eose` marks its end — and stays with the domain.
pub(crate) async fn open_subscription(
    ctx: &NapContext,
    session: &mut Session,
    domain: &str,
    sub_id: &str,
    filters: Vec<Filter>,
) -> Result<Vec<Envelope>, String> {
    session.subscribe_in(domain, sub_id, filters.clone())?;
    let events = match ctx.relay.query(&filters).await {
        Ok(events) => events,
        Err(e) => {
            session.unsubscribe_in(domain, sub_id);
            return Err(format!("query failed: {e}"));
        }
    };
    Ok(events
        .iter()
        .map(|event| event_frame(domain, sub_id, event))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::dispatch;
    use crate::session::{NappletIdentity, MAX_SUBSCRIPTIONS};
    use crate::testing::test_context;
    use serde_json::json;

    fn granted() -> Session {
        let mut s = Session::new(NappletIdentity::new("chat", "aggregate"), ["relay"]);
        s.on_ready();
        s
    }

    async fn subscribe(ctx: &NapContext, s: &mut Session, sub_id: &str) -> Vec<Envelope> {
        let e = Envelope::new("relay.subscribe")
            .with_id(sub_id)
            .with_field("subId", sub_id)
            .with_field("filters", json!({"kinds": [1]}));
        dispatch(ctx, s, &e).await.envelopes().to_vec()
    }

    /// A napplet looping over fresh ids gets `.closed` at the cap, not a
    /// runtime matching thousands of filter sets on every event. Re-using an
    /// id it already holds is fine at any count.
    #[tokio::test]
    async fn subscriptions_are_capped_per_session() {
        let (ctx, _signer) = test_context();
        let mut s = granted();
        for i in 0..MAX_SUBSCRIPTIONS {
            let out = subscribe(&ctx, &mut s, &format!("sub-{i}")).await;
            assert!(
                out.iter().all(|e| e.msg_type != "relay.closed"),
                "subscription {i} was refused under the cap"
            );
        }
        assert_eq!(s.subscription_count(), MAX_SUBSCRIPTIONS);

        let out = subscribe(&ctx, &mut s, "one-too-many").await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].msg_type, "relay.closed");
        assert_eq!(out[0].field("subId").unwrap(), "one-too-many");
        assert!(out[0]
            .field("reason")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("too many live subscriptions"));
        assert_eq!(s.subscription_count(), MAX_SUBSCRIPTIONS);

        // Replacing one already registered is not a new one.
        let out = subscribe(&ctx, &mut s, "sub-3").await;
        assert!(out.iter().any(|e| e.msg_type == "relay.eose"));
        assert_eq!(s.subscription_count(), MAX_SUBSCRIPTIONS);

        // And closing one makes room.
        let close = Envelope::new("relay.close")
            .with_id("c")
            .with_field("subId", "sub-0");
        let _ = dispatch(&ctx, &mut s, &close).await;
        let out = subscribe(&ctx, &mut s, "one-too-many").await;
        assert!(out.iter().any(|e| e.msg_type == "relay.eose"));
        assert_eq!(s.subscription_count(), MAX_SUBSCRIPTIONS);
    }

    /// A relay that cannot answer the backlog.
    struct FailingRelay;

    #[async_trait::async_trait]
    impl nsite_deck::seams::RelayBackend for FailingRelay {
        async fn publish(&self, _event: Event) -> anyhow::Result<()> {
            Ok(())
        }
        async fn query(&self, _filters: &[Filter]) -> anyhow::Result<Vec<Event>> {
            Err(anyhow::anyhow!("store is closed"))
        }
    }

    /// A backlog that fails closes the subscription — and *removes* it. The
    /// napplet drops the id on `.closed`; the runtime must not keep matching
    /// and emitting for it.
    #[tokio::test]
    async fn a_failed_backlog_leaves_no_subscription() {
        let (mut ctx, _signer) = test_context();
        ctx.relay = std::sync::Arc::new(FailingRelay);
        let mut s = granted();

        let out = subscribe(&ctx, &mut s, "feed").await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].msg_type, "relay.closed");
        assert_eq!(out[0].field("subId").unwrap(), "feed");
        assert!(out[0]
            .field("reason")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("query failed"));
        assert_eq!(
            s.subscription_count(),
            0,
            "the failed subscription stayed registered"
        );
    }

    /// Lanes answer from different stores: only the newest of each
    /// replaceable and addressable slot reaches the napplet, and a regular
    /// event twice is once.
    #[test]
    fn only_the_newest_of_a_replaceable_is_returned() {
        use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
        let keys = Keys::generate();
        let at = |secs: u64, b: EventBuilder| {
            b.custom_created_at(Timestamp::from(secs))
                .sign_with_keys(&keys)
                .unwrap()
        };
        let old_profile = at(100, EventBuilder::new(Kind::Metadata, "{}"));
        let new_profile = at(200, EventBuilder::new(Kind::Metadata, "{}"));
        let d = |v: &str| Tag::parse(["d", v]).unwrap();
        let app_a_old = at(
            100,
            EventBuilder::new(Kind::from(35129u16), "").tags([d("a")]),
        );
        let app_a_new = at(
            300,
            EventBuilder::new(Kind::from(35129u16), "").tags([d("a")]),
        );
        let app_b = at(
            150,
            EventBuilder::new(Kind::from(35129u16), "").tags([d("b")]),
        );
        let note = at(50, EventBuilder::text_note("hi"));

        let out = newest_per_slot([
            old_profile,
            new_profile.clone(),
            app_a_new.clone(),
            app_a_old,
            app_b.clone(),
            note.clone(),
            note.clone(),
        ]);
        let ids: Vec<_> = out.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![app_a_new.id, new_profile.id, app_b.id, note.id]);
    }
}
