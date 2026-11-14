//! `MeshGossiper` — the relay hub's hook for what arrives on the mesh relay
//! that Myco itself acts on. Events in general are not gossiped any more:
//! neighborhoods carry them (`crate::neighbor`). What is left here is Myco's
//! own traffic between app peers:
//!
//! - **File-control messages** (private gift wraps, addressed to one peer).
//! - **nsite manifests** (15128/35128) and **napplet manifests**
//!   (15129/35129): interest-aware download-then-forward, so an app update
//!   reaches app peers (`docs/design/nsite/nsite-updates.md` §4,
//!   `docs/design/napplet/napplet-runtime.md` §7.3).

use std::sync::Arc;

use async_trait::async_trait;
use nostr::Event;

use crate::mesh_relay::{Gossiper, Inbound};

use crate::content::Content;

/// Acts on file messages and app manifests arriving from app peers.
pub struct MeshGossiper {
    content: Arc<Content>,
}

impl MeshGossiper {
    pub fn new(content: Arc<Content>) -> Self {
        Self { content }
    }
}

/// Whether `kind` is a napplet manifest that names an updatable address — the
/// replaceable root (`15129`) or addressable named (`35129`) kind. A snapshot
/// (`5129`) is an immutable build nothing installs by address, so it is not.
fn is_napplet_manifest(kind: u16) -> bool {
    kind == myco_napplet_runtime::KIND_ROOT || kind == myco_napplet_runtime::KIND_NAMED
}

#[async_trait]
impl Gossiper for MeshGossiper {
    async fn on_event(&self, event: Event, inbound: Inbound) {
        let kind = event.kind.as_u16();
        // File-control messages are private gift wraps, handled here and
        // never passed on.
        self.content.handle_file_event(&event).await;
        if kind == nsite_deck::KIND_ROOT || kind == nsite_deck::KIND_NAMED {
            self.content.clone().on_manifest_event(event, inbound).await;
            return;
        }
        // Napplet manifests take the same policy with the napplet's checks in
        // front: signed by the installed author, newer than the pinned version,
        // bytes verified before the pin moves.
        if is_napplet_manifest(kind) {
            self.content.on_napplet_manifest_event(event, inbound).await;
        }
    }
}
