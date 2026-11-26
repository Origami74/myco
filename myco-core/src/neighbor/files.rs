//! Files shared with the people in Nearby: Myco's own, interim file share.
//!
//! A file shared into Myco from another app goes to the `nearby`
//! neighborhood: the bytes are kept in its scope here, and an announcement
//! is published into it like any post, so the phones in reach see it and
//! fetch the bytes over their nearby link when someone saves it. Nothing
//! leaves Nearby, and like everything in Nearby it is forgotten when nobody
//! is in reach.
//!
//! - **Anonymous.** Each announcement is signed by a key made for it and
//!   thrown away: Nearby has no names, and a file must not say who sent it.
//! - **Filedrop's format.** The announcement is the NIP-94 event filedrop
//!   publishes (kind 1063, `t=filedrop`, the `file` tag), unencrypted, so
//!   the napplet that replaces this ([roadmap N13]) reads what Myco sent.
//!
//! [roadmap N13]: ../../../docs/roadmap.md

use nostr::{Event, EventBuilder, Filter, Keys, Kind, Tag, TagKind, Timestamp};

use super::data::Provenance;
use super::Neighbors;

/// The announcement's kind (NIP-94 file metadata) and filedrop's tag.
pub const FILE_KIND: u16 = 1063;
pub const FILE_TAG: &str = "filedrop";
/// The largest file shared, as a raw NAP-UPLOAD takes.
pub const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
/// How long an announcement stays up (NIP-40): Nearby forgets sooner when
/// nobody is in reach.
pub const SHARED_FOR_SECS: u64 = 60 * 60;
/// The most files listed, newest first.
pub const MAX_LISTED: usize = 100;

/// A file in Nearby, as the app lists it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NearbyFile {
    /// The announcement's id.
    pub id: String,
    pub sha256: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    /// Unix seconds it was shared.
    pub at: u64,
    /// Shared from this phone.
    pub mine: bool,
}

/// Why a file wasn't shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareError {
    NearbyOff,
    Empty,
    TooLarge,
    Failed,
}

impl ShareError {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NearbyOff => "Nearby is off",
            Self::Empty => "the file is empty",
            Self::TooLarge => "the file is larger than 64 MB",
            Self::Failed => "the file could not be kept",
        }
    }
}

/// A name, type or other text from a file's announcement: no control
/// characters or path separators, and short.
fn clean(s: &str, max: usize) -> String {
    let kept: String = s
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\')
        .collect();
    kept.trim().chars().take(max).collect()
}

/// A MIME type: `type/subtype`, lower case, the characters RFC 6838
/// allows; else none.
fn clean_mime(s: &str) -> Option<String> {
    let m = s.split(';').next()?.trim().to_ascii_lowercase();
    let ok = m.len() <= 100
        && m.split('/').count() == 2
        && m.split('/').all(|p| !p.is_empty())
        && m.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/.+-_".contains(&b));
    ok.then_some(m)
}

/// The announcement for a file of `size` bytes with hash `sha256`.
fn announcement(sha256: &str, name: &str, mime: &str, size: u64, now: u64) -> EventBuilder {
    let t = |name: &str, values: Vec<String>| Tag::custom(TagKind::custom(name), values);
    EventBuilder::new(Kind::from(FILE_KIND), "").tags([
        t("x", vec![sha256.to_string()]),
        t("m", vec![mime.to_string()]),
        t("size", vec![size.to_string()]),
        t(
            "file",
            vec![name.to_string(), mime.to_string(), size.to_string()],
        ),
        t("expiration", vec![(now + SHARED_FOR_SECS).to_string()]),
        t("t", vec![FILE_TAG.to_string()]),
        t("alt", vec![format!("File: {name}")]),
    ])
}

