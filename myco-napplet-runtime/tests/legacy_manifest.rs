// LEGACY NIP-5D (path/aggregate): remove after 2026-12-31
//
//! The legacy path/aggregate schema still parses and resolves through its
//! fallback, with its old rules unchanged. Delete with `src/manifest/legacy.rs`.

use myco_napplet_runtime::error::NappletErrorCode;
use myco_napplet_runtime::manifest::{ManifestFormat, NappletManifest};
use myco_napplet_runtime::resolve::resolve;
use myco_napplet_runtime::testing::{
    FixtureAggregate, NappletBuilder, TestNapplet, FIXTURE_INDEX_HTML,
};
use nsite_deck::aggregate::{compute_aggregate_hash, PathEntry};
use nsite_deck::seams::BlobStore;
use nsite_deck::sync::sha256_hex;
use nsite_deck::testing::MemBlobs;

fn legacy() -> NappletBuilder {
    NappletBuilder::new().legacy()
}

fn parse(builder: NappletBuilder) -> myco_napplet_runtime::Result<NappletManifest> {
    NappletManifest::from_event(builder.build().manifest)
}

async fn store_for(napplet: &TestNapplet) -> MemBlobs {
    let blobs = MemBlobs::new();
    for (_, bytes) in &napplet.blobs {
        blobs.put(bytes).await.unwrap();
    }
    blobs
}

async fn expect_rejection(napplet: TestNapplet, code: NappletErrorCode) {
    let blobs = store_for(&napplet).await;
    let err = resolve(napplet.manifest, &blobs)
        .await
        .expect_err("this napplet must not resolve");
    assert_eq!(err.code, code, "wrong guard rejected: {err}");
}

/// A legacy napplet still opens, and its identity stays the aggregate — the
/// one it had before the schema change, so nothing keyed on it moves.
#[tokio::test]
async fn a_legacy_manifest_resolves_with_the_aggregate_as_identity() {
    let napplet = legacy().requires(&["relay"]).build();
    let blobs = store_for(&napplet).await;
    let resolved = resolve(napplet.manifest, &blobs).await.unwrap();

    let aggregate = compute_aggregate_hash(&[PathEntry {
        path: "/index.html".into(),
        sha256: sha256_hex(FIXTURE_INDEX_HTML.as_bytes()),
    }]);
    assert_eq!(resolved.manifest.format, ManifestFormat::Legacy);
    assert_eq!(resolved.identity(), ("fixture", aggregate.as_str()));
    assert_eq!(resolved.index_html, FIXTURE_INDEX_HTML);
    assert_eq!(resolved.manifest.requires, vec!["relay"]);
    assert_eq!(
        resolved.manifest.description.as_deref(),
        Some("A fixture napplet for tests.")
    );
}

/// "Never rename the aggregate hash to the artifact hash: they differ even
/// for a single file." The same bytes published both ways are two identities.
#[test]
fn the_same_bytes_are_a_different_identity_in_each_schema() {
    let old = parse(legacy()).unwrap();
    let new = parse(NappletBuilder::new()).unwrap();
    assert_eq!(old.artifact, new.artifact);
    assert_ne!(old.identity_hash, new.identity_hash);
}

/// Pinned to the hex the reference JS runtime computes for the fixture. If it
/// ever fails, every legacy napplet stops loading — do not update the constant.
#[test]
fn the_fixture_aggregate_matches_the_reference_implementation() {
    let manifest = parse(legacy()).unwrap();
    assert_eq!(
        manifest.identity_hash,
        "ae61a6e95ad666d2294f88e69769961148aa933ca1fdf59a7f31cf0d2c97c1cb"
    );
    assert_eq!(
        compute_aggregate_hash(&[
            PathEntry {
                path: "/index.html".into(),
                sha256: sha256_hex(FIXTURE_INDEX_HTML.as_bytes()),
            },
            PathEntry {
                path: "/app.js".into(),
                sha256: sha256_hex(b"console.log('hi')"),
            },
        ]),
        "d33c30b3f7ad101e27f2ed4fae460edc51139e41454d66d8c08760d0f561f168"
    );
}

