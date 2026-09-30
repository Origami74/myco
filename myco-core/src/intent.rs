//! NAP-INTENT on this device: the catalog of installed napplets by role,
//! Myco's own built-in handler, and the host's half of `intent.invoke` —
//! resolving, asking the user, opening the handler's window, and handing the
//! payload over once the handler is listening.
//!
//! ## The catalog
//!
//! The Library is the catalog. Each installed napplet's entry keeps the roles
//! its served version declared ([`LibraryItem::archetypes`]), written from the
//! verified manifest at install and whenever the served version moves, and
//! filled lazily for entries written before NAP-INTENT
//! ([`backfill_archetypes`]). The user's default per role lives in settings
//! (`intentDefaults`). Myco itself is one more handler: the `nsite` role,
//! convention `napplet:nsite/open`, opened by the same path a `myco://app/…`
//! link takes ([`MYCO_HANDLER_KEY`]).
//!
//! ## Delivery
//!
//! The runtime sends `shell.ready` from its prelude before any napplet code
//! runs, so a session being established says nothing about whether the
//! napplet is listening yet — and the shim drops an `inc.event` for a topic
//! it has no handler for. So a payload is held per target, keyed by a random
//! token, and delivered when that session subscribes to the convention's
//! topic, or at once if it already has:
//!
//! ```text
//! caller  intent.invoke ──► resolve ──► open-napplet {pointer, token} ──► window host
//! window host  nappletOpen(pointer, token) / nappletBindIntent(session, token)
//!                 └─► bind: the caller hears intent.invoke.result (windowId = session)
//! handler inc.subscribe {topic} ──► inc.subscribe.result, then inc.event {topic, sender, payload}
//! ```
//!
//! Only the session the token was bound to — and only one of the napplet the
//! request resolved to — ever receives the payload, and it receives it once.
//! A token not bound within [`PENDING_INTENT_TTL`] fails the caller's
//! request; a bound payload whose handler never subscribes is dropped then,
//! and with its window.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde_json::Value;

use myco_napplet_runtime::manifest::{Archetype, NappletManifest};
use myco_napplet_runtime::nap::intent::{
    self as nap_intent, Candidate, IntentCatalog, IntentCatalogSnapshot, IntentHandler,
    IntentRequest, Resolution, INVOKE_FAILED, USER_CANCELLED,
};
use myco_napplet_runtime::shell_link::{ChooserCandidate, ToShell};

use crate::content::{Content, LibraryArchetype, LibraryItem, LibraryKind};
use crate::napplet::{ManifestStore, NappletAddr, NappletHost};

/// The key of Myco's own handler — the nsite opener.
pub const MYCO_HANDLER_KEY: &str = "myco";
/// The role Myco handles itself.
pub const NSITE_ARCHETYPE: &str = "nsite";
/// The convention Myco's nsite opener accepts.
pub const NSITE_OPEN: &str = "napplet:nsite/open";

/// How long a payload waits for its handler: to be bound to a window, and
/// then for the napplet in it to listen.
pub const PENDING_INTENT_TTL: Duration = Duration::from_secs(60);
/// How long an "open with…" question stays answerable.
pub const CHOOSER_TTL: Duration = Duration::from_secs(5 * 60);
/// How soon one window may invoke again after an invoke was admitted.
pub const INVOKE_COOLDOWN: Duration = Duration::from_millis(750);
/// The most payloads held at once, device-wide. Each is at most
/// [`nap_intent::MAX_PAYLOAD_BYTES`].
pub const MAX_PENDING_INTENTS: usize = 32;
/// `error` for an invoke refused by the host's rate limit or queue bound.
pub const INTENT_BUSY: &str = "busy: try again in a moment";

/// The user's default handler per archetype, shared by the reducer (which
/// writes it, from the user's answers) and the catalog (which reads it).
pub type IntentDefaults = Arc<RwLock<BTreeMap<String, String>>>;

/// The handler key for a napplet: `<npub>:<d>`, or `<npub>` for a root one —
/// the same shorthand `NappletAddr::parse` reads.
pub fn handler_key(npub: &str, d_tag: Option<&str>) -> String {
    match d_tag {
        Some(d) if !d.is_empty() => format!("{npub}:{d}"),
        _ => npub.to_string(),
    }
}

