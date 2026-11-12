//! NAP-INTENT — a napplet asks the shell to open *another* napplet by role.
//!
//! ```text
//! -> { "type": "intent.available", "id": "a1", "archetype": "profile" }
//! <- { "type": "intent.available.result", "id": "a1",
//!      "availability": { "archetype": "profile", "available": true, "hasDefault": false,
//!        "candidates": [ { "id": "npub1…:profiles", "dTag": "profiles", "title": "Profiles",
//!                          "actions": ["open"], "conventions": ["napplet:profile/open"],
//!                          "contracts": [ { "convention": "napplet:profile/open",
//!                                           "params": ["pubkey"] } ],
//!                          "isDefault": false } ] } }
//! -> { "type": "intent.invoke", "id": "i1",
//!      "request": { "archetype": "profile", "action": "open",
//!                   "convention": "napplet:profile/open", "payload": { "pubkey": "…" },
//!                   "handlerHint": { "address": "35129:<hex>:profiles", "relays": ["wss://…"] } } }
//! <- { "type": "intent.invoke.result", "id": "i1",
//!      "result": { "ok": true, "archetype": "profile", "action": "open", "handled": true,
//!                  "handler": "npub1…:profiles", "windowId": "napplet-7",
//!                  "convention": "napplet:profile/open" } }
//! <- { "type": "intent.deliver",                                    (to the handler)
//!      "delivery": { "sender": "npub1…:feed", "archetype": "profile", "action": "open",
//!                    "convention": "napplet:profile/open", "payload": { "pubkey": "…" } } }
//! <- { "type": "intent.changed", "availability": { … } }              (pushed)
//! ```
//!
//! The split with the host is the same as NAP-LINK's. This module validates
//! the request and knows the resolution rules ([`resolve`]); what it cannot
//! know is what is on screen, so a valid `intent.invoke` comes back as
//! [`Outcome::Intent`] and the host resolves it against its catalog, shows a
//! chooser when one is needed, opens or focuses the handler's window, and
//! answers with [`IntentRequest::handled`] or [`IntentRequest::failed`].
//!
//! `intent.available` and `intent.handlers` are answered here, from the
//! [`IntentCatalog`] seam on the context — the installed-napplet catalog, as
//! the spec requires, so a handler that is not running is still found.
//!
//! ## Two generations of napplet
//!
//! The wire carries both. Napplets built against the first NAP-INTENT draft
//! read a candidate's `dTag`, an answer's `handled`, and take their payload
//! as an `inc.event` on the convention's topic. Current ones read `id` and
//! `contracts`, need `handler` on every accepted answer, and take the payload
//! as `intent.deliver`. Every candidate and answer carries both sets of
//! fields, and the host delivers both ways (see `myco-core`'s `intent`).
//!
//! Catalog identifiers ([`IntentHandler::key`]) are the host's: opaque to a
//! napplet, distinct per publisher and per napplet — never a bare `d` tag.
//!
//! The payload is opaque and never read here. It is only measured: a payload
//! over [`MAX_PAYLOAD_BYTES`] of JSON is refused, because it is held by the
//! host until the handler is ready for it.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::dispatch::{NapContext, Outcome};
use crate::manifest::{convention_archetype, convention_intent, is_convention, is_slug, Archetype};
use crate::napplet_link::NappletLink;
use crate::seams::Envelope;
use crate::session::Session;

/// The largest payload accepted, as serialized JSON.
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;

/// The action a request without one performs.
pub const DEFAULT_ACTION: &str = "open";

/// `error`: nothing installed can take the role.
pub const NO_HANDLER: &str = "no handler";
/// `error`: the role has handlers, but none (or not the resolved one)
/// performs the action.
pub const UNSUPPORTED_ACTION: &str = "unsupported action";
/// `error`: the role has handlers for the action, but none (or not the
/// resolved one) accepts the convention.
pub const UNSUPPORTED_CONVENTION: &str = "unsupported convention";
/// `error`: the user closed the "open with…" chooser.
pub const USER_CANCELLED: &str = "user cancelled";
/// `error`: resolved, but the handler could not be opened.
pub const INVOKE_FAILED: &str = "invoke failed";
/// `error`: the request itself is malformed.
pub const INVALID_REQUEST: &str = "invalid request";
/// `error`: the payload is over [`MAX_PAYLOAD_BYTES`].
pub const PAYLOAD_TOO_LARGE: &str = "payload too large";
/// `error`: the convention does not parse, carries a query or fragment, or
/// names another archetype or action than the request.
pub const INVALID_CONVENTION: &str = "invalid convention";
/// `error`: an `intent.launch` target that is not a napplet coordinate.
pub const INVALID_TARGET: &str = "invalid target";
/// `error`: the host would not launch it now (policy, a review already up).
pub const LAUNCH_REJECTED: &str = "launch rejected";

/// The manifest kind of a root napplet, which `launch` may also name.
pub const ROOT_KIND: u16 = 15129;

/// The manifest kind a `handlerHint` may name: a named napplet.
pub const HINT_KIND: u16 = 35129;
/// The most relay hints kept from a `handlerHint`.
pub const MAX_HINT_RELAYS: usize = 8;

/// One installed napplet (or shell built-in) that can be opened by role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentHandler {
    /// The handler's catalog identifier — for a napplet, `<npub>:<d>` (or the
    /// bare `<npub>` of a root napplet). Sent to napplets as the candidate's
    /// `id`, an answer's `handler` and a delivery's `sender`; opaque to them.
    /// Distinct per publisher, as the spec requires: a bare `d` tag would let
    /// one author's napplet stand in for another's.
    pub key: String,
    /// The handler's `d` tag, for napplets built against the first draft,
    /// which read a candidate's `dTag`.
    pub d_tag: String,
    /// The `35129:<hex>:<d>` coordinate of a named napplet, which a
    /// `handlerHint` is matched against. `None` for root napplets and
    /// built-ins.
    pub address: Option<String>,
    pub title: Option<String>,
    /// How the host opens it — for a napplet, the pointer its Library entry
    /// launches with. Opaque here and never sent to a napplet.
    pub pointer: String,
    /// The roles it declared, each with the convention it accepts for it.
    pub archetypes: Vec<Archetype>,
}

/// What the catalog holds at one moment: every handler, and the user's
/// default per archetype (by handler key).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntentCatalogSnapshot {
    pub handlers: Vec<IntentHandler>,
    pub defaults: BTreeMap<String, String>,
    /// Every installed named napplet, role or not, by its `35129:<hex>:<d>`
    /// coordinate: what a shell handler that opens a napplet by hint needs.
    pub installed: BTreeMap<String, InstalledNapplet>,
}

/// An installed napplet, as the host opens it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstalledNapplet {
    /// See [`IntentHandler::pointer`].
    pub pointer: String,
    pub title: String,
}

