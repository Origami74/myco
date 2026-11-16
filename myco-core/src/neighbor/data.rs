//! The data plane: what `relay`, `outbox` and `resource` reach through an
//! address.
//!
//! Every neighborhood has its own store ([`Scoped`]), so nothing published
//! into one lands in the user's own relay or in another neighborhood. One
//! copy per event, with the set of neighborhoods it belongs to beside it.
//! Links carry it to peers: a publish is forwarded on every link carrying the
//! neighborhood, a subscription pulls from them, and a blob missing here is
//! asked of them (`GET /nh/{pk}/blob/{sha256}?n=`), verified and kept.

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
use super::store::Record;

/// Neighborhood events: their store, which neighborhood each belongs to,
/// and a live feed of new ones.
pub struct Scoped {
    pub store: Arc<dyn RelayBackend>,
    pub blobs: Arc<dyn nsite_deck::BlobStore>,
    /// The device's own kept blobs, for files an extension shares without
    /// copying them (see [`super::updates`]). Set once by the runtime.
    pub kept: std::sync::OnceLock<Arc<dyn nsite_deck::BlobStore>>,
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
            kept: std::sync::OnceLock::new(),
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
        if let Some(bytes) = self.blobs.get(sha256).await? {
            return Ok(Some(bytes));
        }
        match self.kept.get() {
            Some(kept) => kept.get(sha256).await,
            None => Ok(None),
        }
    }

    /// Put files already kept on the device into neighborhood `id`'s scope,
    /// without copying them: they are read from [`Scoped::kept`].
    pub(super) fn share_kept(&self, id: NeighborhoodId, sha256s: &[String]) -> anyhow::Result<()> {
        for sha in sha256s {
            self.scope(id, format!("blob:{sha}"))?;
        }
        Ok(())
    }

    pub(super) async fn add_shared(&self, id: NeighborhoodId, event: Event) -> anyhow::Result<()> {
        self.add(id, event).await
    }

    fn scope(&self, id: NeighborhoodId, key: String) -> anyhow::Result<()> {
        let mut scopes = self.scopes.lock().unwrap();
        scopes.entry(key).or_default().insert(id);
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&*scopes)?)?;
        std::fs::rename(&tmp, &self.file)?;
        Ok(())
    }

    /// Whether `event` is already kept in neighborhood `id`.
    pub fn has(&self, id: NeighborhoodId, event: &Event) -> bool {
        self.holds(id, event)
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

    fn record(&self, n: NeighborhoodId) -> Option<Record> {
        self.records().into_iter().find(|r| r.id == n)
    }

    /// Whether `n` is a neighborhood this device carries over links.
    pub fn carries(&self, n: NeighborhoodId) -> bool {
        self.record(n).is_some()
    }

    /// A blob a linked peer asks for in `n`: only one kept in `n`'s scope,
    /// and only where this side offers blob reads.
    pub async fn link_blob(&self, n: NeighborhoodId, sha256: &str) -> Option<Vec<u8>> {
        let record = self.record(n)?;
        if !record.offer.blob_read {
            return None;
        }
        self.scoped.blob(n, sha256).await.ok().flatten()
    }

    /// Ask the links carrying `n` (or only `peer`'s) for a blob, one after
    /// another, until one has it. What comes back must hash to `sha256`; it
    /// is then kept in `n`'s scope here, so the next ask is local.
    pub(super) async fn blob_from_links(
        &self,
        n: NeighborhoodId,
        sha256: &str,
        peer: Option<&str>,
    ) -> Option<Vec<u8>> {
        let links = self.links.as_ref()?;
        let rest = format!("blob/{sha256}?n={}", n.to_hex());
        for link in links.carrying(n) {
            if peer.is_some_and(|p| p != link.peer) {
                continue;
            }
            let Ok(resp) = super::service::call(&link, reqwest::Method::GET, &rest, None).await
            else {
                continue;
            };
            links.mark_seen(&link.id);
            if !resp.status().is_success()
                || resp.content_length().is_some_and(|l| l > MAX_LINK_BLOB)
            {
                continue;
            }
            let Ok(bytes) = resp.bytes().await else {
                continue;
            };
            if bytes.len() as u64 > MAX_LINK_BLOB || sha256_hex(&bytes) != sha256 {
                tracing::debug!(link = %link.id, "peer sent a blob that is not the one asked for");
                continue;
            }
            if let Err(e) = self.scoped.add_blob(n, &bytes, sha256).await {
                tracing::debug!(error = %e, "could not keep a fetched blob");
            }
            return Some(bytes.to_vec());
        }
        None
    }

    /// What a linked peer's `REQ` in `n` is answered with.
    pub async fn link_backlog(&self, n: NeighborhoodId, filters: &[Filter]) -> Vec<Event> {
        let Some(record) = self.record(n) else {
            return Vec::new();
        };
        let keep = unmuted(&record);
        self.scoped
            .backlog(n, filters)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|e| keep(e))
            .collect()
    }

    /// An event a linked peer sent into `n` with `ttl` hops left: checked,
    /// kept once, and passed on to the other links carrying `n` while hops
    /// and policy allow. `true` when it was taken (new or already held).
    pub async fn receive(&self, n: NeighborhoodId, event: Event, from_link: &str, ttl: u8) -> bool {
        let Some(record) = self.record(n) else {
            return false;
        };
        if event.verify().is_err() || !unmuted(&record)(&event) {
            return false;
        }
        if self.scoped.has(n, &event) {
            return true;
        }
        if self.scoped.add(n, event.clone()).await.is_err() {
            return false;
        }
        self.offer_update(&record, n, from_link, &event);
        if ttl > 0 && record.policy.pass_on {
            let hops = (ttl - 1).min(record.policy.publish_hops);
            self.forward(n, event, Some(from_link), hops);
        }
        true
    }

    /// Send `event` into `n` on every link carrying it but `except`, each
    /// peer allowed `ttl` more hops. Spawned: a slow peer holds up nobody.
    fn forward(&self, n: NeighborhoodId, event: Event, except: Option<&str>, ttl: u8) {
        let Some(links) = &self.links else {
            return;
        };
        for link in links.carrying(n) {
            if Some(link.id.as_str()) == except {
                continue;
            }
            let event = event.clone();
            let links = links.clone();
            tokio::spawn(async move {
                match super::wire::send(&link, n, ttl, &event).await {
                    Ok(true) => links.mark_seen(&link.id),
                    Ok(false) => {
                        links.mark_seen(&link.id);
                        tracing::debug!(link = %link.id, "peer did not take the event")
                    }
                    Err(e) => tracing::debug!(link = %link.id, error = %e, "could not reach peer"),
                }
            });
        }
    }

    /// Ask every link carrying `n` for what matches; what comes back is kept
    /// in `n` and reaches live subscribers. Not passed on.
    fn pull_links(&self, n: NeighborhoodId, filters: Vec<Filter>) {
        let Some(links) = &self.links else {
            return;
        };
        for link in links.carrying(n) {
            let filters = filters.clone();
            let scoped = self.scoped.clone();
            let links = links.clone();
            tokio::spawn(async move {
                match super::wire::pull(&link, n, &filters).await {
                    Ok(events) => {
                        links.mark_seen(&link.id);
                        for event in events {
                            if !scoped.has(n, &event) {
                                let _ = scoped.add(n, event).await;
                            }
                        }
                    }
                    Err(e) => tracing::debug!(link = %link.id, error = %e, "pull failed"),
                }
            });
        }
    }
}

