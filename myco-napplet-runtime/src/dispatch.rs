//! Envelope routing: which NAP handles a message, and whether it may be
//! handled at all.
//!
//! Three refusals live here, and they are deliberately different from each
//! other:
//!
//! - **Unrecognized type — silence.** NIP-5D: "Messages with an unrecognized
//!   `type` MUST be silently ignored." That is what lets a napplet built
//!   against a newer registry run here without erroring on every call to a
//!   domain this build has never heard of. An error reply would also tell a
//!   napplet exactly which domains exist, which is not its business.
//! - **Not yet handshaken — an error.** NAP-SHELL forbids servicing capability
//!   calls before the session is established. The type is recognized, so
//!   silence would look like a lost message.
//! - **Not granted — an error.** The napplet's prelude installs only granted
//!   domains, so a call here means it went around its own namespace. It already
//!   knows its grants from `shell.init`, so saying so leaks nothing.

use std::sync::Arc;

use crate::nap;
use crate::seams::{
    BlobFetcher, BlobStore, Envelope, EventSink, LaneTransport, MeshSink, OutboxResolver,
    RelayBackend, Signer,
};
use crate::session::Session;

/// What the capabilities reach the world through.
///
/// Held once and shared by every session: the seams are per-device, not
/// per-napplet. What differs per napplet is the session — its identity and its
/// grants — which is why they are separate arguments and not one object.
#[derive(Clone)]
pub struct NapContext {
    /// Signs on the user's behalf. A napplet describes an event and gets one
    /// back; it never sees a key.
    pub signer: Arc<dyn Signer>,
    /// Where events are read from.
    pub relay: Arc<dyn RelayBackend>,
    /// Where a napplet's published events are accepted — stored, shown here,
    /// and carried to other people. See [`EventSink`].
    pub sink: Arc<dyn EventSink>,
    /// Hop-limited publish and pull over the mesh, behind NAP-MESH. See
    /// [`MeshSink`].
    pub mesh: Arc<dyn MeshSink>,
    /// NIP-65 relay planning, behind NAP-OUTBOX. See [`OutboxResolver`].
    pub outbox: Arc<dyn OutboxResolver>,
    /// Lane I/O for NAP-OUTBOX. See [`LaneTransport`].
    pub lanes: Arc<dyn LaneTransport>,
    /// This device's Blossom store — asked first for every NAP-RESOURCE
    /// `blossom:` fetch, and where a fetched blob is kept.
    pub blobs: Arc<dyn BlobStore>,
    /// Where a blob the store lacks is fetched from. See [`BlobFetcher`].
    pub fetcher: Arc<dyn BlobFetcher>,
}

/// What to do with an inbound message.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Send these back to the napplet. Empty for a handled message with no
    /// reply, such as a duplicate `shell.ready`.
    Reply(Vec<Envelope>),
    /// Drop it without a word.
    Ignore,
}

impl Outcome {
    /// The envelopes to send, if any.
    pub fn envelopes(&self) -> &[Envelope] {
        match self {
            Self::Reply(envelopes) => envelopes,
            Self::Ignore => &[],
        }
    }
}

/// Whether handling `message` changes the session — the handshake, and
/// opening or closing a subscription. Everything else reads the session for
/// the gate and nothing more.
///
/// A host uses this to decide what to hold while a call runs. A relay read
/// waits on the network for seconds; holding the session across it queues
/// every other call from that window behind it, and a napplet that fires
/// five queries at once has its fifth time out on the shim side before it
/// is even started. A stateless call can run against a snapshot instead,
/// and does — see `NappletHost::frame` in `myco-core`.
pub fn needs_session(message: &Envelope) -> bool {
    matches!(
        (message.domain(), message.action()),
        ("shell", _) | ("relay" | "mesh" | "outbox", "subscribe" | "close")
    )
}

/// A refusal in the shape the message's domain reads errors in.
///
/// Most NAPs put `error` on the `.result` envelope. NAP-RESOURCE does not:
/// its shim resolves every `.result` with `result.blob` and rejects only on a
/// distinct `.error` type, so a refusal sent as a result there would hand the
/// napplet `undefined` where it expects bytes — and the napplet would fall
/// over on `blob.arrayBuffer()` with no idea it had been refused.
fn refusal(message: &Envelope, why: &str) -> Envelope {
    if message.domain() == "resource" {
        let mut out = Envelope::new(format!("{}.error", message.msg_type))
            .with_field("error", "blocked-by-policy")
            .with_field("message", why);
        out.id = message.id.clone();
        return out;
    }
    message.to_error(why)
}

