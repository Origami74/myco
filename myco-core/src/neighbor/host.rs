//! [`Neighbors`]: this device's [`NeighborHost`].

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use myco_napplet_runtime::nap::neighbor::{
    address, CreateRequest, Feature, LinkClass, NeighborError, NeighborHost, NeighborInfo,
    Neighborhood, Offer, Peer,
};
use myco_napplet_runtime::NappletIdentity;

use super::id::NeighborhoodId;
use super::store::{self, napplet_key, Record, Store};

/// One of the user's peers, as the device knows them. `key` is whatever the
/// device reaches them by: the peer's key on the link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerView {
    pub key: String,
    pub name: Option<String>,
    pub reachable: bool,
    /// How the peer is reached, when known: set for peers on an ephemeral
    /// (nearby) link.
    pub link: Option<LinkClass>,
}

/// Who carries which neighborhood: the links it was agreed on.
pub trait PeerDirectory: Send + Sync {
    fn peers(&self, neighborhood: &Record) -> Vec<PeerView>;
}

/// Something the user must agree to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// Start a neighborhood, or join the auto-accepted one of this name.
    Create {
        napplet: String,
        name: String,
        auto_accept: bool,
    },
    /// Stop carrying a neighborhood.
    Leave { napplet: String, name: String },
    /// Offer peers more than before.
    WidenOffer {
        napplet: String,
        name: String,
        offer: Offer,
    },
}

/// Puts an [`Ask`] in front of the user. `true` is yes.
#[async_trait]
pub trait Asker: Send + Sync {
    async fn confirm(&self, ask: Ask) -> bool;
}

/// Says no to everything: the host until the shell can show a prompt.
pub struct NoAsker;

#[async_trait]
impl Asker for NoAsker {
    async fn confirm(&self, _ask: Ask) -> bool {
        false
    }
}

/// The device's neighborhoods, behind NAP-NEIGHBOR.
pub struct Neighbors {
    file: PathBuf,
    store: Mutex<Store>,
    peers: Arc<dyn PeerDirectory>,
    asker: Arc<dyn Asker>,
    /// Neighborhood events and blobs, each in its scope.
    pub(super) scoped: Arc<super::data::Scoped>,
    /// Links, which carry the neighborhoods.
    pub(super) links: Option<Arc<super::link::Links>>,
    /// Told of an update manifest a linked peer shared under an extension
    /// both ends have on (see [`super::updates`]).
    pub(super) on_update: std::sync::OnceLock<super::updates::UpdateSink>,
    /// The napplets this device holds, exposed to linked neighbors where it
    /// shares `napplet-updates` (see [`super::updates::HeldNapplets`]).
    pub(super) held: std::sync::OnceLock<Arc<super::updates::HeldNapplets>>,
    /// False when the file could be neither read nor set aside: then it is
    /// never saved over, and changes last only until the app restarts.
    writable: bool,
    /// The runtime that deletes a left neighborhood's content. The shell's
    /// Leave arrives on the JNI thread, which is not inside any runtime.
    rt: std::sync::OnceLock<tokio::runtime::Handle>,
}

/// What [`Neighbors::open`] reaches the world through.
pub struct Seams {
    pub peers: Arc<dyn PeerDirectory>,
    pub asker: Arc<dyn Asker>,
    /// Where neighborhood events are stored.
    pub scoped_stores: Arc<dyn super::data::EventStores>,
    /// Where their blobs are stored.
    pub scoped_blobs: Arc<dyn nsite_deck::BlobStore>,
    /// Links, if this device has them.
    pub links: Option<Arc<super::link::Links>>,
}

/// Where an address points, for the lanes that route it (phase 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub neighborhood: NeighborhoodId,
    /// The peer's key, for a peer address.
    pub peer: Option<String>,
}

impl Neighbors {
    /// Open `neighborhoods.json` under `data_dir`, dropping the Everyone
    /// neighborhood an older version kept for its paired devices.
    pub fn open(data_dir: &std::path::Path, seams: Seams) -> Self {
        let file = store::path(data_dir);
        let (mut s, writable) = store::load_checked(&file);
        let dropped = s.drop_everyone();
        if (dropped | keep_nearby(&mut s)) && writable {
            if let Err(e) = store::save(&file, &s) {
                tracing::warn!(error = %e, "could not save neighborhoods");
            }
        }
        Self {
            file,
            writable,
            store: Mutex::new(s),
            peers: seams.peers,
            asker: seams.asker,
            scoped: super::data::Scoped::open(data_dir, seams.scoped_stores, seams.scoped_blobs),
            links: seams.links,
            on_update: std::sync::OnceLock::new(),
            held: std::sync::OnceLock::new(),
            rt: std::sync::OnceLock::new(),
        }
    }

