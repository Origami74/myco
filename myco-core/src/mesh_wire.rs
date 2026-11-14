//! The `MESH` envelope: Myco's own framing on the peer-to-peer link.
//!
//! Mesh state used to ride *inside* the objects a relay stores and matches on —
//! an `event-ttl` key added to the event, a `req-ttl` key added to a filter. That
//! made every relay in the mesh implement Myco's protocol, made correctness
//! depend on a backend not round-tripping unknown keys, and pushed routing state
//! through the query language.
//!
//! Now it rides beside them instead:
//!
//! ```text
//! ["MESH", {"ttl": 2}, ["EVENT", <event>]]
//! ```
//!
//! Only pushed events carry an envelope; a `REQ` goes out plain and is
//! answered from the peer's own store. An older build may still wrap its
//! `REQ` with pull metadata (`qid`, `budgetMs`); those keys are ignored.
//!
//! The inner element is **exactly** what would go on the wire to any relay: the
//! event object and every filter object are canonical NIP-01, byte for byte. The
//! proxy reads `meta`, decides, and passes the inner element through unchanged,
//! so nothing is re-encoded anywhere in the path.
//!
//! The wrapper is verb-agnostic on purpose. `["MESH", meta, <anything NIP-01>]`
//! carries `COUNT`, a future `NEG-OPEN`, or anything else without this module
//! learning what they are — and it is one grep to find every mesh frame in a log.
//!
//! **This framing appears on exactly one link.** The nsite talks plain NIP-01 to
//! `localhost:4870`, and the proxy talks plain NIP-01 to whatever relay sits
//! behind it. Only proxy-to-proxy traffic over `.fips` is ours to shape. See
//! `reference/thinning-custom-relay.md` (D1).

use serde::{Deserialize, Serialize};

/// The verb. Anything that does not know it replies `NOTICE` and ignores the
/// frame, which is the correct failure: a relay that is not part of this mesh
/// must not join a flood.
pub const MESH: &str = "MESH";

/// Mesh state travelling alongside a NIP-01 message. Unknown keys (such as an
/// older build's `qid` / `budgetMs`) are ignored when parsing.
///
/// Future fields (a path vector, an origin hint, a rate class) extend this, never
/// the inner message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshMeta {
    /// Remaining forward hops. `0` means store it, do not pass it on.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ttl: u8,
}

fn is_zero(n: &u8) -> bool {
    *n == 0
}

/// How far a pushed event travels: the budget a node stamps on its own events,
/// and the most it will honour from a peer. One constant, because they are the
/// same number for the same reason — the reach a wave is allowed — and having
/// manifests and the clamp each name it separately let them drift.
pub const EVENT_TTL: u8 = 3;

impl MeshMeta {
    /// Metadata for a push carrying `ttl` more hops.
    pub fn push(ttl: u8) -> Self {
        Self { ttl }
    }
}

/// Wrap a NIP-01 message in a `MESH` envelope, ready to write to a peer.
pub fn wrap(meta: &MeshMeta, inner: serde_json::Value) -> String {
    serde_json::json!([MESH, meta, inner]).to_string()
}

/// Split a `MESH` envelope into its metadata and the untouched NIP-01 message
/// inside. `None` for anything that is not one — including a plain NIP-01 frame,
/// which stays valid on this link and means "no mesh metadata": store it, do not
/// forward it.
pub fn unwrap(frame: &serde_json::Value) -> Option<(MeshMeta, serde_json::Value)> {
    let array = frame.as_array()?;
    if array.first()?.as_str()? != MESH {
        return None;
    }
    // A malformed meta is treated as absent rather than fatal: the inner message
    // is still a valid NIP-01 message and is worth handling, just not forwarding.
    let meta = array
        .get(1)
        .and_then(|m| serde_json::from_value::<MeshMeta>(m.clone()).ok())
        .unwrap_or_default();
    let inner = array.get(2)?.clone();
    // Only an array can be a NIP-01 message; anything else is a malformed frame.
    inner.as_array()?;
    Some((meta, inner))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The inner message must survive the round trip untouched — that is the
    /// whole point of wrapping rather than modifying.
    #[test]
    fn the_inner_message_is_carried_verbatim() {
        let event = serde_json::json!({
            "id": "abc", "pubkey": "def", "created_at": 1, "kind": 9,
            "tags": [["d", "mesh"]], "content": "hi", "sig": "beef"
        });
        let inner = serde_json::json!(["EVENT", event]);

        let frame = wrap(&MeshMeta::push(2), inner.clone());
        let parsed: serde_json::Value = serde_json::from_str(&frame).unwrap();
        let (meta, got) = unwrap(&parsed).expect("a MESH frame");

        assert_eq!(meta.ttl, 2);
        assert_eq!(got, inner, "the NIP-01 message is byte-for-byte unchanged");
        assert!(
            !frame.contains("event-ttl") && !frame.contains("req-ttl"),
            "no mesh state inside the event or filters"
        );
    }

    /// A plain NIP-01 frame stays valid on this link and carries no metadata, so
    /// it is stored and never forwarded.
    #[test]
    fn plain_nip01_is_not_a_mesh_frame() {
        let plain = serde_json::json!(["EVENT", { "id": "abc" }]);
        assert!(unwrap(&plain).is_none());

        let req = serde_json::json!(["REQ", "s1", { "kinds": [9] }]);
        assert!(unwrap(&req).is_none());
    }

    /// An older build's pull metadata parses; the extra keys are ignored.
    #[test]
    fn old_pull_metadata_is_ignored() {
        let frame = serde_json::json!([
            "MESH",
            { "ttl": 1, "qid": "q1", "budgetMs": 5000 },
            ["REQ", "s1", { "kinds": [9] }]
        ]);
        let (meta, inner) = unwrap(&frame).expect("a MESH frame");
        assert_eq!(meta, MeshMeta::push(1));
        assert_eq!(inner, serde_json::json!(["REQ", "s1", { "kinds": [9] }]));
    }

    /// A malformed envelope must not take the connection down with it.
    #[test]
    fn a_malformed_envelope_is_rejected_not_fatal() {
        // No inner message.
        assert!(unwrap(&serde_json::json!(["MESH", {"ttl": 1}])).is_none());
        // Inner is not a NIP-01 message.
        assert!(unwrap(&serde_json::json!(["MESH", {"ttl": 1}, "nope"])).is_none());
        // Unreadable meta still yields the inner message, with no hops.
        let (meta, inner) =
            unwrap(&serde_json::json!(["MESH", "junk", ["EVENT", {}]])).expect("inner is usable");
        assert_eq!(meta, MeshMeta::default());
        assert_eq!(inner, serde_json::json!(["EVENT", {}]));
    }
}