/// Where NAP-INTENT reads the installed-napplet catalog from. Implemented by
/// the host over its Library, the verified manifests' `archetype` tags and
/// the user's default handlers.
#[async_trait]
pub trait IntentCatalog: Send + Sync {
    async fn snapshot(&self) -> IntentCatalogSnapshot;
}

/// A catalog with nothing in it: every archetype is unavailable.
pub struct NoIntents;

#[async_trait]
impl IntentCatalog for NoIntents {
    async fn snapshot(&self) -> IntentCatalogSnapshot {
        IntentCatalogSnapshot::default()
    }
}

/// A fixed catalog, for tests and harnesses.
pub struct StaticIntents(pub IntentCatalogSnapshot);

#[async_trait]
impl IntentCatalog for StaticIntents {
    async fn snapshot(&self) -> IntentCatalogSnapshot {
        self.0.clone()
    }
}

/// One candidate for an archetype: the spec's `IntentCandidate`, plus the
/// host's key for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The catalog identifier, [`IntentHandler::key`].
    pub key: String,
    pub d_tag: String,
    /// See [`IntentHandler::address`].
    pub address: Option<String>,
    pub title: Option<String>,
    /// See [`IntentHandler::pointer`].
    pub pointer: String,
    /// Derived from the conventions: each one's `<intent>` part, deduplicated.
    pub actions: Vec<String>,
    pub conventions: Vec<String>,
    /// Each convention with the parameter names its `i` tag advertised.
    pub contracts: Vec<(String, Vec<String>)>,
    pub is_default: bool,
}

impl Candidate {
    /// The wire `IntentCandidate`: the spec's `id` and `contracts`, and the
    /// first draft's `dTag` beside them.
    pub fn to_json(&self) -> Value {
        let contracts: Vec<Value> = self
            .contracts
            .iter()
            .map(|(convention, params)| json!({"convention": convention, "params": params}))
            .collect();
        let mut out = json!({
            "id": self.key,
            "dTag": self.d_tag,
            "actions": self.actions,
            "conventions": self.conventions,
            "contracts": contracts,
            "isDefault": self.is_default,
        });
        if let Some(title) = &self.title {
            out["title"] = json!(title);
        }
        out
    }

    /// The convention this candidate would be delivered `request` on, or
    /// `None` when it cannot take it. An explicit convention must be one the
    /// candidate accepts for the role; with none, the archetype's own
    /// `napplet:<archetype>/<action>` if accepted, else the first accepted
    /// convention whose intent is the action.
    fn convention_for(
        &self,
        archetype: &str,
        action: &str,
        wanted: Option<&str>,
    ) -> Option<String> {
        if !self.actions.iter().any(|a| a == action) {
            return None;
        }
        if let Some(wanted) = wanted {
            return self.conventions.iter().find(|c| *c == wanted).cloned();
        }
        let own = format!("napplet:{archetype}/{action}");
        if self.conventions.contains(&own) {
            return Some(own);
        }
        self.conventions
            .iter()
            .find(|c| convention_intent(c) == Some(action))
            .cloned()
    }
}

/// Every candidate for `archetype`, in catalog order.
pub fn candidates(catalog: &IntentCatalogSnapshot, archetype: &str) -> Vec<Candidate> {
    let default = catalog.defaults.get(archetype);
    catalog
        .handlers
        .iter()
        .filter_map(|h| {
            let mut contracts: Vec<(String, Vec<String>)> = Vec::new();
            for a in h.archetypes.iter().filter(|a| a.slug == archetype) {
                if !contracts.iter().any(|(c, _)| *c == a.convention) {
                    contracts.push((a.convention.clone(), a.params.clone()));
                }
            }
            if contracts.is_empty() {
                return None;
            }
            let conventions: Vec<String> = contracts.iter().map(|(c, _)| c.clone()).collect();
            let mut actions: Vec<String> = Vec::new();
            for c in &conventions {
                if let Some(intent) = convention_intent(c) {
                    if !actions.iter().any(|a| a == intent) {
                        actions.push(intent.to_string());
                    }
                }
            }
            Some(Candidate {
                key: h.key.clone(),
                d_tag: h.d_tag.clone(),
                address: h.address.clone(),
                title: h.title.clone(),
                pointer: h.pointer.clone(),
                actions,
                conventions,
                contracts,
                is_default: default == Some(&h.key),
            })
        })
        .collect()
}

/// The spec's `IntentAvailability` for `archetype`.
pub fn availability(catalog: &IntentCatalogSnapshot, archetype: &str) -> Value {
    let found = candidates(catalog, archetype);
    json!({
        "archetype": archetype,
        "available": !found.is_empty(),
        "candidates": found.iter().map(Candidate::to_json).collect::<Vec<_>>(),
        "hasDefault": found.iter().any(|c| c.is_default),
    })
}

/// Every archetype the catalog can satisfy, sorted.
pub fn archetypes(catalog: &IntentCatalogSnapshot) -> Vec<String> {
    let mut out: Vec<String> = catalog
        .handlers
        .iter()
        .flat_map(|h| h.archetypes.iter().map(|a| a.slug.clone()))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Availability for every archetype the catalog can satisfy — `handlers()`.
pub fn all_availability(catalog: &IntentCatalogSnapshot) -> Vec<Value> {
    archetypes(catalog)
        .iter()
        .map(|a| availability(catalog, a))
        .collect()
}

/// Whom the caller asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandlerChoice {
    /// `handler` absent or `"default"`.
    Default,
    /// `"choose"`: always the chooser.
    Choose,
    /// A specific handler's catalog identifier. The spec wants the user to
    /// have authorised cross-napplet targeting first; Myco has no such grant,
    /// so this is resolved as [`HandlerChoice::Choose`] — the user picks, and
    /// the name the caller gave decides nothing.
    Named(String),
}

/// A `handlerHint`: the napplet the caller recommends for the request, by
/// its `35129:<hex>:<d>` coordinate. A recommendation, never a selection —
/// the user's default wins over it, and whether the napplet can take the
/// request is read from its verified manifest, not from the hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerHint {
    /// The coordinate exactly as sent.
    pub address: String,
    /// Its pubkey, lowercase hex.
    pub author: String,
    /// Its `d` tag: non-empty, opaque, never normalised.
    pub d_tag: String,
    /// Where to look for it. Untrusted discovery input; at most
    /// [`MAX_HINT_RELAYS`].
    pub relays: Vec<String>,
}

impl HandlerHint {
    /// Parse a `handlerHint` object. `None` for anything that is not a kind
    /// [`HINT_KIND`] coordinate with a 64-hex-digit lowercase pubkey and a
    /// non-empty `d`, or whose `relays` is not a list of strings.
    pub fn parse(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let address = object.get("address")?.as_str()?;
        let mut parts = address.splitn(3, ':');
        let kind = parts.next()?;
        let author = parts.next()?;
        let d_tag = parts.next()?;
        if kind != HINT_KIND.to_string()
            || author.len() != 64
            || !author
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            || d_tag.is_empty()
        {
            return None;
        }
        let relays = match object.get("relays") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(list)) => {
                let strings: Option<Vec<String>> = list
                    .iter()
                    .map(|r| r.as_str().map(str::to_string))
                    .collect();
                strings?
                    .into_iter()
                    .filter(|r| r.len() <= 512)
                    .take(MAX_HINT_RELAYS)
                    .collect()
            }
            Some(_) => return None,
        };
        Some(Self {
            address: address.to_string(),
            author: author.to_string(),
            d_tag: d_tag.to_string(),
            relays,
        })
    }
}

