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
pub mod upload;

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
///
/// Each subscription is handed an event once: one its backlog already
/// delivered, or that arrived twice, is skipped.
pub(crate) fn deliveries_in(session: &Session, domain: &str, event: &Event) -> Vec<Envelope> {
    session
        .matching_subscriptions_in(domain, event)
        .into_iter()
        .filter(|sub_id| session.first_sight(domain, sub_id, &event.id))
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
/// then deliver what this device already holds — and after it, `after`
/// (an `eose`, where the domain has one).
///
/// **Streamed** when the host gave the session a [`Streamer`]: the call
/// answers at once with nothing, and a task reads the store and pushes each
/// matching event to the napplet the moment the read returns it, then
/// `after`. Nostr is a stream; nothing here waits for a batch, a count or a
/// time. A session without a streamer (a host that does not stream, tests)
/// gets the backlog and `after` in the reply, as before.
///
/// Registered **before** the read, so an event landing between the two is
/// delivered by the live path rather than falling through the gap. Each
/// subscription is handed an event once ([`Session::first_sight`]): the
/// backlog and the live path share one set, so neither repeats the other.
///
/// A subscription that closes before it started is not left registered: on
/// a failed backlog read answered inline the filters are removed again, so
/// the napplet — which drops the id on `.closed` — is not matched and
/// emitted for by a runtime that kept it. The session's cap
/// ([`crate::session::MAX_SUBSCRIPTIONS`]) is a reason too. A streamed read
/// that fails pushes `<domain>.closed` and ends the subscription's
/// generation: nothing more is delivered for it, and its registration is
/// dropped at the next subscribe.
///
/// The backlog task delivers through its own generation
/// ([`crate::session::SubscriptionHandle`]), never by `subId`: a subscription
/// closed, or replaced under the same id, while its backlog is still being
/// read gets nothing more from it — no old event, no stray `eose`.
///
/// What follows differs per domain — which relays are pulled behind the
/// backlog — and stays with the domain.
///
/// [`Streamer`]: crate::session::Streamer
pub(crate) async fn open_subscription(
    ctx: &NapContext,
    session: &mut Session,
    domain: &str,
    sub_id: &str,
    filters: &[Filter],
    after: Vec<Envelope>,
) -> Result<Vec<Envelope>, String> {
    session.subscribe_in(domain, sub_id, filters.to_vec())?;
    let handle = session.subscription_handle(domain, sub_id);

    if let Some(streamer) = session.streamer().cloned() {
        let relay = ctx.relay.clone();
        let filters = filters.to_vec();
        let domain = domain.to_string();
        let sub_id = sub_id.to_string();
        (streamer.spawn)(Box::pin(async move {
            match relay.query(&filters).await {
                Ok(events) => {
                    for event in &events {
                        if handle.is_closed() {
                            return;
                        }
                        // Checked, recorded and pushed under the generation's
                        // lock: a close or replace cannot slip in between.
                        handle.deliver(&event.id, || {
                            (streamer.push)(event_frame(&domain, sub_id.as_str(), event))
                        });
                    }
                    handle.while_open(|| {
                        for frame in after {
                            (streamer.push)(frame);
                        }
                    });
                }
                Err(e) => {
                    // The napplet drops the id on `.closed`; the generation
                    // ends with it, so nothing more is delivered and the
                    // registration goes at the next subscribe.
                    handle.while_open(|| {
                        (streamer.push)(
                            Envelope::new(format!("{domain}.closed"))
                                .with_field("subId", sub_id.as_str())
                                .with_field("reason", format!("query failed: {e}")),
                        )
                    });
                    handle.close();
                }
            }
        }));
        return Ok(Vec::new());
    }

    let events = match ctx.relay.query(filters).await {
        Ok(events) => events,
        Err(e) => {
            session.unsubscribe_in(domain, sub_id);
            return Err(format!("query failed: {e}"));
        }
    };
    let mut out: Vec<Envelope> = Vec::new();
    for event in &events {
        // Recorded as delivered by dispatch, with the rest of the reply.
        handle.deliver(&event.id, || out.push(event_frame(domain, sub_id, event)));
    }
    out.extend(after);
    Ok(out)
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

    /// A session whose host streams: frames pushed to `pushed`, work spawned
    /// on the test runtime.
    fn streaming(pushed: std::sync::Arc<std::sync::Mutex<Vec<Envelope>>>) -> Session {
        let push = pushed.clone();
        granted().with_streamer(crate::session::Streamer {
            push: std::sync::Arc::new(move |frame| push.lock().unwrap().push(frame)),
            spawn: std::sync::Arc::new(|work| {
                tokio::spawn(work);
            }),
        })
    }

    /// With a streamer, a subscription answers at once with nothing, then
    /// each stored event is pushed as it is read, and `eose` after them.
    #[tokio::test]
    async fn a_streamed_backlog_is_pushed_and_eose_follows_it() {
        let (ctx, _signer) = test_context();
        let keys = nostr::Keys::generate();
        let a = nostr::EventBuilder::text_note("a")
            .sign_with_keys(&keys)
            .unwrap();
        // Another author: the test relay keeps one event per author and kind.
        let b = nostr::EventBuilder::text_note("b")
            .sign_with_keys(&nostr::Keys::generate())
            .unwrap();
        ctx.relay.publish(a.clone()).await.unwrap();
        ctx.relay.publish(b.clone()).await.unwrap();

        let pushed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut s = streaming(pushed.clone());
        let out = subscribe(&ctx, &mut s, "feed").await;
        assert!(out.is_empty(), "the call answers at once: {out:?}");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while pushed.lock().unwrap().len() < 3 && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let frames = pushed.lock().unwrap().clone();
        let types: Vec<&str> = frames.iter().map(|f| f.msg_type.as_str()).collect();
        assert_eq!(types, ["relay.event", "relay.event", "relay.eose"]);
        // Pushed frames count as delivered (NAP-LOCAL).
        assert!(s.was_delivered(&a.id) && s.was_delivered(&b.id));

        // The same event arriving live is not handed to the subscription again;
        // a new one is.
        assert!(deliveries_for(&s, &a).is_empty());
        let c = nostr::EventBuilder::text_note("c")
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(deliveries_for(&s, &c).len(), 1);
        assert!(deliveries_for(&s, &c).is_empty());
    }

    /// A subscription replaced under the same id while its first backlog is
    /// still being read gets nothing from that first read — no old event, no
    /// stray `eose` — and a closed one gets nothing either. The backlog
    /// tasks here are held, then run only after the replace and the close.
    #[tokio::test]
    async fn a_replaced_or_closed_subscription_gets_nothing_from_its_old_backlog() {
        let (ctx, _signer) = test_context();
        let keys = nostr::Keys::generate();
        let old = nostr::EventBuilder::text_note("old")
            .sign_with_keys(&keys)
            .unwrap();
        ctx.relay.publish(old.clone()).await.unwrap();

        type Work = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;
        let held: std::sync::Arc<std::sync::Mutex<Vec<Work>>> = Default::default();
        let pushed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Envelope>::new()));
        let push = pushed.clone();
        let hold = held.clone();
        let mut s = granted().with_streamer(crate::session::Streamer {
            push: std::sync::Arc::new(move |frame| push.lock().unwrap().push(frame)),
            spawn: std::sync::Arc::new(move |work| hold.lock().unwrap().push(work)),
        });
        // A snapshot alive across the replace, as a read in flight holds one.
        let _snapshot = s.clone();

        subscribe(&ctx, &mut s, "feed").await;
        let first = held.lock().unwrap().pop().unwrap();
        // Replaced under the same id (another kind), then the new one closed.
        let e = Envelope::new("relay.subscribe")
            .with_id("again")
            .with_field("subId", "feed")
            .with_field("filters", serde_json::json!({"kinds": [30023]}));
        dispatch(&ctx, &mut s, &e).await;
        let second = held.lock().unwrap().pop().unwrap();
        s.unsubscribe("feed");

        first.await;
        second.await;
        assert!(
            pushed.lock().unwrap().is_empty(),
            "a stale backlog was delivered: {:?}",
            pushed.lock().unwrap()
        );
    }

    /// A streamed backlog read that fails says `.closed`, delivers nothing
    /// more, and frees its slot at the next subscribe.
    #[tokio::test]
    async fn a_failed_streamed_backlog_ends_its_subscription() {
        let (ctx, _signer) = test_context();
        let mut s = granted();
        s.subscribe_in(
            "relay",
            "feed",
            vec![nostr::Filter::new().kind(nostr::Kind::TextNote)],
        )
        .unwrap();
        let handle = s.subscription_handle("relay", "feed");
        handle.close();
        let keys = nostr::Keys::generate();
        let note = nostr::EventBuilder::text_note("hi")
            .sign_with_keys(&keys)
            .unwrap();
        assert!(deliveries_for(&s, &note).is_empty());
        assert_eq!(s.subscription_count(), 1);
        subscribe(&ctx, &mut s, "other").await;
        assert_eq!(
            s.subscription_count(),
            1,
            "the ended subscription kept its slot"
        );
    }

    /// Without a streamer the backlog and `eose` come in the reply, each
    /// event once, and a live repeat of one of them is skipped too.
    #[tokio::test]
    async fn an_inline_backlog_is_deduplicated_against_live_deliveries() {
        let (ctx, _signer) = test_context();
        let keys = nostr::Keys::generate();
        let a = nostr::EventBuilder::text_note("a")
            .sign_with_keys(&keys)
            .unwrap();
        ctx.relay.publish(a.clone()).await.unwrap();
        let mut s = granted();
        let out = subscribe(&ctx, &mut s, "feed").await;
        let types: Vec<&str> = out.iter().map(|f| f.msg_type.as_str()).collect();
        assert_eq!(types, ["relay.event", "relay.eose"]);
        assert!(deliveries_for(&s, &a).is_empty());
    }

    /// A closed subscription is handed nothing more, and a reopened one
    /// starts with an empty seen-set.
    #[tokio::test]
    async fn closing_forgets_what_a_subscription_saw() {
        let (ctx, _signer) = test_context();
        let keys = nostr::Keys::generate();
        let a = nostr::EventBuilder::text_note("a")
            .sign_with_keys(&keys)
            .unwrap();
        let mut s = granted();
        subscribe(&ctx, &mut s, "feed").await;
        assert_eq!(deliveries_for(&s, &a).len(), 1);
        s.unsubscribe("feed");
        assert!(deliveries_for(&s, &a).is_empty());
        subscribe(&ctx, &mut s, "feed").await;
        assert_eq!(deliveries_for(&s, &a).len(), 1);
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
