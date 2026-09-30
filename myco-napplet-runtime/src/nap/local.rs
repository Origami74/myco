//! NAP-LOCAL — keep on this device (`docs/design/napplet/NAP-LOCAL.md`).
//!
//! Myco's shell caches everything a napplet reads, and the cache forgets. A
//! napplet that wants an event to stay asks for it here: `local.publish`
//! stores it in this device's own relay, which nothing evicts, and publishes
//! it nowhere else. Kept is not private: this device's relay is read by
//! paired phones and by the device's own nsites, and with a custom relay
//! configured it *is* that relay.
//!
//! - Given a **template**, the event is signed as the user and kept — a note
//!   to self, a draft. Signing as the user is the `relay` grant's power, so a
//!   template needs `relay` as well as `local`.
//! - Given a **signed event**, it is kept as it is, provided it verifies and
//!   was delivered to this napplet (see `relay::signed_or_template`).
//!
//! **Provisional.** This domain is a Myco addition, and the napplet.run spec
//! may settle keeping differently — folded into `relay.publish`, say. Kept
//! thin so moving it costs little.

use crate::dispatch::NapContext;
use crate::nap::relay::{carries_signed_event, event_json, signed_or_template};
use crate::seams::Envelope;
use crate::session::Session;

/// Handle an inbound `local.*` message.
pub async fn handle(ctx: &NapContext, session: &mut Session, message: &Envelope) -> Vec<Envelope> {
    match message.action() {
        "publish" => vec![publish(ctx, session, message).await],
        // An unrecognized action is silence, as NIP-5D asks.
        _ => Vec::new(),
    }
}

/// `local.publish` — keep the event on this device and nowhere else.
async fn publish(ctx: &NapContext, session: &Session, message: &Envelope) -> Envelope {
    // Before anything is signed: a signer app would otherwise ask the user to
    // approve an event that is then refused.
    if !carries_signed_event(message) && !session.is_granted("relay") {
        return failed(message, "signing as you needs the relay capability");
    }
    let (event, as_is) = match signed_or_template(ctx, session, message, false).await {
        Ok(event) => event,
        Err(e) => return failed(message, e),
    };
    if let Err(e) = ctx.sink.keep(event.clone()).await {
        return failed(message, format!("could not keep: {e}"));
    }
    let id = event.id.to_hex();
    tracing::info!(kind = %event.kind.as_u16(), event = %id, as_is, "napplet kept an event");
    message
        .to_result()
        .with_field("ok", true)
        .with_field("event", event_json(&event))
        .with_field("eventId", id)
}

fn failed(message: &Envelope, error: impl Into<String>) -> Envelope {
    message
        .to_result()
        .with_field("ok", false)
        .with_field("error", error.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seams::Signer;
    use crate::session::NappletIdentity;
    use crate::testing::test_context;
    use nostr::{EventBuilder, Filter, Keys};
    use serde_json::json;

    fn session() -> Session {
        let mut s = Session::new(NappletIdentity::new("notes", "agg"), ["local", "relay"]);
        s.on_ready();
        s
    }

    fn publish_msg(event: serde_json::Value) -> Envelope {
        Envelope::new("local.publish")
            .with_id("p1")
            .with_field("event", event)
    }

    #[tokio::test]
    async fn a_template_is_signed_as_the_user_and_kept() {
        let (ctx, signer) = test_context();
        let mut s = session();
        let out = crate::dispatch::dispatch(
            &ctx,
            &mut s,
            &publish_msg(json!({"kind": 1, "content": "note to self"})),
        )
        .await;
        let r = serde_json::to_value(&out.envelopes()[0]).unwrap();
        assert_eq!(r["ok"], true, "{r}");
        let kept = ctx.relay.query(&[Filter::new()]).await.unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].pubkey, signer.public_key());
    }

    #[tokio::test]
    async fn a_delivered_event_is_kept_as_it_is() {
        let (mut ctx, _signer) = test_context();
        let sink = std::sync::Arc::new(crate::testing::RecordingSink::new());
        ctx.sink = sink.clone();
        let mut s = session();
        let theirs = EventBuilder::text_note("from someone else")
            .sign_with_keys(&Keys::generate())
            .unwrap();
        ctx.relay.publish(theirs.clone()).await.unwrap();
        // Delivered through a query, as a napplet would come by it.
        let q = Envelope::new("relay.query")
            .with_id("q1")
            .with_field("filters", json!({"ids": [theirs.id.to_hex()]}));
        crate::dispatch::dispatch(&ctx, &mut s, &q).await;

        let out = crate::dispatch::dispatch(&ctx, &mut s, &publish_msg(event_json(&theirs))).await;
        let r = serde_json::to_value(&out.envelopes()[0]).unwrap();
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["eventId"], theirs.id.to_hex());
        assert_eq!(
            sink.calls(),
            vec![("keep", theirs.id)],
            "kept through another door"
        );
    }

    /// Signing as the user is the `relay` grant's power: a template under
    /// `local` alone is refused before anything is signed. A delivered event
    /// still keeps.
    #[tokio::test]
    async fn a_template_needs_the_relay_grant() {
        let (ctx, _signer) = test_context();
        let mut s = Session::new(NappletIdentity::new("notes", "agg"), ["local"]);
        s.on_ready();
        let out = crate::dispatch::dispatch(
            &ctx,
            &mut s,
            &publish_msg(json!({"kind": 1, "content": "note to self"})),
        )
        .await;
        let r = serde_json::to_value(&out.envelopes()[0]).unwrap();
        assert_eq!(r["ok"], false);
        assert!(r["error"].as_str().unwrap().contains("relay"));
        assert!(ctx.relay.query(&[Filter::new()]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_undelivered_event_is_refused_even_the_users_own() {
        let (ctx, signer) = test_context();
        let mut s = session();
        let theirs = EventBuilder::text_note("never shown")
            .sign_with_keys(&Keys::generate())
            .unwrap();
        let out = crate::dispatch::dispatch(&ctx, &mut s, &publish_msg(event_json(&theirs))).await;
        let r = serde_json::to_value(&out.envelopes()[0]).unwrap();
        assert_eq!(r["ok"], false);
        assert!(r["error"].as_str().unwrap().contains("delivered"));

        let own = signer
            .sign(EventBuilder::text_note("mine, not shown here").build(signer.public_key()))
            .await
            .unwrap();
        let out = crate::dispatch::dispatch(&ctx, &mut s, &publish_msg(event_json(&own))).await;
        let r = serde_json::to_value(&out.envelopes()[0]).unwrap();
        assert_eq!(r["ok"], false, "the user's own undelivered event was kept");
        assert!(ctx.relay.query(&[Filter::new()]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_forged_event_is_refused() {
        let (ctx, _signer) = test_context();
        let mut s = session();
        let real = EventBuilder::text_note("real")
            .sign_with_keys(&Keys::generate())
            .unwrap();
        let mut forged = event_json(&real);
        forged["content"] = json!("forged");
        let out = crate::dispatch::dispatch(&ctx, &mut s, &publish_msg(forged)).await;
        let r = serde_json::to_value(&out.envelopes()[0]).unwrap();
        assert_eq!(r["ok"], false);
    }
}
