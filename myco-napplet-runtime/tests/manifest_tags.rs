//! The three tags NIP-5D adds to NIP-5A: `requires`, `archetype`, `config`.
//!
//! The load-bearing property, and the one easiest to break by accident: none of
//! them feed the aggregate. Only `path` tags do. A runtime that hashed them
//! would reject every conformant napplet in existence.

use myco_napplet_runtime::error::NappletErrorCode;
use myco_napplet_runtime::manifest::{Archetype, NappletManifest};
use myco_napplet_runtime::testing::NappletBuilder;

fn parse(builder: NappletBuilder) -> myco_napplet_runtime::Result<NappletManifest> {
    NappletManifest::from_event(builder.build().manifest)
}

#[test]
fn parses_requires_archetype_and_config() {
    let manifest = parse(
        NappletBuilder::new()
            .requires(&["shell", "relay", "identity"])
            .archetype("profile", "napplet:profile/open")
            .archetype("note", "napplet:note/open")
            .config(r#"{"type":"object","properties":{"relay":{"type":"string"}}}"#)
            .server("https://blossom.example"),
    )
    .unwrap();

    assert_eq!(manifest.requires, vec!["shell", "relay", "identity"]);
    assert_eq!(
        manifest.archetypes,
        vec![
            Archetype {
                slug: "profile".into(),
                convention: "napplet:profile/open".into()
            },
            Archetype {
                slug: "note".into(),
                convention: "napplet:note/open".into()
            },
        ]
    );
    assert_eq!(manifest.config_schema.unwrap()["type"], "object");
    assert_eq!(manifest.servers, vec!["https://blossom.example"]);
}

/// The trap, pinned: adding `requires` / `archetype` / `config` must not move
/// the aggregate. If it did, a napplet declaring a capability would fail to
/// load while the same napplet declaring none succeeded.
#[test]
fn the_extra_tags_do_not_move_the_aggregate() {
    let bare = parse(NappletBuilder::new().requires(&[])).unwrap();
    let decorated = parse(
        NappletBuilder::new()
            .requires(&["relay", "storage", "intent"])
            .archetype("chat", "napplet:chat/open")
            .config(r#"{"type":"object"}"#)
            .server("https://blossom.example"),
    )
    .unwrap();

    assert_eq!(bare.aggregate, decorated.aggregate);
}

#[test]
fn rejects_a_requires_domain_that_is_not_a_slug() {
    for domain in ["Relay", "nap relay", "relay!", ""] {
        let err = parse(NappletBuilder::new().requires(&[domain])).unwrap_err();
        assert_eq!(
            err.code,
            NappletErrorCode::InvalidManifest,
            "accepted {domain:?}"
        );
    }
}

#[test]
fn deduplicates_repeated_requires() {
    let manifest = parse(NappletBuilder::new().requires(&["relay", "relay", "shell"])).unwrap();
    assert_eq!(manifest.requires, vec!["relay", "shell"]);
}

/// Routing is exact equality over a queryless `napplet:<archetype>/<intent>`
/// identity, so anything that would make two identities route as one — a query,
/// a fragment, an extra segment — is refused at parse.
#[test]
fn rejects_malformed_archetype_conventions() {
    let bad = [
        "NAP-INTENT",                      // a spec id, not a convention
        "profile/open",                    // no scheme
        "napplet:profile",                 // no intent
        "napplet:profile/open/extra",      // too many segments
        "napplet:profile/open?pubkey=abc", // carries a query
        "napplet:profile/open#frag",       // carries a fragment
        "napplet:/open",                   // empty archetype
        "napplet:profile/",                // empty intent
    ];
    for convention in bad {
        let err = parse(NappletBuilder::new().archetype("profile", convention)).unwrap_err();
        assert_eq!(
            err.code,
            NappletErrorCode::InvalidManifest,
            "accepted {convention:?}"
        );
    }
}

#[test]
fn rejects_a_config_schema_that_is_not_a_json_object() {
    for schema in ["not json at all", "[1,2,3]", "\"a string\"", "42"] {
        let err = parse(NappletBuilder::new().config(schema)).unwrap_err();
        assert_eq!(
            err.code,
            NappletErrorCode::InvalidManifest,
            "accepted {schema:?}"
        );
    }
}
