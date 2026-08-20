//! NAP-RELAY — reading and publishing Nostr events on the user's behalf.
//!
//! This is the domain where a napplet acts *as* the user, and the one place the
//! user key is used. The napplet never holds it: it describes an event, the
//! runtime fills in the identity, signs, and publishes. There is no path here
//! that returns key material and none that signs something the runtime did not
//! construct itself.
//!
//! ## What the napplet does not get to choose
//!
//! A publish template arrives from untrusted code, so the runtime owns the
//! fields that decide *who* published and *when*:
//!
//! - **`pubkey`** is always the user key. A napplet naming an author would be
//!   asking the runtime to forge one.
//! - **`created_at`** is always now. Accepting the napplet's would let it
//!   backdate an event into a conversation that has already happened, or
//!   postdate one to sit at the top of a feed indefinitely.
//! - **`id` and `sig`** are computed, never copied.
//!
//! Everything that is genuinely the napplet's message — `kind`, `content`,
//! `tags` — is taken as given. The runtime is a notary, not an editor.
//!
//! ## The grant is per call
//!
//! A `relay` grant covers publishing with no per-event prompt (D8), which means
//! a granted napplet can publish as you at will. That is why the install screen
//! says so in words, and why the grant is checked on every call rather than
//! cached at handshake — revoking it stops the next publish, not the next
//! launch.

use nostr::{Filter, JsonUtil, Kind, Tag, Timestamp, UnsignedEvent};

use crate::dispatch::NapContext;
use crate::seams::Envelope;

/// Handle an inbound `relay.*` message.
pub async fn handle(ctx: &NapContext, message: &Envelope) -> Vec<Envelope> {
    match message.action() {
        "query" => vec![query(ctx, message).await],
        "publish" => vec![publish(ctx, message).await],
        "subscribe" => subscribe(ctx, message).await,
        "close" => Vec::new(),
        // Encryption is not wired up yet. Saying so beats a silent drop, which
        // a napplet would wait on forever.
        "publishEncrypted" => vec![message
            .to_result()
            .with_field("ok", false)
            .with_field("error", "encrypted publishing is not available yet")],
        _ => Vec::new(),
    }
}

/// `relay.query` — collect stored events matching the filters.
async fn query(ctx: &NapContext, message: &Envelope) -> Envelope {
    let filters = match filters_from(message) {
        Ok(filters) => filters,
        Err(e) => return message.to_error(e),
    };

    match ctx.relay.query(&filters).await {
        Ok(events) => {
            let results: Vec<serde_json::Value> = events.iter().map(result_of).collect();
            message
                .to_result()
                .with_field("events", serde_json::Value::Array(results))
        }
        Err(e) => message.to_error(format!("query failed: {e}")),
    }
}

/// `relay.subscribe` — the stored events, then EOSE.
///
/// Everything the relay already holds is delivered, which is what a napplet
/// rendering a profile or a feed is waiting for. Events arriving *after* EOSE
/// need the runtime to push unprompted, which the shell channel does not carry
/// yet — so this is honest about what it is: a subscription that reaches EOSE
/// and then stays quiet, rather than one that silently never fires.
async fn subscribe(ctx: &NapContext, message: &Envelope) -> Vec<Envelope> {
    let sub_id = match message.field("subId").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return vec![message.to_error("subscribe needs a subId")],
    };

    let filters = match filters_from(message) {
        Ok(filters) => filters,
        Err(e) => return vec![message.to_error(e)],
    };

    let mut out = Vec::new();
    match ctx.relay.query(&filters).await {
        Ok(events) => {
            for event in &events {
                out.push(
                    Envelope::new("relay.event")
                        .with_field("subId", sub_id.clone())
                        .with_field("result", result_of(event)),
                );
            }
        }
        Err(e) => {
            return vec![Envelope::new("relay.closed")
                .with_field("subId", sub_id)
                .with_field("reason", format!("query failed: {e}"))];
        }
    }

    out.push(Envelope::new("relay.eose").with_field("subId", sub_id));
    out
}

