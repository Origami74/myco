//! Manifest → blobs → verify → a napplet that may be rendered.
//!
//! Nothing here trusts anything: not a gateway, not a Blossom server, not the
//! local store, and least of all the napplet. Every byte that reaches the
//! output was hashed here and matched against a signature verified here. Any
//! failure rejects the load outright — there is no partial render, and no path
//! from a failed resolve to an iframe.
//!
//! The order matters, and it is NIP-5D's. Signature first, because an unsigned
//! manifest's tags are whatever an attacker wrote. Then the manifest's own
//! conformance, including the aggregate — free, and it decides identity. Blobs
//! last, because they are the expensive part.

use nsite_deck::sync::sha256_hex;

use crate::error::{NappletError, NappletErrorCode, Result};
use crate::manifest::NappletManifest;
use crate::seams::BlobStore;

/// A fully verified napplet, ready to be injected as `iframe.srcdoc`.
#[derive(Debug, Clone)]
pub struct ResolvedNapplet {
    /// The `d` identifier, or `""` for root and snapshot manifests.
    pub d_tag: String,
    /// The verified aggregate — with `d_tag`, the napplet's identity.
    pub aggregate: String,
    /// The verified `/index.html`, decoded as UTF-8.
    pub index_html: String,
    /// The parsed manifest the bytes came from.
    pub manifest: NappletManifest,
}

impl ResolvedNapplet {
    /// The `(dTag, aggregateHash)` tuple, computed from verified bytes.
    pub fn identity(&self) -> (&str, &str) {
        (&self.d_tag, &self.aggregate)
    }
}

/// Resolve a napplet from a candidate manifest event and a blob source.
///
/// The `blobs` seam is untrusted — whatever it returns is re-hashed here — so
/// it can be the local Blossom store, a mesh peer, or an online fetch without
/// changing the guarantee.
pub async fn resolve(event: nostr::Event, blobs: &dyn BlobStore) -> Result<ResolvedNapplet> {
    event.verify().map_err(|e| {
        NappletError::new(
            NappletErrorCode::InvalidSignature,
            format!("manifest signature/id verification failed: {e}"),
        )
    })?;

    // Parses and validates the manifest against NIP-5D: the kind, the
    // single-file rule, the added tags, and the aggregate. Nothing that fails
    // there gets as far as a fetch.
    let manifest = NappletManifest::from_event(event)?;

    let index = manifest.index_entry().ok_or_else(|| {
        NappletError::new(
            NappletErrorCode::MissingIndex,
            "manifest lists no /index.html",
        )
    })?;

    let bytes = blobs
        .get(&index.sha256)
        .await
        .map_err(|e| {
            NappletError::new(
                NappletErrorCode::BlobUnavailable,
                format!("fetching blob {} failed: {e}", index.sha256),
            )
        })?
        .ok_or_else(|| {
            NappletError::new(
                NappletErrorCode::BlobUnavailable,
                format!("no source served blob {}", index.sha256),
            )
        })?;

    let got = sha256_hex(&bytes);
    if !got.eq_ignore_ascii_case(&index.sha256) {
        return Err(NappletError::new(
            NappletErrorCode::BlobHashMismatch,
            format!(
                "blob for {} hashes to {got}, manifest says {}",
                index.path, index.sha256
            ),
        ));
    }

    // Non-UTF-8 bytes are not an HTML document, whatever they are.
    let index_html = String::from_utf8(bytes).map_err(|e| {
        NappletError::new(
            NappletErrorCode::InvalidManifest,
            format!("index.html is not valid UTF-8: {e}"),
        )
    })?;

    let (d_tag, aggregate) = manifest.identity();
    let (d_tag, aggregate) = (d_tag.to_string(), aggregate.to_string());
    Ok(ResolvedNapplet {
        d_tag,
        aggregate,
        index_html,
        manifest,
    })
}
