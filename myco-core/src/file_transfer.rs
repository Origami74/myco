//! Native paired-peer file transfer.
//!
//! Control messages use a standard NIP-17 private-message rumor (kind 14)
//! inside NIP-59's kind-13 seal and kind-1059 gift wrap. The gift-wrapped
//! message carries only transfer metadata and control state. File bytes use a
//! random per-file XChaCha20-Poly1305 key, and that key is separately wrapped
//! with NIP-44 for the recipient's device key.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use nostr::nips::nip44::{self, Version};
use nostr::{Keys, PublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
/// The largest encrypted package a `MAX_FILE_BYTES` plaintext can produce:
/// the magic, the 24-byte nonce and the Poly1305 tag on top of the plaintext.
/// The receiver refuses any blob larger than this **before** reading it, so a
/// paired peer cannot answer a small offer with an unbounded body.
pub const MAX_PACKAGE_BYTES: u64 = MAX_FILE_BYTES as u64 + MAGIC.len() as u64 + 24 + 16;
pub const OFFER_TTL_SECS: u64 = 10 * 60;
/// How long a transfer may sit in one state before its pending control
/// message is sent again. Long enough that a reply in flight over a slow
/// multi-hop path is not doubled, short against the offer TTL.
pub const RESEND_AFTER_SECS: u64 = 12;
/// How many transfers this device will track at once. A Circle member that
/// spams offers would otherwise grow `file_transfers.json` without bound; past
/// this the oldest finished rows go first, and new offers are refused outright
/// rather than evicting something the user still has to answer.
pub const MAX_TRACKED_TRANSFERS: usize = 64;
const MAGIC: &[u8] = b"MYCO-FILE-V1\0";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTransferView {
    pub id: String,
    pub direction: String,
    pub peer_npub: String,
    pub peer_name: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub status: String,
    pub blob_hash: String,
    pub received_path: String,
    /// True only after native decryption completes and Android still needs to
    /// publish the private plaintext into MediaStore. Legacy completed rows
    /// default to false, so an app upgrade does not replay them.
    #[serde(default)]
    pub publish_pending: bool,
    pub error: String,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FileTransferRecord {
    pub view: FileTransferView,
    /// Encrypted local outbox package while an outgoing offer is pending.
    pub source_path: Option<String>,
    /// The random file key, kept only in the app-private transfer record until
    /// it is wrapped for the recipient in the ready message.
    pub key_b64: Option<String>,
    /// Encrypted package size the sender declared in its ready message, used to
    /// bound the download. Zero until a ready message arrives; records written
    /// by an older build deserialize to zero too, which reads as "unknown" and
    /// falls back to the absolute cap.
    #[serde(default)]
    pub ciphertext_size: u64,
    pub expires_at: u64,
    /// When the pending control message was last re-sent (see
    /// `Content::stalled_file_messages`). Zero until the first retry.
    #[serde(default)]
    pub last_resend_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum FileMessage {
    #[serde(rename = "myco.file_offer.v1")]
    Offer {
        transfer_id: String,
        sender_npub: String,
        recipient_npub: String,
        filename: String,
        mime: String,
        size: u64,
        issued_at: u64,
        expires_at: u64,
    },
    #[serde(rename = "myco.file_response.v1")]
    Response {
        transfer_id: String,
        sender_npub: String,
        recipient_npub: String,
        accepted: bool,
        reason: Option<String>,
    },
    #[serde(rename = "myco.file_ready.v1")]
    Ready {
        transfer_id: String,
        sender_npub: String,
        recipient_npub: String,
        filename: String,
        mime: String,
        size: u64,
        blob_hash: String,
        ciphertext_size: u64,
        key_wrap: String,
    },
    #[serde(rename = "myco.file_complete.v1")]
    Complete {
        transfer_id: String,
        sender_npub: String,
        recipient_npub: String,
    },
}

/// Payload types this transport refuses to carry, by MIME and by extension.
///
/// The receiving phone writes what arrives into **public** `Downloads/Myco` and
/// then offers "open with", so an installable package delivered under an
/// innocent-looking name is one tap away from a sideload — and the accept
/// prompt shows a filename, not a type. Nothing about sharing a file between
/// two paired phones needs to move app packages, so they are refused at the
/// boundary instead of being surfaced with a warning nobody reads.
const BLOCKED_MIME: &[&str] = &[
    "application/vnd.android.package-archive",
    "application/vnd.android.dex",
    "application/java-archive",
    "application/x-executable",
    "application/x-sharedlib",
];
const BLOCKED_EXTENSIONS: &[&str] = &[
    "apk", "apks", "apex", "aab", "xapk", "dex", "dm", "jar", "so",
];

/// `Some(reason)` when this file may not cross the transport in either
/// direction. Checked on the way out *and* on the way in: a peer running a
/// patched build does not get to skip the sender-side half.
pub(crate) fn rejected_payload(filename: &str, mime: &str) -> Option<String> {
    let claimed = mime.trim().to_ascii_lowercase();
    if BLOCKED_MIME.iter().any(|m| claimed == *m) {
        return Some("app packages cannot be shared over Myco".to_string());
    }
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext {
        Some(e) if BLOCKED_EXTENSIONS.contains(&e.as_str()) => {
            Some(format!(".{e} files cannot be shared over Myco"))
        }
        _ => None,
    }
}

/// Whether an id that arrived over the wire is one we will act on.
///
/// Locally generated ids are exactly 32 hex characters. Anything else is
/// **refused rather than sanitised** — there is no legitimate sender that
/// produces another shape, and the id is used as a path component when the
/// received file is staged, so an unchecked one is a directory traversal.
pub(crate) fn valid_transfer_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A sha256 as Blossom names it: 64 lowercase hex characters. Checked before
/// the hash is pasted into a URL path.
pub(crate) fn valid_blob_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

impl FileMessage {
    pub(crate) fn transfer_id(&self) -> &str {
        match self {
            FileMessage::Offer { transfer_id, .. }
            | FileMessage::Response { transfer_id, .. }
            | FileMessage::Ready { transfer_id, .. }
            | FileMessage::Complete { transfer_id, .. } => transfer_id,
        }
    }
}

pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(crate) fn new_transfer_id() -> String {
    // The id is a correlation handle, not a secret. The file key below comes
    // from the AEAD's OsRng and is the security-sensitive random value.
    hex::encode(crate::ip_source::random_bytes(16))
}

pub(crate) fn encrypt_file(
    plaintext: &[u8],
    transfer_id: &str,
    recipient_npub: &str,
    filename: &str,
) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    if plaintext.len() > MAX_FILE_BYTES {
        anyhow::bail!("file is larger than the 64 MiB native transfer limit");
    }
    // The AEAD's own CSPRNG (`getrandom` under the hood) for both values —
    // never `ip_source::random_bytes`, which is a clock-seeded xorshift and is
    // only ever safe for non-secret correlation handles.
    let key = XChaCha20Poly1305::generate_key(&mut OsRng);
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let key_bytes = key.to_vec();
    let cipher = XChaCha20Poly1305::new(&key);
    let aad = aad(transfer_id, recipient_npub, filename);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("file encryption failed"))?;
    let mut package = Vec::with_capacity(MAGIC.len() + nonce.len() + ciphertext.len());
    package.extend_from_slice(MAGIC);
    package.extend_from_slice(&nonce);
    package.extend_from_slice(&ciphertext);
    Ok((package, key_bytes))
}

pub(crate) fn decrypt_file(
    package: &[u8],
    key_bytes: &[u8],
    transfer_id: &str,
    recipient_npub: &str,
    filename: &str,
) -> anyhow::Result<Vec<u8>> {
    if key_bytes.len() != 32 || package.len() < MAGIC.len() + 24 {
        anyhow::bail!("invalid encrypted file package");
    }
    if &package[..MAGIC.len()] != MAGIC {
        anyhow::bail!("unrecognised encrypted file package");
    }
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key_bytes));
    let aad = aad(transfer_id, recipient_npub, filename);
    cipher
        .decrypt(
            XNonce::from_slice(&package[MAGIC.len()..MAGIC.len() + 24]),
            Payload {
                msg: &package[MAGIC.len() + 24..],
                aad: &aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("file authentication failed"))
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub(crate) fn encode_key(key: &[u8]) -> String {
    B64.encode(key)
}

pub(crate) fn decode_key(value: &str) -> anyhow::Result<Vec<u8>> {
    let key = B64.decode(value)?;
    if key.len() != 32 {
        anyhow::bail!("invalid file key length");
    }
    Ok(key)
}

pub(crate) fn wrap_key(sender: &Keys, recipient: &PublicKey, key: &[u8]) -> anyhow::Result<String> {
    Ok(nip44::encrypt(
        sender.secret_key(),
        recipient,
        encode_key(key),
        Version::default(),
    )?)
}

pub(crate) fn unwrap_key(
    recipient: &Keys,
    sender: &PublicKey,
    wrapped: &str,
) -> anyhow::Result<Vec<u8>> {
    decode_key(&nip44::decrypt(recipient.secret_key(), sender, wrapped)?)
}

pub(crate) fn safe_filename(name: &str, fallback: &str) -> String {
    let candidate = std::path::Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(fallback)
        .trim();
    let cleaned: String = candidate
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned.chars().take(180).collect()
    }
}

