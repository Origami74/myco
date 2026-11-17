//! Nearby: Myco's proximity extension of NAP-NEIGHBOR.
//!
//! The `nearby` neighborhood is auto-accepted, named `nearby`, and kept by
//! the runtime rather than the user: present while Nearby is on (the
//! default), gone when it is off. It is carried on **ephemeral links**, set
//! up with any device that is directly in reach right now:
//!
//! ```text
//! every 10 s, for each direct peer without an ephemeral link,
//! the side with the lower device npub dials:
//!   POST http://<their npub>.fips:4873/nh/nearby   NearbyRequest (NIP-98, fresh link key,
//!                                                  proofs of the secrets it heard)
//!   <- NearbyResponse { their fresh link key, signed offer, shared ids }
//!   <- 409 { "error": "used", "proof": i }   that secret was taken: retry with the next
//! ```
//!
//! **The nearby secret.** Each device advertises a 4-byte [`Secret`] over
//! BLE, next to its L2CAP PSM. A dialer proves it heard one: each proof is
//! `HMAC(secret, "myco-nearby:" ‖ its new link key)`. A secret works once:
//! the first setup that proves it wins and a new secret is advertised;
//! anyone racing with the same one is told `used` and tries again with the
//! next secret it hears, as many rounds as it takes. A secret is also
//! replaced every [`SECRET_FOR`] and whenever nearby starts over. The
//! server also checks the request came from a FIPS address directly in
//! reach.
//!
//! Ephemeral links carry only auto-accepted neighborhoods both sides carry.
//! They are dropped [`GRACE`] after the peer stops being direct, and are
//! never kept across launches (see `Links::open`).
//!
//! Who counts as direct is behind [`DirectPeers`], so the logic here runs on
//! the host with a fake.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use nostr::{Event, Keys};
use serde::{Deserialize, Serialize};

use super::id::NeighborhoodId;
use super::link::{check_http_auth, http_auth, read_offer, sign_offer, Link, Shared};
use super::service::LinkService;

/// The neighborhood's name. Its id is derived from it like any
/// auto-accepted neighborhood's, so every runtime meets in the same one.
pub const NAME: &str = "nearby";

/// The `nearby` neighborhood's id.
pub fn id() -> NeighborhoodId {
    NeighborhoodId::from_name(NAME)
}

/// How long an ephemeral link outlives its peer's last direct contact.
/// Long enough to ride out a radio dropping and coming back; short enough
/// that someone who left the room leaves the neighborhood.
pub const GRACE: Duration = Duration::from_secs(5 * 60);

/// How often direct peers are checked for missing links.
pub const DIAL_EVERY: Duration = Duration::from_secs(10);

/// An ephemeral link whose peer is direct but hasn't answered for this long
/// is dropped so it can be set up again: the peer restarted and forgot it.
/// Presence asks every 30 s, so this is several missed answers.
pub const STALE_AFTER: Duration = Duration::from_secs(120);

/// How a direct peer is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// A direct radio or physical link: BLE, Wi-Fi Aware, USB.
    Radio,
    /// The same local network: LAN or a hotspot, over plain UDP to a
    /// non-public address.
    Local,
}

impl Reach {
    /// The NAP's coarse class for it.
    pub fn link_class(self) -> myco_napplet_runtime::nap::neighbor::LinkClass {
        use myco_napplet_runtime::nap::neighbor::LinkClass;
        match self {
            Self::Radio => LinkClass::Proximity,
            Self::Local => LinkClass::Local,
        }
    }
}

/// A peer directly in reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectPeer {
    /// Its device npub (the FIPS node key).
    pub npub: String,
    pub reach: Reach,
}

/// Which devices are directly in reach right now.
pub trait DirectPeers: Send + Sync {
    /// Every peer directly in reach.
    fn direct(&self) -> Vec<DirectPeer>;

    /// The direct peer a request's source address belongs to. On the mesh a
    /// source is a FIPS address (`fd…`), derived from the device key, so it
    /// maps back to exactly one npub.
    fn who(&self, source: IpAddr) -> Option<DirectPeer> {
        let IpAddr::V6(source) = source.to_canonical() else {
            return None;
        };
        self.direct()
            .into_iter()
            .find(|p| fips_address(&p.npub) == Some(source))
    }
}

/// Nobody: the default until the runtime wires the node in.
pub struct NoDirectPeers;

impl DirectPeers for NoDirectPeers {
    fn direct(&self) -> Vec<DirectPeer> {
        Vec::new()
    }
}

