//! The current NIP-5D manifest: `x`, `content`, `R` / `O`, `z` / `i`, `icon`,
//! `app` — and what is rejected (only a missing or malformed `x`, a named
//! manifest without `d`, an empty description) versus merely ignored.

use myco_napplet_runtime::error::NappletErrorCode;
use myco_napplet_runtime::manifest::{
    icon_bytes_match, Archetype, IntentAdvert, ManifestFormat, ManifestIcon, NappletManifest,
};
use myco_napplet_runtime::testing::NappletBuilder;

fn parse(builder: NappletBuilder) -> myco_napplet_runtime::Result<NappletManifest> {
    NappletManifest::from_event(builder.build().manifest)
}

fn browser_side_quest() -> nostr::Event {
    let json = include_str!("fixtures/browser-side-quest.json");
    serde_json::from_str(json.trim()).expect("fixture event")
}

/// A real napplet.soy manifest, signed and published 2026-10: the shape every
/// current-format napplet has, parsed as it is on the wire.
#[test]
fn parses_a_published_current_manifest() {
    let event = browser_side_quest();
    event
        .verify()
        .expect("the fixture is a genuine signed event");
    let m = NappletManifest::from_event(event).unwrap();

    assert_eq!(m.format, ManifestFormat::Current);
    assert_eq!(m.d_tag.as_deref(), Some("n-8e5abf9b526"));
    assert_eq!(
        m.artifact,
        "5491f3768667445a6fa4d7bf3e6d1597e6ad599c73532b49691ee334ff19b02b"
    );
    // Identity is the artifact hash itself — no aggregate is computed.
    assert_eq!(
        m.identity(),
        (
            "n-8e5abf9b526",
            "5491f3768667445a6fa4d7bf3e6d1597e6ad599c73532b49691ee334ff19b02b"
        )
    );
    assert_eq!(m.title.as_deref(), Some("Browser Side Quest"));
    assert!(m
        .description
        .as_deref()
        .unwrap()
        .starts_with("Browser Side Quest revives Mozilla's 2012"));
    assert!(m.requires.is_empty());
    assert_eq!(
        m.optional,
        vec!["identity", "storage", "common", "resource", "webrtc", "config"]
    );
    assert_eq!(m.roles, vec!["game", "rpg", "multiplayer"]);
    // `z` without `i` advertises a role but accepts no intent.
    assert!(m.intents.is_empty());
    assert!(m.archetypes.is_empty());
    assert_eq!(
        m.icon,
        Some(ManifestIcon {
            sha256: "ad7421b295bffb9580008e987f96466b6659f17603d4b07e5cb346127b5373a8".into(),
            mime: "image/png".into(),
        })
    );
    assert_eq!(m.servers, vec!["https://blossom.napplet.soy"]);
    let app = m.app.unwrap();
    assert!(app.address.starts_with("32267:266815e0"));
    assert_eq!(app.relay.as_deref(), Some("wss://relay.napplet.soy/"));
    assert!(m.source.unwrap().starts_with("nostr://npub1"));
}

