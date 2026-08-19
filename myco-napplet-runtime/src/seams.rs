//! The trait seams the runtime reaches the world through. Nothing in this crate
//! names a concrete relay, blob store, key store, radio, or WebView — which is
//! what keeps every layer above testable off-device, with no phone in the loop.
//!
//! Two seams are not ours: [`RelayBackend`] and [`BlobStore`] come from
//! `nsite-deck`, because a napplet's manifests live in the same relay and its
//! files in the same Blossom store as an nsite's. Re-exported here so a consumer
//! wires one crate rather than two.
//!
//! The rest are napplet-only:
//!
//! - [`Signer`] — the user key, which never leaves Rust. A napplet asks for a
//!   signature; it never sees a key, and there is no seam through which it
//!   could.
//! - [`OutboxResolver`] — which relays an event should reach, across the three
//!   lanes (local, mesh, internet).
//! - [`NapTransport`] — the shell ↔ Rust channel. A seam so that dispatch,
//!   policy and capabilities never learn whether they are talking over
//!   `addWebMessageListener`, a `WebMessagePort`, or the desktop harness's
//!   WebSocket.

use async_trait::async_trait;
use nostr::{Event, PublicKey, UnsignedEvent};

pub use nsite_deck::seams::{BlobStore, RelayBackend};

/// Signs on the user's behalf. Implemented over the napplet **user key**, which
/// is separate from the mesh device key (D3).
///
/// Mediated signing is the whole point: the napplet describes an event, the
/// runtime decides whether the grant covers it and signs. No capability hands
/// out key material.
#[async_trait]
pub trait Signer: Send + Sync {
    /// The public key events will be signed with.
    async fn public_key(&self) -> anyhow::Result<PublicKey>;

    /// Sign an event the runtime has already authorised.
    async fn sign(&self, unsigned: UnsignedEvent) -> anyhow::Result<Event>;
}

/// One relay a napplet's traffic can travel over.
///
/// The three lanes exist because "offline" is the wrong frame in a mesh: a
/// NIP-65 relay list can name `ws://<npub>.fips:4870` beside `wss://` internet
/// relays, and the outbox model works unmodified (design §7.4). A napplet
/// written for the open web works in a room with no internet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayLane {
    /// The device's own embedded relay.
    Local,
    /// A mesh peer's relay, addressed `ws://<npub>.fips:4870` and resolved over
    /// FIPS on the Rust side — napplets never open sockets.
    Mesh { url: String },
    /// An internet relay, when one is reachable.
    Internet { url: String },
}

/// Resolves which relays to read from and write to for a given pubkey (NIP-65).
#[async_trait]
pub trait OutboxResolver: Send + Sync {
    /// Relays to publish this author's events to.
    async fn write_lanes(&self, author: &PublicKey) -> anyhow::Result<Vec<RelayLane>>;

    /// Relays to read this author's events from.
    async fn read_lanes(&self, author: &PublicKey) -> anyhow::Result<Vec<RelayLane>>;
}

/// One message across the shell ↔ Rust channel: a capability call, its result,
/// or a pushed subscription event.
///
/// `domain.action` is the NAP addressing scheme (`relay.publish`,
/// `shell.supports`). `id` correlates a result with its call and is absent on
/// unsolicited pushes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Envelope {
    /// Correlation id, echoed on the response. `None` for pushes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The NAP capability domain (`shell`, `relay`, `identity`).
    pub domain: String,
    /// The action within the domain.
    pub action: String,
    /// The action's payload, shaped by its NAP.
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// The shell ↔ Rust channel.
///
/// On device this is `addWebMessageListener`, scoped to this window's shell
/// origin — never a wildcard, and never `addJavascriptInterface`, which injects
/// into every frame including the napplet's own and would hand the sandboxed
/// napplet the bridge directly. The desktop harness implements the same trait
/// over a loopback WebSocket so the shell can be driven from a browser against
/// a host build.
#[async_trait]
pub trait NapTransport: Send + Sync {
    /// Await the next inbound envelope. `None` means the channel closed.
    async fn recv(&self) -> anyhow::Result<Option<Envelope>>;

    /// Send an envelope to the shell.
    async fn send(&self, envelope: Envelope) -> anyhow::Result<()>;
}