/// `relay.publish` — sign the napplet's template as the user, and store it.
async fn publish(ctx: &NapContext, message: &Envelope) -> Envelope {
    let Some(template) = message.field("event").and_then(|v| v.as_object()) else {
        return failed(message, "publish needs an event template");
    };

    let kind = match template.get("kind").and_then(|v| v.as_u64()) {
        Some(kind) if kind <= u16::MAX as u64 => Kind::from(kind as u16),
        _ => return failed(message, "the event template needs a kind"),
    };
    let content = template
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut tags = Vec::new();
    if let Some(raw) = template.get("tags").and_then(|v| v.as_array()) {
        for entry in raw {
            let Some(parts) = entry.as_array() else {
                return failed(message, "every tag must be an array of strings");
            };
            let mut values = Vec::with_capacity(parts.len());
            for part in parts {
                match part.as_str() {
                    Some(s) => values.push(s.to_string()),
                    None => return failed(message, "every tag value must be a string"),
                }
            }
            match Tag::parse(values) {
                Ok(tag) => tags.push(tag),
                Err(e) => return failed(message, format!("unusable tag: {e}")),
            }
        }
    }

    let Ok(pubkey) = ctx.signer.public_key().await else {
        return failed(message, "there is no user key on this device yet");
    };

    // The runtime owns author and time; the napplet owns the message.
    let unsigned = UnsignedEvent::new(pubkey, Timestamp::now(), kind, tags, content);

    let signed = match ctx.signer.sign(unsigned).await {
        Ok(event) => event,
        Err(e) => return failed(message, format!("could not sign: {e}")),
    };

    // Accepted, not merely stored: this is what wakes local subscriptions and
    // hands the event to the mesh.
    if let Err(e) = ctx.sink.accept(signed.clone()).await {
        return failed(message, format!("could not publish: {e}"));
    }

    let id = signed.id.to_hex();
    tracing::info!(kind = %kind.as_u16(), event = %id, "napplet published");
    message
        .to_result()
        .with_field("ok", true)
        .with_field(
            "event",
            serde_json::from_str::<serde_json::Value>(&signed.as_json())
                .unwrap_or(serde_json::Value::Null),
        )
        .with_field("eventId", id)
}

/// A publish failure, in the shape NAP-RELAY gives it: `ok` false beside the
/// reason, so a napplet reads one field to branch on.
fn failed(message: &Envelope, error: impl Into<String>) -> Envelope {
    message
        .to_result()
        .with_field("ok", false)
        .with_field("error", error.into())
}

/// A `RelayEventResult`: the raw event, and no sidecar we can honestly fill in.
fn result_of(event: &nostr::Event) -> serde_json::Value {
    serde_json::json!({
        "event": serde_json::from_str::<serde_json::Value>(&event.as_json())
            .unwrap_or(serde_json::Value::Null),
    })
}

