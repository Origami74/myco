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
//! ["MESH", {"ttl": 1, "qid": "…", "budgetMs": 5000}, ["REQ", <sub_id>, <filter>, …]]
//! ```
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
//! `reference/thinning-custom-relay.md` (D1, D8).

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The verb. Anything that does not know it replies `NOTICE` and ignores the
/// frame, which is the correct failure: a relay that is not part of this mesh
/// must not join a flood.
pub const MESH: &str = "MESH";

/// Mesh state travelling alongside a NIP-01 message.
///
/// Future fields (a path vector, an origin hint, a rate class) extend this, never
/// the inner message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshMeta {
    /// Remaining forward hops. `0` means store it, do not pass it on.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ttl: u8,

    /// Query id, stamped by the originating proxy so every node can serve a given
    /// query once. A circle is a graph rather than a tree, so the same query
    /// arrives by several paths; without this each arrival re-fans it and the
    /// cost multiplies. Also the amplification bound — one peer's `REQ` would
    /// otherwise make us issue one per circle member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qid: Option<String>,

    /// Remaining time budget in milliseconds, **relative** — never a wall-clock
    /// deadline, because mesh clocks are not synchronised.
    ///
    /// Its only job is bounding how long a node holds query state. Results that
    /// arrive late are not an error; they stream to whoever is still listening,
    /// and a node whose budget has run out simply drops them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_ms: Option<u32>,
}

fn is_zero(n: &u8) -> bool {
    *n == 0
}

/// How far a pushed event travels: the budget a node stamps on its own events,
/// and the most it will honour from a peer. One constant, because they are the
/// same number for the same reason — the reach a wave is allowed — and having
/// chat, manifests, and the clamp each name it separately let them drift.
///
/// The pull plane's counterpart is [`MAX_REQ_TTL`](crate::mesh_relay::MAX_REQ_TTL),
/// deliberately lower: a flooded read costs more than a flooded write, since
/// every hop answers as well as forwards.
pub const EVENT_TTL: u8 = 3;

/// Share of the remaining budget a hop may spend downstream, keeping the rest to
/// receive and relay. Deadlines are not composed — see [`MeshMeta::budget_ms`].
const BUDGET_SHARE: f64 = 0.6;

impl MeshMeta {
    /// Metadata for a push carrying `ttl` more hops.
    pub fn push(ttl: u8) -> Self {
        Self {
            ttl,
            ..Default::default()
        }
    }

    /// Metadata for a pull: `ttl` more hops under query id `qid`, with `budget_ms`
    /// left to answer in.
    pub fn pull(ttl: u8, qid: impl Into<String>, budget_ms: u32) -> Self {
        Self {
            ttl,
            qid: Some(qid.into()),
            budget_ms: Some(budget_ms),
        }
    }

    /// What to send to the next hop: one fewer hop, and a share of what is left of
    /// the budget. Returns `None` once the hop count is spent.
    pub fn next_hop(&self) -> Option<Self> {
        if self.ttl == 0 {
            return None;
        }
        Some(Self {
            ttl: self.ttl - 1,
            qid: self.qid.clone(),
            budget_ms: self
                .budget_ms
                .map(|ms| (ms as f64 * BUDGET_SHARE).round() as u32),
        })
    }

    /// Clamp the hop count to what this node is willing to honour, so a peer
    /// cannot set a large value and turn us into an amplifier.
    pub fn clamped(mut self, max_ttl: u8) -> Self {
        self.ttl = self.ttl.min(max_ttl);
        self
    }

    /// How long this hop may wait on the peers it forwards to.
    ///
    /// This is what stops deadlines nesting. Without it every hop starts a fresh
    /// full-length timer inside its parent's, so a peer two hops out that
    /// honestly takes most of its window returns after the hop above has already
    /// given up — depth 2 then looks implemented while reliably yielding nothing.
    /// Taking the budget that arrived means the whole wave collapses inside the
    /// originator's window.
    ///
    /// `fallback` is used when no budget rode in — an older peer, or a plain
    /// pull that never carried one. Both ends are bounded: never longer than the
    /// budget allows, and never so short that a slow BLE link cannot answer.
    pub fn hop_timeout(&self, fallback: Duration) -> Duration {
        match self.budget_ms {
            Some(ms) => Duration::from_millis(ms as u64).clamp(MIN_HOP_TIMEOUT, fallback),
            None => fallback,
        }
    }
}

