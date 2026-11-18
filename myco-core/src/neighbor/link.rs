//! Links: the neighborhood protocol's pairwise relationships.
//!
//! A link is set up once from a one-time secret handed over in person (QR,
//! tap, a message) and then used over any network. Each side generates a
//! **link key** for it — neither the device's FIPS key nor the person's
//! social key — so two of a person's peers cannot tie their links together
//! by key. Everything after setup is signed with the link key.
//!
//! ```text
//! A: invite()            -> LinkOffer { link URLs, A.pk, s, neighborhoods, exp }   (QR)
//! B: join(offer)         -> SetupRequest { B.pk, proof = HMAC(s, A.pk‖B.pk‖url), n, offer }
//!    POST <url>/setup
//! A: accept(request,url) -> SetupResponse { A's offer, n on the link }
//! B: finish(response)    -> link stored on both sides
//! ```
//!
//! This module is the protocol only: no sockets. The link service carries
//! it over HTTP; see `docs/design/nap-neighbor/links.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::Engine as _;
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::id::{token, NeighborhoodId};

/// The offer event's kind. A placeholder until the protocol has a number.
pub const K_OFFER: u16 = 37_516;
/// NIP-98's HTTP auth kind.
pub const K_HTTP_AUTH: u16 = 27_235;
/// How far a NIP-98 signature's time may be from ours.
pub const AUTH_WINDOW_SECS: u64 = 60;
/// How long an invitation's secret lasts by default.
pub const INVITE_TTL_SECS: u64 = 15 * 60;
/// The file under the data dir.
pub const FILE: &str = "links.json";

// --- wire types -------------------------------------------------------------

/// A neighborhood as shared when setting up or changing a link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shared {
    pub id: NeighborhoodId,
    pub name: String,
}

/// What the initiator hands over in person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkOffer {
    pub v: u8,
    /// The initiator's link service URLs, most preferred first.
    pub link: Vec<String>,
    /// The initiator's link public key, hex.
    pub pk: String,
    /// The one-time secret, hex.
    pub s: String,
    pub n: Vec<Shared>,
    /// Unix seconds; the secret is refused after this.
    pub exp: u64,
    /// Who is inviting, as they want to be called. A display hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// The link form of an invitation, for QR codes, NFC and messages, like
/// `myco://pair/…` and `myco://share/…`. Still read, no longer written.
pub const INVITE_PREFIX: &str = "myco://link/";

/// An invitation as a napplet link: the `neighborhood/join` convention
/// (NAP-INTENT), which the shell handles itself. It opens Myco where Myco
/// is installed and lands on napplet.link's web page elsewhere. The payload
/// rides in the fragment, which a browser never sends: the web host never
/// sees the one-time secret.
pub const INVITE_LINK: &str = "https://napplet.link/neighborhood/join#invite=";

impl LinkOffer {
    /// The invitation as a link:
    /// `https://napplet.link/neighborhood/join#invite=<base64url of the JSON>`.
    /// Base64url needs no escaping in a fragment.
    pub fn encode(&self) -> String {
        format!(
            "{INVITE_LINK}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(self).expect("serializes"))
        )
    }

    /// Read an invitation: the napplet link (either spelling, any case of
    /// scheme and host), the older `myco://link/…`, or the bare payload.
    pub fn decode(text: &str) -> Option<Self> {
        let text = text.trim();
        let payload = invite_payload(text).unwrap_or_else(|| text.to_string());
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload.trim_end_matches('='))
            .ok()?;
        let offer: Self = serde_json::from_slice(&bytes).ok()?;
        (offer.v == 1).then_some(offer)
    }
}

/// The `invite` value of a `neighborhood/join` napplet link
/// (`https://napplet.link/…` or `napplet:neighborhood/join…`), or the payload
/// of a `myco://link/…`.
///
/// Read exactly as the app's join sheet reads it (`InvitePreview.payload`),
/// so the invitation joined is the one the user was shown: the fragment
/// (`#invite=`, what is written) wins over the query (`?invite=`, what older
/// invitations carry); the key matches in any case; a `/` after `join` is
/// allowed; the value is percent-decoded and trimmed.
fn invite_payload(text: &str) -> Option<String> {
    if let Some(rest) = text.strip_prefix(INVITE_PREFIX) {
        return (!rest.is_empty()).then(|| rest.to_string());
    }
    let rest_at = [
        "https://napplet.link/neighborhood/join",
        "napplet:neighborhood/join",
    ]
    .iter()
    .find(|p| {
        text.get(..p.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(p))
    })
    .map(|p| p.len())?;
    let rest = &text[rest_at..];
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    if !rest.is_empty() && !rest.starts_with(['?', '#']) {
        return None;
    }
    let (before, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let query = before.strip_prefix('?').unwrap_or("");
    invite_param(fragment).or_else(|| invite_param(query))
}

/// The first `invite=` value in `a=b&c=d` parameters, percent-decoded.
fn invite_param(params: &str) -> Option<String> {
    params.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        if !key.eq_ignore_ascii_case("invite") {
            return None;
        }
        let value = percent_decode(value)?;
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// `%XX` escapes decoded; `+` is a space, as `URLDecoder` reads it. `None`
/// for a broken escape or invalid UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = s.get(i + 1..i + 3)?;
                if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return None;
                }
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// The joiner's `POST <link>/setup` body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupRequest {
    pub pk: String,
    pub proof: String,
    /// The neighborhood ids the joiner accepts.
    pub n: Vec<NeighborhoodId>,
    pub offer: Event,
}

/// The initiator's answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupResponse {
    pub offer: Event,
    /// The neighborhood ids now on the link.
    pub n: Vec<NeighborhoodId>,
}

/// One service a participant offers on a link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Service {
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub read: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub write: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub own: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferContent {
    pub services: Vec<Service>,
}

// --- crypto -------------------------------------------------------------------

/// HMAC-SHA256 (RFC 2104).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// The setup proof: the secret bound to both keys and the URL it is sent to,
/// so a relayed answer is useless at another endpoint or with another key.
pub fn proof(secret: &[u8], a_pk: &str, b_pk: &str, url: &str) -> String {
    let mut msg = Vec::new();
    msg.extend_from_slice(a_pk.as_bytes());
    msg.extend_from_slice(b_pk.as_bytes());
    msg.extend_from_slice(url.as_bytes());
    hex::encode(hmac_sha256(secret, &msg))
}

