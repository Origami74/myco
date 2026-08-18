//! The four trait seams `nsite-deck` reaches everything else through. It names no
//! concrete relay, blob store, or transport — these are the boundaries the host
//! app (`myco-core`) plugs `myco-relay` / `myco-blossom` / a FIPS-or-IP source
//! into. See `docs/design/nsite-layer.md` §1.
//!
//! - **storage:** [`RelayBackend`] (manifest events) + [`BlobStore`] (blobs by sha256).
//! - **transport:** [`PeerSource`] (pull) + [`FanoutSink`] (push) — the latter is a
//!   P3 no-op stub here.

use async_trait::async_trait;
use nostr::{Event, Filter, Kind, PublicKey};

/// Stores and queries events (a plain NIP-01 relay). The default is the embedded
/// `myco-relay`; an alternate impl forwards to any other relay — Citrine on the
/// same device, a strfry on the LAN — which is the whole point of keeping this
/// surface to verbs every relay already speaks.
///
/// Replaceable semantics (newest per `(kind, author)` for 15128, per
/// `(kind, author, d-tag)` for 35128) are the backend's responsibility, because
/// they are NIP-01's, not ours.
#[async_trait]
pub trait RelayBackend: Send + Sync {
    /// Store an already-signed, already-verified event.
    ///
    /// Idempotent: a relay dedups by id and collapses replaceable slots on its
    /// own, so re-publishing is a no-op and there is nothing useful to report
    /// back. Deliberately no "was it new?" answer — NIP-01's `OK true` covers
    /// accept and duplicate alike, so no arbitrary backend could supply one, and
    /// nothing should depend on it (`reference/thinning-custom-relay.md`, D2).
    async fn publish(&self, event: Event) -> anyhow::Result<()>;

    /// Events matching any of `filters` — a `REQ` read to `EOSE`.
    ///
    /// Takes real [`nostr::Filter`]s rather than a hand-rolled subset, so the
    /// query surface is exactly NIP-01's: `since`, `until`, ids, and general tag
    /// matching all work, and a remote backend needs no translation layer.
    async fn query(&self, filters: &[Filter]) -> anyhow::Result<Vec<Event>>;
}

/// Operations with no NIP-01 expression, which therefore cannot be required of
/// an arbitrary backend.
///
/// The embedded store implements this; a remote relay generally will not. Where
/// it is absent the UI reports the operation as unavailable rather than silently
/// doing nothing (`reference/thinning-custom-relay.md`, D4).
#[async_trait]
pub trait AdminBackend: Send + Sync {
    /// Drop every stored event (the dev/test wipe; `WipeStores`).
    async fn wipe(&self) -> anyhow::Result<()>;
}

/// The newest event in a replaceable slot: `kind` + `author`, plus the `d-tag`
/// for parameterized-replaceable (35128). `d_tag = None` selects the root (15128)
/// slot.
///
/// A helper over [`RelayBackend::query`] rather than a backend method: it is an
/// ordinary filter, and every relay can already answer it.
pub async fn newest_in_slot(
    relay: &dyn RelayBackend,
    kind: u16,
    author: &PublicKey,
    d_tag: Option<&str>,
) -> anyhow::Result<Option<Event>> {
    let mut filter = Filter::new()
        .kind(Kind::from(kind))
        .author(*author)
        .limit(1);
    if let Some(d) = d_tag {
        filter = filter.identifier(d);
    }
    let mut found = relay.query(&[filter]).await?;
    // A backend is free to return more than `limit` suggests, and ordering is not
    // guaranteed, so pick the newest here rather than trusting the first row.
    found.sort_by_key(|e| std::cmp::Reverse(e.created_at));
    // `identifier()` matches a `#d` tag, which for a root manifest is absent
    // entirely — filter that out, or a named site could answer for the root slot.
    Ok(found
        .into_iter()
        .find(|e| event_d_tag(e).as_deref() == d_tag))
}

/// The `d` tag value of an event, if it has one.
fn event_d_tag(event: &Event) -> Option<String> {
    event.tags.iter().find_map(|t| {
        let s = t.as_slice();
        (s.first().map(String::as_str) == Some("d"))
            .then(|| s.get(1).cloned())
            .flatten()
    })
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