    /// The runtime [`Neighbors::forget`] deletes content on when called
    /// from outside one.
    pub fn set_runtime(&self, handle: tokio::runtime::Handle) {
        let _ = self.rt.set(handle);
    }

    /// Run `f` on the store and save it.
    fn change<T>(&self, f: impl FnOnce(&mut Store) -> T) -> T {
        let mut s = self.store.lock().unwrap();
        let out = f(&mut s);
        if !self.writable {
            tracing::warn!("neighborhoods.json was not set aside; not saving over it");
            return out;
        }
        if let Err(e) = store::save(&self.file, &s) {
            tracing::warn!(error = %e, "could not save neighborhoods");
        }
        out
    }

    /// The neighborhood a napplet's id names. Every napplet with the
    /// `neighbor` domain may use every neighborhood; the ids are its own.
    fn granted(&self, napplet: &str, id_token: &str) -> Result<Record, NeighborError> {
        let s = self.store.lock().unwrap();
        let id = s
            .tokens
            .get(napplet)
            .and_then(|t| t.neighborhoods.get(id_token))
            .copied()
            .ok_or(NeighborError::NotFound)?;
        s.get(id).cloned().ok_or(NeighborError::NotFound)
    }

    /// `record` as `napplet` sees it, minting its tokens.
    fn view(&self, napplet: &str, record: &Record) -> Neighborhood {
        let peers = self.peers.peers(record);
        let token = self.token_for(napplet, record.id);
        Neighborhood {
            address: Some(address::format(&token, None)),
            id: token,
            name: record.name.clone(),
            auto_accept: record.auto_accept,
            peers: peers.len() as u32,
            reachable: peers.iter().filter(|p| p.reachable).count() as u32,
            offer: Some(record.offer.clone()),
        }
    }

    fn token_for(&self, napplet: &str, id: NeighborhoodId) -> String {
        let (token, minted) = {
            let mut s = self.store.lock().unwrap();
            s.tokens
                .entry(napplet.to_string())
                .or_default()
                .neighborhood(id)
        };
        if minted {
            self.change(|_| ());
        }
        token
    }

    fn peer_token_for(&self, napplet: &str, id: NeighborhoodId, peer: &str) -> String {
        let (token, minted) = {
            let mut s = self.store.lock().unwrap();
            s.tokens
                .entry(napplet.to_string())
                .or_default()
                .peer(id, peer)
        };
        if minted {
            self.change(|_| ());
        }
        token
    }

    /// What `address` points at, if `napplet` was given it and still holds
    /// the neighborhood. Any other napplet's address is refused.
    pub fn resolve(&self, napplet: &NappletIdentity, addr: &str) -> Option<Target> {
        let tokens = address::parse(addr)?;
        let key = napplet_key(napplet);
        let s = self.store.lock().unwrap();
        let mine = s.tokens.get(&key)?;
        let id = *mine.neighborhoods.get(&tokens.neighborhood)?;
        s.get(id)?;
        let peer = match tokens.peer {
            None => None,
            Some(p) => {
                let (pid, peer) = mine.peers.get(&p)?;
                if *pid != id {
                    return None;
                }
                Some(peer.clone())
            }
        };
        Some(Target {
            neighborhood: id,
            peer,
        })
    }

    /// Whether Nearby is on: the `nearby` neighborhood is carried.
    pub fn nearby_on(&self) -> bool {
        !self.store.lock().unwrap().nearby_off
    }

    /// Turn Nearby on or off. Off forgets the `nearby` neighborhood; on
    /// brings it back, under the same id. Ending the ephemeral links is the
    /// link service's (see [`super::nearby::end_all`]).
    pub fn set_nearby(&self, on: bool) {
        self.change(|s| {
            s.nearby_off = !on;
            keep_nearby(s);
        });
    }

    /// The auto-accepted neighborhoods carried here: what an ephemeral link
    /// may carry.
    pub fn auto_accepted(&self) -> Vec<super::link::Shared> {
        self.store
            .lock()
            .unwrap()
            .neighborhoods
            .iter()
            .filter(|r| r.auto_accept)
            .map(|r| super::link::Shared {
                id: r.id,
                name: r.name.clone(),
            })
            .collect()
    }

