//! Where the runtime fetches an update's or an app's files and manifests
//! from the neighborhoods: [`NeighborhoodSource`] for one neighborhood,
//! [`ExtensionSource`] for every one that carries an extension.

use std::sync::Arc;

use async_trait::async_trait;
use nostr::{Event, PublicKey};

use super::super::extensions::{NAPPLET_UPDATES, NSITE_UPDATES};
use super::super::id::NeighborhoodId;
use super::super::Neighbors;

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

/// A [`nsite_deck::PeerSource`] over every neighborhood where this device
/// gets `extension`: each one's scope here, then its links, until one has
/// it. How Myco finds an app that is not here yet (opening a Library site,
/// a share link, installing a napplet) and the release APK, before the
/// public internet.
pub struct ExtensionSource {
    pub neighbors: std::sync::Weak<Neighbors>,
    pub extension: &'static str,
}

impl ExtensionSource {
    /// The manifest slot `extension` carries for `author` and `d_tag`, as a
    /// filter. `None` for the release, which has no slot to ask by.
    fn slot_filter(&self, author: &PublicKey, d_tag: Option<&str>) -> Option<nostr::Filter> {
        let kind = match (self.extension, d_tag) {
            (NAPPLET_UPDATES, Some(_)) => myco_napplet_runtime::KIND_NAMED,
            (NAPPLET_UPDATES, None) => myco_napplet_runtime::KIND_ROOT,
            (NSITE_UPDATES, Some(_)) => nsite_deck::KIND_NAMED,
            (NSITE_UPDATES, None) => nsite_deck::KIND_ROOT,
            _ => return None,
        };
        let filter = nostr::Filter::new()
            .kind(nostr::Kind::from(kind))
            .author(*author);
        Some(match d_tag {
            Some(d) => filter.identifier(d),
            None => filter,
        })
    }
}

#[async_trait]
impl nsite_deck::PeerSource for ExtensionSource {
    /// The newest signed manifest for the slot: from each neighborhood's
    /// scope here, else asked of the links carrying it.
    async fn fetch_manifest(
        &self,
        author: &PublicKey,
        d_tag: Option<&str>,
    ) -> anyhow::Result<Option<Event>> {
        let Some(neighbors) = self.neighbors.upgrade() else {
            return Ok(None);
        };
        let Some(filter) = self.slot_filter(author, d_tag) else {
            return Ok(None);
        };
        let filters = [filter];
        let mut newest: Option<Event> = None;
        for record in neighbors.records() {
            if !record.policy.gets(self.extension) {
                continue;
            }
            let mut found = neighbors.kept(record.id, &filters).await;
            if found.is_empty() {
                found = neighbors.ask_links(record.id, &filters).await;
            }
            for event in found {
                if event.pubkey != *author || event.verify().is_err() {
                    continue;
                }
                if newest
                    .as_ref()
                    .is_none_or(|n| n.created_at < event.created_at)
                {
                    newest = Some(event);
                }
            }
        }
        Ok(newest)
    }

    async fn fetch_blob(
        &self,
        sha256_hex: &str,
        _servers: &[String],
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let Some(neighbors) = self.neighbors.upgrade() else {
            return Ok(None);
        };
        for record in neighbors.records() {
            if !record.policy.gets(self.extension) {
                continue;
            }
            if let Some(bytes) = neighbors.link_blob(record.id, sha256_hex).await {
                return Ok(Some(bytes));
            }
            if let Some(bytes) = neighbors.blob_from_links(record.id, sha256_hex, None).await {
                return Ok(Some(bytes));
            }
        }
        Ok(None)
    }
}
