//! The update extensions over links ([`super::extensions`]):
//! `napplet-updates`, `nsite-updates` and `myco-release` work the same way,
//! described here for napplets. A release is its signed announcement and
//! file event plus the APK; one received is kept for the release check, and
//! the APK is fetched when the phone carries the update
//! ([`ExtensionSource`]).
//!
//! **Pushing.** When this device moves an installed napplet to a newer
//! version, the runtime calls [`Neighbors::share_update`]: in every
//! neighborhood it shares `napplet-updates` in, the manifest is kept in
//! scope, the version's files are put in scope (read from the device's own
//! store, not copied), and the manifest is published like any neighborhood
//! event, travelling the neighborhood's publish hops.
//!
//! **Receiving** ([`receive`]). In every neighborhood where this device
//! gets an update extension, it holds one subscription for that
//! extension's kinds open on every link carrying the neighborhood, for as
//! long as it runs. A link that starts, comes back in reach or reconnects is
//! asked again, so it answers with what it holds, then follows with what it
//! shares later. An update shared while nobody was around still arrives at
//! the next meeting.
//!
//! Every update manifest that lands in a neighborhood's scope from a peer
//! (pushed, followed, or pulled for a napplet's subscription) and every one
//! a peer in reach sends again goes to one place, the runtime's
//! [`UpdateSink`]. If the napplet is installed here and the version is
//! newer, the runtime fetches the files from that neighborhood's links
//! ([`NeighborhoodSource`]); once pinned, this device shares it onward in
//! turn. A device that doesn't have the napplet keeps the manifest in the
//! neighborhood (an app store reading the neighborhood sees that someone
//! around holds it) and passes nothing on.

pub(super) mod held;
mod receive;
mod share;
mod sources;

pub use held::HeldNapplets;
pub use sources::{ExtensionSource, NeighborhoodSource};

use std::sync::Arc;

use nostr::Event;

use super::extensions::{MYCO_RELEASE, NAPPLET_UPDATES, NSITE_UPDATES};
use super::id::NeighborhoodId;

/// What the runtime does with an update a linked peer shared: neighborhood,
/// the extension it came under, manifest.
pub type UpdateSink = Arc<dyn Fn(NeighborhoodId, &'static str, Event) + Send + Sync>;

/// Zapstore's release announcement and file event: what `myco-release`
/// carries, with the APK.
const RELEASE_ANNOUNCEMENT: u16 = 30063;
const RELEASE_FILE: u16 = 3063;

/// The update extension an event kind travels under, if any: napplet
/// manifests under `napplet-updates`, nsite manifests under `nsite-updates`.
pub fn extension_for(kind: u16) -> Option<&'static str> {
    if kind == myco_napplet_runtime::KIND_ROOT || kind == myco_napplet_runtime::KIND_NAMED {
        Some(NAPPLET_UPDATES)
    } else if kind == nsite_deck::KIND_ROOT || kind == nsite_deck::KIND_NAMED {
        Some(NSITE_UPDATES)
    } else if kind == RELEASE_ANNOUNCEMENT || kind == RELEASE_FILE {
        Some(MYCO_RELEASE)
    } else {
        None
    }
}

/// The event kinds an update extension carries: the inverse of
/// [`extension_for`].
pub fn kinds_for(extension: &str) -> Vec<nostr::Kind> {
    let kinds: &[u16] = match extension {
        NAPPLET_UPDATES => &[
            myco_napplet_runtime::KIND_ROOT,
            myco_napplet_runtime::KIND_NAMED,
        ],
        NSITE_UPDATES => &[nsite_deck::KIND_ROOT, nsite_deck::KIND_NAMED],
        MYCO_RELEASE => &[RELEASE_ANNOUNCEMENT, RELEASE_FILE],
        _ => &[],
    };
    kinds.iter().copied().map(nostr::Kind::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_update_kind_has_its_extension() {
        assert_eq!(
            extension_for(myco_napplet_runtime::KIND_NAMED),
            Some(NAPPLET_UPDATES)
        );
        assert_eq!(
            extension_for(myco_napplet_runtime::KIND_ROOT),
            Some(NAPPLET_UPDATES)
        );
        assert_eq!(extension_for(nsite_deck::KIND_NAMED), Some(NSITE_UPDATES));
        assert_eq!(extension_for(nsite_deck::KIND_ROOT), Some(NSITE_UPDATES));
        assert_eq!(extension_for(30063), Some(MYCO_RELEASE));
        assert_eq!(extension_for(3063), Some(MYCO_RELEASE));
        // A napplet snapshot names no slot to update; a note is no update.
        assert_eq!(extension_for(myco_napplet_runtime::KIND_SNAPSHOT), None);
        assert_eq!(extension_for(1), None);
    }

    #[test]
    fn each_extension_carries_the_kinds_that_map_back_to_it() {
        for ext in [NAPPLET_UPDATES, NSITE_UPDATES, MYCO_RELEASE] {
            let kinds = kinds_for(ext);
            assert!(!kinds.is_empty());
            for k in kinds {
                assert_eq!(extension_for(k.as_u16()), Some(ext));
            }
        }
        assert!(kinds_for("other").is_empty());
    }
}
