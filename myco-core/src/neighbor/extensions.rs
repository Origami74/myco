//! Extensions to NAP-NEIGHBOR: something a device shares proactively in a
//! neighborhood, beyond what napplets publish there.
//!
//! - [`NAPPLET_UPDATES`] — a NAP extension, for any runtime: when this device
//!   downloads an update for a napplet it has installed, it pushes the
//!   manifest into the neighborhood and serves that version's files there.
//! - [`NSITE_UPDATES`] — Myco's: the same for nsites in the Library.
//! - [`MYCO_RELEASE`] — Myco's: the signed Myco release APK.
//!
//! Each has two switches per neighborhood, both on unless the user turns
//! them off: **share** (push this device's updates there) and **get** (apply
//! updates that arrive there). They are independent, and nothing is agreed
//! with peers: an update is an ordinary neighborhood event, travelling the
//! neighborhood's publish hops, and each device decides for itself whether
//! to act on one.

use super::store::Record;

pub const NAPPLET_UPDATES: &str = "napplet-updates";
pub const NSITE_UPDATES: &str = "nsite-updates";
pub const MYCO_RELEASE: &str = "myco-release";

/// Every extension this build knows, in the order the UI lists them.
pub const KNOWN: [&str; 3] = [NAPPLET_UPDATES, NSITE_UPDATES, MYCO_RELEASE];

/// The extensions this device shares in `record`.
pub fn shared(record: &Record) -> Vec<String> {
    KNOWN
        .iter()
        .filter(|e| record.policy.shares(e))
        .map(|e| e.to_string())
        .collect()
}

/// The extensions this device takes updates of in `record`.
pub fn got(record: &Record) -> Vec<String> {
    KNOWN
        .iter()
        .filter(|e| record.policy.gets(e))
        .map(|e| e.to_string())
        .collect()
}
