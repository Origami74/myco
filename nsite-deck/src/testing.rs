//! Test/dev helpers behind the `testing` feature: a throwaway-key **signed
//! manifest generator** (so host tests exercise real signature verification, and
//! a dev side-load has something to import) plus in-memory [`RelayBackend`] /
//! [`BlobStore`] fakes. Off by default — never compiled into a release build.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use nostr::{Event, EventBuilder, Keys, Kind, PublicKey, Tag};

use crate::model::{KIND_NAMED, KIND_ROOT};
use crate::seams::{BlobStore, ManifestFilter, RelayBackend};
use crate::sync::sha256_hex;

/// A generated, signed test nsite: its manifest event and the blob bytes it
/// references (keyed by sha256 hex).
pub struct TestSite {
    pub author: PublicKey,
    pub manifest: Event,
    pub blobs: Vec<(String, Vec<u8>)>,
}

/// Build a signed manifest (kind 15128 if `d_tag` is `None`, else 35128) over the
/// given `(path, bytes)` files, with a throwaway key. The site is *valid*: the
/// event is signed and every path tag carries the true sha256 of its bytes.
pub fn build_test_site(
    files: &[(&str, &[u8])],
    d_tag: Option<&str>,
    title: Option<&str>,
) -> TestSite {
    build_test_site_with_keys(&Keys::generate(), files, d_tag, title)
}

/// As [`build_test_site`] but with a caller-supplied key (so a site's author npub
/// is stable across calls).
pub fn build_test_site_with_keys(
    keys: &Keys,
    files: &[(&str, &[u8])],
    d_tag: Option<&str>,
    title: Option<&str>,
) -> TestSite {
    let mut tags: Vec<Tag> = Vec::new();
    if let Some(d) = d_tag {
        tags.push(Tag::identifier(d.to_string()));
    }
    let mut blobs = Vec::new();
    for (path, bytes) in files {
        let hash = sha256_hex(bytes);
        tags.push(Tag::parse(["path", path, hash.as_str()]).expect("path tag"));
        blobs.push((hash, bytes.to_vec()));
    }
    if let Some(t) = title {
        tags.push(Tag::parse(["title", t]).expect("title tag"));
    }

    let kind = if d_tag.is_some() {
        KIND_NAMED
    } else {
        KIND_ROOT
    };
    let manifest = EventBuilder::new(Kind::from(kind), "")
        .tags(tags)
        .sign_with_keys(keys)
        .expect("sign manifest");

    TestSite {
        author: keys.public_key(),
        manifest,
        blobs,
    }
}

// --- in-memory seam fakes ---

type Slot = (u16, [u8; 32], Option<String>);

/// An in-memory [`RelayBackend`] with replaceable-slot semantics.
#[derive(Default)]
pub struct MemRelay {
    events: Mutex<HashMap<Slot, Event>>,
}

impl MemRelay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.events.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn event_d_tag(event: &Event) -> Option<String> {
    event.tags.iter().find_map(|t| {
        let s = t.as_slice();
        (s.first().map(String::as_str) == Some("d"))
            .then(|| s.get(1).cloned())
            .flatten()
    })
}

fn slot_of(event: &Event) -> Slot {
    let kind = event.kind.as_u16();
    let d = if kind == KIND_NAMED {
        event_d_tag(event)
    } else {
        None
    };
    (kind, event.pubkey.to_bytes(), d)
}

#[async_trait]
impl RelayBackend for MemRelay {
    async fn store_event(&self, event: Event) -> anyhow::Result<bool> {
        let slot = slot_of(&event);
        let mut map = self.events.lock().unwrap();
        let accept = match map.get(&slot) {
            Some(existing) => event.created_at > existing.created_at,
            None => true,
        };
        if accept {
            map.insert(slot, event);
        }
        Ok(accept)
    }

    async fn get_manifest(
        &self,
        kind: u16,
        author: &PublicKey,
        d_tag: Option<&str>,
    ) -> anyhow::Result<Option<Event>> {
        let slot = (kind, author.to_bytes(), d_tag.map(str::to_string));
        Ok(self.events.lock().unwrap().get(&slot).cloned())
    }

    async fn query(&self, filter: &ManifestFilter) -> anyhow::Result<Vec<Event>> {
        let map = self.events.lock().unwrap();
        let mut out: Vec<Event> = map
            .values()
            .filter(|e| filter.kinds.is_empty() || filter.kinds.contains(&e.kind.as_u16()))
            .filter(|e| filter.authors.is_empty() || filter.authors.contains(&e.pubkey))
            .filter(|e| {
                filter.d_tags.is_empty()
                    || event_d_tag(e).is_some_and(|d| filter.d_tags.contains(&d))
            })
            .cloned()
            .collect();
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        if let Some(limit) = filter.limit {
            out.truncate(limit);
        }
        Ok(out)
    }

    async fn wipe(&self) -> anyhow::Result<()> {
        self.events.lock().unwrap().clear();
        Ok(())
    }
}

/// An in-memory content-addressed [`BlobStore`].
#[derive(Default)]
pub struct MemBlobs {
    blobs: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemBlobs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.blobs.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl BlobStore for MemBlobs {
    async fn has(&self, sha256_hex: &str) -> bool {
        self.blobs
            .lock()
            .unwrap()
            .contains_key(&sha256_hex.to_ascii_lowercase())
    }

    async fn get(&self, sha256_hex: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(self
            .blobs
            .lock()
            .unwrap()
            .get(&sha256_hex.to_ascii_lowercase())
            .cloned())
    }

    async fn put(&self, bytes: &[u8]) -> anyhow::Result<String> {
        let hash = sha256_hex(bytes);
        self.blobs
            .lock()
            .unwrap()
            .insert(hash.clone(), bytes.to_vec());
        Ok(hash)
    }

    async fn wipe(&self) -> anyhow::Result<()> {
        self.blobs.lock().unwrap().clear();
        Ok(())
    }
}
