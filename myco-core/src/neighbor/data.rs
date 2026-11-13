//! The data plane: what `relay`, `outbox` and `resource` reach through an
//! address.
//!
//! **Everyone** rides the Circle's mesh ([`Carrier`]): it is the Circle, and
//! its view is what the Circle sees today — the device's store and what peers
//! bring.
//!
//! **Every other neighborhood** has its own store ([`Scoped`]), so nothing
//! published into one lands in the user's own relay, in Everyone, or in
//! another neighborhood. One copy per event, with the set of neighborhoods it
//! belongs to beside it. Until links exist these neighborhoods have no peers:
//! what is published stays here, and a subscription sees it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use myco_napplet_runtime::nap::neighbor::{Feed, NeighborData, NeighborError};
use myco_napplet_runtime::NappletIdentity;
use nostr::{Event, Filter};
use nsite_deck::RelayBackend;
use tokio::sync::broadcast;

use super::host::{Neighbors, Target};
use super::id::NeighborhoodId;
use super::store::{Record, Role};

/// Neighborhood events: their store, which neighborhood each belongs to,
/// and a live feed of new ones.
pub struct Scoped {
    pub store: Arc<dyn RelayBackend>,
    pub blobs: Arc<dyn nsite_deck::BlobStore>,
    file: PathBuf,
    /// Event id (hex), or `blob:<sha256>` → the neighborhoods it was
    /// published or received in.
    scopes: Mutex<BTreeMap<String, BTreeSet<NeighborhoodId>>>,
    live: broadcast::Sender<(NeighborhoodId, Event)>,
}

/// The scope table, under the data dir.
pub const SCOPES_FILE: &str = "neighborhood-scopes.json";

impl Scoped {
    pub fn open(
        data_dir: &Path,
        store: Arc<dyn RelayBackend>,
        blobs: Arc<dyn nsite_deck::BlobStore>,
    ) -> Self {
        let file = data_dir.join(SCOPES_FILE);
        let scopes = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            store,
            blobs,
            file,
            scopes: Mutex::new(scopes),
            live: broadcast::channel(256).0,
        }
    }

    async fn add(&self, id: NeighborhoodId, event: Event) -> anyhow::Result<()> {
        self.store.publish(event.clone()).await?;
        self.scope(id, event.id.to_hex())?;
        let _ = self.live.send((id, event));
        Ok(())
    }

    async fn add_blob(&self, id: NeighborhoodId, bytes: &[u8], sha256: &str) -> anyhow::Result<()> {
        self.blobs.put(bytes).await?;
        self.scope(id, format!("blob:{sha256}"))
    }

    async fn blob(&self, id: NeighborhoodId, sha256: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let scoped = self
            .scopes
            .lock()
            .unwrap()
            .get(&format!("blob:{sha256}"))
            .is_some_and(|s| s.contains(&id));
        if !scoped {
            return Ok(None);
        }
        self.blobs.get(sha256).await
    }

    fn scope(&self, id: NeighborhoodId, key: String) -> anyhow::Result<()> {
        let mut scopes = self.scopes.lock().unwrap();
        scopes.entry(key).or_default().insert(id);
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&*scopes)?)?;
        std::fs::rename(&tmp, &self.file)?;
        Ok(())
    }

    fn holds(&self, id: NeighborhoodId, event: &Event) -> bool {
        self.scopes
            .lock()
            .unwrap()
            .get(&event.id.to_hex())
            .is_some_and(|s| s.contains(&id))
    }

    async fn backlog(&self, id: NeighborhoodId, filters: &[Filter]) -> anyhow::Result<Vec<Event>> {
        Ok(self
            .store
            .query(filters)
            .await?
            .into_iter()
            .filter(|e| self.holds(id, e))
            .collect())
    }

    fn feed(&self, id: NeighborhoodId) -> myco_napplet_runtime::seams::PulledEvents {
        let rx = self.live.subscribe();
        Box::pin(futures_util::stream::unfold(rx, move |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok((n, event)) if n == id => return Some((event, rx)),
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        }))
    }
}

/// A blob store that keeps nothing: what neighborhood uploads get when their
/// store could not be opened. Uploads then fail rather than land elsewhere.
pub struct NoBlobs;