/// The window hints. Advisory; the host decides.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntentBehavior {
    pub focus: Option<bool>,
    pub new_window: Option<bool>,
    pub reuse: Option<bool>,
}

/// A valid `intent.invoke`, awaiting the host.
#[derive(Debug, Clone, PartialEq)]
pub struct IntentRequest {
    pub archetype: String,
    pub action: String,
    pub convention: Option<String>,
    pub payload: Option<Value>,
    pub handler: HandlerChoice,
    pub behavior: IntentBehavior,
    /// The napplet the caller recommends, if any.
    pub hint: Option<HandlerHint>,
    call: Envelope,
    /// Made from a `napplet.link` rather than sent as `intent.invoke`. No
    /// one is waiting for an `intent.invoke.result`: a napplet that opened
    /// the link is answered `link.open.result` once, by the host, and a link
    /// from outside the app has nobody to answer at all.
    from_link: bool,
    /// The caller was already answered: the host accepted the request before
    /// any handler was open ([`IntentRequest::accept`]). What follows is
    /// delivery, which the caller does not hear about.
    answered: bool,
}

impl IntentRequest {
    /// The request a `napplet.link` stands for: its archetype, its intent as
    /// the action, its exact convention, and its query as the payload. The
    /// handler is the default — a link names a role, never a napplet.
    pub fn from_link(link: &NappletLink) -> Self {
        Self {
            archetype: link.archetype.clone(),
            action: link.intent.clone(),
            convention: Some(link.convention()),
            payload: Some(link.payload()),
            handler: HandlerChoice::Default,
            behavior: IntentBehavior::default(),
            hint: None,
            call: Envelope::new("intent.invoke"),
            from_link: true,
            answered: false,
        }
    }

    /// Whether [`IntentRequest::handled`] and [`IntentRequest::failed`] have
    /// anyone to go to. False for a request made from a link — those are
    /// answered by the host, in the link's own terms — and for one already
    /// answered by [`IntentRequest::accept`].
    pub fn answers_caller(&self) -> bool {
        !self.from_link && !self.answered
    }

    /// Accept the request before its handler is open: the spec's `ok: true`,
    /// which says the runtime took responsibility for delivery and nothing
    /// more. Used when the handler is still to be installed. Every later
    /// answer is suppressed ([`IntentRequest::answers_caller`]).
    pub fn accept(&mut self, handler: &str, convention: &str) -> Envelope {
        self.answered = true;
        self.result(json!({
            "ok": true,
            "archetype": self.archetype,
            "action": self.action,
            "handled": true,
            "handler": handler,
            "convention": convention,
        }))
    }

    /// The convention a delivery of this request carries when the handler
    /// did not narrow it: the request's own, else `napplet:<archetype>/<action>`.
    pub fn requested_convention(&self) -> String {
        self.convention
            .clone()
            .unwrap_or_else(|| format!("napplet:{}/{}", self.archetype, self.action))
    }

    /// The answer once a handler took it: its catalog identifier, the window
    /// it went to and the convention the payload is delivered on.
    pub fn handled(&self, handler: &str, window_id: &str, convention: &str) -> Envelope {
        self.result(json!({
            "ok": true,
            "archetype": self.archetype,
            "action": self.action,
            "handled": true,
            "handler": handler,
            "windowId": window_id,
            "convention": convention,
        }))
    }

    /// The answer when nothing took it.
    pub fn failed(&self, error: &str) -> Envelope {
        failed_result(&self.call, &self.archetype, &self.action, error)
    }

    /// The call's correlation id.
    pub fn id(&self) -> Option<&str> {
        self.call.id.as_deref()
    }

    fn result(&self, result: Value) -> Envelope {
        self.call.to_result().with_field("result", result)
    }
}

fn failed_result(call: &Envelope, archetype: &str, action: &str, error: &str) -> Envelope {
    call.to_result().with_field(
        "result",
        json!({
            "ok": false,
            "archetype": archetype,
            "action": action,
            "handled": false,
            "error": error,
        }),
    )
}

/// What the catalog says about a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Route to this one, delivering on `convention`.
    Handler {
        candidate: Candidate,
        convention: String,
    },
    /// Ask the user. Every entry can take the request, each with the
    /// convention it would be delivered on.
    Choose(Vec<(Candidate, String)>),
    /// Nothing can take it; the `error` to answer with.
    Refused(&'static str),
}

/// Resolve `request` against `catalog`, in the spec's order:
///
/// 1. No candidate for the archetype: [`NO_HANDLER`].
/// 2. Candidates, none performing the action: [`UNSUPPORTED_ACTION`]; none
///    accepting the convention: [`UNSUPPORTED_CONVENTION`].
/// 3. `handler: "choose"` (or a named handler, see [`HandlerChoice::Named`]):
///    the chooser. An explicit choice never falls back to the hint.
/// 4. The user's default for the archetype, when it can take the request.
///    A default that cannot is not applicable and is passed over.
/// 5. The hinted napplet, when it is installed and can take the request.
/// 6. The only candidate that can take it.
/// 7. Otherwise, the chooser.
///
/// A hinted napplet that is not installed is the host's to offer — see
/// [`hint_to_offer`].
pub fn resolve(catalog: &IntentCatalogSnapshot, request: &IntentRequest) -> Resolution {
    let all = candidates(catalog, &request.archetype);
    if all.is_empty() {
        return Resolution::Refused(NO_HANDLER);
    }
    let able = able_candidates(&all, request);
    if able.is_empty() {
        let any_action = all.iter().any(|c| c.actions.contains(&request.action));
        return Resolution::Refused(if any_action {
            UNSUPPORTED_CONVENTION
        } else {
            UNSUPPORTED_ACTION
        });
    }
    match request.handler {
        HandlerChoice::Choose | HandlerChoice::Named(_) => return Resolution::Choose(able),
        HandlerChoice::Default => {}
    }
    let pick = |found: Option<&(Candidate, String)>| {
        found.map(|(candidate, convention)| Resolution::Handler {
            candidate: candidate.clone(),
            convention: convention.clone(),
        })
    };
    if let Some(found) = pick(able.iter().find(|(c, _)| c.is_default)) {
        return found;
    }
    if let Some(hint) = &request.hint {
        if let Some(found) = pick(
            able.iter()
                .find(|(c, _)| c.address.as_deref() == Some(hint.address.as_str())),
        ) {
            return found;
        }
    }
    if able.len() == 1 {
        let (candidate, convention) = able.into_iter().next().expect("one");
        return Resolution::Handler {
            candidate,
            convention,
        };
    }
    Resolution::Choose(able)
}

