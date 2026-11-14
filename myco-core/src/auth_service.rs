//! The **auth plane**: a small HTTP service on `:4873` that carries the
//! neighborhood link service under `/nh` (see [`crate::neighbor::service`]).
//!
//! Link setup is bootstrap — it is what creates a link, and links are what
//! gate everything else — so it has no business travelling on the planes it
//! authorises. It terminates here instead, and the content ports (`:4870`
//! relay, `:24243` Blossom) admit only app peers, with no exceptions.

use std::net::SocketAddr;

use axum::extract::DefaultBodyLimit;
use axum::Router;

/// The auth plane's port. A Myco constant, not negotiated: peers agree by running
/// the same version, which is fine pre-1.0.
///
/// Clear of every number already spoken for: 4869 is what public Nostr relays
/// commonly take, 4870 is our relay, 4871 is the LAN/AP lane (and where
/// pre-4872 Wi-Fi Aware peers still listen), and 4872 is the Aware lane. Those
/// are UDP and this is TCP, so an overlap would not have collided — but a number
/// that means two things in one codebase is a trap for whoever next reads a
/// packet capture.
pub const AUTH_PORT: u16 = 4873;

/// A link setup or offer is a few signed events; anything larger is not one.
const MAX_BODY: usize = 8 * 1024;

/// Serve the auth plane on an already-bound listener: a kernel socket, or the
/// mesh stack's (see [`crate::mesh_net`]).
pub async fn serve<L>(
    links: crate::neighbor::service::LinkService,
    listener: L,
) -> anyhow::Result<()>
where
    L: axum::serve::Listener<Addr = SocketAddr>,
{
    let app = Router::new()
        .merge(crate::neighbor::service::routes(links))
        .layer(DefaultBodyLimit::max(MAX_BODY));
    axum::serve(
        axum::serve::ListenerExt::tap_io(listener, |_: &mut L::Io| {}),
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
