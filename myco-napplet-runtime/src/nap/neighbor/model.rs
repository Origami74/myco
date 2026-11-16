//! NAP-NEIGHBOR's schema types, as they appear on the wire.
//!
//! Field names are the NAP's (`camelCase`), so these serialize straight into
//! result envelopes. Optional fields are left out when absent rather than sent
//! as `null`: the NAP says "no", not "null".

use serde::{Deserialize, Serialize};

/// What a runtime offers within NAP-NEIGHBOR. A napplet checks
/// [`NeighborInfo::features`] before calling an operation it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Feature {
    /// `neighbor.create`.
    Create,
    /// `neighbor.invite`.
    Invite,
    /// `neighbor.leave`.
    Leave,
    /// `neighbor.setOffer`, and `Neighborhood.offer` in results.
    Offer,
    /// `neighbor.mute`.
    Mute,
    /// The proximity extension: the `nearby` neighborhood and
    /// `link: "proximity"`.
    Nearby,
    /// The napplet-updates extension: newer manifests of installed napplets
    /// arrive in neighborhoods, with that version's files fetchable there.
    #[serde(rename = "napplet-updates")]
    NappletUpdates,
}

impl Feature {
    /// The feature an action needs, if it needs one. `info`, `list` and
    /// `peers` are always there.
    pub fn for_action(action: &str) -> Option<Self> {
        match action {
            "create" => Some(Self::Create),
            "invite" => Some(Self::Invite),
            "leave" => Some(Self::Leave),
            "setOffer" => Some(Self::Offer),
            "mute" => Some(Self::Mute),
            _ => None,
        }
    }
}

/// Coarse class of the path to a peer right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkClass {
    /// A direct, local radio or physical link. Proximity extension only.
    Proximity,
    /// The same local network.
    Local,
    /// Anything further.
    Internet,
}

/// `neighbor.info`'s answer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NeighborInfo {
    pub features: Vec<Feature>,
    /// Service names peers may offer (see [`Offer::services`]).
    pub services: Vec<String>,
}

impl NeighborInfo {
    pub fn has(&self, feature: Feature) -> bool {
        self.features.contains(&feature)
    }
}

/// One neighborhood the user carries, as one napplet sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Neighborhood {
    /// Opaque, minted per napplet.
    pub id: String,
    /// Display name. Not unique.
    pub name: String,
    /// Joined by name, and new peers carrying it are accepted without asking.
    pub auto_accept: bool,
    /// The user's peers that carry it.
    pub peers: u32,
    /// Of those, reachable now.
    pub reachable: u32,
    /// `neighborhood:` address for the whole neighborhood.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// What the user offers peers in it. Only with [`Feature::Offer`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<Offer>,
}

/// One of the user's own peers, as seen in one neighborhood.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    /// Opaque, minted per napplet and neighborhood. Not a social identity.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub reachable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<LinkClass>,
    /// Services this peer offers in this neighborhood.
    pub services: Vec<String>,
    /// `neighborhood:` address for this peer, scoped to this neighborhood.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

/// What the user offers peers in one neighborhood.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    pub relay_read: bool,
    pub relay_write: bool,
    pub relay_own: bool,
    pub blob_read: bool,
    pub blob_write: bool,
    pub blob_own: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
}

/// `neighbor.create`'s request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRequest {
    pub name: String,
    /// Join the auto-accepted neighborhood of this name rather than start a
    /// new one.
    #[serde(default)]
    pub auto_accept: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InviteStatus {
    /// The runtime took delivery responsibility. Not that anyone accepted.
    Sent,
    Cancelled,
}

/// `neighbor.invite`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteResult {
    pub status: InviteStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NeighborhoodChange {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PeerChange {
    Added,
    Removed,
    Reachable,
    Unreachable,
    Changed,
}

/// What `neighbor.event` carries. Ids are the receiving napplet's own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum NeighborEvent {
    Neighborhood {
        change: NeighborhoodChange,
        neighborhood: String,
    },
    Peer {
        neighborhood: String,
        peer: String,
        change: PeerChange,
    },
}

/// The NAP's error strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeighborError {
    /// Not offered by this runtime (see [`NeighborInfo::features`]).
    Unsupported,
    /// Not granted, or refused by policy.
    Denied,
    /// The user declined.
    Cancelled,
    /// Unknown or ungranted neighborhood or peer id.
    NotFound,
    /// A malformed request: a missing field, or one of the wrong type.
    Invalid,
}

impl NeighborError {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
            Self::NotFound => "not found",
            Self::Invalid => "invalid",
        }
    }
}

impl std::fmt::Display for NeighborError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::error::Error for NeighborError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_neighborhood_leaves_absent_fields_out() {
        let n = Neighborhood {
            id: "h_2c".into(),
            name: "Household".into(),
            auto_accept: false,
            peers: 3,
            reachable: 2,
            address: Some("neighborhood://h_2c".into()),
            offer: None,
        };
        assert_eq!(
            serde_json::to_value(&n).unwrap(),
            json!({
                "id": "h_2c", "name": "Household", "autoAccept": false,
                "peers": 3, "reachable": 2, "address": "neighborhood://h_2c"
            })
        );
    }

    #[test]
    fn an_offer_uses_the_nap_field_names() {
        let o = Offer {
            relay_read: true,
            blob_own: true,
            ..Offer::default()
        };
        assert_eq!(
            serde_json::to_value(&o).unwrap(),
            json!({
                "relayRead": true, "relayWrite": false, "relayOwn": false,
                "blobRead": false, "blobWrite": false, "blobOwn": true
            })
        );
    }

    #[test]
    fn events_are_tagged_by_type() {
        let e = NeighborEvent::Peer {
            neighborhood: "h_2c".into(),
            peer: "p_7f".into(),
            change: PeerChange::Reachable,
        };
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({"type": "peer", "neighborhood": "h_2c", "peer": "p_7f", "change": "reachable"})
        );
    }

    #[test]
    fn a_create_request_defaults_to_a_new_neighborhood() {
        let r: CreateRequest = serde_json::from_value(json!({"name": "Home"})).unwrap();
        assert!(!r.auto_accept);
    }
}
