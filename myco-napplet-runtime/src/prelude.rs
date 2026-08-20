//! The `window.napplet` prelude: the capability namespace a napplet calls
//! through.
//!
//! NIP-5D requires the namespace exist before any napplet script runs, and the
//! shell cannot script into an opaque origin from outside — so the prelude
//! rides into the `srcdoc` bytes alongside the CSP, injected by
//! [`crate::artifact`] after verification and outside the aggregate.
//!
//! The prelude itself is upstream's, vendored verbatim (see
//! `assets/vendor/README.md`). Upstream owns the domain surface, so tracking
//! their build keeps Myco's `window.napplet.*` identical to every other
//! conformant runtime's; hand-writing one would mean re-deriving their
//! interface on every registry change.
//!
//! ## Every API is injected, always
//!
//! [`render`] installs everything this build implements, regardless of what the
//! user granted. The permission lives *behind* the call: `window.napplet.relay`
//! exists for every napplet, and a napplet without a `relay` grant gets a
//! refusal when it publishes, not a missing object when it looks.
//!
//! The difference matters to the napplet's own logic. An absent namespace entry
//! reads as "this runtime cannot do relay", which is permanent and sends the
//! napplet down its fallback path for good. A refused call reads as "not right
//! now", which it can surface, retry, or ask about — and which stays true when
//! the user changes their mind later without the napplet reloading.
//!
//! Enforcement is [`crate::dispatch`], which refuses ungranted domains whatever
//! the namespace contains — a napplet can always `postMessage` directly, so the
//! namespace was never the boundary.

use crate::session::Session;

/// The vendored prelude IIFE. Defines one global, `NappletShimPrelude`.
const PRELUDE_IIFE: &str = include_str!("../assets/vendor/napplet-shim-prelude.global.js");

/// The global the vendored IIFE defines.
pub const PRELUDE_GLOBAL: &str = "NappletShimPrelude";

/// Render the prelude for a set of domains: the vendored installer, then the
/// call that activates it.
pub fn render(domains: &[String]) -> String {
    let allowlist = serde_json::json!({ "domains": domains });
    format!(
        "{PRELUDE_IIFE}\n{PRELUDE_GLOBAL}.install({allowlist});\n",
        allowlist = allowlist
    )
}

/// Render the prelude for a session — every domain this runtime implements,
/// granted or not. See the module docs for why grants do not filter this.
pub fn render_for(session: &Session) -> String {
    render(&session.available_domains())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{NappletIdentity, Session};

    fn session(granted: &[&str], implemented: &[&str]) -> Session {
        Session::with_implemented(
            NappletIdentity::new("chat", "aggregate"),
            granted.to_vec(),
            implemented.to_vec(),
        )
    }

    /// The vendored artifact has to actually define the global we then call, or
    /// every napplet gets a document that throws before its own code runs.
    #[test]
    fn the_vendored_prelude_defines_the_global_we_activate() {
        assert!(
            PRELUDE_IIFE.contains(&format!("var {PRELUDE_GLOBAL}")),
            "the vendored prelude no longer defines {PRELUDE_GLOBAL}"
        );
        assert!(
            PRELUDE_IIFE.len() > 1000,
            "the vendored prelude looks empty"
        );
    }

    #[test]
    fn the_activation_call_follows_the_installer() {
        let out = render(&["shell".to_string()]);
        let install = out.rfind(&format!("{PRELUDE_GLOBAL}.install(")).unwrap();
        let define = out.find(&format!("var {PRELUDE_GLOBAL}")).unwrap();
        assert!(define < install, "activated before the global is defined");
        assert!(out.trim_end().ends_with(");"));
    }

    /// Every implemented API is installed, whether or not it was granted.
    #[test]
    fn the_allowlist_is_everything_this_build_implements() {
        // Granted nothing at all...
        let s = session(&[], &["shell", "relay"]);
        let out = render_for(&s);
        // ...and `relay` is still installed, because the permission is checked
        // when the napplet calls it, not when it looks for it.
        assert!(out.contains(r#"{"domains":["relay","shell"]}"#));
    }

    /// A domain this build does not implement is not installed, however it was
    /// granted — there would be nothing behind it.
    #[test]
    fn an_unimplemented_domain_is_not_installed() {
        let s = session(&["storage"], &["shell"]);
        assert!(render_for(&s).contains(r#"{"domains":["shell"]}"#));
    }

    #[test]
    fn a_napplet_granted_nothing_still_gets_the_shell_domain() {
        let s = session(&[], &["shell"]);
        assert!(render_for(&s).contains(r#"{"domains":["shell"]}"#));
    }
}