/// Equal without leaking where they differ.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn now() -> u64 {
    Timestamp::now().as_secs()
}

/// An offer event, signed by the link key.
pub fn sign_offer(keys: &Keys, content: &OfferContent, name: Option<&str>) -> Event {
    let mut tags = vec![Tag::identifier("offer")];
    if let Some(name) = name {
        tags.push(Tag::parse(["name", name]).expect("valid tag"));
    }
    EventBuilder::new(
        Kind::from(K_OFFER),
        serde_json::to_string(content).expect("serializes"),
    )
    .tags(tags)
    .sign_with_keys(keys)
    .expect("signs")
}

/// An offer from `peer_pk`, checked: their kind, their key, a valid
/// signature. `None` otherwise.
pub fn read_offer(event: &Event, peer_pk: &str) -> Option<(OfferContent, Option<String>)> {
    if event.kind.as_u16() != K_OFFER || event.pubkey.to_hex() != peer_pk {
        return None;
    }
    event.verify().ok()?;
    let content = serde_json::from_str(&event.content).ok()?;
    let name = event
        .tags
        .iter()
        .find(|t| t.kind().to_string() == "name")
        .and_then(|t| t.content())
        .map(clip_name);
    Some((content, name))
}

/// The longest name kept from a peer: a person's, or a neighborhood's.
pub const MAX_NAME_CHARS: usize = 64;
/// The most additions a peer may hold waiting on one link; past this the
/// oldest are dropped.
pub const MAX_PENDING: usize = 8;

/// A peer-supplied name as kept: trimmed, at most [`MAX_NAME_CHARS`].
pub fn clip_name(name: &str) -> String {
    name.trim().chars().take(MAX_NAME_CHARS).collect()
}

/// A NIP-98 `Authorization` header value for `method url` with `body`,
/// signed by `keys`. A `POST`, or any request with a body, carries the
/// body's sha256 in a `payload` tag; every header carries a random `nonce`,
/// so two requests in the same second are never the same event (a repeated
/// one is refused as a replay).
pub fn http_auth(keys: &Keys, method: &str, url: &str, body: &[u8]) -> String {
    let mut tags = vec![
        Tag::parse(["u", url]).expect("valid tag"),
        Tag::parse(["method", method]).expect("valid tag"),
        Tag::parse(["nonce", &token()]).expect("valid tag"),
    ];
    if method.eq_ignore_ascii_case("POST") || !body.is_empty() {
        tags.push(Tag::parse(["payload", &hex::encode(Sha256::digest(body))]).expect("valid tag"));
    }
    let event = EventBuilder::new(Kind::from(K_HTTP_AUTH), "")
        .tags(tags)
        .sign_with_keys(keys)
        .expect("signs");
    let json = serde_json::to_vec(&event).expect("serializes");
    format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD.encode(json)
    )
}

/// Who signed a NIP-98 header for `method url` with `body`, if it is valid,
/// fresh, and not seen before. A `POST` must carry the body's sha256 in its
/// `payload` tag; any other request that carries one must match too. An
/// accepted header is remembered (see [`Replays`]) and refused the second
/// time.
pub fn check_http_auth(header: &str, method: &str, url: &str, body: &[u8]) -> Option<String> {
    let event = verify_http_auth(header, method, url, body)?;
    replays().first_use(event.id, std::time::Instant::now())?;
    Some(event.pubkey.to_hex())
}

/// How long an accepted NIP-98 id is remembered: past the signing window
/// both ways, after which the timestamp check refuses it anyway.
const REPLAY_FOR: std::time::Duration = std::time::Duration::from_secs(2 * AUTH_WINDOW_SECS);
/// The most accepted NIP-98 ids remembered at once; the oldest go first.
const REPLAY_IDS: usize = 4096;

/// NIP-98 event ids accepted lately: a header is good once.
#[derive(Default)]
struct Replays {
    seen: std::collections::HashSet<nostr::EventId>,
    order: std::collections::VecDeque<(std::time::Instant, nostr::EventId)>,
}

impl Replays {
    /// `Some` the first time `id` is offered within [`REPLAY_FOR`].
    fn first_use(&mut self, id: nostr::EventId, now: std::time::Instant) -> Option<()> {
        while let Some((at, old)) = self.order.front().copied() {
            if now.saturating_duration_since(at) < REPLAY_FOR && self.order.len() < REPLAY_IDS {
                break;
            }
            self.order.pop_front();
            self.seen.remove(&old);
        }
        if !self.seen.insert(id) {
            return None;
        }
        self.order.push_back((now, id));
        Some(())
    }
}

fn replays() -> std::sync::MutexGuard<'static, Replays> {
    static REPLAYS: std::sync::OnceLock<Mutex<Replays>> = std::sync::OnceLock::new();
    REPLAYS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// [`check_http_auth`] without the replay check.
fn verify_http_auth(header: &str, method: &str, url: &str, body: &[u8]) -> Option<Event> {
    let b64 = header.strip_prefix("Nostr ")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .ok()?;
    let event: Event = serde_json::from_slice(&bytes).ok()?;
    if event.kind.as_u16() != K_HTTP_AUTH {
        return None;
    }
    let tag = |name: &str| {
        event
            .tags
            .iter()
            .find(|t| t.kind().to_string() == name)
            .and_then(|t| t.content())
            .map(str::to_string)
    };
    if tag("u")? != url || !tag("method")?.eq_ignore_ascii_case(method) {
        return None;
    }
    match tag("payload") {
        Some(payload) => {
            if !same(
                &payload.to_ascii_lowercase(),
                &hex::encode(Sha256::digest(body)),
            ) {
                return None;
            }
        }
        None if method.eq_ignore_ascii_case("POST") => return None,
        None => {}
    }
    if now().abs_diff(event.created_at.as_secs()) > AUTH_WINDOW_SECS {
        return None;
    }
    event.verify().ok()?;
    Some(event)
}

// --- the store ------------------------------------------------------------------

/// One link, from this side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    /// Local handle.
    pub id: String,
    /// This side's link secret key, hex.
    secret: String,
    /// The other side's link public key, hex.
    pub peer: String,
    /// Neighborhoods carried on the link.
    pub neighborhoods: BTreeSet<NeighborhoodId>,
    /// The other side's last offer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_offer: Option<Event>,
    /// What the other side asked to be called, or the user's name for them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub created: u64,
    /// Neighborhoods the peer asked to add, waiting for this side's user.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<Shared>,
    /// Set on an ephemeral link: one set up without a secret because the
    /// peer's device was directly in reach (see [`super::nearby`]). Such a
    /// link carries auto-accepted neighborhoods only and never outlives the
    /// encounter, or this run of the app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral: Option<Ephemeral>,
}

