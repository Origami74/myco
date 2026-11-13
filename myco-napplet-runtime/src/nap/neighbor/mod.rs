//! NAP-NEIGHBOR — the neighborhoods the user carries, and addresses for them.
//!
//! Spec: napplet/naps#146 (`naps/NAP-NEIGHBOR.md`). A neighborhood is a
//! propagation channel ("home", "festival") carried over the user's own peer
//! connections. This domain carries no data: it tells a napplet which
//! neighborhoods it was granted, which of the user's peers carry each, and a
//! `neighborhood:` address for each one, which the napplet then hands to
//! `relay`, `outbox`, `resource` and `upload`.
//!
//! ```text
//! -> { "type": "neighbor.list", "id": "n1" }
//! <- { "type": "neighbor.list.result", "id": "n1", "neighborhoods": [
//!      { "id": "h_2c", "name": "Household", "autoAccept": false,
//!        "peers": 3, "reachable": 2, "address": "neighborhood://h_2c" } ] }
//! <- { "type": "neighbor.event", "event": { "type": "peer", … } }   (pushed)
//! ```
//!
//! ## What lives where
//!
//! - [`model`] — the schema types, serialized as the NAP names them.
//! - [`seam`] — [`NeighborHost`], everything the device knows. The runtime
//!   holds no neighborhood state.
//! - [`address`] — reading and writing `neighborhood:` addresses, for the
//!   host that mints them and the lanes that route them.
//!
//! The runtime's own job is the wire: parse, gate on `info.features`, call the
//! host, answer in the NAP's shapes. Management calls wait on a person, so a
//! host may take a while to answer them; nothing here holds the session while
//! it does (see [`crate::dispatch::needs_session`]).

pub mod address;
pub mod model;
pub mod seam;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use model::{
    CreateRequest, Feature, InviteResult, InviteStatus, LinkClass, NeighborError, NeighborEvent,
    NeighborInfo, Neighborhood, NeighborhoodChange, Offer, Peer, PeerChange,
};
pub use seam::{NeighborHost, NoNeighborhoods};

use crate::dispatch::NapContext;
use crate::seams::Envelope;
use crate::session::Session;

/// Handle an inbound `neighbor.*` message.
pub async fn handle(ctx: &NapContext, session: &Session, message: &Envelope) -> Vec<Envelope> {
    let action = message.action();
    let known = matches!(
        action,
        "info" | "list" | "peers" | "create" | "invite" | "leave" | "setOffer" | "mute"
    );
    // Unrecognized within the domain: silent, per NIP-5D.
    if !known {
        return Vec::new();
    }

    let host = &ctx.neighbor;
    let info = host.info().await;
    if let Some(needed) = Feature::for_action(action) {
        if !info.has(needed) {
            return vec![failed(message, NeighborError::Unsupported)];
        }
    }

    let napplet = session.identity();
    let reply = match action {
        "info" => message.to_result().with_field("info", to_json(&info)),
        "list" => message
            .to_result()
            .with_field("neighborhoods", to_json(&host.list(napplet).await)),
        "peers" => match neighborhood_of(message) {
            Ok(n) => answer(message, "peers", host.peers(napplet, n).await),
            Err(e) => invalid(message, e),
        },
        "create" => match parse::<CreateRequest>(message, "request") {
            Ok(r) if r.name.trim().is_empty() => invalid(message, "request.name is empty"),
            Ok(r) => answer(message, "neighborhood", host.create(napplet, r).await),
            Err(e) => invalid(message, &e),
        },
        "invite" => match (neighborhood_of(message), candidates_of(message)) {
            (Ok(n), Ok(c)) => answer(message, "result", host.invite(napplet, n, c).await),
            (Err(e), _) | (_, Err(e)) => invalid(message, e),
        },
        "leave" => match neighborhood_of(message) {
            Ok(n) => empty(message, host.leave(napplet, n).await),
            Err(e) => invalid(message, e),
        },
        "setOffer" => match (neighborhood_of(message), parse::<Offer>(message, "offer")) {
            (Ok(n), Ok(o)) => answer(message, "offer", host.set_offer(napplet, n, o).await),
            (Err(e), _) => invalid(message, e),
            (_, Err(e)) => invalid(message, &e),
        },
        "mute" => match (
            neighborhood_of(message),
            author_of(message),
            muted_of(message),
        ) {
            (Ok(n), Ok(a), Ok(m)) => empty(message, host.mute(napplet, n, a, m).await),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => invalid(message, e),
        },
        _ => unreachable!("filtered above"),
    };
    vec![reply]
}

/// The push a session receives for a change in its neighborhoods — `None`
/// when it should hear nothing: not handshaken, or not granted `neighbor`.
/// The event's ids must already be this session's own.
pub fn event_frame(session: &Session, event: &NeighborEvent) -> Option<Envelope> {
    session
        .may_service("neighbor")
        .then(|| Envelope::new("neighbor.event").with_field("event", to_json(event)))
}

fn to_json<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).expect("NAP-NEIGHBOR types always serialize")
}

