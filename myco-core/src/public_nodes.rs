//! Public internet mesh nodes (roadmap N10).
//!
//! When the phone is online it can also peer with public FIPS nodes over the
//! internet, so Circle members who are not in the same room still reach each
//! other through the mesh. A public node is an **ordinary mesh peer**: fips
//! routes through it, nothing more. It is not a Circle member and the Circle
//! gate does not change — it can carry our packets, it cannot read our relay,
//! our Blossom or the contents of anything (every Circle session is end to end
//! Noise above it).
//!
//! # Where the list comes from
//!
//! Public fips nodes announce themselves on Nostr: a parameterised-replaceable
//! **kind 37195** event, `d` = `fips-overlay-v1`, whose content names the
//! node's endpoints (`{"transport":"udp","addr":"203.0.113.7:2121"}`). fips
//! publishes these to `relay.damus.io`, `nos.lol` and `offchain.pub` with a
//! one-hour NIP-40 expiration. Myco reads the same relays with the same filter
//! fips uses (kind + `#d`), verifies every event, and keeps the nodes it could
//! actually dial — see [`parse_advert`].
//!
//! Which of those to **recommend** is the list <https://join.fips.network>
//! stars: the project's own test nodes. Myco ships that list
//! ([`SHIPPED_RECOMMENDED`]) and only uses it to highlight nodes the adverts
//! already found; it never fetches the site. `next` nodes (`test-us03-next`)
//! are left out: their adverts carry another `d`, and Myco cannot speak their
//! protocol.
//!
//! Only nodes advertising right now are listed or dialled. Of the advertising
//! recommended ones, Myco **preselects at most three at random** — once, on
//! the first read with adverts — and keeps that pick in `public_nodes.json`, so
//! phones spread over the test nodes and nothing reshuffles per launch. A
//! preselected node unseen for a day is replaced by another random one. The
//! user's own ticks and unticks win over the pick.
//!
//! # When it dials
//!
//! Off by default; the user turns it on knowing the trade-off (a public node
//! learns this phone's IP address and which mesh addresses it talks to, not
//! the contents). Even when on, nothing is fetched or dialled while
//! mesh-only is on or the internet breaker
//! ([`crate::content::Content::internet_looks_down`]) is tripped. In the
//! background the advert refresh and redials slow right down, and existing
//! links are left to fips's own liveness rather than torn down.
//!
//! A public node never replaces a radio path: it is a different peer from
//! any Circle member, so a member reachable over BLE, Aware or the LAN keeps
//! that direct link, and the internet lane's socket is a backup-role
//! transport on a multi-path core.
//!
//! The decisions here ([`PublicNodes::plan`]) are synchronous and take the
//! clock as an input, so they are host-tested; [`PublicNodes::run`] is the
//! async driver that performs the relay reads and control-socket calls.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use nostr::nips::nip19::{FromBech32, ToBech32};
use nostr::{Event, PublicKey, TagKind};
use serde::{Deserialize, Serialize};

use crate::control_client::PeerView;

/// The fips UDP transport instance public nodes are dialled over.
///
/// Its own instance rather than the LAN lane's socket because that one is
/// pinned to the Wi-Fi network for link-local peers, and a phone on cellular
/// has no Wi-Fi to reach a public node through. This one is outbound-only (no
/// listener: a phone behind NAT is never dialled from the internet anyway) and
/// Kotlin pins it to the network the system picks for an internet, non-VPN
/// request, so a full-tunnel VPN never captures the mesh's own traffic.
pub const PUBLIC_UDP_INSTANCE: &str = "internet";

/// The `transport` a public node is dialled with over the control socket.
fn dial_transport() -> String {
    format!("udp/{PUBLIC_UDP_INSTANCE}")
}

/// The relays fips publishes overlay adverts to by default
/// (`NostrRendezvousConfig::default_advert_relays`), which are also the ones
/// join.fips.network reads.
pub const ADVERT_RELAYS: [&str; 3] = [
    "wss://relay.damus.io",
    "wss://nos.lol",
    "wss://offchain.pub",
];

/// The `protocol` tag fips stamps on an advert: its default traversal app
/// namespace, which happens to be the same string as the `d` identifier.
const ADVERT_PROTOCOL: &str = "fips-overlay-v1";

/// The nodes join.fips.network recommends, as of 2026-10-04, less its `next`
/// entry (`test-us03-next`). Only a highlight: a node here is starred and
/// eligible for the preselection once its advert is found, never dialled
/// without one. Updated by hand when the site's list changes.
pub const SHIPPED_RECOMMENDED: &[(&str, &str)] = &[
    (
        "test-us01",
        "npub1qmc3cvfz0yu2hx96nq3gp55zdan2qclealn7xshgr448d3nh6lks7zel98",
    ),
    (
        "test-us02",
        "npub10yffd020a4ag8zcy75f9pruq3rnghvvhd5hphl9s62zgp35s560qrksp9u",
    ),
    (
        "test-us03",
        "npub136yqae6na688fs75g95ppps3lxe07fvxefj77938zf47uhm6074sxw8ctm",
    ),
    (
        "test-us04",
        "npub1gd7ye2qp2lphhzx75fynnjzaxx4dqanddecet0wtt5ss5ek8h9ps62wdkf",
    ),
    (
        "test-de01",
        "npub1260n42s06vzc7796w0fh3ny7zcpw6tlk4gq3940gmfrzl5c9pv2s3657q8",
    ),
    (
        "test-es01",
        "npub17lpmzulpc98d8ff727k6e98atxn3phzupzsqqwe54ytduym747ws4tw5zm",
    ),
    (
        "test-uk01",
        "npub1u0z26dc4qeneu5rvwvmpfhtwh3522ed6rlgxr9jarrfnjrc6ew4qxjysrs",
    ),
];

/// Advert content larger than this is not an advert. A real one is a few
/// hundred bytes; the cap bounds what a hostile relay can make us parse.
const MAX_CONTENT_BYTES: usize = 4096;
/// More endpoints than this is not a node, it is a list of targets.
const MAX_ENDPOINTS: usize = 16;
/// An advert older than this is ignored even without an expiration tag. fips
/// refreshes every 30 min with a one-hour TTL, so two hours is a node that has
/// gone away.
const MAX_ADVERT_AGE_SECS: u64 = 2 * 3600;
/// How far in the future `created_at` may be before the advert is refused —
/// ordinary clock skew, not a timestamp chosen to win every comparison.
const FUTURE_SKEW_SECS: u64 = 600;
/// Cap on the nodes held from relay traffic. Hundreds of NAT-only browser
/// nodes advertise; only dialable ones are kept, and only this many.
const MAX_DIRECTORY: usize = 256;
/// Events read per relay per refresh, at most.
const MAX_EVENTS_PER_RELAY: usize = 2000;
/// How many non-recommended nodes the Settings list shows.
const MAX_LISTED_OTHERS: usize = 50;

/// How many public nodes to hold links to at once. One is a single point of
/// failure; more than two is battery spent on redundancy nobody needs, since
/// the public nodes peer with each other.
pub const TARGET_LINKS: usize = 2;

/// The driver's cadence.
const TICK: Duration = Duration::from_secs(15);
/// Advert refresh interval in the foreground and in the background.
const FETCH_EVERY_FOREGROUND_MS: u64 = 10 * 60 * 1000;
const FETCH_EVERY_BACKGROUND_MS: u64 = 60 * 60 * 1000;
/// How long one relay read may take.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Redial backoff: first retry after this, doubling to the cap.
const DIAL_BACKOFF_MIN_MS: u64 = 30 * 1000;
const DIAL_BACKOFF_MAX_MS: u64 = 10 * 60 * 1000;
/// In the background no node is redialled more often than this.
const DIAL_BACKOFF_BACKGROUND_MS: u64 = 5 * 60 * 1000;
/// A dial younger than this that has not shown up connected yet is still in
/// flight, and counts toward [`TARGET_LINKS`].
const DIAL_IN_FLIGHT_MS: u64 = 30 * 1000;

// ---------------------------------------------------------------------------
// Persisted choices
// ---------------------------------------------------------------------------

/// One recommended node: a human name and its npub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recommended {
    pub name: String,
    pub npub: String,
}

