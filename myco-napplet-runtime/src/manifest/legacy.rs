// LEGACY NIP-5D (path/aggregate): remove after 2026-12-31
//
//! The legacy NIP-5D manifest schema, kept only so napplets published before
//! the 2026-10-07 schema change still open. No new features land here.
//!
//! The legacy shape is a NIP-5A manifest at the napplet kinds: one
//! `["path", "/index.html", <sha256>]` tag, an aggregate
//! `["x", <aggregate>, "aggregate"]`, a `description` tag, and
//! `requires` / `archetype` tags. Its identity hash is the aggregate, which is
//! **not** the artifact hash even for a single file — the migration guide is
//! explicit that one is never renamed into the other. A legacy napplet
//! republished in the current schema is therefore a new identity.
//!
//! Selected by [`super::NappletManifest::from_event`] only when the event has
//! no bare `["x", <sha256>]` artifact tag and does have `path` tags. Deleting
//! this file, its `mod` line and that one branch removes the fallback.

use nostr::Event;
use nsite_deck::aggregate::{aggregate_tag_value, path_entries_of};

use super::{
    check_kind_and_d, is_slug, validate_convention, Archetype, ManifestFormat, NappletManifest,
};
use crate::error::{NappletError, NappletErrorCode, Result};

/// Whether `event` is shaped like a legacy manifest: `path` tags and no bare
/// artifact `x` tag. The caller has already looked for the bare `x`.
pub(super) fn has_path_tags(event: &Event) -> bool {
    event
        .tags
        .iter()
        .any(|t| t.as_slice().first().map(String::as_str) == Some("path"))
}

/// Parse and validate a legacy manifest. The rules are the pre-2026-10-07 ones,
/// unchanged: single file, strict `requires` / `archetype` / `config` shapes,
/// and an aggregate `x` that, when present, must match the `path` tags.
pub(super) fn parse(event: Event, kind: u16) -> Result<NappletManifest> {
    tracing::debug!(
        id = %event.id,
        "napplet manifest uses the legacy NIP-5D path/aggregate schema"
    );

    let paths = path_entries_of(&event);
    if paths.is_empty() {
        return Err(NappletError::invalid_manifest("manifest has no path tags"));
    }

    // "A napplet is a single self-contained /index.html" — NIP-5D, Manifest.
    // The runtime injects those bytes as `iframe.srcdoc` with an opaque
    // origin, so a multi-file napplet cannot run anywhere.
    if paths.len() > 1 {
        let listed: Vec<&str> = paths.iter().map(|e| e.path.as_str()).collect();
        return Err(NappletError::new(
            NappletErrorCode::MultiFile,
            format!(
                "a napplet is a single self-contained /index.html (NIP-5D), but this \
                 manifest lists {} files ({}) — its author published a bundle no srcdoc \
                 runtime can load",
                listed.len(),
                listed.join(", ")
            ),
        ));
    }

    const INDEX_PATHS: [&str; 3] = ["/index.html", "index.html", "/"];
    let index = paths
        .iter()
        .find(|e| INDEX_PATHS.contains(&e.path.as_str()))
        .ok_or_else(|| {
            NappletError::new(
                NappletErrorCode::MissingIndex,
                "manifest lists no /index.html",
            )
        })?
        .sha256
        .to_ascii_lowercase();

    let mut d_tag = None;
    let mut servers = Vec::new();
    let mut requires: Vec<String> = Vec::new();
    let mut archetypes = Vec::new();
    let mut config_schema = None;
    let mut title = None;
    let mut description = None;

    for tag in event.tags.iter() {
        let slice = tag.as_slice();
        match slice.first().map(String::as_str) {
            Some("d") => d_tag = slice.get(1).cloned(),
            Some("server") => {
                if let Some(v) = slice.get(1).filter(|v| !v.is_empty()) {
                    servers.push(v.clone());
                }
            }
            Some("requires") => {
                let domain = slice
                    .get(1)
                    .ok_or_else(|| NappletError::invalid_manifest("requires tag has no domain"))?;
                if !is_slug(domain) {
                    return Err(NappletError::invalid_manifest(format!(
                        "requires domain {domain:?} is not a lowercase slug"
                    )));
                }
                if !requires.iter().any(|d| d == domain) {
                    requires.push(domain.clone());
                }
            }
            Some("archetype") => archetypes.push(parse_archetype(slice)?),
            Some("config") => {
                let raw = slice
                    .get(1)
                    .ok_or_else(|| NappletError::invalid_manifest("config tag has no schema"))?;
                let schema: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
                    NappletError::invalid_manifest(format!("config schema is not JSON: {e}"))
                })?;
                if !schema.is_object() {
                    return Err(NappletError::invalid_manifest(
                        "config schema must be a JSON object",
                    ));
                }
                if config_schema.is_some() {
                    return Err(NappletError::invalid_manifest(
                        "manifest carries more than one config tag",
                    ));
                }
                config_schema = Some(schema);
            }
            Some("title") => title = slice.get(1).cloned(),
            Some("description") => description = slice.get(1).cloned(),
            _ => {}
        }
    }

    check_kind_and_d(kind, d_tag.as_deref())?;

    // Recompute the aggregate from the path tags; an `x` tag, when present,
    // is corroboration and must agree.
    let computed = nsite_deck::aggregate::compute_aggregate_hash(&paths);
    if let Some(declared) = aggregate_tag_value(event.tags.iter().map(|t| t.as_slice())) {
        if !computed.eq_ignore_ascii_case(&declared) {
            return Err(NappletError::new(
                NappletErrorCode::AggregateMismatch,
                format!("recomputed aggregate {computed} != manifest {declared}"),
            ));
        }
    }

    let author = event.pubkey;
    Ok(NappletManifest {
        event,
        author,
        kind,
        format: ManifestFormat::Legacy,
        d_tag,
        artifact: index,
        identity_hash: computed,
        servers,
        requires,
        optional: Vec::new(),
        roles: archetypes.iter().map(|a: &Archetype| a.slug.clone()).fold(
            Vec::new(),
            |mut out, slug| {
                if !out.contains(&slug) {
                    out.push(slug);
                }
                out
            },
        ),
        intents: Vec::new(),
        archetypes,
        config_schema,
        title,
        description,
        icon: None,
        app: None,
        source: None,
    })
}

fn parse_archetype(slice: &[String]) -> Result<Archetype> {
    if slice.len() != 3 {
        return Err(NappletError::invalid_manifest(
            "archetype tags carry exactly a slug and a convention",
        ));
    }
    let slug = &slice[1];
    if !is_slug(slug) {
        return Err(NappletError::invalid_manifest(format!(
            "archetype slug {slug:?} is not a lowercase slug"
        )));
    }
    let convention = &slice[2];
    validate_convention(convention)?;
    Ok(Archetype {
        slug: slug.clone(),
        convention: convention.clone(),
    })
}