fn answer<T: serde::Serialize>(
    message: &Envelope,
    field: &str,
    outcome: Result<T, NeighborError>,
) -> Envelope {
    match outcome {
        Ok(value) => message.to_result().with_field(field, to_json(&value)),
        Err(e) => failed(message, e),
    }
}

fn empty(message: &Envelope, outcome: Result<(), NeighborError>) -> Envelope {
    match outcome {
        Ok(()) => message.to_result(),
        Err(e) => failed(message, e),
    }
}

fn failed(message: &Envelope, error: NeighborError) -> Envelope {
    message.to_error(error.as_str())
}

/// A request the NAP's types can't hold. The NAP names no error for it, so
/// this says what was wrong rather than borrowing one that means something
/// else.
fn invalid(message: &Envelope, why: &str) -> Envelope {
    message.to_error(format!("invalid request: {why}"))
}

fn neighborhood_of(message: &Envelope) -> Result<&str, &'static str> {
    message
        .field("neighborhood")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("neighborhood is missing")
}

fn candidates_of(message: &Envelope) -> Result<Vec<String>, &'static str> {
    match message.field("candidates") {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or("candidates must be strings"),
        Some(_) => Err("candidates must be a list"),
    }
}

fn author_of(message: &Envelope) -> Result<nostr::PublicKey, &'static str> {
    message
        .field("author")
        .and_then(|v| v.as_str())
        .and_then(|s| nostr::PublicKey::from_hex(s).ok())
        .ok_or("author must be a hex public key")
}

fn muted_of(message: &Envelope) -> Result<bool, &'static str> {
    message
        .field("muted")
        .and_then(|v| v.as_bool())
        .ok_or("muted must be a boolean")
}