    /// Start or join a neighborhood from the shell's own UI: the user is
    /// already the one asking, so no prompt. Granted to no napplet.
    pub fn create_local(&self, name: &str, auto_accept: bool) -> Record {
        let name = name.trim().to_string();
        self.change(|s| {
            let id = if auto_accept {
                NeighborhoodId::from_name(&name)
            } else {
                NeighborhoodId::random()
            };
            // `nearby` is the runtime's, there while Nearby is on: joining
            // it by name would bring it back behind the setting's back.
            if id == super::nearby::id() {
                return s
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| Record::new(id, &name, auto_accept));
            }
            if s.get(id).is_none() {
                s.neighborhoods.push(Record::new(id, &name, auto_accept));
            }
            s.get(id).cloned().expect("just ensured")
        })
    }

    /// Carry a neighborhood someone shared over a link: known from now on,
    /// under the name it was shared with. Granted to no napplet.
    pub fn adopt(&self, shared: &super::link::Shared) {
        self.change(|s| {
            if s.get(shared.id).is_none() {
                s.neighborhoods
                    .push(Record::new(shared.id, &shared.name, false));
            }
        });
    }

    /// Set a neighborhood's reach. The Neighbors tab's.
    pub fn set_policy(
        &self,
        id: NeighborhoodId,
        publish_hops: u8,
        backlog_hops: u8,
        pass_on: bool,
    ) {
        self.change(|s| {
            if let Some(r) = s.get_mut(id) {
                r.policy.publish_hops = publish_hops.min(7);
                r.policy.backlog_hops = backlog_hops.min(7);
                r.policy.pass_on = pass_on;
            }
        });
    }

    /// Rename a neighborhood locally; the id, and everyone else's name for
    /// it, are unchanged.
    pub fn rename(&self, id: NeighborhoodId, name: &str) {
        let name = name.trim();
        // `nearby` keeps its name: it is the same for everyone.
        if name.is_empty() || id == super::nearby::id() {
            return;
        }
        self.change(|s| {
            if let Some(r) = s.get_mut(id) {
                r.name = name.to_string();
            }
        });
    }

    /// Forget a neighborhood. Returns whether it was known. Not `nearby`:
    /// that is turned off, not left ([`Neighbors::set_nearby`]).
    pub fn forget(&self, id: NeighborhoodId) -> bool {
        if id == super::nearby::id() {
            return false;
        }
        let known = self.change(|s| {
            let before = s.neighborhoods.len();
            s.neighborhoods.retain(|r| r.id != id);
            s.neighborhoods.len() != before
        });
        // Its content goes too, off this thread.
        if known {
            let handle = tokio::runtime::Handle::try_current()
                .ok()
                .or_else(|| self.rt.get().cloned());
            match handle {
                Some(handle) => {
                    let scoped = self.scoped.clone();
                    handle.spawn(async move { scoped.forget(id).await });
                }
                None => tracing::warn!("no runtime: a left neighborhood's content stays"),
            }
        }
        known
    }

    /// Set how far a neighborhood pushes uploaded blobs. The Neighbors tab's.
    pub fn set_blob_hops(&self, id: NeighborhoodId, hops: u8) {
        self.change(|s| {
            if let Some(r) = s.get_mut(id) {
                r.policy.blob_hops = hops;
            }
        });
    }

    /// Turn sharing (`share`) or taking (`!share`) an extension's updates
    /// on or off for a neighborhood. The two are independent.
    pub fn set_extension(&self, id: NeighborhoodId, extension: &str, share: bool, on: bool) {
        self.change(|s| {
            if let Some(r) = s.get_mut(id) {
                let off = if share {
                    &mut r.policy.share_off
                } else {
                    &mut r.policy.get_off
                };
                if on {
                    off.remove(extension);
                } else {
                    off.insert(extension.to_string());
                }
            }
        });
    }

    /// The neighborhoods on this device, for the shell's own UI.
    pub fn records(&self) -> Vec<Record> {
        self.store.lock().unwrap().neighborhoods.clone()
    }
}

/// Make the store's `nearby` record match the setting: there, first, while
/// on; gone while off. Returns whether anything changed.
fn keep_nearby(s: &mut Store) -> bool {
    let id = super::nearby::id();
    let here = s.get(id).is_some();
    if s.nearby_off && here {
        s.neighborhoods.retain(|r| r.id != id);
        true
    } else if !s.nearby_off && !here {
        s.neighborhoods.insert(0, nearby_record());
        true
    } else if let Some(r) = s
        .get_mut(id)
        .filter(|r| r.policy == store::Policy::default())
    {
        // Written before `nearby` had its own reach: the general default
        // then, never the user's choice.
        r.policy = nearby_record().policy;
        true
    } else {
        false
    }
}

