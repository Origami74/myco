//! `napplet.link` — a web link that opens a napplet by role.
//!
//! ```text
//! https://napplet.link/<archetype>/<intent>?<params>
//! napplet:<archetype>/<intent>?<params>
//!     ⇔ intent.invoke { archetype, action: <intent>,
//!                       convention: "napplet:<archetype>/<intent>",
//!                       payload: { …params } }
//! ```
//!
//! Both spellings name the same NAP-INTENT request, for any archetype. The
//! `https:` one is the shareable form: it works in a chat message, a QR code
//! or a web page, opens Myco where Myco is installed (an app link on
//! `napplet.link`), and lands on a web page everywhere else. The `napplet:`
//! one is the convention URI itself, as NAP-INTENT's web binding spells it.
//!
//! The query is payload sugar, decoded exactly as the web binding decodes a
//! convention URI's (`projections/web.md`): each `name=value` pair is
//! percent-decoded as text into a flat object of strings. No type coercion,
//! and no form-encoding — `+` is a literal plus sign. A repeated name, a
//! malformed escape or text that is not UTF-8 refuses the whole link rather
//! than guessing which reading the sender meant. So a link and the same
//! invocation from inside a napplet hand the handler the same payload.
//!
//! A fragment refuses a `napplet:` link, as the binding says. On an `https:`
//! link it is dropped: a fragment never leaves the browser, it is the web
//! page's business, and share sheets append them freely.
//!
//! Only the path is case-folded. An alphanumeric-mode QR code upper-cases the
//! whole link; the path is slugs, which are lower-case by definition, so
//! `HTTPS://NAPPLET.LINK/NOTE/OPEN` still names `note/open`. The query is
//! data and is never touched — a link whose query was upper-cased in transit
//! arrives upper-cased.
//!
//! What this module does not decide: whether anything can handle the link
//! (that is the host's catalog), or whether the user wanted it opened (that
//! is the host's UI). It only says what the link asks for.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::manifest::is_slug;
use crate::nap::link::MAX_URL_LEN;

/// The host the `https:` form lives on.
pub const HOST: &str = "napplet.link";

/// Why a string is not a napplet link.
pub const NOT_A_NAPPLET_LINK: &str = "not a napplet link";
/// The path is not exactly `/<archetype>/<intent>` in slugs.
pub const BAD_PATH: &str = "not an archetype and intent";
/// The query does not decode: a malformed escape, text that is not UTF-8, an
/// empty name, or a name given twice.
pub const BAD_QUERY: &str = "malformed query";
/// A `napplet:` link carrying a fragment.
pub const FRAGMENT: &str = "fragment not allowed";

/// A parsed `napplet.link`: a NAP-INTENT request in link form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NappletLink {
    pub archetype: String,
    /// NAP-INTENT's `action`: the `<intent>` half of the convention.
    pub intent: String,
    /// The query, decoded. Sorted, so the same link always yields the same
    /// payload and the same [`NappletLink::web_url`].
    pub params: BTreeMap<String, String>,
}

