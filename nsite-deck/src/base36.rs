//! Base36 host-label codec for **named** nsites (kind 35128).
//!
//! A named site is addressed as `<pubkeyB36><dTag>` where `pubkeyB36` is the raw
//! 32-byte pubkey encoded big-endian in lowercase base36, **always exactly 50
//! characters** (left zero-padded), directly followed by a `dTag` matching
//! `^[a-z0-9-]{1,13}$` (not ending in `-`). 50 + 13 fits one 63-char DNS label.
//!
//! This is a Rust port of the Go reference
//! (`reference/site-deck/internal/gateway/base36.go`); the encoder/decoder and
//! the label regex must match it exactly so hosts round-trip across
//! implementations. The big-integer arithmetic is hand-rolled over `[u8; 32]`
//! (long division / multiply-add) to avoid a bignum dependency.

const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
/// Lowercase-base36 of a 256-bit number never exceeds 50 chars (36^50 > 2^256).
pub const PUBKEY_B36_LEN: usize = 50;
/// d-tag bound from the spec: `^[a-z0-9-]{1,13}$`, no trailing `-`.
pub const MAX_DTAG_LEN: usize = 13;

/// Encode a 32-byte pubkey as a 50-char lowercase-base36 string (left-padded).
pub fn encode_pubkey(pubkey: &[u8; 32]) -> String {
    let mut n = *pubkey; // big-endian working copy; consumed by repeated div-mod
    let mut digits = Vec::with_capacity(PUBKEY_B36_LEN);
    while !is_zero(&n) {
        let rem = div_mod_36(&mut n);
        digits.push(ALPHABET[rem as usize]);
    }
    while digits.len() < PUBKEY_B36_LEN {
        digits.push(b'0');
    }
    digits.reverse();
    // digits.len() can exceed 50 only for an out-of-range input, which is
    // impossible for a 32-byte key; keep the last 50 defensively.
    String::from_utf8(digits).expect("base36 alphabet is ASCII")
}

/// Decode a 50-char base36 label back into a 32-byte pubkey.
pub fn decode_pubkey(s: &str) -> Option<[u8; 32]> {
    if s.len() != PUBKEY_B36_LEN {
        return None;
    }
    let mut n = [0u8; 32];
    for ch in s.bytes() {
        let digit = base36_digit(ch)?;
        if !mul_add_36(&mut n, digit) {
            return None; // overflow → not a valid 32-byte pubkey label
        }
    }
    Some(n)
}

/// Parse a full named-site label `<pubkeyB36><dTag>` into `(pubkey, d_tag)`.
/// Returns `None` if it does not match `^[0-9a-z]{50}[a-z0-9-]{1,13}$` with no
/// trailing `-`.
pub fn parse_named_label(label: &str) -> Option<([u8; 32], String)> {
    if label.len() < PUBKEY_B36_LEN + 1 || label.len() > PUBKEY_B36_LEN + MAX_DTAG_LEN {
        return None;
    }
    let (head, d_tag) = label.split_at(PUBKEY_B36_LEN);
    if !head.bytes().all(|b| base36_digit(b).is_some()) {
        return None;
    }
    if !valid_d_tag(d_tag) {
        return None;
    }
    let pubkey = decode_pubkey(head)?;
    Some((pubkey, d_tag.to_string()))
}

/// `^[a-z0-9-]{1,13}$`, must not end with `-`.
pub fn valid_d_tag(d_tag: &str) -> bool {
    if d_tag.is_empty() || d_tag.len() > MAX_DTAG_LEN || d_tag.ends_with('-') {
        return false;
    }
    d_tag
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

// --- big-integer helpers over a 32-byte big-endian buffer ---

fn is_zero(n: &[u8; 32]) -> bool {
    n.iter().all(|&b| b == 0)
}

/// `n /= 36`, returning the remainder. `n` is big-endian.
fn div_mod_36(n: &mut [u8; 32]) -> u8 {
    let mut carry: u16 = 0;
    for byte in n.iter_mut() {
        let cur = carry * 256 + *byte as u16;
        *byte = (cur / 36) as u8;
        carry = cur % 36;
    }
    carry as u8
}

/// `n = n * 36 + digit`, big-endian. Returns false on 32-byte overflow.
fn mul_add_36(n: &mut [u8; 32], digit: u8) -> bool {
    let mut carry: u16 = digit as u16;
    for byte in n.iter_mut().rev() {
        let cur = *byte as u16 * 36 + carry;
        *byte = (cur & 0xff) as u8;
        carry = cur >> 8;
    }
    carry == 0
}

fn base36_digit(ch: u8) -> Option<u8> {
    match ch {
        b'0'..=b'9' => Some(ch - b'0'),
        b'a'..=b'z' => Some(ch - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_known_pubkey() {
        // 266815e0… is the example author key from docs/reference/nostr-kinds.md.
        let pk_hex = "266815e0c9210dfa324c6cba3573b14bee49da4209a9456f9484e5106cd408a5";
        let mut pk = [0u8; 32];
        hex::decode_to_slice(pk_hex, &mut pk).unwrap();

        let label = encode_pubkey(&pk);
        assert_eq!(label.len(), PUBKEY_B36_LEN, "label = {label}");
        assert!(label.bytes().all(|b| base36_digit(b).is_some()));
        assert_eq!(decode_pubkey(&label), Some(pk));
    }

    #[test]
    fn round_trips_edge_values() {
        for pk in [[0u8; 32], [0xff; 32], {
            let mut a = [0u8; 32];
            a[31] = 1;
            a
        }] {
            let label = encode_pubkey(&pk);
            assert_eq!(label.len(), PUBKEY_B36_LEN);
            assert_eq!(decode_pubkey(&label), Some(pk), "label = {label}");
        }
    }

    #[test]
    fn parses_named_label() {
        let pk_hex = "266815e0c9210dfa324c6cba3573b14bee49da4209a9456f9484e5106cd408a5";
        let mut pk = [0u8; 32];
        hex::decode_to_slice(pk_hex, &mut pk).unwrap();
        let label = format!("{}blog", encode_pubkey(&pk));

        let (got_pk, d_tag) = parse_named_label(&label).unwrap();
        assert_eq!(got_pk, pk);
        assert_eq!(d_tag, "blog");
    }

    #[test]
    fn rejects_bad_d_tags() {
        assert!(!valid_d_tag(""));
        assert!(!valid_d_tag("trailing-"));
        assert!(!valid_d_tag("UPPER"));
        assert!(!valid_d_tag("too-long-dtag-x")); // 14 chars
        assert!(valid_d_tag("blog"));
        assert!(valid_d_tag("a-b-2"));
    }

    #[test]
    fn decode_rejects_wrong_length() {
        assert_eq!(decode_pubkey("abc"), None);
    }
}