/// The `nearby` neighborhood as the runtime starts it. Everyone in reach is
/// a stranger, so its reach is narrow: a publish goes to those in reach and
/// no further, and nothing from others is passed on. Extensions are shared
/// (what "Around you" shows is what devices in reach hold).
fn nearby_record() -> Record {
    let mut r = Record::new(super::nearby::id(), super::nearby::NAME, true);
    r.policy.publish_hops = 1;
    r.policy.pass_on = false;
    r
}

/// Whether `new` offers anything `old` doesn't.
fn widens(old: &Offer, new: &Offer) -> bool {
    let flags = |o: &Offer| {
        [
            o.relay_read,
            o.relay_write,
            o.relay_own,
            o.blob_read,
            o.blob_write,
            o.blob_own,
        ]
    };
    flags(old).iter().zip(flags(new)).any(|(o, n)| n && !o)
        || new.services.iter().any(|s| !old.services.contains(s))
}

#[async_trait]
impl NeighborHost for Neighbors {
    async fn info(&self) -> NeighborInfo {
        // `invite` is not offered to napplets: the shell itself does
        // invitations (over links).
        let mut features = vec![
            Feature::Create,
            Feature::Leave,
            Feature::Offer,
            Feature::Mute,
            Feature::NappletUpdates,
        ];
        if self.nearby_on() {
            features.push(Feature::Nearby);
        }
        NeighborInfo {
            features,
            services: vec!["relay".into(), "blossom".into()],
        }
    }

    async fn list(&self, napplet: &NappletIdentity) -> Vec<Neighborhood> {
        let key = napplet_key(napplet);
        let records: Vec<Record> = self.store.lock().unwrap().neighborhoods.clone();
        records.iter().map(|r| self.view(&key, r)).collect()
    }

    async fn peers(
        &self,
        napplet: &NappletIdentity,
        neighborhood: &str,
    ) -> Result<Vec<Peer>, NeighborError> {
        let key = napplet_key(napplet);
        let record = self.granted(&key, neighborhood)?;
        let nb_token = self.token_for(&key, record.id);
        Ok(self
            .peers
            .peers(&record)
            .into_iter()
            .map(|p| {
                let t = self.peer_token_for(&key, record.id, &p.key);
                Peer {
                    address: Some(address::format(&nb_token, Some(&t))),
                    id: t,
                    name: p.name,
                    reachable: p.reachable,
                    link: p.link,
                    services: services(&record.offer),
                }
            })
            .collect())
    }

    async fn create(
        &self,
        napplet: &NappletIdentity,
        request: CreateRequest,
    ) -> Result<Neighborhood, NeighborError> {
        let key = napplet_key(napplet);
        let name = request.name.trim().to_string();
        // `nearby` is the runtime's: present while the user has Nearby on.
        if request.auto_accept && NeighborhoodId::from_name(&name) == super::nearby::id() {
            return Err(NeighborError::Unsupported);
        }
        let ask = Ask::Create {
            napplet: key.clone(),
            name: name.clone(),
            auto_accept: request.auto_accept,
        };
        if !self.asker.confirm(ask).await {
            return Err(NeighborError::Cancelled);
        }
        let record = self.change(|s| {
            let id = if request.auto_accept {
                NeighborhoodId::from_name(&name)
            } else {
                NeighborhoodId::random()
            };
            if s.get(id).is_none() {
                s.neighborhoods
                    .push(Record::new(id, &name, request.auto_accept));
            }
            s.get(id).expect("just ensured").clone()
        });
        Ok(self.view(&key, &record))
    }

    async fn leave(
        &self,
        napplet: &NappletIdentity,
        neighborhood: &str,
    ) -> Result<(), NeighborError> {
        let key = napplet_key(napplet);
        let record = self.granted(&key, neighborhood)?;
        let ask = Ask::Leave {
            napplet: key,
            name: record.name.clone(),
        };
        if !self.asker.confirm(ask).await {
            return Err(NeighborError::Cancelled);
        }
        // Leaving `nearby` is turning Nearby off, as the shell's switch does
        // (the link service ends its links on its next tick).
        if record.id == super::nearby::id() {
            self.set_nearby(false);
            return Ok(());
        }
        self.change(|s| s.neighborhoods.retain(|r| r.id != record.id));
        self.forget_content(record.id).await;
        Ok(())
    }

