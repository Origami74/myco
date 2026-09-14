//! Settings that have to survive a restart, in `settings.json`.
//!
//! Everything here is persisted rather than held in memory like `offline_only`
//! because it decides how something is *constructed*. The content backends are
//! chosen before anything else opens, and the Wi-Fi Aware socket pool is bound
//! when the node starts — so the answers must already be on disk at startup.
//!
//! Deliberately a plain file rather than a settings framework. A missing or
//! corrupt file means "use the defaults", which is the behaviour we want anyway:
//! a broken settings file must never stop the app opening its own store.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The persisted settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// A relay to store events on instead of the embedded one. `None` (or an
    /// empty string, which is how the UI clears it) means the built-in store.
    ///
    /// Pointing this somewhere is an act of trust: reads are not re-verified,
    /// because NIP-01 makes signature checking the relay's job
    /// (`reference/thinning-custom-relay.md`, D7).
    pub custom_relay_url: Option<String>,

    /// A Blossom server to store blobs on instead of the embedded one.
    ///
    /// Sharper trade-off than the relay: blobs are the bulk of an nsite, so
    /// pointing this at an internet server means a peer pulling an app from us
    /// needs *our* connection (`reference/thinning-custom-relay.md`, D9).
    pub custom_blossom_url: Option<String>,

    /// How many concurrent Wi-Fi Aware data paths this chipset says it
    /// supports, as last reported by Kotlin.
    ///
    /// Persisted because it is not knowable when it is needed. The node binds
    /// the Aware socket pool at start, and `WifiAwareManager.getCharacteristics()`
    /// returns null while Wi-Fi is off — so on a cold launch the number is
    /// simply not available yet. Reading last launch's answer off disk gets it
    /// right every time after the first.
    ///
    /// `None` means never reported: an API below 33 (the call is 33+, above our
    /// minSdk of 29), Wi-Fi off on every launch so far, or a host build.
    pub aware_data_paths: Option<u8>,

    /// How many hops a napplet's `mesh.publish` may travel — the user's cap on
    /// NAP-MESH. `None` means the default, [`crate::mesh_wire::EVENT_TTL`].
    ///
    /// Persisted rather than held in memory because it is a promise the user
    /// made to themselves about what apps may do, and one that must not quietly
    /// reset to the default on the next launch.
    pub napplet_mesh_publish_ttl: Option<u8>,

    /// How many hops a napplet's `mesh.subscribe` backlog pull may travel.
    /// `None` means the default, [`crate::mesh_relay::MAX_REQ_TTL`] — lower
    /// than the publish default, because a flooded read costs more than a
    /// flooded write.
    pub napplet_mesh_subscribe_ttl: Option<u8>,
}

/// The most a user may allow a napplet publish to travel: the same number the
/// mesh clamps any forwarded event to, so a higher setting could never take
/// effect past the first hop and would only mislead.
pub const NAPPLET_MESH_PUBLISH_MAX: u8 = crate::mesh_wire::EVENT_TTL;

/// The most a user may allow a napplet subscribe pull to travel, for the same
/// reason.
pub const NAPPLET_MESH_SUBSCRIBE_MAX: u8 = crate::mesh_relay::MAX_REQ_TTL;

impl Settings {
    /// The configured relay, ignoring an empty value. Trimmed, because a URL
    /// pasted on a phone routinely arrives with whitespace attached.
    pub fn relay_url(&self) -> Option<String> {
        trimmed(self.custom_relay_url.as_deref())
    }

    /// The configured Blossom, ignoring an empty value.
    pub fn blossom_url(&self) -> Option<String> {
        trimmed(self.custom_blossom_url.as_deref())
    }

    /// The user's NAP-MESH caps: what was set, or the defaults, never above
    /// what the mesh itself would honour. A settings file edited by hand to
    /// say 200 reads as the maximum rather than as a promise the mesh breaks.
    pub fn napplet_mesh_limits(&self) -> myco_napplet_runtime::MeshLimits {
        myco_napplet_runtime::MeshLimits {
            publish_ttl: self
                .napplet_mesh_publish_ttl
                .unwrap_or(NAPPLET_MESH_PUBLISH_MAX)
                .min(NAPPLET_MESH_PUBLISH_MAX),
            subscribe_ttl: self
                .napplet_mesh_subscribe_ttl
                .unwrap_or(NAPPLET_MESH_SUBSCRIBE_MAX)
                .min(NAPPLET_MESH_SUBSCRIBE_MAX),
        }
    }
}