/// Every candidate that can take `request`, each with the convention it
/// would be delivered on.
fn able_candidates(all: &[Candidate], request: &IntentRequest) -> Vec<(Candidate, String)> {
    let wanted = request.convention.as_deref();
    all.iter()
        .filter_map(|c| {
            c.convention_for(&request.archetype, &request.action, wanted)
                .map(|conv| (c.clone(), conv))
        })
        .collect()
}

/// The hint the host may offer to install for `request`: present, naming a
/// napplet not in the catalog, on a request that neither chose explicitly nor
/// has an applicable default. Whether the hinted napplet can take the request
/// is for its verified manifest to say ([`contract_accepts`]); the hint
/// itself is never evidence.
pub fn hint_to_offer<'a>(
    catalog: &IntentCatalogSnapshot,
    request: &'a IntentRequest,
) -> Option<&'a HandlerHint> {
    let hint = request.hint.as_ref()?;
    if request.handler != HandlerChoice::Default {
        return None;
    }
    if catalog
        .handlers
        .iter()
        .any(|h| h.address.as_deref() == Some(hint.address.as_str()))
    {
        return None;
    }
    let all = candidates(catalog, &request.archetype);
    let default_applies = able_candidates(&all, request)
        .iter()
        .any(|(c, _)| c.is_default);
    (!default_applies).then_some(hint)
}

/// The convention a napplet advertising `archetypes` would be delivered
/// `request` on, or `None` when its contracts cannot take it — the same rule
/// an installed candidate is held to.
pub fn contract_accepts(archetypes: &[Archetype], request: &IntentRequest) -> Option<String> {
    let probe = IntentHandler {
        key: String::new(),
        d_tag: String::new(),
        address: None,
        title: None,
        pointer: String::new(),
        archetypes: archetypes.to_vec(),
    };
    let catalog = IntentCatalogSnapshot {
        handlers: vec![probe],
        ..IntentCatalogSnapshot::default()
    };
    let all = candidates(&catalog, &request.archetype);
    able_candidates(&all, request)
        .into_iter()
        .next()
        .map(|(_, convention)| convention)
}

/// The `intent.deliver` push a handler receives: the spec's `IntentDelivery`.
/// `sender` is the calling napplet's catalog identifier, which the host
/// derived from its session — never anything the caller sent.
pub fn deliver_frame(
    sender: &str,
    archetype: &str,
    action: &str,
    convention: &str,
    payload: Option<&Value>,
) -> Envelope {
    let mut delivery = json!({
        "sender": sender,
        "archetype": archetype,
        "action": action,
        "convention": convention,
    });
    if let Some(payload) = payload {
        delivery["payload"] = payload.clone();
    }
    Envelope::new("intent.deliver").with_field("delivery", delivery)
}

/// A valid `intent.launch`: open the one napplet `target` names, or offer it
/// for install. No payload, no role, no default: the host opens exactly this
/// napplet or nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchRequest {
    /// The coordinate exactly as sent: `35129:<hex>:<d>` or `15129:<hex>:`.
    pub address: String,
    pub kind: u16,
    /// Lowercase hex.
    pub author: String,
    /// `None` for a root napplet.
    pub d_tag: Option<String>,
    /// Discovery hints. Untrusted; at most [`MAX_HINT_RELAYS`].
    pub relays: Vec<String>,
    pub behavior: IntentBehavior,
    call: Envelope,
}

impl LaunchRequest {
    /// `ok`: the host opened the napplet, or put its install review up.
    pub fn accepted(&self, handler: &str) -> Envelope {
        self.call
            .to_result()
            .with_field("result", json!({"ok": true, "handler": handler}))
    }

    /// Refused before anything happened.
    pub fn failed(&self, error: &str) -> Envelope {
        launch_failed(&self.call, error)
    }
}

fn launch_failed(call: &Envelope, error: &str) -> Envelope {
    call.to_result()
        .with_field("result", json!({"ok": false, "error": error}))
}

/// Validate an `intent.launch`: a kind 35129 coordinate with a non-empty
/// `d`, or a kind 15129 one with an empty `d`; a 64-hex-digit lowercase
/// pubkey; relays, if any, a list of strings.
fn parse_launch(message: &Envelope) -> Result<LaunchRequest, Envelope> {
    let refuse = || launch_failed(message, INVALID_TARGET);
    let target = message.field("target").and_then(Value::as_object);
    let address = target
        .and_then(|t| t.get("address"))
        .and_then(Value::as_str)
        .ok_or_else(refuse)?;
    let mut parts = address.splitn(3, ':');
    let (Some(kind), Some(author), Some(d_tag)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(refuse());
    };
    let kind: u16 = kind.parse().map_err(|_| refuse())?;
    if author.len() != 64
        || !author
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(refuse());
    }
    let d_tag = match (kind, d_tag.is_empty()) {
        (HINT_KIND, false) => Some(d_tag.to_string()),
        (ROOT_KIND, true) => None,
        _ => return Err(refuse()),
    };
    let relays = match target.and_then(|t| t.get("relays")) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(list)) => {
            let strings: Option<Vec<String>> = list
                .iter()
                .map(|r| r.as_str().map(str::to_string))
                .collect();
            strings
                .ok_or_else(refuse)?
                .into_iter()
                .filter(|r| r.len() <= 512)
                .take(MAX_HINT_RELAYS)
                .collect()
        }
        Some(_) => return Err(refuse()),
    };
    let behavior = message
        .field("behavior")
        .and_then(Value::as_object)
        .map(|b| IntentBehavior {
            focus: b.get("focus").and_then(Value::as_bool),
            new_window: b.get("newWindow").and_then(Value::as_bool),
            reuse: b.get("reuse").and_then(Value::as_bool),
        })
        .unwrap_or_default();
    Ok(LaunchRequest {
        address: address.to_string(),
        kind,
        author: author.to_string(),
        d_tag,
        relays,
        behavior,
        call: message.clone(),
    })
}

/// Handle an inbound `intent.*` message.
pub async fn handle(ctx: &NapContext, message: &Envelope) -> Outcome {
    match message.action() {
        "invoke" => match parse_invoke(message) {
            Ok(request) => Outcome::Intent(request),
            Err(reply) => Outcome::Reply(vec![reply]),
        },
        "launch" => match parse_launch(message) {
            Ok(request) => Outcome::Launch(request),
            Err(reply) => Outcome::Reply(vec![reply]),
        },
        "available" => {
            let Some(archetype) = message
                .field("archetype")
                .and_then(Value::as_str)
                .filter(|a| is_slug(a))
            else {
                return Outcome::Reply(vec![message.to_error("archetype is not a slug")]);
            };
            let catalog = ctx.intents.snapshot().await;
            Outcome::Reply(vec![message
                .to_result()
                .with_field("availability", availability(&catalog, archetype))])
        }
        "handlers" => {
            let catalog = ctx.intents.snapshot().await;
            Outcome::Reply(vec![message
                .to_result()
                .with_field("handlers", Value::Array(all_availability(&catalog)))])
        }
        // `intent.changed` is runtime -> napplet only; anything else is unknown.
        _ => Outcome::Reply(Vec::new()),
    }
}