/// The FIPS address of a device npub.
pub fn fips_address(npub: &str) -> Option<Ipv6Addr> {
    fips::PeerIdentity::from_npub(npub)
        .ok()
        .map(|p| p.address().to_ipv6())
}

/// How `peer` is reached, if it counts as nearby. `lane` is the radio
/// layer's record for the peer (`aware`, …), which tells apart what fips
/// reports as plain UDP.
///
/// - BLE, Wi-Fi Aware and USB count: a direct radio or cable.
/// - Plain UDP counts when its address is not public: the same LAN, or a
///   hotspot. Public nodes are reached at public addresses, so they don't.
/// - TCP never counts: it is the internet lane.
///
/// fips lists only its direct link peers, so nothing reached multi-hop is
/// in the snapshot to begin with.
pub fn reach_of(peer: &crate::control_client::PeerView, lane: Option<&str>) -> Option<Reach> {
    const RADIO: [&str; 3] = ["ble", "aware", "usb"];
    if !peer.connected {
        return None;
    }
    let radio_path = peer
        .paths
        .iter()
        .any(|p| p.state == "live" && RADIO.contains(&p.lane.as_str()));
    let transport = match (peer.transport.as_str(), lane) {
        ("udp", Some(lane)) => lane,
        (t, _) => t,
    };
    if radio_path || RADIO.contains(&transport) {
        return Some(Reach::Radio);
    }
    if transport == "udp" {
        let addr: std::net::SocketAddr = peer.transport_addr.parse().ok()?;
        if !crate::public_nodes::is_public_ip(addr.ip()) {
            return Some(Reach::Local);
        }
    }
    None
}

/// [`DirectPeers`] over the runtime's peer snapshot (`show_peers`, polled
/// every second) and the radios' lane record.
pub struct SnapshotPeers {
    pub peers: Arc<Mutex<Vec<crate::control_client::PeerView>>>,
    pub lanes: Box<dyn Fn() -> std::collections::HashMap<String, String> + Send + Sync>,
}

impl DirectPeers for SnapshotPeers {
    fn direct(&self) -> Vec<DirectPeer> {
        let lanes = (self.lanes)();
        self.peers
            .lock()
            .unwrap()
            .iter()
            .filter_map(|p| {
                Some(DirectPeer {
                    npub: p.npub.clone(),
                    reach: reach_of(p, lanes.get(&p.npub).map(String::as_str))?,
                })
            })
            .collect()
    }
}

/// A nearby secret: what a device advertises over BLE, used once.
pub type Secret = [u8; 4];

/// How long an unused secret is advertised before it is replaced.
pub const SECRET_FOR: Duration = Duration::from_secs(300);
/// How long a heard secret is offered in proofs.
pub const HEARD_FOR: Duration = Duration::from_secs(60);
/// Most proofs one request carries, and most secrets remembered either way.
pub const MAX_PROOFS: usize = 16;
/// How long a replaced secret is remembered, to answer `used` rather than
/// refused.
const SPENT_FOR: Duration = Duration::from_secs(300);

fn fresh_secret() -> Secret {
    let bytes = Keys::generate().secret_key().to_secret_bytes();
    [bytes[0], bytes[1], bytes[2], bytes[3]]
}

/// The proof that the holder of link key `pk` heard `secret`.
pub fn proof(secret: &Secret, pk: &str) -> String {
    hex::encode(super::link::hmac_sha256(
        secret,
        format!("myco-nearby:{pk}").as_bytes(),
    ))
}

struct Secrets {
    current: Secret,
    since: Instant,
    /// Replaced ones, newest last: used, timed out, or forgotten.
    spent: std::collections::VecDeque<(Secret, Instant)>,
}

/// Why a setup was not answered with a link.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not allowed; nothing more is said.
    Refused,
    /// Proof `i` was for a secret already used or replaced: try again with
    /// the next one.
    Used(usize),
}

/// Nearby's state on the link service.
#[derive(Default)]
pub struct Nearby {
    direct: OnceLock<Arc<dyn DirectPeers>>,
    /// This device's secret, made on first use.
    secrets: Mutex<Option<Secrets>>,
    /// Secrets heard from others over BLE, with when.
    heard: Mutex<BTreeMap<Secret, Instant>>,
    /// Woken when a new secret is heard, for a dial waiting on one.
    heard_new: tokio::sync::Notify,
    /// Devices being dialed now, so two ticks don't link twice.
    dialing: Mutex<BTreeSet<String>>,
    /// Per ephemeral link, the neighborhoods already offered on it.
    offered: Mutex<BTreeMap<String, BTreeSet<NeighborhoodId>>>,
}