/// The recommended list: [`SHIPPED_RECOMMENDED`], in the site's order.
pub fn recommended() -> Vec<Recommended> {
    SHIPPED_RECOMMENDED
        .iter()
        .map(|(name, npub)| Recommended {
            name: name.to_string(),
            npub: npub.to_string(),
        })
        .collect()
}

/// The file the choices live in, next to `settings.json` but apart from it:
/// the driver task writes it off the reducer thread, and sharing a file with
/// the reducer's own load-modify-save would race.
const CHOICES_FILE: &str = "public_nodes.json";

/// What the user chose, persisted in [`CHOICES_FILE`].
///
/// Selection is Myco's **preselection** — at most [`PRESELECT_MAX`] advertising
/// recommended nodes, picked at random once and kept — plus the user's own
/// deltas against it: `added` wins for any node, `removed` switches a
/// preselected one off and keeps it off.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PublicNodeSettings {
    /// The opt-in. Off by default.
    pub enabled: bool,
    /// Nodes the user selected that Myco had not preselected.
    pub added: BTreeSet<String>,
    /// Preselected nodes the user deselected. They are never preselected again.
    pub removed: BTreeSet<String>,
    /// Myco's random preselection: npub → when it was last seen advertising,
    /// ms since the epoch. A node unseen for [`PRESELECT_REPLACE_AFTER_MS`] is
    /// replaced by another random one.
    pub preselected: std::collections::BTreeMap<String, u64>,
}

/// How many recommended nodes Myco preselects. Random rather than the first
/// few, so phones spread over the test nodes instead of piling onto one.
pub const PRESELECT_MAX: usize = 3;
/// A preselected node that has not advertised for this long is replaced.
const PRESELECT_REPLACE_AFTER_MS: u64 = 24 * 3600 * 1000;
/// Last-seen stamps are only rewritten to disk when they move this much.
const PRESELECT_STAMP_GRANULARITY_MS: u64 = 3600 * 1000;

impl PublicNodeSettings {
    /// Read the choices, falling back to defaults (off) on anything
    /// unreadable, like `settings_store::load`.
    pub fn load(data_dir: &std::path::Path) -> Self {
        match std::fs::read(data_dir.join(CHOICES_FILE)) {
            Ok(raw) => serde_json::from_slice(&raw).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "public nodes: ignoring a corrupt {CHOICES_FILE}");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Write the choices atomically (temp + rename). Callers serialise writes
    /// ([`PublicNodes::save`]); this does not lock.
    fn store(&self, data_dir: &std::path::Path) -> anyhow::Result<()> {
        let path = data_dir.join(CHOICES_FILE);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Whether `npub` is one to dial: added by the user, or preselected and
    /// not switched off.
    pub fn is_selected(&self, npub: &str) -> bool {
        self.added.contains(npub)
            || (self.preselected.contains_key(npub) && !self.removed.contains(npub))
    }

    /// Record a per-node choice as the smallest delta against the
    /// preselection.
    pub fn set_selected(&mut self, npub: &str, selected: bool) {
        let preselected = self.preselected.contains_key(npub);
        if selected {
            self.removed.remove(npub);
            if !preselected {
                self.added.insert(npub.to_string());
            }
        } else {
            self.added.remove(npub);
            if preselected {
                self.removed.insert(npub.to_string());
            }
        }
    }

    /// Keep the preselection current against the nodes advertising now:
    /// stamp the ones seen, drop ones unseen for a day, and fill up to
    /// [`PRESELECT_MAX`] at random from advertising recommended nodes the user
    /// has not switched off. Strangers are never preselected — a node from
    /// outside the recommended list is the user's call.
    ///
    /// `pick(n)` returns an index below `n`; injected so tests are
    /// deterministic. Returns whether anything worth saving changed.
    pub fn maintain_preselection(
        &mut self,
        advertising: &HashSet<String>,
        now_ms: u64,
        pick: &mut dyn FnMut(usize) -> usize,
    ) -> bool {
        let mut changed = false;
        for (npub, seen) in self.preselected.iter_mut() {
            if advertising.contains(npub)
                && now_ms.saturating_sub(*seen) >= PRESELECT_STAMP_GRANULARITY_MS
            {
                *seen = now_ms;
                changed = true;
            }
        }
        let before = self.preselected.len();
        self.preselected
            .retain(|_, seen| now_ms.saturating_sub(*seen) < PRESELECT_REPLACE_AFTER_MS);
        changed |= self.preselected.len() != before;

        let active = self
            .preselected
            .keys()
            .filter(|n| !self.removed.contains(*n))
            .count();
        let mut pool: Vec<String> = recommended()
            .into_iter()
            .map(|r| r.npub)
            .filter(|n| advertising.contains(n))
            .filter(|n| !self.preselected.contains_key(n) && !self.removed.contains(n))
            .collect();
        for _ in active..PRESELECT_MAX {
            if pool.is_empty() {
                break;
            }
            let npub = pool.swap_remove(pick(pool.len()) % pool.len());
            self.preselected.insert(npub, now_ms);
            changed = true;
        }
        changed
    }
}

/// A small xorshift generator seeded from the OS (via a fresh key), for the
/// preselection's one-off draw. Not cryptographic, and needs not be.
fn os_seeded_picker() -> impl FnMut(usize) -> usize {
    let bytes = nostr::Keys::generate().secret_key().secret_bytes();
    let mut state = u64::from_le_bytes(bytes[..8].try_into().unwrap_or([7; 8])) | 1;
    move |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n.max(1) as u64) as usize
    }
}

// ---------------------------------------------------------------------------
// Advert parsing — untrusted input
// ---------------------------------------------------------------------------

/// A public node Myco could dial: verified, unexpired, and naming at least
/// one public UDP address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicNode {
    pub npub: String,
    /// Public UDP endpoints, in the advert's order.
    pub udp: Vec<SocketAddr>,
    /// The advert's `created_at`, seconds.
    pub created_at: u64,
    /// When the advert stops counting, seconds: its NIP-40 expiration, capped
    /// at [`MAX_ADVERT_AGE_SECS`] past `created_at`.
    pub valid_until: u64,
}

impl PublicNode {
    /// The address to dial: the first IPv4 endpoint, because the internet
    /// lane's socket is fips's outbound-only one, which binds `0.0.0.0:0`.
    pub fn dial_addr(&self) -> Option<SocketAddr> {
        self.udp.iter().copied().find(SocketAddr::is_ipv4)
    }
}

/// Why an advert was not taken. Only for tests and debug logs: every one of
/// them is ordinary relay traffic, not a fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reject {
    WrongKind,
    BadSignature,
    TooLarge,
    WrongIdentifier,
    WrongProtocol,
    Expired,
    FutureDated,
    BadContent,
    TooManyEndpoints,
    NoDialableEndpoint,
}

/// The advert body, parsed loosely: transports are strings so an endpoint
/// type this build does not know (`webrtc`, …) costs that endpoint, not the
/// whole advert.
#[derive(Deserialize)]
struct RawAdvert {
    identifier: String,
    version: u32,
    endpoints: Vec<RawEndpoint>,
}

#[derive(Deserialize)]
struct RawEndpoint {
    transport: String,
    addr: String,
}

