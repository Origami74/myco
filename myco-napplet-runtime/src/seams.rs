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

/// Where a napplet's published events go.
///
/// Separate from [`RelayBackend`] because storing and *accepting* are different
/// acts. A store is where an event rests; accepting is what also wakes this
/// device's live subscriptions and hands the event to whatever carries it to
/// other people. A napplet that only stored would have its event signed, saved,
/// and invisible — nothing would redraw here and no peer would ever hear it.
#[async_trait]
pub trait EventSink: Send + Sync {
    /// Take a signed event and do everything accepting it implies.
    async fn accept(&self, event: Event) -> anyhow::Result<()>;
}

/// An [`EventSink`] that only stores — the honest default for a runtime with
/// nothing to fan out to, and what tests use when distribution is not the point.
pub struct StoreOnlySink(pub std::sync::Arc<dyn RelayBackend>);

#[async_trait]
impl EventSink for StoreOnlySink {
    async fn accept(&self, event: Event) -> anyhow::Result<()> {
        self.0.publish(event).await
    }
}

/// One message across the shell ↔ Rust channel: a capability call, its result,
/// or a pushed subscription event.
///
/// The wire format is NIP-5D's, and it is **flat** — the payload's fields sit
/// beside `type` and `id`, not nested under a `payload` key:
///
/// ```text
/// -> { "type": "relay.publish", "id": "a1", "event": { … } }
/// <- { "type": "relay.publish.result", "id": "a1", "ok": true }
/// ```
///
/// `type` is `domain.action` — the NAP addressing scheme, where the domain
/// (`relay`, `shell`, `identity`) names the capability and surfaces to the
/// napplet as `window.napplet.<domain>`. A result echoes the request's `type`
/// with `.result` appended.
///
/// `id` correlates a result with its call. It is absent on unsolicited pushes,
/// and absent on both handshake messages, which occur exactly once per napplet
/// and so have nothing to correlate.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Envelope {
    /// `domain.action`, or `domain.action.result` for a result.
    #[serde(rename = "type")]
    pub msg_type: String,
    /// Correlation id, echoed on the result. `None` for pushes and handshakes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Every other top-level field. Flattened, because the payload *is* the
    /// top level on this wire — nesting it under a key would be a different
    /// protocol that no conformant napplet speaks.
    #[serde(flatten)]
    pub fields: serde_json::Map<String, serde_json::Value>,
}

impl Envelope {
    /// A message with no payload fields.
    pub fn new(msg_type: impl Into<String>) -> Self {
        Self {
            msg_type: msg_type.into(),
            id: None,
            fields: serde_json::Map::new(),
        }
    }

    /// Set the correlation id.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Add one top-level field.
    pub fn with_field(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }

    /// The capability domain — everything before the first `.`.
    pub fn domain(&self) -> &str {
        self.msg_type.split('.').next().unwrap_or("")
    }

    /// The action within the domain: the segment after the domain, with any
    /// trailing `.result` removed.
    pub fn action(&self) -> &str {
        let rest = match self.msg_type.split_once('.') {
            Some((_, rest)) => rest,
            None => return "",
        };
        rest.strip_suffix(".result").unwrap_or(rest)
    }

    /// Whether this is a result rather than a call.
    pub fn is_result(&self) -> bool {
        self.msg_type.ends_with(".result")
    }

    /// The result envelope for this call: the same `type` with `.result`
    /// appended and the same `id`. Result fields are the individual NAP's
    /// business — `ok` belongs to `relay.publish`, not to every message — so
    /// the caller adds them with [`Envelope::with_field`].
    ///
    /// Calling this on a message that is already a result would produce
    /// `x.result.result`, so the suffix is only ever appended once.
    pub fn to_result(&self) -> Self {
        let msg_type = if self.is_result() {
            self.msg_type.clone()
        } else {
            format!("{}.result", self.msg_type)
        };
        Self {
            msg_type,
            id: self.id.clone(),
            fields: serde_json::Map::new(),
        }
    }