impl Nearby {
    /// With `direct` deciding who is in reach.
    pub fn with(direct: Arc<dyn DirectPeers>) -> Self {
        let n = Self::default();
        n.set_direct(direct);
        n
    }

    /// Wire the node in. Once only; later calls are ignored.
    pub fn set_direct(&self, direct: Arc<dyn DirectPeers>) {
        let _ = self.direct.set(direct);
    }

    pub fn direct_peers(&self) -> Vec<DirectPeer> {
        self.direct.get().map(|d| d.direct()).unwrap_or_default()
    }

    fn who(&self, source: IpAddr) -> Option<DirectPeer> {
        self.direct.get()?.who(source)
    }

    /// The secret to advertise now, replaced first if it is older than
    /// [`SECRET_FOR`].
    pub fn secret(&self, now: Instant) -> Secret {
        let mut s = self.secrets.lock().unwrap();
        match s.as_mut() {
            Some(s) if now.saturating_duration_since(s.since) < SECRET_FOR => s.current,
            Some(s) => Self::replace(s, now),
            None => {
                let current = fresh_secret();
                *s = Some(Secrets {
                    current,
                    since: now,
                    spent: Default::default(),
                });
                current
            }
        }
    }

    fn replace(s: &mut Secrets, now: Instant) -> Secret {
        s.spent.push_back((s.current, now));
        while s.spent.len() > MAX_PROOFS
            || s.spent
                .front()
                .is_some_and(|(_, t)| now.saturating_duration_since(*t) > SPENT_FOR)
        {
            s.spent.pop_front();
        }
        s.current = fresh_secret();
        s.since = now;
        s.current
    }

    /// Advertise a new secret from now on.
    pub fn renew(&self, now: Instant) {
        self.secret(now);
        if let Some(s) = self.secrets.lock().unwrap().as_mut() {
            Self::replace(s, now);
        }
    }

    /// Check `proofs` from link key `pk` against this device's secret and,
    /// if one is for the current secret, use it up: a new one is advertised
    /// and nobody else gets in with it. Atomic, so of racing requests
    /// exactly one wins.
    fn take(&self, proofs: &[String], pk: &str, now: Instant) -> Result<(), Refusal> {
        self.secret(now);
        let mut guard = self.secrets.lock().unwrap();
        let s = guard.as_mut().ok_or(Refusal::Refused)?;
        let proofs = &proofs[..proofs.len().min(MAX_PROOFS)];
        if proofs.iter().any(|p| *p == proof(&s.current, pk)) {
            Self::replace(s, now);
            return Ok(());
        }
        for (i, p) in proofs.iter().enumerate() {
            if s.spent.iter().any(|(old, _)| *p == proof(old, pk)) {
                return Err(Refusal::Used(i));
            }
        }
        Err(Refusal::Refused)
    }

    /// A secret heard over BLE. Our own (current or old) is ignored.
    pub fn hear(&self, secret: Secret, now: Instant) {
        if let Some(s) = self.secrets.lock().unwrap().as_ref() {
            if s.current == secret || s.spent.iter().any(|(old, _)| *old == secret) {
                return;
            }
        }
        let mut heard = self.heard.lock().unwrap();
        heard.retain(|_, t| now.saturating_duration_since(*t) <= HEARD_FOR);
        let new = heard.insert(secret, now).is_none();
        while heard.len() > MAX_PROOFS {
            let oldest = heard.iter().min_by_key(|(_, t)| **t).map(|(k, _)| *k);
            match oldest {
                Some(k) => heard.remove(&k),
                None => break,
            };
        }
        drop(heard);
        if new {
            self.heard_new.notify_waiters();
        }
    }

    /// Secrets heard within [`HEARD_FOR`], newest first.
    pub fn heard(&self, now: Instant) -> Vec<Secret> {
        let mut heard: Vec<(Secret, Instant)> = self
            .heard
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, t)| now.saturating_duration_since(**t) <= HEARD_FOR)
            .map(|(k, t)| (*k, *t))
            .collect();
        heard.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
        heard.into_iter().map(|(k, _)| k).take(MAX_PROOFS).collect()
    }

    /// A heard secret turned out to be used: never offer it again.
    fn drop_heard(&self, secret: &Secret) {
        self.heard.lock().unwrap().remove(secret);
    }
}

// --- the wire -------------------------------------------------------------------

