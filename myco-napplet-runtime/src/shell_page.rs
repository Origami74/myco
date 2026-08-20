//! The shell page: trusted HTML shipped with the runtime.
//!
//! Served by `NappletActivity`'s own request interception at the per-napplet
//! shell origin, and by nothing else. It is deliberately not updatable over the
//! mesh — it is code, not content, and a napplet that could replace it would be
//! replacing the thing that contains it.
//!
//! It is embedded in the binary rather than shipped as an Android asset so the
//! page and the runtime that speaks to it cannot drift apart: the framing in
//! [`crate::shell_link`] and the JavaScript that writes it are compiled from
//! the same commit.

/// The name the capability channel is injected under, matching the shell page.
///
/// Registered with `addWebMessageListener` against **this window's shell origin
/// alone** — never a wildcard. The napplet's iframe has an opaque origin, so it
/// matches no rule and never receives the object.
pub const RUNTIME_OBJECT: &str = "mycoNappletRuntime";

/// The shell page's HTML.
pub fn shell_page() -> &'static str {
    include_str!("../assets/shell.html")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::SrcdocArtifact;

    #[test]
    fn the_page_and_the_runtime_agree_on_the_object_name() {
        assert!(
            shell_page().contains(RUNTIME_OBJECT),
            "the shell page reads a different injected object than the runtime registers"
        );
    }

    /// The napplet's iframe must never be created with `allow-same-origin`. The
    /// page takes the sandbox from the runtime's load command rather than
    /// hardcoding one, so what it must not contain is a literal grant.
    #[test]
    fn the_page_never_writes_allow_same_origin() {
        assert!(!shell_page().contains("allow-same-origin"));
        assert_eq!(SrcdocArtifact::SANDBOX, "allow-scripts");
    }

    /// Every inbound message is bound to the frame the shell created. Without
    /// this check any window that can reach the page could speak as the napplet.
    #[test]
    fn the_page_verifies_the_message_source() {
        assert!(shell_page().contains("event.source !== frame.contentWindow"));
    }

    /// The shell wraps what it relays. Forwarding raw would let a napplet
    /// choose its own channel tag.
    #[test]
    fn the_page_wraps_relayed_messages() {
        assert!(shell_page().contains("channel: 'napplet'"));
    }

    /// The shell holds no key and opens no connection of its own.
    #[test]
    fn the_page_opens_nothing() {
        for forbidden in [
            "fetch(",
            "XMLHttpRequest",
            "WebSocket",
            "EventSource",
            "import(",
        ] {
            assert!(
                !shell_page().contains(forbidden),
                "the shell page reaches the network with {forbidden}"
            );
        }
    }
}