fn trimmed(v: Option<&str>) -> Option<String> {
    v.map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_string)
}

fn path_in(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

/// Read the settings, falling back to defaults on anything unreadable.
///
/// A corrupt file is logged and ignored rather than propagated: the alternative
/// is refusing to start because a preference could not be parsed.
pub fn load(data_dir: &Path) -> Settings {
    let path = path_in(data_dir);
    match std::fs::read(&path) {
        Ok(raw) => match serde_json::from_slice(&raw) {
            Ok(settings) => settings,
            Err(e) => {
                tracing::warn!(error = %e, "settings: ignoring corrupt settings.json");
                Settings::default()
            }
        },
        // Absent on first run, which is not a problem.
        Err(_) => Settings::default(),
    }
}

/// Write the settings, atomically (temp + rename) so a crash mid-write cannot
/// leave a half-file that the next launch would discard.
pub fn save(data_dir: &Path, settings: &Settings) -> anyhow::Result<()> {
    let path = path_in(data_dir);
    let json = serde_json::to_vec_pretty(settings)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("myco-settings-{}-{}", std::process::id(), tag))
    }

    #[test]
    fn a_missing_file_is_defaults_not_an_error() {
        let dir = tmp_dir("missing");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load(&dir).relay_url().is_none());
    }

    /// A settings file we cannot parse must not stop the app from starting. The
    /// built-in store is a safe fallback; refusing to launch is not.
    #[test]
    fn a_corrupt_file_falls_back_to_defaults() {
        let dir = tmp_dir("corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(path_in(&dir), b"{not json").unwrap();

        assert!(load(&dir).relay_url().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_url_round_trips_and_empty_clears_it() {
        let dir = tmp_dir("roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        save(
            &dir,
            &Settings {
                custom_relay_url: Some("  ws://10.0.0.5:4869  ".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            load(&dir).relay_url().as_deref(),
            Some("ws://10.0.0.5:4869"),
            "a pasted URL is trimmed"
        );

        // The UI clears by writing an empty string rather than deleting a key.
        save(
            &dir,
            &Settings {
                custom_relay_url: Some(String::new()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(load(&dir).relay_url().is_none(), "empty means built-in");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The NAP-MESH caps: defaults are the mesh's own numbers, a stored value
    /// is honoured, and a value above what the mesh would forward reads as the
    /// maximum rather than as a promise the mesh breaks at the first hop.
    #[test]
    fn napplet_mesh_caps_default_and_clamp() {
        let defaults = Settings::default().napplet_mesh_limits();
        assert_eq!(defaults.publish_ttl, crate::mesh_wire::EVENT_TTL);
        assert_eq!(defaults.subscribe_ttl, crate::mesh_relay::MAX_REQ_TTL);
        assert!(
            defaults.subscribe_ttl < defaults.publish_ttl,
            "a flooded read should default lower than a flooded write"
        );

        let set = Settings {
            napplet_mesh_publish_ttl: Some(1),
            napplet_mesh_subscribe_ttl: Some(0),
            ..Default::default()
        }
        .napplet_mesh_limits();
        assert_eq!((set.publish_ttl, set.subscribe_ttl), (1, 0));

        let wild = Settings {
            napplet_mesh_publish_ttl: Some(200),
            napplet_mesh_subscribe_ttl: Some(200),
            ..Default::default()
        }
        .napplet_mesh_limits();
        assert_eq!(wild.publish_ttl, NAPPLET_MESH_PUBLISH_MAX);
        assert_eq!(wild.subscribe_ttl, NAPPLET_MESH_SUBSCRIBE_MAX);
    }

    #[test]
    fn napplet_mesh_caps_round_trip() {
        let dir = tmp_dir("mesh-caps");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        save(
            &dir,
            &Settings {
                napplet_mesh_publish_ttl: Some(2),
                napplet_mesh_subscribe_ttl: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        let limits = load(&dir).napplet_mesh_limits();
        assert_eq!((limits.publish_ttl, limits.subscribe_ttl), (2, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
