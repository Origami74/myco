//! The **auth plane**: a small HTTP service on `:4871` whose only job is the
//! pairing handshake.
//!
//! Pairing is what *creates* the circle, and the circle is what gates everything
//! else — so bootstrap has no business travelling on the plane it authorises.
//! Before this, an unpaired stranger could publish to the relay port (a kind
//! whitelist in the access gate let the three pairing kinds through), and the
//! handshake event was written into the event store as a side effect even though
//! nothing ever read it back. With a swappable relay behind us that would mean
//! handing a stranger a write path into a store we do not own.
//!
//! So pairing terminates here instead, and the content ports (`:4870` relay,
//! `:24243` Blossom) can require membership with no exceptions.
//!
//! ```text
//! POST /pair    body: a signed pair event (9101 request / 9102 accept / 9103 remove)
//!   200 {"status":"paired"}     an accept, processed — they are in our circle
//!   202 {"status":"pending"}    a request, waiting on the user
//!   403 {"status":"declined"}   refused
//! ```
//!
//! One route, not three: the kind is already in the event. The payload stays a
//! signed Nostr event because the signature *is* the pairing identity proof —
//! what changed is where it stops, not what it looks like.
//!
//! **No tokens, no sessions.** FIPS addresses are identity-derived and Noise-IK
//! authenticated, so the mesh address already *is* the authenticated npub. The
//! only output of this service is a circle membership commit, which is exactly
//! what the content gates read. A session credential would be redundant crypto
//! over an already-authenticated channel.
//!
//! See `reference/thinning-custom-relay.md` (D6).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use nostr::Event;

use crate::content::Content;

/// The auth plane's port. A Myco constant, not negotiated: peers agree by running
/// the same version, which is fine pre-1.0. Chosen just past the relay's 4870 and
/// clear of 4869, which public Nostr relays commonly take.
pub const AUTH_PORT: u16 = 4871;

/// A signed pair event is a few hundred bytes; anything larger is not one.
const MAX_BODY: usize = 8 * 1024;

/// Sustained rate per source address, and the burst it may spend at once.
/// Pairing is user-driven — a human never exceeds this — so the limit only ever
/// bites on something automated.
const RATE_PER_SEC: f64 = 1.0;
const RATE_BURST: f64 = 5.0;

/// Ceiling on pair requests being handled at once, across all sources, so a burst
/// cannot saturate a BLE radio.
const MAX_IN_FLIGHT: usize = 8;

/// Per-source token bucket. A source that empties its bucket is delayed, not
/// banned: there is no identity to ban that costs a mesh peer anything to
/// replace, and a failed attempt is not evidence of bad intent. Failures cost the
/// same as successes for the same reason.
struct Bucket {
    tokens: f64,
    last: Instant,
}

#[derive(Default)]
struct Limiter {
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
    in_flight: Mutex<usize>,
}