impl NappletLink {
    /// Parse either spelling. Anything that is not one — another host, another
    /// scheme — is [`NOT_A_NAPPLET_LINK`]; one that is, but is malformed,
    /// says what is wrong with it.
    pub fn parse(url: &str) -> Result<Self, &'static str> {
        let url = url.trim();
        if url.is_empty() || url.len() > MAX_URL_LEN {
            return Err(NOT_A_NAPPLET_LINK);
        }
        // NAP-LINK's rule: a real link has whitespace and control characters
        // percent-encoded.
        if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(NOT_A_NAPPLET_LINK);
        }
        let Some((scheme, rest)) = url.split_once(':') else {
            return Err(NOT_A_NAPPLET_LINK);
        };
        if scheme.eq_ignore_ascii_case("https") {
            let after = rest.strip_prefix("//").ok_or(NOT_A_NAPPLET_LINK)?;
            let end = after.find(['/', '?', '#']).unwrap_or(after.len());
            let (authority, rest) = after.split_at(end);
            // Exactly the host: no userinfo, no port, no trailing dot. Anything
            // else is some other web page, not this link.
            if !authority.eq_ignore_ascii_case(HOST) {
                return Err(NOT_A_NAPPLET_LINK);
            }
            let rest = rest.split_once('#').map_or(rest, |(before, _)| before);
            let (path, query) = split_query(rest);
            let path = path.strip_prefix('/').ok_or(BAD_PATH)?;
            Self::from_parts(path, query)
        } else if scheme.eq_ignore_ascii_case("napplet") {
            // `napplet:naddr1…` / `napplet://naddr1…` is a different link —
            // install review — and has no `/` between two slugs. An authority
            // form is not this shape either.
            if rest.starts_with("//") {
                return Err(NOT_A_NAPPLET_LINK);
            }
            if rest.contains('#') {
                return Err(FRAGMENT);
            }
            let (path, query) = split_query(rest);
            if !path.contains('/') {
                return Err(NOT_A_NAPPLET_LINK);
            }
            Self::from_parts(path, query)
        } else {
            Err(NOT_A_NAPPLET_LINK)
        }
    }

    fn from_parts(path: &str, query: Option<&str>) -> Result<Self, &'static str> {
        let path = path.to_ascii_lowercase();
        // One trailing slash is how some tools write a path; more is not.
        let path = path.strip_suffix('/').unwrap_or(&path);
        let mut segments = path.split('/');
        let (Some(archetype), Some(intent), None) =
            (segments.next(), segments.next(), segments.next())
        else {
            return Err(BAD_PATH);
        };
        if !is_slug(archetype) || !is_slug(intent) {
            return Err(BAD_PATH);
        }
        Ok(Self {
            archetype: archetype.to_string(),
            intent: intent.to_string(),
            params: match query {
                Some(q) => decode_query(q)?,
                None => BTreeMap::new(),
            },
        })
    }

    /// The queryless convention identity: `napplet:<archetype>/<intent>`.
    pub fn convention(&self) -> String {
        format!("napplet:{}/{}", self.archetype, self.intent)
    }

    /// The NAP-INTENT payload: the params as an object of strings — empty
    /// when the link had none, so a handler always gets an object.
    pub fn payload(&self) -> Value {
        Value::Object(
            self.params
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect::<Map<String, Value>>(),
        )
    }

    /// The canonical `https://napplet.link/…` form, for a browser when nothing
    /// on this device opens the link — whichever spelling it arrived in.
    pub fn web_url(&self) -> String {
        let mut url = format!("https://{HOST}/{}/{}", self.archetype, self.intent);
        let mut sep = '?';
        for (name, value) in &self.params {
            url.push(sep);
            url.push_str(&encode(name));
            url.push('=');
            url.push_str(&encode(value));
            sep = '&';
        }
        url
    }
}

fn split_query(rest: &str) -> (&str, Option<&str>) {
    match rest.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (rest, None),
    }
}

/// Decode a query per the web binding. Empty pairs (`a=1&&b=2`, a trailing
/// `&`) are skipped; a name with no `=` is an empty value.
fn decode_query(query: &str) -> Result<BTreeMap<String, String>, &'static str> {
    let mut out = BTreeMap::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let name = percent_decode(name)?;
        if name.is_empty() {
            return Err(BAD_QUERY);
        }
        let value = percent_decode(value)?;
        if out.insert(name, value).is_some() {
            return Err(BAD_QUERY);
        }
    }
    Ok(out)
}

fn percent_decode(s: &str) -> Result<String, &'static str> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            // Two hex digits, checked as such: `from_str_radix` alone would
            // take `%+F`.
            let hex = bytes
                .get(i + 1..i + 3)
                .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
                .ok_or(BAD_QUERY)?;
            let hex = std::str::from_utf8(hex).map_err(|_| BAD_QUERY)?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| BAD_QUERY)?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| BAD_QUERY)
}

