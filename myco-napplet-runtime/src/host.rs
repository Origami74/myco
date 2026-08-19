//! The per-napplet shell origin.
//!
//! Every napplet's shell page is served from its own loopback origin, so
//! shell-side storage, caches and cookies partition per napplet automatically —
//! inherited from the browser rather than enforced by us (D12). The label is
//! `<pubkeyB36><dTagPart>`, mirroring the nsite host form, under the suffix
//! [`SHELL_SUFFIX`].
//!
//! ## Why not plain `.localhost`
//!
//! The design's literal form is `<pubkeyB36><dTag>.localhost`. That is exactly
//! the host an **nsite** is already served at, so the same author's nsite named
//! `chat` and napplet named `chat` would land on one origin. The two WebView
//! clients are each required to refuse the other's hosts (design §5.5), and they
//! cannot: the labels are indistinguishable.
//!
//! So napplet shells sit one label deeper, at `<label>.napplet.localhost`.
//! Chromium treats *any* host ending in `.localhost` as loopback and a secure
//! context, so nothing is lost, and the suffix makes the refusal in both
//! directions a string comparison instead of a guess.
//!
//! ## Why the d tag is sometimes hashed
//!
//! An nsite d tag is bound by NIP-5A to `^[a-z0-9-]{1,13}$` precisely so
//! `50 + 13` fits one 63-character DNS label. A **napplet** d tag is governed by
//! NIP-5D and carries no such bound, so it may not fit — or may not be a legal
//! DNS label at all. Those fall back to a short hash.
//!
//! The mapping must stay injective or two napplets share an origin and the
//! partitioning D12 buys is gone. It does, in both directions:
//!
//! - Different authors differ in the 50-char `pubkeyB36` prefix.
//! - Within one author, a **direct** d tag part always starts with a lowercase
//!   letter and a **hashed** one always starts with [`HASH_MARKER`], so the two
//!   forms can never be confused for each other.
//! - Within each form: direct is the identity, and hashed is 62 bits of
//!   sha256. Note that a hash collision has to be found *under the victim's own
//!   pubkey*, which means holding the victim's key — so grinding one is
//!   self-inflicted, not an attack path.

use nsite_deck::base36::{encode_pubkey, PUBKEY_B36_LEN};
use nsite_deck::sync::sha256_hex;

/// The suffix every napplet shell origin ends with.
pub const SHELL_SUFFIX: &str = ".napplet.localhost";

/// Marks a hashed d-tag part. A digit, and legal d tags that reach the direct
/// form always start with a letter — that disjointness is what keeps the
/// mapping injective.
const HASH_MARKER: char = '0';

/// Length of the hashed d-tag part, marker included. `50 + 13 = 63`, one DNS
/// label exactly.
const HASHED_LEN: usize = 13;

/// The d-tag bound NIP-5A imposes and NIP-5D does not.
const MAX_DIRECT_DTAG: usize = 13;

/// The host label for a napplet, without the suffix.
///
/// `d_tag` is `None` for root (`15129`) and snapshot (`5129`) manifests, which
/// have none; those get the bare 50-char pubkey label, which no named napplet
/// can collide with because a named one is always longer.
pub fn shell_label(pubkey: &[u8; 32], d_tag: Option<&str>) -> String {
    let prefix = encode_pubkey(pubkey);
    match d_tag {
        None => prefix,
        Some(d) if is_direct(d) => format!("{prefix}{d}"),
        Some(d) => format!("{prefix}{}", hashed_part(d)),
    }
}

/// The full shell host for a napplet: `<label>.napplet.localhost`.
pub fn shell_host(pubkey: &[u8; 32], d_tag: Option<&str>) -> String {
    format!("{}{SHELL_SUFFIX}", shell_label(pubkey, d_tag))
}

/// The shell origin a napplet's WebView is loaded at.
pub fn shell_origin(pubkey: &[u8; 32], d_tag: Option<&str>) -> String {
    format!("http://{}", shell_host(pubkey, d_tag))
}

/// Whether `host` is a napplet shell host.
///
/// The nsite WebView client refuses every host this accepts, and the napplet
/// client refuses every host it does not — otherwise an nsite could navigate
/// itself into a shell origin and inherit the capability channel, which is
/// scoped to that origin.
pub fn is_shell_host(host: &str) -> bool {
    let host = host.split(':').next().unwrap_or(host);
    let host = host.strip_suffix('.').unwrap_or(host); // a fully-qualified name
    let lower = host.to_ascii_lowercase();
    let Some(label) = lower.strip_suffix(SHELL_SUFFIX) else {
        return false;
    };
    label.len() >= PUBKEY_B36_LEN && !label.contains('.')
}

