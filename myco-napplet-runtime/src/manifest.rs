//! NIP-5D napplet manifests: the three kinds, the tags they add to NIP-5A, and
//! the validation that happens before anything is fetched.
//!
//! A napplet manifest **is** a NIP-5A manifest — the same `path` / `server` /
//! `d` / `title` layout `nsite-deck` already parses — at different kinds, plus
//! three tags:
//!
//! | Tag | Meaning |
//! |-----|---------|
//! | `["requires", "<domain>"]` | a capability the napplet needs from its runtime |
//! | `["archetype", "<slug>", "<convention>"]` | a role it can be invoked as |
//! | `["config", "<json-schema>"]` | declarative per-napplet configuration |
//!
//! and one promotion: NIP-5A's aggregate hash becomes the napplet's identity
//! rather than an integrity check, so unlike an nsite a napplet without an
//! aggregate `x` tag is rejected outright.
//!
//! Note what is *not* here: `requires` / `archetype` / `config` do not feed the
//! aggregate. Only `path` tags do — see [`nsite_deck::aggregate`].
//!
//! This layer parses and validates. It does not decide whether a manifest may
//! *load*: the single-file rule and the `/index.html` requirement are load-time
//! policy and live in [`crate::resolve`], so that a conformant multi-file
//! manifest still parses (and can be reported honestly) rather than looking
//! malformed.

use nostr::{Event, PublicKey};
use nsite_deck::aggregate::{aggregate_tag_value, path_entries_of, PathEntry};

use crate::error::{NappletError, NappletErrorCode, Result};

/// Snapshot manifest — a regular event, an immutable point-in-time release.
pub const KIND_SNAPSHOT: u16 = 5129;
/// Root manifest — replaceable, an author's latest unnamed napplet.
pub const KIND_ROOT: u16 = 15129;
/// Named manifest — addressable, carries a `d` tag identifier.
pub const KIND_NAMED: u16 = 35129;

/// All three NIP-5D manifest kinds.
///
/// Not 35128 — that is Myco's *nsite* kind, under NIP-5A. The NAP registry's
/// README calls a napplet "a NIP-5A manifest, kind 35128", which names the
/// parent spec and the parent's kind; every implementation and the build
/// tooling use `5129` / `15129` / `35129`.
pub const KINDS: [u16; 3] = [KIND_SNAPSHOT, KIND_ROOT, KIND_NAMED];

/// Whether `kind` is one of the three NIP-5D manifest kinds.
pub fn is_napplet_kind(kind: u16) -> bool {
    KINDS.contains(&kind)
}

/// A role this napplet can be invoked as by another napplet or by an inbound
/// Android intent, from an `["archetype", "<slug>", "<convention>"]` tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archetype {
    /// The routing slug (`profile`, `chat`).
    pub slug: String,
    /// The queryless payload identity, `napplet:<archetype>/<intent>`.
    pub convention: String,
}

/// A parsed NIP-5D napplet manifest.
///
/// Every field is derived from the event's tags. The aggregate is **verified**
/// here, not merely read: a manifest whose `x` tag disagrees with its `path`
/// tags does not parse.
#[derive(Debug, Clone)]
pub struct NappletManifest {
    pub event: Event,
    pub author: PublicKey,
    pub kind: u16,
    /// The `d` identifier for a named napplet; `None` for root and snapshot.
    pub d_tag: Option<String>,
    /// File entries in tag order, exactly as the `path` tags spell them.
    pub paths: Vec<PathEntry>,
    /// The verified aggregate hash. With [`NappletManifest::d_tag`] this is the
    /// napplet's identity — assigned by the runtime from verified bytes, never
    /// asserted by the napplet.
    pub aggregate: String,
    /// `["server", <blossom-url>]` hints, for the online-fetch path.
    pub servers: Vec<String>,
    /// NAP capability domains the napplet asks for. Not validated against the
    /// registry: the spec set churns (D7), so what is *implemented* and what is
    /// *granted* are decided above this layer.
    pub requires: Vec<String>,
    pub archetypes: Vec<Archetype>,
    /// The `["config", "<json-schema>"]` schema, parsed as JSON.
    pub config_schema: Option<serde_json::Value>,
    pub title: Option<String>,
    pub description: Option<String>,
}

