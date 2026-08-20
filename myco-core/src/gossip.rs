//! `MeshGossiper` — the [`Gossiper`] hook that fans an nsite's events out to the
//! mesh, implementing the **push** plane of `docs/design/event-gossip.md`.
//!
//! **P2 — multi-hop flood.** An event is pushed to the whole Circle's relays
//! (`ws://<npub>.fips:4870`) — every member, not just direct neighbours, so a
//! peer reachable only multi-hop over the mesh still receives it — carrying a
//! decrementing hop budget, carried in the `MESH` envelope:
//!
//! - **Local origin** (a loopback publish from the in-app nsite) originates at
//!   `mesh_wire::EVENT_TTL`.
//! - **Mesh origin** re-forwards with the budget that rode in,
//!   **except back to the sender** (split-horizon), until the budget runs out.
//!
//! The loop guard is the proxy's own seen-set: the gossiper is only ever called
//! the first time this device sees an id, so a copy arriving via a second path is
//! never re-forwarded — and unlike the store's dedup, that holds even after the
//! event has been GC'd (`docs/design/event-gossip.md` §3–4). Manifest kinds
//! (15128/35128) are excluded — they have their own path
//! (`docs/design/nsite-layer.md` §2.1); everything else is gossip-eligible by
//! default (`docs/design/nsite-permissions.md`).

use std::net::IpAddr;
use std::sync::Arc;

use async_trait::async_trait;
use nostr::Event;

use crate::mesh_relay::{Gossiper, Inbound, Origin};

use crate::content::Content;

/// Fans events to connected Circle peers' relays over the mesh, hop-bounded.
pub struct MeshGossiper {
    content: Arc<Content>,
}

impl MeshGossiper {
    pub fn new(content: Arc<Content>) -> Self {
        Self { content }
    }
}

/// v1 gossip eligibility: everything except nsite manifests (which propagate via
/// their own path). See `docs/design/nsite-permissions.md` (`gossip-kinds`).
fn is_gossip_eligible(kind: u16) -> bool {
    kind != nsite_deck::KIND_ROOT && kind != nsite_deck::KIND_NAMED
}

#[async_trait]
impl Gossiper for MeshGossiper {
    async fn on_event(&self, event: Event, inbound: Inbound) {
        let kind = event.kind.as_u16();
        // File-control messages are private gift wraps. Handle them locally
        // before the normal event-gossip path; their MESH ttl is zero so they
        // are stored and delivered only at the addressed peer.
        self.content.handle_file_event(&event).await;
        // nsite manifests propagate over this same push plane (the relay just stored
        // a newer one), but with an interest-aware download-then-forward policy and
        // the active-version gate. See docs/design/nsite-updates.md §4.
        if kind == nsite_deck::KIND_ROOT || kind == nsite_deck::KIND_NAMED {
            self.content.clone().on_manifest_event(event, inbound).await;
            return;
        }
        if !is_gossip_eligible(kind) {
            return;
        }
        // Effective budget: originate at the default for our own publishes; for a
        // mesh-received event use the TTL it carried (absent => 0 => don't forward).
        let effective = match inbound.origin {
            Origin::Local => crate::mesh_wire::EVENT_TTL,
            Origin::Mesh => inbound.event_ttl.unwrap_or(0),
        };
        // A peer we have not granted multihop writes still gets its events
        // stored and shown here; they simply travel no further through us. A
        // clamp of 0 is how that is expressed (D10).
        let peer_cap = match inbound.sender {
            Some(ip) if !self.content.may_forward_from(ip) => 0,
            _ => crate::mesh_wire::EVENT_TTL,
        };
        let fwd = effective.min(crate::mesh_wire::EVENT_TTL).min(peer_cap);
        if fwd == 0 {
            return;
        }
        let out_ttl = fwd - 1;

        // Build the outbound frame once: a canonical NIP-01 EVENT, with the
        // decremented hop budget carried beside it in the envelope rather than
        // added to the event. Nothing has to be stripped before storing.
        let ev_json = match serde_json::to_value(&event) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "gossip: serialize event failed");
                return;
            }
        };
        let frame = crate::mesh_wire::wrap(
            &crate::mesh_wire::MeshMeta::push(out_ttl),
            serde_json::json!(["EVENT", ev_json]),
        );

        // Fan out to the *whole* Circle over persistent pooled connections (no
        // per-message connect), skipping the peer it came from (split-horizon). Not
        // just direct neighbours: a Circle peer reachable only multi-hop (you've
        // moved apart) must still get the message — the routed dial handles it, an
        // offline member's connect fails fast. See `docs/design/event-gossip.md`.
        for npub in self.content.circle_npubs() {
            let ip = match fips::PeerIdentity::from_npub(&npub) {
                Ok(p) => IpAddr::V6(p.address().to_ipv6()),
                Err(_) => continue,
            };
            if inbound.sender == Some(ip) {
                continue;
            }
            self.content.gossip_to_peer(&npub, frame.clone());
        }
    }

    /// Pull plane: forward the REQ's
    /// filters to connected Circle peers carrying the decremented `req_ttl`,
    /// aggregating their matching events. `exclude` is split-horizon.
    async fn on_req(
        &self,
        filters: Vec<serde_json::Value>,
        meta: crate::mesh_wire::MeshMeta,
        exclude: Option<IpAddr>,
    ) -> Vec<Event> {
        self.content.pull_from_peers(filters, meta, exclude).await
    }

    fn on_local_subscribe(&self, key: &str, filters: Vec<serde_json::Value>) {
        self.content.record_local_sub(key.to_string(), filters);
    }

    fn on_local_unsubscribe(&self, key: &str) {
        self.content.drop_local_sub(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local() -> Inbound {
        Inbound {
            origin: Origin::Local,
            event_ttl: None,
            sender: None,
        }
    }

    #[test]
    fn manifests_are_not_gossiped_chat_is() {
        assert!(!is_gossip_eligible(nsite_deck::KIND_ROOT));
        assert!(!is_gossip_eligible(nsite_deck::KIND_NAMED));
        assert!(is_gossip_eligible(9)); // chat
        assert!(is_gossip_eligible(1)); // notes
    }

    /// With no connected peers, fan-out is a no-op (no panic, returns promptly).
    #[tokio::test]
    async fn no_connected_peers_is_noop() {
        let dir = std::env::temp_dir().join(format!("myco-gossip-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());
        let gossiper = MeshGossiper::new(content);

        let keys = nostr::Keys::generate();
        let ev = nostr::EventBuilder::new(nostr::Kind::from(9u16), "nobody around")
            .tags([nostr::Tag::identifier("mesh".to_string())])
            .sign_with_keys(&keys)
            .unwrap();
        // A mesh-origin event whose TTL is already spent must not be re-forwarded.
        gossiper
            .on_event(
                ev.clone(),
                Inbound {
                    origin: Origin::Mesh,
                    event_ttl: Some(0),
                    sender: None,
                },
            )
            .await;
        // A local origin with no peers is also a clean no-op.
        gossiper.on_event(ev, local()).await;

        let _ = std::fs::remove_dir_all(&dir);
    }
}
