//! [`Neighbors`]: this device's [`NeighborHost`].

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use myco_napplet_runtime::nap::neighbor::{
    address, CreateRequest, Feature, NeighborError, NeighborHost, NeighborInfo, Neighborhood,
    Offer, Peer,
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
}

/// What [`Neighbors::open`] reaches the world through.
pub struct Seams {
    pub peers: Arc<dyn PeerDirectory>,
    pub asker: Arc<dyn Asker>,
    /// Where neighborhood events are stored.
    pub scoped_store: Arc<dyn nsite_deck::RelayBackend>,
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
    /// neighborhood an older version kept for the Circle.
    pub fn open(data_dir: &std::path::Path, seams: Seams) -> Self {
        let file = store::path(data_dir);
        let mut s = store::load(&file);
        if s.drop_everyone() {
            if let Err(e) = store::save(&file, &s) {
                tracing::warn!(error = %e, "could not save neighborhoods");
            }
        }
        Self {
            file,
            store: Mutex::new(s),
            peers: seams.peers,
            asker: seams.asker,
            scoped: Arc::new(super::data::Scoped::open(
                data_dir,
                seams.scoped_store,
                seams.scoped_blobs,
            )),
            links: seams.links,
        }
    }

    /// Run `f` on the store and save it.
    fn change<T>(&self, f: impl FnOnce(&mut Store) -> T) -> T {
        let mut s = self.store.lock().unwrap();
        let out = f(&mut s);
        if let Err(e) = store::save(&self.file, &s) {
            tracing::warn!(error = %e, "could not save neighborhoods");
        }
        out
    }

    /// The neighborhood a napplet's id names, if it was granted it.
    fn granted(&self, napplet: &str, id_token: &str) -> Result<Record, NeighborError> {
        let s = self.store.lock().unwrap();
        let id = s
            .tokens
            .get(napplet)
            .and_then(|t| t.neighborhoods.get(id_token))
            .copied()
            .ok_or(NeighborError::NotFound)?;
        s.get(id)
            .filter(|r| r.grants.allows(napplet))
            .cloned()
            .ok_or(NeighborError::NotFound)
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
        s.get(id).filter(|r| r.grants.allows(&key))?;
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
        if name.is_empty() {
            return;
        }
        self.change(|s| {
            if let Some(r) = s.get_mut(id) {
                r.name = name.to_string();
            }
        });
    }

    /// Forget a neighborhood. Returns whether it was known.
    pub fn forget(&self, id: NeighborhoodId) -> bool {
        self.change(|s| {
            let before = s.neighborhoods.len();
            s.neighborhoods.retain(|r| r.id != id);
            s.neighborhoods.len() != before
        })
    }

    /// Set how far a neighborhood pushes uploaded blobs. The Neighbors tab's.
    pub fn set_blob_hops(&self, id: NeighborhoodId, hops: u8) {
        self.change(|s| {
            if let Some(r) = s.get_mut(id) {
                r.policy.blob_hops = hops;
            }
        });
    }

    /// Grant a neighborhood to a napplet, for tests.
    #[cfg(test)]
    pub fn grant_for_test(&self, id: NeighborhoodId, napplet: &NappletIdentity) {
        let key = napplet_key(napplet);
        self.change(|s| {
            if let Some(r) = s.get_mut(id) {
                r.grants.napplets.insert(key);
            }
        });
    }

    /// The neighborhoods on this device, for the shell's own UI.
    pub fn records(&self) -> Vec<Record> {
        self.store.lock().unwrap().neighborhoods.clone()
    }
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
        NeighborInfo {
            // `invite` needs links (to reach anyone new); `nearby` needs the
            // proximity extension. Neither exists yet.
            features: vec![
                Feature::Create,
                Feature::Leave,
                Feature::Offer,
                Feature::Mute,
            ],
            services: vec!["relay".into(), "blossom".into()],
        }
    }

    async fn list(&self, napplet: &NappletIdentity) -> Vec<Neighborhood> {
        let key = napplet_key(napplet);
        let records: Vec<Record> = self
            .store
            .lock()
            .unwrap()
            .neighborhoods
            .iter()
            .filter(|r| r.grants.allows(&key))
            .cloned()
            .collect();
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
                    link: None,
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
            let r = s.get_mut(id).expect("just ensured");
            r.grants.napplets.insert(key.clone());
            r.clone()
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
        self.change(|s| s.neighborhoods.retain(|r| r.id != record.id));
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
            scoped_store: Arc::new(nsite_deck::testing::MemRelay::new()),
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
            },
            PeerView {
                key: "npub1carol".into(),
                name: None,
                reachable: false,
            },
        ]));
        (Neighbors::open(&d, seams(peers, asker.clone())), asker, d)
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
        old.grants.all = true;
        let club = Record::new(NeighborhoodId::random(), "Club", false);
        let s = Store {
            neighborhoods: vec![old, club.clone()],
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
    async fn create_asks_and_grants_only_the_caller() {
        let (n, asker, _) = open(true);
        let club = n.create(&app("chat"), new(" Club ", false)).await.unwrap();
        assert_eq!(club.name, "Club");
        assert!(matches!(asker.1.lock().unwrap()[0], Ask::Create { .. }));
        assert_eq!(n.list(&app("chat")).await.len(), 1);
        assert!(
            n.list(&app("map")).await.is_empty(),
            "map wasn't granted Club"
        );
    }

    #[tokio::test]
    async fn new_neighborhoods_with_one_name_stay_apart_but_auto_accepted_ones_meet() {
        let (n, _, _) = open(true);
        n.create(&app("chat"), new("Home", false)).await.unwrap();
        n.create(&app("chat"), new("Home", false)).await.unwrap();
        n.create(&app("chat"), new("Lowlands", true)).await.unwrap();
        n.create(&app("map"), new("lowlands", true)).await.unwrap();
        let names: Vec<_> = n.records().into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["Home", "Home", "Lowlands"]);
        assert_eq!(
            n.list(&app("map")).await.len(),
            1,
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
}
