//! Files shared with the people around you: Myco's own, interim file share
//! (roadmap N13 moves it into the filedrop napplet).
//!
//! A file shared into Myco from another app is **sealed** with a key made
//! for it (filedrop's envelope, [`seal`]) and the sealed bytes are kept in
//! the `nearby` neighborhood's scope. Nothing is announced: nobody is told,
//! and nothing lists it. What reaches someone is the file's **link**
//! ([`link::FileLink`]), which the sender shows as a QR code or an NFC tap:
//! it carries the blob's hash, the file's name, type and size, and the key
//! in its fragment. The receiver's phone fetches the sealed bytes over its
//! nearby link with the sender, checks the hash, opens them with the key and
//! saves the file.
//!
//! So only the people the sender shows the link to can open the file, the
//! phones in between see sealed bytes and a hash, and like everything in
//! Nearby it is forgotten when nobody is in reach.

pub mod link;
pub mod seal;

use super::Neighbors;
use link::FileLink;

/// The largest file shared, as the old file transfer took.
pub const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
/// How many times a file is fetched from the links before giving up, and
/// how long between tries. Each try has no fixed total limit: only an idle
/// one (`http::TRANSFER_IDLE`) and a minimum rate (`http::TRANSFER_MIN_RATE`).
pub const FETCH_ATTEMPTS: u32 = 4;
pub const FETCH_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(5);

/// Why a file wasn't shared or saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileError {
    NearbyOff,
    Empty,
    TooLarge,
    Failed,
    /// Not had from any phone in reach.
    NotInReach,
    /// The bytes came but didn't open with the link's key.
    WrongKey,
}

impl FileError {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NearbyOff => "Nearby is off",
            Self::Empty => "the file is empty",
            Self::TooLarge => "the file is larger than 64 MB",
            Self::Failed => "the file could not be kept",
            Self::NotInReach => "the phone that shared it isn't in reach",
            Self::WrongKey => "the file didn't open with this code",
        }
    }
}

/// The longest file name kept, as filedrop's `cleanName`.
pub const MAX_NAME: usize = 200;

/// A file name from a link or the share menu, as filedrop's `cleanName`
/// makes it: control characters, bidi marks and overrides (which can make
/// `x\u{202E}gpj.exe` read as `xexe.jpg`) and path separators become `_`;
/// trimmed and at most [`MAX_NAME`] characters.
pub(crate) fn clean(s: &str) -> String {
    let swapped: String = s
        .chars()
        .map(|c| {
            let bidi = matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}');
            if c.is_control() || bidi || c == '/' || c == '\\' {
                '_'
            } else {
                c
            }
        })
        .collect();
    match swapped.trim().chars().take(MAX_NAME).collect::<String>() {
        n if n.is_empty() => "file".to_string(),
        n => n,
    }
}

/// A MIME type: `type/subtype`, lower case, the characters RFC 6838
/// allows; else none.
pub(crate) fn clean_mime(s: &str) -> Option<String> {
    let m = s.split(';').next()?.trim().to_ascii_lowercase();
    let ok = m.len() <= 100
        && m.split('/').count() == 2
        && m.split('/').all(|p| !p.is_empty())
        && m.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/.+-_".contains(&b));
    ok.then_some(m)
}

impl Neighbors {
    /// Share `bytes` (named `name`, of type `mime`) with the people in
    /// Nearby: sealed under a new key and kept in its scope here, announced
    /// to nobody. The link to show whoever should get it.
    pub async fn share_file_nearby(
        &self,
        bytes: Vec<u8>,
        name: &str,
        mime: &str,
    ) -> Result<FileLink, FileError> {
        let n = super::nearby::id();
        if self.record(n).is_none() || !self.nearby_on() {
            return Err(FileError::NearbyOff);
        }
        if bytes.is_empty() {
            return Err(FileError::Empty);
        }
        if bytes.len() > MAX_FILE_BYTES {
            return Err(FileError::TooLarge);
        }
        let key = seal::new_key();
        let sealed = seal::seal(&bytes, &key);
        let x = nsite_deck::sync::sha256_hex(&sealed);
        self.scoped
            .add_blob(n, &sealed, &x)
            .await
            .map_err(|_| FileError::Failed)?;
        tracing::info!(size = bytes.len(), "file sealed and kept for Nearby");
        Ok(FileLink {
            x,
            name: clean(name),
            mime: clean_mime(mime).unwrap_or_else(|| "application/octet-stream".into()),
            size: bytes.len() as u64,
            encrypted: true,
            key: Some(key),
        })
    }

    /// The file a link names: its bytes kept here, else fetched from a phone
    /// in reach over its nearby link ([`FETCH_ATTEMPTS`] tries), checked
    /// against the hash and opened with the link's key.
    pub async fn fetch_nearby_file(&self, link: &FileLink) -> Result<Vec<u8>, FileError> {
        let n = super::nearby::id();
        let mut blob = self
            .scoped
            .blob(n, &link.x)
            .await
            .ok()
            .flatten()
            .filter(|b| nsite_deck::sync::sha256_hex(b) == link.x);
        for attempt in 0..FETCH_ATTEMPTS {
            if blob.is_some() {
                break;
            }
            if attempt > 0 {
                tokio::time::sleep(FETCH_RETRY_AFTER).await;
            }
            blob = self
                .blob_from_links(n, &link.x, None)
                .await
                .filter(|b| nsite_deck::sync::sha256_hex(b) == link.x);
        }
        let blob = blob.ok_or(FileError::NotInReach)?;
        let key = link.key.as_ref().ok_or(FileError::WrongKey)?;
        seal::open(&blob, key).ok_or(FileError::WrongKey)
    }

    /// Whether Nearby can serve the blob `x`: kept here, or held by a phone
    /// in reach (asked with a bounded `HEAD`). `None` when some could not be
    /// asked. A link Myco can parse may still be a filedrop file shared into
    /// another neighborhood; this tells the two apart before Myco claims it.
    pub async fn nearby_has(&self, x: &str) -> Option<bool> {
        let n = super::nearby::id();
        if self.record(n).is_none() || !self.nearby_on() {
            return Some(false);
        }
        self.has_in(n, x, None).await
    }
}

#[cfg(test)]
mod tests;
