//! NAP-INC — the half of inter-napplet communication NAP-INTENT needs: a
//! napplet listening on a topic, and the shell delivering to it.
//!
//! ```text
//! -> { "type": "inc.subscribe", "id": "a1", "topic": "napplet:profile/open" }
//! <- { "type": "inc.subscribe.result", "id": "a1" }
//! <- { "type": "inc.event", "topic": "napplet:profile/open", "sender": "chronofeed",
//!      "payload": { "pubkey": "…" } }                                  (pushed)
//! -> { "type": "inc.unsubscribe", "topic": "napplet:profile/open" }
//! ```
//!
//! What is here, and what is not:
//!
//! - **Subscribe / unsubscribe** — implemented. The topics a session listens
//!   on are kept on the [`Session`]; NAP-INTENT delivers a payload only once
//!   its handler listens on the convention's topic
//!   ([`Session::listens_on`]), because the shim drops an `inc.event` for a
//!   topic nobody subscribed to.
//! - **`inc.emit`** — accepted and **dropped**. Routing one napplet's emit to
//!   every other napplet listening on the topic is a channel between napplets
//!   the user never set up, and nothing Myco hosts needs it yet. The call is
//!   fire-and-forget with no result, so dropping it is indistinguishable, to
//!   the napplet, from nobody listening.
//! - **Channels** — refused: `inc.channel.open` answers with an error (so the
//!   shim rejects at once rather than timing out), `inc.channel.list` answers
//!   an empty list, and the fire-and-forget channel messages are ignored.
//!
//! The only `inc.event` a napplet receives today is the one the shell writes
//! for NAP-INTENT, with `sender` set to the calling napplet's `dTag` — derived
//! from the caller's session, never from anything the caller sent.

use crate::seams::Envelope;
use crate::session::Session;

/// The longest topic accepted. A convention is a few dozen bytes; this bounds
/// what a napplet can make the runtime hold per topic.
pub const MAX_TOPIC_LEN: usize = 256;

/// `error` on `inc.channel.open.result`.
pub const CHANNELS_UNSUPPORTED: &str = "channels are not supported by this runtime";

/// Handle an inbound `inc.*` message.
pub fn handle(session: &mut Session, message: &Envelope) -> Vec<Envelope> {
    match message.action() {
        "subscribe" => {
            let topic = match topic_of(message) {
                Ok(topic) => topic,
                Err(why) => return vec![message.to_error(why)],
            };
            match session.subscribe_topic(topic) {
                Ok(()) => vec![message.to_result()],
                Err(why) => vec![message.to_error(why)],
            }
        }
        "unsubscribe" => {
            if let Ok(topic) = topic_of(message) {
                session.unsubscribe_topic(&topic);
            }
            Vec::new()
        }
        // Fire-and-forget; see the module docs.
        "emit" => Vec::new(),
        "channel.open" => vec![message.to_error(CHANNELS_UNSUPPORTED)],
        "channel.list" => vec![message
            .to_result()
            .with_field("channels", serde_json::Value::Array(Vec::new()))],
        // `channel.emit`, `channel.broadcast`, `channel.close`: no channel can
        // exist, so there is nothing to act on. Anything else is unknown.
        _ => Vec::new(),
    }
}

/// The `topic` field, if it is one this runtime will hold: a non-empty string
/// of at most [`MAX_TOPIC_LEN`] bytes with no control characters. Otherwise
/// opaque — NAP-INC routes by exact equality and never parses a topic.
fn topic_of(message: &Envelope) -> Result<String, &'static str> {
    let topic = message
        .field("topic")
        .and_then(|t| t.as_str())
        .ok_or("topic is missing")?;
    if topic.is_empty() || topic.len() > MAX_TOPIC_LEN || topic.chars().any(char::is_control) {
        return Err("topic is not acceptable");
    }
    Ok(topic.to_string())
}