/// `POST /nh/nearby`'s body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NearbyRequest {
    /// The caller's fresh link key, hex.
    pub pk: String,
    /// The caller's offer, signed by that key.
    pub offer: Event,
    /// The auto-accepted neighborhoods the caller carries, `nearby` among
    /// them.
    pub n: Vec<NeighborhoodId>,
    /// One [`proof`] per secret the caller heard, newest first.
    #[serde(default)]
    pub proofs: Vec<String>,
}

/// The `409` body: proof `proof` was for a used secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsedSecret {
    pub error: String,
    pub proof: usize,
}

/// The answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NearbyResponse {
    /// This side's fresh link key, hex.
    pub pk: String,
    /// This side's offer, signed by that key.
    pub offer: Event,
    /// The neighborhoods now on the link: those both carry.
    pub n: Vec<NeighborhoodId>,
}

/// The auto-accepted neighborhoods this device carries, `nearby` first if
/// it is on. Empty without neighborhoods.
fn ours(svc: &LinkService) -> Vec<Shared> {
    let Some(n) = svc.neighbors.get() else {
        return Vec::new();
    };
    n.auto_accepted()
}

/// The server side of `POST /nh/nearby`. `None` means refused, for any
/// reason: a stranger learns nothing about which check failed.
pub fn answer(
    svc: &LinkService,
    source: Option<IpAddr>,
    auth: Option<&str>,
    body: &[u8],
    now: Instant,
) -> Result<NearbyResponse, Refusal> {
    let refused = || Refusal::Refused;
    // Only a device in reach. Its source address is a FIPS address, which
    // FIPS bound to its device key.
    let peer = svc
        .nearby
        .who(source.ok_or(Refusal::Refused)?)
        .ok_or(Refusal::Refused)?;
    let neighbors = svc.neighbors.get().ok_or(Refusal::Refused)?;
    if !neighbors.nearby_on() {
        return Err(refused());
    }
    let request: NearbyRequest = serde_json::from_slice(body).map_err(|_| refused())?;
    // Signed by the link key it offers, for this very URL.
    let signer = svc
        .bases
        .iter()
        .find_map(|b| check_http_auth(auth?, "POST", &format!("{b}/nh/nearby")))
        .ok_or(Refusal::Refused)?;
    if signer != request.pk {
        return Err(refused());
    }
    read_offer(&request.offer, &request.pk).ok_or(Refusal::Refused)?;
    let carried: BTreeSet<NeighborhoodId> = ours(svc)
        .into_iter()
        .map(|s| s.id)
        .filter(|id| request.n.contains(id))
        .collect();
    if !carried.contains(&id()) {
        return Err(refused());
    }
    // Last, so a refused request never uses the secret up.
    svc.nearby.take(&request.proofs, &request.pk, now)?;
    // A new request from a device we're linked with means it lost its side
    // (a restart): the old link is dead either way.
    if let Some(old) = svc.links.ephemeral_with(&peer.npub) {
        svc.links.end(&old.id);
    }
    let keys = Keys::generate();
    let pk = keys.public_key().to_hex();
    let link = svc
        .links
        .add_ephemeral(&keys, request.offer, carried.clone(), &peer.npub)
        .ok_or(Refusal::Refused)?;
    svc.links.mark_direct(&link.id, peer.reach, now);
    tracing::info!(link = %link.id, n = carried.len(), "nearby link set up");
    Ok(NearbyResponse {
        offer: sign_offer(&keys, &svc.offer(&pk), None),
        pk,
        n: carried.into_iter().collect(),
    })
}

/// How many times a dial tries again after `used`, and how long it waits
/// for a new secret each time.
const USED_ROUNDS: usize = 8;
const WAIT_FOR_SECRET: Duration = Duration::from_secs(5);

/// The dialing side: set up an ephemeral link with `peer`, whose link
/// service is at `base` (`http://<npub>.fips:4873`). Told a secret was
/// used, it drops that one and tries again with the next it hears, up to
/// [`USED_ROUNDS`] times.
pub async fn dial(svc: &LinkService, base: &str, peer: &DirectPeer) -> anyhow::Result<Link> {
    for round in 0..=USED_ROUNDS {
        let mut heard = svc.nearby.heard(Instant::now());
        // Nothing heard at all: no point waiting, the next tick asks again.
        if heard.is_empty() && round == 0 {
            anyhow::bail!("no nearby secret heard");
        }
        if heard.is_empty() {
            let woken = svc.nearby.heard_new.notified();
            let _ = tokio::time::timeout(WAIT_FOR_SECRET, woken).await;
            heard = svc.nearby.heard(Instant::now());
            if heard.is_empty() {
                anyhow::bail!("no nearby secret heard");
            }
        }
        match dial_once(svc, base, peer, &heard).await? {
            Ok(link) => return Ok(link),
            Err(i) => {
                tracing::debug!("nearby secret already used: trying the next");
                if let Some(s) = heard.get(i) {
                    svc.nearby.drop_heard(s);
                }
            }
        }
    }
    anyhow::bail!("every nearby secret tried was already used")
}