impl NappletManifest {
    /// Parse and validate a NIP-5D manifest event.
    ///
    /// Does **not** verify the event's signature — [`crate::resolve`] does that
    /// first, before anything else. Does verify the aggregate, because an
    /// unverifiable identity is not an identity.
    pub fn from_event(event: Event) -> Result<Self> {
        let kind = event.kind.as_u16();
        if !is_napplet_kind(kind) {
            return Err(NappletError::invalid_manifest(format!(
                "event kind {kind} is not a NIP-5D napplet manifest (5129/15129/35129)"
            )));
        }

        let paths = path_entries_of(&event);
        if paths.is_empty() {
            return Err(NappletError::invalid_manifest("manifest has no path tags"));
        }

        let mut d_tag = None;
        let mut servers = Vec::new();
        let mut requires = Vec::new();
        let mut archetypes = Vec::new();
        let mut config_schema = None;
        let mut title = None;
        let mut description = None;

        for tag in event.tags.iter() {
            let slice = tag.as_slice();
            match slice.first().map(String::as_str) {
                Some("d") => d_tag = slice.get(1).cloned(),
                Some("server") => push_value(&mut servers, slice),
                Some("requires") => {
                    let domain = slice.get(1).ok_or_else(|| {
                        NappletError::invalid_manifest("requires tag has no domain")
                    })?;
                    validate_domain(domain)?;
                    if !requires.iter().any(|d| d == domain) {
                        requires.push(domain.clone());
                    }
                }
                Some("archetype") => archetypes.push(parse_archetype(slice)?),
                Some("config") => {
                    let raw = slice.get(1).ok_or_else(|| {
                        NappletError::invalid_manifest("config tag has no schema")
                    })?;
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

        // The kind/d-tag invariant: only the addressable kind is named.
        match (kind, &d_tag) {
            (KIND_NAMED, None) => {
                return Err(NappletError::invalid_manifest(
                    "kind 35129 requires a d tag",
                ))
            }
            (KIND_SNAPSHOT | KIND_ROOT, Some(_)) => {
                return Err(NappletError::invalid_manifest(
                    "only a named manifest (35129) may carry a d tag",
                ))
            }
            _ => {}
        }

        // NIP-5A tolerates a missing aggregate; NIP-5D cannot, because the
        // aggregate *is* the napplet's identity.
        let declared = aggregate_tag_value(event.tags.iter().map(|t| t.as_slice()))
            .ok_or_else(|| NappletError::invalid_manifest("manifest has no aggregate x tag"))?;
        let computed = nsite_deck::aggregate::compute_aggregate_hash(&paths);
        if !computed.eq_ignore_ascii_case(&declared) {
            return Err(NappletError::new(
                NappletErrorCode::AggregateMismatch,
                format!("recomputed aggregate {computed} != manifest {declared}"),
            ));
        }

        let author = event.pubkey;
        Ok(Self {
            event,
            author,
            kind,
            d_tag,
            paths,
            aggregate: computed,
            servers,
            requires,
            archetypes,
            config_schema,
            title,
            description,
        })
    }

    /// The `(dTag, aggregateHash)` identity tuple. `d_tag` is `""` for root and
    /// snapshot manifests, which have none.
    pub fn identity(&self) -> (&str, &str) {
        (self.d_tag.as_deref().unwrap_or(""), &self.aggregate)
    }

    /// The `/index.html` entry, under any of the spellings a manifest may use
    /// for it.
    pub fn index_entry(&self) -> Option<&PathEntry> {
        const INDEX_PATHS: [&str; 3] = ["/index.html", "index.html", "/"];
        self.paths
            .iter()
            .find(|e| INDEX_PATHS.contains(&e.path.as_str()))
    }
}

fn push_value(out: &mut Vec<String>, slice: &[String]) {
    if let Some(value) = slice.get(1) {
        if !value.is_empty() {
            out.push(value.clone());
        }
    }
}

/// A NAP domain is a lowercase slug (`relay`, `intent`, `resource`). The set is
/// deliberately open — pinning it to today's registry would reject a napplet
/// asking for a domain added upstream tomorrow, which is a policy decision, not
/// a parse error.
fn validate_domain(domain: &str) -> Result<()> {
    if is_slug(domain) {
        Ok(())
    } else {
        Err(NappletError::invalid_manifest(format!(
            "requires domain {domain:?} is not a lowercase slug"
        )))
    }
}

fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
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
    // A routing archetype and a payload convention stay orthogonal per
    // NAP-INTENT: one convention may serve several archetypes and the reverse.
    Ok(Archetype {
        slug: slug.clone(),
        convention: convention.clone(),
    })
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
