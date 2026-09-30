//! Which internet relays (and public Blossom servers) are not worth dialling
//! right now — one list for the whole process — and whether the internet
//! has been heard from lately.
//!
//! On a phone, a relay that answered 403 a second ago will answer 403 again,
//! and one whose certificate is wrong will still be wrong. Every round that
//! dials it anyway pays a TLS handshake or a timeout for nothing, and a
//! napplet's answer or an app lookup waits on it. So a dial that fails in a
//! way that will not fix itself in seconds puts the relay on this list for a
//! while, and every internet dial path checks the list first: a relay on it
//! is not dialled and reads as "finished, nothing" at once.
//!
//! ## The rules
//!
//! - **The relay said no: listed at once.** An HTTP 403, 404 or any 5xx
//!   (530 included) at the WebSocket upgrade — or a 5xx from a Blossom
//!   server. An HTTP answer proves the phone is online, so this is about
//!   the relay, whatever else is going on. 1 min, doubling per repeat, up
//!   to 30 min. Two exceptions: **503** ("busy") is capped at 5 min and
//!   honours a `Retry-After` in seconds; **511** ("network authentication
//!   required") is a captive portal talking, not the relay, and is ignored.
//! - **The relay could not be reached: listed at once, if the phone is
//!   online.** A name that does not resolve, a certificate that does not
//!   verify, a refused connection. (Those are the transport failures caught:
//!   DNS and refusals by kind or message, TLS only when it is a certificate
//!   failure — with rustls a handshake failure arrives as an I/O error, and
//!   tungstenite's own `Tls` error carries only an invalid DNS name.)
//!   Offline, every relay fails like this at once, so these count only
//!   while some *other* internet dial succeeded in the last
//!   [`INTERNET_WORKS_WITHIN`]; without that nothing is recorded at all, and
//!   the backoff does not climb while the phone is offline.
//! - **Unless several relays fail that way together.** [`BURST_KEYS`] or
//!   more distinct relays failing to be reached within [`BURST_WINDOW`] is
//!   the network, not the relays: a handover between networks, or a captive
//!   portal (which answers every relay's TLS with its own certificate). The
//!   burst clears "online" and "heard", undoes the listings it made, and
//!   lists nothing until it has been quiet for [`BURST_WINDOW`].
//! - **Timeouts: listed after [`TIMEOUTS_TO_SKIP`] in a row**, under the
//!   same online gate, and only timeouts that say something about the
//!   relay: the connection itself never came up (a relay that connected
//!   and is slow to answer is slow, not gone), within a deadline of at
//!   least [`COUNTED_TIMEOUT_MIN`] — never a napplet's short `timeoutMs`,
//!   never a round's shared cutoff. 30 s, doubling, up to 5 min: a slow
//!   relay is more often slow for a moment than gone.
//! - **A success clears the entry.** Mesh successes (`.fips`, FIPS
//!   `fd00::/8`, a Circle member's Blossom) do not count as "the phone is
//!   online": the mesh works with no internet at all.
//! - **Never on it:** Circle members' relays and Blossom (the peer pool
//!   never consults it, and a mesh address reaching a shared helper is
//!   exempt), and the custom relay or Blossom set in Storage (their
//!   clients, `remote_backend` and `remote_blobs`, never consult it).
//! - **Logged once** per skip period, when the relay is put on the list.
//!
//! ## "Heard" and the internet breaker
//!
//! The internet breaker (`Content::note_internet_round`) notices a *round*
//! in which every internet lane failed and stops trying the internet for a
//! moment. It must not trip on one broken relay while the rest of the
//! internet is answering, so it asks [`internet_heard_since`]: did anything
//! on the internet answer since the round began? "Heard" is wider than
//! "online": any HTTP status (a 429, a 401, a relay's 502 — anything but a
//! captive portal's 511), a certificate (it had to arrive to fail), or a
//! refused connection (a host answered) all count, as does any success.
//! A burst (above) clears it: a captive portal's certificates are the one
//! case where a certificate arriving means the internet is *not* there.
//!
//! In memory only, pruned of what has expired and capped at
//! [`MAX_ENTRIES`]. A restart is rare and a relay that was down may be up by
//! then; a persisted list would hide a recovered relay for up to 30 min
//! after launch to save one failed dial per relay.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// First skip when a relay said no or could not be reached.
const HARD_FIRST: Duration = Duration::from_secs(60);
/// Longest such skip.
const HARD_MAX: Duration = Duration::from_secs(30 * 60);
/// First skip after [`TIMEOUTS_TO_SKIP`] timeouts in a row.
const TIMEOUT_FIRST: Duration = Duration::from_secs(30);
/// Longest skip for timeouts, and for a busy (503) relay.
const TIMEOUT_MAX: Duration = Duration::from_secs(5 * 60);
/// The shortest skip a `Retry-After` is taken to ask for.
const RETRY_AFTER_MIN: Duration = Duration::from_secs(10);
/// Consecutive timeouts that put a relay on the list. One is a moment of
/// bad signal or a relay under load; two in a row, while other relays
/// answer, is a relay not answering.
pub(crate) const TIMEOUTS_TO_SKIP: u32 = 2;
/// A deadline shorter than this says more about the caller's patience than
/// the relay: the default internet dial bounds are 8–15 s, a napplet may
/// ask for 500 ms.
pub(crate) const COUNTED_TIMEOUT_MIN: Duration = Duration::from_secs(8);
/// "The phone is online": some other internet dial succeeded this recently.
const INTERNET_WORKS_WITHIN: Duration = Duration::from_secs(2 * 60);
/// Distinct relays failing to be reached within [`BURST_WINDOW`] that make
/// it the network's fault rather than theirs.
pub(crate) const BURST_KEYS: usize = 3;
/// See [`BURST_KEYS`].
pub(crate) const BURST_WINDOW: Duration = Duration::from_secs(10);
/// Most relays remembered at once. A napplet naming relays can mint new
/// URLs; a bounded map cannot be grown by it.
const MAX_ENTRIES: usize = 512;

