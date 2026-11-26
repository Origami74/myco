//! `neighborhood:` addresses.
//!
//! An address names a neighborhood, or one peer as seen in one neighborhood,
//! for one napplet. The napplet treats it as opaque and hands it to `relay`,
//! `outbox`, `resource` or `upload`; only the runtime that minted it reads
//! it, which is what these helpers are for.
//!
//! ```text
//! neighborhood://<n>        the whole neighborhood
//! neighborhood://<n>/<p>    one peer in it
//! ```
//!
//! `<n>` and `<p>` are tokens the host minted for that napplet, not the
//! neighborhood id or a key: a token means nothing in another napplet, and the
//! host refuses it there. The scheme is never resolved through DNS and never
//! reaches a socket.

/// The URI scheme, without `:`.
pub const SCHEME: &str = "neighborhood";

/// The longest token accepted. Base32 of 32 bytes is 52 characters; this
/// leaves room without letting a napplet send a megabyte "address".
const MAX_TOKEN: usize = 64;

/// What an address names, in the minting napplet's tokens.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AddressTokens {
    pub neighborhood: String,
    pub peer: Option<String>,
}

/// Whether `s` is a `neighborhood:` address at all, well-formed or not. Used to
/// route a URL to the neighborhood lane before anything else looks at it, so
/// a malformed one is refused there rather than dialled as something else.
pub fn is_address(s: &str) -> bool {
    s.get(..SCHEME.len() + 1)
        .is_some_and(|head| head.eq_ignore_ascii_case("neighborhood:"))
}

/// Build an address from tokens. The host's minting side.
pub fn format(neighborhood: &str, peer: Option<&str>) -> String {
    match peer {
        Some(p) => format!("{SCHEME}://{neighborhood}/{p}"),
        None => format!("{SCHEME}://{neighborhood}"),
    }
}

/// Read an address back into tokens, if it has exactly the shape [`format`]
/// produces: a trailing `/` is tolerated; query, fragment, userinfo, port and
/// extra path segments are not.
pub fn parse(s: &str) -> Option<AddressTokens> {
    if !is_address(s) {
        return None;
    }
    let rest = s[SCHEME.len() + 1..].strip_prefix("//")?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let mut parts = rest.split('/');
    let neighborhood = parts.next().filter(|t| valid_token(t))?;
    let peer = match parts.next() {
        None => None,
        Some(p) if valid_token(p) => Some(p.to_string()),
        Some(_) => return None,
    };
    if parts.next().is_some() {
        return None;
    }
    Some(AddressTokens {
        neighborhood: neighborhood.to_string(),
        peer,
    })
}

fn valid_token(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= MAX_TOKEN
        && t.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for (n, p) in [("h2c", None), ("h2c", Some("p7f"))] {
            let a = format(n, p);
            let t = parse(&a).unwrap();
            assert_eq!(t.neighborhood, n);
            assert_eq!(t.peer.as_deref(), p);
        }
    }

    #[test]
    fn recognizes_the_scheme_in_any_case() {
        assert!(is_address("neighborhood://x"));
        assert!(is_address("Neighborhood://x"));
        assert!(!is_address("neighborhood"));
        assert!(!is_address("wss://neighborhood.example"));
        assert!(!is_address("ws://h.hood"));
    }

    #[test]
    fn refuses_anything_but_the_minted_shape() {
        for bad in [
            "neighborhood:x",
            "neighborhood://",
            "neighborhood://a/b/c",
            "neighborhood://a?x=1",
            "neighborhood://a#f",
            "neighborhood://u@a",
            "neighborhood://a:80",
            "neighborhood://A",
            "neighborhood://a//",
        ] {
            assert!(parse(bad).is_none(), "{bad} parsed");
        }
        assert!(parse("neighborhood://a/").is_some(), "trailing slash");
        assert!(parse(&format!("neighborhood://{}", "a".repeat(65))).is_none());
    }
}