/// A d tag usable verbatim: a legal DNS label component that fits beside the
/// 50-char pubkey, and starts with a letter so it cannot be mistaken for a
/// hashed part.
fn is_direct(d_tag: &str) -> bool {
    !d_tag.is_empty()
        && d_tag.len() <= MAX_DIRECT_DTAG
        && d_tag.starts_with(|c: char| c.is_ascii_lowercase())
        && !d_tag.ends_with('-')
        && d_tag
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `HASH_MARKER` followed by 12 base36 characters of the d tag's sha256.
fn hashed_part(d_tag: &str) -> String {
    let digest = sha256_hex(d_tag.as_bytes());
    // 62 bits: 2^62 < 36^12, so the value always fits the 12 characters.
    let head = u64::from_str_radix(&digest[..16], 16).expect("sha256_hex is hex");
    let mut value = head >> 2;

    let mut digits = [b'0'; HASHED_LEN - 1];
    for slot in digits.iter_mut().rev() {
        *slot = b"0123456789abcdefghijklmnopqrstuvwxyz"[(value % 36) as usize];
        value /= 36;
    }
    let mut out = String::with_capacity(HASHED_LEN);
    out.push(HASH_MARKER);
    out.push_str(std::str::from_utf8(&digits).expect("base36 alphabet is ASCII"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: [u8; 32] = [0x11; 32];
    const B: [u8; 32] = [0x22; 32];

    fn label(pubkey: &[u8; 32], d: Option<&str>) -> String {
        shell_label(pubkey, d)
    }

    #[test]
    fn a_short_d_tag_is_carried_verbatim() {
        let got = label(&A, Some("chat"));
        assert!(got.ends_with("chat"));
        assert_eq!(got.len(), PUBKEY_B36_LEN + 4);
    }

    #[test]
    fn root_and_snapshot_get_the_bare_pubkey_label() {
        let got = label(&A, None);
        assert_eq!(got.len(), PUBKEY_B36_LEN);
        assert_eq!(got, encode_pubkey(&A));
    }

    /// Every label has to fit one 63-character DNS label, whatever the d tag.
    #[test]
    fn every_label_fits_one_dns_label() {
        let long = "a-napplet-with-a-very-long-identifier-indeed".repeat(4);
        for d in [None, Some("chat"), Some("a"), Some(long.as_str())] {
            let got = label(&A, d);
            assert!(got.len() <= 63, "{} chars for {d:?}", got.len());
            assert!(
                got.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "illegal DNS label {got:?}"
            );
            assert!(!got.ends_with('-'), "label ends with a hyphen: {got:?}");
        }
    }

    /// The property the whole module exists for: no two napplets share an
    /// origin. Direct and hashed forms are checked against each other, since
    /// that is where a naive implementation collides.
    #[test]
    fn the_mapping_is_injective() {
        let d_tags = [
            None,
            Some("chat"),
            Some("chats"),
            Some("a"),
            Some("0chat"),                       // leading digit -> hashed
            Some("Chat"),                        // uppercase -> hashed
            Some("chat-"),                       // trailing hyphen -> hashed
            Some("a-very-long-identifier-here"), // too long -> hashed
            Some("another-long-identifier"),     // too long -> hashed
            Some("has spaces"),                  // illegal -> hashed
        ];
        let mut seen = std::collections::HashSet::new();
        for pubkey in [&A, &B] {
            for d in d_tags {
                let got = shell_host(pubkey, d);
                assert!(seen.insert(got.clone()), "origin collision on {d:?}: {got}");
            }
        }
        assert_eq!(seen.len(), d_tags.len() * 2);
    }

    /// A d tag that cannot be carried verbatim always starts its part with the
    /// marker, which a direct part never can.
    #[test]
    fn hashed_and_direct_parts_cannot_be_confused() {
        let hashed = label(&A, Some("Chat"));
        let part = &hashed[PUBKEY_B36_LEN..];
        assert_eq!(part.len(), HASHED_LEN);
        assert!(part.starts_with(HASH_MARKER));

        let direct = label(&A, Some("chat"));
        let part = &direct[PUBKEY_B36_LEN..];
        assert!(!part.starts_with(HASH_MARKER));
    }

    #[test]
    fn hashing_is_stable() {
        assert_eq!(label(&A, Some("Chat")), label(&A, Some("Chat")));
        assert_ne!(label(&A, Some("Chat")), label(&A, Some("Chat2")));
    }

    /// The suffix is what lets each WebView client refuse the other's hosts.
    #[test]
    fn shell_hosts_are_recognised_and_nsite_hosts_are_not() {
        let shell = shell_host(&A, Some("chat"));
        assert!(is_shell_host(&shell));
        assert!(is_shell_host(&format!("{shell}:8080")));
        assert!(is_shell_host(&format!("{shell}.")));
        assert!(is_shell_host(&shell.to_ascii_uppercase()));

        // The nsite form for the same author and d tag — the collision this
        // module's suffix exists to prevent.
        let nsite = format!("{}chat.localhost", encode_pubkey(&A));
        assert!(!is_shell_host(&nsite));
        assert!(!is_shell_host("npub1xyz.localhost"));
        assert!(!is_shell_host("example.com"));
        // A shell-looking label nested under another host is not a shell host.
        assert!(!is_shell_host(&format!("evil.{shell}")));
    }
}