/// `event` as a listed file, unless it isn't one (or has expired). Our own
/// sends are not trusted to be well-formed any more than a peer's.
fn parse(event: &Event, mine: bool, now: u64) -> Option<NearbyFile> {
    if event.kind.as_u16() != FILE_KIND {
        return None;
    }
    let tag = |name: &str| {
        event
            .tags
            .iter()
            .find(|t| t.kind().to_string() == name)
            .map(|t| t.as_slice().to_vec())
    };
    if !event
        .tags
        .iter()
        .any(|t| t.kind().to_string() == "t" && t.content() == Some(FILE_TAG))
    {
        return None;
    }
    // Encrypted files are filedrop's to open, with the key from its link.
    if tag("encrypted").is_some() {
        return None;
    }
    let sha256 = tag("x")?.get(1)?.to_ascii_lowercase();
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    if let Some(exp) = tag("expiration").and_then(|t| t.get(1)?.parse::<u64>().ok()) {
        if exp <= now {
            return None;
        }
    }
    let file = tag("file").unwrap_or_default();
    let name = clean(file.get(1).map(String::as_str).unwrap_or("file"), 120);
    let mime = file
        .get(2)
        .cloned()
        .or_else(|| tag("m").and_then(|m| m.get(1).cloned()))
        .and_then(|m| clean_mime(&m))
        .unwrap_or_else(|| "application/octet-stream".into());
    let size = file
        .get(3)
        .or(tag("size").as_ref().and_then(|s| s.get(1)))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Some(NearbyFile {
        id: event.id.to_hex(),
        sha256,
        name: if name.is_empty() { "file".into() } else { name },
        mime,
        size,
        at: event.created_at.as_secs(),
        mine,
    })
}

fn now() -> u64 {
    Timestamp::now().as_secs()
}

impl Neighbors {
    /// Share `bytes` (named `name`, of type `mime`) with the people in
    /// Nearby: kept in its scope here and announced into it. The file as
    /// listed.
    pub async fn share_file_nearby(
        &self,
        bytes: Vec<u8>,
        name: &str,
        mime: &str,
    ) -> Result<NearbyFile, ShareError> {
        let n = super::nearby::id();
        let Some(record) = self.record(n).filter(|_| self.nearby_on()) else {
            return Err(ShareError::NearbyOff);
        };
        if bytes.is_empty() {
            return Err(ShareError::Empty);
        }
        if bytes.len() > MAX_FILE_BYTES {
            return Err(ShareError::TooLarge);
        }
        let name = match clean(name, 120) {
            n if n.is_empty() => "file".to_string(),
            n => n,
        };
        let mime = clean_mime(mime).unwrap_or_else(|| "application/octet-stream".into());
        let sha256 = nsite_deck::sync::sha256_hex(&bytes);
        self.scoped
            .add_blob(n, &bytes, &sha256)
            .await
            .map_err(|_| ShareError::Failed)?;
        let event = announcement(&sha256, &name, &mime, bytes.len() as u64, now())
            .sign_with_keys(&Keys::generate())
            .map_err(|_| ShareError::Failed)?;
        let hops = record.policy.publish_hops;
        let added = self
            .scoped
            .add(n, event.clone(), Provenance::mine(hops), None)
            .await
            .map_err(|_| ShareError::Failed)?;
        if !added.taken() {
            return Err(ShareError::Failed);
        }
        if hops > 0 {
            self.forward(n, event.clone(), None, hops - 1);
        }
        tracing::info!(size = bytes.len(), %mime, "file shared with Nearby");
        parse(&event, true, now()).ok_or(ShareError::Failed)
    }

    /// The files in Nearby now, newest first.
    pub async fn nearby_files(&self) -> Vec<NearbyFile> {
        let n = super::nearby::id();
        if self.record(n).is_none() || !self.nearby_on() {
            return Vec::new();
        }
        let filter = Filter::new().kind(Kind::from(FILE_KIND)).custom_tag(
            nostr::SingleLetterTag::lowercase(nostr::Alphabet::T),
            FILE_TAG,
        );
        let now = now();
        let mut files: Vec<NearbyFile> = self
            .scoped
            .backlog(n, &[filter])
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(event, provenance)| parse(&event, provenance.mine, now))
            .collect();
        files.sort_by_key(|f| std::cmp::Reverse(f.at));
        files.truncate(MAX_LISTED);
        files
    }

    /// The bytes of a file in Nearby: kept here, else fetched from a phone
    /// in reach over its nearby link. Checked against the hash.
    pub async fn nearby_file_bytes(&self, sha256: &str) -> Option<Vec<u8>> {
        let n = super::nearby::id();
        let bytes = match self.scoped.blob(n, sha256).await.ok().flatten() {
            Some(bytes) => bytes,
            None => self.blob_from_links(n, sha256, None).await?,
        };
        (nsite_deck::sync::sha256_hex(&bytes) == sha256).then_some(bytes)
    }
}

#[cfg(test)]
mod tests;
