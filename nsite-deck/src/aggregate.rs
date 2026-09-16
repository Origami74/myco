//! NIP-5A **aggregate hash**: one content address over a manifest's whole file
//! set.
//!
//! A NIP-5A manifest declares each file with a `["path", "<abs-path>",
//! "<sha256>"]` tag and summarises the set with a single
//! `["x", "<hex>", "aggregate"]` tag. Verifying every blob individually proves
//! no file is *corrupt*; only the aggregate proves the set is *whole* — that
//! what is served is the site its author signed, with nothing removed and
//! nothing added by a re-signing intermediary.
//!
//! ```text
//! 1. each path tag becomes the line "<sha256> <abs-path>\n"
//! 2. sort the lines ascending
//! 3. concatenate as UTF-8
//! 4. sha256, lowercase hex
//! ```
//!
//! Only `path` tags feed the aggregate. `server`, `title`, `description` — and,
//! for the NIP-5D napplet manifests that share this shape, `requires`,
//! `archetype` and `config` — do not. Including any of them makes conformant
//! manifests fail to verify.
//!
//! This lives here because NIP-5A is the nsite spec `nsite-deck` implements;
//! `myco-napplet-runtime` depends on it rather than reimplementing it, since a
//! NIP-5D napplet manifest *is* a NIP-5A manifest plus tags.

use crate::sync::sha256_hex;

/// One published file: its absolute path and the lowercase-hex sha256 of its
/// bytes, exactly as they appear in a `path` tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathEntry {
    /// The `path` tag's second element (`/index.html`).
    pub path: String,
    /// The `path` tag's third element — hex sha256 of the file bytes.
    pub sha256: String,
}

/// Compute the NIP-5A aggregate over a set of path entries. Independent of
/// input order: the lines are sorted before hashing.
///
/// Tag values are hashed **verbatim**. Normalising them (lowercasing a hash,
/// say) would compute an aggregate the publisher never signed and turn a valid
/// manifest into a mismatch.
pub fn compute_aggregate_hash(entries: &[PathEntry]) -> String {
    let mut lines: Vec<String> = entries
        .iter()
        .map(|e| format!("{} {}\n", e.sha256, e.path))
        .collect();
    // Rust sorts &str by UTF-8 bytes, which is code-point order. JavaScript's
    // default sort is UTF-16 code-unit order; the two disagree only for paths
    // mixing astral-plane characters with U+E000..U+FFFF. No such path has been
    // seen in the wild, and the spec says "lexicographic" without naming an
    // encoding — noted here so the divergence is found deliberately, not by a
    // napplet that silently refuses to load.
    lines.sort();
    sha256_hex(lines.concat().as_bytes())
}

/// Extract the well-formed `["path", "<abs-path>", "<sha256>"]` tags, in tag
/// order. Malformed and empty-valued `path` tags are skipped — the aggregate
/// sorts anyway, so order matters only for reporting.
pub fn path_entries<'a>(tags: impl IntoIterator<Item = &'a [String]>) -> Vec<PathEntry> {
    let mut out = Vec::new();
    for tag in tags {
        let (Some("path"), Some(path), Some(hash)) =
            (tag.first().map(String::as_str), tag.get(1), tag.get(2))
        else {
            continue;
        };
        if path.is_empty() || hash.is_empty() {
            continue;
        }
        out.push(PathEntry {
            path: path.clone(),
            sha256: hash.clone(),
        });
    }
    out
}

/// Read the declared aggregate from an `["x", "<hex>", "aggregate"]` tag.
///
/// An `x` tag without the `"aggregate"` marker is some other content reference
/// and is ignored.
pub fn aggregate_tag_value<'a>(tags: impl IntoIterator<Item = &'a [String]>) -> Option<String> {
    tags.into_iter().find_map(|tag| {
        let is_aggregate = tag.first().map(String::as_str) == Some("x")
            && tag.get(2).map(String::as_str) == Some("aggregate");
        let value = tag.get(1)?;
        (is_aggregate && !value.is_empty()).then(|| value.clone())
    })
}

/// The outcome of checking a manifest's declared aggregate against its `path`
/// tags.
///
/// Three states, not a bool, because the three mean different things to a
/// caller: nsites tolerate [`Missing`](AggregateCheck::Missing) (a publisher
/// whose tooling predates the tag), while a napplet's identity *is* the
/// aggregate, so it does not. Neither tolerates a mismatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregateCheck {
    /// No aggregate `x` tag on the manifest.
    Missing,
    /// Declared aggregate equals the one recomputed from the `path` tags.
    Match { hash: String },
    /// Declared and recomputed aggregates differ — the file set is not the one
    /// signed. Always fatal.
    Mismatch { declared: String, computed: String },
}

impl AggregateCheck {
    /// The verified aggregate, if the manifest declared one and it matched.
    pub fn verified_hash(&self) -> Option<&str> {
        match self {
            Self::Match { hash } => Some(hash.as_str()),
            _ => None,
        }
    }

