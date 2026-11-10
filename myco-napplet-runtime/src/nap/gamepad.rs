//! NAP-GAMEPAD subscription policy. The trusted shell owns the native reader;
//! its control command is emitted only after this session's capability gate.
use crate::{dispatch::Outcome, seams::Envelope, session::Session};

/// One subscription per window, bounded independently of relay subscriptions.
#[derive(Debug, Clone, Default)]
pub struct GamepadSubscription {
    pub(crate) active: bool,
    window: Option<std::time::Instant>,
    changes: u32,
}

impl GamepadSubscription {
    fn admit(&mut self) -> bool {
        let now = std::time::Instant::now();
        if self
            .window
            .is_none_or(|start| now.duration_since(start).as_secs() >= 1)
        {
            self.window = Some(now);
            self.changes = 0;
        }
        self.changes = self.changes.saturating_add(1);
        self.changes <= 120
    }
}

/// All gamepad messages are fire-and-forget, including policy refusals.
pub fn blocked() -> Envelope {
    Envelope::new("gamepad.state")
        .with_field("available", false)
        .with_field("reason", "blocked")
        .with_field("focused", false)
        .with_field("pads", serde_json::json!([]))
}

/// Reject malformed or unknown actions before the ordinary dispatcher gate.
pub fn recognized(message: &Envelope) -> bool {
    matches!(message.action(), "subscribe" | "unsubscribe")
        && message.id.is_none()
        && message.fields.is_empty()
}

pub fn handle(session: &mut Session, message: &Envelope) -> Outcome {
    let enabled = message.action() == "subscribe";
    if !enabled && !session.gamepad.active {
        return Outcome::Ignore;
    }
    // Always admit an unsubscribe: churn must never prevent teardown.
    if enabled && !session.gamepad.admit() {
        return Outcome::Ignore;
    }
    session.gamepad.active = enabled;
    // Repeated subscribe commands intentionally reach the shell: each requires
    // an immediate snapshot, even if no value changed since the previous one.
    Outcome::Gamepad { enabled }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dispatch::dispatch, session::NappletIdentity, testing::test_context};

    #[tokio::test]
    async fn subscriptions_require_handshake_and_grant_and_have_no_result_ids() {
        let (ctx, _) = test_context();
        let mut session = Session::new(NappletIdentity::new("game", "hash"), ["gamepad"]);
        let subscribe = Envelope::new("gamepad.subscribe");
        let refused = dispatch(&ctx, &mut session, &subscribe).await;
        assert_eq!(refused.envelopes(), &[blocked()]);
        session.on_ready();
        assert_eq!(
            dispatch(&ctx, &mut session, &subscribe).await,
            Outcome::Gamepad { enabled: true }
        );
        assert!(session.gamepad.active);
        assert_eq!(
            dispatch(&ctx, &mut session, &subscribe).await,
            Outcome::Gamepad { enabled: true }
        );
        assert_eq!(
            dispatch(&ctx, &mut session, &Envelope::new("gamepad.unsubscribe")).await,
            Outcome::Gamepad { enabled: false }
        );
        assert!(!session.gamepad.active);
        session.set_granted(Vec::<String>::new());
        assert_eq!(
            dispatch(&ctx, &mut session, &subscribe).await.envelopes(),
            &[blocked()]
        );
        assert_eq!(
            dispatch(&ctx, &mut session, &Envelope::new("gamepad.unsubscribe")).await,
            Outcome::Ignore
        );
    }

    #[tokio::test]
    async fn ignores_unknown_and_malformed_messages_and_bounds_churn() {
        let (ctx, _) = test_context();
        let mut session = Session::new(NappletIdentity::new("game", "hash"), ["gamepad"]);
        session.on_ready();
        for message in [
            Envelope::new("gamepad.state"),
            Envelope::new("gamepad.subscribe").with_id("bad"),
        ] {
            assert_eq!(
                dispatch(&ctx, &mut session, &message).await,
                Outcome::Ignore
            );
        }
        for _ in 0..120 {
            assert_eq!(
                dispatch(&ctx, &mut session, &Envelope::new("gamepad.subscribe")).await,
                Outcome::Gamepad { enabled: true }
            );
        }
        assert_eq!(
            dispatch(&ctx, &mut session, &Envelope::new("gamepad.subscribe")).await,
            Outcome::Ignore
        );
        assert_eq!(
            dispatch(&ctx, &mut session, &Envelope::new("gamepad.unsubscribe")).await,
            Outcome::Gamepad { enabled: false }
        );
    }
}
