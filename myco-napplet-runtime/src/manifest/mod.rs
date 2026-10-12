//! NIP-5D napplet manifests: the three kinds, the tags they carry, and the
//! validation that happens before anything is fetched.
//!
//! A napplet is a single self-contained `/index.html`. Its manifest names that
//! one artifact by hash and describes it:
//!
//! | Tag | Meaning |
//! |-----|---------|
//! | `["x", "<sha256>"]` | the artifact — exactly one; with `d`, the identity |
//! | `["d", "<id>"]` | the napplet identifier, on `35129` only |
//! | `["R", "<domain>"]` / `["O", "<domain>"]` | NAP domains needed / optionally used |
//! | `["z", "<role>"]` | an archetype the napplet advertises |
//! | `["i", "<convention>", "<param>"…]` | an intent it accepts, with parameter names |
//! | `["icon", "<sha256>", "<mime>"]` | optional icon blob |
//! | `["server", "<url>"]` | Blossom hints for the artifact and the icon |
//! | `["title", "…"]`, `["source", "…"]`, `["app", "32267:…", "<relay>"]` | presentation |
//!
//! and the event `content` is the napplet's plain-text description, which must
//! not be empty.
//!
//! A manifest is rejected only for what makes it unrunnable or unaddressable:
//! a missing or malformed `x`, a named manifest without `d`, or an empty
//! description. Everything else that is malformed — an unknown domain shape, an
//! unusable `z` / `i`, a bad icon — is dropped, not fatal, and unknown tags are
//! ignored. `R` / `O` / `z` / `i` are advertisements from an untrusted
//! artifact; none of them grants anything.
//!
//! Manifests published before 2026-10-07 use the legacy path/aggregate schema.
//! Those parse through [`legacy`], a fallback kept in one file so it can be
//! deleted in one go.
//!
//! Pinned against the living NIP-5D (`dskvr/nips` branch `nip/5d`, through
//! `nostr-protocol/nips` PR #2303), §Manifest and §Identity, read 2026-10-12.
//! Re-audit deliberately on change (design §8).

mod legacy;

use nostr::{Event, PublicKey};

use crate::error::{NappletError, Result};

/// Snapshot manifest — a regular event, an immutable point-in-time release.
pub const KIND_SNAPSHOT: u16 = 5129;
/// Root manifest — replaceable, an author's latest unnamed napplet.
pub const KIND_ROOT: u16 = 15129;
/// Named manifest — addressable, carries a `d` tag identifier.
pub const KIND_NAMED: u16 = 35129;

/// All three NIP-5D manifest kinds.
///
/// Not 35128 — that is Myco's *nsite* kind, under NIP-5A.
pub const KINDS: [u16; 3] = [KIND_SNAPSHOT, KIND_ROOT, KIND_NAMED];

/// The icon formats NIP-5D §Icon allows. SVG is deliberately not one.
pub const ICON_MIME_TYPES: [&str; 3] = ["image/png", "image/jpeg", "image/webp"];

/// Whether `kind` is one of the three NIP-5D manifest kinds.
pub fn is_napplet_kind(kind: u16) -> bool {
    KINDS.contains(&kind)
}

/// Which schema a manifest was published in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestFormat {
    /// The artifact-hash schema (`x` / `R` / `O` / `z` / `i`, description in
    /// `content`). The primary path.
    Current,
    /// The pre-2026-10-07 path/aggregate schema. See [`legacy`].
    Legacy,
}

/// A role this napplet can be invoked as: a `z` role and an `i` convention
/// whose archetype segment names it. The dispatch contract NAP-INTENT routes
/// on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archetype {
    /// The routing slug (`profile`, `chat`).
    pub slug: String,
    /// The queryless payload identity, `napplet:<archetype>/<intent>`.
    pub convention: String,
}

/// One `["i", <convention>, <param>…]` advertisement, as published. Eligible
/// for dispatch only when [`NappletManifest::roles`] names its archetype — see
/// [`NappletManifest::archetypes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentAdvert {
    pub convention: String,
    /// Advertised parameter names, in tag order. Names only — never types or
    /// kind constraints (NAP-INTENT §Manifest Catalog Contract).
    pub params: Vec<String>,
}