    /// The failure result for this call. Per the registry's error model, a
    /// result carrying `error` leaves every other result field undefined — so
    /// this deliberately carries nothing else.
    pub fn to_error(&self, error: impl Into<String>) -> Self {
        let mut out = self.to_result();
        out.fields
            .insert("error".to_string(), serde_json::Value::String(error.into()));
        out
    }

    /// Read one top-level field.
    pub fn field(&self, key: &str) -> Option<&serde_json::Value> {
        self.fields.get(key)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The examples are copied from NAP-SHELL and the NIP-5D web projection. A
    /// napplet is built against those bytes, so this pins the shape rather than
    /// merely round-tripping our own struct through itself.
    #[test]
    fn the_wire_format_is_flat() {
        let call: Envelope = serde_json::from_value(
            json!({"type": "relay.publish", "id": "a1", "event": {"kind": 1}}),
        )
        .unwrap();
        assert_eq!(call.msg_type, "relay.publish");
        assert_eq!(call.id.as_deref(), Some("a1"));
        assert_eq!(call.domain(), "relay");
        assert_eq!(call.action(), "publish");
        assert!(!call.is_result());
        // The payload sits at the top level, not under a `payload` key.
        assert_eq!(call.field("event").unwrap()["kind"], 1);

        assert_eq!(
            serde_json::to_value(&call).unwrap(),
            json!({"type": "relay.publish", "id": "a1", "event": {"kind": 1}})
        );
    }

    #[test]
    fn a_result_echoes_the_type_and_id() {
        let call = Envelope::new("relay.publish").with_id("a1");
        assert_eq!(
            serde_json::to_value(call.to_result().with_field("ok", true)).unwrap(),
            json!({"type": "relay.publish.result", "id": "a1", "ok": true})
        );
    }

    /// The registry's error model: a result carrying `error` leaves every other
    /// result field undefined, so a failure never also looks like a success.
    #[test]
    fn an_error_result_carries_only_the_error() {
        let call = Envelope::new("theme.get").with_id("t1");
        assert_eq!(
            serde_json::to_value(call.to_error("no active theme")).unwrap(),
            json!({"type": "theme.get.result", "id": "t1", "error": "no active theme"})
        );
    }

    /// `.result` is appended once. Deriving a result from a result would
    /// produce `relay.publish.result.result`, which nothing answers to.
    #[test]
    fn results_do_not_stack() {
        let result = Envelope::new("relay.publish").with_id("a1").to_result();
        assert_eq!(result.to_result().msg_type, "relay.publish.result");
        assert_eq!(result.domain(), "relay");
        assert_eq!(result.action(), "publish");
    }

    /// Both handshake messages occur exactly once per napplet lifecycle, so
    /// neither carries a correlation id — and `id` must not appear in the JSON
    /// at all rather than appear as null.
    #[test]
    fn the_handshake_carries_no_id() {
        let ready: Envelope = serde_json::from_value(json!({"type": "shell.ready"})).unwrap();
        assert_eq!(ready.id, None);
        assert_eq!(ready.domain(), "shell");
        assert_eq!(ready.action(), "ready");
        assert!(ready.fields.is_empty(), "shell.ready carries no payload");
        assert_eq!(
            serde_json::to_value(&ready).unwrap(),
            json!({"type": "shell.ready"})
        );

        let init = Envelope::new("shell.init")
            .with_field("capabilities", json!({"domains": ["relay", "identity"]}))
            .with_field("services", json!([]));
        assert_eq!(
            serde_json::to_value(&init).unwrap(),
            json!({
                "type": "shell.init",
                "capabilities": {"domains": ["relay", "identity"]},
                "services": []
            })
        );
    }

    #[test]
    fn a_malformed_type_does_not_panic() {
        for msg_type in ["", "shell", "...", ".result"] {
            let envelope = Envelope::new(msg_type);
            let _ = envelope.domain();
            let _ = envelope.action();
        }
    }
}
