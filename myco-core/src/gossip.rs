//! `MeshGossiper` — the [`Gossiper`] hook that fans an nsite's events out to the
//! mesh, implementing the **push** plane of `docs/design/event-gossip.md`.
//!
//! **P1 — 1-hop circle fan-out.** When an event originates on *this* device (a
//! loopback [`Origin::Local`] publish from the in-app nsite), it is pushed to every
//! currently-connected Circle peer's relay (`ws://[fd00::peer]:4869`). Events
//! received *from* a mesh peer ([`Origin::Mesh`]) are stored and shown but **not**
//! re-forwarded — that is the multi-hop flood (`event-ttl`), which lands in P2.
//!
//! Manifest kinds (15128/35128) are excluded: they have their own
//! store-and-forward path (`docs/design/nsite-layer.md` §2.1). Everything else is
//! gossip-eligible by default (`docs/design/nsite-permissions.md` — v1 default-allow).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nostr::Event;

use myco_relay::server::{Gossiper, Origin};

use crate::content::Content;

/// Per-peer fan-out timeout — generous, since a first mesh contact includes BLE
/// session setup. An unreachable peer just fails its own send; others proceed.
const FANOUT_TIMEOUT: Duration = Duration::from_secs(10);

/// Fans local-origin events to connected Circle peers' relays over the mesh.
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
    async fn on_event(&self, event: Event, origin: Origin) {
        // P1: 1-hop. Only fan out events that originated on this device; do not
        // re-forward mesh-received ones (multi-hop `event-ttl` is P2).
        if origin != Origin::Local || !is_gossip_eligible(event.kind.as_u16()) {
            return;
        }
        let peers = self.content.connected_circle_npubs();
        if peers.is_empty() {
            return;
        }

        let sends = peers.into_iter().filter_map(|npub| {
            let peer = match fips::PeerIdentity::from_npub(&npub) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(npub, error = %e, "gossip: bad peer npub, skipping");
                    return None;
                }
            };
            let url = format!("ws://[{}]:4869", peer.address().to_ipv6());
            let event = event.clone();
            Some(async move {
                if !crate::ip_source::publish_event(&url, &event, FANOUT_TIMEOUT).await {
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
        gossiper.on_event(ev, Origin::Local).await;

        let _ = std::fs::remove_dir_all(&dir);
    }
}
