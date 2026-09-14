//! One module per NAP capability domain.
//!
//! A NAP is one capability contract: `NAP-SHELL` is the handshake, `NAP-RELAY`
//! proxies relay reads and writes, `NAP-INTENT` opens another napplet by role.
//! Each is transport-neutral in the registry; what lands here is the runtime
//! half of the web projection, reached through [`crate::dispatch`].
//!
//! `NAP-MESH` is Myco's own — the one capability with no standard equivalent,
//! specified in the registry's form so it can be proposed there.

pub mod identity;
pub mod mesh;
pub mod relay;
pub mod shell;

use crate::seams::Envelope;
use crate::session::Session;

/// Every frame a session should receive for an arriving event, across the
/// domains that subscribe: `relay.event` and `mesh.event`.
///
/// Called for every event this device accepts — its own publishes and anything
/// carried here from a peer — so a subscription behaves the same whichever
/// side of the mesh an event came from. Empty when nothing matches, which is
/// the common case and deliberately cheap.
pub fn deliveries_for(session: &Session, event: &nostr::Event) -> Vec<Envelope> {
    let mut out = relay::deliveries_for(session, event);
    out.extend(mesh::deliveries_for(session, event));
    out
}
