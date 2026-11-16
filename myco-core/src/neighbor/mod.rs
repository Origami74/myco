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
//!   and each napplet's tokens.
//! - [`data`] — the data plane: each neighborhood in its own scoped store,
//!   carried over links.
//! - [`link`] — links: link keys, the one-time-secret setup, signed offers,
//!   NIP-98 checks, `links.json`.
//! - [`extensions`] — what a device shares proactively per neighborhood
//!   (napplet updates, nsite updates, the Myco release).
//! - [`updates`] — the update extensions: pushing a new version's manifest
//!   to linked peers, and fetching one they pushed.
//! - [`host`] — [`Neighbors`], the [`NeighborHost`](myco_napplet_runtime::NeighborHost)
//!   napplets talk to, and the seams it reaches peers and the user through.
//!
//! Neighborhoods are carried over links only.

pub mod data;
pub mod extensions;
pub mod host;
pub mod id;
pub mod link;
pub mod service;
pub mod store;
pub mod updates;
pub mod view;
pub mod wire;

#[cfg(test)]
mod e2e;

pub use host::{LinkPeers, Neighbors, NoAsker, Seams};
pub use id::NeighborhoodId;
