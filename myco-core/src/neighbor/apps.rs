//! App peers: the devices this one shares apps with.
//!
//! Myco's own basics — fetching an app a neighbor has, app and release
//! updates — ride the neighborhoods that allow it (Apps, on by default). The
//! peer set is every linked device in such a neighborhood, by the device npub
//! in its link offer's `.fips` address.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use nostr::nips::nip19::FromBech32;
use nostr::PublicKey;

use super::link::Links;
use super::store::Record;
use super::Neighbors;
use crate::content::{AppPeer, Content, PeerPerms};

/// How often the set is recomputed. Joining, leaving and Apps toggles also
/// recompute at once, through the runtime.
const EVERY: Duration = Duration::from_secs(5);

/// The device npub in a `.fips` URL (`ws://npub1….fips:4870`), if any.
pub fn device_npub(url: &str) -> Option<String> {
    let host = url.split("://").nth(1)?.split(['/', ':']).next()?;
    let npub = host.strip_suffix(".fips")?;
    PublicKey::from_bech32(npub).ok()?;
    Some(npub.to_string())
}

/// Every linked device sharing an extension with us somewhere, once each.
pub fn app_peers(records: &[Record], links: &Links) -> Vec<AppPeer> {
    let mut out: Vec<AppPeer> = Vec::new();
    for link in links.all() {
        let shares = records.iter().any(|r| {
            super::extensions::KNOWN
                .iter()
                .any(|e| super::extensions::shared(r, &link, r.id, e))
        });
        if !shares {
            continue;
        }
        let Some(npub) = link.services().iter().find_map(|s| device_npub(&s.url)) else {
            continue;
        };
        if out.iter().any(|p| p.npub == npub) {
            continue;
        }
        out.push(AppPeer {
            npub,
            name: link.name.clone().unwrap_or_default(),
            perms: PeerPerms::default(),
        });
    }
    out
}

/// Recompute the set once.
pub fn refresh(neighbors: &Neighbors, links: &Links, content: &Content) {
    content.set_app_peers(app_peers(&neighbors.records(), links));
}

/// Keep the set current, for link changes the runtime does not see (a peer
/// ending a link, a share arriving).
pub async fn keep_current(
    links: Arc<Links>,
    neighbors: Arc<OnceLock<Arc<Neighbors>>>,
    content: Arc<Content>,
) {
    loop {
        if let Some(n) = neighbors.get() {
            refresh(n, &links, &content);
        }
        tokio::time::sleep(EVERY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::nips::nip19::ToBech32;

    #[test]
    fn reads_the_npub_from_a_fips_url() {
        let npub = nostr::Keys::generate().public_key().to_bech32().unwrap();
        assert_eq!(
            device_npub(&format!("ws://{npub}.fips:4870")).as_deref(),
            Some(npub.as_str())
        );
        assert_eq!(
            device_npub(&format!("http://{npub}.fips:4873/nh")).as_deref(),
            Some(npub.as_str())
        );
        assert_eq!(device_npub("wss://relay.example.com"), None);
        assert_eq!(device_npub("ws://nonsense.fips:4870"), None);
    }
}