#[test]
fn parses_r_o_z_i_and_config() {
    let manifest = parse(
        NappletBuilder::new()
            .requires(&["shell", "relay", "identity"])
            .optional(&["theme"])
            .archetype("profile", "napplet:profile/open")
            .archetype("note", "napplet:note/open")
            .config(r#"{"type":"object","properties":{"relay":{"type":"string"}}}"#)
            .server("https://blossom.example"),
    )
    .unwrap();

    assert_eq!(manifest.requires, vec!["shell", "relay", "identity"]);
    assert_eq!(manifest.optional, vec!["theme"]);
    assert_eq!(
        manifest.declared_domains(),
        vec!["shell", "relay", "identity", "theme"]
    );
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

/// NIP-5D: a domain declared both required and optional is optional.
#[test]
fn a_domain_in_r_and_o_is_optional() {
    let m = parse(
        NappletBuilder::new()
            .requires(&["relay", "theme"])
            .optional(&["theme"]),
    )
    .unwrap();
    assert_eq!(m.requires, vec!["relay"]);
    assert_eq!(m.optional, vec!["theme"]);
}

/// The trap, pinned: decorating a manifest must not move its identity.
#[test]
fn the_extra_tags_do_not_move_the_identity() {
    let bare = parse(NappletBuilder::new().requires(&[])).unwrap();
    let decorated = parse(
        NappletBuilder::new()
            .requires(&["relay", "storage", "intent"])
            .archetype("chat", "napplet:chat/open")
            .icon(&"ab".repeat(32), "image/png")
            .config(r#"{"type":"object"}"#)
            .server("https://blossom.example"),
    )
    .unwrap();

    assert_eq!(bare.identity_hash, decorated.identity_hash);
}

/// Rule 6: unknown tags are ignored, never a reason to reject.
#[test]
fn unknown_tags_are_ignored() {
    let m = parse(
        NappletBuilder::new()
            .tag(&["t", "game"])
            .tag(&["soy-source-repository", "30617:x:y", "linked"])
            .tag(&["future-tag"]),
    )
    .unwrap();
    assert_eq!(m.format, ManifestFormat::Current);
}

/// NAP-INTENT: an `i` is eligible only when its archetype segment matches a
/// `z` on the same manifest, regardless of tag order. Parameter names are
/// kept as advertised.
#[test]
fn intents_pair_with_roles_by_archetype_not_by_position() {
    let m = parse(
        NappletBuilder::new()
            .tag(&["i", "napplet:feed/edit", "filters", "relays"])
            .tag(&["i", "napplet:note/open"])
            .tag(&["z", "feed"]),
    )
    .unwrap();
    assert_eq!(
        m.intents,
        vec![
            IntentAdvert {
                convention: "napplet:feed/edit".into(),
                params: vec!["filters".into(), "relays".into()],
            },
            IntentAdvert {
                convention: "napplet:note/open".into(),
                params: vec![],
            },
        ]
    );
    // `note` has no `z`, so `napplet:note/open` is not dispatchable.
    assert_eq!(
        m.archetypes,
        vec![Archetype {
            slug: "feed".into(),
            convention: "napplet:feed/edit".into()
        }]
    );
}

/// Unusable advertisements contribute nothing; they do not invalidate the
/// manifest (NAP-INTENT).
#[test]
fn malformed_advertisements_are_dropped_not_fatal() {
    let mut builder = NappletBuilder::new()
        .requires(&["Relay", "relay"])
        .tag(&["O", "nap theme"])
        .tag(&["z", "Feed"])
        .tag(&["z", "feed"]);
    for bad in [
        "NAP-INTENT",
        "profile/open",
        "napplet:feed",
        "napplet:feed/open/extra",
        "napplet:feed/open?pubkey=abc",
        "napplet:feed/open#frag",
    ] {
        builder = builder.tag(&["i", bad]);
    }
    let m = parse(builder.tag(&["i", "napplet:feed/open"])).unwrap();
    assert_eq!(m.requires, vec!["relay"]);
    assert!(m.optional.is_empty());
    assert_eq!(m.roles, vec!["feed"]);
    assert_eq!(m.intents.len(), 1);
    assert_eq!(m.archetypes.len(), 1);
}

/// NIP-5D §Icon: an invalid icon falls back to generic artwork and never
/// prevents loading.
#[test]
fn a_bad_icon_is_tolerated() {
    let good = "ab".repeat(32);
    for (sha, mime) in [
        ("not-a-hash", "image/png"),
        (good.as_str(), "image/svg+xml"),
        (good.as_str(), ""),
    ] {
        let m = parse(NappletBuilder::new().icon(sha, mime)).unwrap();
        assert_eq!(m.icon, None, "kept icon {sha:?} {mime:?}");
    }
    let m =
        parse(
            NappletBuilder::new()
                .icon(&good, "image/webp")
                .tag(&["icon", &good, "image/png"]),
        )
        .unwrap();
    assert_eq!(m.icon, None, "two icon tags are ambiguous");
}

#[test]
fn icon_bytes_are_sniffed_against_the_declared_type() {
    assert!(icon_bytes_match("image/png", b"\x89PNG\r\n\x1a\n...."));
    assert!(icon_bytes_match("image/jpeg", &[0xFF, 0xD8, 0xFF, 0xE0]));
    assert!(icon_bytes_match("image/webp", b"RIFF\0\0\0\0WEBPVP8 "));
    assert!(!icon_bytes_match("image/png", &[0xFF, 0xD8, 0xFF, 0xE0]));
    assert!(!icon_bytes_match("image/svg+xml", b"<svg/>"));
}

#[test]
fn rejects_an_empty_description() {
    for text in ["", "   \n"] {
        let err = parse(NappletBuilder::new().description(text)).unwrap_err();
        assert_eq!(err.code, NappletErrorCode::InvalidManifest);
    }
}

#[test]
fn rejects_a_malformed_or_repeated_x() {
    let err = parse(NappletBuilder::new().tag(&["x", &"cd".repeat(32)])).unwrap_err();
    assert_eq!(err.code, NappletErrorCode::InvalidManifest);

    let event = NappletBuilder::new()
        .aggregate(myco_napplet_runtime::testing::FixtureAggregate::Omitted)
        .tag(&["x", "not-a-sha256"])
        .build()
        .manifest;
    let err = NappletManifest::from_event(event).unwrap_err();
    assert_eq!(err.code, NappletErrorCode::InvalidManifest);
}

#[test]
fn rejects_a_named_manifest_without_d() {
    let err = parse(NappletBuilder::new().d_tag(None)).unwrap_err();
    assert_eq!(err.code, NappletErrorCode::InvalidManifest);
}

/// A bad `config` is a Myco extension gone wrong, not a reason to refuse the
/// napplet: it is dropped.
#[test]
fn a_bad_config_schema_is_dropped() {
    for schema in ["not json at all", "[1,2,3]", "\"a string\"", "42"] {
        let m = parse(NappletBuilder::new().config(schema)).unwrap();
        assert!(m.config_schema.is_none(), "kept {schema:?}");
    }
}