    /// Whether this is a mismatch — the case no caller may serve through.
    pub fn is_mismatch(&self) -> bool {
        matches!(self, Self::Mismatch { .. })
    }
}

/// Check a manifest event's aggregate tag against its `path` tags.
pub fn check_aggregate(event: &nostr::Event) -> AggregateCheck {
    let tags: Vec<&[String]> = event.tags.iter().map(|t| t.as_slice()).collect();
    let Some(declared) = aggregate_tag_value(tags.iter().copied()) else {
        return AggregateCheck::Missing;
    };
    let computed = compute_aggregate_hash(&path_entries(tags.iter().copied()));
    if computed.eq_ignore_ascii_case(&declared) {
        AggregateCheck::Match { hash: computed }
    } else {
        AggregateCheck::Mismatch { declared, computed }
    }
}

/// The `path` entries of a manifest event, in tag order.
pub fn path_entries_of(event: &nostr::Event) -> Vec<PathEntry> {
    path_entries(event.tags.iter().map(|t| t.as_slice()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, sha: &str) -> PathEntry {
        PathEntry {
            path: path.to_string(),
            sha256: sha.to_string(),
        }
    }

    /// The algorithm, pinned to the hex the **reference JS implementation**
    /// produces for the same two files. Recomputing the expected value with the
    /// same code under test would only prove self-consistency; this constant is
    /// what proves a manifest published by other tooling verifies here.
    ///
    /// Cross-checked against `@kehto/nip/5a`'s `computeAggregateHash`. If it
    /// ever fails, this implementation diverged from the reference and every
    /// published aggregate stops matching — do not "fix" it by updating the
    /// constant.
    #[test]
    fn known_vector() {
        let got = compute_aggregate_hash(&[
            entry("/a.txt", &sha256_hex(b"a")),
            entry("/b.txt", &sha256_hex(b"b")),
        ]);
        assert_eq!(
            got,
            "f2ea60d3bfc0af7fb6c6a6107bcbd86ba73c6c35310cfcaca28a58dd1da39411"
        );
    }

    #[test]
    fn independent_of_input_order() {
        let a = entry("/a.txt", &sha256_hex(b"a"));
        let z = entry("/z.txt", &sha256_hex(b"z"));
        assert_eq!(
            compute_aggregate_hash(&[a.clone(), z.clone()]),
            compute_aggregate_hash(&[z, a])
        );
    }

    /// The trap this module exists to avoid: only `path` tags feed the
    /// aggregate. A napplet's `requires` / `archetype` / `config` tags — and an
    /// nsite's `server` / `title` — must not change the result, or conformant
    /// manifests fail to load.
    #[test]
    fn only_path_tags_feed_the_aggregate() {
        let hash = sha256_hex(b"<h1>hi</h1>");
        let bare: Vec<Vec<String>> = vec![vec!["path".into(), "/index.html".into(), hash.clone()]];
        let decorated: Vec<Vec<String>> = vec![
            vec!["d".into(), "chat".into()],
            vec!["path".into(), "/index.html".into(), hash.clone()],
            vec!["requires".into(), "relay".into()],
            vec![
                "archetype".into(),
                "chat".into(),
                "napplet:chat/open".into(),
            ],
            vec!["config".into(), "{\"type\":\"object\"}".into()],
            vec!["server".into(), "https://blossom.example".into()],
            vec!["title".into(), "Chat".into()],
        ];
        let of = |tags: &[Vec<String>]| {
            compute_aggregate_hash(&path_entries(tags.iter().map(|t| t.as_slice())))
        };
        assert_eq!(of(&bare), of(&decorated));
    }

    #[test]
    fn aggregate_tag_needs_the_aggregate_marker() {
        let tags: Vec<Vec<String>> = vec![
            vec!["x".into(), "deadbeef".into()],
            vec!["x".into(), "cafe".into(), "something-else".into()],
        ];
        assert_eq!(aggregate_tag_value(tags.iter().map(|t| t.as_slice())), None);

        let marked: Vec<Vec<String>> = vec![vec!["x".into(), "cafe".into(), "aggregate".into()]];
        assert_eq!(
            aggregate_tag_value(marked.iter().map(|t| t.as_slice())).as_deref(),
            Some("cafe")
        );
    }

    #[test]
    fn malformed_path_tags_are_skipped() {
        let tags: Vec<Vec<String>> = vec![
            vec!["path".into()],
            vec!["path".into(), "/only-a-path".into()],
            vec!["path".into(), "".into(), "abc".into()],
            vec!["path".into(), "/ok".into(), "".into()],
            vec!["path".into(), "/ok".into(), "abc".into()],
        ];
        let entries = path_entries(tags.iter().map(|t| t.as_slice()));
        assert_eq!(entries, vec![entry("/ok", "abc")]);
    }
}
