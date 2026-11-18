//! The napplets installed here, exposed to linked neighbors where this
//! device shares `napplet-updates`: answered when asked, never pushed.

use std::sync::Arc;

use nostr::{Event, PublicKey};

use super::super::Neighbors;

/// The napplets installed on this device — in the Library, with the
/// version's manifest stored and its files in storage — exposed to linked
/// neighbors in each neighborhood where it shares `napplet-updates`.
/// Nothing is pushed: a neighbor's backlog request is answered with the
/// manifests, and its blob request with their `index.html` and icon. Nothing
/// else in storage is reachable this way: not a napplet only cached here,
/// nor any other file of an installed one.
pub struct HeldNapplets {
    /// Manifests: kept and cached, read as one.
    pub relay: Arc<dyn nsite_deck::RelayBackend>,
    /// Files: kept and cached, read as one.
    pub blobs: Arc<dyn nsite_deck::BlobStore>,
    /// The napplet slots installed now.
    pub installed: Installed,
}

/// The napplet slots installed now, read when asked: author and `d` tag
/// (`None` for an author's root napplet).
pub type Installed = Arc<dyn Fn() -> Vec<(PublicKey, Option<String>)> + Send + Sync>;

/// The napplet manifest kinds that name a slot.
fn napplet_kinds() -> [nostr::Kind; 2] {
    [
        nostr::Kind::from(myco_napplet_runtime::KIND_ROOT),
        nostr::Kind::from(myco_napplet_runtime::KIND_NAMED),
    ]
}

impl HeldNapplets {
    /// The installed napplets' manifests among `events`.
    fn installed_among(&self, events: Vec<Event>) -> Vec<myco_napplet_runtime::NappletManifest> {
        let installed = (self.installed)();
        events
            .into_iter()
            .filter_map(|e| myco_napplet_runtime::NappletManifest::from_event(e).ok())
            .filter(|m| {
                installed
                    .iter()
                    .any(|(author, d)| *author == m.author && *d == m.d_tag)
            })
            .collect()
    }

    /// Installed napplets' manifests matching `filters` whose entry file is
    /// here.
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
        let found = self.relay.query(&narrowed).await.unwrap_or_default();
        let mut out = Vec::new();
        for m in self.installed_among(found) {
            if self.blobs.has(&m.artifact).await {
                out.push(m.event);
            }
        }
        out
    }

    /// A file of a napplet installed here: its `index.html` or icon.
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

    /// Whether some installed napplet's manifest names `sha256` as its
    /// entry or icon.
    async fn names(&self, sha256: &str) -> bool {
        let all = [nostr::Filter::new().kinds(napplet_kinds())];
        let found = self.relay.query(&all).await.unwrap_or_default();
        self.installed_among(found)
            .into_iter()
            .any(|m| m.artifact == sha256 || m.icon.as_ref().is_some_and(|i| i.sha256 == sha256))
    }
}

impl Neighbors {
    /// Expose the napplets this device holds. Set once by the runtime.
    pub fn set_held(&self, held: HeldNapplets) {
        let _ = self.held.set(Arc::new(held));
    }
}
