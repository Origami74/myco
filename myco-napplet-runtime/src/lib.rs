//! `myco-napplet-runtime` — a [NIP-5D] napplet runtime: transport-agnostic,
//! Android-free, and testable with no device in the loop.
//!
//! Myco **never authors, signs, or publishes** a napplet. It loads napplets
//! built and published elsewhere, exactly as it does nsites — so every rule
//! here is one an externally-built manifest is *judged against*, never one Myco
//! can arrange to satisfy. When a napplet is refused, the fix is upstream of
//! this device; the runtime's job is to say clearly what was wrong.
//!
//! Where an nsite is a document Myco *serves*, a napplet is a program Myco
//! *hosts* — and hosting means mediating. Every capability a napplet uses is
//! one Myco decided to grant, implemented by Myco, on the napplet's behalf. The
//! relay, the blob store, the mesh and the signing key all sit behind the seams
//! in [`seams`], which the napplet asks through rather than reaches around.
//!
//! A napplet is a single self-contained `/index.html` in Blossom, named by a
//! signed manifest at kinds `5129` / `15129` / `35129` whose `x` tag is the
//! artifact's sha256. That hash, with the `d` identifier, is the napplet's
//! **identity**. The manifest also advertises required and optional NAP
//! domains (`R` / `O`), archetypes and intents (`z` / `i`), and an optional
//! icon. Napplets published before 2026-10-07 used NIP-5A's path/aggregate
//! shape instead; that still parses, through an isolated legacy fallback in
//! [`manifest`].
//!
//! ```text
//! manifest.rs  the NIP-5D kinds and the tags they add
//! resolve.rs   manifest → blobs → verify → renderable artifact
//! seams.rs     RelayBackend / BlobStore / Signer / EventSink / MeshSink / NapTransport
//! error.rs     one error type; every variant is a refusal to render
//! ```
//!
//! Design: `docs/design/napplet/napplet-runtime.md`.
//!
//! [NIP-5D]: https://github.com/nostr-protocol/nips/pull/2303
//! [NIP-5A]: https://github.com/nostr-protocol/nips/blob/master/5A.md

pub mod artifact;
pub mod delivered;
pub mod dispatch;
pub mod error;
pub mod host;
pub mod manifest;
pub mod nap;
pub mod prelude;
pub mod resolve;
pub mod seams;
pub mod session;
pub mod shell_link;
pub mod shell_page;

#[cfg(feature = "testing")]
pub mod testing;

pub use artifact::{assemble, CspPolicy, Injection, SrcdocArtifact};
pub use dispatch::{dispatch, needs_session, NapContext, Outcome};
pub use error::{NappletError, NappletErrorCode, Result};
pub use host::{is_shell_host, shell_host, shell_label, shell_origin, SHELL_SUFFIX};
pub use manifest::{
    icon_bytes_match, is_napplet_kind, AppListing, Archetype, IntentAdvert, ManifestFormat,
    ManifestIcon, NappletManifest, ICON_MIME_TYPES, KINDS, KIND_NAMED, KIND_ROOT, KIND_SNAPSHOT,
};
pub use nap::deliveries_for;
pub use nap::intent::{IntentCatalog, IntentCatalogSnapshot, IntentHandler, NoIntents};
pub use prelude::{
    render_for as render_prelude, PRELUDE_GLOBAL, SIGNING_TIMEOUT, SUPPLEMENT_GLOBAL,
};
pub use resolve::{resolve, ResolvedNapplet};
pub use seams::{
    is_mesh_relay_url, BlobFetcher, BlobHints, BlobReach, BlobStore, Direction, EarlyAnswer,
    Envelope, EventSink, HttpsError, HttpsErrorCode, HttpsFetcher, LaneTransport, MeshLimits,
    MeshReach, MeshSink, NapTransport, NoFetcher, NoHttps, NoUploads, OutboxResolver, PlanSource,
    RelayBackend, RelayLane, RelayPlan, ScopeOwner, Signer, StoreOnlySink, UploadBlob, UploadError,
    UploadErrorCode, UploadSink, Uploaded, WorkScope, MAX_BLOB_HASHES,
};
pub use session::{
    Appearance, NappletIdentity, Session, Streamer, SubscriptionHandle, IMPLEMENTED_DOMAINS,
    MANDATORY_DOMAINS,
};
pub use shell_link::{ChooserCandidate, ShellAction, ToRuntime, ToShell};
pub use shell_page::{shell_page, RUNTIME_OBJECT};
