//! `MeshGossiper` — the [`Gossiper`] hook that fans an nsite's events out to the
//! mesh, implementing the **push** plane of `docs/design/event-gossip.md`.
//!
//! **P2 — multi-hop flood.** An event is pushed to connected Circle peers'
//! relays (`ws://[fd00::peer]:4869`) carrying a decrementing `event-ttl`:
//!
//! - **Local origin** (a loopback publish from the in-app nsite) originates at
//!   `DEFAULT_EVENT_TTL`.
//! - **Mesh origin** re-forwards with the TTL that rode in (`event-ttl`),
//!   **except back to the sender** (split-horizon), until the budget runs out.
//!
//! The loop guard is the relay's id-dedup: the gossiper is only ever called for an
//! event that was *new* to the store, so a copy arriving via a second path is
//! never re-forwarded (`docs/design/event-gossip.md` §3–4). Manifest kinds
//! (15128/35128) are excluded — they have their own path
//! (`docs/design/nsite-layer.md` §2.1); everything else is gossip-eligible by
//! default (`docs/design/nsite-permissions.md`).

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nostr::Event;

use myco_relay::server::{Gossiper, Inbound, Origin};

use crate::content::Content;

/// Per-peer fan-out timeout — generous, since a first mesh contact includes BLE
/// session setup. An unreachable peer just fails its own send; others proceed.
const FANOUT_TIMEOUT: Duration = Duration::from_secs(10);

/// Hop budget this device stamps on its **own** events (experimental default).
const DEFAULT_EVENT_TTL: u8 = 3;
/// Clamp on any TTL we'll honour/forward, so a peer can't set a huge value and
/// turn us into a flood amplifier (`docs/design/event-gossip.md` §3).
const MAX_EVENT_TTL: u8 = 3;

/// Fans events to connected Circle peers' relays over the mesh, with `event-ttl`.
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
        if !is_gossip_eligible(event.kind.as_u16()) {
            return;
        }
        // Effective budget: originate at the default for our own publishes; for a
        // mesh-received event use the TTL it carried (absent => 0 => don't forward).
        let effective = match inbound.origin {
            Origin::Local => DEFAULT_EVENT_TTL,
            Origin::Mesh => inbound.event_ttl.unwrap_or(0),
        };
        let fwd = effective.min(MAX_EVENT_TTL);
        if fwd == 0 {
            return;
        }
        let out_ttl = fwd - 1;

        let peers = self.content.connected_circle_npubs();
        let sends = peers.into_iter().filter_map(|npub| {
            let peer = match fips::PeerIdentity::from_npub(&npub) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(npub, error = %e, "gossip: bad peer npub, skipping");
                    return None;
                }
            };
            let ip = peer.address().to_ipv6();
            // Split-horizon: never forward back to the peer it came from.
            if inbound.sender == Some(IpAddr::V6(ip)) {
                return None;
            }
            let url = format!("ws://[{ip}]:4869");
            let event = event.clone();
            Some(async move {
                if !crate::ip_source::publish_event(&url, &event, out_ttl, FANOUT_TIMEOUT).await {
                    tracing::debug!(npub, "gossip: peer relay unreachable");
                }
            })
        });
        futures_util::future::join_all(sends).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local() -> Inbound {
        Inbound { origin: Origin::Local, event_ttl: None, sender: None }
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
            .on_event(ev.clone(), Inbound { origin: Origin::Mesh, event_ttl: Some(0), sender: None })
            .await;
        // A local origin with no peers is also a clean no-op.
        gossiper.on_event(ev, local()).await;

        let _ = std::fs::remove_dir_all(&dir);
    }
}