/// Route one inbound message for one napplet's session.
pub async fn dispatch(ctx: &NapContext, session: &mut Session, message: &Envelope) -> Outcome {
    // Results travel runtime -> napplet. One arriving the other way is either a
    // confused napplet or a reflected message; either way there is nothing to
    // service.
    if message.is_result() {
        return Outcome::Ignore;
    }

    let domain = message.domain();
    if !session.implements(domain) {
        return Outcome::Ignore;
    }

    // NAP-SHELL is the handshake, so it is reachable before the session is
    // established — it is what establishes it. Everything else is not.
    if domain != "shell" {
        if !session.is_established() {
            return Outcome::Reply(vec![refusal(
                message,
                "session not established: send shell.ready first",
            )]);
        }
        // The permission, enforced here and only here. Every implemented API
        // is in the napplet's namespace whatever it was granted, so this is the
        // boundary — and it is checked per call, so revoking a grant takes
        // effect on the next call rather than at the next reload.
        if !session.is_granted(domain) {
            // Logged because the napplet's own error is invisible from outside:
            // a refused call and a call never made look identical in logcat,
            // and telling them apart is the first question anyone asks when a
            // napplet quietly does nothing.
            tracing::info!(
                napplet = %session.identity().d_tag,
                %domain,
                action = %message.action(),
                "refused: capability not granted to this napplet"
            );
            return Outcome::Reply(vec![refusal(
                message,
                &format!("capability {domain} was not granted to this napplet"),
            )]);
        }
    }

    match domain {
        "shell" => Outcome::Reply(nap::shell::handle(session, message)),
        "identity" => Outcome::Reply(nap::identity::handle(ctx, message).await),
        "relay" => Outcome::Reply(nap::relay::handle(ctx, session, message).await),
        "mesh" => Outcome::Reply(nap::mesh::handle(ctx, session, message).await),
        "outbox" => Outcome::Reply(nap::outbox::handle(ctx, session, message).await),
        "resource" => Outcome::Reply(nap::resource::handle(ctx, message).await),
        // Implemented, granted, established — and still unrouted. Reaching here
        // means the implemented set grew without a handler, which is a bug in
        // this crate rather than anything the napplet did.
        _ => Outcome::Reply(vec![message.to_error("capability is not wired up")]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::NappletIdentity;
    use crate::testing::test_context;
    use serde_json::json;

    /// A refused resource call is a `resource.bytes.error`, never a result:
    /// the shim would resolve a result with an undefined blob.
    #[tokio::test]
    async fn a_resource_refusal_is_an_error_envelope() {
        let (ctx, _signer) = test_context();
        let mut s = Session::new(NappletIdentity::new("pics", "aggregate"), ["relay"]);
        s.on_ready();
        let out = dispatch(
            &ctx,
            &mut s,
            &Envelope::new("resource.bytes")
                .with_id("b1")
                .with_field("url", "blossom:sha256:00"),
        )
        .await;
        let r = serde_json::to_value(&out.envelopes()[0]).unwrap();
        assert_eq!(r["type"], "resource.bytes.error");
        assert_eq!(r["id"], "b1");
        assert_eq!(r["error"], "blocked-by-policy");
        assert!(r["message"].as_str().unwrap().contains("not granted"));

        // And the other domains keep the registry's result-with-error shape.
        let out = dispatch(&ctx, &mut s, &Envelope::new("mesh.info").with_id("i1")).await;
        let r = serde_json::to_value(&out.envelopes()[0]).unwrap();
        assert_eq!(r["type"], "mesh.info.result");
        assert!(r["error"].is_string());
    }

    /// What `needs_session` says is stateless must leave the session as it
    /// found it — a host runs those against a snapshot and throws it away.
    #[tokio::test]
    async fn stateless_calls_do_not_change_the_session() {
        let (ctx, _signer) = test_context();
        let mut s = Session::new(
            NappletIdentity::new("chat", "aggregate"),
            ["relay", "mesh", "outbox", "resource", "identity"],
        );
        s.on_ready();
        s.subscribe("keep", vec![nostr::Filter::new()]);
        let before = s.clone();
        for (t, fields) in [
            ("relay.query", json!({"filters": {"kinds": [1]}})),
            (
                "relay.publish",
                json!({"event": {"kind": 1, "content": "x"}}),
            ),
            ("mesh.info", json!({})),
            (
                "mesh.publish",
                json!({"event": {"kind": 1, "content": "x"}}),
            ),
            ("outbox.query", json!({"filters": {"kinds": [1]}})),
            ("outbox.resolveRelays", json!({"target": {}})),
            ("resource.info", json!({})),
            ("identity.getPublicKey", json!({})),
        ] {
            let mut e = Envelope::new(t).with_id("x");
            for (k, v) in fields.as_object().unwrap() {
                e = e.with_field(k.clone(), v.clone());
            }
            assert!(!needs_session(&e), "{t} was marked stateful");
            let _ = dispatch(&ctx, &mut s, &e).await;
            assert_eq!(
                s.subscription_count(),
                before.subscription_count(),
                "{t} changed the session"
            );
        }
        for t in [
            "shell.ready",
            "relay.subscribe",
            "relay.close",
            "mesh.subscribe",
            "mesh.close",
            "outbox.subscribe",
            "outbox.close",
        ] {
            assert!(needs_session(&Envelope::new(t)), "{t} was marked stateless");
        }
    }

    fn session(granted: &[&str]) -> Session {
        Session::new(NappletIdentity::new("chat", "aggregate"), granted.to_vec())
    }

    /// A runtime that also implements `relay`, so the paths that only exist for
    /// a non-handshake capability can be exercised before one is wired up.
    fn session_with_relay(granted: &[&str]) -> Session {
        Session::with_implemented(
            NappletIdentity::new("chat", "aggregate"),
            granted.to_vec(),
            ["shell", "relay"],
        )
    }

    fn ready() -> Envelope {
        Envelope::new("shell.ready")
    }

    /// The whole handshake, against the JSON in NAP-SHELL.
    #[tokio::test]
    async fn the_handshake_answers_ready_with_init() {
        let (ctx, _signer) = test_context();
        let mut s = session(&[]);
        let out = dispatch(&ctx, &mut s, &ready()).await;
        let sent = out.envelopes();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            serde_json::to_value(&sent[0]).unwrap(),
            json!({
                "type": "shell.init",
                // Everything this build implements, sorted — not what the
                // napplet was granted. See `Session::available_domains`.
                "capabilities": {"domains": s.available_domains()},
                "services": []
            })
        );
        assert!(s.is_established());
    }

    /// "The runtime MUST send shell.init exactly once per napplet lifecycle."
    #[tokio::test]
    async fn init_is_sent_exactly_once_however_often_ready_arrives() {
        let (ctx, _signer) = test_context();
        let mut s = session(&[]);
        assert_eq!(dispatch(&ctx, &mut s, &ready()).await.envelopes().len(), 1);
        for _ in 0..5 {
            assert!(
                dispatch(&ctx, &mut s, &ready())
                    .await
                    .envelopes()
                    .is_empty(),
                "a duplicate shell.ready resent the environment"
            );
        }
    }

    /// "MUST NOT service capability calls for a napplet whose session has not
    /// been established by the handshake."
    #[tokio::test]
    async fn a_capability_call_before_the_handshake_is_refused() {
        let (ctx, _signer) = test_context();
        let mut s = session_with_relay(&["relay"]);
        let call = Envelope::new("relay.publish").with_id("x1");
        let out = dispatch(&ctx, &mut s, &call).await;

        let sent = out.envelopes();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].msg_type, "relay.publish.result");
        assert_eq!(sent[0].id.as_deref(), Some("x1"));
        assert!(sent[0].field("error").is_some());
        assert!(
            !s.is_established(),
            "a refused call must not establish anything"
        );
    }

    /// The permission boundary. The API is in the napplet's namespace and
    /// `supports()` says so, and the call is still refused — which is the whole
    /// point of moving the check behind the call.
    #[tokio::test]
    async fn an_ungranted_capability_is_refused_after_the_handshake() {
        let (ctx, _signer) = test_context();
        let mut s = session_with_relay(&[]);
        dispatch(&ctx, &mut s, &ready()).await;

        let call = Envelope::new("relay.publish").with_id("x1");
        let sent = dispatch(&ctx, &mut s, &call).await.envelopes().to_vec();
        assert_eq!(sent.len(), 1);
        let error = sent[0].field("error").unwrap().as_str().unwrap();
        assert!(error.contains("relay"), "unhelpful error: {error}");
        // The error model: a result carrying `error` carries nothing else.
        assert_eq!(sent[0].fields.len(), 1);

        // And the napplet was told the API exists, so this is a refusal it can
        // act on rather than a runtime that cannot do relay at all.
        assert!(s.offers("relay"));
        assert!(s.available_domains().contains(&"relay".to_string()));
    }

    /// A granted, implemented domain with no handler is this crate's bug, not
    /// the napplet's — but it must still fail closed rather than fall through.
    /// (`relay` has no handler yet, so a granted call lands on that path.)
    #[tokio::test]
    async fn an_unwired_capability_fails_closed() {
        let (ctx, _signer) = test_context();
        let mut s = session_with_relay(&["relay"]);
        dispatch(&ctx, &mut s, &ready()).await;

        let call = Envelope::new("relay.publish").with_id("x1");
        let sent = dispatch(&ctx, &mut s, &call).await.envelopes().to_vec();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].field("error").is_some());
    }

    /// A grant the runtime does not implement is invisible: the environment
    /// never advertises it, so a call is an unrecognized type rather than a
    /// refusal that would confirm the domain exists.
    #[tokio::test]
    async fn a_grant_for_an_unimplemented_domain_stays_silent() {
        let (ctx, _signer) = test_context();
        // `storage` is a real NAP domain this build has not implemented.
        let mut s = session(&["storage"]);
        dispatch(&ctx, &mut s, &ready()).await;
        let call = Envelope::new("storage.setItem").with_id("x1");
        assert_eq!(dispatch(&ctx, &mut s, &call).await, Outcome::Ignore);
    }

    /// NIP-5D's forward-compatibility rule: a napplet built against a newer
    /// registry must not be met with errors on every unknown call.
    #[tokio::test]
    async fn an_unrecognized_domain_is_ignored_in_silence() {
        let (ctx, _signer) = test_context();
        let mut s = session(&[]);
        dispatch(&ctx, &mut s, &ready()).await;
        for msg_type in ["future.thing", "storage.setItem", "nonsense", ""] {
            let call = Envelope::new(msg_type).with_id("x1");
            assert_eq!(
                dispatch(&ctx, &mut s, &call).await,
                Outcome::Ignore,
                "{msg_type} should have been ignored"
            );
        }
    }

    /// `shell.supports` is answered locally by the napplet from its cached
    /// environment. Over the wire it is not a message at all.
    #[tokio::test]
    async fn shell_supports_is_not_a_wire_message() {
        let (ctx, _signer) = test_context();
        let mut s = session(&[]);
        dispatch(&ctx, &mut s, &ready()).await;
        let call = Envelope::new("shell.supports")
            .with_id("x1")
            .with_field("domain", "relay");
        assert!(dispatch(&ctx, &mut s, &call).await.envelopes().is_empty());
    }

    /// A result arriving from the napplet is not something to service.
    #[tokio::test]
    async fn an_inbound_result_is_ignored() {
        let (ctx, _signer) = test_context();
        let mut s = session(&[]);
        let reflected = Envelope::new("shell.init.result").with_id("x1");
        assert_eq!(dispatch(&ctx, &mut s, &reflected).await, Outcome::Ignore);
    }

    /// The environment is scoped per napplet: a session's offered domains never
    /// depend on what another napplet was granted.
    #[tokio::test]
    async fn the_environment_is_scoped_to_one_napplet() {
        let (ctx, _signer) = test_context();
        let mut granted = session(&["shell"]);
        let mut ungranted = session(&[]);
        let a = dispatch(&ctx, &mut granted, &ready()).await.envelopes()[0].clone();
        let b = dispatch(&ctx, &mut ungranted, &ready()).await.envelopes()[0].clone();
        assert_eq!(a.field("capabilities"), b.field("capabilities"));
        assert_eq!(
            a.field("capabilities").unwrap()["domains"],
            json!(granted.available_domains())
        );
    }
}
