//! Extensions to NAP-NEIGHBOR: something a device shares proactively in a
//! neighborhood, beyond what napplets publish there.
//!
//! - [`NAPPLET_UPDATES`] — a NAP extension, for any runtime: when this device
//!   downloads an update for a napplet it has installed, it pushes the
//!   manifest into the neighborhood and serves that version's files there.
//! - [`NSITE_UPDATES`] — Myco's: the same for nsites in the Library.
//! - [`MYCO_RELEASE`] — Myco's: the signed Myco release APK.
//!
//! Each is a switch per neighborhood, on unless the user turns it off. A
//! device lists the ones it has on in its offer for each neighborhood
//! ([`super::link::OfferContent::extensions`]); something is shared over a
//! link only when both sides have that extension on there.

use super::id::NeighborhoodId;
use super::link::Link;
use super::store::Record;

pub const NAPPLET_UPDATES: &str = "napplet-updates";
pub const NSITE_UPDATES: &str = "nsite-updates";
pub const MYCO_RELEASE: &str = "myco-release";

/// Every extension this build knows, in the order the UI lists them.
pub const KNOWN: [&str; 3] = [NAPPLET_UPDATES, NSITE_UPDATES, MYCO_RELEASE];

/// The extensions this device has on in `record`.
pub fn ours(record: &Record) -> Vec<String> {
    KNOWN
        .iter()
        .filter(|e| record.policy.has(e))
        .map(|e| e.to_string())
        .collect()
}

/// Whether `extension` is on at both ends of `link` in `n`.
pub fn shared(record: &Record, link: &Link, n: NeighborhoodId, extension: &str) -> bool {
    record.policy.has(extension)
        && link.neighborhoods.contains(&n)
        && link.peer_extensions(n).iter().any(|e| e == extension)
}
