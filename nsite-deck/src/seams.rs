//! The four trait seams `nsite-deck` reaches everything else through. It names no
//! concrete relay, blob store, or transport — these are the boundaries the host
//! app (`myco-core`) plugs `myco-relay` / `myco-blossom` / a FIPS-or-IP source
//! into. See `docs/design/nsite-layer.md` §1.
//!
//! - **storage:** [`RelayBackend`] (manifest events) + [`BlobStore`] (blobs by sha256).
//! - **transport:** [`PeerSource`] (pull) + [`FanoutSink`] (push) — the latter is a
//!   P3 no-op stub here.

use async_trait::async_trait;
use nostr::{Event, PublicKey};

/// A basic event filter — the entire query surface Myco's gateway and discovery
/// use (`{kinds, authors, #d, limit}`, `docs/design/nsite-layer.md` §2.1). No
/// general tag/`since`/`until` filtering: manifests are tiny and addressed by
/// `(kind, author, d-tag)`.
#[derive(Debug, Clone, Default)]
pub struct ManifestFilter {
    pub kinds: Vec<u16>,
    pub authors: Vec<PublicKey>,
    pub d_tags: Vec<String>,
    pub limit: Option<usize>,
}

/// Stores and queries manifest events (a plain NIP-01 store). The default is the
/// embedded `myco-relay`; an alternate impl could forward to a local relay app
/// (e.g. Citrine). Replaceable semantics — keep only the newest event per
/// `(kind, author)` for 15128 and per `(kind, author, d-tag)` for 35128 — are
/// the backend's responsibility.
#[async_trait]
pub trait RelayBackend: Send + Sync {
    /// Store an already-signed, already-verified event. Returns `true` if it was
    /// accepted as the newest in its (replaceable) slot, `false` if an
    /// equal-or-newer event already won that slot.
    async fn store_event(&self, event: Event) -> anyhow::Result<bool>;

    /// The newest manifest for a replaceable slot: `kind` + `author`, plus the
    /// `d-tag` for parameterized-replaceable (35128). `d_tag = None` selects the
    /// root (15128) slot.
    async fn get_manifest(
        &self,
        kind: u16,
        author: &PublicKey,
        d_tag: Option<&str>,
    ) -> anyhow::Result<Option<Event>>;

    /// Events matching the basic filter — used by discovery ("nsites around me").
    async fn query(&self, filter: &ManifestFilter) -> anyhow::Result<Vec<Event>>;

    /// Drop every stored event (the dev/test wipe; `WipeStores`).
    async fn wipe(&self) -> anyhow::Result<()>;
}

/// A content-addressed blob store keyed by lowercase-hex sha256. Blobs are
/// immutable and self-authenticating (the hash *is* the identity), so the store
/// verifies `sha256(bytes) == name` on read and write and needs no signatures.
#[async_trait]
pub trait BlobStore: Send + Sync {
    /// Existence check (a `HEAD`), used by the "all referenced blobs present?"
    /// gate before serving a site.
    async fn has(&self, sha256_hex: &str) -> bool;

    /// Fetch a blob by its sha256 hex; `None` if absent. Implementations verify
    /// the bytes hash to `sha256_hex` and treat a mismatch as absent/corrupt.
    async fn get(&self, sha256_hex: &str) -> anyhow::Result<Option<Vec<u8>>>;

    /// Store bytes, keyed by `sha256(bytes)`; returns the sha256 hex.
    async fn put(&self, bytes: &[u8]) -> anyhow::Result<String>;

    /// Drop every blob (the dev/test wipe; `WipeStores`).
    async fn wipe(&self) -> anyhow::Result<()>;
}

/// Pull / reconcile a manifest + blobs from some reachable source. The default
/// (P2) is an IP source over public relays/Blossom; the FIPS-peer source (P3)
/// implements the same trait. The sync engine calls these; it does not care how
/// the bytes arrive.
#[async_trait]
pub trait PeerSource: Send + Sync {
    /// Fetch the author's signed manifest event (root or named).
    async fn fetch_manifest(
        &self,
        author: &PublicKey,
        d_tag: Option<&str>,
    ) -> anyhow::Result<Option<Event>>;

    /// Fetch one blob by sha256 hex. `servers` are the manifest's `["server",…]`
    /// hints (online fallback); a FIPS source ignores them.
    async fn fetch_blob(
        &self,
        sha256_hex: &str,
        servers: &[String],
    ) -> anyhow::Result<Option<Vec<u8>>>;
}

/// Push an accepted manifest to connected peers (the propagator's fanout). The
/// relay itself never fans out. **P3** — the default impl is a no-op so
/// `nsite-deck` compiles against all four seams in P2.
#[async_trait]
pub trait FanoutSink: Send + Sync {
    async fn broadcast(&self, _event: &Event) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A `FanoutSink` that drops everything — the P2 default until the P3 propagator
/// lands.
pub struct NoopFanout;

#[async_trait]
impl FanoutSink for NoopFanout {}
