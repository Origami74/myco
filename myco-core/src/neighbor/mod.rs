//! Neighborhoods on this device: NAP-NEIGHBOR's host side.
//!
//! A neighborhood is a propagation channel ("home", "festival") carried over
//! the user's own peer connections; see `.planning/neighbour/` for the design
//! and napplet/naps#146 for the napplet contract. This folder holds
//! everything Myco-specific about it; the wire lives in
//! `myco_napplet_runtime::nap::neighbor`.
//!
//! - [`id`] — neighborhood ids: random, or derived from a name.
//! - [`store`] — `neighborhoods.json`: records, policy, offers, mutes,
//!   grants, and each napplet's tokens.
//! - [`data`] — the data plane: Everyone over the Circle's mesh
//!   ([`carrier`]), every other neighborhood in its own scoped store.
//! - [`link`] — links: link keys, the one-time-secret setup, signed offers,
//!   NIP-98 checks, `links.json`.
//! - [`host`] — [`Neighbors`], the [`NeighborHost`](myco_napplet_runtime::NeighborHost)
//!   napplets talk to, and the seams it reaches peers and the user through.
//!
//! The Circle is the first neighborhood: **Everyone**, carried by every paired
//! device and granted to every napplet with the `neighbor` domain, so what
//! NAP-MESH reached stays reachable.

pub mod carrier;
pub mod data;
pub mod host;
pub mod id;
pub mod link;
pub mod store;

pub use carrier::MeshCarrier;
pub use host::{CirclePeers, Neighbors, NoAsker, Seams};
