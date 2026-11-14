//! The **auth plane**: a small HTTP service on `:4873` that carries the
//! neighborhood link service under `/nh` (see [`crate::neighbor::service`]).
//!
//! Link setup is bootstrap — it is what creates a link, and links are what
//! gate everything else — so it has no business travelling on the planes it
//! authorises. It terminates here instead, and the content ports (`:4870`
//! relay, `:24243` Blossom) admit only app peers, with no exceptions.
//!
//! It is the one port open to anyone, so every request is paced per source
//! address ([`Limiter`]) and bodies are capped.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{ConnectInfo, DefaultBodyLimit, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
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

/// Sustained requests per second per source address, and the burst it may
/// spend at once. One device can hold several links to us (one per
/// invitation), each checking in every 30 s and fetching offers, so the
/// burst is roomy; only something automated runs it dry.
const RATE_PER_SEC: f64 = 2.0;
const RATE_BURST: f64 = 20.0;

/// Sources remembered at once. Past this, idle (full) buckets are dropped,
/// so a peer dialling from many addresses can't grow the table.
const MAX_SOURCES: usize = 1024;

/// Per-source token bucket. A source that empties it is told to slow down
/// (429), not banned: a mesh address costs nothing to replace, so there is
/// no identity worth banning, and a failed request is not evidence of intent.
struct Bucket {
    tokens: f64,
    last: Instant,
}

#[derive(Default)]
pub(crate) struct Limiter {
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

impl Limiter {
    /// Take a token for `ip`, or `false` if its bucket is empty.
    fn take(&self, ip: IpAddr, now: Instant) -> bool {
        let mut buckets = self.buckets.lock().unwrap();
        if buckets.len() >= MAX_SOURCES {
            buckets.retain(|_, b| {
                let refilled =
                    b.tokens + now.saturating_duration_since(b.last).as_secs_f64() * RATE_PER_SEC;
                refilled < RATE_BURST
            });
        }
        let bucket = buckets.entry(ip).or_insert(Bucket {
            tokens: RATE_BURST,
            last: now,
        });
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * RATE_PER_SEC).min(RATE_BURST);
        bucket.last = now;
        if bucket.tokens < 1.0 {
            return false;
        }
        bucket.tokens -= 1.0;
        true
    }
}

async fn pace(
    State(limiter): State<Arc<Limiter>>,
    ConnectInfo(from): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    if !limiter.take(from.ip(), Instant::now()) {
        return (StatusCode::TOO_MANY_REQUESTS, "slow down").into_response();
    }
    next.run(request).await
}

/// Serve the auth plane on an already-bound listener: a kernel socket, or the
/// mesh stack's (see [`crate::mesh_net`]).
pub async fn serve<L>(
    links: crate::neighbor::service::LinkService,
    listener: L,
) -> anyhow::Result<()>
where
    L: axum::serve::Listener<Addr = SocketAddr>,
{
    axum::serve(
        axum::serve::ListenerExt::tap_io(listener, |_: &mut L::Io| {}),
        app(links).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

fn app(links: crate::neighbor::service::LinkService) -> Router {
    Router::new()
        .merge(crate::neighbor::service::routes(links))
        .layer(middleware::from_fn_with_state(
            Arc::new(Limiter::default()),
            pace,
        ))
        .layer(DefaultBodyLimit::max(MAX_BODY))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ip(n: u8) -> IpAddr {
        IpAddr::from([0xfd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, n])
    }

    #[test]
    fn a_burst_then_a_refill() {
        let limiter = Limiter::default();
        let t = Instant::now();
        for _ in 0..RATE_BURST as usize {
            assert!(limiter.take(ip(1), t));
        }
        assert!(!limiter.take(ip(1), t), "the burst is spent");
        assert!(limiter.take(ip(2), t), "another source has its own bucket");
        assert!(limiter.take(ip(1), t + Duration::from_secs(1)), "refilled");
    }

    #[test]
    fn the_table_stays_bounded() {
        let limiter = Limiter::default();
        let t = Instant::now();
        for i in 0..(MAX_SOURCES + 10) {
            let a = IpAddr::from((i as u128 | 0xfd00 << 112).to_be_bytes());
            limiter.take(a, t);
        }
        let later = t + Duration::from_secs(60);
        limiter.take(ip(1), later);
        assert!(limiter.buckets.lock().unwrap().len() <= MAX_SOURCES);
    }
}