/// One setup attempt with proofs for `heard`. `Ok(Err(i))`: proof `i` was
/// for a used secret.
async fn dial_once(
    svc: &LinkService,
    base: &str,
    peer: &DirectPeer,
    heard: &[Secret],
) -> anyhow::Result<Result<Link, usize>> {
    let ours = ours(svc);
    if !ours.iter().any(|s| s.id == id()) {
        anyhow::bail!("Nearby is off");
    }
    let keys = Keys::generate();
    let pk = keys.public_key().to_hex();
    let request = NearbyRequest {
        offer: sign_offer(&keys, &svc.offer(&pk), None),
        proofs: heard.iter().map(|s| proof(s, &pk)).collect(),
        pk,
        n: ours.iter().map(|s| s.id).collect(),
    };
    let url = format!("{base}/nh/nearby");
    let resp = super::service::client_for(&url)
        .post(&url)
        .map_err(|e| anyhow::anyhow!("{url}: {}", e.0))?
        .header("authorization", http_auth(&keys, "POST", &url))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&request)?)
        .send()
        .await?;
    if resp.status() == reqwest::StatusCode::CONFLICT {
        let used: UsedSecret = serde_json::from_slice(&resp.bytes().await?)?;
        return Ok(Err(used.proof));
    }
    if !resp.status().is_success() {
        anyhow::bail!("{url}: refused ({})", resp.status());
    }
    let response: NearbyResponse = serde_json::from_slice(&resp.bytes().await?)?;
    // Only what we sent can be on the link, whatever they say.
    let carried: BTreeSet<NeighborhoodId> = response
        .n
        .iter()
        .copied()
        .filter(|id| ours.iter().any(|s| s.id == *id))
        .collect();
    if !carried.contains(&id()) {
        anyhow::bail!("their answer left out nearby");
    }
    if read_offer(&response.offer, &response.pk).is_none() {
        anyhow::bail!("their offer was not valid");
    }
    let link = svc
        .links
        .add_ephemeral(&keys, response.offer, carried, &peer.npub)
        .ok_or_else(|| anyhow::anyhow!("their offer was not valid"))?;
    svc.links.mark_direct(&link.id, peer.reach, Instant::now());
    Ok(Ok(link))
}

/// Whether this device dials `peer`: the lower device npub does, so two
/// phones that see each other set up one link, not two.
pub fn initiates(own: &str, peer: &str) -> bool {
    own < peer
}

/// What one tick decided.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Ephemeral links to end: their peer is gone past [`GRACE`], or it
    /// stopped answering while in reach ([`STALE_AFTER`]).
    pub end: Vec<String>,
    /// Direct peers to dial.
    pub dial: Vec<DirectPeer>,
}

/// One tick's bookkeeping, without the network: refresh when each
/// ephemeral link's peer was last direct, pick the links to end and the
/// peers to dial.
pub fn plan(own: &str, direct: &[DirectPeer], links: &super::link::Links, now: Instant) -> Plan {
    let mut out = Plan::default();
    for link in links.ephemeral() {
        let device = &link.ephemeral.as_ref().expect("ephemeral").device;
        if let Some(p) = direct.iter().find(|p| &p.npub == device) {
            links.mark_direct(&link.id, p.reach, now);
            let silent = links
                .last_seen(&link.id)
                .is_none_or(|t| now.saturating_duration_since(t) > STALE_AFTER);
            if silent {
                out.end.push(link.id.clone());
            }
            continue;
        }
        let gone_for = links
            .last_direct(&link.id)
            .map(|(t, _)| now.saturating_duration_since(t))
            .unwrap_or(Duration::MAX);
        if gone_for > GRACE {
            out.end.push(link.id);
        }
    }
    for p in direct {
        let linked = links
            .ephemeral_with(&p.npub)
            .is_some_and(|l| !out.end.contains(&l.id));
        if !linked && initiates(own, &p.npub) {
            out.dial.push(p.clone());
        }
    }
    out
}

