//! The **user key**: the identity a napplet publishes as, and the account the
//! Settings header shows.
//!
//! Deliberately not the device key. The device key signs mesh traffic, pairing
//! and gossip — it *is* this phone on the mesh — and reusing it for social
//! events would tie everything a person says to the hardware they said it on,
//! permanently and irrevocably. D3 splits them, and the Account page must keep
//! them apart.
//!
//! Generated as a guest on first launch, so every install has an identity
//! from the start; an install from before that gets one on its next launch.
//! Replaced by logging in with an `nsec`, and removed by logging out — after
//! which there is no user key at all, and napplets have no identity, until the
//! person logs in again or asks for a new guest.
//!
//! The secret leaves Rust by one path only: the Account page's reveal, behind
//! a warning (`AppRuntime::reveal_nsec`). No capability exposes it: a napplet
//! asks for a signature and gets an event back.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use nostr::Keys;

const KEY_FILE: &str = "user.nsec";
/// The account's sidecar: guest number, where the key came from, and whether
/// the guest profile has reached the internet yet. Named for its first use.
const META_FILE: &str = "user-guest.json";
/// Present after a logout, so the next launch does not answer "no key" with a
/// new guest. Only read when there is no key: a key on disk always wins.
const LOGGED_OUT_FILE: &str = "user-logged-out";

/// Where the user key came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Generated here, with a `Myco Guest NNNNN` profile.
    Guest,
    /// Brought by the person, as an `nsec` (or hex secret).
    Nsec,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Guest => "guest",
            Origin::Nsec => "nsec",
        }
    }
}

