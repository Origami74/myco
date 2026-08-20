//! The whole off-device path in one place: a signed fixture goes in, a load
//! command and a completed handshake come out.
//!
//! Each stage has its own tests. This one exists because the stages have to
//! agree with each other — the identity the session is keyed by, the domains
//! the prelude installs, and the domains `shell.init` advertises all have to be
//! the same values, and nothing but an assembled run proves it.

use myco_napplet_runtime::artifact::{assemble, Injection, SrcdocArtifact};
use myco_napplet_runtime::dispatch::dispatch;
use myco_napplet_runtime::prelude::render_for;
use myco_napplet_runtime::resolve::resolve;
use myco_napplet_runtime::seams::Envelope;
use myco_napplet_runtime::session::{NappletIdentity, Session};
use myco_napplet_runtime::shell_link::{ShellAction, ToRuntime, ToShell};
use myco_napplet_runtime::testing::{build_test_napplet, FIXTURE_INDEX_HTML};
use nsite_deck::seams::BlobStore;
use nsite_deck::testing::MemBlobs;
use serde_json::json;

#[tokio::test]
async fn a_verified_napplet_reaches_the_shell_and_completes_the_handshake() {
    // --- the napplet arrives -------------------------------------------
    let napplet = build_test_napplet();
    let blobs = MemBlobs::new();
    for (_, bytes) in &napplet.blobs {
        blobs.put(bytes).await.unwrap();
    }

    // --- verify --------------------------------------------------------
    let resolved = resolve(napplet.manifest, &blobs).await.unwrap();
    assert_eq!(resolved.index_html, FIXTURE_INDEX_HTML);

    // --- the session, keyed by the identity computed from those bytes ---
    let identity = NappletIdentity::from(&resolved);
    assert_eq!(identity.d_tag, "fixture");
    assert_eq!(identity.aggregate, resolved.aggregate);
    let mut session = Session::new(identity.clone(), ["shell"]);

    // --- assemble ------------------------------------------------------
    let prelude = render_for(&session);
    let artifact = assemble(
        &resolved.index_html,
        &Injection {
            prelude_js: Some(&prelude),
            ..Default::default()
        },
    );

    // The napplet's own bytes survive intact; the injected parts precede them.
    let doc = artifact.as_str();
    assert!(doc.contains("Fixture Napplet"));
    let csp = doc.find("Content-Security-Policy").unwrap();
    let installed = doc.rfind("NappletShimPrelude.install(").unwrap();
    let napplet_code = doc.find("Fixture Napplet").unwrap();
    assert!(csp < installed && installed < napplet_code);

    // --- the shell mounts, and is handed the bytes ----------------------
    let mounted: ToRuntime =
        serde_json::from_value(json!({"channel": "shell", "action": "mounted"})).unwrap();
    assert_eq!(
        mounted,
        ToRuntime::Shell {
            action: ShellAction::Mounted
        }
    );

    let ToShell::Shell {
        action,
        artifact: bytes,
        sandbox,
    } = ToShell::load(&artifact)
    else {
        panic!("expected a load command");
    };
    assert_eq!(action, "load");
    assert_eq!(sandbox, SrcdocArtifact::SANDBOX);
    assert_eq!(bytes, doc);

    // --- the napplet says it is ready -----------------------------------
    let relayed: ToRuntime =
        serde_json::from_value(json!({"channel": "napplet", "message": {"type": "shell.ready"}}))
            .unwrap();
    let ToRuntime::Napplet { message } = relayed else {
        panic!("expected a relayed napplet message");
    };

    let replies = dispatch(&mut session, &message).envelopes().to_vec();
    assert_eq!(replies.len(), 1);
    assert_eq!(
        serde_json::to_value(&replies[0]).unwrap(),
        json!({
            "type": "shell.init",
            "capabilities": {"domains": ["shell"]},
            "services": []
        })
    );
    assert!(session.is_established());

    // --- the namespace and the environment agree ------------------------
    // The domains the prelude installed are the domains shell.init advertised.
    // Two different code paths, one value; if they ever diverge a napplet's
    // supports() check and its actual namespace disagree.
    let advertised = replies[0].field("capabilities").unwrap()["domains"].clone();
    assert_eq!(advertised, json!(session.available_domains()));
    assert!(prelude.contains(r#"{"domains":["shell"]}"#));

    // --- and the session never re-establishes ---------------------------
    assert!(dispatch(&mut session, &message).envelopes().is_empty());
    assert_eq!(session.identity(), &identity);
}

/// A napplet that fails verification never reaches assembly. There is no
/// artifact, no load command, and no session — the pipeline stops at resolve.
#[tokio::test]
async fn a_tampered_napplet_never_becomes_an_artifact() {
    use myco_napplet_runtime::testing::NappletBuilder;

    let napplet = NappletBuilder::new().break_signature().build();
    let blobs = MemBlobs::new();
    for (_, bytes) in &napplet.blobs {
        blobs.put(bytes).await.unwrap();
    }

    assert!(
        resolve(napplet.manifest, &blobs).await.is_err(),
        "a tampered napplet must not resolve, so nothing downstream can run"
    );
}

/// The permission is behind the call, not in the namespace.
///
/// A napplet granted nothing still gets every implemented API injected and
/// still sees `supports()` say yes — and its call is still refused. That is the
/// shape that lets a napplet ask rather than give up: an absent API reads as
/// "this runtime will never do relay", a refused call reads as "not right now".
#[tokio::test]
async fn the_namespace_is_not_the_enforcement() {
    let mut session = Session::with_implemented(
        NappletIdentity::new("chat", "aggregate"),
        // Granted nothing beyond the mandatory shell domain...
        Vec::<String>::new(),
        ["shell", "relay"],
    );
    dispatch(&mut session, &Envelope::new("shell.ready"));

    // ...and relay is installed anyway.
    let prelude = render_for(&session);
    let install = prelude.rfind("NappletShimPrelude.install(").unwrap();
    assert_eq!(
        prelude[install..].trim_end(),
        r#"NappletShimPrelude.install({"domains":["relay","shell"]});"#
    );

    // The call is what gets refused.
    let call = Envelope::new("relay.publish").with_id("x1");
    let replies = dispatch(&mut session, &call).envelopes().to_vec();
    assert_eq!(replies.len(), 1);
    assert!(replies[0].field("error").is_some());
}