fn aad(transfer_id: &str, recipient_npub: &str, filename: &str) -> Vec<u8> {
    format!("myco-file-v1|{transfer_id}|{recipient_npub}|{filename}").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_file_round_trips_and_binds_metadata() {
        let (package, key) =
            encrypt_file(b"secret photo", "transfer-1", "npub-b", "photo.jpg").unwrap();
        assert_ne!(package, b"secret photo");
        assert_eq!(
            decrypt_file(&package, &key, "transfer-1", "npub-b", "photo.jpg").unwrap(),
            b"secret photo"
        );
        assert!(decrypt_file(&package, &key, "transfer-2", "npub-b", "photo.jpg").is_err());
        assert!(decrypt_file(&package, &key, "transfer-1", "npub-other", "photo.jpg").is_err());
    }

    #[test]
    fn tampering_fails_authentication() {
        let (mut package, key) = encrypt_file(b"secret", "transfer-1", "npub-b", "a.txt").unwrap();
        *package.last_mut().unwrap() ^= 1;
        assert!(decrypt_file(&package, &key, "transfer-1", "npub-b", "a.txt").is_err());
    }

    /// The id is a path component when the received file is staged, so a sender
    /// does not get to choose its shape.
    #[test]
    fn transfer_ids_from_the_wire_must_be_plain_hex() {
        assert!(valid_transfer_id(&new_transfer_id()));
        assert!(!valid_transfer_id(
            "../../../../data/data/app.myco/files/evil"
        ));
        assert!(!valid_transfer_id(""));
        assert!(!valid_transfer_id("nothex00000000000000000000000000"));
        assert!(!valid_transfer_id("abcdef"));
        assert!(!valid_blob_hash("../secret"));
        assert!(!valid_blob_hash(&"A".repeat(64)));
        assert!(valid_blob_hash(&"a1".repeat(32)));
    }

    /// The name and the type are checked independently, because the attack is
    /// precisely that they disagree — an app package called `holiday.jpg`, or a
    /// `.apk` politely typed as an image.
    #[test]
    fn app_packages_cannot_be_shared_under_any_name() {
        assert!(rejected_payload("photo.jpg", "image/jpeg").is_none());
        assert!(rejected_payload("notes.pdf", "application/pdf").is_none());
        assert!(
            rejected_payload("holiday.jpg", "application/vnd.android.package-archive").is_some()
        );
        assert!(rejected_payload("evil.apk", "image/jpeg").is_some());
        assert!(rejected_payload("EVIL.APK", "image/jpeg").is_some());
        assert!(rejected_payload("evil.apk", "IMAGE/JPEG").is_some());
    }

    #[test]
    fn filenames_cannot_escape_received_directory() {
        assert_eq!(safe_filename("../../photo.jpg", "file.bin"), "photo.jpg");
        assert_eq!(safe_filename("", "file.bin"), "file.bin");
    }
}