/// Parse and validate one overlay advert event.
///
/// Everything a relay hands us is untrusted, so this checks, in order: the
/// kind, the size, the signature (the relay pool verifies too; this is the
/// boundary that must not depend on it), the `d` and `protocol` tags, the
/// timestamps, and the body. Of the endpoints it keeps only UDP ones that are
/// literal public socket addresses — no hostnames (a name would make the
/// advert's author choose what we resolve), no `nat` (that needs fips's
/// Nostr traversal, which Myco does not run), and nothing loopback, private,
/// link-local, CGNAT, documentation or otherwise unroutable, so an advert
/// cannot point this phone at its own LAN.
pub fn parse_advert(event: &Event, now_secs: u64) -> Result<PublicNode, Reject> {
    if event.kind.as_u16() != fips::nostr::ADVERT_KIND {
        return Err(Reject::WrongKind);
    }
    if event.content.len() > MAX_CONTENT_BYTES {
        return Err(Reject::TooLarge);
    }
    if event.verify().is_err() {
        return Err(Reject::BadSignature);
    }
    if event.tags.identifier() != Some(fips::nostr::ADVERT_IDENTIFIER) {
        return Err(Reject::WrongIdentifier);
    }
    let protocol = event
        .tags
        .find(TagKind::custom("protocol"))
        .and_then(|tag| tag.content());
    if protocol != Some(ADVERT_PROTOCOL) {
        return Err(Reject::WrongProtocol);
    }

    let created_at = event.created_at.as_secs();
    if created_at > now_secs.saturating_add(FUTURE_SKEW_SECS) {
        return Err(Reject::FutureDated);
    }
    let age_limit = created_at.saturating_add(MAX_ADVERT_AGE_SECS);
    let valid_until = match event.tags.expiration() {
        Some(expires) => expires.as_secs().min(age_limit),
        None => age_limit,
    };
    if valid_until <= now_secs {
        return Err(Reject::Expired);
    }

    let advert: RawAdvert = serde_json::from_str(&event.content).map_err(|_| Reject::BadContent)?;
    if advert.identifier != fips::nostr::ADVERT_IDENTIFIER
        || advert.version != fips::nostr::ADVERT_VERSION
    {
        return Err(Reject::WrongIdentifier);
    }
    if advert.endpoints.len() > MAX_ENDPOINTS {
        return Err(Reject::TooManyEndpoints);
    }
    let udp: Vec<SocketAddr> = advert
        .endpoints
        .iter()
        .filter(|e| e.transport.eq_ignore_ascii_case("udp"))
        .filter_map(|e| e.addr.trim().parse::<SocketAddr>().ok())
        .filter(|a| a.port() != 0 && is_public_ip(a.ip()))
        .collect();
    if udp.is_empty() {
        return Err(Reject::NoDialableEndpoint);
    }

    Ok(PublicNode {
        npub: event.pubkey.to_bech32().map_err(|_| Reject::BadSignature)?,
        udp,
        created_at,
        valid_until,
    })
}

/// Whether `ip` is a globally routable unicast address — the only kind a
/// stranger's advert may make this phone send to.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let s = v6.segments();
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local, incl. the fips mesh fd00::/8
                || (s[0] & 0xffc0) == 0xfe80 // link-local
                || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
                || (s[0..6] == [0, 0, 0, 0, 0, 0])) // IPv4-compatible, deprecated
        }
    }
}

fn is_public_v4(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    !(v4.is_unspecified()
        || v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_documentation()
        || v4.is_multicast()
        || o[0] == 0 // "this network"
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // CGNAT 100.64/10
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // IETF protocol assignments
        || (o[0] == 198 && (o[1] & 0xfe) == 18) // benchmarking 198.18/15
        || o[0] >= 240) // reserved
}

// ---------------------------------------------------------------------------
// The state machine
// ---------------------------------------------------------------------------

/// Everything outside this module that decides whether to act.
#[derive(Debug, Clone, Copy)]
pub struct Gate {
    /// Mesh-only is on: never touch the internet.
    pub offline_only: bool,
    /// The internet breaker is tripped.
    pub internet_down: bool,
    /// The node's control socket is up.
    pub node_live: bool,
}

/// What one tick should do. Produced by [`PublicNodes::plan`], executed by
/// the driver.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TickPlan {
    /// Read the advert relays.
    pub fetch: bool,
    /// `(npub, address)` to dial over the internet lane.
    pub connect: Vec<(String, String)>,
    /// Npubs to disconnect: public nodes this module dialled that the user
    /// no longer wants.
    pub disconnect: Vec<String>,
}

#[derive(Debug, Default, Clone)]
struct DialState {
    last_attempt_ms: u64,
    failures: u32,
    last_error: String,
}

#[derive(Debug, Default, Clone)]
struct FetchStatus {
    running: bool,
    last_ms: u64,
    relays_answered: u8,
    error: String,
}

#[derive(Default)]
struct Inner {
    settings: PublicNodeSettings,
    directory: HashMap<String, PublicNode>,
    dials: HashMap<String, DialState>,
    foreground: bool,
    fetch: FetchStatus,
    fetch_requested: bool,
}

/// The process's public-node state: the user's choices, the nodes heard of,
/// and the dial bookkeeping. Shared between the reducer (actions, `state()`)
/// and the driver task.
pub struct PublicNodes {
    data_dir: PathBuf,
    inner: Mutex<Inner>,
    /// Held across a whole save, so the reducer and the driver task never
    /// write the file at once, and the last write carries the newest choices.
    save_lock: Mutex<()>,
    wake: tokio::sync::Notify,
}

