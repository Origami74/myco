//! Assembling the bytes that go into `iframe.srcdoc`.
//!
//! ## The napplet always runs in an iframe
//!
//! Never as the WebView's top-level document. The WebView loads the **shell** —
//! Myco's own trusted page, shipped in the APK — and the shell creates a
//! `sandbox="allow-scripts"` iframe whose `srcdoc` is what this module builds.
//! NIP-5D requires exactly this, and requires that `allow-same-origin` is never
//! among the tokens.
//!
//! The isolation comes from that nesting. `srcdoc` without `allow-same-origin`
//! gives the napplet an **opaque origin**: no `localStorage`, no `IndexedDB`, no
//! cookies, no reach into the shell's DOM, and no way to read anything belonging
//! to the shell's origin or to another napplet. Load the same bytes as the
//! WebView's top-level document instead and every one of those protections is
//! gone — the napplet would *be* the shell origin, inherit its storage, and (on
//! device) sit inside the origin the capability channel is scoped to.
//!
//! That is why [`assemble`] returns a [`SrcdocArtifact`] rather than a `String`:
//! the type says where the bytes are allowed to go.
//!
//! Two things have to reach the napplet's document that are **not** the
//! napplet's own bytes: a Content-Security-Policy, and the `window.napplet`
//! prelude. Both are injected here, after verification, and neither is part of
//! the aggregate — NIP-5D is explicit that the injected policy "MUST remain
//! outside the bytes used to compute `aggregateHash`, just like the injected
//! `window.napplet` namespace". Assembling *after* the hash check is what keeps
//! that true by construction: [`crate::resolve`] hands over verified bytes and
//! nothing here feeds back into identity.
//!
//! ## Why a `meta` and not a header
//!
//! A `srcdoc` document has no HTTP response to carry a CSP header on, so the
//! policy has to be a `<meta http-equiv>` — and it only governs what is parsed
//! *after* it. NIP-5D therefore requires it be the first element in `head`,
//! before any napplet-controlled resource or script.
//!
//! ## Why the injection point is position zero
//!
//! The obvious implementation searches the document for `<head>` and inserts
//! after it. That hands the placement decision to attacker-controlled text: a
//! napplet that opens with `<!-- <head> -->` gets our policy injected into a
//! comment, and silently loses CSP entirely while appearing to have it.
//!
//! So nothing is searched for. The document is rebuilt as
//!
//! ```text
//! <!doctype html> <meta CSP> <script prelude> <the napplet's bytes>
//! ```
//!
//! and the HTML parser does the rest: a `<meta>` before `<html>` is placed by
//! the "before html" / "before head" insertion modes as the first child of a
//! head the parser creates. The only text inspected is the very start of the
//! document, to drop a doctype the napplet declared itself — a position no
//! comment can move.

/// The Content-Security-Policy injected into every napplet document.
///
/// The default is deny-everything plus exactly what a single-file napplet
/// needs. An opaque origin does not, on its own, stop `fetch`, WebSocket,
/// `Worker`, or subresource loads — NIP-5D says so directly — so this is what
/// actually keeps a napplet off the network. Everything it is allowed to reach
/// the outside world with goes through the capability seam instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CspPolicy(String);

impl Default for CspPolicy {
    fn default() -> Self {
        Self(
            [
                // Nothing loads unless a directive below says otherwise.
                "default-src 'none'",
                // A napplet's executable JS is necessarily inline: an opaque
                // origin has no server to fetch a `<script src>` from.
                "script-src 'unsafe-inline'",
                "style-src 'unsafe-inline'",
                // Self-contained assets the build tooling inlines.
                "img-src data: blob:",
                "font-src data:",
                "media-src data: blob:",
                // The point of the exercise: no fetch, no WebSocket, no
                // EventSource. Relay access is a capability, not a socket.
                "connect-src 'none'",
                "form-action 'none'",
                "base-uri 'none'",
                "object-src 'none'",
            ]
            .join("; "),
        )
    }
}