/// Why a relay could not be reached — the three kinds this list catches.
pub(crate) const WHY_DNS: &str = "name does not resolve";
pub(crate) const WHY_CERT: &str = "bad TLS certificate";
pub(crate) const WHY_REFUSED: &str = "connection refused";

/// Why a dial failed, as far as the list cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Failure {
    /// The relay answered, with a refusal or an error status. Counts
    /// whatever the phone's connectivity.
    Refused(&'static str),
    /// The relay answered 503, perhaps saying for how long. As `Refused`,
    /// with a shorter ceiling.
    Busy(Option<Duration>),
    /// The relay could not be reached: [`WHY_DNS`], [`WHY_CERT`],
    /// [`WHY_REFUSED`]. Counts only while the phone is online, and not in a
    /// burst.
    Unreachable(&'static str),
    /// The connection did not come up by a counted deadline. Counts only
    /// while the phone is online, and only [`TIMEOUTS_TO_SKIP`] in a row.
    Timeout,
}

#[derive(Debug)]
struct Entry {
    /// Skipped until then, if on the list.
    until: Option<Instant>,
    /// The last skip's length, to double from.
    last_skip: Option<Duration>,
    /// Timeouts since the last success or skip.
    timeouts: u32,
    /// When a dial to it last connected.
    connected: Option<Instant>,
    /// Last change, for eviction at the cap.
    touched: Instant,
}

impl Entry {
    fn new(now: Instant) -> Self {
        Self {
            until: None,
            last_skip: None,
            timeouts: 0,
            connected: None,
            touched: now,
        }
    }

    /// Nothing about it matters any more: no skip running, nothing counted.
    fn is_spent(&self, now: Instant) -> bool {
        self.until.is_none_or(|until| until <= now) && self.timeouts == 0
    }
}

/// Unreachable failures of the last [`BURST_WINDOW`], and whether a burst
/// is on.
#[derive(Default)]
struct Bursts {
    /// When, which relay, and whether that failure listed it.
    recent: Vec<(Instant, String, bool)>,
    /// A burst is on until then.
    until: Option<Instant>,
}

/// The list. One per process ([`current`]); tests make their own to drive
/// the clock.
#[derive(Default)]
pub(crate) struct RelayHealth {
    entries: Mutex<HashMap<String, Entry>>,
    /// Relays the pool holds a socket to right now, by key, with how many.
    /// A pooled read rides a connection that came up before it started, so
    /// "connected during this call" is not the only way to be up.
    open: Mutex<HashMap<String, usize>>,
    last_success: Mutex<Option<Instant>>,
    heard: Mutex<Option<Instant>>,
    bursts: Mutex<Bursts>,
}

/// The same relay however it was written: the scheme and host in lower case
/// (the parser does that), the scheme's default port dropped, a trailing
/// slash dropped — the rule [`crate::ip_source::same_relay`] applies, plus
/// the port. The path keeps its case.
pub(crate) fn key(url: &str) -> String {
    let url = url.trim();
    let Ok(parsed) = nostr::Url::parse(url) else {
        return url.trim_end_matches('/').to_string();
    };
    let Some(host) = parsed.host_str() else {
        return url.trim_end_matches('/').to_string();
    };
    // `port()` is `None` for the scheme's default port.
    let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
    let mut out = format!("{}://{host}{port}{}", parsed.scheme(), parsed.path());
    if let Some(query) = parsed.query() {
        out.push('?');
        out.push_str(query);
    }
    out.trim_end_matches('/').to_string()
}

/// A mesh address: a `.fips` name or a FIPS `fd00::/8` address. Never
/// listed, and its successes are not "the phone is online" — the mesh has
/// its own reachability and backoff (the peer relay pool).
fn is_mesh(url: &str) -> bool {
    use nostr::types::url::Host;
    let Ok(parsed) = nostr::Url::parse(url.trim()) else {
        return false;
    };
    match parsed.host() {
        Some(Host::Domain(d)) => d.to_ascii_lowercase().ends_with(".fips"),
        Some(Host::Ipv6(v6)) => v6.segments()[0] & 0xff00 == 0xfd00,
        _ => false,
    }
}

/// The process's list. In tests, one per test thread — each `#[tokio::test]`
/// runs on its own — so a loopback port another test recycled cannot find a
/// mock relay already skipped; [`reset`] empties it.
#[cfg(not(test))]
pub(crate) fn current() -> Arc<RelayHealth> {
    static GLOBAL: std::sync::OnceLock<Arc<RelayHealth>> = std::sync::OnceLock::new();
    GLOBAL
        .get_or_init(|| Arc::new(RelayHealth::default()))
        .clone()
}

#[cfg(test)]
thread_local! {
    static LOCAL: std::cell::RefCell<Arc<RelayHealth>> =
        std::cell::RefCell::new(Arc::new(RelayHealth::default()));
}

#[cfg(test)]
pub(crate) fn current() -> Arc<RelayHealth> {
    LOCAL.with(|l| l.borrow().clone())
}

/// A fresh list for this test thread.
#[cfg(test)]
pub(crate) fn reset() {
    LOCAL.with(|l| *l.borrow_mut() = Arc::new(RelayHealth::default()));
}

impl RelayHealth {
    /// Whether `url` is on the list at `now`.
    pub(crate) fn skipped_at(&self, url: &str, now: Instant) -> bool {
        if is_mesh(url) {
            return false;
        }
        self.entries
            .lock()
            .unwrap()
            .get(&key(url))
            .and_then(|e| e.until)
            .is_some_and(|until| now < until)
    }

    pub(crate) fn skipped(&self, url: &str) -> bool {
        self.skipped_at(url, Instant::now())
    }

    /// A dial to `url` connected (and, for a fetch, was answered): off the
    /// list, counters reset — and, unless it is a mesh address, the phone is
    /// online.
    pub(crate) fn succeeded_at(&self, url: &str, now: Instant) {
        if is_mesh(url) {
            return;
        }
        let mut entries = self.entries.lock().unwrap();
        let mut entry = Entry::new(now);
        entry.connected = Some(now);
        Self::insert(&mut entries, key(url), entry, now);
        *self.last_success.lock().unwrap() = Some(now);
        *self.heard.lock().unwrap() = Some(now);
    }

    pub(crate) fn succeeded(&self, url: &str) {
        self.succeeded_at(url, Instant::now())
    }

    /// Something on the internet answered at `now` without a success —
    /// see "Heard" in the module docs.
    pub(crate) fn heard_at(&self, url: &str, now: Instant) {
        if !is_mesh(url) {
            *self.heard.lock().unwrap() = Some(now);
        }
    }

    /// Whether anything on the internet answered at or after `since`.
    pub(crate) fn heard_since(&self, since: Instant) -> bool {
        self.heard.lock().unwrap().is_some_and(|at| at >= since)
    }

    /// Whether a dial to `url` connected at or after `since` — how a timeout
    /// tells "never came up" from "came up and was slow".
    fn connected_since(&self, url: &str, since: Instant) -> bool {
        if self.open.lock().unwrap().contains_key(&key(url)) {
            return true;
        }
        self.entries
            .lock()
            .unwrap()
            .get(&key(url))
            .and_then(|e| e.connected)
            .is_some_and(|at| at >= since)
    }

    /// Note an unreachable failure of `k` for the burst rule. Returns
    /// whether it is part of a burst — the network, not the relay.
    fn in_burst(&self, k: &str, now: Instant) -> bool {
        let mut bursts = self.bursts.lock().unwrap();
        bursts
            .recent
            .retain(|(at, _, _)| now.saturating_duration_since(*at) < BURST_WINDOW);
        bursts.recent.push((now, k.to_string(), false));
        if bursts.until.is_some_and(|until| now < until) {
            bursts.until = Some(now + BURST_WINDOW);
            return true;
        }
        let mut keys: Vec<&str> = bursts.recent.iter().map(|(_, k, _)| k.as_str()).collect();
        keys.sort_unstable();
        keys.dedup();
        if keys.len() < BURST_KEYS {
            return false;
        }
        // The network, not the relays: forget "online" and "heard", and undo
        // what this burst listed.
        bursts.until = Some(now + BURST_WINDOW);
        let undo: Vec<String> = bursts
            .recent
            .iter()
            .filter(|(_, _, listed)| *listed)
            .map(|(_, k, _)| k.clone())
            .collect();
        drop(bursts);
        *self.last_success.lock().unwrap() = None;
        *self.heard.lock().unwrap() = None;
        let mut entries = self.entries.lock().unwrap();
        for k in &undo {
            entries.remove(k);
        }
        tracing::info!(
            undone = undo.len(),
            "several relays unreachable at once: the network, not the relays; nothing listed"
        );
        true
    }

    /// A dial to `url` failed. Returns whether this put it on the list.
    pub(crate) fn failed_at(&self, url: &str, failure: Failure, now: Instant) -> bool {
        if is_mesh(url) {
            return false;
        }
        let k = key(url);
        if let Failure::Unreachable(why) = failure {
            if self.in_burst(&k, now) {
                return false;
            }
            // A certificate arrived, or a host refused: something answered.
            if why == WHY_CERT || why == WHY_REFUSED {
                *self.heard.lock().unwrap() = Some(now);
            }
        }
        if matches!(failure, Failure::Refused(_) | Failure::Busy(_)) {
            *self.heard.lock().unwrap() = Some(now);
        }
        let online = self
            .last_success
            .lock()
            .unwrap()
            .is_some_and(|at| now.saturating_duration_since(at) < INTERNET_WORKS_WITHIN);
        if matches!(failure, Failure::Unreachable(_) | Failure::Timeout) && !online {
            // Offline, or not known to be online: every relay fails alike,
            // and none of it is this relay's fault. Record nothing.
            return false;
        }
        let mut entries = self.entries.lock().unwrap();
        if !entries.contains_key(&k) {
            Self::insert(&mut entries, k.clone(), Entry::new(now), now);
        }
        let entry = entries.get_mut(&k).expect("inserted above");
        entry.touched = now;
        if entry.until.is_some_and(|until| now < until) {
            // Already skipped: a dial that raced the listing changes nothing.
            return false;
        }
        let (first, max) = match failure {
            Failure::Refused(_) | Failure::Unreachable(_) => (HARD_FIRST, HARD_MAX),
            Failure::Busy(_) => (HARD_FIRST, TIMEOUT_MAX),
            Failure::Timeout => {
                entry.timeouts += 1;
                if entry.timeouts < TIMEOUTS_TO_SKIP {
                    return false;
                }
                (TIMEOUT_FIRST, TIMEOUT_MAX)
            }
        };
        let skip = match (failure, entry.last_skip) {
            (Failure::Busy(Some(asked)), _) => asked.clamp(RETRY_AFTER_MIN, HARD_MAX),
            (_, Some(last)) => (last * 2).clamp(first, max),
            (_, None) => first,
        };
        entry.until = Some(now + skip);
        entry.last_skip = Some(skip);
        entry.timeouts = 0;
        drop(entries);
        if matches!(failure, Failure::Unreachable(_)) {
            if let Some(last) = self.bursts.lock().unwrap().recent.last_mut() {
                last.2 = true;
            }
        }
        let why = match failure {
            Failure::Refused(why) | Failure::Unreachable(why) => why,
            Failure::Busy(_) => "HTTP 503",
            Failure::Timeout => "timed out repeatedly",
        };
        tracing::info!(
            url,
            why,
            skip_secs = skip.as_secs(),
            "relay put on the skip list"
        );
        true
    }

    pub(crate) fn failed(&self, url: &str, failure: Failure) -> bool {
        self.failed_at(url, failure, Instant::now())
    }

    /// Insert, first pruning what is spent and, at the cap, evicting the
    /// least recently touched entries.
    fn insert(entries: &mut HashMap<String, Entry>, k: String, entry: Entry, now: Instant) {
        if !entries.contains_key(&k) && entries.len() >= MAX_ENTRIES {
            // A spent entry's only content is `connected`, which a timeout
            // compares against a moment ago: losing an old one loses nothing.
            entries.retain(|_, e| {
                !e.is_spent(now)
                    || e.connected.is_some_and(|at| {
                        now.saturating_duration_since(at) < COUNTED_TIMEOUT_MIN * 4
                    })
            });
            while entries.len() >= MAX_ENTRIES {
                let oldest = entries
                    .iter()
                    .min_by_key(|(_, e)| e.touched)
                    .map(|(k, _)| k.clone());
                match oldest {
                    Some(oldest) => entries.remove(&oldest),
                    None => break,
                };
            }
        }
        entries.insert(k, entry);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

/// Whether `url` is on the process-wide list now.
pub(crate) fn is_skipped(url: &str) -> bool {
    current().skipped(url)
}

/// Whether anything on the internet answered at or after `since` — the
/// internet breaker's question. See "Heard" in the module docs.
pub(crate) fn internet_heard_since(since: Instant) -> bool {
    current().heard_since(since)
}

/// Fail at once, without dialling, if `url` is listed — the first line of
/// every internet dial.
pub(crate) fn check(url: &str) -> anyhow::Result<()> {
    if is_skipped(url) {
        anyhow::bail!("relay skipped: recently failed");
    }
    Ok(())
}

/// Record how a WebSocket dial to `url` ended: connected clears it, a
/// refusal or an unreachable relay lists it (see the rules), any HTTP answer
/// is "heard", anything else (a protocol hiccup, our own skip) leaves it
/// alone.
pub(crate) fn record_ws<T>(url: &str, result: &anyhow::Result<T>) {
    use tokio_tungstenite::tungstenite::Error as WsError;
    match result {
        Ok(_) => current().succeeded(url),
        Err(e) => {
            if let Some(WsError::Http(response)) = e.downcast_ref::<WsError>() {
                if http_heard(response.status().as_u16()) {
                    current().heard_at(url, Instant::now());
                }
            }
            if let Some(failure) = classify_ws(e) {
                current().failed(url, failure);
            }
        }
    }
}

/// The relay pool opened a socket to `url` (see `relay_pool`); until the
/// matching [`socket_closed`], a timeout on it is the relay being slow, not
/// gone.
pub(crate) fn socket_opened(url: &str) {
    let health = current();
    *health.open.lock().unwrap().entry(key(url)).or_default() += 1;
}

/// The socket [`socket_opened`] counted is gone.
pub(crate) fn socket_closed(url: &str) {
    let health = current();
    let mut open = health.open.lock().unwrap();
    let k = key(url);
    if let Some(n) = open.get_mut(&k) {
        *n -= 1;
        if *n == 0 {
            open.remove(&k);
        }
    }
}

/// Record a Blossom server's HTTP answer: 5xx lists it (503 as busy, with
/// its `Retry-After`), anything else short of 511 is the server up and
/// answering.
pub(crate) fn record_http_status(url: &str, status: u16, retry_after: Option<&str>) {
    match status {
        503 => {
            current().failed(url, Failure::Busy(parse_retry_after(retry_after)));
        }
        511 => {}
        500..=599 => {
            current().failed(url, Failure::Refused("HTTP 5xx"));
        }
        _ => current().succeeded(url),
    }
}

/// Record a Blossom fetch that got no HTTP answer.
pub(crate) fn record_http_error(url: &str, error: &reqwest::Error) {
    if let Some(failure) = classify_http(error) {
        current().failed(url, failure);
    }
}

/// A relay name that did not resolve, found before any dial (the private-
/// address guard resolves first).
pub(crate) fn record_dns_failure(url: &str) {
    current().failed(url, Failure::Unreachable(WHY_DNS));
}

/// As [`tokio::time::timeout`], also counting a timeout against `url` —
/// when it may say something about the relay: `duration` is at least
/// [`COUNTED_TIMEOUT_MIN`] and the connection never came up.
///
/// Round-end cutoffs shared by several relays use `tokio::time::timeout_at`
/// directly: running out of a shared budget is not the relay's doing.
pub(crate) async fn timeout<F: std::future::Future>(
    url: &str,
    duration: Duration,
    fut: F,
) -> Result<F::Output, tokio::time::error::Elapsed> {
    timeout_counting(url, duration, COUNTED_TIMEOUT_MIN, fut).await
}

/// [`timeout`] with the counted minimum given — for tests.
pub(crate) async fn timeout_counting<F: std::future::Future>(
    url: &str,
    duration: Duration,
    counted_min: Duration,
    fut: F,
) -> Result<F::Output, tokio::time::error::Elapsed> {
    let started = Instant::now();
    let out = tokio::time::timeout(duration, fut).await;
    let health = current();
    if out.is_err() && duration >= counted_min && !health.connected_since(url, started) {
        health.failed(url, Failure::Timeout);
    }
    out
}

/// Whether an HTTP status means the internet answered: anything but a
/// captive portal's 511.
fn http_heard(status: u16) -> bool {
    status != 511
}

/// `Retry-After` in seconds; the HTTP-date form is not worth a parser here.
fn parse_retry_after(value: Option<&str>) -> Option<Duration> {
    value?.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Which failures of a WebSocket dial count, and how.
///
/// With rustls, a TLS handshake failure (a bad certificate) arrives as an
/// I/O error and is caught by [`classify_io`]; tungstenite's own `Tls` error
/// carries only an invalid DNS name, and is not a relay failure worth
/// listing.
pub(crate) fn classify_ws(error: &anyhow::Error) -> Option<Failure> {
    use tokio_tungstenite::tungstenite::Error as WsError;
    let ws = error.downcast_ref::<WsError>()?;
    match ws {
        WsError::Http(response) => {
            let status = response.status().as_u16();
            if status == 503 {
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok());
                return Some(Failure::Busy(parse_retry_after(retry_after)));
            }
            classify_status(status).map(Failure::Refused)
        }
        WsError::Io(io) => classify_io(io).map(Failure::Unreachable),
        _ => None,
    }
}

/// 403 and 404 at the upgrade: this relay will not serve us. 5xx: it is
/// broken, or behind a proxy that says so (Cloudflare's 530) — except 511,
/// a captive portal's, which says nothing about the relay.
pub(crate) fn classify_status(status: u16) -> Option<&'static str> {
    match status {
        403 => Some("HTTP 403"),
        404 => Some("HTTP 404"),
        511 => None,
        500..=599 => Some("HTTP 5xx"),
        _ => None,
    }
}

/// An I/O error that means the relay could not be reached: a name that
/// does not resolve, a certificate that does not verify, or a refused
/// connection. Other TLS failures are not caught.
///
/// DNS and certificates are matched on the message: std reports a failed
/// lookup as an `io::Error` of an unstable kind with the text "failed to
/// lookup address information", and tokio-rustls hands a handshake failure
/// back as `InvalidData` wrapping the rustls error ("invalid peer
/// certificate: …"). Neither has a stable type to match; a message that
/// changes only stops the list catching that case — which the end-to-end
/// tests would notice.
pub(crate) fn classify_io(io: &std::io::Error) -> Option<&'static str> {
    if io.kind() == std::io::ErrorKind::ConnectionRefused {
        return Some(WHY_REFUSED);
    }
    let text = io.to_string();
    if text.contains("failed to lookup address") {
        return Some(WHY_DNS);
    }
    if io.kind() == std::io::ErrorKind::InvalidData && text.contains("certificate") {
        return Some(WHY_CERT);
    }
    None
}

/// As [`classify_ws`], for an HTTP fetch (a Blossom server): the transport
/// failures. A status is the caller's to judge — a 404 is an answer there.
pub(crate) fn classify_http(error: &reqwest::Error) -> Option<Failure> {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(e) = source {
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            if let Some(why) = classify_io(io) {
                return Some(Failure::Unreachable(why));
            }
        }
        let text = e.to_string();
        if text.contains("failed to lookup address") || text.contains("dns error") {
            return Some(Failure::Unreachable(WHY_DNS));
        }
        if text.contains("invalid peer certificate") {
            return Some(Failure::Unreachable(WHY_CERT));
        }
        source = e.source();
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const R: &str = "wss://relay.example.com";
    const OTHER: &str = "wss://other.example";

    fn listed(h: &RelayHealth, url: &str, now: Instant) -> bool {
        h.skipped_at(url, now)
    }

    /// A health list that has seen the internet work at `t`.
    fn online_at(t: Instant) -> RelayHealth {
        let h = RelayHealth::default();
        h.succeeded_at(OTHER, t);
        h
    }

    #[test]
    fn a_refusal_skips_at_once_and_backs_off_doubling_to_the_cap() {
        // No success seen at all: a refusal still counts, it proves the
        // phone is online.
        let h = RelayHealth::default();
        let t0 = Instant::now();
        assert!(h.failed_at(R, Failure::Refused("HTTP 403"), t0));
        assert!(listed(&h, R, t0 + Duration::from_secs(59)));
        assert!(!listed(&h, R, t0 + Duration::from_secs(61)));

        // Fails again once the skip is over: twice as long.
        let t1 = t0 + Duration::from_secs(61);
        assert!(h.failed_at(R, Failure::Refused("HTTP 403"), t1));
        assert!(listed(&h, R, t1 + Duration::from_secs(119)));
        assert!(!listed(&h, R, t1 + Duration::from_secs(121)));

        // And never past 30 minutes.
        let mut t = t1;
        for _ in 0..10 {
            t += Duration::from_secs(31 * 60);
            h.failed_at(R, Failure::Refused("HTTP 5xx"), t);
        }
        assert!(listed(&h, R, t + Duration::from_secs(29 * 60)));
        assert!(!listed(&h, R, t + Duration::from_secs(30 * 60 + 1)));
    }

    /// The review's case: a phone with no signal fails DNS (or TLS, or a
    /// refused connection) on every relay. Nothing is listed, and nothing
    /// climbs, however long it stays offline.
    #[test]
    fn offline_nothing_unreachable_is_recorded_and_nothing_climbs() {
        let h = RelayHealth::default();
        let t0 = Instant::now();
        for i in 0..20 {
            let t = t0 + Duration::from_secs(i * 120);
            for why in [
                "name does not resolve",
                "bad TLS certificate",
                "connection refused",
            ] {
                assert!(!h.failed_at(R, Failure::Unreachable(why), t));
            }
            assert!(!h.failed_at(R, Failure::Timeout, t));
        }
        assert!(!listed(&h, R, t0 + Duration::from_secs(20 * 120)));
        assert_eq!(h.len(), 0, "an offline failure left an entry");

        // Back online: the first unreachable failure is the first step.
        let back = t0 + Duration::from_secs(3600);
        h.succeeded_at(OTHER, back);
        assert!(h.failed_at(R, Failure::Unreachable("name does not resolve"), back));
        assert!(!listed(&h, R, back + Duration::from_secs(61)));
    }

    #[test]
    fn online_an_unreachable_relay_skips_at_once() {
        let t0 = Instant::now();
        let h = online_at(t0);
        assert!(h.failed_at(R, Failure::Unreachable("bad TLS certificate"), t0));
        assert!(listed(&h, R, t0 + Duration::from_secs(30)));
    }

    /// Only an internet success says the phone is online: a Circle member's
    /// relay or Blossom answering over the mesh does not.
    #[test]
    fn a_mesh_success_is_not_the_internet_working() {
        let h = RelayHealth::default();
        let t0 = Instant::now();
        for mesh in [
            "ws://npub1abc.fips:4870",
            "http://npub1abc.fips:24243",
            "http://[fd00::1234]:24243",
        ] {
            h.succeeded_at(mesh, t0);
        }
        assert!(!h.failed_at(R, Failure::Unreachable("name does not resolve"), t0));
        assert!(!h.failed_at(R, Failure::Timeout, t0));
        assert!(!h.failed_at(R, Failure::Timeout, t0));
        assert!(!listed(&h, R, t0));
    }

    #[test]
    fn a_success_clears_the_entry_and_the_backoff() {
        let h = RelayHealth::default();
        let t0 = Instant::now();
        h.failed_at(R, Failure::Refused("HTTP 404"), t0);
        h.succeeded_at(R, t0 + Duration::from_secs(1));
        assert!(!listed(&h, R, t0 + Duration::from_secs(2)));
        // Back to the first step, not a doubled one.
        let t1 = t0 + Duration::from_secs(3);
        h.failed_at(R, Failure::Refused("HTTP 404"), t1);
        assert!(!listed(&h, R, t1 + Duration::from_secs(61)));
    }

    #[test]
    fn timeouts_skip_after_two_in_a_row_while_online() {
        let t0 = Instant::now();
        let h = online_at(t0);
        assert!(!h.failed_at(R, Failure::Timeout, t0 + Duration::from_secs(1)));
        assert!(!listed(&h, R, t0 + Duration::from_secs(2)));
        assert!(h.failed_at(R, Failure::Timeout, t0 + Duration::from_secs(2)));
        let t1 = t0 + Duration::from_secs(2);
        assert!(listed(&h, R, t1 + Duration::from_secs(29)));
        assert!(!listed(&h, R, t1 + Duration::from_secs(31)));
    }

    #[test]
    fn a_success_between_timeouts_resets_the_count() {
        let t0 = Instant::now();
        let h = online_at(t0);
        h.failed_at(R, Failure::Timeout, t0);
        h.succeeded_at(R, t0 + Duration::from_secs(1));
        assert!(!h.failed_at(R, Failure::Timeout, t0 + Duration::from_secs(2)));
    }

    #[test]
    fn an_old_success_is_not_online() {
        let t0 = Instant::now();
        let h = online_at(t0);
        let later = t0 + Duration::from_secs(10 * 60);
        assert!(!h.failed_at(R, Failure::Timeout, later));
        assert!(!h.failed_at(R, Failure::Timeout, later));
        assert!(!h.failed_at(R, Failure::Unreachable("connection refused"), later));
        assert!(!listed(&h, R, later));
    }

    #[test]
    fn keys_follow_same_relay_plus_the_default_port() {
        assert_eq!(key("wss://Relay.Example.com/"), key(R));
        assert_eq!(key("wss://relay.example.com:443"), key(R));
        assert_eq!(
            key("ws://relay.example.com:80/"),
            key("ws://relay.example.com")
        );
        assert_ne!(key("wss://relay.example.com:4443"), key(R));
        // The path is the relay's; its case is kept.
        assert_ne!(
            key("wss://relay.example.com/Inbox"),
            key("wss://relay.example.com/inbox")
        );
    }

    /// Spent entries are pruned on insert, and the map never passes its cap.
    #[test]
    fn the_list_prunes_and_is_capped() {
        let h = RelayHealth::default();
        let t0 = Instant::now();
        for i in 0..MAX_ENTRIES + 50 {
            h.failed_at(
                &format!("wss://r{i}.example"),
                Failure::Refused("HTTP 403"),
                t0,
            );
        }
        assert!(h.len() <= MAX_ENTRIES);
        // An hour later every skip is over: the next insert prunes them.
        let later = t0 + Duration::from_secs(3600);
        h.failed_at("wss://fresh.example", Failure::Refused("HTTP 403"), later);
        assert!(h.len() < 10, "spent entries were kept: {}", h.len());
        assert!(listed(&h, "wss://fresh.example", later));
    }

    #[test]
    fn a_mesh_relay_or_blossom_is_never_listed() {
        let h = RelayHealth::default();
        let t0 = Instant::now();
        for url in [
            "ws://npub1abc.fips:4870",
            "http://npub1abc.fips:24243",
            "http://[fd00::1234]:24243",
        ] {
            assert!(!h.failed_at(url, Failure::Refused("HTTP 5xx"), t0));
            assert!(!listed(&h, url, t0));
        }
    }

    #[test]
    fn statuses_that_skip_and_statuses_that_do_not() {
        for s in [403, 404, 500, 502, 503, 530, 599] {
            assert!(classify_status(s).is_some(), "{s} should skip");
        }
        for s in [101, 200, 400, 401, 429] {
            assert!(classify_status(s).is_none(), "{s} should not skip");
        }
    }

    #[test]
    fn dns_certificate_and_refused_are_unreachable_and_others_are_not() {
        let dns = std::io::Error::other(
            "failed to lookup address information: nodename nor servname provided",
        );
        assert_eq!(classify_io(&dns), Some("name does not resolve"));
        let cert = std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid peer certificate: UnknownIssuer",
        );
        assert_eq!(classify_io(&cert), Some("bad TLS certificate"));
        let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        assert_eq!(classify_io(&refused), Some("connection refused"));
        let reset = std::io::Error::from(std::io::ErrorKind::ConnectionReset);
        assert_eq!(classify_io(&reset), None);
    }

    #[test]
    fn a_websocket_upgrade_status_is_a_refusal() {
        use tokio_tungstenite::tungstenite::http::Response;
        use tokio_tungstenite::tungstenite::Error as WsError;
        let with = |status: u16| {
            let resp = Response::builder().status(status).body(None).unwrap();
            anyhow::Error::from(WsError::Http(resp))
        };
        assert_eq!(classify_ws(&with(530)), Some(Failure::Refused("HTTP 5xx")));
        assert_eq!(classify_ws(&with(403)), Some(Failure::Refused("HTTP 403")));
        assert_eq!(classify_ws(&with(429)), None);
        assert_eq!(classify_ws(&anyhow::anyhow!("something else")), None);
    }

    /// Mark the process online, as a real internet success would.
    fn seen_online() {
        reset();
        current().succeeded("wss://myco-relay-health-online.example");
    }

    /// A server that answers every connection with `status` and no body,
    /// counting connections. Stands in for a relay whose upgrade is refused
    /// (403, a Cloudflare 530) and a Blossom server that is down (503).
    pub(crate) async fn answering(
        status: u16,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let counted = hits.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf).await;
                let reply = format!(
                    "HTTP/1.1 {status} Nope\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            }
        });
        (format!("127.0.0.1:{}", addr.port()), hits)
    }

    /// A server that accepts TCP and never says a word: the WebSocket
    /// upgrade never completes.
    async fn silent() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        format!("ws://{addr}")
    }

    fn hits(n: &std::sync::Arc<std::sync::atomic::AtomicUsize>) -> usize {
        n.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// A relay refusing the upgrade with a 530 is dialled once; the next
    /// query is answered "skipped" without a connection.
    #[tokio::test]
    async fn a_refused_upgrade_is_dialled_once_then_skipped() {
        let (addr, served) = answering(530).await;
        let url = format!("ws://{addr}");
        assert!(
            crate::ip_source::query_relay_filters(&url, vec![serde_json::json!({})])
                .await
                .is_err()
        );
        assert_eq!(hits(&served), 1);
        assert!(is_skipped(&url));

        let second = crate::ip_source::query_relay_filters(&url, vec![serde_json::json!({})]).await;
        assert!(second.unwrap_err().to_string().contains("skipped"));
        assert_eq!(hits(&served), 1, "a skipped relay was dialled");

        // Publishing goes through the same list.
        let event = nostr::EventBuilder::text_note("x")
            .sign_with_keys(&nostr::Keys::generate())
            .unwrap();
        assert!(crate::ip_source::publish_to_relay(&url, &event)
            .await
            .is_err());
        assert_eq!(hits(&served), 1);
    }

    /// A 403 lists a relay; a 429 (slow down) does not.
    #[tokio::test]
    async fn a_forbidden_relay_is_listed_and_a_rate_limited_one_is_not() {
        let (forbidden, _) = answering(403).await;
        let forbidden = format!("ws://{forbidden}");
        let _ =
            crate::ip_source::query_relay_filters(&forbidden, vec![serde_json::json!({})]).await;
        assert!(is_skipped(&forbidden));

        let (limited, served) = answering(429).await;
        let limited = format!("ws://{limited}");
        let _ = crate::ip_source::query_relay_filters(&limited, vec![serde_json::json!({})]).await;
        let _ = crate::ip_source::query_relay_filters(&limited, vec![serde_json::json!({})]).await;
        assert!(!is_skipped(&limited));
        assert_eq!(hits(&served), 2);
    }

    /// The real DNS path, end to end: a name that does not resolve
    /// (`.invalid` never does, with or without a network) is listed once the
    /// phone has been online.
    #[tokio::test]
    async fn a_name_that_does_not_resolve_is_listed_when_online() {
        seen_online();
        let url = "wss://myco-relay-health-test.invalid";
        let _ = crate::ip_source::query_relay_filters(url, vec![serde_json::json!({})]).await;
        assert!(is_skipped(url), "a DNS failure was not listed");
    }

    /// A refused connection, end to end: listed once online.
    #[tokio::test]
    async fn a_refused_connection_is_listed_when_online() {
        seen_online();
        // Bind and drop: nothing listens on the port any more.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let url = format!("ws://127.0.0.1:{port}");
        let _ = crate::ip_source::query_relay_filters(&url, vec![serde_json::json!({})]).await;
        assert!(is_skipped(&url));
    }

    /// Two connect-phase timeouts at a counted deadline list a relay.
    #[tokio::test]
    async fn two_connect_timeouts_list_a_relay() {
        seen_online();
        let url = silent().await;
        for _ in 0..2 {
            let dial = crate::ip_source::query_relay_filters(&url, vec![serde_json::json!({})]);
            assert!(
                timeout_counting(&url, Duration::from_millis(200), Duration::ZERO, dial)
                    .await
                    .is_err()
            );
        }
        assert!(is_skipped(&url));
    }

    /// A relay that connects and is slow to answer is slow, not gone: its
    /// timeouts are not counted.
    #[tokio::test]
    async fn a_relay_that_connects_and_stalls_is_not_listed() {
        seen_online();
        let (url, _) =
            crate::ip_source::tests::mock_relay_delayed(Vec::new(), Duration::from_secs(10)).await;
        for _ in 0..3 {
            let dial = crate::ip_source::query_relay_filters(&url, vec![serde_json::json!({})]);
            assert!(
                timeout_counting(&url, Duration::from_millis(300), Duration::ZERO, dial)
                    .await
                    .is_err()
            );
        }
        assert!(!is_skipped(&url));
    }

    /// A napplet cannot get a relay listed by asking for a short
    /// `timeoutMs`: an outbox read with 500 ms that times out, again and
    /// again, counts for nothing.
    #[tokio::test]
    async fn a_napplets_short_timeout_never_lists_a_relay() {
        use myco_napplet_runtime::seams::{LaneTransport, RelayLane};
        seen_online();
        let url = silent().await;
        let dir = std::env::temp_dir().join(format!(
            "myco-relay-health-napplet-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        let content = std::sync::Arc::new(crate::content::Content::open(&dir).unwrap());
        let svc = crate::outbox::OutboxService::new(
            content.relay(),
            std::sync::Arc::new(std::sync::Mutex::new(None)),
            content,
            "npub1me".to_string(),
        )
        .allowing_private_dials();
        for _ in 0..4 {
            svc.query(
                &[RelayLane::Internet { url: url.clone() }],
                &[nostr::Filter::new()],
                Duration::from_millis(500),
            )
            .await;
        }
        assert!(
            !is_skipped(&url),
            "a napplet's 500 ms timeout listed a relay"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Blossom server answering 503 is listed and not asked again; one
    /// answering 404 ("not here") is not held against it.
    #[tokio::test]
    async fn a_failing_blossom_server_is_skipped_and_a_404_is_not() {
        use nsite_deck::seams::PeerSource;
        let (down, down_hits) = answering(503).await;
        let (missing, missing_hits) = answering(404).await;
        let (down, missing) = (format!("http://{down}"), format!("http://{missing}"));
        let source =
            crate::ip_source::IpPeerSource::new(Vec::new(), vec![down.clone(), missing.clone()]);
        let want = "00".repeat(32);
        assert!(source.fetch_blob(&want, &[]).await.unwrap().is_none());
        assert!(is_skipped(&down));
        assert!(!is_skipped(&missing));

        assert!(source.fetch_blob(&want, &[]).await.unwrap().is_none());
        assert_eq!(
            hits(&down_hits),
            1,
            "a skipped Blossom server was asked again"
        );
        assert_eq!(hits(&missing_hits), 2);
    }

    /// The review's case: a network switch. One success, then four relays
    /// fail DNS within a second — that is the network, and none is listed.
    #[test]
    fn a_burst_of_unreachable_relays_lists_none() {
        let t0 = Instant::now();
        let h = online_at(t0);
        for i in 0..4 {
            let url = format!("wss://r{i}.example");
            h.failed_at(
                &url,
                Failure::Unreachable(WHY_DNS),
                t0 + Duration::from_millis(200 * i),
            );
        }
        for i in 0..4 {
            assert!(
                !listed(
                    &h,
                    &format!("wss://r{i}.example"),
                    t0 + Duration::from_secs(1)
                ),
                "r{i} was listed in a burst"
            );
        }
        // And the phone no longer counts as online: a lone failure right
        // after lists nothing either.
        let later = t0 + Duration::from_secs(30);
        assert!(!h.failed_at(R, Failure::Unreachable(WHY_DNS), later));
    }

    /// A lone certificate failure is the internet answering (the breaker
    /// hears it); the same failure on three relays at once is a captive
    /// portal, and clears what was heard.
    #[test]
    fn a_certificate_is_heard_unless_everyone_sends_one() {
        let t0 = Instant::now();
        let h = RelayHealth::default();
        h.failed_at(R, Failure::Unreachable(WHY_CERT), t0);
        assert!(h.heard_since(t0));

        let t1 = t0 + Duration::from_secs(60);
        for i in 0..3 {
            h.failed_at(
                &format!("wss://p{i}.example"),
                Failure::Unreachable(WHY_CERT),
                t1,
            );
        }
        assert!(
            !h.heard_since(t0),
            "a captive portal's certificates were heard"
        );

        // A DNS failure is never heard.
        let h = RelayHealth::default();
        h.failed_at(R, Failure::Unreachable(WHY_DNS), t0);
        assert!(!h.heard_since(t0));
    }

    #[test]
    fn a_busy_relay_is_capped_short_and_retry_after_is_honoured() {
        let t0 = Instant::now();
        let h = RelayHealth::default();
        let mut t = t0;
        for _ in 0..10 {
            t += Duration::from_secs(31 * 60);
            h.failed_at(R, Failure::Busy(None), t);
        }
        assert!(
            !listed(&h, R, t + Duration::from_secs(5 * 60 + 1)),
            "503 past 5 min"
        );

        let h = RelayHealth::default();
        h.failed_at(R, Failure::Busy(Some(Duration::from_secs(120))), t0);
        assert!(listed(&h, R, t0 + Duration::from_secs(119)));
        assert!(!listed(&h, R, t0 + Duration::from_secs(121)));
    }

    #[test]
    fn a_captive_portals_511_is_neither_listed_nor_heard() {
        assert_eq!(classify_status(511), None);
        assert!(!http_heard(511));
        assert!(http_heard(429) && http_heard(502));
        use tokio_tungstenite::tungstenite::http::Response;
        use tokio_tungstenite::tungstenite::Error as WsError;
        let busy = Response::builder()
            .status(503)
            .header("Retry-After", "90")
            .body(None)
            .unwrap();
        assert_eq!(
            classify_ws(&anyhow::Error::from(WsError::Http(busy))),
            Some(Failure::Busy(Some(Duration::from_secs(90))))
        );
    }

    /// End to end through reqwest and hyper: a Blossom server whose name
    /// does not resolve is listed — the guard against their error messages
    /// drifting out from under `classify_http`.
    #[tokio::test]
    async fn a_blossom_server_that_does_not_resolve_is_listed() {
        use nsite_deck::seams::PeerSource;
        seen_online();
        let server = "https://myco-relay-health-blossom.invalid";
        let source = crate::ip_source::IpPeerSource::new(Vec::new(), vec![server.to_string()]);
        assert!(source
            .fetch_blob(&"00".repeat(32), &[])
            .await
            .unwrap()
            .is_none());
        assert!(is_skipped(server), "reqwest's DNS error was not recognised");
    }

    /// A relay answering 429 is up: heard, not listed.
    #[tokio::test]
    async fn any_http_answer_is_heard() {
        reset();
        let started = Instant::now();
        let (addr, _) = answering(429).await;
        let url = format!("ws://{addr}");
        let _ = crate::ip_source::query_relay_filters(&url, vec![serde_json::json!({})]).await;
        assert!(internet_heard_since(started));
        assert!(!is_skipped(&url));
    }
}
