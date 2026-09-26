//! NAP-LINK — a napplet asks the user to open a link.
//!
//! ```text
//! -> { "type": "link.open", "id": "l1", "url": "https://…", "options": { "label": "…" } }
//! <- { "type": "link.open.result", "id": "l1", "status": "opened" }
//! <- { "type": "link.open.result", "id": "l1", "status": "denied", "error": "unsupported-scheme" }
//! ```
//!
//! The napplet never navigates anything itself — its frame cannot, by the
//! sandbox and the window host's navigation rules. It *asks*, and every
//! outcome ends at something the user answers:
//!
//! - `https:` / `http:` go to the system browser.
//! - `nostr:naddr1…` (or `napplet:naddr1…`) naming a **napplet** manifest
//!   (kinds 35129 / 15129 / 5129) opens Myco's install review for it, over the
//!   running napplet. Never an install: the user confirms on the review sheet
//!   (napplet-runtime §7.7).
//! - Anything else is `denied` with `unsupported-scheme`; a URL that does not
//!   parse is `denied` with `invalid-url`.
//!
//! `status: "opened"` means the link was handed to that user-facing surface —
//! not that the user went on to accept it. A napplet learns nothing about what
//! the user did next, which is the point.
//!
//! This module only classifies. Whether a valid link is admitted right now —
//! a review already on screen, a napplet asking too often — is the host's call,
//! because only the host can see its own UI. So a valid link comes back as
//! [`Outcome::Link`], and the host answers with [`LinkRequest::opened`] or
//! [`LinkRequest::denied`].
//!
//! `options.label` is untrusted display text and is never used to decide
//! anything; the host does not show it next to the URL, where it could claim
//! to be a different destination.

use nostr::nips::nip19::{FromBech32, Nip19Coordinate};

use crate::dispatch::Outcome;
use crate::manifest::{KIND_NAMED, KIND_ROOT, KIND_SNAPSHOT};
use crate::seams::Envelope;

/// The longest URL accepted. Generous for any real link, and a bound on what
/// a napplet can push through the host into an intent.
pub const MAX_URL_LEN: usize = 4096;

/// `error` for a URL that does not parse.
pub const INVALID_URL: &str = "invalid-url";
/// `error` for a well-formed URL this runtime will not open.
pub const UNSUPPORTED_SCHEME: &str = "unsupported-scheme";
/// `error` for a link refused by the host's policy (rate limit, a review
/// already showing).
pub const BLOCKED_BY_POLICY: &str = "blocked-by-policy";
/// `error` for a link the user declined.
pub const USER_DENIED: &str = "user-denied";

/// Where a valid link goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// An `http:` / `https:` URL, for the system browser. As the napplet sent
    /// it, trimmed.
    Web(String),
    /// A napplet pointer — the bare `naddr1…`, lower-cased — for install
    /// review.
    Napplet(String),
}

/// A valid `link.open`, awaiting the host's decision.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkRequest {
    pub target: LinkTarget,
    call: Envelope,
}

impl LinkRequest {
    /// The answer when the host handed the link to the user.
    pub fn opened(&self) -> Envelope {
        self.call.to_result().with_field("status", "opened")
    }

    /// The answer when the host refused it; `error` is one of the codes above
    /// or another short string.
    pub fn denied(&self, error: &str) -> Envelope {
        denied(&self.call, error)
    }
}

fn denied(call: &Envelope, error: &str) -> Envelope {
    call.to_result()
        .with_field("status", "denied")
        .with_field("error", error)
}

/// Handle an inbound `link.*` message.
pub fn handle(message: &Envelope) -> Outcome {
    if message.action() != "open" {
        // NIP-5D: unrecognized types are ignored.
        return Outcome::Reply(Vec::new());
    }
    let Some(url) = message.field("url").and_then(|u| u.as_str()) else {
        return Outcome::Reply(vec![denied(message, INVALID_URL)]);
    };
    match classify(url) {
        Ok(target) => Outcome::Link(LinkRequest {
            target,
            call: message.clone(),
        }),
        Err(error) => Outcome::Reply(vec![denied(message, error)]),
    }
}

