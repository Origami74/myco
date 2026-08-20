//! NAP-SHELL — the bootstrap handshake.
//!
//! Two messages resolve a dependency neither side can break alone: the runtime
//! cannot deliver its capability list until the napplet's receiver is live, and
//! the napplet cannot enumerate capabilities until told.
//!
//! ```text
//! -> { "type": "shell.ready" }
//! <- { "type": "shell.init", "capabilities": { "domains": [ … ] }, "services": [] }
//! ```
//!
//! `shell.supports()` and `shell.services()` are **not** wire messages. The
//! napplet caches the environment from `shell.init` and answers them locally,
//! which is why they are absent here — a `shell.supports` arriving over the
//! wire is an unrecognized type, and [`crate::dispatch`] ignores it silently.

use crate::seams::Envelope;
use crate::session::Session;

/// Handle an inbound `shell.*` message.
///
/// Returns the envelopes to send back — at most one, and only ever for the
/// first `shell.ready`.
pub fn handle(session: &mut Session, message: &Envelope) -> Vec<Envelope> {
    if message.action() != "ready" {
        // Every other `shell.*` type is either napplet-local (`supports`,
        // `services`) or unknown. NIP-5D: unrecognized types are ignored.
        return Vec::new();
    }

    // The identity bound here is the one assigned at creation. `shell.ready`
    // carries no payload precisely so there is nothing in it to bind to.
    if !session.on_ready() {
        // A duplicate. No second session, no resent environment.
        return Vec::new();
    }

    vec![init_for(session)]
}

/// The `shell.init` environment for a session: the domains offered to this
/// napplet, and the named services exposed to it.
pub fn init_for(session: &Session) -> Envelope {
    Envelope::new("shell.init")
        .with_field(
            "capabilities",
            serde_json::json!({ "domains": session.offered_domains() }),
        )
        // No named services yet. The field is required, so it is present and
        // empty rather than absent.
        .with_field("services", serde_json::Value::Array(Vec::new()))
}