/// The manifest's `["icon", <sha256>, <mime>]`, when well-formed and of a
/// supported type. A display hint only: it is not part of the identity, and a
/// missing or unusable icon never stops a napplet from loading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestIcon {
    /// Lowercase-hex sha256 of the icon blob.
    pub sha256: String,
    /// One of [`ICON_MIME_TYPES`].
    pub mime: String,
}

/// The proposed `["app", "32267:<pk>:<id>", "<relay>"]` pointer to an app
/// listing. Presentation only; nothing here dereferences it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppListing {
    pub address: String,
    pub relay: Option<String>,
}

/// A parsed NIP-5D napplet manifest.
#[derive(Debug, Clone)]
pub struct NappletManifest {
    pub event: Event,
    pub author: PublicKey,
    pub kind: u16,
    pub format: ManifestFormat,
    /// The `d` identifier for a named napplet; `None` for root and snapshot.
    pub d_tag: Option<String>,
    /// Lowercase-hex sha256 of the single `/index.html` artifact — what is
    /// fetched, and what the fetched bytes must hash to.
    pub artifact: String,
    /// The hash half of the napplet's `(dTag, hash)` identity: the artifact
    /// hash for a current manifest, the recomputed aggregate for a legacy one.
    /// The two differ even over the same bytes, so a napplet republished in
    /// the current schema is a new identity.
    pub identity_hash: String,
    /// `["server", <blossom-url>]` hints, for the artifact and the icon.
    pub servers: Vec<String>,
    /// `R` domains: what the napplet needs for full functionality. Not
    /// validated against the registry — what is *implemented* and what is
    /// *granted* are decided above this layer.
    pub requires: Vec<String>,
    /// `O` domains: integrations it can do without. A domain declared both
    /// ways is optional.
    pub optional: Vec<String>,
    /// `z` roles, as advertised.
    pub roles: Vec<String>,
    /// `i` advertisements, as published.
    pub intents: Vec<IntentAdvert>,
    /// The eligible dispatch contracts: each `i` whose archetype segment
    /// matches a `z` on this manifest, paired with that role.
    pub archetypes: Vec<Archetype>,
    /// The `["config", "<json-schema>"]` schema, parsed as JSON.
    pub config_schema: Option<serde_json::Value>,
    pub title: Option<String>,
    /// Plain text — render with text APIs only, never as HTML or Markdown.
    pub description: Option<String>,
    pub icon: Option<ManifestIcon>,
    pub app: Option<AppListing>,
    pub source: Option<String>,
}