/// The largest blob taken from a link.
const MAX_LINK_BLOB: u64 = 64 * 1024 * 1024;

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(bytes))
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
        match target.peer {
            // To that one peer, with no hops to pass it on.
            Some(peer) => {
                let link = self
                    .links
                    .as_ref()
                    .and_then(|l| l.by_peer(&peer))
                    .filter(|l| l.neighborhoods.contains(&record.id))
                    .ok_or(NeighborError::NotFound)?;
                self.scoped
                    .add(record.id, event.clone())
                    .await
                    .map_err(failed)?;
                let n = record.id;
                tokio::spawn(async move {
                    if let Err(e) = super::wire::send(&link, n, 0, &event).await {
                        tracing::debug!(error = %e, "could not reach the peer");
                    }
                });
                Ok(())
            }
            None => {
                self.scoped
                    .add(record.id, event.clone())
                    .await
                    .map_err(failed)?;
                let hops = record.policy.publish_hops;
                if hops > 0 {
                    self.forward(record.id, event, None, hops - 1);
                }
                Ok(())
            }
        }
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
        match target.peer {
            None => {
                let backlog = self
                    .scoped
                    .backlog(record.id, &filters)
                    .await
                    .map_err(failed)?;
                // Subscribed before the pull starts, so nothing it brings is
                // missed.
                let live = self.scoped.feed(record.id);
                if record.policy.backlog_hops > 0 {
                    self.pull_links(record.id, filters.clone());
                }
                Ok(Feed {
                    backlog: backlog.into_iter().filter(|e| keep(e)).collect(),
                    live: Box::pin(live.filter(move |e| std::future::ready(keep(e)))),
                })
            }
            // Reading one peer's view is not supported yet.
            Some(_) => Err(NeighborError::Unsupported),
        }
    }

    async fn blob(
        &self,
        napplet: &NappletIdentity,
        address: &str,
        sha256: &str,
    ) -> Result<Option<Vec<u8>>, NeighborError> {
        let (target, record) = self.target(napplet, address)?;
        if target.peer.is_none() {
            if let Some(bytes) = self.scoped.blob(record.id, sha256).await.map_err(failed)? {
                return Ok(Some(bytes));
            }
        }
        Ok(self
            .blob_from_links(record.id, sha256, target.peer.as_deref())
            .await)
    }

    /// Kept here in the neighborhood's scope.
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
        // `blob_hops` pushes nothing yet: links do not forward blobs, so an
        // upload stays here until a peer asks for it.
        self.scoped
            .add_blob(record.id, &bytes, sha256)
            .await
            .map_err(failed)
    }
}

