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

use async_trait::async_trait;
use nostr::nips::nip19::ToBech32;
use nostr::Event;

use myco_relay::server::{Gossiper, Inbound, Origin};

use crate::content::Content;

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
        let kind = event.kind.as_u16();
        // Pairing signals are point-to-point (dialed straight to the target's
        // relay) — handle them, never gossip. Only act on mesh-delivered ones.
        if kind == crate::content::KIND_PAIR_REQUEST || kind == crate::content::KIND_PAIR_ACCEPT {
            if inbound.origin == Origin::Mesh {
                self.content.handle_pair_event(&event);
                // A peer just accepted our pair request → they're a reachable source
                // *now*. Retry any not-yet-ready downloads from them immediately,
                // rather than waiting for the next connected-peer poll edge.
                if kind == crate::content::KIND_PAIR_ACCEPT {
                    if let Ok(npub) = event.pubkey.to_bech32() {
                        for addr in self.content.retriable_library_addrs() {
                            let content = self.content.clone();
                            let holder = npub.clone();
                            tokio::spawn(async move { content.open_site(addr, Some(holder)).await });
                        }
                    }
                }
            }
            return;
        }
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
            Origin::Local => DEFAULT_EVENT_TTL,
            Origin::Mesh => inbound.event_ttl.unwrap_or(0),
        };
        let fwd = effective.min(MAX_EVENT_TTL);
        if fwd == 0 {
            return;
        }
        let out_ttl = fwd - 1;

        // Build the outbound relay frame once: the canonical event plus the
        // decremented transient `event-ttl` (a top-level field, stripped on store).
        let mut ev_json = match serde_json::to_value(&event) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "gossip: serialize event failed");
                return;
            }
        };
        if let Some(obj) = ev_json.as_object_mut() {
            obj.insert("event-ttl".to_string(), serde_json::json!(out_ttl));
        }
        let frame = serde_json::json!(["EVENT", ev_json]).to_string();

        // Fan out over persistent pooled connections (no per-message connect),
        // skipping the peer it came from (split-horizon).
        for npub in self.content.connected_circle_npubs() {
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

    /// Pull plane (`docs/design/event-gossip.md`, req-ttl): forward the REQ's
    /// filters to connected Circle peers carrying the decremented `req_ttl`,
    /// aggregating their matching events. `exclude` is split-horizon.
    async fn on_req(
        &self,
        filters: Vec<serde_json::Value>,
        req_ttl: u8,
        exclude: Option<IpAddr>,
    ) -> Vec<Event> {
        self.content.pull_from_peers(filters, req_ttl, exclude).await
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
