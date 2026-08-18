//! Myco's own vocabulary for BLE connect-attempt diagnostics.
//!
//! These types used to be fips's (`fips::transport::ble::attempts`), read out
//! of a process-global log in the transport. That log is gone: the restacked
//! fips counts connect outcomes into `BleStats` instead, queryable over the
//! control socket's `show_transports`.
//!
//! Myco already *owned* the accumulation — [`crate::attempt_store`] merges,
//! deduplicates, persists and evicts on its own retention policy, and only
//! round-tripped through fips types because it was retrofitted onto a
//! fips-owned ring. Owning the record type removes that round trip and lets
//! the store and the Dev-tab merge keep their shape while the producer is
//! rewired.
//!
//! # TODO(stage 2): nothing produces these yet
//!
//! `AppRuntime::ble_attempts()` returns an empty slice, so every consumer here
//! renders "no recorded history". Stage 2 fills them from `BleStats` counters
//! read over `show_transports`. Note the shape mismatch to resolve then: these
//! are *per-attempt* records keyed by BLE address, and `BleStats` is
//! *aggregate* counters per transport — the Dev tab's per-peer attempt rows
//! cannot be reconstructed from counters alone, so stage 2 either reshapes the
//! rows or asks fips for something per-peer.

/// Maximum attempts retained per peer address, oldest dropped first. Myco's
/// own retention policy now; it happens to match the ring fips used to keep.
pub const MAX_ATTEMPTS_PER_PEER: usize = 20;

/// Which side of the L2CAP connection this node took for an attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BleRole {
    /// This node dialled the peer (outbound probe).
    Central,
    /// This node accepted the peer's connection (inbound).
    Peripheral,
}

impl BleRole {
    /// Stable lowercase wire label, as persisted and as sent across the FFI.
    pub fn as_str(&self) -> &'static str {
        match self {
            BleRole::Central => "central",
            BleRole::Peripheral => "peripheral",
        }
    }
}

/// How one discovery-to-resolution cycle ended. Exactly one value per attempt,
/// so one cycle is one record rather than several fragments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BleAttemptOutcome {
    /// The connection was promoted into the pool and is carrying traffic.
    Connected,
    /// The L2CAP connect exceeded the configured connect timeout.
    ConnectTimeout,
    /// The L2CAP connect returned an error.
    ConnectError,
    /// The connection opened but the pubkey exchange did not complete.
    PubkeyExchangeFailed,
    /// The cross-probe tiebreaker resolved in the peer's favour, so this side
    /// dropped its connection and deferred to the peer's.
    LostTiebreaker,
    /// The connection was usable but the pool had no room for it.
    PoolRejected,
    /// The peer was already connected under a different link address, so this
    /// duplicate was dropped — a peer rotating resolvable private addresses
    /// being absorbed rather than filling the pool.
    DuplicateNode,
}

impl BleAttemptOutcome {
    /// Stable kebab-case wire label, as persisted and as sent across the FFI.
    pub fn as_str(&self) -> &'static str {
        match self {
            BleAttemptOutcome::Connected => "connected",
            BleAttemptOutcome::ConnectTimeout => "connect-timeout",
            BleAttemptOutcome::ConnectError => "connect-error",
            BleAttemptOutcome::PubkeyExchangeFailed => "pubkey-exchange-failed",
            BleAttemptOutcome::LostTiebreaker => "lost-tiebreaker",
            BleAttemptOutcome::PoolRejected => "pool-rejected",
            BleAttemptOutcome::DuplicateNode => "duplicate-node",
        }
    }
}

/// One resolved connect attempt against one peer.
#[derive(Clone, Debug)]
pub struct BleAttempt {
    /// Wall-clock milliseconds since the Unix epoch at which the attempt
    /// resolved. Wall clock rather than monotonic because these are persisted
    /// and must compare across process lifetimes.
    pub at_ms: u64,
    /// The peer's BLE address.
    pub ble_addr: String,
    /// The peer's node address in hex, when the attempt got far enough to learn
    /// one; empty otherwise. Never guessed.
    pub node_addr_hex: String,
    /// Which role this node took.
    pub role: BleRole,
    /// Milliseconds between the address being discovered and this resolution;
    /// `0` when no discovery stamp was recorded.
    pub discovery_ms: u64,
    /// How the attempt ended.
    pub outcome: BleAttemptOutcome,
}

/// Everything recorded about one peer address.
#[derive(Clone, Debug)]
pub struct BlePeerAttempts {
    /// The peer's BLE address.
    pub ble_addr: String,
    /// The node address hex learned for this peer, or empty if no attempt has
    /// carried one yet.
    pub node_addr_hex: String,
    /// Count of sends to this peer that failed at the link.
    pub send_failures: u64,
    /// Attempts oldest-first, capped at [`MAX_ATTEMPTS_PER_PEER`].
    pub attempts: Vec<BleAttempt>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The labels cross the FFI into the Kotlin Dev tab and are written into
    /// the persisted JSONL, so they are a compatibility surface, not cosmetics.
    #[test]
    fn wire_labels_are_stable() {
        assert_eq!(BleRole::Central.as_str(), "central");
        assert_eq!(BleRole::Peripheral.as_str(), "peripheral");
        assert_eq!(BleAttemptOutcome::Connected.as_str(), "connected");
        assert_eq!(
            BleAttemptOutcome::PubkeyExchangeFailed.as_str(),
            "pubkey-exchange-failed"
        );
        assert_eq!(BleAttemptOutcome::DuplicateNode.as_str(), "duplicate-node");
    }
}
