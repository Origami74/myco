//! The shell ↔ Rust link: framing, and the rule that keeps a napplet from
//! impersonating the shell.
//!
//! Two very different conversations share one channel. The shell has its own
//! small control traffic — "I have mounted", "here are the bytes to load" — and
//! it also relays NAP messages to and from the napplet in its iframe. If both
//! travelled untagged, a napplet could send `{"type":"…"}` shaped like shell
//! control traffic and the runtime would act on it.
//!
//! So every frame names its `channel`. The shell is trusted code and tags what
//! it forwards; the runtime believes the tag *only* because the shell, not the
//! napplet, is what writes it — the napplet's own messages arrive at the shell
//! through `postMessage` and are wrapped, never passed through.
//!
//! ```text
//! -> { "channel": "shell",   "action": "mounted" }
//! <- { "channel": "shell",   "action": "load", "artifact": "…", "sandbox": "allow-scripts" }
//! -> { "channel": "napplet", "message": { "type": "shell.ready" } }
//! <- { "channel": "napplet", "message": { "type": "shell.init", … } }
//! ```
//!
//! ## Why the artifact travels over the channel
//!
//! The napplet's bytes are pushed to the shell and assigned to `srcdoc`. They
//! are never served at the shell's origin. Serving them there would make them
//! reachable by URL, and anything navigating to that URL would be running the
//! napplet *as* the shell origin — the origin the capability channel is scoped
//! to, with the shell's storage attached. Pushing them keeps the napplet's only
//! home the opaque origin inside the iframe.

use crate::artifact::SrcdocArtifact;
use crate::seams::Envelope;

/// A frame from the shell to the runtime.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "channel", rename_all = "lowercase")]
pub enum ToRuntime {
    /// The shell's own control traffic. Only the shell can send this, because
    /// only the shell writes the `channel` tag.
    Shell { action: ShellAction },
    /// A NAP message relayed from the napplet's iframe, after the shell
    /// verified `MessageEvent.source` against the frame it created.
    Napplet { message: Envelope },
}

/// What the shell tells the runtime about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellAction {
    /// The shell page is up and its listeners are installed. The runtime
    /// answers with [`ToShell::Load`].
    Mounted,
}

/// A frame from the runtime to the shell.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "channel", rename_all = "lowercase")]
pub enum ToShell {
    /// Create the iframe with these bytes and this sandbox.
    Shell {
        action: String,
        artifact: String,
        sandbox: String,
    },
    /// Deliver a NAP message into the napplet's iframe.
    Napplet { message: Envelope },
}

impl ToShell {
    /// The command that puts a verified napplet on screen.
    pub fn load(artifact: &SrcdocArtifact) -> Self {
        Self::Shell {
            action: "load".to_string(),
            artifact: artifact.as_str().to_string(),
            sandbox: SrcdocArtifact::SANDBOX.to_string(),
        }
    }

    /// Wrap a NAP message for delivery to the napplet.
    pub fn to_napplet(message: Envelope) -> Self {
        Self::Napplet { message }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{assemble, Injection};
    use serde_json::json;

    #[test]
    fn the_shell_reports_mounting() {
        let frame: ToRuntime =
            serde_json::from_value(json!({"channel": "shell", "action": "mounted"})).unwrap();
        assert_eq!(
            frame,
            ToRuntime::Shell {
                action: ShellAction::Mounted
            }
        );
    }

    #[test]
    fn a_relayed_napplet_message_keeps_the_nap_wire_format_intact() {
        let frame: ToRuntime = serde_json::from_value(
            json!({"channel": "napplet", "message": {"type": "shell.ready"}}),
        )
        .unwrap();
        let ToRuntime::Napplet { message } = frame else {
            panic!("expected a napplet frame");
        };
        assert_eq!(message.msg_type, "shell.ready");
    }

    /// The impersonation this framing exists to prevent: a napplet's own
    /// message, whatever it says, is wrapped by the shell and can only ever
    /// arrive on the napplet channel. It never reaches shell control traffic.
    #[test]
    fn a_napplet_cannot_forge_shell_control_traffic() {
        // What a hostile napplet postMessages to its parent, hoping to be
        // treated as the shell.
        let forged = json!({"channel": "shell", "action": "mounted"});

        // The shell wraps what it received; it never forwards it raw. The
        // inner `channel` is now just a field of a NAP message, and the frame
        // is a napplet frame — the outer tag is the only one that routes.
        let wrapped =
            serde_json::from_value::<ToRuntime>(json!({"channel": "napplet", "message": forged}));

        // It does not even parse: a NAP message must carry a `type`, and this
        // one carries a `channel`. A relay that fails to parse is dropped.
        assert!(
            wrapped.is_err(),
            "a forged control frame must not parse as anything"
        );

        // And a well-formed napplet message stays on the napplet channel
        // however much it looks like control traffic.
        let disguised = serde_json::from_value::<ToRuntime>(json!({
            "channel": "napplet",
            "message": {"type": "shell.ready", "action": "mounted"}
        }))
        .unwrap();
        assert!(
            matches!(disguised, ToRuntime::Napplet { .. }),
            "the outer channel tag is what routes, not anything inside the message"
        );
    }

    #[test]
    fn the_load_command_carries_the_sandbox_it_must_be_created_with() {
        let artifact = assemble("<p>hi</p>", &Injection::default());
        let ToShell::Shell {
            action,
            artifact: bytes,
            sandbox,
        } = ToShell::load(&artifact)
        else {
            panic!("expected a shell command");
        };
        assert_eq!(action, "load");
        assert_eq!(sandbox, "allow-scripts");
        assert!(!sandbox.contains("allow-same-origin"));
        assert_eq!(bytes, artifact.as_str());
    }

    #[test]
    fn frames_round_trip_through_json() {
        let artifact = assemble("<p>hi</p>", &Injection::default());
        for frame in [
            ToShell::load(&artifact),
            ToShell::to_napplet(Envelope::new("shell.init")),
        ] {
            let json = serde_json::to_value(&frame).unwrap();
            assert_eq!(serde_json::from_value::<ToShell>(json).unwrap(), frame);
        }
    }
}
