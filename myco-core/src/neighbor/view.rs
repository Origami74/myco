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

pub fn build(records: &[Record], links: &Links) -> NeighborhoodsView {
    let all = links.all();
    NeighborhoodsView {
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