/// End every ephemeral link and forget all nearby knew: Nearby was turned
/// off, nobody is in reach any more, or the node stopped or started. The
/// next peer in reach meets this device as a stranger, on fresh link keys.
pub fn end_all(svc: &LinkService) -> Vec<Link> {
    let all = svc.links.ephemeral();
    for link in &all {
        svc.links.end(&link.id);
    }
    svc.nearby.offered.lock().unwrap().clear();
    svc.nearby.dialing.lock().unwrap().clear();
    svc.nearby.heard.lock().unwrap().clear();
    svc.nearby.renew(Instant::now());
    all
}

/// One tick: end links whose peer left, dial peers newly in reach, and keep
/// each ephemeral link carrying the auto-accepted neighborhoods both sides
/// have. `base_for` gives a device npub's link service base.
pub async fn tick(
    svc: &LinkService,
    own: &str,
    base_for: &(dyn Fn(&str) -> String + Sync),
    now: Instant,
) {
    let Some(neighbors) = svc.neighbors.get().cloned() else {
        return;
    };
    if !neighbors.nearby_on() {
        for link in end_all(svc) {
            tell_ended(link);
        }
        return;
    }
    // Replaced once it is old, used or not.
    svc.nearby.secret(now);
    let direct = svc.nearby.direct_peers();
    if direct.is_empty() {
        // Nobody to tell: they are out of reach.
        if !end_all(svc).is_empty() {
            tracing::info!("nobody in reach: nearby forgotten");
        }
        return;
    }
    let plan = plan(own, &direct, &svc.links, now);
    for link_id in &plan.end {
        if let Some(link) = svc.links.all().into_iter().find(|l| &l.id == link_id) {
            svc.links.end(link_id);
            svc.nearby.offered.lock().unwrap().remove(link_id);
            tracing::info!(link = %link_id, "nearby link ended: out of reach");
            tell_ended(link);
        }
    }
    // All at once: one peer slow to answer mustn't hold up the rest.
    let dials = plan.dial.into_iter().filter_map(|peer| {
        if !svc.nearby.dialing.lock().unwrap().insert(peer.npub.clone()) {
            return None;
        }
        let base = base_for(&peer.npub);
        Some(async move {
            let result = dial(svc, &base, &peer).await;
            svc.nearby.dialing.lock().unwrap().remove(&peer.npub);
            match result {
                Ok(link) => tracing::info!(link = %link.id, "nearby link set up"),
                Err(e) => tracing::debug!(error = %e, "nearby dial failed"),
            }
        })
    });
    futures_util::future::join_all(dials.collect::<Vec<_>>()).await;
    reconcile(svc).await;
}

/// Keep ephemeral links in step with this side's auto-accepted
/// neighborhoods: offer ones joined since (the other side takes those it
/// carries too), and drop ones left.
pub async fn reconcile(svc: &LinkService) {
    let ours = ours(svc);
    for link in svc.links.ephemeral() {
        let left: Vec<NeighborhoodId> = link
            .neighborhoods
            .iter()
            .copied()
            .filter(|id| !ours.iter().any(|s| s.id == *id))
            .collect();
        let add: Vec<Shared> = {
            let mut offered = svc.nearby.offered.lock().unwrap();
            let done = offered.entry(link.id.clone()).or_default();
            let add: Vec<Shared> = ours
                .iter()
                .filter(|s| !link.neighborhoods.contains(&s.id) && !done.contains(&s.id))
                .cloned()
                .collect();
            done.extend(add.iter().map(|s| s.id));
            add
        };
        if left.is_empty() && add.is_empty() {
            continue;
        }
        if !left.is_empty() {
            svc.links.set_neighborhoods(&link.id, &[], &left);
        }
        let change = super::service::Change {
            add: add.clone(),
            remove: left,
        };
        let Ok(body) = serde_json::to_value(&change) else {
            continue;
        };
        let Ok(resp) =
            super::service::call(&link, reqwest::Method::POST, "neighborhoods", Some(body)).await
        else {
            continue;
        };
        let Ok(bytes) = resp.bytes().await else {
            continue;
        };
        let Ok(changed) = serde_json::from_slice::<super::service::Changed>(&bytes) else {
            continue;
        };
        // What they took, and we still carry.
        let took: Vec<NeighborhoodId> = add
            .iter()
            .map(|s| s.id)
            .filter(|id| changed.n.contains(id))
            .collect();
        if !took.is_empty() {
            svc.links.set_neighborhoods(&link.id, &took, &[]);
        }
    }
}

/// Tell the peer a link ended, best effort.
fn tell_ended(link: Link) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            let _ = super::service::call(&link, reqwest::Method::POST, "end", None).await;
        });
    }
}

