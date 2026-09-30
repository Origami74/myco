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
        /// Where this window's delivered blobs are served, on the shell's own
        /// origin: `<blobs><sha256>`. Holds a per-window secret, so only the
        /// shell — never the napplet, which does not see this command — can
        /// form the URL.
        blobs: String,
    },
    /// Deliver a NAP message into the napplet's iframe.
    Napplet { message: Envelope },
    /// Tear the window down and open it again — a new session, a fresh
    /// handshake, the napplet's startup calls made over with the grants as
    /// they now stand. Sent when the user changes a grant on the app's sheet:
    /// a live grant covers the *next* call, but a napplet subscribes once at
    /// startup and does not retry a refusal, so a subscription refused before
    /// the switch would otherwise never exist. Handled by the window host,
    /// not the shell page — the shell never reloads in place.
    Relaunch,
    /// Hand this `http(s):` URL to the system browser: a NAP-LINK
    /// `link.open` the host admitted. Handled by the window host, which may
    /// ask the user first; never by the shell page, which does not navigate.
    #[serde(rename = "open-external")]
    OpenExternal { url: String },
    /// Put Myco's install review for this napplet pointer in front of the
    /// user, over the running napplet: a NAP-LINK `link.open` naming another
    /// napplet. Handled by the window host. It **fetches for review** and
    /// nothing more — installing is the user's answer on the sheet.
    #[serde(rename = "review-napplet")]
    ReviewNapplet { pointer: String },
    /// Open (or bring forward) the napplet at `pointer` in its own window,
    /// and bind the NAP-INTENT delivery `token` to that window's session —
    /// the handler another napplet's `intent.invoke` resolved to. Handled by
    /// the window host, which starts the handler's task with the token; the
    /// window that opens (or the one already open, in `onNewIntent`) hands it
    /// back, and the runtime delivers the payload once the napplet listens.
    /// The token names a pending delivery and nothing else: it grants
    /// nothing, and an unknown or expired one binds to nothing.
    #[serde(rename = "open-napplet")]
    OpenNapplet {
        pointer: String,
        title: String,
        token: String,
    },
    /// Open the nsite whose host label is `host`, through Myco's own nsite
    /// opener (`myco://app/<host>`): NAP-INTENT's built-in handler for the
    /// `nsite` archetype. Handled by the window host.
    #[serde(rename = "open-nsite")]
    OpenNsite { host: String },
    /// Ask the user which app should handle a NAP-INTENT request: the
    /// "open with…" chooser, drawn over the calling window. Answered through
    /// the reducer (`answer_intent_chooser`) with `token`; closing it is a
    /// cancel. `candidates` carry the host's keys — the window host is Myco's
    /// own code, never the napplet, which is only told the answer.
    #[serde(rename = "choose-intent-handler")]
    ChooseIntentHandler {
        token: String,
        archetype: String,
        action: String,
        candidates: Vec<ChooserCandidate>,
    },
}

/// One entry in the "open with…" chooser.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChooserCandidate {
    /// The host's key for the handler (`<npub>:<d>`, or a built-in's name).
    pub key: String,
    pub title: String,
    /// The napplet pointer, for the window host to find its icon; empty for
    /// a built-in.
    #[serde(default)]
    pub pointer: String,
}

impl ToShell {
    /// The command that puts a verified napplet on screen.
    pub fn load(artifact: &SrcdocArtifact, blobs: &str) -> Self {
        Self::Shell {
            action: "load".to_string(),
            artifact: artifact.as_str().to_string(),
            sandbox: SrcdocArtifact::SANDBOX.to_string(),
            blobs: blobs.to_string(),
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
            blobs,
        } = ToShell::load(&artifact, "/_blob/t/")
        else {
            panic!("expected a shell command");
        };
        assert_eq!(action, "load");
        assert_eq!(sandbox, "allow-scripts");
        assert!(!sandbox.contains("allow-same-origin"));
        assert_eq!(bytes, artifact.as_str());
        assert_eq!(blobs, "/_blob/t/");
    }

    /// The window host switches on these exact channel names.
    #[test]
    fn host_commands_use_the_channel_names_the_window_host_reads() {
        let v = serde_json::to_value(ToShell::OpenExternal {
            url: "https://example.com".into(),
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"channel": "open-external", "url": "https://example.com"})
        );
        let v = serde_json::to_value(ToShell::ReviewNapplet {
            pointer: "naddr1x".into(),
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"channel": "review-napplet", "pointer": "naddr1x"})
        );
        assert_eq!(
            serde_json::to_value(ToShell::Relaunch).unwrap(),
            serde_json::json!({"channel": "relaunch"})
        );
        assert_eq!(
            serde_json::to_value(ToShell::OpenNapplet {
                pointer: "npub1x:profiles".into(),
                title: "Profiles".into(),
                token: "t1".into(),
            })
            .unwrap(),
            serde_json::json!({"channel": "open-napplet", "pointer": "npub1x:profiles",
                               "title": "Profiles", "token": "t1"})
        );
        assert_eq!(
            serde_json::to_value(ToShell::OpenNsite {
                host: "npub1x".into()
            })
            .unwrap(),
            serde_json::json!({"channel": "open-nsite", "host": "npub1x"})
        );
        assert_eq!(
            serde_json::to_value(ToShell::ChooseIntentHandler {
                token: "t2".into(),
                archetype: "profile".into(),
                action: "open".into(),
                candidates: vec![ChooserCandidate {
                    key: "npub1x:profiles".into(),
                    title: "Profiles".into(),
                    pointer: "npub1x:profiles".into(),
                }],
            })
            .unwrap(),
            serde_json::json!({"channel": "choose-intent-handler", "token": "t2",
                               "archetype": "profile", "action": "open",
                               "candidates": [{"key": "npub1x:profiles", "title": "Profiles",
                                               "pointer": "npub1x:profiles"}]})
        );
    }

    #[test]
    fn frames_round_trip_through_json() {
        let artifact = assemble("<p>hi</p>", &Injection::default());
        for frame in [
            ToShell::load(&artifact, "/_blob/t/"),
            ToShell::to_napplet(Envelope::new("shell.init")),
            ToShell::Relaunch,
            ToShell::OpenExternal {
                url: "https://example.com".into(),
            },
            ToShell::ReviewNapplet {
                pointer: "naddr1x".into(),
            },
        ] {
            let json = serde_json::to_value(&frame).unwrap();
            assert_eq!(serde_json::from_value::<ToShell>(json).unwrap(), frame);
        }
    }
}
