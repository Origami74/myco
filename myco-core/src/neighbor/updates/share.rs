//! Sending: a new version this device moved to is published into the
//! neighborhoods where it shares the extension.

use std::sync::Arc;

use nostr::Event;

use super::super::Neighbors;

impl Neighbors {
    /// Serve shared files from the device's own store. Set once.
    pub fn set_kept_blobs(&self, kept: Arc<dyn nsite_deck::BlobStore>) {
        let _ = self.scoped.kept.set(kept);
    }

    /// Share a new version under `extension`: in every neighborhood this
    /// device shares it in, keep the manifest and `files` in scope and
    /// publish the manifest like any neighborhood event — to every link
    /// carrying it, with the neighborhood's publish hops. Returns in how
    /// many neighborhoods it was sent.
    pub async fn share_update(&self, extension: &str, manifest: Event, files: &[String]) -> usize {
        let mut sent = 0;
        for record in self.records() {
            if !record.policy.shares(extension) {
                continue;
            }
            let n = record.id;
            if let Err(e) = self.scoped.share_kept(n, files) {
                tracing::warn!(error = %e, "could not put an update's files in scope");
                continue;
            }
            // Kept as this device's own, even if it was pulled here before:
            // given to peers asking from now on.
            if let Err(e) = self.scoped.add_shared(n, manifest.clone()).await {
                tracing::warn!(error = %e, "could not keep an update manifest");
                continue;
            }
            let hops = record.policy.publish_hops;
            if hops > 0 {
                self.forward(n, manifest.clone(), None, hops - 1);
                sent += 1;
            }
        }
        tracing::info!(event = %manifest.id, extension, neighborhoods = sent, "update shared");
        sent
    }
}
