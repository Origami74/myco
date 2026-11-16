//! What this device keeps about its neighborhoods: `neighborhoods.json`.
//!
//! One record per neighborhood the user carries — its id, local name, policy
//! and offer, muted authors, and which napplets were granted it — plus, per
//! napplet, the tokens its ids and addresses are made of. Written whole,
//! through a temp file and a rename, on every change: it is small, and changes
//! are rare and user-driven.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use myco_napplet_runtime::nap::neighbor::Offer;
use serde::{Deserialize, Serialize};

use super::id::{token, NeighborhoodId};

/// The file under the data dir.
pub const FILE: &str = "neighborhoods.json";

/// How far traffic travels, per neighborhood. Myco settings, not NAP input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    /// Hops a publish travels.
    pub publish_hops: u8,
    /// Hops a backlog request travels.
    pub backlog_hops: u8,
    /// Whether events from others are passed on.
    pub pass_on: bool,
    /// Hops an uploaded blob is pushed. 0: kept here, and peers fetch it when
    /// they want it. Blobs are heavy, so pushing is for neighborhoods like
    /// the user's own devices.
    #[serde(default)]
    pub blob_hops: u8,
    /// Extensions (see [`super::extensions`]) the user turned off here.
    /// Every one is on unless listed, including ones a later build adds.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub extensions_off: BTreeSet<String>,
}

impl Policy {
    /// Whether `extension` is on in this neighborhood.
    pub fn has(&self, extension: &str) -> bool {
        !self.extensions_off.contains(extension)
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            publish_hops: 3,
            backlog_hops: 2,
            pass_on: true,
            blob_hops: 0,
            extensions_off: BTreeSet::new(),
        }
    }
}

/// What makes a neighborhood special to this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    /// An ordinary neighborhood.
    Plain,
    /// An older version's paired devices, wrapped as a neighborhood. No longer carried: kept
    /// so old `neighborhoods.json` files still read, and dropped on open
    /// ([`Store::drop_everyone`]).
    Everyone,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: NeighborhoodId,
    /// The user's local name for it.
    pub name: String,
    pub auto_accept: bool,
    pub role: Role,
    #[serde(default)]
    pub policy: Policy,
    pub offer: Offer,
    /// Authors (hex keys) whose events are hidden and not passed on here.
    #[serde(default)]
    pub muted: BTreeSet<String>,
}

impl Record {
    /// A neighborhood the user starts or joins, with the default offer.
    pub fn new(id: NeighborhoodId, name: &str, auto_accept: bool) -> Self {
        Self {
            id,
            name: name.trim().to_string(),
            auto_accept,
            role: Role::Plain,
            policy: Policy::default(),
            offer: default_offer(),
            muted: BTreeSet::new(),
        }
    }
}

/// What a new neighborhood offers: its own content, both ways; blobs read.
pub fn default_offer() -> Offer {
    Offer {
        relay_read: true,
        relay_write: true,
        blob_read: true,
        ..Offer::default()
    }
}

/// One napplet's tokens.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    /// Token → neighborhood.
    #[serde(default)]
    pub neighborhoods: BTreeMap<String, NeighborhoodId>,
    /// Token → (neighborhood, peer key).
    #[serde(default)]
    pub peers: BTreeMap<String, (NeighborhoodId, String)>,
}

impl Tokens {
    /// This napplet's token for `id`, minted on first use.
    pub fn neighborhood(&mut self, id: NeighborhoodId) -> (String, bool) {
        if let Some((t, _)) = self.neighborhoods.iter().find(|(_, v)| **v == id) {
            return (t.clone(), false);
        }
        let t = token();
        self.neighborhoods.insert(t.clone(), id);
        (t, true)
    }