impl PublicNodes {
    pub fn new(data_dir: impl Into<PathBuf>, settings: PublicNodeSettings) -> Self {
        Self {
            data_dir: data_dir.into(),
            inner: Mutex::new(Inner {
                settings,
                // Off screen until Kotlin reports a start: a process brought
                // up in the background (always-on VPN, a restart after a
                // kill) gets no lifecycle callback until the app is shown.
                foreground: false,
                ..Inner::default()
            }),
            save_lock: Mutex::new(()),
            wake: tokio::sync::Notify::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Persist the choices as they are now. The snapshot is taken under the
    /// save lock, so a save that started earlier can never land after, and
    /// overwrite, a newer one.
    fn save(&self) -> anyhow::Result<()> {
        let _saving = self.save_lock.lock().unwrap_or_else(|e| e.into_inner());
        let choices = self.lock().settings.clone();
        choices.store(&self.data_dir)
    }

    /// The opt-in. Turning it on asks for an advert read at once; turning it
    /// off has the driver drop the links on its next pass, which it is woken
    /// for now.
    pub fn set_enabled(&self, enabled: bool) -> anyhow::Result<()> {
        {
            let mut inner = self.lock();
            inner.settings.enabled = enabled;
            if enabled {
                inner.fetch_requested = true;
            }
        }
        self.wake.notify_one();
        self.save()
    }

    /// Select or deselect one node.
    pub fn set_selected(&self, npub: &str, selected: bool) -> anyhow::Result<()> {
        if PublicKey::from_bech32(npub).is_err() {
            anyhow::bail!("not an npub: {npub}");
        }
        self.lock().settings.set_selected(npub, selected);
        self.wake.notify_one();
        self.save()
    }

    /// Read the advert relays on the next pass, even with the feature off —
    /// the Settings list asks for this so a user can see the nodes before
    /// opting in. Mesh-only and the breaker still apply.
    pub fn request_refresh(&self) {
        self.lock().fetch_requested = true;
        self.wake.notify_one();
    }

    /// Whether the app is on screen. Off screen, refreshes and redials slow.
    pub fn set_foreground(&self, foreground: bool) {
        let changed = {
            let mut inner = self.lock();
            let changed = inner.foreground != foreground;
            inner.foreground = foreground;
            changed
        };
        if changed && foreground {
            self.wake.notify_one();
        }
    }

    /// Take freshly parsed nodes into the directory: newest advert per npub
    /// wins, and past the cap the stalest are dropped.
    pub fn observe(&self, nodes: impl IntoIterator<Item = PublicNode>, now_secs: u64) {
        let mut inner = self.lock();
        for node in nodes {
            let newer = inner
                .directory
                .get(&node.npub)
                .is_none_or(|have| have.created_at <= node.created_at);
            if newer {
                inner.directory.insert(node.npub.clone(), node);
            }
        }
        inner.directory.retain(|_, n| n.valid_until > now_secs);
        if inner.directory.len() > MAX_DIRECTORY {
            let recommended: HashSet<String> = recommended().into_iter().map(|r| r.npub).collect();
            let mut by_age: Vec<(String, u64)> = inner
                .directory
                .values()
                .filter(|n| !recommended.contains(&n.npub))
                .map(|n| (n.npub.clone(), n.valid_until))
                .collect();
            by_age.sort_by_key(|(_, until)| *until);
            let overflow = inner.directory.len() - MAX_DIRECTORY;
            for (npub, _) in by_age.into_iter().take(overflow) {
                inner.directory.remove(&npub);
            }
        }
    }

    /// Bring the random preselection up to date with what is advertising now
    /// ([`PublicNodeSettings::maintain_preselection`]). Only nodes with an
    /// address this phone can dial count as advertising, so an IPv6-only node
    /// never takes a slot. Returns whether something changed and wants saving.
    fn maintain_preselection(&self, now_ms: u64, pick: &mut dyn FnMut(usize) -> usize) -> bool {
        let mut inner = self.lock();
        let now_secs = now_ms / 1000;
        let advertising: HashSet<String> = inner
            .directory
            .values()
            .filter(|n| n.valid_until > now_secs && n.dial_addr().is_some())
            .map(|n| n.npub.clone())
            .collect();
        inner
            .settings
            .maintain_preselection(&advertising, now_ms, pick)
    }

    /// Decide what this tick does. Pure over the held state, the clock and
    /// the gate; records the dials it plans so the next tick sees them.
    ///
    /// `connected` is the npubs fips reports connected; `circle` the user's
    /// Circle, whose members are never disconnected from here even if one of
    /// them happens to run a public node. Only links this module dialled are
    /// ever disconnected: a peer that also advertises publicly but reached us
    /// another way (a LAN daemon, a radio) is not ours to drop.
    pub fn plan(
        &self,
        now_ms: u64,
        gate: Gate,
        connected: &HashSet<String>,
        circle: &HashSet<String>,
    ) -> TickPlan {
        let mut inner = self.lock();
        let inner = &mut *inner;
        let now_secs = now_ms / 1000;
        let recommended = recommended();
        let known: HashSet<&str> = inner
            .directory
            .keys()
            .map(String::as_str)
            .chain(recommended.iter().map(|r| r.npub.as_str()))
            .chain(inner.settings.added.iter().map(String::as_str))
            .chain(inner.settings.preselected.keys().map(String::as_str))
            .collect();
        let public_connected: Vec<&String> = connected
            .iter()
            .filter(|n| known.contains(n.as_str()))
            .collect();

        // A dial that has come up is a success: its backoff resets.
        for npub in &public_connected {
            if let Some(dial) = inner.dials.get_mut(*npub) {
                dial.failures = 0;
                dial.last_error.clear();
            }
        }

        let mut plan = TickPlan::default();
        let internet_ok = !gate.offline_only && !gate.internet_down;
        let enabled = inner.settings.enabled && !gate.offline_only;

        // Links the user no longer wants go, whatever the internet is doing.
        if gate.node_live {
            plan.disconnect = public_connected
                .iter()
                .filter(|n| inner.dials.contains_key(n.as_str()))
                .filter(|n| !circle.contains(n.as_str()))
                .filter(|n| !enabled || !inner.settings.is_selected(n))
                .map(|n| (*n).clone())
                .collect();
            plan.disconnect.sort();
        }

        if !internet_ok {
            return plan;
        }

        let fetch_every = if inner.foreground {
            FETCH_EVERY_FOREGROUND_MS
        } else {
            FETCH_EVERY_BACKGROUND_MS
        };
        let fetch_due = enabled && now_ms.saturating_sub(inner.fetch.last_ms) >= fetch_every;
        if !inner.fetch.running && (inner.fetch_requested || fetch_due) {
            plan.fetch = true;
            inner.fetch_requested = false;
        }

        if !enabled {
            return plan;
        }

        if !gate.node_live {
            return plan;
        }

        // How many links are up or on their way.
        let in_flight = inner
            .dials
            .iter()
            .filter(|(npub, d)| {
                !connected.contains(*npub)
                    && d.last_attempt_ms > 0
                    && now_ms.saturating_sub(d.last_attempt_ms) < DIAL_IN_FLIGHT_MS
            })
            .count();
        let have = public_connected
            .iter()
            .filter(|n| inner.settings.is_selected(n))
            .count()
            + in_flight;
        if have >= TARGET_LINKS {
            return plan;
        }

        // Candidates: selected, advertised now, not connected, and due. The
        // recommended ones first in their published order, then the newest.
        let rank = |npub: &str| recommended.iter().position(|r| r.npub == npub);
        let mut candidates: Vec<&PublicNode> = inner
            .directory
            .values()
            .filter(|n| n.valid_until > now_secs)
            .filter(|n| !connected.contains(&n.npub))
            .filter(|n| inner.settings.is_selected(&n.npub))
            .filter(|n| n.dial_addr().is_some())
            .filter(|n| {
                inner.dials.get(&n.npub).is_none_or(|d| {
                    now_ms.saturating_sub(d.last_attempt_ms) >= backoff_ms(d, inner.foreground)
                })
            })
            .collect();
        candidates.sort_by(|a, b| {
            let (ra, rb) = (rank(&a.npub), rank(&b.npub));
            match (ra, rb) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => b.created_at.cmp(&a.created_at).then(a.npub.cmp(&b.npub)),
            }
        });
        for node in candidates.into_iter().take(TARGET_LINKS - have) {
            let Some(addr) = node.dial_addr() else {
                continue;
            };
            plan.connect.push((node.npub.clone(), addr.to_string()));
        }
        for (npub, _) in &plan.connect {
            let dial = inner.dials.entry(npub.clone()).or_default();
            // A previous dial that never came up counts as a failure now
            // that it is being retried.
            if dial.last_attempt_ms > 0 {
                dial.failures = dial.failures.saturating_add(1);
            }
            dial.last_attempt_ms = now_ms;
        }
        plan
    }

    /// Forget a node's dial after its link was dropped on purpose, so a later
    /// link to it that Myco did not make is left alone.
    fn forget_dial(&self, npub: &str) {
        self.lock().dials.remove(npub);
    }

    /// Note a dial the control socket refused outright.
    fn record_dial_error(&self, npub: &str, error: &str) {
        if let Some(dial) = self.lock().dials.get_mut(npub) {
            dial.last_error = error.chars().take(200).collect();
        }
    }

    /// The Settings and Dev view.
    pub fn view(&self, peers: &[PeerView], gate: Gate, now_ms: u64) -> PublicNodesView {
        let inner = self.lock();
        let now_secs = now_ms / 1000;
        let recommended = recommended();
        let peer_by_npub: HashMap<&str, &PeerView> =
            peers.iter().map(|p| (p.npub.as_str(), p)).collect();

        let row = |npub: &str, name: String, is_recommended: bool| {
            let node = inner
                .directory
                .get(npub)
                .filter(|n| n.valid_until > now_secs);
            let peer = peer_by_npub.get(npub).filter(|p| p.connected);
            let dial = inner.dials.get(npub);
            let selected = inner.settings.is_selected(npub);
            let state = if peer.is_some() {
                "connected"
            } else if !inner.settings.enabled || !selected {
                "idle"
            } else if dial.is_some_and(|d| {
                d.last_attempt_ms > 0
                    && now_ms.saturating_sub(d.last_attempt_ms) < DIAL_IN_FLIGHT_MS
            }) {
                "connecting"
            } else {
                "waiting"
            };
            PublicNodeView {
                npub: npub.to_string(),
                name,
                recommended: is_recommended,
                selected,
                advertised: node.is_some(),
                endpoint: node
                    .and_then(|n| n.dial_addr().or_else(|| n.udp.first().copied()))
                    .map(|a| a.to_string())
                    .unwrap_or_default(),
                advertised_at_ms: node.map(|n| n.created_at * 1000).unwrap_or(0),
                state: state.to_string(),
                srtt_ms: peer.and_then(|p| p.srtt_ms),
                connected_since_ms: peer.map(|p| p.authenticated_at_ms).unwrap_or(0),
                last_error: dial.map(|d| d.last_error.clone()).unwrap_or_default(),
            }
        };

        // Only nodes advertising right now are listed: a recommended node
        // without a live advert drops out, and comes back when it advertises.
        let advertising = |npub: &str| {
            inner
                .directory
                .get(npub)
                .is_some_and(|n| n.valid_until > now_secs)
        };
        let mut nodes: Vec<PublicNodeView> = recommended
            .iter()
            .filter(|r| advertising(&r.npub))
            .map(|r| row(&r.npub, r.name.clone(), true))
            .collect();
        let mut others: Vec<&PublicNode> = inner
            .directory
            .values()
            .filter(|n| n.valid_until > now_secs)
            .filter(|n| !recommended.iter().any(|r| r.npub == n.npub))
            .collect();
        others.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.npub.cmp(&b.npub)));
        for node in others.into_iter().take(MAX_LISTED_OTHERS) {
            nodes.push(row(&node.npub, short_npub(&node.npub), false));
        }

