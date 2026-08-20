//! The **user key**: the identity a napplet publishes as.
//!
//! Deliberately not the device key. The device key signs mesh traffic, pairing
//! and gossip — it *is* this phone on the mesh — and reusing it for social
//! events would tie everything a person says to the hardware they said it on,
//! permanently and irrevocably. D3 splits them, and the Identity screen must
//! keep them apart.
//!
//! Generated on first napplet use rather than at install, so a device that
//! never runs a napplet never has a social identity at all, and so existing
//! installs need no migration.
//!
//! Like the device key, the secret never leaves Rust. There is no FFI call that
//! returns it and no capability that exposes it: a napplet asks for a signature
//! and gets an event back.

use std::path::{Path, PathBuf};

use nostr::Keys;

const KEY_FILE: &str = "user.nsec";
const GUEST_FILE: &str = "user-guest.json";

/// The user key, plus the guest label drawn with it.
pub struct UserKey {
    pub keys: Keys,
    /// The five digits in `Myco Guest 12345`.
    ///
    /// Drawn once at key generation and persisted, rather than derived from the
    /// pubkey. Deriving it would make the label a shortened fingerprint of the
    /// identity — something that looks checkable and is not. Collisions across
    /// the mesh are expected and harmless: the pubkey is the identity, the
    /// number is only a label.
    pub guest_number: String,
}

impl UserKey {
    /// The name a freshly generated user is given.
    pub fn guest_name(&self) -> String {
        format!("Myco Guest {}", self.guest_number)
    }
}

/// Load the user key, generating and persisting one on first use.
pub fn load_or_generate(data_dir: &Path) -> anyhow::Result<UserKey> {
    let path = data_dir.join(KEY_FILE);
    let guest_path = data_dir.join(GUEST_FILE);

    if path.exists() {
        let raw = std::fs::read_to_string(&path)?.trim().to_string();
        if !raw.is_empty() {
            let keys = Keys::parse(&raw)
                .map_err(|e| anyhow::anyhow!("stored user key is unreadable: {e}"))?;
            return Ok(UserKey {
                keys,
                guest_number: read_guest_number(&guest_path),
            });
        }
    }

    let keys = Keys::generate();
    let guest_number = draw_guest_number(&keys);

    // Key first: a guest label with no key behind it is recoverable on the next
    // launch, a key whose label failed to write is only cosmetic.
    write_private(&path, &keys.secret_key().to_secret_hex())?;
    let _ = std::fs::write(
        &guest_path,
        serde_json::json!({ "guestNumber": guest_number }).to_string(),
    );

    Ok(UserKey { keys, guest_number })
}

/// Whether a user key exists yet — i.e. whether a napplet has ever run here.
pub fn exists(data_dir: &Path) -> bool {
    data_dir.join(KEY_FILE).exists()
}

/// The kind 0 published for a newly generated user.
///
/// A new user is never a bare pubkey: they have a name from the first event
/// they sign. The bio carries a link to Myco, so every event a guest publishes
/// is also an invitation — and it is a default, not a watermark. A user who
/// edits their profile through a napplet overwrites it, link included.
pub fn guest_profile_json(user: &UserKey) -> String {
    serde_json::json!({
        "name": user.guest_name(),
        "display_name": user.guest_name(),
        "about": "Sent from Myco — a mesh that works with no internet. https://zapstore.dev/app/app.myco",
    })
    .to_string()
}

fn read_guest_number(path: &Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| {
            v.get("guestNumber")
                .and_then(|n| n.as_str().map(String::from))
        })
        .unwrap_or_else(|| "00000".to_string())
}

/// Five digits, drawn from the key's own bytes.
///
/// Not a security property and not meant to be one — it is a label, and the
/// only requirement is that it is stable for a given install, which persisting
/// it provides.
fn draw_guest_number(keys: &Keys) -> String {
    let bytes = keys.public_key().to_bytes();
    let n = u32::from_be_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]) % 100_000;
    format!("{n:05}")
}

/// Write a secret with an owner-only mode where the platform has one.
fn write_private(path: &PathBuf, contents: &str) -> anyhow::Result<()> {
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "myco-user-key-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_same_key_comes_back_on_every_later_launch() {
        let dir = temp_dir();
        assert!(!exists(&dir), "a fresh install has no user key");

        let first = load_or_generate(&dir).unwrap();
        assert!(exists(&dir));
        let second = load_or_generate(&dir).unwrap();

        assert_eq!(first.keys.public_key(), second.keys.public_key());
        assert_eq!(first.guest_number, second.guest_number);
    }

    /// The key that signs what a person says must not be the key that identifies
    /// their hardware on the mesh.
    #[test]
    fn two_installs_get_different_keys() {
        let a = load_or_generate(&temp_dir()).unwrap();
        let b = load_or_generate(&temp_dir()).unwrap();
        assert_ne!(a.keys.public_key(), b.keys.public_key());
    }

    #[test]
    fn a_guest_is_named_not_a_bare_pubkey() {
        let user = load_or_generate(&temp_dir()).unwrap();
        assert_eq!(user.guest_number.len(), 5);
        assert!(user.guest_number.chars().all(|c| c.is_ascii_digit()));
        assert!(user.guest_name().starts_with("Myco Guest "));

        let profile: serde_json::Value = serde_json::from_str(&guest_profile_json(&user)).unwrap();
        assert_eq!(profile["name"], user.guest_name());
        // Every event a guest publishes carries an invitation.
        assert!(profile["about"].as_str().unwrap().contains("zapstore"));
    }

    /// A label that survives losing its sidecar, rather than a launch that fails.
    #[test]
    fn a_missing_guest_label_does_not_lose_the_key() {
        let dir = temp_dir();
        let first = load_or_generate(&dir).unwrap();
        std::fs::remove_file(dir.join(GUEST_FILE)).unwrap();

        let second = load_or_generate(&dir).unwrap();
        assert_eq!(first.keys.public_key(), second.keys.public_key());
    }
}

/// The [`Signer`](myco_napplet_runtime::seams::Signer) a napplet's capability
/// calls are mediated through.
///
/// Holds the keys and hands back signed events. There is no method that returns
/// key material, and no FFI path to one: a napplet describes an event and gets
/// an event back, or an error.
pub struct UserSigner {
    keys: Keys,
}

impl UserSigner {
    pub fn new(keys: Keys) -> Self {
        Self { keys }
    }
}

#[async_trait::async_trait]
impl myco_napplet_runtime::seams::Signer for UserSigner {
    async fn public_key(&self) -> anyhow::Result<nostr::PublicKey> {
        Ok(self.keys.public_key())
    }

    async fn sign(&self, unsigned: nostr::UnsignedEvent) -> anyhow::Result<nostr::Event> {
        unsigned
            .sign_with_keys(&self.keys)
            .map_err(|e| anyhow::anyhow!("signing failed: {e}"))
    }
}