/// Myco's own handlers.
pub fn builtin_handlers() -> Vec<IntentHandler> {
    vec![IntentHandler {
        key: MYCO_HANDLER_KEY.to_string(),
        d_tag: MYCO_HANDLER_KEY.to_string(),
        title: Some("Myco".to_string()),
        pointer: String::new(),
        archetypes: vec![Archetype {
            slug: NSITE_ARCHETYPE.to_string(),
            convention: NSITE_OPEN.to_string(),
        }],
    }]
}

/// The catalog as the Library and the defaults stand: every installed
/// napplet that declared a role, then Myco's built-ins.
pub fn catalog_from(
    library: &[LibraryItem],
    defaults: &BTreeMap<String, String>,
) -> IntentCatalogSnapshot {
    let mut handlers: Vec<IntentHandler> = library
        .iter()
        .filter(|i| i.kind == LibraryKind::Napplet)
        .filter_map(|i| {
            let archetypes = i.archetypes.as_ref().filter(|a| !a.is_empty())?;
            let key = handler_key(&i.author_npub, i.d_tag.as_deref());
            let d_tag = i.d_tag.clone().unwrap_or_default();
            let title = Some(i.title.clone())
                .filter(|t| !t.is_empty())
                .or_else(|| Some(d_tag.clone()).filter(|d| !d.is_empty()));
            Some(IntentHandler {
                pointer: if i.pointer.is_empty() {
                    key.clone()
                } else {
                    i.pointer.clone()
                },
                key,
                d_tag,
                title,
                archetypes: archetypes
                    .iter()
                    .map(|a| Archetype {
                        slug: a.slug.clone(),
                        convention: a.convention.clone(),
                    })
                    .collect(),
            })
        })
        .collect();
    handlers.extend(builtin_handlers());
    IntentCatalogSnapshot {
        handlers,
        defaults: defaults.clone(),
    }
}

/// Fill in the archetypes of installed napplets whose entries never recorded
/// any, from the manifest this device serves for them. An entry whose
/// manifest is not here stays unrecorded and is tried again next time.
/// Returns how many were filled.
pub async fn backfill_archetypes(content: &Content) -> usize {
    let missing: Vec<(String, Option<String>)> = content
        .library_snapshot()
        .into_iter()
        .filter(|i| i.kind == LibraryKind::Napplet && i.archetypes.is_none())
        .map(|i| (i.author_npub, i.d_tag))
        .collect();
    let mut filled = 0;
    for (npub, d_tag) in missing {
        let Ok(addr) = NappletAddr::parse(&handler_key(&npub, d_tag.as_deref())) else {
            continue;
        };
        let Ok(Some(event)) = content
            .current(addr.kind(), &addr.author, addr.d_tag.as_deref())
            .await
        else {
            continue;
        };
        let Ok(manifest) = NappletManifest::from_event(event) else {
            continue;
        };
        content.record_napplet_archetypes(
            &npub,
            d_tag.as_deref(),
            LibraryArchetype::of_manifest(&manifest),
        );
        filled += 1;
    }
    filled
}

/// The [`IntentCatalog`] over this device's Library and the user's defaults.
pub struct LibraryIntents {
    content: Arc<Content>,
    defaults: IntentDefaults,
}

impl LibraryIntents {
    pub fn new(content: Arc<Content>, defaults: IntentDefaults) -> Self {
        Self { content, defaults }
    }
}

#[async_trait::async_trait]
impl IntentCatalog for LibraryIntents {
    async fn snapshot(&self) -> IntentCatalogSnapshot {
        backfill_archetypes(&self.content).await;
        let defaults = self.defaults.read().unwrap().clone();
        catalog_from(&self.content.library_snapshot(), &defaults)
    }
}

/// The nsite host label a `napplet:nsite/open` payload names: its `host`
/// label, else its manifest `naddr` (kind 35128 / 15128), else its `url`.
/// `None` when none of them names an nsite.
pub fn nsite_host(payload: Option<&Value>) -> Option<String> {
    use nostr::nips::nip19::{FromBech32, Nip19Coordinate};
    let payload = payload?.as_object()?;
    let text = |k: &str| payload.get(k).and_then(Value::as_str).map(str::trim);
    if let Some(addr) =
        text("host").and_then(|h| nsite_deck::host::resolve_label(&h.to_ascii_lowercase()))
    {
        return Some(addr.host_label());
    }
    if let Some(naddr) = text("naddr") {
        let bare = naddr
            .strip_prefix("nostr:")
            .unwrap_or(naddr)
            .to_ascii_lowercase();
        if let Ok(c) = Nip19Coordinate::from_bech32(&bare) {
            let kind = c.coordinate.kind.as_u16();
            if kind == 35128 || kind == 15128 {
                let d = c.coordinate.identifier.clone();
                return Some(
                    nsite_deck::SiteAddr {
                        author: c.coordinate.public_key,
                        d_tag: (kind == 35128 && !d.is_empty()).then_some(d),
                    }
                    .host_label(),
                );
            }
        }
    }
    text("url")
        .and_then(nsite_deck::parse_link)
        .map(|addr| addr.host_label())
}

