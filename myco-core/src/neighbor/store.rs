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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    /// Hops a publish travels.
    pub publish_hops: u8,
    /// Hops a backlog request travels.
    pub backlog_hops: u8,
    /// Whether events from others are passed on.
    pub pass_on: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            publish_hops: 3,
            backlog_hops: 2,
            pass_on: true,
        }
    }
}

/// Who may see a neighborhood through NAP-NEIGHBOR.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grants {
    /// Every napplet with the `neighbor` domain. Everyone starts this way, so
    /// what NAP-MESH reached stays reachable.
    #[serde(default)]
    pub all: bool,
    /// Napplet keys (see [`napplet_key`]).
    #[serde(default)]
    pub napplets: BTreeSet<String>,
}

impl Grants {
    pub fn allows(&self, napplet: &str) -> bool {
        self.all || self.napplets.contains(napplet)
    }
}

/// What makes a neighborhood special to this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    /// An ordinary neighborhood.
    Plain,
    /// The one the Circle became; carried by every paired peer.
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
    #[serde(default)]
    pub grants: Grants,
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
            grants: Grants::default(),
        }
    }

    /// The Circle, as a neighborhood: random id, every napplet, and the offer
    /// the Circle makes today.
    pub fn everyone() -> Self {
        Self {
            role: Role::Everyone,
            offer: Offer {
                relay_read: true,
                relay_write: true,
                relay_own: true,
                blob_read: true,
                blob_write: true,
                blob_own: true,
                services: Vec::new(),
            },
            grants: Grants {
                all: true,
                napplets: BTreeSet::new(),
            },
            ..Self::new(NeighborhoodId::random(), "Everyone", false)
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

    pub fn everyone(&self) -> Option<&Record> {
        self.neighborhoods.iter().find(|r| r.role == Role::Everyone)
    }

    /// Add Everyone if this device has none — first launch, or the first
    /// launch after the Circle became a neighborhood. Returns whether it did.
    pub fn ensure_everyone(&mut self) -> bool {
        if self.everyone().is_some() {
            return false;
        }
        self.neighborhoods.insert(0, Record::everyone());
        true
    }
}

/// The key a napplet's grants and tokens are filed under: author and `d`
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
    fn everyone_is_added_once_and_offers_what_the_circle_did() {
        let mut s = Store::default();
        assert!(s.ensure_everyone());
        assert!(!s.ensure_everyone());
        let e = s.everyone().unwrap();
        assert!(e.grants.all);
        assert!(e.offer.relay_own && e.offer.blob_own && e.offer.blob_write);
        assert_eq!(e.policy, Policy::default());
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
        s.ensure_everyone();
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