    /// This napplet's token for `peer` in `id`, minted on first use.
    pub fn peer(&mut self, id: NeighborhoodId, peer: &str) -> (String, bool) {
        if let Some((t, _)) = self.peers.iter().find(|(_, (n, p))| *n == id && p == peer) {
            return (t.clone(), false);
        }
        let t = token();
        self.peers.insert(t.clone(), (id, peer.to_string()));
        (t, true)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Store {
    #[serde(default)]
    pub neighborhoods: Vec<Record>,
    /// Napplet key → its tokens.
    #[serde(default)]
    pub tokens: BTreeMap<String, Tokens>,
}

impl Store {
    pub fn get(&self, id: NeighborhoodId) -> Option<&Record> {
        self.neighborhoods.iter().find(|r| r.id == id)
    }

    pub fn get_mut(&mut self, id: NeighborhoodId) -> Option<&mut Record> {
        self.neighborhoods.iter_mut().find(|r| r.id == id)
    }

    /// Drop the Everyone neighborhood an older version kept for its paired devices.
    /// Returns whether there was one.
    pub fn drop_everyone(&mut self) -> bool {
        let before = self.neighborhoods.len();
        self.neighborhoods.retain(|r| r.role != Role::Everyone);
        self.neighborhoods.len() != before
    }
}

/// The key a napplet's tokens are filed under: author and `d`
/// tag, so they survive a new version of the napplet.
pub fn napplet_key(napplet: &myco_napplet_runtime::NappletIdentity) -> String {
    format!("{}:{}", napplet.author, napplet.d_tag)
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// Read the store; a missing or unreadable file is an empty store. An
/// unreadable one is logged and set aside rather than overwritten, so a bad
/// write never silently loses the user's neighborhoods.
pub fn load(file: &Path) -> Store {
    match std::fs::read(file) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(store) => store,
            Err(e) => {
                tracing::warn!(error = %e, "neighborhoods.json unreadable; keeping it aside");
                let _ = std::fs::rename(file, file.with_extension("json.bad"));
                Store::default()
            }
        },
        Err(_) => Store::default(),
    }
}

pub fn save(file: &Path, store: &Store) -> anyhow::Result<()> {
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(store)?)?;
    std::fs::rename(&tmp, file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("myco-neighbor-store-{}", token()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn an_old_everyone_record_is_dropped() {
        let club = Record::new(NeighborhoodId::random(), "Club", false);
        // As an older version wrote it.
        let old = serde_json::json!({
            "neighborhoods": [
                {
                    "id": NeighborhoodId::random(),
                    "name": "Everyone",
                    "autoAccept": false,
                    "role": "everyone",
                    "offer": default_offer(),
                    "grants": { "all": true },
                },
                club,
            ],
        });
        let mut s: Store = serde_json::from_value(old).unwrap();
        assert!(s.drop_everyone());
        assert_eq!(s.neighborhoods, [club]);
        assert!(!s.drop_everyone());
    }

    #[test]
    fn tokens_are_stable_per_target() {
        let mut t = Tokens::default();
        let id = NeighborhoodId::random();
        let (a, minted) = t.neighborhood(id);
        assert!(minted);
        assert_eq!(t.neighborhood(id), (a.clone(), false));
        let (p, _) = t.peer(id, "npub1x");
        assert_ne!(p, a);
        assert_eq!(t.peer(id, "npub1x").0, p);
        assert_ne!(t.peer(id, "npub1y").0, p);
    }

    #[test]
    fn saves_and_loads() {
        let file = path(&dir());
        let mut s = Store::default();
        s.neighborhoods
            .push(Record::new(NeighborhoodId::random(), "Club", false));
        s.tokens
            .entry("a:chat".into())
            .or_default()
            .neighborhood(s.neighborhoods[0].id);
        save(&file, &s).unwrap();
        assert_eq!(load(&file), s);
    }

    #[test]
    fn an_unreadable_file_is_set_aside_not_lost() {
        let d = dir();
        let file = path(&d);
        std::fs::write(&file, b"{not json").unwrap();
        assert_eq!(load(&file), Store::default());
        assert!(d.join("neighborhoods.json.bad").exists());
    }
}
