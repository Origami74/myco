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
pub mod outbox;
pub mod relay;
pub mod resource;
pub mod shell;

use nostr::{Event, Filter};

use crate::dispatch::NapContext;
use crate::seams::Envelope;
use crate::session::Session;

/// The domains that keep live subscriptions, each delivering `<domain>.event`.
const SUBSCRIBING_DOMAINS: [&str; 3] = ["relay", "mesh", "outbox"];

/// Every frame a session should receive for an arriving event, across the
/// domains that subscribe: `relay.event`, `mesh.event` and `outbox.event`.
///
/// Called for every event this device accepts — its own publishes and anything
/// carried here from a peer — so a subscription behaves the same whichever
/// side of the mesh an event came from. Empty when nothing matches, which is
/// the common case and deliberately cheap.
pub fn deliveries_for(session: &Session, event: &Event) -> Vec<Envelope> {
    SUBSCRIBING_DOMAINS
        .iter()
        .flat_map(|domain| deliveries_in(session, domain, event))
        .collect()
}

/// The `<domain>.event` frames a session should receive for `event` in one
/// domain: one per matching subscription, gated on the session being
/// established and still granted the domain (see
/// [`Session::matching_subscriptions_in`]).
pub(crate) fn deliveries_in(session: &Session, domain: &str, event: &Event) -> Vec<Envelope> {
    session
        .matching_subscriptions_in(domain, event)
        .into_iter()
        .map(|sub_id| event_frame(domain, sub_id, event))
        .collect()
}

/// One `<domain>.event` frame: the spec's `RelayEventResult`, `{ event }`.
pub(crate) fn event_frame(domain: &str, sub_id: impl Into<String>, event: &Event) -> Envelope {
    Envelope::new(format!("{domain}.event"))
        .with_field("subId", sub_id.into())
        .with_field("result", relay::result_of(event))
}

/// The first half every subscribing domain shares: register the filters,
/// then answer what the local relay already holds.
///
/// Registered **before** the read, so an event landing between the two is
/// delivered by the live path rather than falling through the gap. A napplet
/// may see it twice; Nostr subscriptions are at-least-once and a duplicate id
/// is something every client already handles, whereas a missed event is
/// invisible. Returns the backlog frames, or the reason the subscription
/// closed before it started — each domain wraps that in its own `.closed`.
///
/// What follows differs per domain — which relays are pulled behind the
/// backlog, and whether an `eose` marks its end — and stays with the domain.
pub(crate) async fn open_subscription(
    ctx: &NapContext,
    session: &mut Session,
    domain: &str,
    sub_id: &str,
    filters: Vec<Filter>,
) -> Result<Vec<Envelope>, String> {
    session.subscribe_in(domain, sub_id, filters.clone());
    let events = ctx
        .relay
        .query(&filters)
        .await
        .map_err(|e| format!("query failed: {e}"))?;
    Ok(events
        .iter()
        .map(|event| event_frame(domain, sub_id, event))
        .collect())
}