impl NappletManifest {
    /// Parse and validate a NIP-5D manifest event.
    ///
    /// Does **not** verify the event's signature — [`crate::resolve`] does that
    /// first. The current schema is the primary path; the legacy one is tried
    /// only for an event with no artifact `x` tag and with `path` tags.
    pub fn from_event(event: Event) -> Result<Self> {
        let kind = event.kind.as_u16();
        if !is_napplet_kind(kind) {
            return Err(NappletError::invalid_manifest(format!(
                "event kind {kind} is not a NIP-5D napplet manifest (5129/15129/35129)"
            )));
        }

        let artifacts: Vec<&String> = event
            .tags
            .iter()
            .filter_map(|t| {
                let s = t.as_slice();
                // An `["x", <hex>, "aggregate"]` is the legacy aggregate, not
                // an artifact reference.
                (s.first().map(String::as_str) == Some("x")
                    && s.get(2).map(String::as_str) != Some("aggregate"))
                .then(|| s.get(1))
                .flatten()
            })
            .collect();

        // LEGACY NIP-5D (path/aggregate): remove after 2026-12-31
        if artifacts.is_empty() && legacy::has_path_tags(&event) {
            return legacy::parse(event, kind);
        }

        let artifact = match artifacts.as_slice() {
            [] => {
                return Err(NappletError::invalid_manifest(
                    "manifest carries no x artifact tag",
                ))
            }
            [one] => *one,
            _ => {
                return Err(NappletError::invalid_manifest(
                    "manifest carries more than one x tag",
                ))
            }
        };
        if !is_sha256_hex(artifact) {
            return Err(NappletError::invalid_manifest(format!(
                "x tag {artifact:?} is not a sha256"
            )));
        }
        let artifact = artifact.to_ascii_lowercase();

        if event.content.trim().is_empty() {
            return Err(NappletError::invalid_manifest(
                "manifest content (the napplet's description) is empty",
            ));
        }

        let mut d_tag = None;
        let mut servers = Vec::new();
        let mut requires: Vec<String> = Vec::new();
        let mut optional: Vec<String> = Vec::new();
        let mut roles: Vec<String> = Vec::new();
        let mut intents: Vec<IntentAdvert> = Vec::new();
        let mut config_schema = None;
        let mut title = None;
        let mut icon = None;
        let mut icon_seen = false;
        let mut app = None;
        let mut source = None;

        for tag in event.tags.iter() {
            let slice = tag.as_slice();
            let value = slice.get(1).filter(|v| !v.is_empty());
            match slice.first().map(String::as_str) {
                Some("d") => d_tag = slice.get(1).cloned(),
                Some("server") => push_unique(&mut servers, value),
                Some("R") => push_unique(&mut requires, value.filter(|d| is_slug(d))),
                Some("O") => push_unique(&mut optional, value.filter(|d| is_slug(d))),
                Some("z") => push_unique(&mut roles, value.filter(|r| is_slug(r))),
                Some("i") => {
                    // An unusable advertisement contributes no contract; it
                    // does not invalidate the manifest (NAP-INTENT).
                    if let Some(convention) = value.filter(|c| is_convention(c)) {
                        if !intents.iter().any(|i| &i.convention == convention) {
                            intents.push(IntentAdvert {
                                convention: convention.clone(),
                                params: slice[2..].to_vec(),
                            });
                        }
                    }
                }
                Some("icon") => {
                    // 0-1: a second icon tag makes the choice ambiguous, so
                    // neither is used and the generic artwork shows.
                    icon = if icon_seen { None } else { parse_icon(slice) };
                    icon_seen = true;
                }
                Some("config") if config_schema.is_none() => {
                    config_schema = value
                        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                        .filter(serde_json::Value::is_object);
                }
                Some("title") => title = value.cloned(),
                Some("source") => source = value.cloned(),
                Some("app") => {
                    app = value
                        .filter(|a| a.starts_with("32267:"))
                        .map(|a| AppListing {
                            address: a.clone(),
                            relay: slice.get(2).filter(|r| !r.is_empty()).cloned(),
                        })
                }
                _ => {}
            }
        }

        check_kind_and_d(kind, d_tag.as_deref())?;

        // NIP-5D: "A domain in both is O" — optional wins.
        requires.retain(|d| !optional.contains(d));

        let archetypes = eligible_contracts(&roles, &intents);
        let author = event.pubkey;
        let description = Some(event.content.clone());
        Ok(Self {
            event,
            author,
            kind,
            format: ManifestFormat::Current,
            d_tag,
            identity_hash: artifact.clone(),
            artifact,
            servers,
            requires,
            optional,
            roles,
            intents,
            archetypes,
            config_schema,
            title,
            description,
            icon,
            app,
            source,
        })
    }

    /// The `(dTag, identityHash)` identity tuple. `d_tag` is `""` for root and
    /// snapshot manifests, which have none.
    pub fn identity(&self) -> (&str, &str) {
        (self.d_tag.as_deref().unwrap_or(""), &self.identity_hash)
    }

    /// Every domain the napplet declares, required first then optional — what
    /// an install review shows. Declaring is not granting.
    pub fn declared_domains(&self) -> Vec<String> {
        let mut out = self.requires.clone();
        for d in &self.optional {
            if !out.contains(d) {
                out.push(d.clone());
            }
        }
        out
    }
}