/// The `filters` field, as NIP-01 filters.
///
/// Filters come from untrusted code, so an unreadable one is refused rather
/// than quietly dropped — a napplet that asked for something specific and got
/// everything, or nothing, would have no way to tell.
fn filters_from(message: &Envelope) -> Result<Vec<Filter>, String> {
    let Some(raw) = message.field("filters") else {
        return Err("this call needs filters".to_string());
    };
    // One filter or a list of them; the spec allows both.
    let list = match raw {
        serde_json::Value::Array(items) => items.clone(),
        object @ serde_json::Value::Object(_) => vec![object.clone()],
        _ => return Err("filters must be an object or a list of objects".to_string()),
    };

    let mut out = Vec::with_capacity(list.len());
    for item in list {
        match serde_json::from_value::<Filter>(item) {
            Ok(filter) => out.push(filter),
            Err(e) => return Err(format!("unreadable filter: {e}")),
        }
    }
    if out.is_empty() {
        return Err("this call needs at least one filter".to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::dispatch;
    use crate::session::{NappletIdentity, Session};
    use crate::testing::test_context;
    use nostr::{EventBuilder, Keys};
    use serde_json::json;

    fn granted() -> Session {
        let mut s = Session::new(NappletIdentity::new("chat", "aggregate"), ["relay"]);
        s.on_ready();
        s
    }

    async fn call(ctx: &NapContext, envelope: Envelope) -> Vec<Envelope> {
        dispatch(ctx, &mut granted(), &envelope)
            .await
            .envelopes()
            .to_vec()
    }

    #[tokio::test]
    async fn publishes_as_the_user_and_stores_the_event() {
        let (ctx, signer) = test_context();
        let out = call(
            &ctx,
            Envelope::new("relay.publish").with_id("b2").with_field(
                "event",
                json!({"kind": 1, "content": "hello world", "tags": [["t", "myco"]]}),
            ),
        )
        .await;

        assert_eq!(out.len(), 1);
        let reply = &out[0];
        assert_eq!(reply.msg_type, "relay.publish.result");
        assert_eq!(reply.id.as_deref(), Some("b2"));
        assert_eq!(reply.field("ok").unwrap(), &json!(true));

        let event = reply.field("event").unwrap();
        assert_eq!(event["content"], "hello world");
        assert_eq!(event["kind"], 1);
        assert_eq!(event["tags"], json!([["t", "myco"]]));
        // Signed as the user, by the runtime.
        assert_eq!(event["pubkey"], json!(signer.public_key().to_hex()));
        assert!(!event["sig"].as_str().unwrap().is_empty());
        assert_eq!(reply.field("eventId").unwrap(), &event["id"]);

        // And it is actually in the relay, not merely signed.
        let stored = ctx
            .relay
            .query(&[Filter::new().author(signer.public_key())])
            .await
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].content, "hello world");
    }

    /// The napplet writes the message; the runtime writes who and when. A
    /// template naming another author, or a time of its choosing, must not
    /// carry through — that is forgery with extra steps.
    #[tokio::test]
    async fn the_napplet_cannot_choose_the_author_or_the_time() {
        let (ctx, signer) = test_context();
        let impostor = Keys::generate();
        let long_ago = 1_000_000_000u64;

        let out = call(
            &ctx,
            Envelope::new("relay.publish").with_id("b2").with_field(
                "event",
                json!({
                    "kind": 1,
                    "content": "not mine",
                    "tags": [],
                    "pubkey": impostor.public_key().to_hex(),
                    "created_at": long_ago,
                    "id": "0".repeat(64),
                    "sig": "0".repeat(128),
                }),
            ),
        )
        .await;

        let event = out[0].field("event").unwrap();
        assert_eq!(event["pubkey"], json!(signer.public_key().to_hex()));
        assert_ne!(event["pubkey"], json!(impostor.public_key().to_hex()));
        assert!(
            event["created_at"].as_u64().unwrap() > long_ago,
            "the napplet backdated its event"
        );
        assert_ne!(event["id"], json!("0".repeat(64)));
        assert_ne!(event["sig"], json!("0".repeat(128)));
    }

    #[tokio::test]
    async fn queries_return_stored_events() {
        let (ctx, _signer) = test_context();
        let author = Keys::generate();
        let note = EventBuilder::text_note("stored already")
            .sign_with_keys(&author)
            .unwrap();
        ctx.relay.publish(note).await.unwrap();

        let out = call(
            &ctx,
            Envelope::new("relay.query")
                .with_id("c3")
                .with_field("filters", json!([{"kinds": [1], "limit": 10}])),
        )
        .await;

        assert_eq!(out[0].msg_type, "relay.query.result");
        let events = out[0].field("events").unwrap().as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["event"]["content"], "stored already");
    }

    /// Subscribe delivers what the relay holds, then EOSE — the shape a napplet
    /// waits on before it renders.
    #[tokio::test]
    async fn subscribe_delivers_then_reaches_eose() {
        let (ctx, _signer) = test_context();
        // Two authors, because the in-memory relay keeps one slot per
        // (kind, author) — two notes from one author would overwrite.
        for content in ["one", "two"] {
            let note = EventBuilder::text_note(content)
                .sign_with_keys(&Keys::generate())
                .unwrap();
            ctx.relay.publish(note).await.unwrap();
        }

        let out = call(
            &ctx,
            Envelope::new("relay.subscribe")
                .with_id("a1")
                .with_field("subId", "sub-1")
                .with_field("filters", json!([{"kinds": [1]}])),
        )
        .await;

        let (events, rest): (Vec<_>, Vec<_>) =
            out.iter().partition(|e| e.msg_type == "relay.event");
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.field("subId").unwrap() == "sub-1"));
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].msg_type, "relay.eose");
        assert_eq!(rest[0].field("subId").unwrap(), "sub-1");
    }

    /// A single filter object, not only a list — the spec allows both.
    #[tokio::test]
    async fn a_lone_filter_object_is_accepted() {
        let (ctx, _signer) = test_context();
        let out = call(
            &ctx,
            Envelope::new("relay.query")
                .with_id("c3")
                .with_field("filters", json!({"kinds": [1]})),
        )
        .await;
        assert!(out[0].field("events").is_some());
    }

    /// Unreadable input is refused rather than silently treated as "everything"
    /// or "nothing" — a napplet could not tell those apart from a real answer.
    #[tokio::test]
    async fn unusable_input_is_refused_not_guessed() {
        let (ctx, _signer) = test_context();

        for envelope in [
            Envelope::new("relay.query").with_id("x"),
            Envelope::new("relay.query")
                .with_id("x")
                .with_field("filters", json!("everything")),
            Envelope::new("relay.query")
                .with_id("x")
                .with_field("filters", json!([])),
        ] {
            let out = call(&ctx, envelope).await;
            assert!(
                out[0].field("error").is_some(),
                "guessed instead of refusing"
            );
        }

        // A publish with no template, and one whose tags are not strings.
        for envelope in [
            Envelope::new("relay.publish").with_id("x"),
            Envelope::new("relay.publish")
                .with_id("x")
                .with_field("event", json!({"kind": 1, "tags": [[1, 2]]})),
        ] {
            let out = call(&ctx, envelope).await;
            assert_eq!(out[0].field("ok").unwrap(), &json!(false));
            assert!(out[0].field("error").is_some());
        }
    }

    /// Encryption is not built yet. A napplet is told so, rather than left
    /// waiting on a reply that never comes.
    #[tokio::test]
    async fn encrypted_publishing_says_it_is_unavailable() {
        let (ctx, _signer) = test_context();
        let out = call(
            &ctx,
            Envelope::new("relay.publishEncrypted")
                .with_id("f6")
                .with_field("event", json!({"kind": 4, "content": "secret"}))
                .with_field("recipient", "abc"),
        )
        .await;
        assert_eq!(out[0].field("ok").unwrap(), &json!(false));
        assert!(out[0].field("error").is_some());
    }

    /// A published event must be *handed on*, not only written. Storing alone
    /// leaves it invisible: nothing on this device redraws, and no peer ever
    /// hears it — which is exactly how a doorbell that rings nowhere looks.
    #[tokio::test]
    async fn a_published_event_reaches_the_sink() {
        use crate::testing::RecordingSink;
        use std::sync::Arc;

        let (base, signer) = test_context();
        let sink = Arc::new(RecordingSink::new());
        let ctx = NapContext {
            signer: base.signer.clone(),
            relay: base.relay.clone(),
            sink: sink.clone(),
        };

        call(
            &ctx,
            Envelope::new("relay.publish")
                .with_id("b2")
                .with_field("event", json!({"kind": 1, "content": "ding", "tags": []})),
        )
        .await;

        let accepted = sink.accepted();
        assert_eq!(accepted.len(), 1, "the event was never handed on");
        assert_eq!(accepted[0].content, "ding");
        assert_eq!(accepted[0].pubkey, signer.public_key());
    }

    /// A refused publish hands on nothing. The grant is checked before the
    /// event is signed, so there is nothing to leak downstream either.
    #[tokio::test]
    async fn a_refused_publish_reaches_no_sink() {
        use crate::testing::RecordingSink;
        use std::sync::Arc;

        let (base, _signer) = test_context();
        let sink = Arc::new(RecordingSink::new());
        let ctx = NapContext {
            signer: base.signer.clone(),
            relay: base.relay.clone(),
            sink: sink.clone(),
        };

        let mut ungranted = Session::new(
            NappletIdentity::new("chat", "aggregate"),
            Vec::<String>::new(),
        );
        ungranted.on_ready();
        let msg = Envelope::new("relay.publish")
            .with_id("b2")
            .with_field("event", json!({"kind": 1, "content": "ding", "tags": []}));
        dispatch(&ctx, &mut ungranted, &msg).await;

        assert!(
            sink.accepted().is_empty(),
            "a refused publish was handed on"
        );
    }

    /// Without the grant, nothing publishes — and the refusal never signs.
    #[tokio::test]
    async fn an_ungranted_napplet_cannot_publish() {
        let (ctx, signer) = test_context();
        let mut ungranted = Session::new(
            NappletIdentity::new("chat", "aggregate"),
            Vec::<String>::new(),
        );
        ungranted.on_ready();

        let msg = Envelope::new("relay.publish").with_id("b2").with_field(
            "event",
            json!({"kind": 1, "content": "should not appear", "tags": []}),
        );
        let out = dispatch(&ctx, &mut ungranted, &msg)
            .await
            .envelopes()
            .to_vec();

        assert_eq!(out.len(), 1);
        assert!(out[0].field("error").is_some());
        assert!(out[0].field("event").is_none());

        let stored = ctx
            .relay
            .query(&[Filter::new().author(signer.public_key())])
            .await
            .unwrap();
        assert!(stored.is_empty(), "a refused publish still wrote an event");
    }
}
