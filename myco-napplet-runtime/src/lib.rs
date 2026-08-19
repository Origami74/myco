//! `myco-napplet-runtime` — a [NIP-5D] napplet runtime: transport-agnostic,
//! Android-free, and testable with no device in the loop.
//!
//! Where an nsite is a document Myco *serves*, a napplet is a program Myco
//! *hosts* — and hosting means mediating. Every capability a napplet uses is
//! one Myco decided to grant, implemented by Myco, on the napplet's behalf. The
//! relay, the blob store, the mesh and the signing key all sit behind the seams
//! in [`seams`], which the napplet asks through rather than reaches around.
//!
//! A napplet is distributed exactly like an nsite: a signed manifest whose
//! `path` tags map paths to sha256 hashes, with the bytes in Blossom. NIP-5D is
//! [NIP-5A]'s manifest shape at kinds `5129` / `15129` / `35129`, plus
//! `requires` / `archetype` / `config` tags, and with the aggregate hash
//! promoted from an integrity check to the napplet's **identity**. The shared
//! NIP-5A primitives therefore live in `nsite-deck`, not here; this crate is
//! what is genuinely napplet-only.
//!
//! ```text
//! manifest.rs  the NIP-5D kinds and the tags they add
//! resolve.rs   manifest → blobs → verify → renderable artifact
//! seams.rs     RelayBackend / BlobStore / Signer / OutboxResolver / NapTransport
//! error.rs     one error type; every variant is a refusal to render
//! ```
//!
//! Design: `docs/design/napplet/napplet-runtime.md`.
//!
//! [NIP-5D]: https://github.com/nostr-protocol/nips/pull/2303
//! [NIP-5A]: https://github.com/nostr-protocol/nips/blob/master/5A.md

pub mod error;
pub mod manifest;
pub mod resolve;
pub mod seams;

#[cfg(feature = "testing")]
pub mod testing;

pub use error::{NappletError, NappletErrorCode, Result};
pub use manifest::{
    is_napplet_kind, Archetype, NappletManifest, KINDS, KIND_NAMED, KIND_ROOT, KIND_SNAPSHOT,
};
pub use resolve::{resolve, ResolvedNapplet};
pub use seams::{
    BlobStore, Envelope, NapTransport, OutboxResolver, RelayBackend, RelayLane, Signer,
};