/// Only the addressable kind is named, and it must be.
fn check_kind_and_d(kind: u16, d_tag: Option<&str>) -> Result<()> {
    match (kind, d_tag) {
        (KIND_NAMED, None) => Err(NappletError::invalid_manifest(
            "kind 35129 requires a d tag",
        )),
        (KIND_SNAPSHOT | KIND_ROOT, Some(_)) => Err(NappletError::invalid_manifest(
            "only a named manifest (35129) may carry a d tag",
        )),
        _ => Ok(()),
    }
}

fn push_unique(out: &mut Vec<String>, value: Option<&String>) {
    if let Some(v) = value {
        if !out.contains(v) {
            out.push(v.clone());
        }
    }
}

/// NAP-INTENT: a convention is eligible for dispatch by a role only when its
/// archetype segment matches a `z` on the same manifest. Tag order does not
/// matter, and an `i` is never paired with a nearby `z` by position.
fn eligible_contracts(roles: &[String], intents: &[IntentAdvert]) -> Vec<Archetype> {
    intents
        .iter()
        .filter_map(|i| {
            let archetype = convention_archetype(&i.convention)?;
            roles.iter().any(|r| r == archetype).then(|| Archetype {
                slug: archetype.to_string(),
                convention: i.convention.clone(),
            })
        })
        .collect()
}

fn parse_icon(slice: &[String]) -> Option<ManifestIcon> {
    let sha256 = slice.get(1).filter(|h| is_sha256_hex(h))?;
    let mime = slice
        .get(2)
        .filter(|m| ICON_MIME_TYPES.contains(&m.as_str()))?;
    Some(ManifestIcon {
        sha256: sha256.to_ascii_lowercase(),
        mime: mime.clone(),
    })
}

/// 64 hex characters.
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether `bytes` start like the image format `mime` declares — PNG, JPEG or
/// WebP. A cheap first gate before a platform decoder; NIP-5D requires that an
/// icon decode as its declared format, not merely be served as it.
pub fn icon_bytes_match(mime: &str, bytes: &[u8]) -> bool {
    match mime {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        "image/webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        _ => false,
    }
}

/// A lowercase slug: `[a-z0-9][a-z0-9-]*`. Archetype slugs, NAP domains and
/// NAP-INTENT actions share it.
pub fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Whether `convention` is a queryless `napplet:<archetype>/<intent>`
/// identity — the shape an `i` tag and a NAP-INTENT request carry.
pub fn is_convention(convention: &str) -> bool {
    validate_convention(convention).is_ok()
}

/// The `<intent>` half of a convention (`open` in `napplet:note/open`) — what
/// NAP-INTENT calls the action. `None` for anything that is not a convention.
pub fn convention_intent(convention: &str) -> Option<&str> {
    if !is_convention(convention) {
        return None;
    }
    convention.rsplit_once('/').map(|(_, intent)| intent)
}

/// The `<archetype>` half of a convention (`note` in `napplet:note/open`).
pub fn convention_archetype(convention: &str) -> Option<&str> {
    if !is_convention(convention) {
        return None;
    }
    convention
        .strip_prefix("napplet:")?
        .split_once('/')
        .map(|(archetype, _)| archetype)
}

/// A convention is a **queryless** `napplet:<archetype>/<intent>` identity.
/// Routing is exact equality over it, so a query string, a fragment, or extra
/// path segments would make two identities that route as one.
fn validate_convention(convention: &str) -> Result<()> {
    let bad = |why: &str| {
        Err(NappletError::invalid_manifest(format!(
            "archetype convention {convention:?} {why}"
        )))
    };
    if convention.starts_with("NAP-") {
        return bad("is a numbered NAP identifier, not a convention");
    }
    let Some(rest) = convention.strip_prefix("napplet:") else {
        return bad("must start with `napplet:`");
    };
    let mut parts = rest.split('/');
    let (Some(archetype), Some(intent), None) = (parts.next(), parts.next(), parts.next()) else {
        return bad("must be `napplet:<archetype>/<intent>`");
    };
    if archetype.is_empty() || intent.is_empty() {
        return bad("must be `napplet:<archetype>/<intent>`");
    }
    if rest.contains(['?', '#', ' ']) {
        return bad("must be queryless and fragmentless");
    }
    Ok(())
}