fn parse<T: serde::de::DeserializeOwned>(message: &Envelope, field: &str) -> Result<T, String> {
    let value = message
        .field(field)
        .ok_or_else(|| format!("{field} is missing"))?;
    serde_json::from_value(value.clone()).map_err(|e| format!("{field}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::testing::MemNeighbors;
    use super::*;
    use crate::dispatch::dispatch;
    use crate::session::NappletIdentity;
    use crate::testing::test_context;
    use serde_json::json;
    use std::sync::Arc;

    fn session(granted: &[&str]) -> Session {
        let mut s = Session::new(NappletIdentity::new("chat", "aggregate"), granted.to_vec());
        s.on_ready();
        s
    }

    fn with(host: MemNeighbors) -> (NapContext, Arc<MemNeighbors>) {
        let (mut ctx, _signer) = test_context();
        let host = Arc::new(host);
        ctx.neighbor = host.clone();
        (ctx, host)
    }

    fn call(t: &str, fields: serde_json::Value) -> Envelope {
        let mut e = Envelope::new(t).with_id("n1");
        for (k, v) in fields.as_object().unwrap() {
            e = e.with_field(k.clone(), v.clone());
        }
        e
    }

    async fn one(ctx: &NapContext, s: &mut Session, e: Envelope) -> serde_json::Value {
        let out = dispatch(ctx, s, &e).await;
        assert_eq!(out.envelopes().len(), 1, "{:?}", out);
        serde_json::to_value(&out.envelopes()[0]).unwrap()
    }

    #[tokio::test]
    async fn list_answers_with_the_hosts_neighborhoods() {
        let (ctx, _) = with(MemNeighbors::household());
        let mut s = session(&["neighbor"]);
        let r = one(&ctx, &mut s, call("neighbor.list", json!({}))).await;
        assert_eq!(r["type"], "neighbor.list.result");
        assert_eq!(r["id"], "n1");
        assert_eq!(r["neighborhoods"][0]["name"], "Household");
        assert_eq!(r["neighborhoods"][0]["address"], "neighborhood://h2c");
    }

    #[tokio::test]
    async fn a_device_without_neighborhoods_lists_none_and_supports_nothing() {
        let (ctx, _signer) = test_context();
        let mut s = session(&["neighbor"]);
        let r = one(&ctx, &mut s, call("neighbor.info", json!({}))).await;
        assert_eq!(r["info"], json!({"features": [], "services": []}));
        let r = one(&ctx, &mut s, call("neighbor.list", json!({}))).await;
        assert_eq!(r["neighborhoods"], json!([]));
        let r = one(
            &ctx,
            &mut s,
            call("neighbor.create", json!({"request": {"name": "Home"}})),
        )
        .await;
        assert_eq!(r["error"], "unsupported");
    }

    #[tokio::test]
    async fn an_operation_missing_from_features_is_unsupported_without_asking_the_host() {
        let mut host = MemNeighbors::household();
        host.info.features.retain(|f| *f != Feature::Leave);
        let (ctx, host) = with(host);
        let mut s = session(&["neighbor"]);
        let r = one(
            &ctx,
            &mut s,
            call("neighbor.leave", json!({"neighborhood": "h2c"})),
        )
        .await;
        assert_eq!(r["error"], "unsupported");
        assert!(host.calls().is_empty());
    }

    #[tokio::test]
    async fn peers_of_an_unknown_neighborhood_is_not_found() {
        let (ctx, _) = with(MemNeighbors::household());
        let mut s = session(&["neighbor"]);
        let r = one(
            &ctx,
            &mut s,
            call("neighbor.peers", json!({"neighborhood": "nope"})),
        )
        .await;
        assert_eq!(r["error"], "not found");
        let r = one(
            &ctx,
            &mut s,
            call("neighbor.peers", json!({"neighborhood": "h2c"})),
        )
        .await;
        assert_eq!(r["peers"][0]["id"], "p7f");
    }

    #[tokio::test]
    async fn create_passes_the_request_and_answers_the_neighborhood() {
        let (ctx, host) = with(MemNeighbors::household());
        let mut s = session(&["neighbor"]);
        let r = one(
            &ctx,
            &mut s,
            call(
                "neighbor.create",
                json!({"request": {"name": "Lowlands 2027", "autoAccept": true}}),
            ),
        )
        .await;
        assert_eq!(r["neighborhood"]["name"], "Lowlands 2027");
        assert_eq!(r["neighborhood"]["autoAccept"], true);
        assert_eq!(host.calls(), vec!["create Lowlands 2027 auto"]);
    }

    #[tokio::test]
    async fn a_declined_prompt_is_cancelled() {
        let mut host = MemNeighbors::household();
        host.decline = true;
        let (ctx, _) = with(host);
        let mut s = session(&["neighbor"]);
        let r = one(
            &ctx,
            &mut s,
            call("neighbor.create", json!({"request": {"name": "Home"}})),
        )
        .await;
        assert_eq!(
            r,
            json!({"type": "neighbor.create.result", "id": "n1", "error": "cancelled"})
        );
    }

    #[tokio::test]
    async fn malformed_requests_say_what_was_wrong() {
        let (ctx, host) = with(MemNeighbors::household());
        let mut s = session(&["neighbor"]);
        for (t, fields) in [
            ("neighbor.peers", json!({})),
            ("neighbor.create", json!({})),
            ("neighbor.create", json!({"request": {"name": "  "}})),
            (
                "neighbor.invite",
                json!({"neighborhood": "h2c", "candidates": "p7f"}),
            ),
            (
                "neighbor.setOffer",
                json!({"neighborhood": "h2c", "offer": {"relayRead": 1}}),
            ),
            (
                "neighbor.mute",
                json!({"neighborhood": "h2c", "author": "zz", "muted": true}),
            ),
            (
                "neighbor.mute",
                json!({"neighborhood": "h2c", "author": "a".repeat(64)}),
            ),
        ] {
            let r = one(&ctx, &mut s, call(t, fields)).await;
            let err = r["error"].as_str().unwrap_or_default();
            assert!(err.starts_with("invalid request: "), "{t}: {r}");
        }
        assert!(host.calls().is_empty(), "nothing reached the host");
    }

    #[tokio::test]
    async fn mute_reaches_the_host_with_a_parsed_key() {
        let (ctx, host) = with(MemNeighbors::household());
        let mut s = session(&["neighbor"]);
        let author = nostr::Keys::generate().public_key().to_hex();
        let r = one(
            &ctx,
            &mut s,
            call(
                "neighbor.mute",
                json!({"neighborhood": "h2c", "author": author, "muted": true}),
            ),
        )
        .await;
        assert_eq!(r, json!({"type": "neighbor.mute.result", "id": "n1"}));
        assert_eq!(host.calls(), vec![format!("mute h2c {author} true")]);
    }

    #[tokio::test]
    async fn an_ungranted_napplet_is_refused() {
        let (ctx, host) = with(MemNeighbors::household());
        let mut s = session(&["relay"]);
        let r = one(&ctx, &mut s, call("neighbor.list", json!({}))).await;
        assert!(r["error"].is_string());
        assert!(host.calls().is_empty());
    }

    #[tokio::test]
    async fn unknown_actions_are_silent() {
        let (ctx, _) = with(MemNeighbors::household());
        let mut s = session(&["neighbor"]);
        let out = dispatch(&ctx, &mut s, &call("neighbor.serve", json!({}))).await;
        assert!(out.envelopes().is_empty());
    }

    #[test]
    fn calls_are_stateless() {
        for t in ["neighbor.list", "neighbor.create", "neighbor.mute"] {
            assert!(!crate::dispatch::needs_session(&Envelope::new(t)), "{t}");
        }
    }

    #[test]
    fn events_are_pushed_only_to_granted_established_sessions() {
        let e = NeighborEvent::Neighborhood {
            change: NeighborhoodChange::Added,
            neighborhood: "h2c".into(),
        };
        let frame = event_frame(&session(&["neighbor"]), &e).unwrap();
        assert_eq!(frame.msg_type, "neighbor.event");
        assert_eq!(frame.field("event").unwrap()["type"], "neighborhood");
        assert!(event_frame(&session(&[]), &e).is_none());
        let fresh = Session::new(NappletIdentity::new("chat", "aggregate"), ["neighbor"]);
        assert!(event_frame(&fresh, &e).is_none());
    }
}
