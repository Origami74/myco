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

use crate::nap;
use crate::seams::Envelope;
use crate::session::Session;

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

/// Route one inbound message for one napplet's session.
pub fn dispatch(session: &mut Session, message: &Envelope) -> Outcome {
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
            return Outcome::Reply(vec![
                message.to_error("session not established: send shell.ready first")
            ]);
        }
        if !session.offers(domain) {
            return Outcome::Reply(vec![message.to_error(format!(
                "capability {domain} was not granted to this napplet"
            ))]);
        }
    }

    match domain {
        "shell" => Outcome::Reply(nap::shell::handle(session, message)),
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
    use serde_json::json;

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
    #[test]
    fn the_handshake_answers_ready_with_init() {
        let mut s = session(&[]);
        let out = dispatch(&mut s, &ready());
        let sent = out.envelopes();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            serde_json::to_value(&sent[0]).unwrap(),
            json!({
                "type": "shell.init",
                "capabilities": {"domains": ["shell"]},
                "services": []
            })
        );
        assert!(s.is_established());
    }

    /// "The runtime MUST send shell.init exactly once per napplet lifecycle."
    #[test]
    fn init_is_sent_exactly_once_however_often_ready_arrives() {
        let mut s = session(&[]);
        assert_eq!(dispatch(&mut s, &ready()).envelopes().len(), 1);
        for _ in 0..5 {
            assert!(
                dispatch(&mut s, &ready()).envelopes().is_empty(),
                "a duplicate shell.ready resent the environment"
            );
        }
    }

    /// "MUST NOT service capability calls for a napplet whose session has not
    /// been established by the handshake."
    #[test]
    fn a_capability_call_before_the_handshake_is_refused() {
        let mut s = session_with_relay(&["relay"]);
        let call = Envelope::new("relay.publish").with_id("x1");
        let out = dispatch(&mut s, &call);

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

    /// A granted call still refused after the handshake, because the grant is
    /// absent. The napplet's prelude installs only granted domains, so getting
    /// here means it went around its own namespace.
    #[test]
    fn an_ungranted_capability_is_refused_after_the_handshake() {
        let mut s = session_with_relay(&[]);
        dispatch(&mut s, &ready());

        let call = Envelope::new("relay.publish").with_id("x1");
        let sent = dispatch(&mut s, &call).envelopes().to_vec();
        assert_eq!(sent.len(), 1);
        let error = sent[0].field("error").unwrap().as_str().unwrap();
        assert!(error.contains("relay"), "unhelpful error: {error}");
        // The error model: a result carrying `error` carries nothing else.
        assert_eq!(sent[0].fields.len(), 1);
    }

    /// A granted, implemented domain with no handler is this crate's bug, not
    /// the napplet's — but it must still fail closed rather than fall through.
    #[test]
    fn an_unwired_capability_fails_closed() {
        let mut s = session_with_relay(&["relay"]);
        dispatch(&mut s, &ready());

        let call = Envelope::new("relay.publish").with_id("x1");
        let sent = dispatch(&mut s, &call).envelopes().to_vec();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].field("error").is_some());
    }

    /// A grant the runtime does not implement is invisible: the environment
    /// never advertises it, so a call is an unrecognized type rather than a
    /// refusal that would confirm the domain exists.
    #[test]
    fn a_grant_for_an_unimplemented_domain_stays_silent() {
        let mut s = session(&["relay"]);
        dispatch(&mut s, &ready());
        let call = Envelope::new("relay.publish").with_id("x1");
        assert_eq!(dispatch(&mut s, &call), Outcome::Ignore);
    }

    /// NIP-5D's forward-compatibility rule: a napplet built against a newer
    /// registry must not be met with errors on every unknown call.
    #[test]
    fn an_unrecognized_domain_is_ignored_in_silence() {
        let mut s = session(&[]);
        dispatch(&mut s, &ready());
        for msg_type in ["future.thing", "relay.publish", "nonsense", ""] {
            let call = Envelope::new(msg_type).with_id("x1");
            assert_eq!(
                dispatch(&mut s, &call),
                Outcome::Ignore,
                "{msg_type} should have been ignored"
            );
        }
    }

    /// `shell.supports` is answered locally by the napplet from its cached
    /// environment. Over the wire it is not a message at all.
    #[test]
    fn shell_supports_is_not_a_wire_message() {
        let mut s = session(&[]);
        dispatch(&mut s, &ready());
        let call = Envelope::new("shell.supports")
            .with_id("x1")
            .with_field("domain", "relay");
        assert!(dispatch(&mut s, &call).envelopes().is_empty());
    }

    /// A result arriving from the napplet is not something to service.
    #[test]
    fn an_inbound_result_is_ignored() {
        let mut s = session(&[]);
        let reflected = Envelope::new("shell.init.result").with_id("x1");
        assert_eq!(dispatch(&mut s, &reflected), Outcome::Ignore);
    }

    /// The environment is scoped per napplet: a session's offered domains never
    /// depend on what another napplet was granted.
    #[test]
    fn the_environment_is_scoped_to_one_napplet() {
        let mut granted = session(&["shell"]);
        let mut ungranted = session(&[]);
        let a = dispatch(&mut granted, &ready()).envelopes()[0].clone();
        let b = dispatch(&mut ungranted, &ready()).envelopes()[0].clone();
        assert_eq!(a.field("capabilities"), b.field("capabilities"));
        assert_eq!(
            a.field("capabilities").unwrap()["domains"],
            json!(["shell"])
        );
    }
}
