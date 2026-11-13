//! What the shell's neighborhood screens show: the `neighborhoods` field of
//! the app state.

use serde::Serialize;

use super::link::Links;
use super::store::{Record, Role};

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeighborhoodsView {
    pub list: Vec<NeighborhoodView>,
    pub links: Vec<LinkView>,
    /// The open invitation, encoded for a QR code, if one was just made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite: Option<String>,
    /// The last link action's outcome, for a line of text.
    pub status: String,
    /// Everyone the user is connected to, with the neighborhoods each
    /// carries: the map's nodes.
    pub peers: Vec<PeerNode>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerNode {
    pub key: String,
    pub name: String,
    pub reachable: bool,
    /// Neighborhood ids this peer carries with the user.
    pub neighborhoods: Vec<String>,
    /// The link, when the peer is a link rather than a paired Circle member.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_id: Option<String>,
}

/// A paired Circle member, as the map shows them.
pub struct CircleMember {
    pub npub: String,
    pub name: String,
    pub reachable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeighborhoodView {
    pub id: String,
    pub name: String,
    pub auto_accept: bool,
    pub everyone: bool,
    /// Links carrying it.
    pub links: usize,
    pub publish_hops: u8,
    pub backlog_hops: u8,
    pub blob_hops: u8,
    pub pass_on: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkView {
    pub id: String,
    pub name: String,
    /// Neighborhood ids carried.
    pub neighborhoods: Vec<String>,
    /// Shares waiting for the user.
    pub pending: Vec<PendingView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingView {
    pub id: String,
    pub name: String,
}

pub fn build(records: &[Record], links: &Links, circle: &[CircleMember]) -> NeighborhoodsView {
    let all = links.all();
    let everyone = records
        .iter()
        .find(|r| r.role == Role::Everyone)
        .map(|r| r.id.to_hex());
    let mut peers: Vec<PeerNode> = circle
        .iter()
        .map(|c| PeerNode {
            key: c.npub.clone(),
            name: c.name.clone(),
            reachable: c.reachable,
            neighborhoods: everyone.iter().cloned().collect(),
            link_id: None,
        })
        .collect();
    peers.extend(all.iter().map(|l| {
        PeerNode {
            key: l.peer.clone(),
            name: l
                .name
                .clone()
                .unwrap_or_else(|| format!("peer {}", &l.peer[..8])),
            reachable: links.reachable(&l.id),
            neighborhoods: l.neighborhoods.iter().map(|id| id.to_hex()).collect(),
            link_id: Some(l.id.clone()),
        }
    }));
    NeighborhoodsView {
        peers,
        list: records
            .iter()
            .map(|r| NeighborhoodView {
                id: r.id.to_hex(),
                name: r.name.clone(),
                auto_accept: r.auto_accept,
                everyone: r.role == Role::Everyone,
                links: all
                    .iter()
                    .filter(|l| l.neighborhoods.contains(&r.id))
                    .count(),
                publish_hops: r.policy.publish_hops,
                backlog_hops: r.policy.backlog_hops,
                blob_hops: r.policy.blob_hops,
                pass_on: r.policy.pass_on,
            })
            .collect(),
        links: all
            .iter()
            .map(|l| LinkView {
                id: l.id.clone(),
                name: l
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("peer {}", &l.peer[..8])),
                neighborhoods: l.neighborhoods.iter().map(|id| id.to_hex()).collect(),
                pending: l
                    .pending
                    .iter()
                    .map(|p| PendingView {
                        id: p.id.to_hex(),
                        name: p.name.clone(),
                    })
                    .collect(),
            })
            .collect(),
        invite: None,
        status: String::new(),
    }
}