    async fn set_offer(
        &self,
        napplet: &NappletIdentity,
        neighborhood: &str,
        offer: Offer,
    ) -> Result<Offer, NeighborError> {
        let key = napplet_key(napplet);
        let record = self.granted(&key, neighborhood)?;
        // What `nearby` offers is the runtime's, the same to everyone in reach.
        if record.id == super::nearby::id() {
            return Err(NeighborError::Unsupported);
        }
        if widens(&record.offer, &offer) {
            let ask = Ask::WidenOffer {
                napplet: key,
                name: record.name.clone(),
                offer: offer.clone(),
            };
            if !self.asker.confirm(ask).await {
                return Err(NeighborError::Cancelled);
            }
        }
        // Only what this runtime can carry stays in the offer.
        let mut offer = offer;
        offer.services.retain(|s| s == "relay" || s == "blossom");
        self.change(|s| {
            if let Some(r) = s.get_mut(record.id) {
                r.offer = offer.clone();
            }
        });
        Ok(offer)
    }

    async fn mute(
        &self,
        napplet: &NappletIdentity,
        neighborhood: &str,
        author: nostr::PublicKey,
        muted: bool,
    ) -> Result<(), NeighborError> {
        let key = napplet_key(napplet);
        let record = self.granted(&key, neighborhood)?;
        self.change(|s| {
            if let Some(r) = s.get_mut(record.id) {
                if muted {
                    r.muted.insert(author.to_hex());
                } else {
                    r.muted.remove(&author.to_hex());
                }
            }
        });
        Ok(())
    }
}

/// The services a peer offers here. Until offers travel over links, a peer
/// is assumed to offer what this device does.
fn services(offer: &Offer) -> Vec<String> {
    let mut out = Vec::new();
    if offer.relay_read || offer.relay_write {
        out.push("relay".to_string());
    }
    if offer.blob_read || offer.blob_write {
        out.push("blossom".to_string());
    }
    out
}

/// Who carries what on this device: each link carries the neighborhoods
/// agreed on it.
pub struct LinkPeers(pub Option<Arc<super::link::Links>>);

