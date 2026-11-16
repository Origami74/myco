//! The update extensions over links ([`super::extensions`]):
//! `napplet-updates` and `nsite-updates` work the same way, described here
//! for napplets.
//!
//! **Pushing.** When this device moves an installed napplet to a newer
//! version, the runtime calls [`Neighbors::share_update`]: in every
//! neighborhood with `napplet-updates` on, the manifest is kept in scope, the
//! version's files are put in scope (read from the device's own store, not
//! copied), and the manifest is sent with no hops left to each link where
//! the peer has the extension on too.
//!
//! **Receiving.** A napplet manifest arriving on such a link is kept like any
//! neighborhood event, and handed to the runtime's [`UpdateSink`]. If the
//! napplet is installed here and the version is newer, the runtime fetches
//! the files from that neighborhood's links ([`NeighborhoodSource`]); once
//! pinned, this device shares it onward in turn. A device that doesn't have
//! the napplet keeps the manifest in the neighborhood (an app store reading
//! the neighborhood sees that someone around holds it) and passes nothing
//! on.

use std::sync::Arc;

use async_trait::async_trait;
use nostr::{Event, PublicKey};

use super::extensions::{shared, NAPPLET_UPDATES, NSITE_UPDATES};
use super::id::NeighborhoodId;
use super::store::Record;
use super::Neighbors;

/// What the runtime does with an update a linked peer shared: neighborhood,
/// the extension it came under, manifest.
pub type UpdateSink = Arc<dyn Fn(NeighborhoodId, &'static str, Event) + Send + Sync>;

/// The update extension an event kind travels under, if any: napplet
/// manifests under `napplet-updates`, nsite manifests under `nsite-updates`.
pub fn extension_for(kind: u16) -> Option<&'static str> {
    if kind == myco_napplet_runtime::KIND_ROOT || kind == myco_napplet_runtime::KIND_NAMED {
        Some(NAPPLET_UPDATES)
    } else if kind == nsite_deck::KIND_ROOT || kind == nsite_deck::KIND_NAMED {
        Some(NSITE_UPDATES)
    } else {
        None
    }
}

impl Neighbors {
    /// Where updates linked peers share are handed. Set once by the runtime.
    pub fn on_update(&self, sink: UpdateSink) {
        let _ = self.on_update.set(sink);
    }

    /// Serve shared files from the device's own store. Set once.
    pub fn set_kept_blobs(&self, kept: Arc<dyn nsite_deck::BlobStore>) {
        let _ = self.scoped.kept.set(kept);
    }

    /// Share a new version under `extension`: in every neighborhood with it
    /// on, keep the manifest and `files` in scope, and send the manifest to
    /// each link whose peer has it on too. Returns how many links were sent
    /// to.
    pub async fn share_update(&self, extension: &str, manifest: Event, files: &[String]) -> usize {
        let mut sent = 0;
        for record in self.records() {
            if !record.policy.has(extension) {
                continue;
            }
            let n = record.id;
            if let Err(e) = self.scoped.share_kept(n, files) {
                tracing::warn!(error = %e, "could not put an update's files in scope");
                continue;
            }
            if !self.scoped.has(n, &manifest) {
                if let Err(e) = self.scoped.add_shared(n, manifest.clone()).await {
                    tracing::warn!(error = %e, "could not keep an update manifest");
                    continue;
                }
            }
            let Some(links) = &self.links else {
                continue;
            };
            for link in links.carrying(n) {
                if !shared(&record, &link, n, extension) {
                    continue;
                }
                sent += 1;
                let manifest = manifest.clone();
                let links = links.clone();
                tokio::spawn(async move {
                    match super::wire::send(&link, n, 0, &manifest).await {
                        Ok(_) => links.mark_seen(&link.id),
                        Err(e) => {
                            tracing::debug!(link = %link.id, error = %e, "update push failed")
                        }
                    }
                });
            }
        }
        tracing::info!(event = %manifest.id, extension, links = sent, "update shared");
        sent
    }

    /// A neighborhood event just kept from `from_link`: if it is an update
    /// under an extension shared on that link, hand it to the runtime.
    pub(super) fn offer_update(
        &self,
        record: &Record,
        n: NeighborhoodId,
        from_link: &str,
        event: &Event,
    ) {
        let Some(extension) = extension_for(event.kind.as_u16()) else {
            return;
        };
        let Some(sink) = self.on_update.get() else {
            return;
        };
        let Some(link) = self
            .links
            .as_ref()
            .and_then(|l| l.all().into_iter().find(|l| l.id == from_link))
        else {
            return;
        };
        if shared(record, &link, n, extension) {
            sink(n, extension, event.clone());
        }
    }
}

/// A [`nsite_deck::PeerSource`] over one neighborhood: blobs come from its
/// scope here, then from the links carrying it. Manifests are not asked for:
/// an update arrives with its manifest.
pub struct NeighborhoodSource {
    pub neighbors: Arc<Neighbors>,
    pub n: NeighborhoodId,
}

#[async_trait]
impl nsite_deck::PeerSource for NeighborhoodSource {
    async fn fetch_manifest(
        &self,
        _author: &PublicKey,
        _d_tag: Option<&str>,
    ) -> anyhow::Result<Option<Event>> {
        Ok(None)
    }

    async fn fetch_blob(
        &self,
        sha256_hex: &str,
        _servers: &[String],
    ) -> anyhow::Result<Option<Vec<u8>>> {
        if let Some(bytes) = self.neighbors.link_blob(self.n, sha256_hex).await {
            return Ok(Some(bytes));
        }
        Ok(self
            .neighbors
            .blob_from_links(self.n, sha256_hex, None)
            .await)
    }
}