#[async_trait]
impl nsite_deck::BlobStore for NoBlobs {
    async fn has(&self, _sha256_hex: &str) -> bool {
        false
    }
    async fn get(&self, _sha256_hex: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    async fn put(&self, _bytes: &[u8]) -> anyhow::Result<String> {
        anyhow::bail!("no neighborhood blob store on this device")
    }
    async fn wipe(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl Neighbors {
    fn target(
        &self,
        napplet: &NappletIdentity,
        address: &str,
    ) -> Result<(Target, Record), NeighborError> {
        let target = self
            .resolve(napplet, address)
            .ok_or(NeighborError::NotFound)?;
        let record = self
            .records()
            .into_iter()
            .find(|r| r.id == target.neighborhood)
            .ok_or(NeighborError::NotFound)?;
        Ok((target, record))
    }
}

/// Drop events by muted authors.
fn unmuted(record: &Record) -> impl Fn(&Event) -> bool + Send + Sync + 'static {
    let muted = record.muted.clone();
    move |e: &Event| !muted.contains(&e.pubkey.to_hex())
}

fn failed(e: anyhow::Error) -> NeighborError {
    tracing::warn!(error = %e, "neighborhood carriage failed");
    NeighborError::Denied
}

#[async_trait]
impl NeighborData for Neighbors {
    async fn publish(
        &self,
        napplet: &NappletIdentity,
        address: &str,
        event: Event,
    ) -> Result<(), NeighborError> {
        let (target, record) = self.target(napplet, address)?;
        match (record.role, target.peer) {
            (Role::Everyone, Some(peer)) => self.carrier.send_to(&peer, event).await,
            (Role::Everyone, None) => {
                self.carrier
                    .publish(event, record.policy.publish_hops)
                    .await
            }
            // No links yet, so no peers to send to: kept in its scope here.
            (Role::Plain, Some(_)) => return Err(NeighborError::NotFound),
            (Role::Plain, None) => self.scoped.add(record.id, event).await,
        }
        .map_err(failed)
    }

    async fn subscribe(
        &self,
        napplet: &NappletIdentity,
        address: &str,
        filters: Vec<Filter>,
    ) -> Result<Feed, NeighborError> {
        use futures_util::StreamExt as _;
        let (target, record) = self.target(napplet, address)?;
        let keep = unmuted(&record);
        match (record.role, target.peer) {
            (Role::Everyone, None) => {
                let backlog = self.carrier.local(&filters).await.map_err(failed)?;
                let pulled = self
                    .carrier
                    .pull(&filters, record.policy.backlog_hops)
                    .await
                    .map_err(failed)?;
                let live = futures_util::stream::select(pulled, self.carrier.live());
                Ok(Feed {
                    backlog: backlog.into_iter().filter(|e| keep(e)).collect(),
                    live: Box::pin(live.filter(move |e| std::future::ready(keep(e)))),
                })
            }
            (Role::Plain, None) => {
                let backlog = self
                    .scoped
                    .backlog(record.id, &filters)
                    .await
                    .map_err(failed)?;
                let live = self.scoped.feed(record.id);
                Ok(Feed {
                    backlog: backlog.into_iter().filter(|e| keep(e)).collect(),
                    live: Box::pin(live.filter(move |e| std::future::ready(keep(e)))),
                })
            }
            // Reading one peer's view needs a link to that peer.
            (_, Some(_)) => Err(NeighborError::Unsupported),
        }
    }

    async fn blob(
        &self,
        napplet: &NappletIdentity,
        address: &str,
        sha256: &str,
    ) -> Result<Option<Vec<u8>>, NeighborError> {
        let (target, record) = self.target(napplet, address)?;
        match (record.role, target.peer) {
            (Role::Everyone, peer) => Ok(self.carrier.blob(sha256, peer.as_deref()).await),
            (Role::Plain, None) => self.scoped.blob(record.id, sha256).await.map_err(failed),
            (Role::Plain, Some(_)) => Err(NeighborError::NotFound),
        }
    }

    /// Kept here in the neighborhood's scope, then pushed `blob_hops` out
    /// (0 by default: peers fetch it when they want it).
    async fn upload(
        &self,
        napplet: &NappletIdentity,
        address: &str,
        bytes: Vec<u8>,
        sha256: &str,
    ) -> Result<(), NeighborError> {
        let (target, record) = self.target(napplet, address)?;
        if target.peer.is_some() {
            return Err(NeighborError::NotFound);
        }
        let hops = record.policy.blob_hops;
        match record.role {
            Role::Everyone => {
                self.carrier.keep_blob(&bytes).await.map_err(failed)?;
                if hops > 0 {
                    let took = self.carrier.push_blob(&bytes, hops).await;
                    tracing::info!(sha256, hops, took, "neighborhood blob pushed");
                }
            }
            // No links yet, so nobody to push to: kept here.
            Role::Plain => self
                .scoped
                .add_blob(record.id, &bytes, sha256)
                .await
                .map_err(failed)?,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::carrier::testing::MemCarrier;
    use super::super::host::{Ask, Asker, PeerDirectory, PeerView, Seams};
    use super::*;
    use myco_napplet_runtime::nap::neighbor::{CreateRequest, NeighborHost};

    struct Yes;
    #[async_trait]
    impl Asker for Yes {
        async fn confirm(&self, _: Ask) -> bool {
            true
        }
    }

    struct Bob;
    impl PeerDirectory for Bob {
        fn peers(&self, r: &Record) -> Vec<PeerView> {
            if r.role != Role::Everyone {
                return Vec::new();
            }
            vec![PeerView {
                key: "npub1bob".into(),
                name: None,
                reachable: true,
            }]
        }
    }

    fn open() -> (Neighbors, Arc<MemCarrier>) {
        let d = std::env::temp_dir().join(format!("myco-nb-data-{}", super::super::id::token()));
        std::fs::create_dir_all(&d).unwrap();
        let carrier = Arc::new(MemCarrier::default());
        let n = Neighbors::open(
            &d,
            Seams {
                peers: Arc::new(Bob),
                asker: Arc::new(Yes),
                carrier: carrier.clone(),
                scoped_store: Arc::new(nsite_deck::testing::MemRelay::new()),
                scoped_blobs: Arc::new(nsite_deck::testing::MemBlobs::new()),
            },
        );
        (n, carrier)
    }

    fn app() -> NappletIdentity {
        NappletIdentity::new("chat", "hash")
    }

    fn note(text: &str) -> Event {
        nostr::EventBuilder::text_note(text)
            .sign_with_keys(&nostr::Keys::generate())
            .unwrap()
    }

    fn kind1() -> Vec<Filter> {
        vec![Filter::new().kind(nostr::Kind::TextNote)]
    }

    async fn addresses(n: &Neighbors) -> (String, String) {
        let everyone = n.list(&app()).await.remove(0).address.unwrap();
        let club = n
            .create(
                &app(),
                CreateRequest {
                    name: "Club".into(),
                    auto_accept: false,
                },
            )
            .await
            .unwrap()
            .address
            .unwrap();
        (everyone, club)
    }

    #[tokio::test]
    async fn everyone_publishes_over_the_mesh_with_its_hops() {
        let (n, carrier) = open();
        let (everyone, _) = addresses(&n).await;
        n.publish(&app(), &everyone, note("hi all")).await.unwrap();
        assert_eq!(carrier.flooded.lock().unwrap()[0].1, 3);
    }

    #[tokio::test]
    async fn a_neighborhood_keeps_its_events_apart() {
        let (n, carrier) = open();
        let (everyone, club) = addresses(&n).await;
        n.publish(&app(), &club, note("club only")).await.unwrap();
        n.publish(&app(), &everyone, note("for everyone"))
            .await
            .unwrap();

        let club_feed = n.subscribe(&app(), &club, kind1()).await.unwrap();
        let texts: Vec<_> = club_feed
            .backlog
            .iter()
            .map(|e| e.content.clone())
            .collect();
        assert_eq!(texts, ["club only"]);

        let all = n.subscribe(&app(), &everyone, kind1()).await.unwrap();
        let texts: Vec<_> = all.backlog.iter().map(|e| e.content.clone()).collect();
        assert_eq!(
            texts,
            ["for everyone"],
            "the club's event never reached the device store"
        );
        assert_eq!(
            carrier.flooded.lock().unwrap().len(),
            1,
            "the club's was not flooded"
        );
    }

    #[tokio::test]
    async fn two_neighborhoods_do_not_see_each_other() {
        let (n, _) = open();
        let (_, club) = addresses(&n).await;
        let other = n
            .create(
                &app(),
                CreateRequest {
                    name: "Other".into(),
                    auto_accept: false,
                },
            )
            .await
            .unwrap()
            .address
            .unwrap();
        n.publish(&app(), &club, note("club")).await.unwrap();
        assert!(n
            .subscribe(&app(), &other, kind1())
            .await
            .unwrap()
            .backlog
            .is_empty());
    }

    #[tokio::test]
    async fn new_events_reach_a_live_subscription_in_their_scope_only() {
        use futures_util::StreamExt as _;
        let (n, _) = open();
        let (_, club) = addresses(&n).await;
        let mut feed = n.subscribe(&app(), &club, kind1()).await.unwrap();
        n.publish(&app(), &club, note("live")).await.unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(1), feed.live.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.content, "live");
    }

    #[tokio::test]
    async fn muted_authors_are_dropped() {
        let (n, carrier) = open();
        let (everyone, _) = addresses(&n).await;
        let loud = note("loud");
        carrier.device.lock().unwrap().push(loud.clone());
        let id = n.list(&app()).await.remove(0).id;
        n.mute(&app(), &id, loud.pubkey, true).await.unwrap();
        assert!(n
            .subscribe(&app(), &everyone, kind1())
            .await
            .unwrap()
            .backlog
            .is_empty());
    }

    #[tokio::test]
    async fn a_peer_address_sends_to_that_peer_only() {
        let (n, carrier) = open();
        let id = n.list(&app()).await.remove(0).id;
        let bob = n.peers(&app(), &id).await.unwrap().remove(0);
        n.publish(&app(), bob.address.as_deref().unwrap(), note("psst"))
            .await
            .unwrap();
        assert_eq!(carrier.sent.lock().unwrap()[0].0, "npub1bob");
        assert!(carrier.flooded.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn everyone_blobs_come_from_the_mesh_and_stay_out_of_other_neighborhoods() {
        let (n, carrier) = open();
        let (everyone, club) = addresses(&n).await;
        carrier.blobs.lock().unwrap().insert("ab".into(), vec![1]);
        assert_eq!(
            n.blob(&app(), &everyone, "ab").await.unwrap(),
            Some(vec![1])
        );
        assert_eq!(n.blob(&app(), &club, "ab").await.unwrap(), None);
    }

    #[tokio::test]
    async fn an_upload_is_kept_in_its_scope_only() {
        let (n, carrier) = open();
        let (everyone, club) = addresses(&n).await;
        let bytes = b"club picture".to_vec();
        let sha = nsite_deck::sha256_hex(&bytes);
        n.upload(&app(), &club, bytes.clone(), &sha).await.unwrap();
        assert_eq!(n.blob(&app(), &club, &sha).await.unwrap(), Some(bytes));
        assert_eq!(n.blob(&app(), &everyone, &sha).await.unwrap(), None);
        assert!(carrier.pushed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn blobs_are_pushed_only_when_the_neighborhood_says_so() {
        let (n, carrier) = open();
        let (everyone, _) = addresses(&n).await;
        let bytes = b"photo".to_vec();
        let sha = nsite_deck::sha256_hex(&bytes);
        n.upload(&app(), &everyone, bytes.clone(), &sha)
            .await
            .unwrap();
        assert!(
            carrier.pushed.lock().unwrap().is_empty(),
            "0 hops by default"
        );

        n.set_blob_hops(n.records()[0].id, 2);
        n.upload(&app(), &everyone, bytes, &sha).await.unwrap();
        assert_eq!(*carrier.pushed.lock().unwrap(), [2]);
    }

    #[tokio::test]
    async fn another_napplets_address_is_refused() {
        let (n, _) = open();
        let (everyone, _) = addresses(&n).await;
        let mut other = app();
        other.d_tag = "map".into();
        assert_eq!(
            n.publish(&other, &everyone, note("x")).await.unwrap_err(),
            NeighborError::NotFound
        );
    }
}