impl PeerDirectory for LinkPeers {
    fn peers(&self, neighborhood: &Record) -> Vec<PeerView> {
        let Some(links) = &self.0 else {
            return Vec::new();
        };
        // Reachable when the link was in contact lately.
        links
            .carrying(neighborhood.id)
            .into_iter()
            .map(|l| PeerView {
                reachable: links.reachable(&l.id),
                // A nearby peer's path is how it was last in reach.
                link: l
                    .ephemeral
                    .as_ref()
                    .and_then(|_| links.last_direct(&l.id))
                    .map(|(_, reach)| reach.link_class()),
                key: l.peer,
                name: l.name,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// The same peers in every neighborhood.
    struct Fixed(Vec<PeerView>);
    impl PeerDirectory for Fixed {
        fn peers(&self, _: &Record) -> Vec<PeerView> {
            self.0.clone()
        }
    }

    struct Answer(AtomicBool, Mutex<Vec<Ask>>);
    #[async_trait]
    impl Asker for Answer {
        async fn confirm(&self, ask: Ask) -> bool {
            self.1.lock().unwrap().push(ask);
            self.0.load(Ordering::Relaxed)
        }
    }

    fn seams(peers: Arc<dyn PeerDirectory>, asker: Arc<dyn Asker>) -> Seams {
        Seams {
            peers,
            asker,
            scoped_stores: Arc::new(crate::neighbor::data::MemStores::default()),
            scoped_blobs: Arc::new(nsite_deck::testing::MemBlobs::new()),
            links: None,
        }
    }

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("myco-neighbors-{}", super::super::id::token()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn open(yes: bool) -> (Neighbors, Arc<Answer>, PathBuf) {
        let d = dir();
        let asker = Arc::new(Answer(AtomicBool::new(yes), Mutex::new(Vec::new())));
        let peers = Arc::new(Fixed(vec![
            PeerView {
                key: "npub1bob".into(),
                name: Some("Bob".into()),
                reachable: true,
                link: None,
            },
            PeerView {
                key: "npub1carol".into(),
                name: None,
                reachable: false,
                link: None,
            },
        ]));
        let n = Neighbors::open(&d, seams(peers, asker.clone()));
        // Most tests are about neighborhoods the user makes.
        n.set_nearby(false);
        (n, asker, d)
    }

    fn app(d: &str) -> NappletIdentity {
        let mut id = NappletIdentity::new(d, "hash");
        id.author = "a".repeat(64);
        id
    }

    fn new(name: &str, auto_accept: bool) -> CreateRequest {
        CreateRequest {
            name: name.into(),
            auto_accept,
        }
    }

    /// `napplet` starts a neighborhood called `name`; the prompt it took is
    /// forgotten.
    async fn club(n: &Neighbors, asker: &Answer, napplet: &str, name: &str) -> Neighborhood {
        let nb = n.create(&app(napplet), new(name, false)).await.unwrap();
        asker.1.lock().unwrap().clear();
        nb
    }

    #[tokio::test]
    async fn a_neighborhood_is_listed_to_its_napplet_with_its_peers() {
        let (n, asker, _) = open(true);
        club(&n, &asker, "chat", "Club").await;
        let list = n.list(&app("chat")).await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Club");
        assert_eq!((list[0].peers, list[0].reachable), (2, 1));
        let peers = n.peers(&app("chat"), &list[0].id).await.unwrap();
        assert_eq!(peers[0].name.as_deref(), Some("Bob"));
        assert!(peers[0]
            .address
            .as_deref()
            .unwrap()
            .starts_with("neighborhood://"));
    }

    #[tokio::test]
    async fn no_neighborhood_until_one_is_made() {
        let (n, _, _) = open(true);
        assert!(n.records().is_empty());
        assert!(n.list(&app("chat")).await.is_empty());
    }

    #[tokio::test]
    async fn an_old_everyone_is_dropped_on_open() {
        let d = dir();
        let mut old = Record::new(NeighborhoodId::random(), "Everyone", false);
        old.role = store::Role::Everyone;
        let club = Record::new(NeighborhoodId::random(), "Club", false);
        let s = Store {
            neighborhoods: vec![old, club.clone()],
            nearby_off: true,
            ..Store::default()
        };
        store::save(&store::path(&d), &s).unwrap();
        let asker = Arc::new(Answer(AtomicBool::new(true), Mutex::new(Vec::new())));
        let n = Neighbors::open(&d, seams(Arc::new(Fixed(Vec::new())), asker));
        assert_eq!(n.records(), std::slice::from_ref(&club));
        assert_eq!(
            store::load(&store::path(&d)).neighborhoods,
            [club],
            "and saved without it"
        );
    }

    #[tokio::test]
    async fn ids_are_per_napplet_and_stable_across_reopen() {
        let (n, asker, d) = open(true);
        n.create(&app("chat"), new("Lowlands", true)).await.unwrap();
        n.create(&app("map"), new("Lowlands", true)).await.unwrap();
        let a = n.list(&app("chat")).await[0].id.clone();
        let b = n.list(&app("map")).await[0].id.clone();
        assert_ne!(a, b, "two napplets cannot correlate ids");
        assert_eq!(n.list(&app("chat")).await[0].id, a);
        let again = Neighbors::open(&d, seams(Arc::new(Fixed(Vec::new())), asker));
        assert_eq!(again.list(&app("chat")).await[0].id, a);
        assert_eq!(again.records().len(), 1);
    }

    #[tokio::test]
    async fn another_napplets_id_and_address_mean_nothing() {
        let (n, asker, _) = open(true);
        let theirs = club(&n, &asker, "chat", "Club").await;
        assert_eq!(
            n.peers(&app("map"), &theirs.id).await.unwrap_err(),
            NeighborError::NotFound
        );
        let addr = theirs.address.unwrap();
        assert!(n.resolve(&app("chat"), &addr).is_some());
        assert!(n.resolve(&app("map"), &addr).is_none());
    }

    #[tokio::test]
    async fn a_peer_address_resolves_to_that_peer() {
        let (n, asker, _) = open(true);
        let nb = club(&n, &asker, "chat", "Club").await;
        let bob = n.peers(&app("chat"), &nb.id).await.unwrap().remove(0);
        let t = n
            .resolve(&app("chat"), bob.address.as_deref().unwrap())
            .unwrap();
        assert_eq!(t.peer.as_deref(), Some("npub1bob"));
        // A peer token under another neighborhood's token resolves to nothing.
        let other = club(&n, &asker, "chat", "Other").await;
        let mixed = address::format(
            &address::parse(other.address.as_deref().unwrap())
                .unwrap()
                .neighborhood,
            Some(&bob.id),
        );
        assert!(n.resolve(&app("chat"), &mixed).is_none());
    }

    #[tokio::test]
    async fn create_asks_and_every_napplet_sees_it() {
        let (n, asker, _) = open(true);
        let club = n.create(&app("chat"), new(" Club ", false)).await.unwrap();
        assert_eq!(club.name, "Club");
        assert!(matches!(asker.1.lock().unwrap()[0], Ask::Create { .. }));
        assert_eq!(n.list(&app("chat")).await.len(), 1);
        let theirs = n.list(&app("map")).await;
        assert_eq!(theirs.len(), 1, "map uses Club too");
        assert_ne!(theirs[0].id, club.id, "under its own token");
    }

    #[tokio::test]
    async fn new_neighborhoods_with_one_name_stay_apart_but_auto_accepted_ones_meet() {
        let (n, _, _) = open(true);
        n.create(&app("chat"), new("Home", false)).await.unwrap();
        n.create(&app("chat"), new("Home", false)).await.unwrap();
        n.create(&app("chat"), new("Lowlands", true)).await.unwrap();
        n.create(&app("map"), new("lowlands", true)).await.unwrap();
        let names: Vec<_> = n.records().into_iter().map(|r| r.name).collect();
        assert_eq!(
            names,
            ["Home", "Home", "Lowlands"],
            "map joined the same Lowlands"
        );
    }

    #[tokio::test]
    async fn a_declined_prompt_changes_nothing() {
        let (n, _, _) = open(false);
        let r = n.create(&app("chat"), new("Club", false)).await;
        assert_eq!(r.unwrap_err(), NeighborError::Cancelled);
        assert!(n.records().is_empty());
    }

    #[tokio::test]
    async fn leaving_asks_and_forgets() {
        let (n, asker, _) = open(true);
        let nb = club(&n, &asker, "chat", "Club").await;
        n.leave(&app("chat"), &nb.id).await.unwrap();
        assert!(matches!(asker.1.lock().unwrap()[0], Ask::Leave { .. }));
        assert!(n.records().is_empty());
    }

    #[tokio::test]
    async fn narrowing_an_offer_needs_no_prompt_but_widening_does() {
        let (n, asker, _) = open(true);
        let e = club(&n, &asker, "chat", "Club").await;
        let narrow = Offer {
            relay_read: true,
            ..Offer::default()
        };
        n.set_offer(&app("chat"), &e.id, narrow.clone())
            .await
            .unwrap();
        assert!(asker.1.lock().unwrap().is_empty());
        let wide = Offer {
            blob_own: true,
            services: vec!["relay".into(), "voice".into()],
            ..narrow
        };
        let got = n.set_offer(&app("chat"), &e.id, wide).await.unwrap();
        assert_eq!(asker.1.lock().unwrap().len(), 1);
        assert_eq!(got.services, ["relay"], "unknown services dropped");
    }

    #[tokio::test]
    async fn mute_is_kept_per_neighborhood() {
        let (n, asker, _) = open(true);
        let e = club(&n, &asker, "chat", "Club").await;
        club(&n, &asker, "chat", "Other").await;
        let author = nostr::Keys::generate().public_key();
        n.mute(&app("chat"), &e.id, author, true).await.unwrap();
        assert!(n.records()[0].muted.contains(&author.to_hex()));
        assert!(n.records()[1].muted.is_empty());
        n.mute(&app("chat"), &e.id, author, false).await.unwrap();
        assert!(n.records()[0].muted.is_empty());
    }

    #[tokio::test]
    async fn blob_hops_are_kept_per_neighborhood() {
        let (n, asker, _) = open(true);
        club(&n, &asker, "chat", "Club").await;
        let id = n.records()[0].id;
        assert_eq!(n.records()[0].policy.blob_hops, 0, "0 by default");
        n.set_blob_hops(id, 2);
        assert_eq!(n.records()[0].policy.blob_hops, 2);
    }

    #[tokio::test]
    async fn nearby_is_there_by_default_for_existing_installs_too() {
        let d = dir();
        // A file written before Nearby existed.
        let club = Record::new(NeighborhoodId::random(), "Club", false);
        let old = serde_json::json!({ "neighborhoods": [club] });
        std::fs::write(store::path(&d), old.to_string()).unwrap();
        let asker = Arc::new(Answer(AtomicBool::new(true), Mutex::new(Vec::new())));
        let n = Neighbors::open(&d, seams(Arc::new(Fixed(Vec::new())), asker));
        assert!(n.nearby_on());
        let records = n.records();
        assert_eq!(records[0].id, super::super::nearby::id());
        assert_eq!(records[0].name, "nearby");
        assert!(records[0].auto_accept);
        assert_eq!(records[1], club);
        assert!(n.info().await.features.contains(&Feature::Nearby));
        let listed = n.list(&app("chat")).await;
        assert_eq!(listed[0].name, "nearby", "every napplet sees it");
    }

    #[tokio::test]
    async fn turning_nearby_off_forgets_it_and_the_setting_is_kept() {
        let (n, _, d) = open(true);
        n.set_nearby(true);
        assert_eq!(n.records().len(), 1);
        n.set_nearby(false);
        assert!(n.records().is_empty());
        assert!(!n.info().await.features.contains(&Feature::Nearby));
        let asker = Arc::new(Answer(AtomicBool::new(true), Mutex::new(Vec::new())));
        let again = Neighbors::open(&d, seams(Arc::new(Fixed(Vec::new())), asker));
        assert!(!again.nearby_on(), "off survives a restart");
        assert!(again.records().is_empty());
        // Joining it by name from the shell doesn't bring it back.
        again.create_local("Nearby", true);
        assert!(again.records().is_empty());
        again.set_nearby(true);
        assert_eq!(again.records()[0].id, super::super::nearby::id());
    }

    #[tokio::test]
    async fn a_napplet_cannot_create_invite_or_set_the_offer_of_nearby() {
        let (n, asker, _) = open(true);
        n.set_nearby(true);
        let nearby = n.list(&app("chat")).await.remove(0);
        assert_eq!(nearby.name, "nearby");
        assert_eq!(
            n.create(&app("chat"), new(" Nearby ", true))
                .await
                .unwrap_err(),
            NeighborError::Unsupported
        );
        assert_eq!(
            n.invite(&app("chat"), &nearby.id, Vec::new())
                .await
                .unwrap_err(),
            NeighborError::Unsupported
        );
        assert_eq!(
            n.set_offer(&app("chat"), &nearby.id, Offer::default())
                .await
                .unwrap_err(),
            NeighborError::Unsupported
        );
        assert!(asker.1.lock().unwrap().is_empty(), "nobody was asked");
        assert!(n.nearby_on());
        // Nor the shell's leave or rename.
        assert!(!n.forget(super::super::nearby::id()));
        n.rename(super::super::nearby::id(), "Party");
        assert_eq!(n.records()[0].name, "nearby");
        // A neighborhood the user made called "nearby" is just a neighborhood.
        assert!(n.create(&app("chat"), new("nearby", false)).await.is_ok());
    }

    #[tokio::test]
    async fn a_napplet_leaving_nearby_turns_it_off_once_the_user_agrees() {
        let (n, asker, _) = open(false);
        n.set_nearby(true);
        let nearby = n.list(&app("chat")).await.remove(0);
        assert_eq!(
            n.leave(&app("chat"), &nearby.id).await.unwrap_err(),
            NeighborError::Cancelled
        );
        assert!(n.nearby_on(), "declined: still on");
        asker.0.store(true, Ordering::Relaxed);
        n.leave(&app("chat"), &nearby.id).await.unwrap();
        assert!(matches!(
            asker.1.lock().unwrap().last(),
            Some(Ask::Leave { name, .. }) if name == "nearby"
        ));
        assert!(!n.nearby_on());
        assert!(n.records().is_empty());
    }

    #[tokio::test]
    async fn nearby_reaches_only_those_in_reach_and_passes_nothing_on() {
        let (n, _, d) = open(true);
        n.set_nearby(true);
        let policy = n.records()[0].policy.clone();
        assert_eq!((policy.publish_hops, policy.pass_on), (1, false));
        assert!(policy.shares(super::super::extensions::NAPPLET_UPDATES));
        // A record written with the general default is narrowed on open.
        let mut s = store::load(&store::path(&d));
        s.neighborhoods[0].policy = store::Policy::default();
        store::save(&store::path(&d), &s).unwrap();
        let asker = Arc::new(Answer(AtomicBool::new(true), Mutex::new(Vec::new())));
        let again = Neighbors::open(&d, seams(Arc::new(Fixed(Vec::new())), asker));
        assert_eq!(again.records()[0].policy, policy);
    }
}
