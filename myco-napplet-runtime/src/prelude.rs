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
//! ## One source for the namespace and for `supports()`
//!
//! [`render`] takes the domains a session offers and passes exactly those to
//! the installer, which installs only what it is given. The same set goes into
//! `shell.init`. So a napplet that was not granted `relay` finds no
//! `window.napplet.relay` to call *and* is told `supports("relay") === false`:
//! the namespace and the environment cannot disagree, because they are computed
//! from one value.
//!
//! That is defence in depth, not the enforcement itself. A napplet can always
//! `postMessage` whatever it likes; [`crate::dispatch`] refuses ungranted
//! domains regardless of what its namespace contains.
//!
//! Note also that the vendored bundle *contains* every domain's
//! implementation — the allowlist decides what is installed, not what ships.
//! A napplet without a `relay` grant has no `window.napplet.relay`, but the
//! code for one is still in its document. Nothing in the runtime relies on the
//! bytes being absent.

use crate::session::Session;

/// The vendored prelude IIFE. Defines one global, `NappletShimPrelude`.
const PRELUDE_IIFE: &str = include_str!("../assets/vendor/napplet-shim-prelude.global.js");

/// The global the vendored IIFE defines.
pub const PRELUDE_GLOBAL: &str = "NappletShimPrelude";

/// Render the prelude for a set of offered domains: the vendored installer,
/// then the call that activates it with this napplet's allowlist.
pub fn render(domains: &[String]) -> String {
    let allowlist = serde_json::json!({ "domains": domains });
    format!(
        "{PRELUDE_IIFE}\n{PRELUDE_GLOBAL}.install({allowlist});\n",
        allowlist = allowlist
    )
}

/// Render the prelude for a session — the domains it offers, and nothing else.
pub fn render_for(session: &Session) -> String {
    render(&session.offered_domains())
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

    /// The property worth having: a napplet is handed exactly the domains its
    /// session offers, so its namespace cannot promise more than `supports()`.
    #[test]
    fn the_allowlist_is_the_sessions_offered_set() {
        let s = session(&["relay", "storage"], &["shell", "relay"]);
        let offered = s.offered_domains();
        assert_eq!(offered, vec!["relay".to_string(), "shell".to_string()]);

        let out = render_for(&s);
        assert!(out.contains(r#"{"domains":["relay","shell"]}"#));
        // `storage` was granted but is not implemented, so it is neither
        // offered nor installed.
        assert!(!out.ends_with("storage\"]});\n"));
    }

    #[test]
    fn a_napplet_granted_nothing_still_gets_the_shell_domain() {
        let s = session(&[], &["shell"]);
        assert!(render_for(&s).contains(r#"{"domains":["shell"]}"#));
    }
}
