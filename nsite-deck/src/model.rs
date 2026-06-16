//! The nsite manifest model: parse a signed Nostr event (kind 15128 root /
//! 35128 named) into the `path -> sha256` map and metadata the gateway serves.
//! See `docs/reference/nostr-kinds.md` for the tag layout.

use std::collections::BTreeMap;

use nostr::{Event, PublicKey};

/// Replaceable root-site manifest — one per pubkey, no `d` tag.
pub const KIND_ROOT: u16 = 15128;
/// Parameterized-replaceable named-site manifest — one per `(pubkey, d-tag)`.
pub const KIND_NAMED: u16 = 35128;

/// The manifest kind implied by whether a `d-tag` is present.
pub fn kind_for(d_tag: Option<&str>) -> u16 {
    match d_tag {
        None => KIND_ROOT,
        Some(_) => KIND_NAMED,
    }
}

/// The cache/single-flight key for a site: `"npub"` (root) or `"npub:dtag"`
/// (named) — the convention used for the manifest slot throughout the design.
pub fn site_key(author_npub: &str, d_tag: Option<&str>) -> String {
    match d_tag {
        None => author_npub.to_string(),
        Some(d) => format!("{author_npub}:{d}"),
    }
}

/// A parsed manifest: the author-signed event plus the `path -> sha256` map and
/// the optional metadata tags.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub event: Event,
    pub author: PublicKey,
    pub kind: u16,
    pub d_tag: Option<String>,
    /// Absolute path (`/index.html`) -> lowercase-hex sha256 of the file bytes.
    pub paths: BTreeMap<String, String>,
    /// `["server", <blossom-url>]` hints — online-fallback only.
    pub servers: Vec<String>,
    pub title: Option<String>,
    pub description: Option<String>,
}

impl Manifest {
    /// Parse a manifest event. Does **not** verify the signature — callers verify
    /// before storing (the store/sync paths do). Returns an error only if the
    /// kind is not a manifest kind.
    pub fn from_event(event: Event) -> anyhow::Result<Self> {
        let kind = event.kind.as_u16();
        if kind != KIND_ROOT && kind != KIND_NAMED {
            anyhow::bail!("event kind {kind} is not an nsite manifest (15128/35128)");
        }

        let mut paths = BTreeMap::new();
        let mut servers = Vec::new();
        let mut d_tag = None;
        let mut title = None;
        let mut description = None;

        for tag in event.tags.iter() {
            let slice = tag.as_slice();
            match slice.first().map(String::as_str) {
                Some("d") => d_tag = slice.get(1).cloned(),
                Some("path") => {
                    if let (Some(path), Some(hash)) = (slice.get(1), slice.get(2)) {
                        if !path.is_empty() && !hash.is_empty() {
                            paths.insert(path.clone(), hash.to_ascii_lowercase());
                        }
                    }
                }
                Some("server") => {
                    if let Some(url) = slice.get(1) {
                        servers.push(url.clone());
                    }
                }
                Some("title") => title = slice.get(1).cloned(),
                Some("description") => description = slice.get(1).cloned(),
                _ => {}
            }
        }

        // Enforce the kind/d-tag invariant from the spec.
        match (kind, &d_tag) {
            (KIND_ROOT, Some(_)) => anyhow::bail!("kind 15128 must not carry a d tag"),
            (KIND_NAMED, None) => anyhow::bail!("kind 35128 requires a d tag"),
            _ => {}
        }

        let author = event.pubkey;
        Ok(Self {
            event,
            author,
            kind,
            d_tag,
            paths,
            servers,
            title,
            description,
        })
    }

    /// Look up the sha256 hex a path maps to.
    pub fn hash_for(&self, path: &str) -> Option<&str> {
        self.paths.get(path).map(String::as_str)
    }

    /// Every distinct blob hash the manifest references.
    pub fn blob_hashes(&self) -> impl Iterator<Item = &str> {
        self.paths.values().map(String::as_str)
    }

    /// `"npub"` / `"npub:dtag"` cache key for this manifest.
    pub fn site_key(&self) -> String {
        use nostr::nips::nip19::ToBech32;
        let npub = self.author.to_bech32().unwrap_or_default();
        site_key(&npub, self.d_tag.as_deref())
    }
}
