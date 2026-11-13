//! [`Carrier`]: how Everyone's traffic travels today — over the Circle's mesh.
//!
//! Everyone is the Circle, so it rides what NAP-MESH already uses: the
//! relay hub's hop-limited flood for publishes, the Circle pull for backlog,
//! the hub's live stream, and the Circle members' Blossom for blobs. Links
//! (phase 4) replace this for every neighborhood; until then only Everyone
//! has peers to carry anything to.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use myco_napplet_runtime::seams::{MeshSink, PulledEvents};
use nostr::{Event, Filter};
use nsite_deck::PeerSource as _;

/// Everyone's transport. A seam so the host is testable without a node.
#[async_trait]
pub trait Carrier: Send + Sync {
    /// Store here and flood to the Circle, `hops` rings out.
    async fn publish(&self, event: Event, hops: u8) -> anyhow::Result<()>;
    /// Send to one paired peer only.
    async fn send_to(&self, peer: &str, event: Event) -> anyhow::Result<()>;
    /// What this device holds that matches.
    async fn local(&self, filters: &[Filter]) -> anyhow::Result<Vec<Event>>;
    /// Every new event this device accepts, from now on.
    fn live(&self) -> PulledEvents;
    /// Ask the Circle, `hops` rings out, for matching events.
    async fn pull(&self, filters: &[Filter], hops: u8) -> anyhow::Result<PulledEvents>;
    /// A blob from this device, else from Circle peers (`peer`: only that one).
    async fn blob(&self, sha256: &str, peer: Option<&str>) -> Option<Vec<u8>>;
}

/// The Circle's mesh, as the napplet host already reaches it.
pub struct MeshCarrier {
    pub mesh: Arc<dyn MeshSink>,
    pub content: Arc<crate::content::Content>,
    pub hub: Arc<Mutex<Option<Arc<crate::mesh_relay::RelayHub>>>>,
}

/// How long one peer gets to hand over a blob.
const BLOB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
/// The largest blob fetched for a napplet; NAP-RESOURCE's cap.
const MAX_BLOB_BYTES: usize = 16 * 1024 * 1024;

#[async_trait]
impl Carrier for MeshCarrier {
    async fn publish(&self, event: Event, hops: u8) -> anyhow::Result<()> {
        self.mesh.publish(event, hops).await
    }

    async fn send_to(&self, peer: &str, event: Event) -> anyhow::Result<()> {
        // Stored here first, as any publish is; then one frame to one peer.
        self.content.relay().publish(event.clone()).await?;
        let frame = serde_json::to_string(&serde_json::json!(["EVENT", event]))?;
        let url = myco_napplet_runtime::seams::mesh_relay_url(peer);
        self.content.peer_relays().send(peer, &url, frame);
        Ok(())
    }

    async fn local(&self, filters: &[Filter]) -> anyhow::Result<Vec<Event>> {
        self.content.relay().query(filters).await
    }

    fn live(&self) -> PulledEvents {
        let hub = self.hub.lock().unwrap().clone();
        let Some(hub) = hub else {
            return Box::pin(futures_util::stream::empty());
        };
        let rx = hub.live_events();
        Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok(event) => return Some((event, rx)),
                    // A slow reader skips what it missed rather than ending.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                }
            }
        }))
    }

    async fn pull(&self, filters: &[Filter], hops: u8) -> anyhow::Result<PulledEvents> {
        let raw = filters
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()?;
        self.mesh.pull(raw, hops).await
    }

    async fn blob(&self, sha256: &str, peer: Option<&str>) -> Option<Vec<u8>> {
        use futures_util::StreamExt as _;
        if let Ok(Some(bytes)) = self.content.blobs().get(sha256).await {
            return Some(bytes);
        }
        let pool = self.content.peer_relays();
        let mut asks: futures_util::stream::FuturesUnordered<_> = self
            .content
            .reachable_npubs()
            .into_iter()
            .filter(|npub| peer.is_none_or(|p| p == npub))
            .filter_map(|npub| crate::ip_source::mesh_source_for(pool.clone(), &npub).ok())
            .map(|source| source.with_max_blob_bytes(MAX_BLOB_BYTES))
            .map(|source| async move {
                match tokio::time::timeout(BLOB_TIMEOUT, source.fetch_blob(sha256, &[])).await {
                    Ok(Ok(Some(bytes))) => Some(bytes),
                    _ => None,
                }
            })
            .collect();
        while let Some(found) = asks.next().await {
            if found.is_some() {
                return found;
            }
        }
        None
    }
}

#[cfg(test)]
pub mod testing {
    //! [`MemCarrier`]: Everyone's transport without a node.

    use super::*;

    /// Records what was sent; "the device" is a list of events.
    #[derive(Default)]
    pub struct MemCarrier {
        pub device: Mutex<Vec<Event>>,
        pub flooded: Mutex<Vec<(Event, u8)>>,
        pub sent: Mutex<Vec<(String, Event)>>,
        pub blobs: Mutex<std::collections::HashMap<String, Vec<u8>>>,
    }

    #[async_trait]
    impl Carrier for MemCarrier {
        async fn publish(&self, event: Event, hops: u8) -> anyhow::Result<()> {
            self.device.lock().unwrap().push(event.clone());
            self.flooded.lock().unwrap().push((event, hops));
            Ok(())
        }
        async fn send_to(&self, peer: &str, event: Event) -> anyhow::Result<()> {
            self.sent.lock().unwrap().push((peer.to_string(), event));
            Ok(())
        }
        async fn local(&self, filters: &[Filter]) -> anyhow::Result<Vec<Event>> {
            let options = nostr::filter::MatchEventOptions::new();
            Ok(self
                .device
                .lock()
                .unwrap()
                .iter()
                .filter(|e| filters.iter().any(|f| f.match_event(e, options)))
                .cloned()
                .collect())
        }
        fn live(&self) -> PulledEvents {
            Box::pin(futures_util::stream::empty())
        }
        async fn pull(&self, _filters: &[Filter], _hops: u8) -> anyhow::Result<PulledEvents> {
            Ok(Box::pin(futures_util::stream::empty()))
        }
        async fn blob(&self, sha256: &str, _peer: Option<&str>) -> Option<Vec<u8>> {
            self.blobs.lock().unwrap().get(sha256).cloned()
        }
    }
}