impl Limiter {
    /// Take a token for `ip`, or `false` if the bucket is empty.
    fn take(&self, ip: IpAddr, now: Instant) -> bool {
        let mut buckets = self.buckets.lock().unwrap();
        // Bound the table itself: without this a hostile peer could grow it by
        // dialling from many addresses. Sources at full tokens are idle by
        // definition, so dropping them loses nothing.
        if buckets.len() > 1024 {
            buckets.retain(|_, b| b.tokens < RATE_BURST);
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

/// Shared handler state.
#[derive(Clone)]
struct AuthState {
    content: Arc<Content>,
    limiter: Arc<Limiter>,
}

/// Bind the auth listener. IPv6 binds are `IPV6_V6ONLY` so `[::]:port` does not
/// collide with a loopback squatter, matching the other mesh servers.
pub fn bind(addr: SocketAddr) -> anyhow::Result<tokio::net::TcpListener> {
    let domain = if addr.is_ipv6() {
        socket2::Domain::IPV6
    } else {
        socket2::Domain::IPV4
    };
    let socket = socket2::Socket::new(domain, socket2::Type::STREAM, Some(socket2::Protocol::TCP))?;
    if addr.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(128)?;
    Ok(tokio::net::TcpListener::from_std(socket.into())?)
}

/// Serve the auth plane on an already-bound listener.
pub async fn serve_on(
    content: Arc<Content>,
    listener: tokio::net::TcpListener,
) -> anyhow::Result<()> {
    let state = AuthState {
        content,
        limiter: Arc::new(Limiter::default()),
    };
    let app = Router::new()
        .route("/pair", post(pair))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(state);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

fn json(status: StatusCode, body: serde_json::Value) -> Response {
    (status, axum::Json(body)).into_response()
}

async fn pair(
    State(st): State<AuthState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    body: String,
) -> Response {
    let ip = addr.ip();

    if !st.limiter.take(ip, Instant::now()) {
        return json(
            StatusCode::TOO_MANY_REQUESTS,
            serde_json::json!({ "status": "slow down" }),
        );
    }
    // Concurrency ceiling, released when this handler returns.
    let _guard = match InFlight::acquire(&st.limiter) {
        Some(g) => g,
        None => {
            return json(
                StatusCode::SERVICE_UNAVAILABLE,
                serde_json::json!({ "status": "busy" }),
            )
        }
    };

    let Ok(event) = serde_json::from_str::<Event>(&body) else {
        return json(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "status": "malformed" }),
        );
    };

    // The signature is the identity proof: it is what says this pair request
    // really comes from that npub, and it is checked before anything is touched.
    if event.verify().is_err() {
        return json(
            StatusCode::FORBIDDEN,
            serde_json::json!({ "status": "declined", "reason": "bad signature" }),
        );
    }

    // Freshness is now an explicit check rather than a side effect of the store
    // GCing expired events, since nothing is stored any more.
    if myco_relay::expiration(&event).is_some_and(|exp| exp <= crate::content::now_secs()) {
        return json(
            StatusCode::FORBIDDEN,
            serde_json::json!({ "status": "declined", "reason": "expired" }),
        );
    }

    let kind = event.kind.as_u16();
    if !matches!(
        kind,
        crate::content::KIND_PAIR_REQUEST
            | crate::content::KIND_PAIR_ACCEPT
            | crate::content::KIND_PAIR_REMOVE
    ) {
        return json(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "status": "declined", "reason": "not a pairing event" }),
        );
    }

    // Membership is committed **before** we reply, so a peer that dials the relay
    // the instant it sees a 200 is already admitted by the gate.
    st.content.handle_pair_event(&event);

    let status = match kind {
        crate::content::KIND_PAIR_ACCEPT => (StatusCode::OK, "paired"),
        crate::content::KIND_PAIR_REMOVE => (StatusCode::OK, "unpaired"),
        // A request needs a human, so it is accepted-for-processing, not done.
        _ => (StatusCode::ACCEPTED, "pending"),
    };
    tracing::info!(peer = %ip, kind, status = status.1, "auth: pair handled");
    json(status.0, serde_json::json!({ "status": status.1 }))
}

/// RAII guard for the in-flight ceiling.
struct InFlight(Arc<Limiter>);