impl CspPolicy {
    /// A policy from an explicit directive string. The caller owns its
    /// correctness — [`CspPolicy::default`] is what napplets are held to.
    pub fn custom(policy: impl Into<String>) -> Self {
        Self(policy.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What to inject alongside the napplet's own bytes.
#[derive(Debug, Clone, Default)]
pub struct Injection<'a> {
    pub csp: CspPolicy,
    /// The `window.napplet` prelude: trusted JavaScript, shipped in the APK,
    /// never fetched and never mesh-updatable. NIP-5D requires the namespace be
    /// installed before any napplet script runs, and the shell cannot script
    /// into an opaque origin from outside — so it rides in here.
    ///
    /// `None` assembles a document with no capability surface at all, which is
    /// what a napplet granted nothing should see.
    pub prelude_js: Option<&'a str>,
}

/// The assembled document for a verified napplet.
///
/// **Only ever assigned to `iframe.srcdoc`, on an iframe carrying
/// `sandbox="allow-scripts"` and not `allow-same-origin`.** Never navigated to,
/// never served over HTTP, never made a WebView's top-level document — each of
/// those would hand the napplet the shell's origin and undo the isolation the
/// sandbox exists to provide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrcdocArtifact(String);

impl SrcdocArtifact {
    /// The document bytes, for assignment to `srcdoc`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The `sandbox` attribute the hosting iframe must carry. Fixed by NIP-5D:
    /// `allow-scripts` and nothing else. `allow-same-origin` alongside
    /// `allow-scripts` would let the frame reach out and clear its own sandbox.
    pub const SANDBOX: &'static str = "allow-scripts";
}

/// Build the `srcdoc` document for a verified napplet.
pub fn assemble(index_html: &str, injection: &Injection<'_>) -> SrcdocArtifact {
    let body = strip_leading_doctype(index_html);

    let mut out = String::with_capacity(index_html.len() + 1024);
    out.push_str("<!doctype html>\n");
    out.push_str("<meta http-equiv=\"Content-Security-Policy\" content=\"");
    out.push_str(&escape_attribute(injection.csp.as_str()));
    out.push_str("\">\n");
    if let Some(prelude) = injection.prelude_js {
        out.push_str("<script>");
        out.push_str(&neutralize_script_end(prelude));
        out.push_str("</script>\n");
    }
    out.push_str(body);
    SrcdocArtifact(out)
}

/// Drop a doctype the napplet declared, so ours is first and the document does
/// not fall into quirks mode. Only the very start of the document is inspected;
/// a doctype anywhere else is ignored by the parser anyway.
fn strip_leading_doctype(html: &str) -> &str {
    let trimmed = html.trim_start();
    let Some(rest) = strip_prefix_ignore_ascii_case(trimmed, "<!doctype") else {
        return trimmed;
    };
    match rest.find('>') {
        Some(end) => rest[end + 1..].trim_start(),
        // An unterminated doctype: the whole document is one malformed tag.
        None => "",
    }
}

fn strip_prefix_ignore_ascii_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

/// Escape a double-quoted attribute value. The policy is ours, not the
/// napplet's, but a policy that escaped its own attribute would end the `meta`
/// tag early and put the rest into the document as markup.
fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Keep a prelude from closing its own `<script>` element. Same reasoning as
/// above: trusted source, but a `</script>` inside it would hand the remainder
/// of the prelude to the parser as markup.
fn neutralize_script_end(js: &str) -> String {
    js.replace("</script", "<\\/script")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assembled(html: &str) -> String {
        assemble(html, &Injection::default()).as_str().to_string()
    }

    /// The CSP has to be the first element the parser sees, or the napplet's own
    /// markup is parsed under no policy at all.
    #[test]
    fn the_policy_precedes_every_napplet_byte() {
        let out = assembled("<html><head><script>alert(1)</script></head></html>");
        let meta = out.find("Content-Security-Policy").unwrap();
        let napplet = out.find("alert(1)").unwrap();
        assert!(meta < napplet);
        assert!(out.starts_with("<!doctype html>"));
    }

    /// The trap this module is built around: a napplet that opens with a fake
    /// head in a comment must not be able to steer the policy into it.
    #[test]
    fn a_decoy_head_cannot_capture_the_policy() {
        let decoy = "<!-- <head> --><html><head><script>steal()</script></head></html>";
        let out = assembled(decoy);

        let meta = out.find("Content-Security-Policy").unwrap();
        assert!(
            meta < out.find("<!--").unwrap(),
            "policy landed after the decoy"
        );
        assert!(meta < out.find("steal()").unwrap());
    }

    /// A napplet's own doctype is dropped rather than left to precede ours,
    /// which would either duplicate it or drop the page into quirks mode.
    #[test]
    fn a_napplet_doctype_is_replaced_not_duplicated() {
        for html in [
            "<!doctype html><p>hi</p>",
            "<!DOCTYPE HTML><p>hi</p>",
            "  \n<!DoCtYpE html>\n<p>hi</p>",
        ] {
            let out = assemble(html, &Injection::default());
            let out = out.as_str();
            assert_eq!(
                out.to_lowercase().matches("<!doctype").count(),
                1,
                "{html:?}"
            );
            assert!(out.starts_with("<!doctype html>"));
            assert!(out.ends_with("<p>hi</p>"));
        }
    }

    #[test]
    fn a_document_with_no_doctype_is_left_alone() {
        let out = assembled("<p>hi</p>");
        assert!(out.starts_with("<!doctype html>"));
        assert!(out.ends_with("<p>hi</p>"));
    }

    #[test]
    fn the_default_policy_denies_the_network() {
        let csp = CspPolicy::default();
        assert!(csp.as_str().contains("default-src 'none'"));
        assert!(csp.as_str().contains("connect-src 'none'"));
        // Inline script is unavoidable — an opaque origin has no server to
        // fetch an external one from — so it must be granted deliberately.
        assert!(csp.as_str().contains("script-src 'unsafe-inline'"));
    }

    #[test]
    fn the_prelude_is_injected_before_the_napplet_and_after_the_policy() {
        let out = assemble(
            "<p>hi</p>",
            &Injection {
                prelude_js: Some("window.napplet = {};"),
                ..Default::default()
            },
        );
        let out = out.as_str();
        let meta = out.find("Content-Security-Policy").unwrap();
        let prelude = out.find("window.napplet").unwrap();
        let napplet = out.find("<p>hi</p>").unwrap();
        assert!(meta < prelude && prelude < napplet);
    }

    /// Absent a prelude the document carries no capability surface at all —
    /// what a napplet granted nothing should see.
    #[test]
    fn no_prelude_means_no_script_element() {
        assert!(!assembled("<p>hi</p>").contains("<script>"));
    }

    #[test]
    fn a_prelude_cannot_close_its_own_script_element() {
        let out = assemble(
            "<p>napplet</p>",
            &Injection {
                prelude_js: Some("var s = '</script><img onerror=x>';"),
                ..Default::default()
            },
        );
        let out = out.as_str();
        let opened = out.find("<script>").unwrap();
        let closed = out.find("</script>").unwrap();
        assert!(
            !out[opened..closed].contains("</script"),
            "prelude closed its own element"
        );
    }

    #[test]
    fn a_policy_cannot_escape_its_attribute() {
        let out = assemble(
            "<p>hi</p>",
            &Injection {
                csp: CspPolicy::custom("default-src 'none'\"><script>escaped()</script>"),
                ..Default::default()
            },
        );
        let out = out.as_str();
        assert!(!out.contains("<script>escaped()"));
        assert!(out.contains("&quot;"));
    }
}

#[cfg(test)]
mod sandbox_tests {
    use super::*;

    /// The napplet runs in an iframe, never as the top-level document, and the
    /// sandbox is exactly `allow-scripts`. `allow-same-origin` alongside it
    /// would let the frame reach into the shell's origin and clear its own
    /// sandbox — every isolation guarantee here rests on its absence.
    #[test]
    fn the_sandbox_never_grants_same_origin() {
        assert_eq!(SrcdocArtifact::SANDBOX, "allow-scripts");
        assert!(!SrcdocArtifact::SANDBOX.contains("allow-same-origin"));
    }
}