        PublicNodesView {
            enabled: inner.settings.enabled,
            blocked: if gate.offline_only {
                "offline-only".to_string()
            } else if gate.internet_down {
                "no-internet".to_string()
            } else {
                String::new()
            },
            foreground: inner.foreground,
            fetching: inner.fetch.running,
            last_fetch_ms: inner.fetch.last_ms,
            relays_answered: inner.fetch.relays_answered,
            relays_asked: ADVERT_RELAYS.len() as u8,
            fetch_error: inner.fetch.error.clone(),
            target_links: TARGET_LINKS as u8,
            nodes,
        }
    }

    // --- the async driver ---------------------------------------------------

    /// Run forever: every [`TICK`], or sooner when an action wakes it, plan
    /// a tick and carry it out.
    pub async fn run(
        self: std::sync::Arc<Self>,
        control: crate::control_client::ControlClient,
        content: std::sync::Arc<crate::content::Content>,
        peer_cache: std::sync::Arc<Mutex<Vec<PeerView>>>,
        node_live: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(TICK) => {}
                _ = self.wake.notified() => {}
            }
            let gate = Gate {
                offline_only: content.is_offline_only(),
                internet_down: content.internet_looks_down(),
                node_live: node_live.load(std::sync::atomic::Ordering::Relaxed),
            };
            let connected: HashSet<String> = peer_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter(|p| p.connected && !p.npub.is_empty())
                .map(|p| p.npub.clone())
                .collect();
            let circle: HashSet<String> = content.circle_npubs().into_iter().collect();

            let plan = self.plan(now_ms(), gate, &connected, &circle);
            if plan.fetch {
                self.fetch_adverts().await;
                // A fresh read changes what can be dialled: go round again at
                // once rather than waiting out the tick. The plan above has
                // already recorded its own dials, so they still go ahead.
                self.wake.notify_one();
            }

            for npub in &plan.disconnect {
                match control
                    .request("disconnect", Some(serde_json::json!({ "npub": npub })))
                    .await
                {
                    Ok(_) => {
                        tracing::info!(npub, "public node: disconnected");
                        self.forget_dial(npub);
                    }
                    Err(e) => tracing::debug!(npub, error = %e, "public node: disconnect failed"),
                }
            }
            for (npub, addr) in &plan.connect {
                match control.connect_peer(npub, addr, &dial_transport()).await {
                    Ok(()) => tracing::info!(npub, addr, "public node: dialling"),
                    Err(e) => {
                        tracing::warn!(npub, addr, error = %e, "public node: dial refused");
                        self.record_dial_error(npub, &e);
                    }
                }
            }
        }
    }

    /// Read the advert relays and take what verifies.
    async fn fetch_adverts(&self) {
        let recommended_hex: Vec<String> = {
            self.lock().fetch.running = true;
            recommended()
                .iter()
                .filter_map(|r| PublicKey::from_bech32(&r.npub).ok())
                .map(|pk| pk.to_hex())
                .collect()
        };
        let now = now_ms() / 1000;
        let d = fips::nostr::ADVERT_IDENTIFIER;
        // fips's own advert filter (kind + `#d`, see `nostr/runtime.rs`), twice:
        // the recent adverts of everyone (bounded), and the recommended nodes
        // by author, as fips looks a node up, so hundreds of browser nodes
        // crowding the first one's limit cannot hide the ones we want most.
        let filters = vec![
            serde_json::json!({
                "kinds": [fips::nostr::ADVERT_KIND],
                "#d": [d],
                "since": now.saturating_sub(MAX_ADVERT_AGE_SECS),
                "limit": 500,
            }),
            serde_json::json!({
                "kinds": [fips::nostr::ADVERT_KIND],
                "#d": [d],
                "authors": recommended_hex,
            }),
        ];
        let reads = ADVERT_RELAYS.iter().map(|url| {
            let filters = filters.clone();
            async move {
                let mut got: Vec<Event> = Vec::new();
                let result = tokio::time::timeout(
                    FETCH_TIMEOUT,
                    crate::relay_pool::request(url, filters, |event| {
                        if got.len() < MAX_EVENTS_PER_RELAY {
                            got.push(event);
                        }
                    }),
                )
                .await;
                let answered = matches!(result, Ok(Ok(()))) || !got.is_empty();
                if let Ok(Err(e)) = &result {
                    tracing::debug!(url, error = %e, "public nodes: advert read failed");
                }
                (answered, got)
            }
        });
        let results = futures_util::future::join_all(reads).await;

        let mut answered = 0u8;
        let mut nodes = Vec::new();
        let mut rejected = 0usize;
        for (ok, events) in results {
            answered += u8::from(ok);
            for event in events {
                match parse_advert(&event, now) {
                    Ok(node) => nodes.push(node),
                    Err(_) => rejected += 1,
                }
            }
        }
        let kept = nodes.len();
        self.observe(nodes, now);
        // The first read with adverts picks the preselection; later ones keep
        // it current. Persisted, so it does not reshuffle per launch.
        if self.maintain_preselection(now_ms(), &mut os_seeded_picker()) {
            if let Err(e) = self.save() {
                tracing::warn!(error = %e, "public nodes: could not save the preselection");
            }
        }
        let mut inner = self.lock();
        inner.fetch.running = false;
        inner.fetch.last_ms = now_ms();
        inner.fetch.relays_answered = answered;
        inner.fetch.error = if answered == 0 {
            "no advert relay answered".to_string()
        } else {
            String::new()
        };
        tracing::info!(
            answered,
            kept,
            rejected,
            directory = inner.directory.len(),
            "public nodes: adverts read"
        );
    }
}

/// Redial delay for one node: doubling from the minimum per failure, capped,
/// and never under the background floor while off screen.
fn backoff_ms(dial: &DialState, foreground: bool) -> u64 {
    let base = DIAL_BACKOFF_MIN_MS
        .saturating_mul(1u64 << dial.failures.min(10))
        .min(DIAL_BACKOFF_MAX_MS);
    if foreground {
        base
    } else {
        base.max(DIAL_BACKOFF_BACKGROUND_MS)
    }
}