/// Run Nearby for the life of the runtime: a [`tick`] every [`DIAL_EVERY`].
pub async fn keep_nearby(svc: LinkService, own: String) {
    let base_for = |npub: &str| format!("http://{npub}.fips:{}", crate::auth_service::AUTH_PORT);
    let mut every = tokio::time::interval(DIAL_EVERY);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        every.tick().await;
        tick(&svc, &own, &base_for, Instant::now()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_client::{PeerPath, PeerView};

    fn pv(transport: &str, addr: &str) -> PeerView {
        PeerView {
            npub: "npub1x".into(),
            connected: true,
            transport: transport.into(),
            transport_addr: addr.into(),
            ..PeerView::default()
        }
    }

    #[test]
    fn radios_and_the_local_network_count_the_internet_does_not() {
        assert_eq!(reach_of(&pv("ble", "ble0/AA"), None), Some(Reach::Radio));
        assert_eq!(reach_of(&pv("usb", ""), None), Some(Reach::Radio));
        assert_eq!(
            reach_of(&pv("udp", "[fe80::1]:2121"), Some("aware")),
            Some(Reach::Radio)
        );
        assert_eq!(
            reach_of(&pv("udp", "192.168.8.20:2121"), None),
            Some(Reach::Local),
            "same Wi-Fi"
        );
        assert_eq!(
            reach_of(&pv("udp", "[::ffff:10.0.0.4]:2121"), None),
            Some(Reach::Local)
        );
        assert_eq!(
            reach_of(&pv("udp", "8.8.8.8:2121"), None),
            None,
            "a public node"
        );
        assert_eq!(reach_of(&pv("tcp", "192.168.1.2:443"), None), None);
        let mut gone = pv("ble", "ble0/AA");
        gone.connected = false;
        assert_eq!(reach_of(&gone, None), None);
        // A live BLE path counts even while the internet path is active.
        let mut multi = pv("tcp", "8.8.8.8:443");
        multi.paths = vec![PeerPath {
            lane: "ble".into(),
            state: "live".into(),
            ..PeerPath::default()
        }];
        assert_eq!(reach_of(&multi, None), Some(Reach::Radio));
    }

    #[test]
    fn a_source_address_maps_to_the_direct_peer_it_belongs_to() {
        struct Two(Vec<DirectPeer>);
        impl DirectPeers for Two {
            fn direct(&self) -> Vec<DirectPeer> {
                self.0.clone()
            }
        }
        use nostr::nips::nip19::ToBech32;
        let npub = |k: &Keys| k.public_key().to_bech32().unwrap();
        let (a, b) = (Keys::generate(), Keys::generate());
        let peers = Two(vec![DirectPeer {
            npub: npub(&a),
            reach: Reach::Radio,
        }]);
        let a_ip = IpAddr::V6(fips_address(&npub(&a)).unwrap());
        let b_ip = IpAddr::V6(fips_address(&npub(&b)).unwrap());
        assert_eq!(a_ip.to_string().get(..2), Some("fd"));
        assert_eq!(peers.who(a_ip).map(|p| p.npub), Some(npub(&a)));
        assert_eq!(peers.who(b_ip), None, "a FIPS peer not in reach");
        assert_eq!(peers.who("127.0.0.1".parse().unwrap()), None);
    }

    #[test]
    fn the_lower_npub_dials() {
        assert!(initiates("npub1aaa", "npub1bbb"));
        assert!(!initiates("npub1bbb", "npub1aaa"));
        assert!(!initiates("npub1aaa", "npub1aaa"));
    }

    fn links() -> super::super::link::Links {
        let d = std::env::temp_dir().join(format!("myco-nearby-{}", super::super::id::token()));
        std::fs::create_dir_all(&d).unwrap();
        super::super::link::Links::open(&d)
    }

    /// An ephemeral link with `device`, as if just set up.
    fn ephemeral(links: &super::super::link::Links, device: &str) -> Link {
        let theirs = Keys::generate();
        let offer = sign_offer(&theirs, &Default::default(), Some("Bob"));
        let link = links
            .add_ephemeral(&Keys::generate(), offer, [id()].into(), device)
            .unwrap();
        links.mark_direct(&link.id, Reach::Radio, Instant::now());
        link
    }

    fn peer(npub: &str) -> DirectPeer {
        DirectPeer {
            npub: npub.into(),
            reach: Reach::Radio,
        }
    }

    #[test]
    fn only_peers_without_a_link_are_dialed_and_only_by_the_lower_side() {
        let l = links();
        ephemeral(&l, "npub1b");
        let p = plan(
            "npub1a",
            &[peer("npub1b"), peer("npub1c"), peer("npub10")],
            &l,
            Instant::now(),
        );
        assert_eq!(p.dial, [peer("npub1c")], "b is linked, 0 dials us");
        assert!(p.end.is_empty());
    }

    #[test]
    fn a_link_is_dropped_once_its_peer_is_gone_past_the_grace_period() {
        let l = links();
        let link = ephemeral(&l, "npub1b");
        let now = Instant::now();
        assert!(plan("npub1a", &[], &l, now + GRACE / 2).end.is_empty());
        assert_eq!(
            plan("npub1a", &[], &l, now + GRACE + Duration::from_secs(1)).end,
            [link.id]
        );
    }

    #[test]
    fn being_in_reach_again_restarts_the_grace_period() {
        let l = links();
        ephemeral(&l, "npub1b");
        let now = Instant::now();
        let back = now + GRACE / 4;
        // In reach, and it answered lately.
        l.mark_seen(&l.ephemeral()[0].id);
        assert!(plan("npub1a", &[peer("npub1b")], &l, back).end.is_empty());
        assert!(
            plan("npub1a", &[], &l, now + GRACE + Duration::from_secs(1))
                .end
                .is_empty()
        );
    }

    #[test]
    fn a_link_in_reach_that_stopped_answering_is_set_up_again() {
        let l = links();
        let link = ephemeral(&l, "npub1b");
        let later = Instant::now() + STALE_AFTER + Duration::from_secs(1);
        let p = plan("npub1a", &[peer("npub1b")], &l, later);
        assert_eq!(p.end, [link.id]);
        assert_eq!(p.dial, [peer("npub1b")], "and dialed afresh");
    }

    #[test]
    fn ephemeral_links_do_not_survive_a_restart() {
        let d = std::env::temp_dir().join(format!("myco-nearby-{}", super::super::id::token()));
        std::fs::create_dir_all(&d).unwrap();
        let l = super::super::link::Links::open(&d);
        ephemeral(&l, "npub1b");
        assert_eq!(l.all().len(), 1);
        assert!(super::super::link::Links::open(&d).all().is_empty());
    }

    #[test]
    fn a_secret_lets_exactly_one_racing_setup_in_and_tells_the_rest_it_was_used() {
        let n = Nearby::default();
        let now = Instant::now();
        let s = n.secret(now);
        let (alice, carol) = ("aa".repeat(32), "cc".repeat(32));
        assert_eq!(n.take(&[proof(&s, &alice)], &alice, now), Ok(()));
        assert_ne!(n.secret(now), s, "a used secret is replaced at once");
        assert_eq!(
            n.take(&["00".repeat(32), proof(&s, &carol)], &carol, now),
            Err(Refusal::Used(1)),
            "the loser hears which proof was used"
        );
        let next = n.secret(now);
        assert_eq!(
            n.take(&[proof(&next, &carol)], &carol, now),
            Ok(()),
            "and gets in with the next"
        );
    }

    #[test]
    fn a_proof_for_another_key_or_no_secret_is_refused() {
        let n = Nearby::default();
        let now = Instant::now();
        let s = n.secret(now);
        let (alice, eve) = ("aa".repeat(32), "ee".repeat(32));
        assert_eq!(
            n.take(&[proof(&s, &alice)], &eve, now),
            Err(Refusal::Refused)
        );
        assert_eq!(n.take(&[], &eve, now), Err(Refusal::Refused));
        assert_eq!(n.secret(now), s, "a refusal uses nothing up");
    }

    #[test]
    fn an_unused_secret_is_replaced_in_time_and_then_reads_as_used() {
        let n = Nearby::default();
        let now = Instant::now();
        let s = n.secret(now);
        let later = now + SECRET_FOR + Duration::from_secs(1);
        assert_ne!(n.secret(later), s);
        let pk = "aa".repeat(32);
        assert_eq!(n.take(&[proof(&s, &pk)], &pk, later), Err(Refusal::Used(0)));
    }

    #[test]
    fn our_own_secret_is_never_heard_and_heard_ones_age_out() {
        let n = Nearby::default();
        let now = Instant::now();
        n.hear(n.secret(now), now);
        assert!(n.heard(now).is_empty());
        n.hear([1, 2, 3, 4], now);
        assert_eq!(n.heard(now), vec![[1, 2, 3, 4]]);
        assert!(n.heard(now + HEARD_FOR + Duration::from_secs(1)).is_empty());
    }
}