/// Validate an `intent.invoke`. The refusal is a structured failed result, as
/// the spec asks — the shim rejects a result it cannot read as one.
fn parse_invoke(message: &Envelope) -> Result<IntentRequest, Envelope> {
    let request = message.field("request").and_then(Value::as_object);
    let archetype_raw = request
        .and_then(|r| r.get("archetype"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let action_raw = request
        .and_then(|r| r.get("action"))
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_ACTION);
    let refuse = |why: &str| failed_result(message, archetype_raw, action_raw, why);
    let Some(request) = request else {
        return Err(refuse(INVALID_REQUEST));
    };
    if !is_slug(archetype_raw) {
        return Err(refuse(INVALID_REQUEST));
    }
    let action = match request.get("action") {
        None | Some(Value::Null) => DEFAULT_ACTION.to_string(),
        Some(Value::String(a)) if is_slug(a) => a.clone(),
        Some(_) => return Err(refuse(INVALID_REQUEST)),
    };
    // The convention is the stable, queryless identity. It must name the
    // request's own archetype and action: a binding that derived them from
    // the URI cannot disagree with it, so a request that does is malformed.
    let convention = match request.get("convention") {
        None | Some(Value::Null) => None,
        Some(Value::String(c)) if is_convention(c) => {
            if convention_archetype(c) != Some(archetype_raw)
                || convention_intent(c) != Some(action.as_str())
            {
                return Err(refuse(INVALID_CONVENTION));
            }
            Some(c.clone())
        }
        Some(_) => return Err(refuse(INVALID_CONVENTION)),
    };
    let handler = match request.get("handler") {
        None | Some(Value::Null) => HandlerChoice::Default,
        Some(Value::String(h)) if h == "default" => HandlerChoice::Default,
        Some(Value::String(h)) if h == "choose" => HandlerChoice::Choose,
        Some(Value::String(h)) if !h.is_empty() && h.len() <= 256 => {
            HandlerChoice::Named(h.clone())
        }
        Some(_) => return Err(refuse(INVALID_REQUEST)),
    };
    let payload = match request.get("payload") {
        None => None,
        Some(p) => {
            let size = serde_json::to_vec(p).map(|b| b.len()).unwrap_or(usize::MAX);
            if size > MAX_PAYLOAD_BYTES {
                return Err(refuse(PAYLOAD_TOO_LARGE));
            }
            Some(p.clone())
        }
    };
    let behavior = request
        .get("behavior")
        .and_then(Value::as_object)
        .map(|b| IntentBehavior {
            focus: b.get("focus").and_then(Value::as_bool),
            new_window: b.get("newWindow").and_then(Value::as_bool),
            reuse: b.get("reuse").and_then(Value::as_bool),
        })
        .unwrap_or_default();
    let hint = match request.get("handlerHint") {
        None | Some(Value::Null) => None,
        Some(h) => match HandlerHint::parse(h) {
            Some(hint) => Some(hint),
            None => return Err(refuse(INVALID_REQUEST)),
        },
    };
    Ok(IntentRequest {
        archetype: archetype_raw.to_string(),
        action,
        convention,
        payload,
        handler,
        behavior,
        hint,
        call: message.clone(),
        from_link: false,
        answered: false,
    })
}

/// The push a session receives when an archetype's availability changes —
/// `None` when it should hear nothing: not yet handshaken, or not granted
/// `intent`.
pub fn changed_frame(session: &Session, availability: &Value) -> Option<Envelope> {
    session
        .may_service("intent")
        .then(|| Envelope::new("intent.changed").with_field("availability", availability.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{dispatch, needs_session};
    use crate::session::NappletIdentity;
    use crate::testing::test_context;
    use std::sync::Arc;

    fn arch(slug: &str, convention: &str) -> Archetype {
        Archetype {
            slug: slug.into(),
            convention: convention.into(),
            params: Vec::new(),
        }
    }

    /// The coordinate the test handler `key` is published at.
    fn address_of(key: &str) -> String {
        let d_tag = key.rsplit(':').next().unwrap();
        format!("35129:{}:{d_tag}", "ab".repeat(32))
    }

    fn handler(key: &str, archetypes: Vec<Archetype>) -> IntentHandler {
        let d_tag = key.rsplit(':').next().unwrap();
        IntentHandler {
            key: key.into(),
            d_tag: d_tag.into(),
            address: Some(address_of(key)),
            title: Some(format!("Title {d_tag}")),
            pointer: key.into(),
            archetypes,
        }
    }

    fn catalog(handlers: Vec<IntentHandler>, defaults: &[(&str, &str)]) -> IntentCatalogSnapshot {
        IntentCatalogSnapshot {
            handlers,
            installed: BTreeMap::new(),
            defaults: defaults
                .iter()
                .map(|(a, k)| (a.to_string(), k.to_string()))
                .collect(),
        }
    }

    fn invoke(request: Value) -> Envelope {
        Envelope::new("intent.invoke")
            .with_id("i1")
            .with_field("request", request)
    }

    fn request(value: Value) -> IntentRequest {
        match parse_invoke(&invoke(value)) {
            Ok(r) => r,
            Err(e) => panic!("refused: {e:?}"),
        }
    }

    fn session(granted: &[&str]) -> Session {
        let mut s = Session::new(NappletIdentity::new("feed", "aggregate"), granted.to_vec());
        s.on_ready();
        s
    }

    fn profiles() -> IntentHandler {
        handler(
            "npub1a:profiles",
            vec![
                arch("profile", "napplet:profile/open"),
                arch("profile", "napplet:profile/edit"),
            ],
        )
    }

    fn cards() -> IntentHandler {
        handler(
            "npub1b:cards",
            vec![arch("profile", "napplet:profile/open")],
        )
    }

    #[test]
    fn actions_are_derived_from_conventions() {
        let mut profiles = profiles();
        profiles.archetypes[0].params = vec!["pubkey".into()];
        let c = &candidates(&catalog(vec![profiles], &[]), "profile")[0];
        assert_eq!(c.actions, vec!["open", "edit"]);
        assert_eq!(
            c.conventions,
            vec!["napplet:profile/open", "napplet:profile/edit"]
        );
        // The spec's `id` and `contracts`, and the first draft's `dTag`.
        assert_eq!(
            c.to_json(),
            json!({"id": "npub1a:profiles", "dTag": "profiles", "title": "Title profiles",
                   "actions": ["open", "edit"],
                   "conventions": ["napplet:profile/open", "napplet:profile/edit"],
                   "contracts": [
                       {"convention": "napplet:profile/open", "params": ["pubkey"]},
                       {"convention": "napplet:profile/edit", "params": []}],
                   "isDefault": false})
        );
    }

    /// Two publishers' napplets with the same `d` tag are two candidates,
    /// told apart by `id`.
    #[test]
    fn identifiers_tell_publishers_apart() {
        let theirs = handler(
            "npub1z:profiles",
            vec![arch("profile", "napplet:profile/open")],
        );
        let cat = catalog(vec![profiles(), theirs], &[]);
        let ids: Vec<String> = candidates(&cat, "profile")
            .into_iter()
            .map(|c| c.key)
            .collect();
        assert_eq!(ids, vec!["npub1a:profiles", "npub1z:profiles"]);
    }

    fn hint(key: &str) -> Value {
        json!({"address": address_of(key), "relays": ["wss://relay.example"]})
    }

    #[test]
    fn an_installed_hint_wins_over_the_chooser_but_not_the_default() {
        let cat = catalog(vec![profiles(), cards()], &[]);
        let hinted = request(json!({"archetype": "profile", "handlerHint": hint("npub1b:cards")}));
        assert!(matches!(
            resolve(&cat, &hinted),
            Resolution::Handler { candidate, .. } if candidate.key == "npub1b:cards"
        ));
        assert_eq!(
            hint_to_offer(&cat, &hinted),
            None,
            "installed: nothing to offer"
        );

        let with_default = catalog(vec![profiles(), cards()], &[("profile", "npub1a:profiles")]);
        assert!(matches!(
            resolve(&with_default, &hinted),
            Resolution::Handler { candidate, .. } if candidate.key == "npub1a:profiles"
        ));

        // An explicit choice never falls back to the hint.
        let choose = request(json!({"archetype": "profile", "handler": "choose",
                                    "handlerHint": hint("npub1b:cards")}));
        assert!(matches!(resolve(&cat, &choose), Resolution::Choose(_)));
    }

    #[test]
    fn a_hint_that_cannot_take_it_is_passed_over() {
        let cat = catalog(vec![profiles(), cards()], &[]);
        // cards accepts only `open`; the hint does not make it accept `edit`.
        let edit = request(json!({"archetype": "profile", "action": "edit",
                                  "handlerHint": hint("npub1b:cards")}));
        assert!(matches!(
            resolve(&cat, &edit),
            Resolution::Handler { candidate, .. } if candidate.key == "npub1a:profiles"
        ));
    }

    #[test]
    fn an_uninstalled_hint_is_offered_unless_a_default_applies() {
        let cat = catalog(vec![profiles()], &[]);
        let hinted = request(json!({"archetype": "profile", "handlerHint": hint("npub1n:newapp")}));
        assert_eq!(
            hint_to_offer(&cat, &hinted).map(|h| h.d_tag.as_str()),
            Some("newapp")
        );
        // With nothing installed for the role at all, too.
        let empty = catalog(vec![], &[]);
        assert!(hint_to_offer(&empty, &hinted).is_some());

        let with_default = catalog(vec![profiles()], &[("profile", "npub1a:profiles")]);
        assert_eq!(hint_to_offer(&with_default, &hinted), None);
        let choose = request(json!({"archetype": "profile", "handler": "choose",
                                    "handlerHint": hint("npub1n:newapp")}));
        assert_eq!(hint_to_offer(&cat, &choose), None);
    }

    #[test]
    fn a_manifest_contract_decides_whether_a_hint_can_take_it() {
        let open = request(json!({"archetype": "profile"}));
        let edit = request(json!({"archetype": "profile", "action": "edit"}));
        let contracts = vec![arch("profile", "napplet:profile/open")];
        assert_eq!(
            contract_accepts(&contracts, &open).as_deref(),
            Some("napplet:profile/open")
        );
        assert_eq!(contract_accepts(&contracts, &edit), None);
        assert_eq!(contract_accepts(&[], &open), None);
    }

    #[test]
    fn hints_are_validated() {
        let ok = request(json!({"archetype": "profile", "handlerHint": hint("npub1a:profiles")}));
        let h = ok.hint.unwrap();
        assert_eq!(h.author, "ab".repeat(32));
        assert_eq!(h.d_tag, "profiles");
        assert_eq!(h.relays, vec!["wss://relay.example"]);
        // `d` is opaque: case and colons kept.
        let d = request(json!({"archetype": "profile",
            "handlerHint": {"address": format!("35129:{}:My:App", "ab".repeat(32))}}));
        assert_eq!(d.hint.unwrap().d_tag, "My:App");

        for bad in [
            json!({"address": format!("30023:{}:x", "ab".repeat(32))}),
            json!({"address": format!("35129:{}:x", "AB".repeat(32))}),
            json!({"address": format!("35129:{}:", "ab".repeat(32))}),
            json!({"address": "35129:abc:x"}),
            json!({"address": format!("35129:{}:x", "ab".repeat(32)), "relays": [1]}),
            json!("35129:x:y"),
        ] {
            let e = parse_invoke(&invoke(json!({"archetype": "profile", "handlerHint": bad})))
                .unwrap_err();
            assert_eq!(
                serde_json::to_value(&e).unwrap()["result"]["error"],
                INVALID_REQUEST
            );
        }
    }

    #[test]
    fn availability_reports_candidates_and_the_default() {
        let cat = catalog(vec![profiles(), cards()], &[("profile", "npub1b:cards")]);
        let a = availability(&cat, "profile");
        assert_eq!(a["available"], true);
        assert_eq!(a["hasDefault"], true);
        assert_eq!(a["candidates"].as_array().unwrap().len(), 2);
        assert_eq!(a["candidates"][1]["isDefault"], true);
        let none = availability(&cat, "pet");
        assert_eq!(
            none,
            json!({"archetype": "pet", "available": false, "candidates": [], "hasDefault": false})
        );
        assert_eq!(archetypes(&cat), vec!["profile"]);
    }

    #[test]
    fn the_sole_candidate_is_resolved_directly() {
        let cat = catalog(vec![profiles()], &[]);
        let r = resolve(&cat, &request(json!({"archetype": "profile"})));
        let Resolution::Handler {
            candidate,
            convention,
        } = r
        else {
            panic!("expected a handler, got {r:?}");
        };
        assert_eq!(candidate.key, "npub1a:profiles");
        assert_eq!(
            convention, "napplet:profile/open",
            "the archetype's own convention"
        );
    }

    #[test]
    fn the_default_wins_over_several() {
        let cat = catalog(vec![profiles(), cards()], &[("profile", "npub1b:cards")]);
        let r = resolve(&cat, &request(json!({"archetype": "profile"})));
        assert!(
            matches!(&r, Resolution::Handler { candidate, .. } if candidate.key == "npub1b:cards"),
            "{r:?}"
        );
    }

    #[test]
    fn several_without_a_default_ask_the_user() {
        let cat = catalog(vec![profiles(), cards()], &[]);
        let r = resolve(&cat, &request(json!({"archetype": "profile"})));
        let Resolution::Choose(list) = r else {
            panic!("expected the chooser");
        };
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn choose_and_a_named_handler_always_ask() {
        let cat = catalog(vec![profiles()], &[("profile", "npub1a:profiles")]);
        for handler in ["choose", "profiles", "someone-else"] {
            let r = resolve(
                &cat,
                &request(json!({"archetype": "profile", "handler": handler})),
            );
            assert!(
                matches!(r, Resolution::Choose(ref l) if l.len() == 1),
                "{handler}: {r:?}"
            );
        }
        let r = resolve(
            &cat,
            &request(json!({"archetype": "profile", "handler": "default"})),
        );
        assert!(matches!(r, Resolution::Handler { .. }));
    }

    #[test]
    fn nothing_installed_is_no_handler() {
        let cat = catalog(vec![profiles()], &[]);
        assert_eq!(
            resolve(&cat, &request(json!({"archetype": "emoji-list"}))),
            Resolution::Refused(NO_HANDLER)
        );
    }

    #[test]
    fn unsupported_action_and_convention() {
        let cat = catalog(vec![profiles(), cards()], &[]);
        assert_eq!(
            resolve(
                &cat,
                &request(json!({"archetype": "profile", "action": "share"}))
            ),
            Resolution::Refused(UNSUPPORTED_ACTION)
        );
        // A convention naming another role never gets this far: it is an
        // invalid request (see `requests_are_validated`).
        // An action only one candidate has narrows to it.
        let r = resolve(
            &cat,
            &request(json!({"archetype": "profile", "action": "edit"})),
        );
        assert!(
            matches!(&r, Resolution::Handler { candidate, convention }
                if candidate.key == "npub1a:profiles" && convention == "napplet:profile/edit"),
            "{r:?}"
        );
    }

    #[test]
    fn a_default_that_cannot_take_it_is_passed_over() {
        // Only an applicable default wins; cards cannot edit, profiles can.
        let cat = catalog(vec![profiles(), cards()], &[("profile", "npub1b:cards")]);
        assert!(matches!(
            resolve(
                &cat,
                &request(json!({"archetype": "profile", "action": "edit"}))
            ),
            Resolution::Handler { candidate, .. } if candidate.key == "npub1a:profiles"
        ));
        // A default left behind by an uninstalled napplet is no default.
        let cat = catalog(vec![profiles()], &[("profile", "npub1gone:x")]);
        assert!(matches!(
            resolve(&cat, &request(json!({"archetype": "profile"}))),
            Resolution::Handler { .. }
        ));
    }

    #[test]
    fn requests_are_validated() {
        let refused = |value: Value| {
            let e = parse_invoke(&invoke(value)).unwrap_err();
            serde_json::to_value(&e).unwrap()
        };
        let r = refused(json!({"archetype": "Not A Slug"}));
        assert_eq!(r["type"], "intent.invoke.result");
        assert_eq!(r["id"], "i1");
        assert_eq!(r["result"]["ok"], false);
        assert_eq!(r["result"]["handled"], false);
        assert_eq!(r["result"]["action"], "open");
        assert_eq!(r["result"]["error"], INVALID_REQUEST);

        assert_eq!(
            refused(json!({"archetype": "note", "convention": "napplet:note/open?id=x"}))["result"]
                ["error"],
            INVALID_CONVENTION,
            "a convention carries no query"
        );
        assert_eq!(
            refused(json!({"archetype": "note", "convention": "napplet:note/open#naddr1x"}))
                ["result"]["error"],
            INVALID_CONVENTION,
            "nor a fragment"
        );
        assert_eq!(
            refused(json!({"archetype": "note", "convention": "napplet:profile/open"}))["result"]
                ["error"],
            INVALID_CONVENTION,
            "the convention names the request's archetype"
        );
        assert_eq!(
            refused(json!({"archetype": "note", "action": "edit",
                           "convention": "napplet:note/open"}))["result"]["error"],
            INVALID_CONVENTION,
            "and its action"
        );
        assert_eq!(
            refused(json!({"archetype": "note", "action": 3}))["result"]["error"],
            INVALID_REQUEST
        );
        let big = "x".repeat(MAX_PAYLOAD_BYTES);
        assert_eq!(
            refused(json!({"archetype": "note", "payload": {"blob": big}}))["result"]["error"],
            PAYLOAD_TOO_LARGE
        );
        let bare = Envelope::new("intent.invoke").with_id("i1");
        let r = serde_json::to_value(parse_invoke(&bare).unwrap_err()).unwrap();
        assert_eq!(r["result"]["archetype"], "");

        let ok = request(json!({"archetype": "note", "payload": {"id": "abc"},
                                "behavior": {"focus": true}}));
        assert_eq!(ok.action, "open");
        assert_eq!(ok.handler, HandlerChoice::Default);
        assert_eq!(ok.behavior.focus, Some(true));
        assert_eq!(ok.payload, Some(json!({"id": "abc"})));
    }

    #[test]
    fn answers_have_the_spec_shape() {
        let r = request(json!({"archetype": "note"}));
        assert_eq!(
            serde_json::to_value(r.handled("noteview", "napplet-3", "napplet:note/open")).unwrap(),
            json!({"type": "intent.invoke.result", "id": "i1",
                   "result": {"ok": true, "archetype": "note", "action": "open", "handled": true,
                              "handler": "noteview", "windowId": "napplet-3",
                              "convention": "napplet:note/open"}})
        );
        assert_eq!(
            serde_json::to_value(r.failed(NO_HANDLER)).unwrap(),
            json!({"type": "intent.invoke.result", "id": "i1",
                   "result": {"ok": false, "archetype": "note", "action": "open",
                              "handled": false, "error": "no handler"}})
        );
    }

    /// Accepted before the handler is open: `ok`, the handler that will get
    /// it, and nothing more for this caller afterwards.
    #[test]
    fn an_early_acceptance_answers_once() {
        let mut r = request(json!({"archetype": "note"}));
        assert!(r.answers_caller());
        assert_eq!(
            serde_json::to_value(r.accept("npub1n:notes", "napplet:note/open")).unwrap(),
            json!({"type": "intent.invoke.result", "id": "i1",
                   "result": {"ok": true, "archetype": "note", "action": "open", "handled": true,
                              "handler": "npub1n:notes", "convention": "napplet:note/open"}})
        );
        assert!(!r.answers_caller());
        assert_eq!(r.requested_convention(), "napplet:note/open");
    }

    #[test]
    fn a_delivery_has_the_spec_shape() {
        let frame = deliver_frame(
            "npub1a:feed",
            "profile",
            "open",
            "napplet:profile/open",
            Some(&json!({"pubkey": "abc"})),
        );
        assert_eq!(
            serde_json::to_value(frame).unwrap(),
            json!({"type": "intent.deliver",
                   "delivery": {"sender": "npub1a:feed", "archetype": "profile", "action": "open",
                                "convention": "napplet:profile/open",
                                "payload": {"pubkey": "abc"}}})
        );
        let bare = deliver_frame("myco", "nsite", "open", "napplet:nsite/open", None);
        assert!(serde_json::to_value(bare).unwrap()["delivery"]
            .get("payload")
            .is_none());
    }

    #[tokio::test]
    async fn invoke_is_handed_to_the_host() {
        let (ctx, _signer) = test_context();
        let mut s = session(&["intent"]);
        let out = dispatch(&ctx, &mut s, &invoke(json!({"archetype": "note"}))).await;
        let Outcome::Intent(request) = out else {
            panic!("expected an intent, got {out:?}");
        };
        assert_eq!(request.archetype, "note");
        assert!(!needs_session(&invoke(json!({}))));
    }

    #[tokio::test]
    async fn available_and_handlers_read_the_catalog() {
        let (mut ctx, _signer) = test_context();
        ctx.intents = Arc::new(StaticIntents(catalog(vec![profiles()], &[])));
        let mut s = session(&["intent"]);
        let out = dispatch(
            &ctx,
            &mut s,
            &Envelope::new("intent.available")
                .with_id("a1")
                .with_field("archetype", "profile"),
        )
        .await;
        let r = serde_json::to_value(&out.envelopes()[0]).unwrap();
        assert_eq!(r["type"], "intent.available.result");
        assert_eq!(r["availability"]["available"], true);
        assert_eq!(r["availability"]["candidates"][0]["dTag"], "profiles");
        assert_eq!(
            r["availability"]["candidates"][0]["id"], "npub1a:profiles",
            "the catalog identifier, which names the publisher"
        );

        let out = dispatch(
            &ctx,
            &mut s,
            &Envelope::new("intent.handlers").with_id("h1"),
        )
        .await;
        let r = serde_json::to_value(&out.envelopes()[0]).unwrap();
        assert_eq!(r["handlers"].as_array().unwrap().len(), 1);
        assert_eq!(r["handlers"][0]["archetype"], "profile");

        let out = dispatch(
            &ctx,
            &mut s,
            &Envelope::new("intent.available")
                .with_id("a2")
                .with_field("archetype", "NOPE"),
        )
        .await;
        assert!(out.envelopes()[0].field("error").is_some());
    }

    #[tokio::test]
    async fn an_ungranted_intent_is_refused() {
        let (ctx, _signer) = test_context();
        let mut s = session(&["relay"]);
        let out = dispatch(&ctx, &mut s, &invoke(json!({"archetype": "note"}))).await;
        assert!(out.envelopes()[0]
            .field("error")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("intent"));
    }

    #[test]
    fn changed_is_pushed_only_to_granted_sessions() {
        let a = json!({"archetype": "note"});
        let push = changed_frame(&session(&["intent"]), &a).unwrap();
        assert_eq!(
            serde_json::to_value(&push).unwrap(),
            json!({"type": "intent.changed", "availability": {"archetype": "note"}})
        );
        assert!(changed_frame(&session(&[]), &a).is_none());
        let fresh = Session::new(NappletIdentity::new("x", "y"), ["intent"]);
        assert!(changed_frame(&fresh, &a).is_none());
    }

    /// A link is the same request an `intent.invoke` would be, resolved by
    /// the same rules — with its exact convention, so a handler that does not
    /// accept it is not picked.
    #[test]
    fn a_link_resolves_like_the_invoke_it_stands_for() {
        let link = NappletLink::parse("https://napplet.link/profile/open?pubkey=abc").unwrap();
        let from_link = IntentRequest::from_link(&link);
        assert!(!from_link.answers_caller());
        assert!(request(json!({"archetype": "profile"})).answers_caller());
        assert_eq!(from_link.archetype, "profile");
        assert_eq!(from_link.action, "open");
        assert_eq!(
            from_link.convention.as_deref(),
            Some("napplet:profile/open")
        );
        assert_eq!(from_link.payload, Some(json!({"pubkey": "abc"})));
        assert_eq!(from_link.handler, HandlerChoice::Default);

        let c = catalog(
            vec![
                handler("npub1a:p", vec![arch("profile", "napplet:profile/open")]),
                handler("npub1b:q", vec![arch("profile", "napplet:profile/view")]),
            ],
            &[],
        );
        let Resolution::Handler { candidate, .. } = resolve(&c, &from_link) else {
            panic!("expected the one handler that accepts the convention");
        };
        assert_eq!(candidate.key, "npub1a:p");

        let none = NappletLink::parse("napplet:pet/open").unwrap();
        assert_eq!(
            resolve(&c, &IntentRequest::from_link(&none)),
            Resolution::Refused(NO_HANDLER)
        );
    }

    #[test]
    fn launches_are_validated() {
        let launch = |target: Value| {
            parse_launch(
                &Envelope::new("intent.launch")
                    .with_id("l1")
                    .with_field("target", target),
            )
        };
        let hex = "ab".repeat(32);
        let named = launch(json!({"address": format!("35129:{hex}:My:App"),
                                  "relays": ["wss://r.example"]}))
        .unwrap();
        assert_eq!(named.kind, HINT_KIND);
        assert_eq!(named.d_tag.as_deref(), Some("My:App"));
        assert_eq!(named.relays, vec!["wss://r.example"]);
        let root = launch(json!({"address": format!("15129:{hex}:")})).unwrap();
        assert_eq!(root.d_tag, None);

        for bad in [
            json!({"address": format!("35129:{hex}:")}),
            json!({"address": format!("15129:{hex}:x")}),
            json!({"address": format!("5129:{hex}:x")}),
            json!({"address": format!("35129:{}:x", "AB".repeat(32))}),
            json!({"address": "35129:abc:x"}),
            json!({"address": format!("35129:{hex}:x"), "relays": "wss://r"}),
            json!("35129:x:y"),
        ] {
            let e = serde_json::to_value(launch(bad.clone()).unwrap_err()).unwrap();
            assert_eq!(
                e,
                json!({"type": "intent.launch.result", "id": "l1",
                       "result": {"ok": false, "error": INVALID_TARGET}}),
                "{bad}"
            );
        }

        assert_eq!(
            serde_json::to_value(named.accepted("npub1x:app")).unwrap(),
            json!({"type": "intent.launch.result", "id": "l1",
                   "result": {"ok": true, "handler": "npub1x:app"}})
        );
    }

    #[tokio::test]
    async fn launch_is_handed_to_the_host() {
        let (ctx, _signer) = test_context();
        let mut s = session(&["intent"]);
        let out = dispatch(
            &ctx,
            &mut s,
            &Envelope::new("intent.launch").with_id("l1").with_field(
                "target",
                json!({"address": format!("35129:{}:x", "ab".repeat(32))}),
            ),
        )
        .await;
        assert!(matches!(out, Outcome::Launch(_)), "{out:?}");
    }
}