/// Floor on a forwarded hop's timeout. A budget worn down by several hops can get
/// small; below this there is no point dialling at all over BLE, where a connect
/// alone can take a second.
const MIN_HOP_TIMEOUT: Duration = Duration::from_millis(1500);

/// A fresh query id for a pull this node originates.
///
/// Only has to be unique among queries in flight nearby, so 8 random bytes is
/// ample. A **forwarded** pull must carry the id it arrived with instead — that
/// is what lets every node downstream serve it once.
pub fn new_query_id() -> String {
    hex::encode(crate::ip_source::random_bytes(8))
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

    /// Hops decrement and the budget shrinks with depth, rather than each hop
    /// nesting a fresh full-length deadline inside its parent's.
    #[test]
    fn hops_decrement_and_the_budget_shrinks() {
        let start = MeshMeta::pull(2, "q1", 10_000);

        let hop1 = start.next_hop().expect("hops left");
        assert_eq!(hop1.ttl, 1);
        assert_eq!(hop1.qid.as_deref(), Some("q1"), "the query id is carried");
        assert_eq!(hop1.budget_ms, Some(6_000));

        let hop2 = hop1.next_hop().expect("one more hop");
        assert_eq!(hop2.ttl, 0);
        assert_eq!(hop2.budget_ms, Some(3_600));

        assert!(hop2.next_hop().is_none(), "the wave terminates");
    }

    /// A forwarded hop waits no longer than the budget that arrived.
    ///
    /// This is the whole point of carrying one. With a fresh full-length timer
    /// per hop, each hop's window sits inside its parent's, so a peer two hops
    /// out that honestly uses most of its time returns after the hop above has
    /// given up — depth 2 looks implemented and reliably yields nothing.
    #[test]
    fn a_hop_waits_within_the_budget_it_was_given() {
        let fallback = Duration::from_secs(8);

        // A budget shorter than the fallback wins.
        let tight = MeshMeta::pull(1, "q", 3_000);
        assert_eq!(tight.hop_timeout(fallback), Duration::from_millis(3_000));

        // A budget longer than the fallback does not extend us: a peer cannot
        // ask us to hold a connection open for a minute.
        let greedy = MeshMeta::pull(1, "q", 600_000);
        assert_eq!(greedy.hop_timeout(fallback), fallback);

        // No budget at all (an older peer) falls back to the fixed timeout.
        assert_eq!(MeshMeta::push(1).hop_timeout(fallback), fallback);

        // A budget worn down to nearly nothing still leaves time to dial, since
        // a BLE connect alone can take about a second.
        let spent = MeshMeta::pull(1, "q", 10);
        assert_eq!(spent.hop_timeout(fallback), MIN_HOP_TIMEOUT);
    }

    /// Each hop's window fits strictly inside its parent's.
    ///
    /// Hop windows nest rather than add up: hop 2 does its waiting *inside* hop
    /// 1's window. So the property that makes a wave terminate is that every hop
    /// allows less time than the hop above it, and the first allows no more than
    /// the originator's budget. A fixed timeout per hop fails at the first step —
    /// every hop would allow exactly as long as its parent, so the deepest one is
    /// still running when the originator has already given up.
    #[test]
    fn each_hop_waits_less_than_the_one_above_it() {
        let fallback = Duration::from_secs(8);
        let originator_budget = Duration::from_millis(10_000);

        let mut meta = MeshMeta::pull(3, "q", 10_000);
        let mut windows = Vec::new();
        while let Some(next) = meta.next_hop() {
            windows.push(next.hop_timeout(fallback));
            meta = next;
        }

        assert_eq!(windows.len(), 3, "three hops of budget");
        assert!(
            windows[0] <= originator_budget,
            "the first hop stays inside the originator's budget"
        );
        for pair in windows.windows(2) {
            assert!(
                pair[1] < pair[0],
                "each hop must allow less than its parent, got {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    /// A peer cannot set a large hop count and make us amplify for it.
    #[test]
    fn a_hostile_ttl_is_clamped() {
        let meta = MeshMeta::push(255).clamped(3);
        assert_eq!(meta.ttl, 3);
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