/// Decide where `url` would go, or why it goes nowhere.
pub fn classify(url: &str) -> Result<LinkTarget, &'static str> {
    let url = url.trim();
    if url.is_empty() || url.len() > MAX_URL_LEN {
        return Err(INVALID_URL);
    }
    // No whitespace or control characters anywhere: a real URL has them
    // percent-encoded, and one smuggled raw into an intent is a URL the user
    // is not shown correctly.
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(INVALID_URL);
    }
    let Some((scheme, rest)) = url.split_once(':') else {
        return Err(INVALID_URL);
    };
    if !is_scheme(scheme) {
        return Err(INVALID_URL);
    }

    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" => {
            let Some(after) = rest.strip_prefix("//") else {
                return Err(INVALID_URL);
            };
            let authority = after.split(['/', '?', '#']).next().unwrap_or("");
            // Userinfo is refused: `https://bank.example@evil.example/` reads
            // as one host and goes to another.
            if authority.is_empty() || authority.contains('@') {
                return Err(INVALID_URL);
            }
            Ok(LinkTarget::Web(url.to_string()))
        }
        "nostr" | "napplet" => {
            let entity = rest.trim_start_matches("//").trim_end_matches('/');
            let lowered = entity.to_ascii_lowercase();
            // Another NIP-19 entity — a profile, a note — is a fine `nostr:`
            // link, just not one this runtime opens.
            if !lowered.starts_with("naddr1") {
                return if scheme.eq_ignore_ascii_case("napplet") {
                    Err(INVALID_URL)
                } else if is_nip19_entity(&lowered) {
                    Err(UNSUPPORTED_SCHEME)
                } else {
                    Err(INVALID_URL)
                };
            }
            let coordinate = Nip19Coordinate::from_bech32(&lowered).map_err(|_| INVALID_URL)?;
            let kind = coordinate.coordinate.kind.as_u16();
            if kind == KIND_NAMED || kind == KIND_ROOT || kind == KIND_SNAPSHOT {
                Ok(LinkTarget::Napplet(lowered))
            } else {
                // An naddr for something that is not a napplet — an nsite,
                // an article. Opening it is not this runtime's business.
                Err(UNSUPPORTED_SCHEME)
            }
        }
        _ => Err(UNSUPPORTED_SCHEME),
    }
}