/// A fresh token: 128 bits from the OS.
fn new_token() -> String {
    use chacha20poly1305::aead::rand_core::RngCore as _;
    let mut bytes = [0u8; 16];
    chacha20poly1305::aead::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// A payload on its way to a handler.
struct PendingIntent {
    /// Which napplet may receive it: the manifest author (hex) and `d` tag
    /// (`""` for a root napplet) of the resolved handler.
    author: String,
    d_tag: String,
    /// The handler's dTag, as the caller is told it.
    handler: String,
    /// The convention — the INC topic it is delivered on.
    convention: String,
    payload: Option<Value>,
    /// The calling napplet's dTag: the delivery's `sender`.
    sender: String,
    /// The calling window, which hears the result.
    caller: String,
    /// The call still to be answered — `Some` until the token is bound (or
    /// fails).
    request: Option<IntentRequest>,
    /// The window the token was bound to.
    bound: Option<String>,
    created: Instant,
}

/// An "open with…" question waiting on the user.
struct PendingChoice {
    caller: String,
    sender: String,
    request: IntentRequest,
    options: Vec<(Candidate, String)>,
    created: Instant,
}

/// The host's NAP-INTENT state.
#[derive(Default)]
pub(crate) struct IntentDesk {
    pending: Mutex<HashMap<String, PendingIntent>>,
    choosers: Mutex<HashMap<String, PendingChoice>>,
    last_invoke: Mutex<HashMap<String, Instant>>,
    /// Availability per archetype as last pushed, for `intent.changed`.
    /// `None` until the first baseline.
    availability: Mutex<Option<BTreeMap<String, Value>>>,
}

impl NappletHost {
    /// Frames for `session_id`'s own window, sent through its outbox.
    fn send_to(&self, session_id: &str, frames: Vec<ToShell>) {
        let outbox = {
            let sessions = self.sessions.lock().unwrap();
            match sessions.get(session_id) {
                Some(live) => live.outbox.clone(),
                None => return,
            }
        };
        for frame in frames {
            let _ = outbox.send(frame);
        }
    }

    /// The host's half of `intent.invoke` from the window `caller`: resolve,
    /// then open the handler, ask the user, or refuse. Returns the frames for
    /// the caller's window; the result may come later (see the module docs).
    pub(crate) async fn intent_invoke(
        &self,
        caller: &str,
        request: IntentRequest,
        now: Instant,
    ) -> Vec<ToShell> {
        let session = {
            let sessions = self.sessions.lock().unwrap();
            match sessions.get(caller) {
                Some(live) => live.session.clone(),
                None => return Vec::new(),
            }
        };
        let sender = session.lock().await.identity().d_tag.clone();

        // One request per moment per window: an invoke opens windows and
        // puts sheets up, and a napplet looping on it must not.
        {
            let mut last = self.intents.last_invoke.lock().unwrap();
            if last
                .get(caller)
                .is_some_and(|t| now.saturating_duration_since(*t) < INVOKE_COOLDOWN)
            {
                return vec![ToShell::to_napplet(request.failed(INTENT_BUSY))];
            }
            last.insert(caller.to_string(), now);
        }
        if self
            .intents
            .choosers
            .lock()
            .unwrap()
            .values()
            .any(|c| c.caller == caller)
        {
            return vec![ToShell::to_napplet(request.failed(INTENT_BUSY))];
        }

        let catalog = self.ctx.intents.snapshot().await;
        match nap_intent::resolve(&catalog, &request) {
            Resolution::Refused(error) => {
                tracing::info!(
                    session = caller,
                    archetype = %request.archetype,
                    error,
                    "intent refused"
                );
                vec![ToShell::to_napplet(request.failed(error))]
            }
            Resolution::Handler {
                candidate,
                convention,
            } => self.intent_route(caller, &sender, request, &candidate, &convention, now),
            Resolution::Choose(options) => {
                let token = new_token();
                let candidates = options
                    .iter()
                    .map(|(c, _)| ChooserCandidate {
                        key: c.key.clone(),
                        title: c.title.clone().unwrap_or_else(|| c.d_tag.clone()),
                        pointer: c.pointer.clone(),
                    })
                    .collect();
                let command = ToShell::ChooseIntentHandler {
                    token: token.clone(),
                    archetype: request.archetype.clone(),
                    action: request.action.clone(),
                    candidates,
                };
                self.intents.choosers.lock().unwrap().insert(
                    token,
                    PendingChoice {
                        caller: caller.to_string(),
                        sender,
                        request,
                        options,
                        created: now,
                    },
                );
                vec![command]
            }
        }
    }

    /// Send a resolved request to `candidate`: Myco's nsite opener answers at
    /// once; a napplet gets a pending payload and an `open-napplet` command,
    /// and the caller hears back when the window binds the token.
    fn intent_route(
        &self,
        caller: &str,
        sender: &str,
        request: IntentRequest,
        candidate: &Candidate,
        convention: &str,
        now: Instant,
    ) -> Vec<ToShell> {
        if candidate.key == MYCO_HANDLER_KEY {
            let Some(host) = nsite_host(request.payload.as_ref()) else {
                return vec![ToShell::to_napplet(request.failed(INVOKE_FAILED))];
            };
            tracing::info!(session = caller, %host, "intent: opening an nsite");
            return vec![
                ToShell::OpenNsite { host: host.clone() },
                ToShell::to_napplet(request.handled(MYCO_HANDLER_KEY, &host, convention)),
            ];
        }
        let Ok(addr) =
            NappletAddr::parse(&candidate.pointer).or_else(|_| NappletAddr::parse(&candidate.key))
        else {
            return vec![ToShell::to_napplet(request.failed(INVOKE_FAILED))];
        };
        let mut pending = self.intents.pending.lock().unwrap();
        if pending.len() >= MAX_PENDING_INTENTS {
            return vec![ToShell::to_napplet(request.failed(INTENT_BUSY))];
        }
        let token = new_token();
        tracing::info!(
            session = caller,
            handler = %candidate.d_tag,
            %convention,
            "intent: opening the handler"
        );
        let payload = request.payload.clone();
        pending.insert(
            token.clone(),
            PendingIntent {
                author: addr.author.to_hex(),
                d_tag: addr.d_tag.clone().unwrap_or_default(),
                handler: candidate.d_tag.clone(),
                convention: convention.to_string(),
                payload,
                sender: sender.to_string(),
                caller: caller.to_string(),
                request: Some(request),
                bound: None,
                created: now,
            },
        );
        vec![ToShell::OpenNapplet {
            pointer: candidate.pointer.clone(),
            title: candidate.title.clone().unwrap_or_default(),
            token,
        }]
    }

    /// The archetype an open chooser asks about — for the reducer, which
    /// records "Always use this" before the answer is routed.
    pub fn intent_chooser_archetype(&self, token: &str) -> Option<String> {
        self.intents
            .choosers
            .lock()
            .unwrap()
            .get(token)
            .map(|c| c.request.archetype.clone())
    }

    /// The user answered the chooser `token`: `choice` is the picked
    /// handler's key, or `None` for a cancel. Frames go to the calling
    /// window. Returns false when there was no such question (answered, or
    /// expired).
    pub fn answer_intent_chooser(&self, token: &str, choice: Option<&str>) -> bool {
        self.answer_intent_chooser_at(token, choice, Instant::now())
    }

    pub(crate) fn answer_intent_chooser_at(
        &self,
        token: &str,
        choice: Option<&str>,
        now: Instant,
    ) -> bool {
        let Some(asked) = self.intents.choosers.lock().unwrap().remove(token) else {
            return false;
        };
        let picked = choice.and_then(|key| asked.options.iter().find(|(c, _)| c.key == key));
        let frames = match picked {
            None => {
                tracing::info!(session = %asked.caller, "intent chooser cancelled");
                vec![ToShell::to_napplet(asked.request.failed(USER_CANCELLED))]
            }
            Some((candidate, convention)) => self.intent_route(
                &asked.caller,
                &asked.sender,
                asked.request.clone(),
                candidate,
                convention,
                now,
            ),
        };
        self.send_to(&asked.caller, frames);
        true
    }

    /// Bind the payload `token` names to the window `session_id`, which the
    /// window host opened (or brought forward) for it. Answers the caller and
    /// delivers at once if the napplet already listens. Refused — false, and
    /// nothing delivered — for an unknown or expired token, and for a window
    /// of any napplet other than the one the request resolved to.
    pub async fn bind_intent(&self, session_id: &str, token: &str) -> bool {
        let session = {
            let sessions = self.sessions.lock().unwrap();
            match sessions.get(session_id) {
                Some(live) => live.session.clone(),
                None => return false,
            }
        };
        let (author, d_tag) = {
            let s = session.lock().await;
            (s.identity().author.clone(), s.identity().d_tag.clone())
        };
        let answer = {
            let mut pending = self.intents.pending.lock().unwrap();
            let Some(p) = pending.get_mut(token) else {
                return false;
            };
            if p.bound.is_some() || p.created.elapsed() > PENDING_INTENT_TTL {
                return false;
            }
            if p.author != author || p.d_tag != d_tag {
                tracing::warn!(
                    session = session_id,
                    "intent token offered to a window of another napplet; refused"
                );
                return false;
            }
            p.bound = Some(session_id.to_string());
            p.request.take().map(|request| {
                (
                    p.caller.clone(),
                    request.handled(&p.handler, session_id, &p.convention),
                )
            })
        };
        if let Some((caller, result)) = answer {
            self.send_to(&caller, vec![ToShell::to_napplet(result)]);
        }
        let now = self.intent_deliveries(session_id).await;
        self.send_to(session_id, now);
        true
    }

    /// The window host could not open the handler for `token` (the napplet
    /// did not verify, the user declined): the caller hears `error`.
    pub fn fail_intent(&self, token: &str, error: &str) {
        let removed = self.intents.pending.lock().unwrap().remove(token);
        if let Some(mut p) = removed {
            if let Some(request) = p.request.take() {
                self.send_to(&p.caller, vec![ToShell::to_napplet(request.failed(error))]);
            }
        }
    }

    /// The payloads bound to `session_id` whose topic it now listens on, as
    /// `inc.event` frames — each removed as it is handed over, so it is
    /// delivered once.
    pub(crate) async fn intent_deliveries(&self, session_id: &str) -> Vec<ToShell> {
        let waiting: Vec<(String, String)> = self
            .intents
            .pending
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, p)| p.bound.as_deref() == Some(session_id))
            .map(|(token, p)| (token.clone(), p.convention.clone()))
            .collect();
        if waiting.is_empty() {
            return Vec::new();
        }
        let session = {
            let sessions = self.sessions.lock().unwrap();
            match sessions.get(session_id) {
                Some(live) => live.session.clone(),
                None => return Vec::new(),
            }
        };
        let ready: Vec<String> = {
            let s = session.lock().await;
            waiting
                .into_iter()
                .filter(|(_, topic)| s.listens_on(topic))
                .map(|(token, _)| token)
                .collect()
        };
        let mut pending = self.intents.pending.lock().unwrap();
        ready
            .into_iter()
            .filter_map(|token| pending.remove(&token))
            .map(|p| {
                tracing::info!(
                    session = session_id,
                    topic = %p.convention,
                    sender = %p.sender,
                    "intent payload delivered"
                );
                ToShell::to_napplet(myco_napplet_runtime::nap::inc::event_frame(
                    &p.convention,
                    &p.sender,
                    p.payload.as_ref(),
                ))
            })
            .collect()
    }

    /// Drop what has waited too long: a payload past [`PENDING_INTENT_TTL`]
    /// (its caller, if never answered, hears [`INVOKE_FAILED`]) and a chooser
    /// past [`CHOOSER_TTL`].
    pub(crate) fn sweep_intents(&self, now: Instant) {
        let expired: Vec<PendingIntent> = {
            let mut pending = self.intents.pending.lock().unwrap();
            let tokens: Vec<String> = pending
                .iter()
                .filter(|(_, p)| now.saturating_duration_since(p.created) > PENDING_INTENT_TTL)
                .map(|(t, _)| t.clone())
                .collect();
            tokens.iter().filter_map(|t| pending.remove(t)).collect()
        };
        for mut p in expired {
            tracing::info!(topic = %p.convention, bound = ?p.bound, "intent payload expired");
            if let Some(request) = p.request.take() {
                self.send_to(
                    &p.caller,
                    vec![ToShell::to_napplet(request.failed(INVOKE_FAILED))],
                );
            }
        }
        self.intents
            .choosers
            .lock()
            .unwrap()
            .retain(|_, c| now.saturating_duration_since(c.created) <= CHOOSER_TTL);
        self.intents
            .last_invoke
            .lock()
            .unwrap()
            .retain(|_, t| now.saturating_duration_since(*t) < INVOKE_COOLDOWN);
    }

    /// A window closed: what was bound to it, and the questions it asked, go
    /// with it.
    pub(crate) fn drop_intents_of(&self, session_id: &str) {
        self.intents
            .pending
            .lock()
            .unwrap()
            .retain(|_, p| p.bound.as_deref() != Some(session_id));
        self.intents
            .choosers
            .lock()
            .unwrap()
            .retain(|_, c| c.caller != session_id);
        self.intents.last_invoke.lock().unwrap().remove(session_id);
    }

    /// How many payloads are waiting — for tests.
    #[cfg(test)]
    pub(crate) fn pending_intent_count(&self) -> usize {
        self.intents.pending.lock().unwrap().len()
    }

    /// Record the catalog's availability as the baseline `intent.changed`
    /// diffs against, if none was recorded yet.
    pub(crate) async fn intent_baseline(&self) {
        if self.intents.availability.lock().unwrap().is_some() {
            return;
        }
        let catalog = self.ctx.intents.snapshot().await;
        let now = availability_map(&catalog);
        let mut slot = self.intents.availability.lock().unwrap();
        if slot.is_none() {
            *slot = Some(now);
        }
    }

    /// The catalog or a default may have changed: push `intent.changed` for
    /// every archetype whose availability differs from what was last pushed,
    /// to every open napplet granted `intent`.
    pub async fn intents_changed(&self) {
        let catalog = self.ctx.intents.snapshot().await;
        let now = availability_map(&catalog);
        let before = self
            .intents
            .availability
            .lock()
            .unwrap()
            .replace(now.clone());
        let Some(before) = before else {
            return;
        };
        let mut changed: Vec<Value> = Vec::new();
        for archetype in before.keys().chain(now.keys()) {
            if before.get(archetype) != now.get(archetype)
                && !changed.iter().any(|a| a["archetype"] == archetype.as_str())
            {
                changed.push(nap_intent::availability(&catalog, archetype));
            }
        }
        if changed.is_empty() {
            return;
        }
        let live: Vec<_> = {
            let sessions = self.sessions.lock().unwrap();
            sessions
                .values()
                .map(|l| (l.session.clone(), l.outbox.clone()))
                .collect()
        };
        for (session, outbox) in live {
            let session = session.lock().await;
            for availability in &changed {
                if let Some(frame) = nap_intent::changed_frame(&session, availability) {
                    let _ = outbox.send(ToShell::to_napplet(frame));
                }
            }
        }
    }
}