#[cfg(test)]
mod tests {
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

    /// Bob carries every neighborhood.
    struct Bob;
    impl PeerDirectory for Bob {
        fn peers(&self, _: &Record) -> Vec<PeerView> {
            vec![PeerView {
                key: "npub1bob".into(),
                name: None,
                reachable: true,
            }]
        }
    }

    fn open() -> Neighbors {
        let d = std::env::temp_dir().join(format!("myco-nb-data-{}", super::super::id::token()));
        std::fs::create_dir_all(&d).unwrap();
        Neighbors::open(
            &d,
            Seams {
                peers: Arc::new(Bob),
                asker: Arc::new(Yes),
                scoped_store: Arc::new(nsite_deck::testing::MemRelay::new()),
                scoped_blobs: Arc::new(nsite_deck::testing::MemBlobs::new()),
                links: None,
            },
        )
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

    /// A new neighborhood's address.
    async fn create(n: &Neighbors, name: &str) -> String {
        n.create(
            &app(),
            CreateRequest {
                name: name.into(),
                auto_accept: false,
            },
        )
        .await
        .unwrap()
        .address
        .unwrap()
    }

    async fn texts(n: &Neighbors, address: &str) -> Vec<String> {
        n.subscribe(&app(), address, kind1())
            .await
            .unwrap()
            .backlog
            .into_iter()
            .map(|e| e.content)
            .collect()
    }

    #[tokio::test]
    async fn two_neighborhoods_do_not_see_each_other() {
        let n = open();
        let club = create(&n, "Club").await;
        let other = create(&n, "Other").await;
        n.publish(&app(), &club, note("club")).await.unwrap();
        n.publish(&app(), &other, note("other")).await.unwrap();
        assert_eq!(texts(&n, &club).await, ["club"]);
        assert_eq!(texts(&n, &other).await, ["other"]);
    }

    #[tokio::test]
    async fn new_events_reach_a_live_subscription_in_their_scope_only() {
        use futures_util::StreamExt as _;
        let n = open();
        let club = create(&n, "Club").await;
        let other = create(&n, "Other").await;
        let mut feed = n.subscribe(&app(), &club, kind1()).await.unwrap();
        n.publish(&app(), &other, note("elsewhere")).await.unwrap();
        n.publish(&app(), &club, note("live")).await.unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(1), feed.live.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.content, "live");
    }

    #[tokio::test]
    async fn muted_authors_are_dropped() {
        let n = open();
        let club = create(&n, "Club").await;
        let loud = note("loud");
        n.publish(&app(), &club, loud.clone()).await.unwrap();
        let id = n.list(&app()).await.remove(0).id;
        n.mute(&app(), &id, loud.pubkey, true).await.unwrap();
        assert!(texts(&n, &club).await.is_empty());
    }

    #[tokio::test]
    async fn a_peer_address_asks_only_that_peer() {
        let n = open();
        create(&n, "Club").await;
        let id = n.list(&app()).await.remove(0).id;
        let bob = n.peers(&app(), &id).await.unwrap().remove(0);
        let addr = bob.address.as_deref().unwrap();
        assert_eq!(
            n.subscribe(&app(), addr, kind1()).await.err(),
            Some(NeighborError::Unsupported)
        );
        // No link to Bob: nobody to ask for a blob, nobody to send to.
        assert_eq!(n.blob(&app(), addr, "ab").await.unwrap(), None);
        assert_eq!(
            n.publish(&app(), addr, note("psst")).await.unwrap_err(),
            NeighborError::NotFound
        );
    }

    #[tokio::test]
    async fn an_upload_is_kept_in_its_scope_only() {
        let n = open();
        let club = create(&n, "Club").await;
        let other = create(&n, "Other").await;
        let bytes = b"club picture".to_vec();
        let sha = nsite_deck::sha256_hex(&bytes);
        n.upload(&app(), &club, bytes.clone(), &sha).await.unwrap();
        assert_eq!(n.blob(&app(), &club, &sha).await.unwrap(), Some(bytes));
        assert_eq!(n.blob(&app(), &other, &sha).await.unwrap(), None);
    }

    #[tokio::test]
    async fn another_napplets_address_is_refused() {
        let n = open();
        let club = create(&n, "Club").await;
        let mut other = app();
        other.d_tag = "map".into();
        assert_eq!(
            n.publish(&other, &club, note("x")).await.unwrap_err(),
            NeighborError::NotFound
        );
    }
}