/// RFC 3986 `scheme`: a letter, then letters, digits, `+`, `-`, `.`.
fn is_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Whether `s` starts with a NIP-19 prefix — shape only, not a decode.
fn is_nip19_entity(s: &str) -> bool {
    ["npub1", "nprofile1", "note1", "nevent1", "nrelay1", "nsec1"]
        .iter()
        .any(|p| s.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::dispatch;
    use crate::session::{NappletIdentity, Session};
    use crate::testing::test_context;
    use nostr::nips::nip01::Coordinate;
    use nostr::nips::nip19::ToBech32;
    use nostr::{Keys, Kind};

    fn naddr(kind: u16, d: &str) -> String {
        let coordinate =
            Coordinate::new(Kind::from(kind), Keys::generate().public_key()).identifier(d);
        Nip19Coordinate::new(coordinate, Vec::<nostr::RelayUrl>::new())
            .to_bech32()
            .unwrap()
    }

    fn open(url: &str) -> Envelope {
        Envelope::new("link.open")
            .with_id("l1")
            .with_field("url", url)
            .with_field("options", serde_json::json!({"label": "Totally your bank"}))
    }

    async fn established() -> (crate::dispatch::NapContext, Session) {
        let (ctx, _signer) = test_context();
        let mut s = Session::new(
            NappletIdentity::new("chat", "aggregate"),
            crate::session::DEFAULT_GRANTS.to_vec(),
        );
        s.on_ready();
        (ctx, s)
    }

    /// The headline case: a napplet pointing at another napplet produces a
    /// review request — and nothing that could install it.
    #[tokio::test]
    async fn a_napplet_naddr_asks_the_host_for_a_review() {
        let (ctx, mut s) = established().await;
        let pointer = naddr(KIND_NAMED, "dingdong");
        let out = dispatch(&ctx, &mut s, &open(&format!("nostr:{pointer}"))).await;
        let Outcome::Link(request) = out else {
            panic!("expected a link request, got {out:?}");
        };
        assert_eq!(request.target, LinkTarget::Napplet(pointer));
        let r = serde_json::to_value(request.opened()).unwrap();
        assert_eq!(
            r,
            serde_json::json!({"type": "link.open.result", "id": "l1", "status": "opened"})
        );
    }

    #[test]
    fn every_napplet_kind_and_spelling_is_a_review() {
        for kind in [KIND_NAMED, KIND_ROOT, KIND_SNAPSHOT] {
            let pointer = naddr(kind, if kind == KIND_NAMED { "x" } else { "" });
            for url in [
                format!("nostr:{pointer}"),
                format!("nostr://{pointer}"),
                format!("NOSTR:{}", pointer.to_ascii_uppercase()),
                format!("napplet:{pointer}"),
                format!("napplet://{pointer}/"),
            ] {
                assert_eq!(
                    classify(&url),
                    Ok(LinkTarget::Napplet(pointer.clone())),
                    "{url}"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_web_link_goes_to_the_browser() {
        let (ctx, mut s) = established().await;
        let out = dispatch(&ctx, &mut s, &open("https://example.com/a?b=c#d")).await;
        let Outcome::Link(request) = out else {
            panic!("expected a link request");
        };
        assert_eq!(
            request.target,
            LinkTarget::Web("https://example.com/a?b=c#d".into())
        );
        assert_eq!(
            classify("http://example.com"),
            Ok(LinkTarget::Web("http://example.com".into()))
        );
    }

    async fn denied_with(url: &str) -> String {
        let (ctx, mut s) = established().await;
        let out = dispatch(&ctx, &mut s, &open(url)).await;
        let sent = out.envelopes();
        assert_eq!(sent.len(), 1, "{url}: {out:?}");
        assert_eq!(sent[0].msg_type, "link.open.result");
        assert_eq!(sent[0].id.as_deref(), Some("l1"));
        assert_eq!(sent[0].field("status").unwrap(), "denied", "{url}");
        sent[0]
            .field("error")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn other_schemes_and_entities_are_unsupported() {
        let npub = Keys::generate().public_key().to_bech32().unwrap();
        for url in [
            "intent://scan#Intent;scheme=zxing;end".to_string(),
            "tel:+15550100".to_string(),
            "javascript:alert(1)".to_string(),
            "file:///sdcard/secret".to_string(),
            "myco://napplet/x".to_string(),
            format!("nostr:{npub}"),
            // An naddr, but for an nsite (35128) — not a napplet.
            format!("nostr:{}", naddr(35128, "site")),
            // A long-form article.
            format!("nostr:{}", naddr(30023, "post")),
        ] {
            assert_eq!(denied_with(&url).await, UNSUPPORTED_SCHEME, "{url}");
        }
    }

    #[tokio::test]
    async fn malformed_links_are_invalid() {
        for url in [
            "",
            "   ",
            "not a url",
            "no-scheme-here",
            "1http://x",
            "https:example.com",
            "https://",
            "https:///path",
            "https://bank.example@evil.example/",
            "https://exa mple.com",
            "nostr:naddr1notbech32",
            "nostr:garbage",
            "napplet:npub1whatever",
        ] {
            assert_eq!(denied_with(url).await, INVALID_URL, "{url:?}");
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN));
        assert_eq!(denied_with(&long).await, INVALID_URL);

        // A missing url is invalid too, not a crash.
        let (ctx, mut s) = established().await;
        let out = dispatch(&ctx, &mut s, &Envelope::new("link.open").with_id("l1")).await;
        assert_eq!(out.envelopes()[0].field("error").unwrap(), INVALID_URL);
    }

    /// The label never decides anything: two calls differing only in label
    /// classify the same.
    #[test]
    fn the_label_is_display_text_only() {
        let a = handle(&open("https://example.com"));
        let b = handle(
            &Envelope::new("link.open")
                .with_id("l1")
                .with_field("url", "https://example.com"),
        );
        let (Outcome::Link(a), Outcome::Link(b)) = (a, b) else {
            panic!("expected link requests");
        };
        assert_eq!(a.target, b.target);
    }

    #[test]
    fn a_denial_carries_status_and_error() {
        let Outcome::Link(request) = handle(&open("https://example.com")) else {
            panic!("expected a link request");
        };
        let r = serde_json::to_value(request.denied(BLOCKED_BY_POLICY)).unwrap();
        assert_eq!(
            r,
            serde_json::json!({
                "type": "link.open.result",
                "id": "l1",
                "status": "denied",
                "error": "blocked-by-policy"
            })
        );
    }

    /// `link` is gated like every other domain: ungranted, it is refused
    /// before any classification happens.
    #[tokio::test]
    async fn an_ungranted_link_is_refused() {
        let (ctx, _signer) = test_context();
        let mut s = Session::new(NappletIdentity::new("chat", "aggregate"), ["relay"]);
        s.on_ready();
        let out = dispatch(&ctx, &mut s, &open("https://example.com")).await;
        let sent = out.envelopes();
        assert_eq!(sent.len(), 1);
        assert!(sent[0]
            .field("error")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("link"));
    }
}