/// One `inc.event`: a delivery on `topic` from the napplet `sender` (a dTag
/// the runtime attests). `payload` is omitted when there is none.
pub fn event_frame(topic: &str, sender: &str, payload: Option<&serde_json::Value>) -> Envelope {
    let frame = Envelope::new("inc.event")
        .with_field("topic", topic)
        .with_field("sender", sender);
    match payload {
        Some(payload) => frame.with_field("payload", payload.clone()),
        None => frame,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{dispatch, needs_session};
    use crate::session::{NappletIdentity, MAX_INC_TOPICS};
    use crate::testing::test_context;
    use serde_json::json;

    fn session(granted: &[&str]) -> Session {
        let mut s = Session::new(NappletIdentity::new("feed", "aggregate"), granted.to_vec());
        s.on_ready();
        s
    }

    fn subscribe(topic: &str) -> Envelope {
        Envelope::new("inc.subscribe")
            .with_id("a1")
            .with_field("topic", topic)
    }

    #[tokio::test]
    async fn subscribe_is_confirmed_and_recorded() {
        let (ctx, _signer) = test_context();
        let mut s = session(&["inc"]);
        let out = dispatch(&ctx, &mut s, &subscribe("napplet:profile/open")).await;
        assert_eq!(
            serde_json::to_value(&out.envelopes()[0]).unwrap(),
            json!({"type": "inc.subscribe.result", "id": "a1"})
        );
        assert!(s.listens_on("napplet:profile/open"));
        assert!(!s.listens_on("napplet:profile/edit"), "exact match only");
        assert!(!s.listens_on("napplet:profile"), "no prefix match");
    }

    #[tokio::test]
    async fn unsubscribe_forgets_the_topic_and_says_nothing() {
        let (ctx, _signer) = test_context();
        let mut s = session(&["inc"]);
        dispatch(&ctx, &mut s, &subscribe("napplet:note/open")).await;
        let out = dispatch(
            &ctx,
            &mut s,
            &Envelope::new("inc.unsubscribe").with_field("topic", "napplet:note/open"),
        )
        .await;
        assert!(out.envelopes().is_empty());
        assert!(!s.listens_on("napplet:note/open"));
        // Unsubscribing from a topic never subscribed is not an error.
        let out = dispatch(
            &ctx,
            &mut s,
            &Envelope::new("inc.unsubscribe").with_field("topic", "nope"),
        )
        .await;
        assert!(out.envelopes().is_empty());
    }

    #[tokio::test]
    async fn a_bad_topic_is_refused_and_not_recorded() {
        let (ctx, _signer) = test_context();
        let mut s = session(&["inc"]);
        for bad in [
            json!(""),
            json!(42),
            json!("a\nb"),
            json!("x".repeat(MAX_TOPIC_LEN + 1)),
        ] {
            let e = Envelope::new("inc.subscribe")
                .with_id("a1")
                .with_field("topic", bad.clone());
            let out = dispatch(&ctx, &mut s, &e).await;
            assert!(out.envelopes()[0].field("error").is_some(), "{bad}");
        }
        assert!(s.inc_topics().is_empty());
    }

    #[tokio::test]
    async fn topics_are_capped() {
        let (ctx, _signer) = test_context();
        let mut s = session(&["inc"]);
        for i in 0..MAX_INC_TOPICS {
            let out = dispatch(&ctx, &mut s, &subscribe(&format!("t{i}"))).await;
            assert!(out.envelopes()[0].field("error").is_none());
        }
        let out = dispatch(&ctx, &mut s, &subscribe("one-too-many")).await;
        assert!(out.envelopes()[0].field("error").is_some());
        // Re-subscribing to a held topic is fine at the cap.
        let out = dispatch(&ctx, &mut s, &subscribe("t3")).await;
        assert!(out.envelopes()[0].field("error").is_none());
    }

    #[tokio::test]
    async fn emit_goes_nowhere_and_channels_are_refused() {
        let (ctx, _signer) = test_context();
        let mut s = session(&["inc"]);
        let out = dispatch(
            &ctx,
            &mut s,
            &Envelope::new("inc.emit")
                .with_field("topic", "napplet:note/open")
                .with_field("payload", json!({"id": "x"})),
        )
        .await;
        assert!(out.envelopes().is_empty());

        let out = dispatch(
            &ctx,
            &mut s,
            &Envelope::new("inc.channel.open")
                .with_id("c1")
                .with_field("target", "other"),
        )
        .await;
        let r = serde_json::to_value(&out.envelopes()[0]).unwrap();
        assert_eq!(r["type"], "inc.channel.open.result");
        assert_eq!(r["error"], CHANNELS_UNSUPPORTED);
        assert!(r.get("channelId").is_none());

        let out = dispatch(
            &ctx,
            &mut s,
            &Envelope::new("inc.channel.list").with_id("l1"),
        )
        .await;
        assert_eq!(
            serde_json::to_value(&out.envelopes()[0]).unwrap(),
            json!({"type": "inc.channel.list.result", "id": "l1", "channels": []})
        );
    }

    /// A listener stops hearing anything once `inc` is switched off.
    #[tokio::test]
    async fn listening_needs_the_grant() {
        let (ctx, _signer) = test_context();
        let mut s = session(&["inc"]);
        dispatch(&ctx, &mut s, &subscribe("napplet:note/open")).await;
        s.set_granted(Vec::<String>::new());
        assert!(!s.listens_on("napplet:note/open"));

        let mut ungranted = session(&[]);
        let out = dispatch(&ctx, &mut ungranted, &subscribe("napplet:note/open")).await;
        assert!(out.envelopes()[0]
            .field("error")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("inc"));
    }

    #[test]
    fn subscribe_and_unsubscribe_change_the_session() {
        assert!(needs_session(&Envelope::new("inc.subscribe")));
        assert!(needs_session(&Envelope::new("inc.unsubscribe")));
        assert!(!needs_session(&Envelope::new("inc.emit")));
    }

    #[test]
    fn an_event_frame_carries_topic_sender_and_payload() {
        let with = event_frame("napplet:note/open", "feed", Some(&json!({"id": "x"})));
        assert_eq!(
            serde_json::to_value(&with).unwrap(),
            json!({"type": "inc.event", "topic": "napplet:note/open", "sender": "feed",
                   "payload": {"id": "x"}})
        );
        let without = event_frame("t", "feed", None);
        assert!(without.field("payload").is_none());
        assert!(without.id.is_none());
    }
}
