//! The update extensions over links ([`super::extensions`]):
//! `napplet-updates`, `nsite-updates` and `myco-release` work the same way,
//! described here for napplets. A release is its signed announcement and
//! file event plus the APK; one received is kept for the release check, and
//! the APK is fetched when the phone carries the update
//! ([`ExtensionSource`]).
//!
//! **Pushing.** When this device moves an installed napplet to a newer
//! version, the runtime calls [`Neighbors::share_update`]: in every
//! neighborhood it shares `napplet-updates` in, the manifest is kept in
//! scope, the version's files are put in scope (read from the device's own
//! store, not copied), and the manifest is published like any neighborhood
//! event, travelling the neighborhood's publish hops.
//!
//! **Receiving.** A napplet manifest arriving in a neighborhood is kept and
//! passed on like any neighborhood event. Where this device gets
//! `napplet-updates`, it is also handed to the runtime's [`UpdateSink`]. If the
//! napplet is installed here and the version is newer, the runtime fetches
//! the files from that neighborhood's links ([`NeighborhoodSource`]); once
//! pinned, this device shares it onward in turn. A device that doesn't have
//! the napplet keeps the manifest in the neighborhood (an app store reading
//! the neighborhood sees that someone around holds it) and passes nothing
//! on.

use std::sync::Arc;

use async_trait::async_trait;
use nostr::{Event, PublicKey};

use super::extensions::{MYCO_RELEASE, NAPPLET_UPDATES, NSITE_UPDATES};
use super::id::NeighborhoodId;
use super::store::Record;
use super::Neighbors;

/// What the runtime does with an update a linked peer shared: neighborhood,
/// the extension it came under, manifest.
pub type UpdateSink = Arc<dyn Fn(NeighborhoodId, &'static str, Event) + Send + Sync>;

/// Zapstore's release announcement and file event: what `myco-release`
/// carries, with the APK.
const RELEASE_ANNOUNCEMENT: u16 = 30063;
const RELEASE_FILE: u16 = 3063;

/// The update extension an event kind travels under, if any: napplet
/// manifests under `napplet-updates`, nsite manifests under `nsite-updates`.
pub fn extension_for(kind: u16) -> Option<&'static str> {
    if kind == myco_napplet_runtime::KIND_ROOT || kind == myco_napplet_runtime::KIND_NAMED {
        Some(NAPPLET_UPDATES)
    } else if kind == nsite_deck::KIND_ROOT || kind == nsite_deck::KIND_NAMED {
        Some(NSITE_UPDATES)
    } else if kind == RELEASE_ANNOUNCEMENT || kind == RELEASE_FILE {
        Some(MYCO_RELEASE)
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
            if !self.scoped.has(n, &manifest) {
                if let Err(e) = self.scoped.add_shared(n, manifest.clone()).await {
                    tracing::warn!(error = %e, "could not keep an update manifest");
                    continue;
                }
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

    /// A neighborhood event just kept: if it is an update under an extension
    /// this device gets in `n`, hand it to the runtime.
    pub(super) fn offer_update(&self, record: &Record, n: NeighborhoodId, event: &Event) {
        let Some(extension) = extension_for(event.kind.as_u16()) else {
            return;
        };
        let Some(sink) = self.on_update.get() else {
            return;
        };
        if record.policy.gets(extension) {
            sink(n, extension, event.clone());
        }
    }
}

/// The napplets this device holds — its manifest stored and its files in
/// storage, installed or not — exposed to linked neighbors in each
/// neighborhood where it shares `napplet-updates`. Nothing is pushed: a
/// neighbor's backlog request is answered with the manifests, and its blob
/// request with their files (`index.html` and icon). Nothing else in
/// storage is reachable this way.
pub struct HeldNapplets {
    /// Manifests: kept and cached, read as one.
    pub relay: Arc<dyn nsite_deck::RelayBackend>,
    /// Files: kept and cached, read as one.
    pub blobs: Arc<dyn nsite_deck::BlobStore>,
}

/// The napplet manifest kinds that name a slot.
fn napplet_kinds() -> [nostr::Kind; 2] {
    [
        nostr::Kind::from(myco_napplet_runtime::KIND_ROOT),
        nostr::Kind::from(myco_napplet_runtime::KIND_NAMED),
    ]
}

impl HeldNapplets {
    /// Napplet manifests matching `filters` whose entry file is here.
    pub async fn manifests(&self, filters: &[nostr::Filter]) -> Vec<Event> {
        let narrowed: Vec<nostr::Filter> = filters
            .iter()
            .filter_map(|f| {
                let kinds: Vec<nostr::Kind> = match &f.kinds {
                    Some(asked) => napplet_kinds()
                        .into_iter()
                        .filter(|k| asked.contains(k))
                        .collect(),
                    None => napplet_kinds().to_vec(),
                };
                (!kinds.is_empty()).then(|| f.clone().kinds(kinds))
            })
            .collect();
        if narrowed.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for event in self.relay.query(&narrowed).await.unwrap_or_default() {
            let Ok(m) = myco_napplet_runtime::NappletManifest::from_event(event.clone()) else {
                continue;
            };
            if self.blobs.has(&m.artifact).await {
                out.push(event);
            }
        }
        out
    }

    /// A file of a napplet held here: its `index.html` or icon.
    pub async fn file(&self, sha256: &str) -> Option<Vec<u8>> {
        if !self.names(sha256).await {
            return None;
        }
        self.blobs.get(sha256).await.ok().flatten()
    }

    /// Whether a file of a napplet held here is in storage, without reading it.
    pub async fn has_file(&self, sha256: &str) -> bool {
        self.names(sha256).await && self.blobs.has(sha256).await
    }

    /// Whether some held napplet's manifest names `sha256` as its entry or
    /// icon.
    async fn names(&self, sha256: &str) -> bool {
        let all = [nostr::Filter::new().kinds(napplet_kinds())];
        self.relay
            .query(&all)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|e| myco_napplet_runtime::NappletManifest::from_event(e).ok())
            .any(|m| m.artifact == sha256 || m.icon.as_ref().is_some_and(|i| i.sha256 == sha256))
    }
}

impl Neighbors {
    /// Expose the napplets this device holds. Set once by the runtime.
    pub fn set_held(&self, held: HeldNapplets) {
        let _ = self.held.set(Arc::new(held));
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
            let mut found = neighbors.link_backlog(record.id, &filters).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_update_kind_has_its_extension() {
        assert_eq!(
            extension_for(myco_napplet_runtime::KIND_NAMED),
            Some(NAPPLET_UPDATES)
        );
        assert_eq!(
            extension_for(myco_napplet_runtime::KIND_ROOT),
            Some(NAPPLET_UPDATES)
        );
        assert_eq!(extension_for(nsite_deck::KIND_NAMED), Some(NSITE_UPDATES));
        assert_eq!(extension_for(nsite_deck::KIND_ROOT), Some(NSITE_UPDATES));
        assert_eq!(extension_for(30063), Some(MYCO_RELEASE));
        assert_eq!(extension_for(3063), Some(MYCO_RELEASE));
        // A napplet snapshot names no slot to update; a note is no update.
        assert_eq!(extension_for(myco_napplet_runtime::KIND_SNAPSHOT), None);
        assert_eq!(extension_for(1), None);
    }
}