/// What an ephemeral link remembers about its encounter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ephemeral {
    /// The peer's device npub: the FIPS node that was in reach. Its FIPS
    /// session authenticated this key, which is what stood in for the
    /// one-time secret.
    pub device: String,
}

impl Link {
    pub fn keys(&self) -> Keys {
        Keys::parse(&self.secret).expect("stored secret is valid")
    }

    pub fn own_pk(&self) -> String {
        self.keys().public_key().to_hex()
    }

    /// The other side's services, from their verified offer.
    pub fn services(&self) -> Vec<Service> {
        self.peer_offer
            .as_ref()
            .and_then(|e| read_offer(e, &self.peer))
            .map(|(c, _)| c.services)
            .unwrap_or_default()
    }

    /// The other side's device npub: the `.fips` host its offered services
    /// are reached at (`http://npub1….fips:4873/nh/…`), if any.
    pub fn device_npub(&self) -> Option<String> {
        self.services().iter().find_map(|s| device_npub(&s.url))
    }
}

/// The device npub in a `.fips` URL (`http://npub1….fips:4873/nh`), if any.
pub fn device_npub(url: &str) -> Option<String> {
    use nostr::nips::nip19::FromBech32;
    let host = url.split("://").nth(1)?.split(['/', ':']).next()?;
    let npub = host.strip_suffix(".fips")?;
    nostr::PublicKey::from_bech32(npub).ok()?;
    Some(npub.to_string())
}

/// An invitation not yet used, on the initiator's side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Invite {
    secret_key: String,
    s: String,
    n: Vec<Shared>,
    exp: u64,
    /// Our name, as the invitation gave it: also signed into our answer.
    #[serde(default)]
    from: Option<String>,
}

/// A join in progress, on the joiner's side.
#[derive(Debug, Clone)]
pub struct Joining {
    keys: Keys,
    peer: String,
    accepted: Vec<NeighborhoodId>,
    /// The inviter's name from the invitation, if their answer has none.
    from: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    #[serde(default)]
    links: Vec<Link>,
    /// Keyed by the initiator's link public key.
    #[serde(default)]
    invites: BTreeMap<String, Invite>,
}

/// Why a setup was refused. Said to the other side only as "refused".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    UnknownInvite,
    Expired,
    BadProof,
    BadOffer,
}

/// This device's links and open invitations: `links.json`.
pub struct Links {
    file: PathBuf,
    saved: Mutex<Saved>,
    /// When each link last answered or called, this run only.
    seen: Mutex<BTreeMap<String, std::time::Instant>>,
    /// When each ephemeral link's device was last directly in reach, and
    /// over what. This run only, like the links themselves.
    direct: Mutex<BTreeMap<String, (std::time::Instant, super::nearby::Reach)>>,
    /// Bumped whenever a link starts, ends, changes what it carries, or
    /// comes back in reach: what live subscriptions follow.
    changes: tokio::sync::watch::Sender<u64>,
    /// Link relay sockets open per link, held by [`RelaySlot`]s.
    sockets: std::sync::Arc<Mutex<BTreeMap<String, usize>>>,
}

/// How long after the last exchange a link still counts as reachable.
pub const REACHABLE_FOR: std::time::Duration = std::time::Duration::from_secs(120);
/// The most link relay sockets one link may hold open at once.
pub const MAX_RELAY_SOCKETS: usize = 4;

/// One of a link's [`MAX_RELAY_SOCKETS`], given back when dropped.
pub struct RelaySlot {
    sockets: std::sync::Arc<Mutex<BTreeMap<String, usize>>>,
    link_id: String,
}

impl Drop for RelaySlot {
    fn drop(&mut self) {
        let mut sockets = self.sockets.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = sockets.get_mut(&self.link_id) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                sockets.remove(&self.link_id);
            }
        }
    }
}

/// What `links.json` holds: the links and invites, ephemeral links left
/// out — they live for this run only, in memory, secret keys and all.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Persisted<'a> {
    links: Vec<&'a Link>,
    invites: &'a BTreeMap<String, Invite>,
}

/// Write `value` to `file` whole or not at all: a temp file, synced to disk,
/// then renamed over it.
fn write_atomically(file: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    use std::io::Write as _;
    let tmp = file.with_extension("json.tmp");
    let bytes = serde_json::to_vec(value)?;
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, file)?;
    Ok(())
}