fn short_npub(npub: &str) -> String {
    if npub.len() > 20 {
        format!("{}…{}", &npub[..10], &npub[npub.len() - 6..])
    } else {
        npub.to_string()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// The view
// ---------------------------------------------------------------------------

/// Public nodes as Settings and the Dev tab show them.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PublicNodesView {
    /// The opt-in.
    pub enabled: bool,
    /// Why nothing is happening although it is on: `offline-only`,
    /// `no-internet`, or empty.
    pub blocked: String,
    /// Whether the app is on screen (off screen, everything slows).
    pub foreground: bool,
    /// An advert read is under way.
    pub fetching: bool,
    /// When the last advert read finished, ms since the epoch; 0 if never.
    pub last_fetch_ms: u64,
    /// How many advert relays answered the last read, of `relays_asked`.
    pub relays_answered: u8,
    pub relays_asked: u8,
    /// Why the last read got nothing; empty otherwise.
    pub fetch_error: String,
    /// How many links Myco holds at once.
    pub target_links: u8,
    /// Recommended nodes first, in their published order; then the other
    /// dialable nodes heard of, newest first.
    pub nodes: Vec<PublicNodeView>,
}

/// One public node.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PublicNodeView {
    pub npub: String,
    /// The recommended name (`test-us01`), else an abbreviated npub.
    pub name: String,
    /// join.fips.network recommends it.
    pub recommended: bool,
    /// The user wants it dialled (when the feature is on).
    pub selected: bool,
    /// It has a live, dialable advert right now.
    pub advertised: bool,
    /// The address it would be dialled at; empty without an advert.
    pub endpoint: String,
    /// The advert's `created_at`, ms; 0 without one.
    pub advertised_at_ms: u64,
    /// `connected`, `connecting`, `waiting` (selected, between dials),
    /// or `idle` (not selected, or the feature off). Only advertising nodes
    /// are listed at all.
    pub state: String,
    /// Link round trip as fips measures it, while connected.
    pub srtt_ms: Option<f64>,
    /// When the session authenticated, ms; 0 when not connected.
    pub connected_since_ms: u64,
    /// The last dial the control socket refused, if any.
    pub last_error: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};

    const NOW: u64 = 1_791_150_000;

    fn advert_with(keys: &Keys, content: &str, tags: Vec<Tag>, created_at: u64) -> Event {
        EventBuilder::new(Kind::Custom(fips::nostr::ADVERT_KIND), content)
            .tags(tags)
            .custom_created_at(Timestamp::from(created_at))
            .sign_with_keys(keys)
            .unwrap()
    }

    fn standard_tags(expires: u64) -> Vec<Tag> {
        vec![
            Tag::identifier(fips::nostr::ADVERT_IDENTIFIER),
            Tag::custom(TagKind::custom("protocol"), [ADVERT_PROTOCOL]),
            Tag::custom(TagKind::custom("version"), ["1"]),
            Tag::expiration(Timestamp::from(expires)),
        ]
    }

    fn body(endpoints: &str) -> String {
        format!(r#"{{"identifier":"fips-overlay-v1","version":1,"endpoints":[{endpoints}]}}"#)
    }

    fn advert(keys: &Keys, endpoints: &str) -> Event {
        advert_with(keys, &body(endpoints), standard_tags(NOW + 3000), NOW - 600)
    }

    /// The shape a real test node publishes is taken, with its UDP address.
    #[test]
    fn a_real_shaped_advert_parses() {
        let keys = Keys::generate();
        let event = advert(
            &keys,
            r#"{"transport":"udp","addr":"217.77.8.91:2121"},{"transport":"tcp","addr":"217.77.8.91:443"}"#,
        );
        let node = parse_advert(&event, NOW).unwrap();
        assert_eq!(node.npub, keys.public_key().to_bech32().unwrap());
        assert_eq!(node.udp, vec!["217.77.8.91:2121".parse().unwrap()]);
        assert_eq!(node.dial_addr(), Some("217.77.8.91:2121".parse().unwrap()));
        assert_eq!(node.valid_until, NOW + 3000);
    }

    /// An endpoint type this build does not know costs that endpoint only.
    #[test]
    fn unknown_transports_do_not_sink_the_advert() {
        let keys = Keys::generate();
        let event = advert(
            &keys,
            r#"{"transport":"webrtc","addr":"02ab"},{"transport":"udp","addr":"88.208.241.33:2121"}"#,
        );
        assert!(parse_advert(&event, NOW).is_ok());
    }

    /// Nothing this phone could dial over the internet is kept: NAT-only
    /// adverts, hostnames, and every private or special-purpose range.
    #[test]
    fn unroutable_endpoints_are_refused() {
        let keys = Keys::generate();
        for addr in [
            "nat",
            "localhost:2121",
            "test-us01.fips.network:2121",
            "127.0.0.1:2121",
            "10.0.0.5:2121",
            "172.16.1.1:2121",
            "192.168.1.10:2121",
            "169.254.1.1:2121",
            "100.64.0.1:2121",
            "0.0.0.0:2121",
            "198.18.0.1:2121",
            "192.0.2.1:2121",
            "224.0.0.1:2121",
            "255.255.255.255:2121",
            "[::1]:2121",
            "[fe80::1]:2121",
            "[fd00::1]:2121",
            "[2001:db8::1]:2121",
            "[::ffff:192.168.1.1]:2121",
            "1.2.3.4:0",
        ] {
            let event = advert(&keys, &format!(r#"{{"transport":"udp","addr":"{addr}"}}"#));
            assert_eq!(
                parse_advert(&event, NOW),
                Err(Reject::NoDialableEndpoint),
                "{addr} must not be dialable"
            );
        }
        // A public IPv6 address is a real endpoint, though only IPv4 is dialled.
        let event = advert(&keys, r#"{"transport":"udp","addr":"[2a01:4f8::1]:2121"}"#);
        let node = parse_advert(&event, NOW).unwrap();
        assert_eq!(node.dial_addr(), None);
    }

    /// A tampered event fails the signature check even though it parses.
    #[test]
    fn a_forged_advert_is_refused() {
        let keys = Keys::generate();
        let event = advert(&keys, r#"{"transport":"udp","addr":"217.77.8.91:2121"}"#);
        let mut json = serde_json::to_value(&event).unwrap();
        json["content"] =
            serde_json::Value::String(body(r#"{"transport":"udp","addr":"203.0.114.9:2121"}"#));
        let forged: Event = serde_json::from_value(json).unwrap();
        assert_eq!(parse_advert(&forged, NOW), Err(Reject::BadSignature));
    }

    #[test]
    fn tags_timestamps_and_size_are_enforced() {
        let keys = Keys::generate();
        let good = r#"{"transport":"udp","addr":"217.77.8.91:2121"}"#;

        let mut tags = standard_tags(NOW + 3000);
        tags[0] = Tag::identifier("fips-overlay-v1-next");
        let next = advert_with(&keys, &body(good), tags, NOW - 60);
        assert_eq!(parse_advert(&next, NOW), Err(Reject::WrongIdentifier));

        let mut tags = standard_tags(NOW + 3000);
        tags.remove(1);
        let no_protocol = advert_with(&keys, &body(good), tags, NOW - 60);
        assert_eq!(parse_advert(&no_protocol, NOW), Err(Reject::WrongProtocol));

        let expired = advert_with(&keys, &body(good), standard_tags(NOW - 1), NOW - 4000);
        assert_eq!(parse_advert(&expired, NOW), Err(Reject::Expired));

        // No expiration tag: the age cap still retires it.
        let mut tags = standard_tags(0);
        tags.pop();
        let ancient = advert_with(&keys, &body(good), tags, NOW - MAX_ADVERT_AGE_SECS - 1);
        assert_eq!(parse_advert(&ancient, NOW), Err(Reject::Expired));

        // An expiration far out does not outlive the age cap.
        let far = advert_with(&keys, &body(good), standard_tags(NOW + 999_999), NOW - 60);
        assert_eq!(
            parse_advert(&far, NOW).unwrap().valid_until,
            NOW - 60 + MAX_ADVERT_AGE_SECS
        );

        let future = advert_with(&keys, &body(good), standard_tags(NOW + 9000), NOW + 3600);
        assert_eq!(parse_advert(&future, NOW), Err(Reject::FutureDated));

        let padding = " ".repeat(MAX_CONTENT_BYTES);
        let huge = advert_with(
            &keys,
            &format!("{}{padding}", body(good)),
            standard_tags(NOW + 3000),
            NOW - 60,
        );
        assert_eq!(parse_advert(&huge, NOW), Err(Reject::TooLarge));

        let many = vec![good; MAX_ENDPOINTS + 1].join(",");
        let flood = advert(&keys, &many);
        assert_eq!(parse_advert(&flood, NOW), Err(Reject::TooManyEndpoints));

        let garbage = advert_with(&keys, "not json", standard_tags(NOW + 3000), NOW - 60);
        assert_eq!(parse_advert(&garbage, NOW), Err(Reject::BadContent));

        let wrong_kind = EventBuilder::new(Kind::TextNote, body(good))
            .tags(standard_tags(NOW + 3000))
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(parse_advert(&wrong_kind, NOW), Err(Reject::WrongKind));
    }

    /// test-us03-next, as join.fips.network lists it.
    const NEXT_NPUB: &str = "npub15m6c4ghuegx4pcde6tra8f7smn8vfv2wundyxwhkjynuerkrzmgsy09sh3";

    /// Every shipped entry is well formed and none is a `next` node, so the
    /// floor is never broken.
    #[test]
    fn the_shipped_list_is_valid() {
        for (name, npub) in SHIPPED_RECOMMENDED {
            assert!(PublicKey::from_bech32(npub).is_ok(), "{name}");
            assert!(!name.ends_with("-next"), "{name} is a next node");
            assert_ne!(*npub, NEXT_NPUB);
        }
    }

    /// A deterministic picker for tests: a fixed sequence of draws.
    fn picker(draws: &[usize]) -> impl FnMut(usize) -> usize + '_ {
        let mut i = 0;
        move |n| {
            let d = draws[i % draws.len()];
            i += 1;
            d % n
        }
    }

    fn advertising(npubs: &[&str]) -> HashSet<String> {
        npubs.iter().map(|s| s.to_string()).collect()
    }

    /// At most three, only from advertising recommended nodes, chosen by the
    /// picker rather than list order, and then left alone.
    #[test]
    fn preselection_is_random_capped_and_stable() {
        let rec: Vec<&str> = SHIPPED_RECOMMENDED.iter().map(|(_, n)| *n).collect();
        let stranger = Keys::generate().public_key().to_bech32().unwrap();
        let mut all = rec.clone();
        all.push(&stranger);
        let live = advertising(&all);

        let mut s = PublicNodeSettings::default();
        assert!(s.maintain_preselection(&live, 1_000, &mut picker(&[6, 0, 3])));
        let first: BTreeSet<String> = s.preselected.keys().cloned().collect();
        assert_eq!(first.len(), PRESELECT_MAX);
        assert!(
            !first.contains(&stranger),
            "strangers are never preselected"
        );
        assert!(first.iter().all(|n| rec.contains(&n.as_str())));
        let first_three: BTreeSet<String> = rec[..3].iter().map(|n| n.to_string()).collect();
        assert_ne!(first, first_three, "not simply the first three");
        assert!(first.iter().all(|n| s.is_selected(n)));

        // Later reads with other draws: nothing reshuffles.
        assert!(!s.maintain_preselection(&live, 2_000, &mut picker(&[1, 2, 3])));
        assert_eq!(
            s.preselected.keys().cloned().collect::<BTreeSet<_>>(),
            first
        );

        // The same draws give the same pick; other draws a different one.
        let mut a = PublicNodeSettings::default();
        let mut b = PublicNodeSettings::default();
        a.maintain_preselection(&live, 1_000, &mut picker(&[6, 0, 3]));
        b.maintain_preselection(&live, 1_000, &mut picker(&[0, 0, 0]));
        assert_eq!(
            a.preselected.keys().cloned().collect::<BTreeSet<_>>(),
            first
        );
        assert_ne!(a.preselected, b.preselected);
    }

    /// Nothing advertising: nothing is picked, and the first read with adverts
    /// does the picking. Fewer than three advertising: no top-up.
    #[test]
    fn preselection_waits_for_adverts_and_never_tops_up_with_strangers() {
        let mut s = PublicNodeSettings::default();
        assert!(!s.maintain_preselection(&HashSet::new(), 1_000, &mut picker(&[0])));
        assert!(s.preselected.is_empty());

        let stranger = Keys::generate().public_key().to_bech32().unwrap();
        let live = advertising(&[SHIPPED_RECOMMENDED[2].1, &stranger, NEXT_NPUB]);
        s.maintain_preselection(&live, 2_000, &mut picker(&[0]));
        assert_eq!(
            s.preselected.keys().cloned().collect::<Vec<_>>(),
            vec![SHIPPED_RECOMMENDED[2].1.to_string()]
        );
    }

    /// The user's word wins: a deselected node stays off and another takes
    /// its place; one gone quiet for a day is swapped for another.
    #[test]
    fn preselection_respects_the_user_and_replaces_the_long_gone() {
        let rec: Vec<&str> = SHIPPED_RECOMMENDED.iter().map(|(_, n)| *n).collect();
        let live = advertising(&rec);
        let mut s = PublicNodeSettings::default();
        s.maintain_preselection(&live, 1_000, &mut picker(&[0]));
        let gone = s.preselected.keys().next().unwrap().clone();
        let off = s.preselected.keys().nth(1).unwrap().clone();

        s.set_selected(&off, false);
        assert!(!s.is_selected(&off));
        s.maintain_preselection(&live, 2_000, &mut picker(&[0]));
        assert!(!s.is_selected(&off), "deselected stays deselected");
        let selected =
            |s: &PublicNodeSettings| s.preselected.keys().filter(|n| s.is_selected(n)).count();
        assert_eq!(selected(&s), PRESELECT_MAX, "a deselected one is replaced");

        // `gone` stops advertising for over a day: replaced.
        let quiet: HashSet<String> = live.iter().filter(|n| **n != gone).cloned().collect();
        let later = 1_000 + PRESELECT_REPLACE_AFTER_MS + 1;
        assert!(s.maintain_preselection(&quiet, later, &mut picker(&[0])));
        assert!(!s.preselected.contains_key(&gone));
        assert_eq!(selected(&s), PRESELECT_MAX);

        // A user-added stranger is selected whatever the preselection says.
        let stranger = Keys::generate().public_key().to_bech32().unwrap();
        s.set_selected(&stranger, true);
        assert!(s.is_selected(&stranger) && s.added.contains(&stranger));
        s.set_selected(&stranger, false);
        assert!(!s.added.contains(&stranger) && !s.is_selected(&stranger));
    }

    // --- plan ----------------------------------------------------------------

    fn dir_with(n: usize) -> (tempdir::Dir, PublicNodes, Vec<String>) {
        let dir = tempdir::Dir::new();
        let nodes = PublicNodes::new(
            dir.path(),
            PublicNodeSettings {
                enabled: true,
                ..Default::default()
            },
        );
        nodes.set_foreground(true);
        let rec = recommended();
        let picked: Vec<String> = rec.iter().take(n).map(|r| r.npub.clone()).collect();
        // Preselected in list order here, so the plan tests read plainly; the
        // random draw has its own tests.
        for npub in &picked {
            nodes
                .lock()
                .settings
                .preselected
                .insert(npub.clone(), NOW * 1000);
        }
        nodes.observe(
            picked.iter().enumerate().map(|(i, npub)| PublicNode {
                npub: npub.clone(),
                udp: vec![format!("203.0.113.{}:2121", i + 1).parse().unwrap()],
                created_at: NOW - 60,
                valid_until: NOW + 3000,
            }),
            NOW,
        );
        nodes.lock().fetch.last_ms = NOW * 1000;
        (dir, nodes, picked)
    }

    /// Record that the driver dialled `npub`, as `plan` does for its dials.
    fn dialled(nodes: &PublicNodes, npub: &str) {
        nodes.lock().dials.insert(
            npub.to_string(),
            DialState {
                last_attempt_ms: NOW * 1000 - 60_000,
                ..Default::default()
            },
        );
    }

    fn open_gate() -> Gate {
        Gate {
            offline_only: false,
            internet_down: false,
            node_live: true,
        }
    }

    #[test]
    fn it_dials_the_recommended_nodes_up_to_the_target() {
        let (_d, nodes, picked) = dir_with(4);
        let none = HashSet::new();
        let plan = nodes.plan(NOW * 1000, open_gate(), &none, &none);
        let dialled: Vec<&String> = plan.connect.iter().map(|(n, _)| n).collect();
        assert_eq!(
            dialled,
            vec![&picked[0], &picked[1]],
            "recommended order, capped"
        );
        assert_eq!(plan.connect[0].1, "203.0.113.1:2121");

        // In flight: the next tick dials nothing more.
        let again = nodes.plan(NOW * 1000 + 15_000, open_gate(), &none, &none);
        assert!(again.connect.is_empty());

        // One came up and one did not: after the in-flight window the missing
        // one is retried once, and counted as a failure...
        let up: HashSet<String> = [picked[0].clone()].into();
        let t1 = NOW * 1000 + 31_000;
        let later = nodes.plan(t1, open_gate(), &up, &none);
        assert_eq!(later.connect.len(), 1);
        assert_eq!(later.connect[0].0, picked[1]);
        assert_eq!(nodes.lock().dials[&picked[1]].failures, 1);
        assert_eq!(nodes.lock().dials[&picked[0]].failures, 0, "it came up");

        // ...then backs off, and the gap goes to the next candidate.
        let t2 = t1 + 31_000;
        let next = nodes.plan(t2, open_gate(), &up, &none);
        assert_eq!(next.connect.len(), 1);
        assert_eq!(next.connect[0].0, picked[2]);
    }

    #[test]
    fn mesh_only_and_a_tripped_breaker_never_dial() {
        let (_d, nodes, picked) = dir_with(3);
        let none = HashSet::new();
        let up: HashSet<String> = [picked[0].clone()].into();
        dialled(&nodes, &picked[0]);

        let offline = Gate {
            offline_only: true,
            ..open_gate()
        };
        let plan = nodes.plan(NOW * 1000, offline, &up, &none);
        assert!(plan.connect.is_empty() && !plan.fetch);
        assert_eq!(
            plan.disconnect,
            vec![picked[0].clone()],
            "mesh-only drops the link"
        );

        nodes.request_refresh();
        let down = Gate {
            internet_down: true,
            ..open_gate()
        };
        let plan = nodes.plan(NOW * 1000, down, &up, &none);
        assert_eq!(
            plan,
            TickPlan::default(),
            "no internet: keep links, dial nothing"
        );
        assert!(
            nodes.lock().fetch_requested,
            "the refresh waits for the internet"
        );
    }

    #[test]
    fn switching_off_drops_public_links_but_never_a_circle_member() {
        let (_d, nodes, picked) = dir_with(3);
        nodes.lock().settings.enabled = false;
        for npub in &picked {
            dialled(&nodes, npub);
        }
        let up: HashSet<String> = picked.iter().cloned().collect();
        let circle: HashSet<String> = [picked[1].clone()].into();
        let plan = nodes.plan(NOW * 1000, open_gate(), &up, &circle);
        let mut want = vec![picked[0].clone(), picked[2].clone()];
        want.sort();
        assert_eq!(plan.disconnect, want);
        assert!(plan.connect.is_empty());
    }

    #[test]
    fn a_deselected_node_is_dropped_and_not_redialled() {
        let (_d, nodes, picked) = dir_with(3);
        nodes.lock().settings.set_selected(&picked[0], false);
        dialled(&nodes, &picked[0]);
        let up: HashSet<String> = [picked[0].clone()].into();
        let plan = nodes.plan(NOW * 1000, open_gate(), &up, &HashSet::new());
        assert_eq!(plan.disconnect, vec![picked[0].clone()]);
        let dialled: Vec<&String> = plan.connect.iter().map(|(n, _)| n).collect();
        assert_eq!(dialled, vec![&picked[1], &picked[2]]);
    }

    #[test]
    fn off_it_fetches_only_when_asked_and_never_dials() {
        let (_d, nodes, _) = dir_with(2);
        nodes.lock().settings.enabled = false;
        let none = HashSet::new();
        assert_eq!(
            nodes.plan(NOW * 1000 + 3_600_000, open_gate(), &none, &none),
            TickPlan::default()
        );
        nodes.request_refresh();
        let plan = nodes.plan(NOW * 1000, open_gate(), &none, &none);
        assert!(plan.fetch && plan.connect.is_empty());
    }

    /// A peer that advertises publicly but reached this phone some other way
    /// (a LAN daemon, a radio) is never dropped from here, on or off; once a
    /// dial's link has been dropped on purpose, a later link is not ours either.
    #[test]
    fn only_links_it_dialled_are_ever_dropped() {
        let (_d, nodes, picked) = dir_with(3);
        let up: HashSet<String> = picked.iter().cloned().collect();
        let none = HashSet::new();

        nodes.lock().settings.enabled = false;
        assert!(nodes
            .plan(NOW * 1000, open_gate(), &up, &none)
            .disconnect
            .is_empty());

        nodes.lock().settings.enabled = true;
        nodes.lock().settings.set_selected(&picked[0], false);
        assert!(nodes
            .plan(NOW * 1000, open_gate(), &up, &none)
            .disconnect
            .is_empty());

        dialled(&nodes, &picked[0]);
        let plan = nodes.plan(NOW * 1000, open_gate(), &up, &none);
        assert_eq!(plan.disconnect, vec![picked[0].clone()]);
        nodes.forget_dial(&picked[0]);
        assert!(nodes
            .plan(NOW * 1000, open_gate(), &up, &none)
            .disconnect
            .is_empty());
    }

    /// A recommended node advertising only IPv6, which this phone does not
    /// dial, is never preselected.
    #[test]
    fn an_ipv6_only_node_is_not_preselected() {
        let (_d, nodes, _) = dir_with(0);
        let rec = recommended();
        nodes.observe(
            [PublicNode {
                npub: rec[0].npub.clone(),
                udp: vec!["[2a01:4f8::1]:2121".parse().unwrap()],
                created_at: NOW - 60,
                valid_until: NOW + 3000,
            }],
            NOW,
        );
        assert!(!nodes.maintain_preselection(NOW * 1000, &mut picker(&[0])));
        assert!(nodes.lock().settings.preselected.is_empty());
    }

    /// A process that starts off screen paces itself as off screen until
    /// Kotlin reports a start.
    #[test]
    fn it_starts_off_screen() {
        let dir = tempdir::Dir::new();
        let nodes = PublicNodes::new(dir.path(), PublicNodeSettings::default());
        assert!(!nodes.lock().foreground);
    }

    #[test]
    fn redials_back_off_and_slow_in_the_background() {
        let d = DialState {
            failures: 0,
            ..Default::default()
        };
        assert_eq!(backoff_ms(&d, true), DIAL_BACKOFF_MIN_MS);
        assert_eq!(backoff_ms(&d, false), DIAL_BACKOFF_BACKGROUND_MS);
        let worn = DialState {
            failures: 30,
            ..Default::default()
        };
        assert_eq!(backoff_ms(&worn, true), DIAL_BACKOFF_MAX_MS);

        let (_d, nodes, _) = dir_with(1);
        nodes.set_foreground(false);
        let none = HashSet::new();
        let t0 = NOW * 1000;
        assert_eq!(nodes.plan(t0, open_gate(), &none, &none).connect.len(), 1);
        assert!(nodes
            .plan(t0 + 60_000, open_gate(), &none, &none)
            .connect
            .is_empty());
        assert_eq!(
            nodes
                .plan(t0 + DIAL_BACKOFF_BACKGROUND_MS, open_gate(), &none, &none)
                .connect
                .len(),
            1
        );
    }

    #[test]
    fn the_view_lists_advertising_nodes_recommended_first() {
        let (_d, nodes, picked) = dir_with(2);
        let stranger = PublicNode {
            npub: Keys::generate().public_key().to_bech32().unwrap(),
            udp: vec!["198.51.100.200:2121".parse().unwrap()],
            created_at: NOW,
            valid_until: NOW + 3000,
        };
        nodes.observe([stranger.clone()], NOW);
        let peers = vec![PeerView {
            npub: picked[0].clone(),
            connected: true,
            srtt_ms: Some(42.0),
            ..Default::default()
        }];
        let view = nodes.view(&peers, open_gate(), NOW * 1000);
        // Two recommended nodes advertise; the rest do not and are not shown.
        assert_eq!(view.nodes.len(), 3);
        assert!(view.nodes[..2]
            .iter()
            .all(|n| n.recommended && n.advertised));
        assert_eq!(view.nodes[0].name, "test-us01");
        assert_eq!(view.nodes[0].state, "connected");
        assert_eq!(view.nodes[0].srtt_ms, Some(42.0));
        assert_eq!(view.nodes[1].state, "waiting");
        let last = view.nodes.last().unwrap();
        assert!(!last.recommended && !last.selected);
        assert_eq!(last.npub, stranger.npub);
        assert_eq!(last.state, "idle");
    }

    #[test]
    fn choices_persist_in_settings() {
        let (dir, nodes, picked) = dir_with(1);
        nodes.set_enabled(true).unwrap();
        nodes.set_selected(&picked[0], false).unwrap();
        let saved = PublicNodeSettings::load(dir.path());
        assert!(saved.enabled);
        assert!(saved.removed.contains(&picked[0]));
        assert!(nodes.set_selected("not-an-npub", true).is_err());
        // Apart from settings.json, which the reducer rewrites on its own.
        assert!(!dir.path().join("settings.json").exists());
    }

    /// A throwaway directory, removed on drop.
    mod tempdir {
        pub struct Dir(std::path::PathBuf);
        impl Dir {
            pub fn new() -> Self {
                static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let p = std::env::temp_dir().join(format!(
                    "myco-public-nodes-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                ));
                std::fs::create_dir_all(&p).unwrap();
                Self(p)
            }
            pub fn path(&self) -> &std::path::Path {
                &self.0
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
