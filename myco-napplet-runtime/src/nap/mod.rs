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
pub mod mesh;
pub mod outbox;
pub mod relay;
pub mod resource;
pub mod shell;

use nostr::{Event, Filter};

use crate::dispatch::NapContext;
use crate::seams::Envelope;
use crate::session::Session;

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
    SUBSCRIBING_DOMAINS
        .iter()
        .flat_map(|domain| deliveries_in(session, domain, event))
        .collect()
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
}
