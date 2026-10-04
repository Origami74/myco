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
//! Two things the vendored build does not carry are supplied by Myco's own
//! supplement (`assets/myco-prelude.js`), installed right after it: the
//! napplet-side half of NAP-SHELL (`shell.supports()` and `shell.services`,
//! which the vendored shim has no `shell` domain for), and NAP-MESH, which the
//! vendored installer filters out because it is not in the upstream registry.
//! The supplement takes the same domain list, so the two cannot disagree.
//!
//! The supplement also replaces one vendored function, `outbox.publish`, with
//! a copy that posts the same envelope and resolves the same result object but
//! waits [`SIGNING_TIMEOUT`] instead of the vendored 30 s — a publish is signed
//! first, and with a signer app that is a person approving it. `mesh.publish`
//! gets the same wait. (`relay.publish` needs nothing: the vendored one has no
//! timer.)
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

/// Myco's supplement: NAP-SHELL's napplet side, and NAP-MESH.
const SUPPLEMENT_IIFE: &str = include_str!("../assets/myco-prelude.js");

/// The global the supplement defines.
pub const SUPPLEMENT_GLOBAL: &str = "MycoPrelude";

/// How long the prelude's publishes (`outbox.publish`, `mesh.publish`) wait
/// for their result before rejecting with "timed out" — the supplement's
/// `SIGNING_TIMEOUT_MS`, mirrored here so the host's own signing bound can be
/// checked against it. It is a backstop for a runtime that never answers:
/// whatever signs a napplet's events must give up (and answer `ok: false`)
/// well before it.
pub const SIGNING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Render the prelude for a set of domains: the vendored installer, the call
/// that activates it, and the readiness signal.
///
/// ## Why we send `shell.ready` and the shim does not
///
/// NAP-SHELL says the signal is "normally emitted automatically by the
/// runtime-provided shim at load" — and the vendored `@napplet/shim` prelude
/// carries no `shell` domain at all, so it never sends one. The reference web
/// runtime posts it from its own injected namespace for the same reason.
///
/// Without it nothing establishes the session, and every capability call is
/// refused with "session not established" — long after the napplet believes it
/// started up, and with no message visible to say what is missing.
///
/// It goes **after** `install`, not before: `shell.ready` means "my receiver is
/// live", and the receiver is what `install` puts in place. Sent first, the
/// runtime would answer `shell.init` into a napplet that is not listening yet,
/// and the environment would be lost.
///
/// A duplicate is harmless — NAP-SHELL requires a second `shell.ready` be
/// idempotent — so a shim that starts sending its own costs nothing.
///
/// The supplement is installed between the two, for the same reason: its
/// `shell.init` listener must be live before `shell.ready` invites the reply.
pub fn render(domains: &[String]) -> String {
    let allowlist = serde_json::json!({ "domains": domains });
    format!(
        "{PRELUDE_IIFE}\n\
         {PRELUDE_GLOBAL}.install({allowlist});\n\
         {SUPPLEMENT_IIFE}\n\
         {SUPPLEMENT_GLOBAL}.install({allowlist});\n\
         parent.postMessage({{ type: \"shell.ready\" }}, \"*\");\n",
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

    /// The supplement is defined, then activated with the *same* allowlist,
    /// after the vendored installer has put `window.napplet` in place and
    /// before readiness is signalled — its `shell.init` listener has to be
    /// live before the reply it invites.
    #[test]
    fn the_supplement_runs_after_the_vendored_installer_and_before_ready() {
        let out = render(&["mesh".to_string(), "shell".to_string()]);
        let vendored = out.rfind(&format!("{PRELUDE_GLOBAL}.install(")).unwrap();
        let define = out.find(&format!("var {SUPPLEMENT_GLOBAL}")).unwrap();
        let supplement = out
            .rfind(&format!(
                "{SUPPLEMENT_GLOBAL}.install({{\"domains\":[\"mesh\",\"shell\"]}})"
            ))
            .unwrap();
        let ready = out.rfind("shell.ready").unwrap();
        assert!(vendored < define && define < supplement && supplement < ready);
    }

    /// What the supplement is for: the two namespaces the vendored build lacks.
    #[test]
    fn the_supplement_installs_shell_and_mesh() {
        assert!(SUPPLEMENT_IIFE.contains("installShell("));
        assert!(SUPPLEMENT_IIFE.contains("installMesh("));
        assert!(SUPPLEMENT_IIFE.contains(r#"domains.has("mesh")"#));
        for wire in ["mesh.info", "mesh.publish", "mesh.subscribe", "mesh.close"] {
            assert!(
                SUPPLEMENT_IIFE.contains(&format!(r#"type: "{wire}""#)),
                "the supplement never sends {wire}"
            );
        }
        for push in ["mesh.event", "mesh.eose", "mesh.closed", "shell.init"] {
            assert!(
                SUPPLEMENT_IIFE.contains(&format!(r#""{push}""#)),
                "the supplement never routes {push}"
            );
        }
    }

    /// The supplement's timer is the constant the host checks its signing
    /// bound against — they cannot drift apart.
    #[test]
    fn the_supplement_signing_timeout_matches_the_constant() {
        let js = format!("var SIGNING_TIMEOUT_MS = {};", SIGNING_TIMEOUT.as_millis());
        assert!(SUPPLEMENT_IIFE.contains(&js), "expected `{js}`");
    }

    /// A publish waits on signing, which with a signer app is a person
    /// approving it: the vendored `outbox.publish` gives up after 30 s, so the
    /// supplement replaces it, and `mesh.publish` gets the same wait.
    #[test]
    fn publishes_wait_the_signing_timeout() {
        assert!(SUPPLEMENT_IIFE.contains(r#"domains.has("outbox") && napplet.outbox"#));
        assert!(SUPPLEMENT_IIFE.contains("napplet.outbox.publish = function publish("));
        assert!(SUPPLEMENT_IIFE.contains(r#"type: "outbox.publish""#));
        assert!(SUPPLEMENT_IIFE.contains(r#"msg.type !== "outbox.publish.result""#));
        // The same result fields the vendored resolver copies.
        for field in ["ok", "event", "eventId", "relays", "error"] {
            assert!(
                SUPPLEMENT_IIFE.contains(&format!("result.{field} = msg.{field}"))
                    || SUPPLEMENT_IIFE.contains(&format!("{{ {field}: msg.{field} }}")),
                "the outbox result drops `{field}`"
            );
        }
        assert!(SUPPLEMENT_IIFE.contains("}, SIGNING_TIMEOUT_MS);\n    }"));
        // Sent like the vendored one: a message the structured clone refuses
        // is retried as a snapshot, and a send that still fails settles the call.
        assert!(SUPPLEMENT_IIFE.contains("postCloneable(message);"));
        assert!(SUPPLEMENT_IIFE.contains(r#"e.name !== "DataCloneError""#));
        // The override only matters because the vendored one is short and is
        // installed first; if upstream changes either, revisit it.
        assert!(PRELUDE_IIFE.contains("outbox.publish timed out"));
        let vendored = PRELUDE_IIFE
            .find("function publish(template, options)")
            .unwrap();
        let body = &PRELUDE_IIFE[vendored..vendored + 400];
        assert!(
            body.contains("REQUEST_TIMEOUT_MS9"),
            "vendored outbox.publish changed"
        );
        assert!(
            body.contains(r#"type: "outbox.publish""#),
            "vendored outbox.publish envelope changed"
        );
    }

    /// NAP-UPLOAD: the vendored installer carries the namespace, and the
    /// supplement replaces `upload.upload` with one that waits as long as a
    /// publish — the authorization is signed as the user first. The vendored
    /// one gives up after 30 s.
    #[test]
    fn upload_waits_the_signing_timeout() {
        assert!(PRELUDE_IIFE.contains(r#"domains.has("upload")"#));
        assert!(PRELUDE_IIFE.contains("upload.upload timed out"));
        assert!(SUPPLEMENT_IIFE.contains(r#"domains.has("upload") && napplet.upload"#));
        assert!(SUPPLEMENT_IIFE.contains("napplet.upload.upload = function upload("));
        assert!(SUPPLEMENT_IIFE.contains(r#"{ type: "upload.upload", request: req }"#));
        assert!(SUPPLEMENT_IIFE.contains(r#"["upload.upload.result"]"#));
        assert!(SUPPLEMENT_IIFE.contains("resolve(msg.result)"));
        // And `upload` reaches both installers' allowlist.
        let s = Session::new(NappletIdentity::new("d", "a"), Vec::<String>::new());
        assert!(s.available_domains().contains(&"upload".to_string()));
    }

    /// NAP-LINK and NAP-THEME need nothing from the supplement: the vendored
    /// installer carries both namespaces and routes their results, gated on
    /// the same domain list.
    #[test]
    fn the_vendored_prelude_installs_link_and_theme() {
        for domain in ["link", "theme"] {
            assert!(
                PRELUDE_IIFE.contains(&format!(r#"domains.has("{domain}")"#)),
                "the vendored prelude no longer installs {domain}"
            );
        }
        for wire in [
            "link.open",
            "link.open.result",
            "theme.get",
            "theme.changed",
        ] {
            assert!(PRELUDE_IIFE.contains(&format!(r#""{wire}""#)), "{wire}");
        }
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

#[cfg(test)]
mod handshake_tests {
    use super::*;

    /// The bug this exists to prevent: with no `shell.ready` the session never
    /// establishes, and a napplet that started up perfectly well is refused
    /// every capability it asks for.
    ///
    /// The vendored shim carries no `shell` domain and never sends one, so the
    /// prelude does — which is what NAP-SHELL means by the runtime-provided
    /// shim emitting it at load.
    #[test]
    fn the_prelude_signals_readiness() {
        let out = render(&["shell".to_string()]);
        assert!(
            out.contains(r#"{ type: "shell.ready" }"#),
            "the prelude never signals readiness, so no session can establish"
        );
    }

    /// Order is the whole point. `shell.ready` means "my receiver is live", and
    /// `install` is what puts the receiver in place — sent first, the runtime
    /// would answer into a napplet that is not listening and the environment
    /// would be lost.
    #[test]
    fn readiness_is_signalled_after_the_namespace_is_installed() {
        let out = render(&["shell".to_string()]);
        let installed = out.rfind(&format!("{PRELUDE_GLOBAL}.install(")).unwrap();
        let ready = out.rfind("shell.ready").unwrap();
        assert!(
            installed < ready,
            "readiness was signalled before the receiver existed"
        );
    }

    /// The opaque origin has no origin string to match, so `'*'` is required
    /// rather than lax — see the NIP-5D web projection.
    #[test]
    fn readiness_is_posted_to_the_parent_with_a_wildcard_origin() {
        let out = render(&["shell".to_string()]);
        assert!(out.contains(r#"parent.postMessage({ type: "shell.ready" }, "*")"#));
    }
}
