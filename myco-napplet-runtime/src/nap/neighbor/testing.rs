//! [`MemNeighbors`]: an in-memory [`NeighborHost`] for host tests.

use std::sync::Mutex;

use async_trait::async_trait;

use super::model::{
    CreateRequest, Feature, InviteResult, InviteStatus, NeighborError, NeighborInfo, Neighborhood,
    Offer, Peer,
};
use super::seam::{Feed, NeighborData, NeighborHost};
use crate::session::NappletIdentity;

/// A fixed set of neighborhoods and peers, the same for every napplet, that
/// records what it was asked to do.
pub struct MemNeighbors {
    pub info: NeighborInfo,
    pub neighborhoods: Vec<Neighborhood>,
    /// `(neighborhood id, peer)`.
    pub peers: Vec<(String, Peer)>,
    /// Answer every prompt with "no".
    pub decline: bool,
    /// Blobs any known address serves, by sha256.
    pub blobs: Mutex<std::collections::HashMap<String, Vec<u8>>>,
    /// What was published, per address.
    pub published: Mutex<Vec<(String, nostr::Event)>>,
    calls: Mutex<Vec<String>>,
}

impl MemNeighbors {
    /// Every feature but `nearby`; one neighborhood, "Household" (`h2c`),
    /// carried by one reachable peer (`p7f`).
    pub fn household() -> Self {
        Self {
            info: NeighborInfo {
                features: vec![
                    Feature::Create,
                    Feature::Invite,
                    Feature::Leave,
                    Feature::Offer,
                    Feature::Mute,
                ],
                services: vec!["relay".into(), "blossom".into()],
            },
            neighborhoods: vec![Neighborhood {
                id: "h2c".into(),
                name: "Household".into(),
                auto_accept: false,
                peers: 1,
                reachable: 1,
                address: Some(super::address::format("h2c", None)),
                offer: Some(Offer::default()),
            }],
            peers: vec![(
                "h2c".into(),
                Peer {
                    id: "p7f".into(),
                    name: Some("Bob".into()),
                    reachable: true,
                    link: None,
                    services: vec!["relay".into()],
                    address: Some(super::address::format("h2c", Some("p7f"))),
                },
            )],
            decline: false,
            blobs: Mutex::default(),
            published: Mutex::default(),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// What the host was asked to change, in order.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    fn known(&self, neighborhood: &str) -> Result<(), NeighborError> {
        self.neighborhoods
            .iter()
            .any(|n| n.id == neighborhood)
            .then_some(())
            .ok_or(NeighborError::NotFound)
    }

    fn asked(&self) -> Result<(), NeighborError> {
        if self.decline {
            Err(NeighborError::Cancelled)
        } else {
            Ok(())
        }
    }
}

impl MemNeighbors {
    /// Whether `addr` names one of the fixed neighborhoods.
    fn addressed(&self, addr: &str) -> Result<(), NeighborError> {
        let t = super::address::parse(addr).ok_or(NeighborError::NotFound)?;
        self.known(&t.neighborhood)
    }
}

#[async_trait]
impl NeighborData for MemNeighbors {
    async fn publish(
        &self,
        _napplet: &NappletIdentity,
        address: &str,
        event: nostr::Event,
    ) -> Result<(), NeighborError> {
        self.addressed(address)?;
        self.published
            .lock()
            .unwrap()
            .push((address.to_string(), event));
        Ok(())
    }

    /// The backlog is what was published to the same address; nothing live.
    async fn subscribe(
        &self,
        _napplet: &NappletIdentity,
        address: &str,
        _filters: Vec<nostr::Filter>,
    ) -> Result<Feed, NeighborError> {
        self.addressed(address)?;
        let backlog = self
            .published
            .lock()
            .unwrap()
            .iter()
            .filter(|(a, _)| a == address)
            .map(|(_, e)| e.clone())
            .collect();
        Ok(Feed {
            backlog,
            live: Box::pin(futures_util::stream::empty()),
        })
    }

    async fn blob(
        &self,
        _napplet: &NappletIdentity,
        address: &str,
        sha256: &str,
    ) -> Result<Option<Vec<u8>>, NeighborError> {
        self.addressed(address)?;
        self.record(format!("blob {sha256}"));
        Ok(self.blobs.lock().unwrap().get(sha256).cloned())
    }
}

#[async_trait]
impl NeighborHost for MemNeighbors {
    async fn info(&self) -> NeighborInfo {
        self.info.clone()
    }

    async fn list(&self, _napplet: &NappletIdentity) -> Vec<Neighborhood> {
        self.neighborhoods.clone()
    }

    async fn peers(
        &self,
        _napplet: &NappletIdentity,
        neighborhood: &str,
    ) -> Result<Vec<Peer>, NeighborError> {
        self.known(neighborhood)?;
        Ok(self
            .peers
            .iter()
            .filter(|(n, _)| n == neighborhood)
            .map(|(_, p)| p.clone())
            .collect())
    }

    async fn create(
        &self,
        _napplet: &NappletIdentity,
        request: CreateRequest,
    ) -> Result<Neighborhood, NeighborError> {
        self.asked()?;
        let auto = if request.auto_accept { " auto" } else { "" };
        self.record(format!("create {}{auto}", request.name));
        Ok(Neighborhood {
            id: "new".into(),
            name: request.name,
            auto_accept: request.auto_accept,
            peers: 0,
            reachable: 0,
            address: Some(super::address::format("new", None)),
            offer: None,
        })
    }

    async fn invite(
        &self,
        _napplet: &NappletIdentity,
        neighborhood: &str,
        candidates: Vec<String>,
    ) -> Result<InviteResult, NeighborError> {
        self.known(neighborhood)?;
        self.asked()?;
        self.record(format!("invite {neighborhood} {}", candidates.join(",")));
        Ok(InviteResult {
            status: InviteStatus::Sent,
            count: Some(candidates.len() as u32),
        })
    }

    async fn leave(
        &self,
        _napplet: &NappletIdentity,
        neighborhood: &str,
    ) -> Result<(), NeighborError> {
        self.known(neighborhood)?;
        self.asked()?;
        self.record(format!("leave {neighborhood}"));
        Ok(())
    }

    async fn set_offer(
        &self,
        _napplet: &NappletIdentity,
        neighborhood: &str,
        offer: Offer,
    ) -> Result<Offer, NeighborError> {
        self.known(neighborhood)?;
        self.asked()?;
        self.record(format!("setOffer {neighborhood}"));
        Ok(offer)
    }

    async fn mute(
        &self,
        _napplet: &NappletIdentity,
        neighborhood: &str,
        author: nostr::PublicKey,
        muted: bool,
    ) -> Result<(), NeighborError> {
        self.known(neighborhood)?;
        self.record(format!("mute {neighborhood} {} {muted}", author.to_hex()));
        Ok(())
    }
}