fn availability_map(catalog: &IntentCatalogSnapshot) -> BTreeMap<String, Value> {
    nap_intent::archetypes(catalog)
        .into_iter()
        .map(|a| {
            let v = nap_intent::availability(catalog, &a);
            (a, v)
        })
        .collect()
}

/// Settings › Default apps: one row per archetype something installed can
/// handle, with its candidates and the current default.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntentArchetypeView {
    pub archetype: String,
    /// The default's key, or `""` when the user has not chosen one (or chose
    /// one no longer installed).
    pub default_key: String,
    pub candidates: Vec<IntentCandidateView>,
}

/// One app that can handle an archetype, for Settings.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntentCandidateView {
    pub key: String,
    pub title: String,
    /// For the icon; empty for Myco's built-in.
    pub pointer: String,
}

/// The Default apps rows for the Library and defaults as they stand.
pub fn archetype_views(
    library: &[LibraryItem],
    defaults: &BTreeMap<String, String>,
) -> Vec<IntentArchetypeView> {
    let catalog = catalog_from(library, defaults);
    nap_intent::archetypes(&catalog)
        .into_iter()
        .map(|archetype| {
            let candidates = nap_intent::candidates(&catalog, &archetype);
            let default_key = candidates
                .iter()
                .find(|c| c.is_default)
                .map(|c| c.key.clone())
                .unwrap_or_default();
            IntentArchetypeView {
                archetype,
                default_key,
                candidates: candidates
                    .into_iter()
                    .map(|c| IntentCandidateView {
                        title: c.title.clone().unwrap_or_else(|| c.d_tag.clone()),
                        key: c.key,
                        pointer: c.pointer,
                    })
                    .collect(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