/// Percent-encode everything but RFC 3986's unreserved characters.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn link(archetype: &str, intent: &str, params: &[(&str, &str)]) -> NappletLink {
        NappletLink {
            archetype: archetype.into(),
            intent: intent.into(),
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn both_spellings_name_the_same_request() {
        let want = link("profile", "open", &[("pubkey", "abc")]);
        for url in [
            "https://napplet.link/profile/open?pubkey=abc",
            "https://napplet.link/profile/open/?pubkey=abc",
            "  https://napplet.link/profile/open?pubkey=abc  ",
            "napplet:profile/open?pubkey=abc",
            "NAPPLET:profile/open?pubkey=abc",
        ] {
            assert_eq!(NappletLink::parse(url), Ok(want.clone()), "{url}");
        }
        assert_eq!(want.convention(), "napplet:profile/open");
        assert_eq!(want.payload(), json!({"pubkey": "abc"}));
    }

    #[test]
    fn any_archetype_and_intent_parse() {
        let l = NappletLink::parse("https://napplet.link/emoji-list/add").unwrap();
        assert_eq!(l, link("emoji-list", "add", &[]));
        assert_eq!(l.payload(), json!({}));
        let l = NappletLink::parse("napplet:pet/feed-2").unwrap();
        assert_eq!(l.convention(), "napplet:pet/feed-2");
    }

    /// A QR code in alphanumeric mode upper-cases everything: the path folds
    /// back to its slugs, the query is left as it came.
    #[test]
    fn an_upper_cased_link_folds_its_path_only() {
        let l = NappletLink::parse("HTTPS://NAPPLET.LINK/NOTE/OPEN?ID=ABC").unwrap();
        assert_eq!(l, link("note", "open", &[("ID", "ABC")]));
    }

    #[test]
    fn the_query_decodes_as_text_without_form_rules() {
        let l = NappletLink::parse(
            "https://napplet.link/note/open?text=a+b%20c&emoji=%F0%9F%A4%99&empty=&bare&&",
        )
        .unwrap();
        assert_eq!(
            l.payload(),
            json!({"text": "a+b c", "emoji": "🤙", "empty": "", "bare": ""})
        );
        // Escaped `&` and `=` are data.
        let l = NappletLink::parse("napplet:note/open?q=a%26b%3Dc").unwrap();
        assert_eq!(l.params["q"], "a&b=c");
    }

    #[test]
    fn a_bad_query_refuses_the_link() {
        for url in [
            "https://napplet.link/note/open?a=1&a=2",
            "https://napplet.link/note/open?a=%",
            "https://napplet.link/note/open?a=%4",
            "https://napplet.link/note/open?a=%zz",
            "https://napplet.link/note/open?a=%+F",
            "https://napplet.link/note/open?a=%FF",
            "https://napplet.link/note/open?=x",
            "napplet:note/open?a=1&a=1",
        ] {
            assert_eq!(NappletLink::parse(url), Err(BAD_QUERY), "{url}");
        }
    }

    #[test]
    fn fragments_are_dropped_on_the_web_and_refused_in_a_convention() {
        assert_eq!(
            NappletLink::parse("https://napplet.link/note/open?id=x#top"),
            Ok(link("note", "open", &[("id", "x")]))
        );
        assert_eq!(
            NappletLink::parse("https://napplet.link/note/open#top"),
            Ok(link("note", "open", &[]))
        );
        assert_eq!(
            NappletLink::parse("napplet:note/open?id=x#top"),
            Err(FRAGMENT)
        );
    }

    #[test]
    fn the_path_is_exactly_two_slugs() {
        for url in [
            "https://napplet.link",
            "https://napplet.link/",
            "https://napplet.link/note",
            "https://napplet.link/note/",
            "https://napplet.link/note/open/extra",
            "https://napplet.link/note/open//",
            "https://napplet.link//open",
            "https://napplet.link/note_x/open",
            "https://napplet.link/-note/open",
            "https://napplet.link/no%74e/open",
            "https://napplet.link/note/op.en",
            "https://napplet.link?x=1",
            "napplet:note/",
            "napplet:/open",
            "napplet:a/b/c",
        ] {
            assert_eq!(NappletLink::parse(url), Err(BAD_PATH), "{url}");
        }
    }

    /// Other hosts, ports, userinfo and schemes are some other link — the
    /// caller decides what those are, this module does not claim them.
    #[test]
    fn other_links_are_not_napplet_links() {
        for url in [
            "",
            "https://example.com/note/open",
            "https://napplet.link.evil.example/note/open",
            "https://evil.example/napplet.link/note/open",
            "https://napplet.link:8443/note/open",
            "https://user@napplet.link/note/open",
            "https://napplet.link./note/open",
            "http://napplet.link/note/open",
            "https:napplet.link/note/open",
            "nostr:note/open",
            "myco://napplet/note/open",
            "napplet://note/open",
            "napplet:naddr1qqxnzd3cxqmrzv3exgmr2wfeqgsxu35yyt0mwjjh8pcz4zprhxegz69t4wr9t74vk6zne58wzh0waycrqsqqqa28",
            "https://napplet.link/note/open with space",
            "https://napplet.link/note/open?a=\u{7}",
        ] {
            assert_eq!(NappletLink::parse(url), Err(NOT_A_NAPPLET_LINK), "{url:?}");
        }
        let long = format!(
            "https://napplet.link/note/open?q={}",
            "a".repeat(MAX_URL_LEN)
        );
        assert_eq!(NappletLink::parse(&long), Err(NOT_A_NAPPLET_LINK));
    }

    #[test]
    fn the_web_url_is_canonical_and_round_trips() {
        let l = NappletLink::parse("napplet:NOTE/Open?z=1&text=a+b%20c%2Fd&emoji=%F0%9F%A4%99")
            .unwrap();
        let web = l.web_url();
        assert_eq!(
            web,
            "https://napplet.link/note/open?emoji=%F0%9F%A4%99&text=a%2Bb%20c%2Fd&z=1"
        );
        assert_eq!(NappletLink::parse(&web), Ok(l));
        assert_eq!(
            link("profile", "open", &[]).web_url(),
            "https://napplet.link/profile/open"
        );
    }
}
