//! NAP-INTENT — a napplet asks the shell to open *another* napplet by role.
//!
//! ```text
//! -> { "type": "intent.available", "id": "a1", "archetype": "profile" }
//! <- { "type": "intent.available.result", "id": "a1",
//!      "availability": { "archetype": "profile", "available": true, "hasDefault": false,
//!        "candidates": [ { "dTag": "profiles", "title": "Profiles", "actions": ["open"],
//!                          "conventions": ["napplet:profile/open"], "isDefault": false } ] } }
//! -> { "type": "intent.invoke", "id": "i1",
//!      "request": { "archetype": "profile", "convention": "napplet:profile/open",
//!                   "payload": { "pubkey": "…" } } }
//! <- { "type": "intent.invoke.result", "id": "i1",
//!      "result": { "ok": true, "archetype": "profile", "action": "open", "handled": true,
//!                  "handler": "profiles", "windowId": "napplet-7",
//!                  "convention": "napplet:profile/open" } }
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
//! The payload is opaque and never read here. It is only measured: a payload
//! over [`MAX_PAYLOAD_BYTES`] of JSON is refused, because it is held by the
//! host until the handler is ready for it.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::dispatch::{NapContext, Outcome};
use crate::manifest::{convention_intent, is_convention, is_slug, Archetype};
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

/// One installed napplet (or shell built-in) that can be opened by role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentHandler {
    /// The host's key for the handler — for a napplet, `<npub>:<d>`. Opaque
    /// here, never sent to a napplet: it names an author, which is more than
    /// the spec's `dTag` does.
    pub key: String,
    /// What a napplet is told the handler is called: its `d` tag.
    pub d_tag: String,
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
    pub key: String,
    pub d_tag: String,
    pub title: Option<String>,
    /// See [`IntentHandler::pointer`].
    pub pointer: String,
    /// Derived from the conventions: each one's `<intent>` part, deduplicated.
    pub actions: Vec<String>,
    pub conventions: Vec<String>,
    pub is_default: bool,
}

impl Candidate {
    /// The wire `IntentCandidate`. The key stays with the host.
    pub fn to_json(&self) -> Value {
        let mut out = json!({
            "dTag": self.d_tag,
            "actions": self.actions,
            "conventions": self.conventions,
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
            let conventions: Vec<String> = h
                .archetypes
                .iter()
                .filter(|a| a.slug == archetype)
                .map(|a| a.convention.clone())
                .fold(Vec::new(), |mut acc, c| {
                    if !acc.contains(&c) {
                        acc.push(c);
                    }
                    acc
                });
            if conventions.is_empty() {
                return None;
            }
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
                title: h.title.clone(),
                pointer: h.pointer.clone(),
                actions,
                conventions,
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
    /// A specific napplet's dTag. The spec wants the user to have authorised
    /// cross-napplet targeting first; Myco has no such grant, so this is
    /// resolved as [`HandlerChoice::Choose`] — the user picks, and the name
    /// the caller gave decides nothing.
    Named(String),
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
    call: Envelope,
}

impl IntentRequest {
    /// The answer once a handler took it: its dTag, the window it went to and
    /// the convention the payload is delivered on.
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

/// Resolve `request` against `catalog`:
///
/// 1. No candidate for the archetype: [`NO_HANDLER`].
/// 2. Candidates, none performing the action: [`UNSUPPORTED_ACTION`]; none
///    accepting the convention: [`UNSUPPORTED_CONVENTION`].
/// 3. `handler: "choose"` (or a named dTag, see [`HandlerChoice::Named`]):
///    the chooser.
/// 4. The user's default for the archetype, when it can take the request.
///    A default that cannot is refused rather than stepped around — the spec
///    routes to the default when there is one.
/// 5. The only candidate that can take it.
/// 6. Otherwise, the chooser.
pub fn resolve(catalog: &IntentCatalogSnapshot, request: &IntentRequest) -> Resolution {
    let all = candidates(catalog, &request.archetype);
    if all.is_empty() {
        return Resolution::Refused(NO_HANDLER);
    }
    let wanted = request.convention.as_deref();
    let able: Vec<(Candidate, String)> = all
        .iter()
        .filter_map(|c| {
            c.convention_for(&request.archetype, &request.action, wanted)
                .map(|conv| (c.clone(), conv))
        })
        .collect();
    let why_not = |c: &Candidate| {
        if c.actions.contains(&request.action) {
            UNSUPPORTED_CONVENTION
        } else {
            UNSUPPORTED_ACTION
        }
    };
    if able.is_empty() {
        let any_action = all
            .iter()
            .any(|c| c.actions.contains(&request.action));
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
    if let Some(default) = all.iter().find(|c| c.is_default) {
        return match able.into_iter().find(|(c, _)| c.key == default.key) {
            Some((candidate, convention)) => Resolution::Handler {
                candidate,
                convention,
            },
            None => Resolution::Refused(why_not(default)),
        };
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

/// Handle an inbound `intent.*` message.
pub async fn handle(ctx: &NapContext, message: &Envelope) -> Outcome {
    match message.action() {
        "invoke" => match parse_invoke(message) {
            Ok(request) => Outcome::Intent(request),
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
    let convention = match request.get("convention") {
        None | Some(Value::Null) => None,
        Some(Value::String(c)) if is_convention(c) => Some(c.clone()),
        Some(_) => return Err(refuse(UNSUPPORTED_CONVENTION)),
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
    Ok(IntentRequest {
        archetype: archetype_raw.to_string(),
        action,
        convention,
        payload,
        handler,
        behavior,
        call: message.clone(),
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
        }
    }

    fn handler(key: &str, archetypes: Vec<Archetype>) -> IntentHandler {
        let d_tag = key.rsplit(':').next().unwrap();
        IntentHandler {
            key: key.into(),
            d_tag: d_tag.into(),
            title: Some(format!("Title {d_tag}")),
            pointer: key.into(),
            archetypes,
        }
    }

    fn catalog(handlers: Vec<IntentHandler>, defaults: &[(&str, &str)]) -> IntentCatalogSnapshot {
        IntentCatalogSnapshot {
            handlers,
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
        let c = &candidates(&catalog(vec![profiles()], &[]), "profile")[0];
        assert_eq!(c.actions, vec!["open", "edit"]);
        assert_eq!(
            c.conventions,
            vec!["napplet:profile/open", "napplet:profile/edit"]
        );
        assert_eq!(
            c.to_json(),
            json!({"dTag": "profiles", "title": "Title profiles",
                   "actions": ["open", "edit"],
                   "conventions": ["napplet:profile/open", "napplet:profile/edit"],
                   "isDefault": false})
        );
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
        assert_eq!(
            resolve(
                &cat,
                &request(json!({"archetype": "profile", "convention": "napplet:person/open"}))
            ),
            Resolution::Refused(UNSUPPORTED_CONVENTION)
        );
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
    fn a_default_that_cannot_take_it_is_refused() {
        let cat = catalog(vec![profiles(), cards()], &[("profile", "npub1b:cards")]);
        assert_eq!(
            resolve(
                &cat,
                &request(json!({"archetype": "profile", "action": "edit"}))
            ),
            Resolution::Refused(UNSUPPORTED_ACTION)
        );
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
            UNSUPPORTED_CONVENTION,
            "a convention carries no query"
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
        assert!(
            !r.to_string().contains("npub1a"),
            "the host key never reaches a napplet"
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
}