impl InFlight {
    fn acquire(limiter: &Arc<Limiter>) -> Option<Self> {
        let mut n = limiter.in_flight.lock().unwrap();
        if *n >= MAX_IN_FLIGHT {
            return None;
        }
        *n += 1;
        Some(Self(limiter.clone()))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        *self.0.in_flight.lock().unwrap() -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, n))
    }

    /// The bucket has to allow a normal human pairing burst and still stop a
    /// runaway, and it must refill over time rather than latching shut.
    #[test]
    fn rate_limit_allows_a_burst_then_refills() {
        let limiter = Limiter::default();
        let t0 = Instant::now();

        for i in 0..RATE_BURST as usize {
            assert!(limiter.take(ip(1), t0), "burst token {i} should be allowed");
        }
        assert!(!limiter.take(ip(1), t0), "burst exhausted");

        // A second later, one token is back.
        let t1 = t0 + std::time::Duration::from_secs(1);
        assert!(limiter.take(ip(1), t1), "bucket refills");
        assert!(!limiter.take(ip(1), t1));

        // One source running dry does not affect another.
        assert!(limiter.take(ip(2), t1), "limits are per source");
    }

    /// An unpaired stranger must be able to pair here — and only here.
    ///
    /// This is the whole point of the auth plane: the handshake lands, membership
    /// is committed, and nothing was written to the event store on the way. The
    /// relay's gate refusing the same event is the other half, covered by
    /// `content::tests`.
    #[tokio::test]
    async fn a_stranger_can_pair_and_nothing_is_stored() {
        use nostr::nips::nip19::ToBech32;

        let dir = std::env::temp_dir().join(format!("myco-auth-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_on(content.clone(), listener));

        // A device we have never seen asks to pair.
        let stranger = nostr::Keys::generate();
        let us = nostr::Keys::generate().public_key().to_bech32().unwrap();
        let event = crate::content::build_pair_event(
            &stranger,
            crate::content::KIND_PAIR_REQUEST,
            &us,
            "Stranger",
            "s3cret",
        )
        .expect("build pair request");

        let resp = reqwest::Client::new()
            .post(format!("http://{addr}/pair"))
            .body(serde_json::to_string(&event).unwrap())
            .send()
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            reqwest::StatusCode::ACCEPTED,
            "a request is pending a human, not yet paired"
        );

        let pending = content.pending_pairs_snapshot();
        assert_eq!(pending.len(), 1, "the request surfaced for the user");
        assert_eq!(pending[0].npub, stranger.public_key().to_bech32().unwrap());
        assert_eq!(pending[0].secret, "s3cret");

        assert_eq!(
            content
                .relay_store()
                .expect("embedded store in tests")
                .count(),
            0,
            "the handshake is control traffic — nothing reaches the event store"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tampered pair event must be refused. The signature is the only thing
    /// tying a request to an npub, so this is the door itself.
    #[tokio::test]
    async fn a_forged_pair_event_is_refused() {
        use nostr::nips::nip19::ToBech32;

        let dir = std::env::temp_dir().join(format!("myco-auth-forge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let content = Arc::new(Content::open(&dir).unwrap());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_on(content.clone(), listener));

        let stranger = nostr::Keys::generate();
        let us = nostr::Keys::generate().public_key().to_bech32().unwrap();
        let event = crate::content::build_pair_event(
            &stranger,
            crate::content::KIND_PAIR_ACCEPT,
            &us,
            "Stranger",
            "",
        )
        .unwrap();
        // Claim a different display name after signing.
        let mut raw = serde_json::to_value(&event).unwrap();
        raw["tags"] = serde_json::json!([["n", "Someone Else"]]);

        let resp = reqwest::Client::new()
            .post(format!("http://{addr}/pair"))
            .body(raw.to_string())
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN);
        assert!(
            content.circle_snapshot().is_empty(),
            "a forged accept must not put anyone in the circle"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The ceiling must release as handlers finish, or the service latches at
    /// "busy" after the first burst.
    #[test]
    fn in_flight_ceiling_releases() {
        let limiter = Arc::new(Limiter::default());
        let guards: Vec<_> = (0..MAX_IN_FLIGHT)
            .map(|_| InFlight::acquire(&limiter).expect("under the ceiling"))
            .collect();
        assert!(
            InFlight::acquire(&limiter).is_none(),
            "at the ceiling, further requests are refused"
        );
        drop(guards);
        assert!(
            InFlight::acquire(&limiter).is_some(),
            "capacity returns once handlers finish"
        );
    }
}
