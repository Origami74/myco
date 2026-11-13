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
//! it over HTTP; see `.planning/neighbour/neighborhood-protocol.md`.

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
}

impl LinkOffer {
    /// Compact text for a QR code or a link: base64url of the JSON.
    pub fn encode(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(self).expect("serializes"))
    }

    pub fn decode(text: &str) -> Option<Self> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(text.trim())
            .ok()?;
        let offer: Self = serde_json::from_slice(&bytes).ok()?;
        (offer.v == 1).then_some(offer)
    }
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
        .map(str::to_string);
    Some((content, name))
}

/// A NIP-98 `Authorization` header value for `method url`, signed by `keys`.
pub fn http_auth(keys: &Keys, method: &str, url: &str) -> String {
    let event = EventBuilder::new(Kind::from(K_HTTP_AUTH), "")
        .tags([
            Tag::parse(["u", url]).expect("valid tag"),
            Tag::parse(["method", method]).expect("valid tag"),
        ])
        .sign_with_keys(keys)
        .expect("signs");
    let json = serde_json::to_vec(&event).expect("serializes");
    format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD.encode(json)
    )
}

/// Who signed a NIP-98 header for `method url`, if it is valid and fresh.
pub fn check_http_auth(header: &str, method: &str, url: &str) -> Option<String> {
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
    if now().abs_diff(event.created_at.as_secs()) > AUTH_WINDOW_SECS {
        return None;
    }
    event.verify().ok()?;
    Some(event.pubkey.to_hex())
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
}

/// An invitation not yet used, on the initiator's side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Invite {
    secret_key: String,
    s: String,
    n: Vec<Shared>,
    exp: u64,
}

/// A join in progress, on the joiner's side.
#[derive(Debug, Clone)]
pub struct Joining {
    keys: Keys,
    peer: String,
    accepted: Vec<NeighborhoodId>,
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
}

/// How long after the last exchange a link still counts as reachable.
pub const REACHABLE_FOR: std::time::Duration = std::time::Duration::from_secs(120);

impl Links {
    pub fn open(data_dir: &Path) -> Self {
        let file = data_dir.join(FILE);
        let saved = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            file,
            saved: Mutex::new(saved),
            seen: Mutex::default(),
        }
    }

    fn change<T>(&self, f: impl FnOnce(&mut Saved) -> T) -> T {
        let mut saved = self.saved.lock().unwrap();
        let out = f(&mut saved);
        let tmp = self.file.with_extension("json.tmp");
        let written = serde_json::to_vec(&*saved)
            .map_err(anyhow::Error::from)
            .and_then(|b| std::fs::write(&tmp, b).map_err(Into::into))
            .and_then(|_| std::fs::rename(&tmp, &self.file).map_err(Into::into));
        if let Err(e) = written {
            tracing::warn!(error = %e, "could not save links");
        }
        out
    }

    /// Record that `link_id` just answered or called.
    pub fn mark_seen(&self, link_id: &str) {
        self.seen
            .lock()
            .unwrap()
            .insert(link_id.to_string(), std::time::Instant::now());
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
    pub fn invite(
        &self,
        urls: Vec<String>,
        neighborhoods: Vec<Shared>,
        ttl_secs: u64,
    ) -> LinkOffer {
        let keys = Keys::generate();
        let s = format!("{}{}{}", token(), token(), token());
        let offer = LinkOffer {
            v: 1,
            link: urls,
            pk: keys.public_key().to_hex(),
            s: s.clone(),
            n: neighborhoods.clone(),
            exp: now() + ttl_secs,
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
            };
            saved.links.push(link.clone());
            let response = SetupResponse {
                offer: sign_offer(&keys, our_offer, our_name),
                n: carried.into_iter().collect(),
            };
            Ok((link, response))
        })
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
            },
        ))
    }

    /// The joiner's side, after the answer: store the link.
    pub fn finish(&self, joining: Joining, response: &SetupResponse) -> Result<Link, Refused> {
        let (_, name) = read_offer(&response.offer, &joining.peer).ok_or(Refused::BadOffer)?;
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
        };
        self.change(|saved| saved.links.push(link.clone()));
        Ok(link)
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
    /// answers. Ones the link already carries are not held. Returns every
    /// id now waiting.
    pub fn hold_pending(&self, link_id: &str, add: Vec<Shared>) -> Vec<NeighborhoodId> {
        self.change(|saved| {
            let Some(link) = saved.links.iter_mut().find(|l| l.id == link_id) else {
                return Vec::new();
            };
            for shared in add {
                let known = link.neighborhoods.contains(&shared.id)
                    || link.pending.iter().any(|p| p.id == shared.id);
                if !known {
                    link.pending.push(shared);
                }
            }
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
        // Through the QR and back.
        let offer = LinkOffer::decode(&offer.encode()).unwrap();
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
    fn a_secret_works_once() {
        let (a, _) = (Links::open(&dir()), ());
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
        let h = http_auth(&keys, "POST", "http://a.fips:4873/nh/offer");
        assert_eq!(
            check_http_auth(&h, "POST", "http://a.fips:4873/nh/offer"),
            Some(keys.public_key().to_hex())
        );
        assert!(check_http_auth(&h, "GET", "http://a.fips:4873/nh/offer").is_none());
        assert!(check_http_auth(&h, "POST", "http://b.fips:4873/nh/offer").is_none());
        assert!(check_http_auth("Nostr not-base64!", "POST", "x").is_none());
    }
}
