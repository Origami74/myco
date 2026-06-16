//! Sync + import: pull a manifest + its blobs from a [`PeerSource`], verify every
//! blob's sha256, and mirror them into the local relay + Blossom so the gateway
//! can serve direct. Mirrors the §5.2 pull sequence in
//! `docs/design/nsite-layer.md`; v0 has no version dirs / atomic swap.
//!
//! The push/propagator half (`FanoutSink`) is P3 — not driven here.

use sha2::{Digest, Sha256};

use crate::host::SiteAddr;
use crate::model::Manifest;
use crate::seams::{BlobStore, PeerSource, RelayBackend};

/// Outcome of a single-site sync — maps onto the FFI `SiteStatus.state`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOutcome {
    /// Manifest stored and all referenced blobs present locally.
    Ready,
    /// No reachable source had the manifest.
    Unreachable,
    /// Manifest found but a blob was missing or failed verification — the sync
    /// aborted and the manifest was **not** activated (so a retry is clean).
    Incomplete { present: usize, total: usize },
}

/// Lowercase-hex sha256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Verify an event's id + signature, then store it. Returns whether it was
/// accepted as the newest in its replaceable slot. Rejects on bad signature —
/// the self-authenticating guarantee is what makes any source trustworthy.
pub async fn verify_and_store_event(
    relay: &dyn RelayBackend,
    event: nostr::Event,
) -> anyhow::Result<bool> {
    event
        .verify()
        .map_err(|e| anyhow::anyhow!("event signature/id verification failed: {e}"))?;
    relay.store_event(event).await
}

/// Import an already-verified, externally-authored site: store each blob (keyed
/// by sha256, verified) and then the signed manifest. Blobs first, manifest last
/// — so a half-import never makes the site "ready". `blobs_by_hash` must cover
/// every hash the manifest references.
pub async fn import_site(
    relay: &dyn RelayBackend,
    blobs: &dyn BlobStore,
    manifest_event: nostr::Event,
    blobs_by_hash: &[(String, Vec<u8>)],
) -> anyhow::Result<SyncOutcome> {
    manifest_event
        .verify()
        .map_err(|e| anyhow::anyhow!("manifest verification failed: {e}"))?;
    let manifest = Manifest::from_event(manifest_event.clone())?;

    let mut lookup = std::collections::HashMap::new();
    for (hash, bytes) in blobs_by_hash {
        lookup.insert(hash.to_ascii_lowercase(), bytes);
    }

    let needed: std::collections::HashSet<&str> = manifest.blob_hashes().collect();
    let total = needed.len();
    let mut present = 0usize;
    for hash in &needed {
        if blobs.has(hash).await {
            present += 1;
            continue;
        }
        let Some(bytes) = lookup.get(*hash) else {
            return Ok(SyncOutcome::Incomplete { present, total });
        };
        let got = sha256_hex(bytes);
        if got != *hash {
            anyhow::bail!("blob hash mismatch: manifest {hash}, bytes {got}");
        }
        blobs.put(bytes).await?;
        present += 1;
    }

    relay.store_event(manifest_event).await?;
    Ok(SyncOutcome::Ready)
}

/// Pull a site from a reachable source and mirror it locally (§5.2). Fetches the
/// manifest, then every referenced blob (preferring the local Blossom — blobs are
/// content-addressed, so a blob cached for another site is the same blob),
/// verifies each sha256, and stores the manifest **last**. Any blob miss/mismatch
/// aborts and leaves the manifest unstored.
pub async fn sync_site(
    relay: &dyn RelayBackend,
    blobs: &dyn BlobStore,
    source: &dyn PeerSource,
    addr: &SiteAddr,
) -> anyhow::Result<SyncOutcome> {
    let Some(manifest_event) = source
        .fetch_manifest(&addr.author, addr.d_tag.as_deref())
        .await?
    else {
        return Ok(SyncOutcome::Unreachable);
    };
    manifest_event
        .verify()
        .map_err(|e| anyhow::anyhow!("fetched manifest verification failed: {e}"))?;
    let manifest = Manifest::from_event(manifest_event.clone())?;

    let needed: std::collections::HashSet<String> =
        manifest.blob_hashes().map(|h| h.to_string()).collect();
    let total = needed.len();
    let mut present = 0usize;

    for hash in &needed {
        if blobs.has(hash).await {
            present += 1;
            continue;
        }
        let fetched = source.fetch_blob(hash, &manifest.servers).await?;
        let Some(bytes) = fetched else {
            return Ok(SyncOutcome::Incomplete { present, total });
        };
        let got = sha256_hex(&bytes);
        if got != *hash {
            // Self-authenticating: a mismatch means the source served bad bytes;
            // abort rather than cache corruption.
            return Ok(SyncOutcome::Incomplete { present, total });
        }
        blobs.put(&bytes).await?;
        present += 1;
    }

    // All blobs verified+stored → activate by storing the signed manifest.
    relay.store_event(manifest_event).await?;
    Ok(SyncOutcome::Ready)
}