impl Links {
    pub fn open(data_dir: &Path) -> Self {
        let file = data_dir.join(FILE);
        let mut saved: Saved = match std::fs::read(&file) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(saved) => saved,
                Err(e) => {
                    // Set aside, not overwritten: a bad write must never
                    // silently lose the user's links.
                    tracing::warn!(error = %e, "links.json unreadable; keeping it aside");
                    let _ = std::fs::rename(&file, file.with_extension("json.bad"));
                    Saved::default()
                }
            },
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(error = %e, "links.json unreadable; keeping it aside");
                    let _ = std::fs::rename(&file, file.with_extension("json.bad"));
                }
                Saved::default()
            }
        };
        // Ephemeral links belong to an encounter, and the encounter ended
        // with the last run. They are no longer written; an older file may
        // still hold some, dropped at the next save.
        saved.links.retain(|l| l.ephemeral.is_none());
        Self {
            file,
            saved: Mutex::new(saved),
            seen: Mutex::default(),
            direct: Mutex::default(),
            changes: tokio::sync::watch::channel(0).0,
            sockets: Default::default(),
        }
    }

    /// Run `f` on the links, then save and tell watchers, but only if it
    /// changed anything: a refused setup or an unchanged offer costs no
    /// fsync and wakes no link socket (a stranger can send 50 refused
    /// setups a second).
    fn change<T>(&self, f: impl FnOnce(&mut Saved) -> T) -> T {
        let mut saved = self.saved.lock().unwrap();
        let before = saved.clone();
        let out = f(&mut saved);
        if *saved == before {
            return out;
        }
        let persisted = Persisted {
            links: saved
                .links
                .iter()
                .filter(|l| l.ephemeral.is_none())
                .collect(),
            invites: &saved.invites,
        };
        if let Err(e) = write_atomically(&self.file, &persisted) {
            tracing::warn!(error = %e, "could not save links");
        }
        drop(saved);
        self.changed();
        out
    }

    /// Whether link `link_id` still exists and carries `n`.
    pub fn carries(&self, link_id: &str, n: NeighborhoodId) -> bool {
        self.saved
            .lock()
            .unwrap()
            .links
            .iter()
            .any(|l| l.id == link_id && l.neighborhoods.contains(&n))
    }

    /// One of `link_id`'s relay sockets, or `None` when it holds
    /// [`MAX_RELAY_SOCKETS`] already.
    pub fn relay_slot(&self, link_id: &str) -> Option<RelaySlot> {
        let mut sockets = self.sockets.lock().unwrap();
        let open = sockets.entry(link_id.to_string()).or_default();
        if *open >= MAX_RELAY_SOCKETS {
            return None;
        }
        *open += 1;
        Some(RelaySlot {
            sockets: self.sockets.clone(),
            link_id: link_id.to_string(),
        })
    }

    fn changed(&self) {
        self.changes.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Told whenever a link starts, ends, changes what it carries, or comes
    /// back in reach.
    pub fn watch(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// Record that `link_id` just answered or called.
    pub fn mark_seen(&self, link_id: &str) {
        let back = self
            .seen
            .lock()
            .unwrap()
            .insert(link_id.to_string(), std::time::Instant::now())
            .is_none_or(|t| t.elapsed() >= REACHABLE_FOR);
        if back {
            self.changed();
        }
    }

    /// Whether `link_id` was in contact within [`REACHABLE_FOR`].
    pub fn reachable(&self, link_id: &str) -> bool {
        self.seen
            .lock()
            .unwrap()
            .get(link_id)
            .is_some_and(|t| t.elapsed() < REACHABLE_FOR)
    }

    pub fn all(&self) -> Vec<Link> {
        self.saved.lock().unwrap().links.clone()
    }

    pub fn by_peer(&self, peer_pk: &str) -> Option<Link> {
        self.saved
            .lock()
            .unwrap()
            .links
            .iter()
            .find(|l| l.peer == peer_pk)
            .cloned()
    }

    /// The links carrying `id`.
    pub fn carrying(&self, id: NeighborhoodId) -> Vec<Link> {
        self.all()
            .into_iter()
            .filter(|l| l.neighborhoods.contains(&id))
            .collect()
    }

    /// Start a link: a fresh link key and one-time secret, offered with
    /// `neighborhoods`, reachable at `urls`.
    #[cfg(test)]
    pub fn invite(
        &self,
        urls: Vec<String>,
        neighborhoods: Vec<Shared>,
        ttl_secs: u64,
    ) -> LinkOffer {
        self.invite_from(urls, neighborhoods, ttl_secs, None)
    }

    /// As [`Links::invite`], saying who it is from. The name goes only to
    /// whoever gets the invitation: it is never advertised.
    pub fn invite_from(
        &self,
        urls: Vec<String>,
        neighborhoods: Vec<Shared>,
        ttl_secs: u64,
        from: Option<String>,
    ) -> LinkOffer {
        let from = from.map(|f| f.trim().to_string()).filter(|f| !f.is_empty());
        let keys = Keys::generate();
        let s = format!("{}{}{}", token(), token(), token());
        let offer = LinkOffer {
            v: 1,
            link: urls,
            pk: keys.public_key().to_hex(),
            s: s.clone(),
            n: neighborhoods.clone(),
            exp: now() + ttl_secs,
            from: from.clone(),
        };
        self.change(|saved| {
            saved.invites.retain(|_, i| i.exp > now());
            saved.invites.insert(
                offer.pk.clone(),
                Invite {
                    secret_key: keys.secret_key().to_secret_hex(),
                    s,
                    n: neighborhoods,
                    exp: offer.exp,
                    from,
                },
            );
        });
        offer
    }

    /// The initiator's side of `POST <url>/setup`: check the proof, spend
    /// the secret, store the link, answer with our offer. `own_pk` is the
    /// link key the request was meant for (from the URL or the request).
    pub fn accept(
        &self,
        own_pk: &str,
        url: &str,
        request: &SetupRequest,
        our_offer: &OfferContent,
        our_name: Option<&str>,
    ) -> Result<(Link, SetupResponse), Refused> {
        self.change(|saved| {
            let invite = saved.invites.get(own_pk).ok_or(Refused::UnknownInvite)?;
            if invite.exp <= now() {
                saved.invites.remove(own_pk);
                return Err(Refused::Expired);
            }
            let s = hex::decode(&invite.s).map_err(|_| Refused::UnknownInvite)?;
            if !same(&proof(&s, own_pk, &request.pk, url), &request.proof) {
                return Err(Refused::BadProof);
            }
            let (_, name) = read_offer(&request.offer, &request.pk).ok_or(Refused::BadOffer)?;
            let invite = saved.invites.remove(own_pk).expect("checked above");
            let keys = Keys::parse(&invite.secret_key).map_err(|_| Refused::UnknownInvite)?;
            let carried: BTreeSet<NeighborhoodId> = invite
                .n
                .iter()
                .map(|n| n.id)
                .filter(|id| request.n.contains(id))
                .collect();
            let link = Link {
                id: token(),
                secret: keys.secret_key().to_secret_hex(),
                peer: request.pk.clone(),
                neighborhoods: carried.clone(),
                peer_offer: Some(request.offer.clone()),
                name,
                created: now(),
                pending: Vec::new(),
                ephemeral: None,
            };
            saved.links.push(link.clone());
            let response = SetupResponse {
                offer: sign_offer(&keys, our_offer, our_name.or(invite.from.as_deref())),
                n: carried.into_iter().collect(),
            };
            Ok((link, response))
        })
        .inspect(|(link, _)| self.mark_seen(&link.id))
    }

    /// The joiner's side, before the request is sent: a fresh link key, the
    /// proof for `url`, and our offer, made by `our_offer` from the new key
    /// (our link URL contains it). `accept` is which offered neighborhoods
    /// to take.
    pub fn join(
        offer: &LinkOffer,
        url: &str,
        accept: Vec<NeighborhoodId>,
        our_offer: &dyn Fn(&str) -> OfferContent,
        our_name: Option<&str>,
    ) -> Option<(SetupRequest, Joining)> {
        if offer.exp <= now() {
            return None;
        }
        let s = hex::decode(&offer.s).ok()?;
        let keys = Keys::generate();
        let pk = keys.public_key().to_hex();
        let accepted: Vec<NeighborhoodId> = accept
            .into_iter()
            .filter(|id| offer.n.iter().any(|n| n.id == *id))
            .collect();
        let signed = sign_offer(&keys, &our_offer(&pk), our_name);
        let request = SetupRequest {
            proof: proof(&s, &offer.pk, &pk, url),
            pk,
            n: accepted.clone(),
            offer: signed,
        };
        Some((
            request,
            Joining {
                keys,
                peer: offer.pk.clone(),
                accepted,
                from: offer.from.clone(),
            },
        ))
    }

    /// The joiner's side, after the answer: store the link.
    pub fn finish(&self, joining: Joining, response: &SetupResponse) -> Result<Link, Refused> {
        let (_, name) = read_offer(&response.offer, &joining.peer).ok_or(Refused::BadOffer)?;
        let name = name.or(joining.from.as_deref().map(clip_name));
        let carried: BTreeSet<NeighborhoodId> = response
            .n
            .iter()
            .copied()
            .filter(|id| joining.accepted.contains(id))
            .collect();
        let link = Link {
            id: token(),
            secret: joining.keys.secret_key().to_secret_hex(),
            peer: joining.peer,
            neighborhoods: carried,
            peer_offer: Some(response.offer.clone()),
            name,
            created: now(),
            pending: Vec::new(),
            ephemeral: None,
        };
        self.change(|saved| saved.links.push(link.clone()));
        // It just answered the setup.
        self.mark_seen(&link.id);
        Ok(link)
    }

    /// Store an ephemeral link (see [`super::nearby`]): set up without a
    /// secret, with `device` directly in reach. `keys` is this side's fresh
    /// link key, `peer_offer` the other side's checked offer.
    pub fn add_ephemeral(
        &self,
        keys: &Keys,
        peer_offer: Event,
        neighborhoods: BTreeSet<NeighborhoodId>,
        device: &str,
    ) -> Option<Link> {
        let peer = peer_offer.pubkey.to_hex();
        let (_, name) = read_offer(&peer_offer, &peer)?;
        let link = Link {
            id: token(),
            secret: keys.secret_key().to_secret_hex(),
            peer,
            neighborhoods,
            peer_offer: Some(peer_offer),
            name,
            created: now(),
            pending: Vec::new(),
            ephemeral: Some(Ephemeral {
                device: device.to_string(),
            }),
        };
        self.change(|saved| saved.links.push(link.clone()));
        self.mark_seen(&link.id);
        Some(link)
    }

    /// The ephemeral links.
    pub fn ephemeral(&self) -> Vec<Link> {
        self.all()
            .into_iter()
            .filter(|l| l.ephemeral.is_some())
            .collect()
    }

    /// The ephemeral link with `device`, if there is one.
    pub fn ephemeral_with(&self, device: &str) -> Option<Link> {
        self.ephemeral()
            .into_iter()
            .find(|l| l.ephemeral.as_ref().is_some_and(|e| e.device == device))
    }

    /// Record that an ephemeral link's device is directly in reach, and how.
    pub fn mark_direct(&self, link_id: &str, reach: super::nearby::Reach, at: std::time::Instant) {
        self.direct
            .lock()
            .unwrap()
            .insert(link_id.to_string(), (at, reach));
    }

    /// When an ephemeral link's device was last directly in reach, and how.
    pub fn last_direct(&self, link_id: &str) -> Option<(std::time::Instant, super::nearby::Reach)> {
        self.direct.lock().unwrap().get(link_id).copied()
    }

    /// When `link_id` last answered or called, this run.
    pub fn last_seen(&self, link_id: &str) -> Option<std::time::Instant> {
        self.seen.lock().unwrap().get(link_id).copied()
    }

    /// Add and remove neighborhoods on a link. Returns what it carries now.
    pub fn set_neighborhoods(
        &self,
        link_id: &str,
        add: &[NeighborhoodId],
        remove: &[NeighborhoodId],
    ) -> Option<BTreeSet<NeighborhoodId>> {
        self.change(|saved| {
            let link = saved.links.iter_mut().find(|l| l.id == link_id)?;
            link.neighborhoods.extend(add.iter().copied());
            for id in remove {
                link.neighborhoods.remove(id);
            }
            Some(link.neighborhoods.clone())
        })
    }

    /// Hold neighborhoods the peer asked to add until this side's user
    /// answers. Ones the link already carries are not held. At most
    /// [`MAX_PENDING`] wait per link, the oldest dropped first, each name
    /// clipped to [`MAX_NAME_CHARS`]. Returns every id now waiting.
    pub fn hold_pending(&self, link_id: &str, add: Vec<Shared>) -> Vec<NeighborhoodId> {
        self.change(|saved| {
            let Some(link) = saved.links.iter_mut().find(|l| l.id == link_id) else {
                return Vec::new();
            };
            for shared in add {
                let known = link.neighborhoods.contains(&shared.id)
                    || link.pending.iter().any(|p| p.id == shared.id);
                if !known {
                    link.pending.push(Shared {
                        id: shared.id,
                        name: clip_name(&shared.name),
                    });
                }
            }
            let over = link.pending.len().saturating_sub(MAX_PENDING);
            link.pending.drain(..over);
            link.pending.iter().map(|p| p.id).collect()
        })
    }

    /// Answer a pending addition: carried on yes, dropped either way.
    /// Returns the neighborhood when it was pending.
    pub fn resolve_pending(
        &self,
        link_id: &str,
        id: NeighborhoodId,
        accept: bool,
    ) -> Option<Shared> {
        self.change(|saved| {
            let link = saved.links.iter_mut().find(|l| l.id == link_id)?;
            let at = link.pending.iter().position(|p| p.id == id)?;
            let shared = link.pending.remove(at);
            if accept {
                link.neighborhoods.insert(id);
            }
            Some(shared)
        })
    }

    /// Store a newer offer from the peer, if it verifies.
    pub fn update_offer(&self, link_id: &str, event: Event) -> bool {
        self.change(|saved| {
            let Some(link) = saved.links.iter_mut().find(|l| l.id == link_id) else {
                return false;
            };
            let Some((_, name)) = read_offer(&event, &link.peer) else {
                return false;
            };
            let newer = link
                .peer_offer
                .as_ref()
                .is_none_or(|old| event.created_at >= old.created_at);
            if newer {
                link.peer_offer = Some(event);
                if name.is_some() {
                    link.name = name;
                }
            }
            newer
        })
    }

    /// Drop a link.
    pub fn end(&self, link_id: &str) -> bool {
        self.direct.lock().unwrap().remove(link_id);
        self.seen.lock().unwrap().remove(link_id);
        self.change(|saved| {
            let before = saved.links.len();
            saved.links.retain(|l| l.id != link_id);
            saved.links.len() != before
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("myco-links-{}", token()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn offer_content() -> OfferContent {
        OfferContent {
            services: vec![Service {
                kind: "relay".into(),
                url: "ws://a.fips:4870".into(),
                read: true,
                write: true,
                own: false,
            }],
        }
    }

    #[test]
    fn hmac_matches_rfc_4231() {
        assert_eq!(
            hex::encode(hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        let long_key = [0xaa; 131];
        assert_eq!(
            hex::encode(hmac_sha256(
                &long_key,
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    /// A refused setup changes nothing, so it writes nothing and wakes no
    /// watcher.
    #[test]
    fn a_refused_setup_neither_writes_nor_wakes() {
        let d = dir();
        let a = Links::open(&d);
        let url = "http://a.fips:4873/nh";
        let offer = a.invite(vec![url.into()], vec![], INVITE_TTL_SECS);
        let file = d.join(FILE);
        let written = std::fs::metadata(&file).unwrap().modified().unwrap();
        let mut watch = a.watch();
        watch.borrow_and_update();
        let (mut request, _) =
            Links::join(&offer, url, vec![], &|_| offer_content(), None).unwrap();
        request.proof = "00".repeat(32);
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(a
            .accept(&offer.pk, url, &request, &offer_content(), None)
            .is_err());
        assert!(!watch.has_changed().unwrap(), "no watcher woken");
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            written,
            "links.json not rewritten"
        );
    }

    /// Shared with the app's `InvitePreviewTest.sharedVectors`: every
    /// spelling must yield the same payload on both sides, so the core joins
    /// the invitation the join sheet showed. Keep the two lists in step.
    #[test]
    fn invite_vectors_match_the_app() {
        let payload = "eyJ2IjoxfQ";
        let first = format!("%{:02X}{}", payload.as_bytes()[0], &payload[1..]);
        for v in [
            format!("https://napplet.link/neighborhood/join?invite=other#invite={payload}"),
            format!("https://napplet.link/neighborhood/join#invite={payload}"),
            format!("https://napplet.link/neighborhood/join?invite={payload}"),
            format!("HTTPS://NAPPLET.LINK/neighborhood/join/?invite={payload}"),
            format!("https://napplet.link/neighborhood/join#INVITE={first}"),
            format!("https://napplet.link/neighborhood/join?Invite={payload}"),
            format!("napplet:neighborhood/join#invite={payload}"),
            format!("{INVITE_PREFIX}{payload}"),
        ] {
            assert_eq!(invite_payload(&v).as_deref(), Some(payload), "{v}");
        }
        for v in [
            "https://napplet.link/neighborhood/joinx?invite=x".to_string(),
            "https://example.com/neighborhood/join#invite=x".to_string(),
            "https://napplet.link/neighborhood/join".to_string(),
            "https://napplet.link/neighborhood/join#invite=".to_string(),
            "https://napplet.link/neighborhood/join#invites=x".to_string(),
            "https://napplet.link/neighborhood/join#invite=%zz".to_string(),
        ] {
            assert_eq!(invite_payload(&v), None, "{v}");
        }
    }

    /// A link carrying two invitations joins the fragment's, the one the
    /// join sheet shows; padding is accepted.
    #[test]
    fn decode_takes_the_fragment_invitation() {
        let a = Links::open(&dir());
        let shown = a.invite(
            vec!["http://a.fips:4873/nh".into()],
            vec![],
            INVITE_TTL_SECS,
        );
        let other = a.invite(
            vec!["http://m.fips:4873/nh".into()],
            vec![],
            INVITE_TTL_SECS,
        );
        let p = |o: &LinkOffer| o.encode().strip_prefix(INVITE_LINK).unwrap().to_string();
        let link = format!(
            "https://napplet.link/neighborhood/join?invite={}#invite={}",
            p(&other),
            p(&shown)
        );
        assert_eq!(LinkOffer::decode(&link), Some(shown.clone()));
        let padded =
            base64::engine::general_purpose::URL_SAFE.encode(serde_json::to_vec(&shown).unwrap());
        assert_eq!(
            LinkOffer::decode(&format!("{INVITE_LINK}{padded}")),
            Some(shown)
        );
    }

    fn setup() -> (Links, Links, Link, Link) {
        let (a, b) = (Links::open(&dir()), Links::open(&dir()));
        let home = Shared {
            id: NeighborhoodId::random(),
            name: "Home".into(),
        };
        let club = Shared {
            id: NeighborhoodId::random(),
            name: "Club".into(),
        };
        let url = "http://a.fips:4873/nh";
        let offer = a.invite(vec![url.into()], vec![home.clone(), club], INVITE_TTL_SECS);
        // Through the QR and back, as a link and bare.
        let link = offer.encode();
        assert!(link.starts_with("https://napplet.link/neighborhood/join#invite="));
        let payload = link.strip_prefix(INVITE_LINK).unwrap();
        for spelling in [
            payload.to_string(),
            format!("{INVITE_PREFIX}{payload}"),
            format!("napplet:neighborhood/join?invite={payload}"),
            format!("napplet:neighborhood/join#invite={payload}"),
            // Older invitations carried it in the query.
            format!("https://napplet.link/neighborhood/join?invite={payload}"),
            format!("https://napplet.link/neighborhood/join?x=1#invite={payload}"),
            // An alphanumeric QR upper-cases scheme and host, never the query.
            format!("HTTPS://NAPPLET.LINK/NEIGHBORHOOD/JOIN?invite={payload}#x"),
            format!("HTTPS://NAPPLET.LINK/NEIGHBORHOOD/JOIN#invite={payload}"),
        ] {
            assert_eq!(
                LinkOffer::decode(&spelling),
                Some(offer.clone()),
                "{spelling}"
            );
        }
        let offer = LinkOffer::decode(&link).unwrap();
        let (request, joining) = Links::join(
            &offer,
            url,
            vec![home.id],
            &|_| offer_content(),
            Some("Bob"),
        )
        .unwrap();
        let (a_link, response) = a
            .accept(&offer.pk, url, &request, &offer_content(), Some("Alice"))
            .unwrap();
        let b_link = b.finish(joining, &response).unwrap();
        (a, b, a_link, b_link)
    }

    #[test]
    fn a_link_is_set_up_with_the_neighborhoods_both_sides_want() {
        let (_, _, a_link, b_link) = setup();
        assert_eq!(a_link.peer, b_link.own_pk());
        assert_eq!(b_link.peer, a_link.own_pk());
        assert_eq!(a_link.neighborhoods, b_link.neighborhoods);
        assert_eq!(a_link.neighborhoods.len(), 1, "only Home was accepted");
        assert_eq!(a_link.name.as_deref(), Some("Bob"));
        assert_eq!(b_link.name.as_deref(), Some("Alice"));
        assert_eq!(a_link.services()[0].kind, "relay");
    }

    #[test]
    fn names_travel_in_the_invitation_and_the_answer_only() {
        let (a, b) = (Links::open(&dir()), Links::open(&dir()));
        let url = "http://a.fips:4873/nh";
        let offer = a.invite_from(
            vec![url.into()],
            vec![],
            INVITE_TTL_SECS,
            Some(" Alice ".into()),
        );
        assert_eq!(offer.from.as_deref(), Some("Alice"));
        let (request, joining) =
            Links::join(&offer, url, vec![], &|_| offer_content(), Some("Bob")).unwrap();
        // The inviter answers with no name of its own: the invitation's is used.
        let (a_link, response) = a
            .accept(&offer.pk, url, &request, &offer_content(), None)
            .unwrap();
        assert_eq!(a_link.name.as_deref(), Some("Bob"));
        let b_link = b.finish(joining, &response).unwrap();
        assert_eq!(b_link.name.as_deref(), Some("Alice"));
    }

    #[test]
    fn a_secret_works_once() {
        let a = Links::open(&dir());
        let url = "http://a.fips:4873/nh";
        let offer = a.invite(vec![url.into()], vec![], INVITE_TTL_SECS);
        let (request, _) = Links::join(&offer, url, vec![], &|_| offer_content(), None).unwrap();
        assert!(a
            .accept(&offer.pk, url, &request, &offer_content(), None)
            .is_ok());
        assert_eq!(
            a.accept(&offer.pk, url, &request, &offer_content(), None)
                .unwrap_err(),
            Refused::UnknownInvite
        );
    }

    #[test]
    fn a_proof_for_another_url_or_key_is_refused() {
        let a = Links::open(&dir());
        let url = "http://a.fips:4873/nh";
        let offer = a.invite(vec![url.into()], vec![], INVITE_TTL_SECS);
        let (request, _) = Links::join(
            &offer,
            "http://evil.example/nh",
            vec![],
            &|_| offer_content(),
            None,
        )
        .unwrap();
        assert_eq!(
            a.accept(&offer.pk, url, &request, &offer_content(), None)
                .unwrap_err(),
            Refused::BadProof
        );
        let (mut request, _) =
            Links::join(&offer, url, vec![], &|_| offer_content(), None).unwrap();
        request.pk = Keys::generate().public_key().to_hex();
        assert_eq!(
            a.accept(&offer.pk, url, &request, &offer_content(), None)
                .unwrap_err(),
            Refused::BadProof
        );
    }

    #[test]
    fn an_expired_invitation_is_refused() {
        let a = Links::open(&dir());
        let url = "http://a.fips:4873/nh";
        let offer = a.invite(vec![url.into()], vec![], 0);
        assert!(Links::join(&offer, url, vec![], &|_| offer_content(), None).is_none());
        let mut fresh = offer.clone();
        fresh.exp = now() + 60;
        let (request, _) = Links::join(&fresh, url, vec![], &|_| offer_content(), None).unwrap();
        assert_eq!(
            a.accept(&offer.pk, url, &request, &offer_content(), None)
                .unwrap_err(),
            Refused::Expired
        );
    }

    #[test]
    fn an_offer_signed_by_someone_else_is_refused() {
        let a = Links::open(&dir());
        let url = "http://a.fips:4873/nh";
        let offer = a.invite(vec![url.into()], vec![], INVITE_TTL_SECS);
        let (mut request, _) =
            Links::join(&offer, url, vec![], &|_| offer_content(), None).unwrap();
        request.offer = sign_offer(&Keys::generate(), &offer_content(), None);
        assert_eq!(
            a.accept(&offer.pk, url, &request, &offer_content(), None)
                .unwrap_err(),
            Refused::BadOffer
        );
    }

    #[test]
    fn links_persist_and_neighborhoods_change() {
        let d = dir();
        let a = Links::open(&d);
        let url = "http://a.fips:4873/nh";
        let offer = a.invite(vec![url.into()], vec![], INVITE_TTL_SECS);
        let (request, _) = Links::join(&offer, url, vec![], &|_| offer_content(), None).unwrap();
        let (link, _) = a
            .accept(&offer.pk, url, &request, &offer_content(), None)
            .unwrap();
        let id = NeighborhoodId::random();
        a.set_neighborhoods(&link.id, &[id], &[]).unwrap();
        let again = Links::open(&d);
        assert_eq!(again.carrying(id).len(), 1);
        assert_eq!(
            again.by_peer(&link.peer).unwrap().keys().public_key(),
            link.keys().public_key()
        );
        assert!(again.end(&link.id));
        assert!(again.all().is_empty());
    }

    #[test]
    fn a_newer_offer_replaces_the_old_one() {
        let (a, b, a_link, b_link) = setup();
        let newer = OfferContent::default();
        let event = sign_offer(&b_link.keys(), &newer, Some("Robert"));
        assert!(a.update_offer(&a_link.id, event));
        let stored = a.by_peer(&a_link.peer).unwrap();
        assert!(stored.services().is_empty());
        assert_eq!(stored.name.as_deref(), Some("Robert"));
        // Signed by the wrong key: ignored.
        let forged = sign_offer(&Keys::generate(), &offer_content(), None);
        assert!(!a.update_offer(&a_link.id, forged));
        let _ = b;
    }

    #[test]
    fn http_auth_round_trips_and_binds_method_and_url() {
        let keys = Keys::generate();
        let url = "http://a.fips:4873/nh/offer";
        let h = http_auth(&keys, "POST", url, b"{}");
        assert!(check_http_auth(&h, "GET", url, b"{}").is_none());
        assert!(check_http_auth(&h, "POST", "http://b.fips:4873/nh/offer", b"{}").is_none());
        assert!(check_http_auth("Nostr not-base64!", "POST", "x", b"").is_none());
        assert_eq!(
            check_http_auth(&h, "POST", url, b"{}"),
            Some(keys.public_key().to_hex())
        );
    }

    #[test]
    fn a_post_is_bound_to_its_body() {
        let keys = Keys::generate();
        let url = "http://a.fips:4873/nh/x/neighborhoods";
        let h = http_auth(&keys, "POST", url, b"{\"add\":[]}");
        assert!(check_http_auth(&h, "POST", url, b"{\"add\":[1]}").is_none());
        assert!(check_http_auth(&h, "POST", url, b"{\"add\":[]}").is_some());
        // A body-less POST still signs the empty body.
        let h = http_auth(&keys, "POST", url, b"");
        assert!(check_http_auth(&h, "POST", url, b"x").is_none());
        assert!(check_http_auth(&h, "POST", url, b"").is_some());
        // A POST signed without a payload tag is refused.
        let bare = EventBuilder::new(Kind::from(K_HTTP_AUTH), "")
            .tags([
                Tag::parse(["u", url]).unwrap(),
                Tag::parse(["method", "POST"]).unwrap(),
            ])
            .sign_with_keys(&keys)
            .unwrap();
        let bare = format!(
            "Nostr {}",
            base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&bare).unwrap())
        );
        assert!(check_http_auth(&bare, "POST", url, b"").is_none());
    }

    #[test]
    fn a_header_is_good_once() {
        let keys = Keys::generate();
        let url = "http://a.fips:4873/nh/x/offer";
        let h = http_auth(&keys, "GET", url, b"");
        assert!(check_http_auth(&h, "GET", url, b"").is_some());
        assert!(check_http_auth(&h, "GET", url, b"").is_none(), "a replay");
        // Two requests in the same second are still two headers.
        assert!(check_http_auth(&http_auth(&keys, "GET", url, b""), "GET", url, b"").is_some());
    }

    #[test]
    fn the_replay_memory_forgets_the_old_and_stays_bounded() {
        let mut r = Replays::default();
        let t = std::time::Instant::now();
        let id = |i: u32| nostr::EventId::from_byte_array(Sha256::digest(i.to_be_bytes()).into());
        assert!(r.first_use(id(0), t).is_some());
        assert!(r.first_use(id(0), t).is_none());
        assert!(r.first_use(id(0), t + REPLAY_FOR).is_some(), "forgotten");
        for i in 1..(REPLAY_IDS as u32 + 100) {
            r.first_use(id(i), t + REPLAY_FOR);
        }
        assert!(r.order.len() <= REPLAY_IDS && r.seen.len() <= REPLAY_IDS);
    }

    #[test]
    fn pending_additions_and_names_are_bounded() {
        let (a, _, a_link, _) = setup();
        let many: Vec<Shared> = (0..(MAX_PENDING + 3))
            .map(|_| Shared {
                id: NeighborhoodId::random(),
                name: "x".repeat(1000),
            })
            .collect();
        let waiting = a.hold_pending(&a_link.id, many.clone());
        assert_eq!(waiting.len(), MAX_PENDING);
        assert_eq!(waiting[0], many[3].id, "the oldest went first");
        let stored = a.by_peer(&a_link.peer).unwrap();
        assert!(stored
            .pending
            .iter()
            .all(|p| p.name.chars().count() == MAX_NAME_CHARS));
        let long = sign_offer(&Keys::generate(), &offer_content(), Some(&"é".repeat(500)));
        let (_, name) = read_offer(&long, &long.pubkey.to_hex()).unwrap();
        assert_eq!(name.unwrap().chars().count(), MAX_NAME_CHARS);
    }

    #[test]
    fn an_unreadable_file_is_set_aside_not_overwritten() {
        let d = dir();
        std::fs::write(d.join(FILE), b"{not json").unwrap();
        let links = Links::open(&d);
        assert!(links.all().is_empty());
        assert_eq!(
            std::fs::read(d.join("links.json.bad")).unwrap(),
            b"{not json"
        );
        links.invite(vec![], vec![], INVITE_TTL_SECS);
        assert_eq!(
            std::fs::read(d.join("links.json.bad")).unwrap(),
            b"{not json"
        );
    }

    #[test]
    fn ephemeral_links_are_never_written() {
        let d = dir();
        let links = Links::open(&d);
        let peer = Keys::generate();
        let mine = Keys::generate();
        let link = links
            .add_ephemeral(
                &mine,
                sign_offer(&peer, &offer_content(), None),
                BTreeSet::from([NeighborhoodId::random()]),
                "device",
            )
            .unwrap();
        assert_eq!(links.ephemeral().len(), 1, "kept in memory");
        let on_disk = std::fs::read_to_string(d.join(FILE)).unwrap();
        assert!(!on_disk.contains(&mine.secret_key().to_secret_hex()));
        assert!(!on_disk.contains(&link.id));
        assert!(Links::open(&d).all().is_empty());
    }

    #[test]
    fn a_link_holds_a_bounded_number_of_relay_sockets() {
        let a = Links::open(&dir());
        let slots: Vec<_> = (0..MAX_RELAY_SOCKETS)
            .map(|_| a.relay_slot("l").unwrap())
            .collect();
        assert!(a.relay_slot("l").is_none());
        assert!(a.relay_slot("other").is_some());
        drop(slots);
        assert!(a.relay_slot("l").is_some());
    }

    #[test]
    fn reads_the_device_npub_from_a_fips_url() {
        use nostr::nips::nip19::ToBech32;
        let npub = Keys::generate().public_key().to_bech32().unwrap();
        assert_eq!(
            device_npub(&format!("http://{npub}.fips:4873/nh")).as_deref(),
            Some(npub.as_str())
        );
        assert_eq!(device_npub("wss://relay.example.com"), None);
        assert_eq!(device_npub("ws://nonsense.fips:4873"), None);
    }
}
