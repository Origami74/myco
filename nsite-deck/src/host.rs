//! Host / link resolution: turn a `<host>.nsite` label (or a pasted link) into a
//! `(author pubkey, d-tag)` site address. See `docs/design/nsite-layer.md` §3.2.
//!
//! - **Root** site: the label is an `npub1…` (NIP-19 bech32 of the author).
//! - **Named** site: the label is `<pubkeyB36><dTag>` (base36 50-char pubkey +
//!   d-tag), decoded by [`crate::base36`].
//!
//! v0 has no alias store, so there is no alias-chain walk (the reference's
//! `resolveStem` recursion); resolution is a single step. The depth guard is kept
//! as a constant for when aliases land.

use nostr::nips::nip19::FromBech32;
use nostr::PublicKey;

use crate::base36;

/// A resolved site address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteAddr {
    pub author: PublicKey,
    /// `None` = root site (kind 15128); `Some` = named site (kind 35128).
    pub d_tag: Option<String>,
}

impl SiteAddr {
    /// `"npub"` / `"npub:dtag"` cache key.
    pub fn site_key(&self) -> String {
        use nostr::nips::nip19::ToBech32;
        let npub = self.author.to_bech32().unwrap_or_default();
        crate::model::site_key(&npub, self.d_tag.as_deref())
    }

    /// The canonical `<host>` label (no suffix): `npub1…` for root, base36 for named.
    pub fn host_label(&self) -> String {
        match &self.d_tag {
            None => {
                use nostr::nips::nip19::ToBech32;
                self.author.to_bech32().unwrap_or_default()
            }
            Some(d) => format!("{}{}", base36::encode_pubkey(&self.author.to_bytes()), d),
        }
    }
}

/// Resolve a gateway host (`<label>.nsite`, optionally `.nsite.lol`, optional
/// trailing dot, optional `:port`) into a [`SiteAddr`]. Returns `None` for an
/// unresolvable host (the gateway bounces those to the Library "not in your
/// Library" page).
pub fn resolve_host(host: &str) -> Option<SiteAddr> {
    let label = strip_suffix(host)?;
    resolve_label(label)
}

/// Resolve a bare label (no `.nsite` suffix) — an `npub` or a `<base36><dtag>`.
pub fn resolve_label(label: &str) -> Option<SiteAddr> {
    // Root: a valid npub.
    if label.starts_with("npub1") {
        if let Ok(author) = PublicKey::from_bech32(label) {
            return Some(SiteAddr {
                author,
                d_tag: None,
            });
        }
    }
    // Named: base36 pubkey + d-tag.
    if let Some((pk_bytes, d_tag)) = base36::parse_named_label(label) {
        if let Ok(author) = PublicKey::from_slice(&pk_bytes) {
            return Some(SiteAddr {
                author,
                d_tag: Some(d_tag),
            });
        }
    }
    None
}

/// Parse a **pasted** nsite link into a [`SiteAddr`]. Accepts the forms a user is
/// likely to paste:
/// `npub1…`, `<host>.nsite`, `<host>.nsite.lol`, with or without a
/// `http(s)://` scheme, a path, or a trailing slash.
pub fn parse_link(input: &str) -> Option<SiteAddr> {
    let mut s = input.trim();
    // Strip scheme.
    if let Some(rest) = s.split_once("://") {
        s = rest.1;
    }
    // Strip path / query / fragment — keep only the host authority.
    s = s
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(s)
        .trim_end_matches('.');
    if s.is_empty() {
        return None;
    }
    // A bare npub (no suffix) is the simplest paste.
    if let Some(addr) = resolve_label(s) {
        return Some(addr);
    }
    resolve_host(s)
}

/// Strip a `.nsite` / `.nsite.lol` suffix and any `:port` / trailing dot,
/// returning the leading label.
fn strip_suffix(host: &str) -> Option<&str> {
    let host = host.trim().trim_end_matches('.');
    let host = host.split(':').next().unwrap_or(host); // drop :port
    let lower = host.to_ascii_lowercase();
    if let Some(idx) = lower.rfind(".nsite.lol") {
        if idx + ".nsite.lol".len() == lower.len() {
            return Some(&host[..idx]);
        }
    }
    if let Some(idx) = lower.rfind(".nsite") {
        if idx + ".nsite".len() == lower.len() {
            return Some(&host[..idx]);
        }
    }
    // The in-app WebView serves nsites under `<label>.localhost` (not `.nsite`):
    // Chromium classifies `*.localhost` as loopback + a secure context, so the
    // page can open `ws://localhost:4870` to the embedded relay without tripping
    // Private/Local Network Access (see docs/design/nsite-layer.md host suffix).
    if let Some(idx) = lower.rfind(".localhost") {
        if idx + ".localhost".len() == lower.len() {
            return Some(&host[..idx]);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_npub() -> (PublicKey, String) {
        use nostr::nips::nip19::ToBech32;
        let pk_hex = "266815e0c9210dfa324c6cba3573b14bee49da4209a9456f9484e5106cd408a5";
        let pk = PublicKey::parse(pk_hex).unwrap();
        (pk, pk.to_bech32().unwrap())
    }

    #[test]
    fn resolves_root_npub_host() {
        let (pk, npub) = sample_npub();
        let addr = resolve_host(&format!("{npub}.nsite")).unwrap();
        assert_eq!(addr.author, pk);
        assert_eq!(addr.d_tag, None);
    }

    #[test]
    fn resolves_named_host_round_trip() {
        let (pk, _) = sample_npub();
        let label = format!("{}blog", base36::encode_pubkey(&pk.to_bytes()));
        let addr = resolve_host(&format!("{label}.nsite")).unwrap();
        assert_eq!(addr.author, pk);
        assert_eq!(addr.d_tag.as_deref(), Some("blog"));
        // host_label is the inverse.
        assert_eq!(addr.host_label(), label);
    }

    #[test]
    fn parses_pasted_links() {
        let (pk, npub) = sample_npub();
        for input in [
            npub.clone(),
            format!("{npub}.nsite"),
            format!("https://{npub}.nsite.lol"),
            format!("http://{npub}.nsite.lol/index.html"),
            format!("{npub}.nsite.lol/"),
        ] {
            let addr = parse_link(&input).unwrap_or_else(|| panic!("failed on {input}"));
            assert_eq!(addr.author, pk, "input = {input}");
            assert_eq!(addr.d_tag, None, "input = {input}");
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(resolve_host("example.com").is_none());
        assert!(resolve_host("notanpub.nsite").is_none());
        assert!(parse_link("").is_none());
    }
}