/// The user key, plus what the sidecar says about it.
pub struct UserKey {
    pub keys: Keys,
    pub origin: Origin,
    /// The five digits in `Myco Guest 12345`. Empty for an imported key.
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

/// What a launch finds.
pub enum Startup {
    /// A key from an earlier launch.
    Existing(UserKey),
    /// No key and no logout: a first launch (or the first since this landed).
    /// A guest was just generated, and its profile is still to be published.
    NewGuest(UserKey),
    /// The person logged out and has not logged back in.
    LoggedOut,
}

/// Load the user key, generating a guest when there has never been one.
pub fn startup(data_dir: &Path) -> anyhow::Result<Startup> {
    if let Some(user) = load(data_dir)? {
        return Ok(Startup::Existing(user));
    }
    if data_dir.join(LOGGED_OUT_FILE).exists() {
        return Ok(Startup::LoggedOut);
    }
    Ok(Startup::NewGuest(generate_guest(data_dir)?))
}

/// The stored user key, if there is one.
pub fn load(data_dir: &Path) -> anyhow::Result<Option<UserKey>> {
    let path = data_dir.join(KEY_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)?.trim().to_string();
    if raw.is_empty() {
        return Ok(None);
    }
    let keys =
        Keys::parse(&raw).map_err(|e| anyhow::anyhow!("stored user key is unreadable: {e}"))?;
    let meta = read_meta(data_dir);
    let origin = match meta.get("origin").and_then(|o| o.as_str()) {
        Some("nsec") => Origin::Nsec,
        _ => Origin::Guest,
    };
    let guest_number = match origin {
        Origin::Guest => meta
            .get("guestNumber")
            .and_then(|n| n.as_str())
            .unwrap_or("00000")
            .to_string(),
        Origin::Nsec => String::new(),
    };
    Ok(Some(UserKey {
        keys,
        origin,
        guest_number,
    }))
}

/// Generate a new guest, replacing any key (the caller logs out first).
pub fn generate_guest(data_dir: &Path) -> anyhow::Result<UserKey> {
    let keys = Keys::generate();
    let guest_number = draw_guest_number(&keys);
    // Key first: a guest label with no key behind it is recoverable on the next
    // launch, a key whose label failed to write is only cosmetic.
    write_private(&data_dir.join(KEY_FILE), &keys.secret_key().to_secret_hex())?;
    write_meta(
        data_dir,
        serde_json::json!({
            "guestNumber": guest_number,
            "origin": Origin::Guest.as_str(),
            "profilePublished": false,
        }),
    );
    let _ = std::fs::remove_file(data_dir.join(LOGGED_OUT_FILE));
    Ok(UserKey {
        keys,
        origin: Origin::Guest,
        guest_number,
    })
}

/// Log in with a pasted secret: `nsec1…` or 64 hex characters.
pub fn import(data_dir: &Path, secret: &str) -> anyhow::Result<UserKey> {
    let secret = secret.trim();
    // Pasting the wrong half of a key pair is the likely mistake, and the
    // parser's own error would not say so.
    if secret.starts_with("npub1") {
        anyhow::bail!(
            "That is a public key (npub). Paste the secret key, which starts with nsec1."
        );
    }
    let keys = Keys::parse(secret)
        .map_err(|_| anyhow::anyhow!("That is not a valid nsec. Check it was copied in full."))?;
    write_private(&data_dir.join(KEY_FILE), &keys.secret_key().to_secret_hex())?;
    write_meta(
        data_dir,
        serde_json::json!({ "origin": Origin::Nsec.as_str() }),
    );
    let _ = std::fs::remove_file(data_dir.join(LOGGED_OUT_FILE));
    Ok(UserKey {
        keys,
        origin: Origin::Nsec,
        guest_number: String::new(),
    })
}

/// Remove the user key. The marker is written first, so a kill between the
/// two leaves either a logged-in key (a key wins over the marker) or a clean
/// logout — never an empty directory that the next launch reads as a first
/// launch and fills with a new guest.
pub fn logout(data_dir: &Path) -> anyhow::Result<()> {
    std::fs::write(data_dir.join(LOGGED_OUT_FILE), b"")?;
    match std::fs::remove_file(data_dir.join(KEY_FILE)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let _ = std::fs::remove_file(data_dir.join(META_FILE));
    Ok(())
}

/// Whether the guest profile still has to reach the internet. Absent means
/// published: a guest from before this was tracked is not re-published.
pub fn profile_pending(data_dir: &Path) -> bool {
    read_meta(data_dir)
        .get("profilePublished")
        .and_then(|p| p.as_bool())
        .map(|published| !published)
        .unwrap_or(false)
}

/// Record that the guest profile has reached at least one public relay.
pub fn mark_profile_published(data_dir: &Path) {
    let mut meta = read_meta(data_dir);
    if let Some(obj) = meta.as_object_mut() {
        obj.insert("profilePublished".into(), true.into());
        write_meta(data_dir, meta);
    }
}

/// The kind 0 published for a newly generated user.
///
/// A new user is never a bare pubkey: they have a name from the first event
/// they sign. The bio carries a link to Myco, so every event a guest publishes
/// is also an invitation — and it is a default, not a watermark. A user who
/// edits their profile through a napplet overwrites it, link included.
pub fn guest_profile_json(user: &UserKey, picture: Option<&str>) -> String {
    let mut profile = serde_json::json!({
        "name": user.guest_name(),
        "display_name": user.guest_name(),
        "about": GUEST_ABOUT,
    });
    if let Some(picture) = picture {
        profile["picture"] = picture.into();
    }
    profile.to_string()
}

pub const GUEST_ABOUT: &str = "I'm a guest user of the Myco app. Join me at https://getmyco.app";

fn read_meta(data_dir: &Path) -> serde_json::Value {
    std::fs::read_to_string(data_dir.join(META_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn write_meta(data_dir: &Path, meta: serde_json::Value) {
    let _ = std::fs::write(data_dir.join(META_FILE), meta.to_string());
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

/// Write a secret atomically, with an owner-only mode where the platform has
/// one.
///
/// Temp file + rename, like `settings_store::save` and `save_library`: a kill
/// between truncate and write used to leave an empty `user.nsec`, which the
/// next launch read as "no key" and answered with a new social identity. The
/// mode is set on the temp file at creation (`OpenOptionsExt::mode`), so the
/// secret is never on disk world-readable, not even between a write and a
/// `chmod`. A stale temp file from an interrupted write is removed and the
/// new one created exclusively — never read, and never inherited with
/// whatever mode it had.
fn write_private(path: &PathBuf, contents: &str) -> anyhow::Result<()> {
    use std::io::Write;

    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// The logged-in keys, shared by the signer and the account. `None` while
/// logged out.
pub type Slot = Arc<RwLock<Option<Keys>>>;

/// The [`Signer`](myco_napplet_runtime::seams::Signer) a napplet's capability
/// calls are mediated through.
///
/// Reads the account's slot on every call, so a login or logout takes effect
/// on an open napplet's next call rather than its next launch. There is no
/// method that returns key material: a napplet describes an event and gets an
/// event back, or an error.
pub struct UserSigner {
    slot: Slot,
}

impl UserSigner {
    pub fn new(slot: Slot) -> Self {
        Self { slot }
    }

    fn keys(&self) -> anyhow::Result<Keys> {
        self.slot
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(|| anyhow::anyhow!("not logged in"))
    }
}

#[async_trait::async_trait]
impl myco_napplet_runtime::seams::Signer for UserSigner {
    async fn public_key(&self) -> anyhow::Result<nostr::PublicKey> {
        Ok(self.keys()?.public_key())
    }

    async fn sign(&self, unsigned: nostr::UnsignedEvent) -> anyhow::Result<nostr::Event> {
        unsigned
            .sign_with_keys(&self.keys()?)
            .map_err(|e| anyhow::anyhow!("signing failed: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        // A counter beside the clock: tests run in parallel, and two that
        // drew the same nanosecond shared a directory.
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "myco-user-key-{}-{}-{n}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn startup_user(dir: &Path) -> UserKey {
        match startup(dir).unwrap() {
            Startup::Existing(user) | Startup::NewGuest(user) => user,
            Startup::LoggedOut => panic!("logged out"),
        }
    }

    #[test]
    fn the_same_key_comes_back_on_every_later_launch() {
        let dir = temp_dir();
        let first = match startup(&dir).unwrap() {
            Startup::NewGuest(user) => user,
            _ => panic!("a fresh install gets a new guest"),
        };
        let second = match startup(&dir).unwrap() {
            Startup::Existing(user) => user,
            _ => panic!("the second launch finds the first key"),
        };
        assert_eq!(first.keys.public_key(), second.keys.public_key());
        assert_eq!(first.guest_number, second.guest_number);
        assert_eq!(second.origin, Origin::Guest);
    }

    /// The key that signs what a person says must not be the key that identifies
    /// their hardware on the mesh.
    #[test]
    fn two_installs_get_different_keys() {
        let a = startup_user(&temp_dir());
        let b = startup_user(&temp_dir());
        assert_ne!(a.keys.public_key(), b.keys.public_key());
    }

    #[test]
    fn a_guest_is_named_not_a_bare_pubkey() {
        let user = startup_user(&temp_dir());
        assert_eq!(user.guest_number.len(), 5);
        assert!(user.guest_number.chars().all(|c| c.is_ascii_digit()));
        assert!(user.guest_name().starts_with("Myco Guest "));

        let profile: serde_json::Value =
            serde_json::from_str(&guest_profile_json(&user, Some("https://x/y.jpg"))).unwrap();
        assert_eq!(profile["name"], user.guest_name());
        assert_eq!(profile["picture"], "https://x/y.jpg");
        // Every event a guest publishes carries an invitation.
        assert!(profile["about"]
            .as_str()
            .unwrap()
            .contains("https://getmyco.app"));
    }

    /// The secret lands by rename, owner-only from the first byte: no temp
    /// file is left behind, the mode is 0600 on unix, and a temp file planted
    /// by an interrupted earlier write is overwritten rather than read.
    #[test]
    fn the_key_is_written_atomically_and_private() {
        let dir = temp_dir();
        let key = dir.join(KEY_FILE);
        let tmp = key.with_extension("tmp");
        std::fs::write(&tmp, "not a key").unwrap();

        let user = startup_user(&dir);

        assert!(key.is_file(), "the key was not written");
        assert!(!tmp.exists(), "the temp file was left behind");
        let stored = std::fs::read_to_string(&key).unwrap();
        assert_eq!(stored.trim(), user.keys.secret_key().to_secret_hex());
        assert_ne!(stored.trim(), "not a key", "the planted temp file was read");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&key).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the key is readable by others: {mode:o}");
        }
    }

    /// A label that survives losing its sidecar, rather than a launch that fails.
    #[test]
    fn a_missing_guest_label_does_not_lose_the_key() {
        let dir = temp_dir();
        let first = startup_user(&dir);
        std::fs::remove_file(dir.join(META_FILE)).unwrap();

        let second = startup_user(&dir);
        assert_eq!(first.keys.public_key(), second.keys.public_key());
        assert!(!profile_pending(&dir), "a lost sidecar must not re-publish");
    }

    #[test]
    fn a_new_guest_profile_is_pending_until_marked() {
        let dir = temp_dir();
        startup_user(&dir);
        assert!(profile_pending(&dir));
        mark_profile_published(&dir);
        assert!(!profile_pending(&dir));
        assert!(matches!(startup(&dir).unwrap(), Startup::Existing(_)));
    }

    /// A guest from before publishing was tracked has only `guestNumber` in
    /// its sidecar, and is not re-published.
    #[test]
    fn an_older_guest_is_not_republished() {
        let dir = temp_dir();
        write_private(
            &dir.join(KEY_FILE),
            &Keys::generate().secret_key().to_secret_hex(),
        )
        .unwrap();
        std::fs::write(dir.join(META_FILE), r#"{"guestNumber":"01234"}"#).unwrap();
        let user = startup_user(&dir);
        assert_eq!(user.origin, Origin::Guest);
        assert_eq!(user.guest_number, "01234");
        assert!(!profile_pending(&dir));
    }

    #[test]
    fn logout_is_remembered_across_launches() {
        let dir = temp_dir();
        startup_user(&dir);
        logout(&dir).unwrap();
        assert!(!dir.join(KEY_FILE).exists(), "the key is still on disk");
        assert!(matches!(startup(&dir).unwrap(), Startup::LoggedOut));
        assert!(matches!(startup(&dir).unwrap(), Startup::LoggedOut));

        let fresh = generate_guest(&dir).unwrap();
        match startup(&dir).unwrap() {
            Startup::Existing(user) => {
                assert_eq!(user.keys.public_key(), fresh.keys.public_key())
            }
            _ => panic!("a new guest after logout is logged in"),
        }
    }

    #[test]
    fn an_nsec_logs_in_and_hex_does_too() {
        let theirs = Keys::generate();
        let dir = temp_dir();
        logout(&dir).unwrap();

        let nsec = nostr::ToBech32::to_bech32(theirs.secret_key()).unwrap();
        let user = import(&dir, &format!("  {nsec}\n")).unwrap();
        assert_eq!(user.keys.public_key(), theirs.public_key());
        assert_eq!(user.origin, Origin::Nsec);
        assert!(
            !profile_pending(&dir),
            "an imported key has no guest profile"
        );

        let loaded = startup_user(&dir);
        assert_eq!(loaded.origin, Origin::Nsec);
        assert_eq!(loaded.keys.public_key(), theirs.public_key());

        let hex = import(&temp_dir(), &theirs.secret_key().to_secret_hex()).unwrap();
        assert_eq!(hex.keys.public_key(), theirs.public_key());
    }

    #[test]
    fn a_pasted_npub_says_what_went_wrong() {
        let npub = nostr::ToBech32::to_bech32(&Keys::generate().public_key()).unwrap();
        let err = import(&temp_dir(), &npub).err().unwrap().to_string();
        assert!(err.contains("npub"), "{err}");
        let err = import(&temp_dir(), "nsec1nope").err().unwrap().to_string();
        assert!(err.contains("not a valid nsec"), "{err}");
    }

    #[tokio::test]
    async fn the_signer_follows_login_and_logout() {
        use myco_napplet_runtime::seams::Signer;
        let slot: Slot = Arc::new(RwLock::new(None));
        let signer = UserSigner::new(slot.clone());
        assert!(signer.public_key().await.is_err(), "signs while logged out");

        let keys = Keys::generate();
        *slot.write().unwrap() = Some(keys.clone());
        assert_eq!(signer.public_key().await.unwrap(), keys.public_key());
        let unsigned = nostr::EventBuilder::text_note("hi").build(keys.public_key());
        assert_eq!(
            signer.sign(unsigned).await.unwrap().pubkey,
            keys.public_key()
        );

        *slot.write().unwrap() = None;
        assert!(signer.public_key().await.is_err());
    }
}
