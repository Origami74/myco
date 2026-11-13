//! [`NeighborHost`]: where NAP-NEIGHBOR reaches the device's neighborhoods.
//!
//! The runtime parses and answers the wire; the host owns everything real —
//! which neighborhoods exist, who carries them, what each napplet was granted,
//! the per-napplet ids and addresses, and the prompts that put a person in
//! front of every change. Every call names the napplet, because every id and
//! address is that napplet's own.

use async_trait::async_trait;

use super::model::{
    CreateRequest, InviteResult, NeighborError, NeighborInfo, Neighborhood, Offer, Peer,
};
use crate::seams::PulledEvents;
use crate::session::NappletIdentity;

/// What a subscription on an address starts with.
pub struct Feed {
    /// What this device already holds in that scope, matching the filters.
    pub backlog: Vec<nostr::Event>,
    /// Everything after: peers' backlog, then new events in the scope, until
    /// the stream ends or the subscription is closed (dropping it stops it).
    pub live: PulledEvents,
}

/// The device's neighborhoods, behind NAP-NEIGHBOR.
///
/// Every operation but [`info`](NeighborHost::info) and
/// [`list`](NeighborHost::list) defaults to `unsupported`, so a host offers
/// exactly what it implements, and [`info`](NeighborHost::info) must say so.
#[async_trait]
pub trait NeighborHost: NeighborData {
    /// Features and service names. Read per call.
    async fn info(&self) -> NeighborInfo;

    /// The neighborhoods granted to `napplet`, in its own ids.
    async fn list(&self, napplet: &NappletIdentity) -> Vec<Neighborhood>;

    /// The user's own peers that carry `neighborhood`.
    async fn peers(
        &self,
        napplet: &NappletIdentity,
        neighborhood: &str,
    ) -> Result<Vec<Peer>, NeighborError> {
        let _ = (napplet, neighborhood);
        Err(NeighborError::NotFound)
    }

    /// Ask the user to start (or, with `auto_accept`, join) a neighborhood.
    /// Resolves after the user answers; grants it to `napplet` on yes.
    async fn create(
        &self,
        napplet: &NappletIdentity,
        request: CreateRequest,
    ) -> Result<Neighborhood, NeighborError> {
        let _ = (napplet, request);
        Err(NeighborError::Unsupported)
    }

    /// Ask the user to share `neighborhood` with people. `candidates` are
    /// this napplet's peer ids from its other neighborhoods.
    async fn invite(
        &self,
        napplet: &NappletIdentity,
        neighborhood: &str,
        candidates: Vec<String>,
    ) -> Result<InviteResult, NeighborError> {
        let _ = (napplet, neighborhood, candidates);
        Err(NeighborError::Unsupported)
    }

    /// Ask the user to stop carrying `neighborhood` on all their links.
    async fn leave(
        &self,
        napplet: &NappletIdentity,
        neighborhood: &str,
    ) -> Result<(), NeighborError> {
        let _ = (napplet, neighborhood);
        Err(NeighborError::Unsupported)
    }

    /// Ask the user to change their offer. Resolves to the offer in force.
    async fn set_offer(
        &self,
        napplet: &NappletIdentity,
        neighborhood: &str,
        offer: Offer,
    ) -> Result<Offer, NeighborError> {
        let _ = (napplet, neighborhood, offer);
        Err(NeighborError::Unsupported)
    }

    /// Hide, and stop passing on, events by `author` in `neighborhood`.
    async fn mute(
        &self,
        napplet: &NappletIdentity,
        neighborhood: &str,
        author: nostr::PublicKey,
        muted: bool,
    ) -> Result<(), NeighborError> {
        let _ = (napplet, neighborhood, author, muted);
        Err(NeighborError::Unsupported)
    }
}

/// The data plane: what `relay`, `outbox` and `resource` do with an address.
/// Separate from [`NeighborHost`]'s management half only in purpose — one
/// host implements both.
#[async_trait]
pub trait NeighborData: Send + Sync {
    /// Publish a signed event into the scope `address` names: the
    /// neighborhood, or that one peer.
    async fn publish(
        &self,
        napplet: &NappletIdentity,
        address: &str,
        event: nostr::Event,
    ) -> Result<(), NeighborError> {
        let _ = (napplet, address, event);
        Err(NeighborError::NotFound)
    }

    /// Read the scope `address` names: what is held here, then what arrives.
    async fn subscribe(
        &self,
        napplet: &NappletIdentity,
        address: &str,
        filters: Vec<nostr::Filter>,
    ) -> Result<Feed, NeighborError> {
        let _ = (napplet, address, filters);
        Err(NeighborError::NotFound)
    }

    /// Fetch a blob from the scope `address` names. `Ok(None)` when nobody
    /// there has it. The caller verifies the hash.
    async fn blob(
        &self,
        napplet: &NappletIdentity,
        address: &str,
        sha256: &str,
    ) -> Result<Option<Vec<u8>>, NeighborError> {
        let _ = (napplet, address, sha256);
        Err(NeighborError::NotFound)
    }
}

/// A device with no neighborhoods: no features, nothing listed. What a host
/// passes until it implements NAP-NEIGHBOR.
pub struct NoNeighborhoods;

#[async_trait]
impl NeighborHost for NoNeighborhoods {
    async fn info(&self) -> NeighborInfo {
        NeighborInfo::default()
    }

    async fn list(&self, _napplet: &NappletIdentity) -> Vec<Neighborhood> {
        Vec::new()
    }
}

#[async_trait]
impl NeighborData for NoNeighborhoods {}