#[tokio::test]
async fn rejects_an_aggregate_mismatch() {
    expect_rejection(
        legacy().aggregate(FixtureAggregate::Corrupt).build(),
        NappletErrorCode::AggregateMismatch,
    )
    .await;
}

/// The legacy `x` is corroboration: without it the aggregate is recomputed.
#[tokio::test]
async fn a_missing_aggregate_tag_still_resolves_to_the_same_identity() {
    let with_tag = parse(legacy()).unwrap();
    let undeclared = legacy().aggregate(FixtureAggregate::Omitted).build();
    let blobs = store_for(&undeclared).await;
    let resolved = resolve(undeclared.manifest, &blobs).await.unwrap();
    assert_eq!(resolved.identity_hash, with_tag.identity_hash);
}

#[tokio::test]
async fn rejects_a_multi_file_bundle() {
    let napplet = legacy().file("/app.js", b"console.log('hi')").build();
    let blobs = store_for(&napplet).await;
    let err = resolve(napplet.manifest.clone(), &blobs).await.unwrap_err();
    assert_eq!(err.code, NappletErrorCode::MultiFile);
    assert!(err.message.contains("/app.js"), "unhelpful error: {err}");
    assert!(
        err.message.contains("single self-contained"),
        "unhelpful error: {err}"
    );
    assert!(!err.message.to_lowercase().contains("rebuild"));
}

#[tokio::test]
async fn rejects_a_manifest_with_no_index() {
    expect_rejection(
        legacy()
            .files(&[("/main.html", b"<h1>wrong name</h1>")])
            .build(),
        NappletErrorCode::MissingIndex,
    )
    .await;
}

/// The fallback is chosen only when there is no artifact `x`: a manifest
/// carrying both an artifact `x` and `path` tags is current.
#[test]
fn an_artifact_x_wins_over_path_tags() {
    let m = parse(NappletBuilder::new().tag(&["path", "/index.html", &"ab".repeat(32)])).unwrap();
    assert_eq!(m.format, ManifestFormat::Current);
}

// The legacy rules are kept as they were: strict.

#[test]
fn rejects_a_requires_domain_that_is_not_a_slug() {
    for domain in ["Relay", "nap relay", "relay!", ""] {
        let err = parse(legacy().requires(&[domain])).unwrap_err();
        assert_eq!(err.code, NappletErrorCode::InvalidManifest, "{domain:?}");
    }
}

#[test]
fn rejects_malformed_archetype_conventions() {
    for convention in [
        "NAP-INTENT",
        "profile/open",
        "napplet:profile",
        "napplet:profile/open/extra",
        "napplet:profile/open?pubkey=abc",
        "napplet:profile/open#frag",
        "napplet:/open",
        "napplet:profile/",
    ] {
        let err = parse(legacy().archetype("profile", convention)).unwrap_err();
        assert_eq!(
            err.code,
            NappletErrorCode::InvalidManifest,
            "{convention:?}"
        );
    }
}

#[test]
fn rejects_a_config_schema_that_is_not_a_json_object() {
    for schema in ["not json at all", "[1,2,3]", "\"a string\"", "42"] {
        let err = parse(legacy().config(schema)).unwrap_err();
        assert_eq!(err.code, NappletErrorCode::InvalidManifest, "{schema:?}");
    }
}

/// Older tooling wrote the aggregate as a bare `["x", <aggregate>]`, with no
/// marker. The `path` tags make it legacy; the bare `x` is not mistaken for
/// an artifact hash.
#[tokio::test]
async fn a_legacy_manifest_with_an_unmarked_aggregate_stays_legacy() {
    let aggregate = compute_aggregate_hash(&[PathEntry {
        path: "/index.html".into(),
        sha256: sha256_hex(FIXTURE_INDEX_HTML.as_bytes()),
    }]);
    let napplet = legacy()
        .aggregate(FixtureAggregate::Omitted)
        .tag(&["x", &aggregate])
        .build();
    let blobs = store_for(&napplet).await;
    let resolved = resolve(napplet.manifest, &blobs).await.unwrap();
    assert_eq!(resolved.manifest.format, ManifestFormat::Legacy);
    assert_eq!(resolved.identity_hash, aggregate);
    assert_eq!(resolved.index_html, FIXTURE_INDEX_HTML);
}
