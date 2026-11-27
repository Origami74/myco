//! NAP-CVM — the ContextVM bridge (napplet/naps PR #31, `naps/NAP-CVM.md`).
//!
//! ContextVM carries Model Context Protocol (MCP) JSON-RPC over Nostr: a
//! request is a kind 25910 event `p`-tagged to the server's key, its answer
//! a kind 25910 event signed by the server, `e`-tagging the request. A
//! napplet names a server and an MCP message; the runtime does everything
//! else — keys, signing, relays, correlation, initialization, encryption and
//! the payment policy.
//!
//! What this module decides, and why:
//!
//! - **One transport key per window and server.** Each napplet endpoint (a
//!   window: the session's work scope) gets a fresh, never-stored key for each
//!   server it talks to. Anything a server keys on the client pubkey —
//!   sessions, paid authorizations, server requests — therefore belongs to
//!   one endpoint and nothing else, which is the isolation the spec requires,
//!   for free. The key is not the user's identity and is never shown as one.
//! - **Correlation by event id.** A server answers the request *event*, so
//!   pending requests are keyed by the id of the event that carried them. A
//!   napplet's JSON-RPC ids are passed through untouched; they only have to
//!   be unique within its own session.
//! - **Listen before publish.** 25910 is ephemeral: a relay does not keep it
//!   for a subscription that arrives late. The listen on the transport key is
//!   live before the first request goes out.
//! - **No payment handler.** Myco has no ContextVM payment rail. A payment
//!   gate (`-32042`, `-32043`, `notifications/payment_required`) ends the call
//!   at once: `payment denied` where the napplet's own option forbids paying
//!   (`deny`) or would need a standing allowance Myco has none of (`allow`),
//!   `payment required` for `prompt`. Nothing is ever paid.
//! - **No registry.** `registry.*` validates its input and answers
//!   `unsupported method`; `registry.has` answers `false`.
//!
//! Discovery reads CEP-6 announcements (kinds 11316–11320) and CEP-17 relay
//! lists (kind 10002) from the discovery relays — the ContextVM defaults and
//! the user's own — or the napplet's hints. A request naming no relays looks
//! the server's relay list up there too, and is published to its inbox
//! relays and answered on its outbox ones. Encryption (CEP-4) is used from
//! the first message for a server whose announcement offers it, and
//! otherwise once the server shows it — a `support_encryption` tag on
//! anything it sends, or a gift-wrapped message. Gift-wrapped answers are
//! always accepted.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use nostr::nips::nip44;
use nostr::{
    Event, EventBuilder, EventId, Filter, JsonUtil, Keys, Kind, PublicKey, Tag, Timestamp,
};
use serde_json::{json, Map, Value};
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::dispatch::{NapContext, Outcome};
use crate::seams::{CvmListen, CvmRelays, Envelope, NoCvmRelays, RelayLane};
use crate::session::Session;

/// ContextVM's message kind: requests, responses and notifications.
pub const KIND_MESSAGE: u16 = 25910;
/// CEP-4 gift wrap, and its ephemeral variant (CEP-19).
pub const KIND_GIFT_WRAP: u16 = 1059;
pub const KIND_GIFT_WRAP_EPHEMERAL: u16 = 21059;
/// CEP-6 announcements: server, tools, resources, resource templates, prompts.
pub const ANNOUNCEMENT_KINDS: [u16; 5] = [11316, 11317, 11318, 11319, 11320];
const KIND_SERVER: u16 = 11316;
/// CEP-17 relay list (NIP-65's kind).
const KIND_RELAY_LIST: u16 = 10002;

/// `timeoutMs` when the napplet gives none.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// The spec's ceiling for `timeoutMs`, and Myco's policy maximum.
pub const MAX_TIMEOUT_MS: u64 = 300_000;
/// Requests one window may have waiting, across servers.
pub const MAX_PENDING: usize = 32;
/// Servers one window may hold a session with at once.
pub const MAX_SERVERS: usize = 16;
/// Relay hints taken from one server reference or query.
const MAX_RELAY_HINTS: usize = 8;
/// How long a request may spend looking up a server's relay list.
const RELAY_LIST_LOOKUP: Duration = Duration::from_secs(4);
/// How long a server's relay list (or its absence) is trusted.
const RELAY_LIST_TTL: Duration = Duration::from_secs(60 * 60);
const NO_RELAY_LIST_TTL: Duration = Duration::from_secs(10 * 60);
/// Servers whose relay lists are remembered.
const RELAY_LIST_MEMORY: usize = 256;
/// How long discovery reads relays for.
const DISCOVER_TIMEOUT: Duration = Duration::from_secs(8);
/// How long a publish may wait for a relay's `OK`, inside the call's deadline.
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(10);
/// The MCP revision NAP-CVM links.
const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
/// Request events one session remembers as expired, so what still arrives
/// for them is dropped rather than delivered as an event.
const EXPIRED_MEMORY: usize = 256;
/// Server requests one session holds for the napplet to answer. What a
/// server sends beyond that is dropped, not delivered: the server picks the
/// pace, so the bound is the runtime's.
pub const MAX_SERVER_REQUESTS: usize = 64;
/// What the runtime tells a server it is, in `initialize`. Generic on
/// purpose: the transport key is thrown away per window and server, and a
/// build version in plaintext on a public relay would tie them back together.
const CLIENT_NAME: &str = "napplet-shell";
const CLIENT_VERSION: &str = "1.0.0";

// The spec's error strings.
pub const INVALID_MESSAGE: &str = "invalid message";
pub const INVALID_REQUEST: &str = "invalid request";
pub const INVALID_TIMEOUT: &str = "invalid timeout";
pub const TIMEOUT_OUT_OF_POLICY: &str = "timeout out of policy";
pub const UNSUPPORTED_METHOD: &str = "unsupported method";
pub const UNSUPPORTED_KIND: &str = "unsupported kind";
pub const SERVER_NOT_FOUND: &str = "server not found";
pub const RELAY_TIMEOUT: &str = "relay timeout";
pub const INITIALIZATION_FAILED: &str = "initialization failed";
pub const SESSION_CLOSED: &str = "session closed";
pub const PAYMENT_REQUIRED: &str = "payment required";
pub const PAYMENT_DENIED: &str = "payment denied";
pub const POLICY_DENIED: &str = "policy denied";

/// A frame to the napplet's window, outside any call's reply.
type Push = Arc<dyn Fn(Envelope) + Send + Sync>;

/// The device's ContextVM bridge: one per device, shared by every window.
/// What is per window lives in an [`Endpoint`], keyed by the session's work
/// scope and dropped when the window closes.
pub struct Cvm {
    relays: Arc<dyn CvmRelays>,
    endpoints: Mutex<HashMap<u64, Arc<Endpoint>>>,
    max_timeout_ms: u64,
    /// Servers' CEP-17 relay lists, as last read: from discovery, or looked
    /// up by a request that named no relays. `None` is "has none".
    ///
    /// One per device, so only what the device's own discovery relays say
    /// goes in — never what a napplet's relay hints brought back, which
    /// would let one napplet steer another's requests — and a list never
    /// replaces a newer one, so an old signed list replayed is no use.
    relay_lists: Mutex<HashMap<PublicKey, RememberedList>>,
    /// Servers whose newest CEP-6 server announcement says it takes CEP-4
    /// encryption, with that announcement's time: their sessions encrypt
    /// from the first message. Filled under the same rules as `relay_lists`.
    encrypting: Mutex<HashMap<PublicKey, (Timestamp, bool)>>,
}

/// A relay list as remembered: when it was read, and the list with its
/// `created_at` — or `None` for "has none".
type RememberedList = (std::time::Instant, Option<(Timestamp, ServerRelays)>);

/// Where a server takes requests and where it answers, from its CEP-17 list.
/// An unmarked relay is both.
#[derive(Debug, Clone, PartialEq)]
struct ServerRelays {
    /// Unmarked and `read`: where requests are published.
    inbox: Vec<String>,
    /// Unmarked and `write`: where it answers — listened on too.
    outbox: Vec<String>,
}

impl Default for Cvm {
    /// A bridge that reaches nothing, for hosts and tests without ContextVM.
    fn default() -> Self {
        Self::new(Arc::new(NoCvmRelays))
    }
}

impl Cvm {
    pub fn new(relays: Arc<dyn CvmRelays>) -> Self {
        Self {
            relays,
            endpoints: Mutex::new(HashMap::new()),
            max_timeout_ms: MAX_TIMEOUT_MS,
            relay_lists: Mutex::new(HashMap::new()),
            encrypting: Mutex::new(HashMap::new()),
        }
    }

    /// Lower the policy maximum for `timeoutMs`: a request above it is
    /// refused with `timeout out of policy`, never clamped.
    pub fn with_max_timeout_ms(mut self, ms: u64) -> Self {
        self.max_timeout_ms = ms.clamp(1, MAX_TIMEOUT_MS);
        self
    }

    /// The window's endpoint, made on first use. Torn down — every session
    /// closed, every waiting call answered `session closed` — when the
    /// session's work scope is cancelled, which is the window closing.
    fn endpoint(self: &Arc<Self>, session: &Session) -> Option<Arc<Endpoint>> {
        let scope = session.work().session();
        let id = scope.id()?;
        let mut endpoints = self.endpoints.lock().unwrap();
        if let Some(endpoint) = endpoints.get(&id) {
            return Some(endpoint.clone());
        }
        let endpoint = Arc::new(Endpoint {
            push: session.streamer().map(|s| s.push.clone()),
            servers: Mutex::new(HashMap::new()),
            pending: Arc::new(AtomicU64::new(0)),
        });
        endpoints.insert(id, endpoint.clone());
        drop(endpoints);
        let hub: Weak<Cvm> = Arc::downgrade(self);
        scope.on_cancel(move || {
            if let Some(hub) = hub.upgrade() {
                let gone = hub.endpoints.lock().unwrap().remove(&id);
                if let Some(endpoint) = gone {
                    endpoint.shutdown();
                }
            }
        });
        Some(endpoint)
    }

    /// End a window's ContextVM work now: every session closed, every waiting
    /// call answered `session closed`. `window` is the session's
    /// [`crate::seams::WorkScope::id`]. A host calls this when it closes the
    /// window, because a call still running holds a snapshot of the session —
    /// and with it the work scope whose end would otherwise do this.
    pub fn end_window(&self, window: u64) {
        let gone = self.endpoints.lock().unwrap().remove(&window);
        if let Some(endpoint) = gone {
            endpoint.shutdown();
        }
    }

    /// How many windows have ContextVM work open.
    pub fn open_windows(&self) -> usize {
        self.endpoints.lock().unwrap().len()
    }

    /// The endpoint, only if it already exists — for `close`, which must not
    /// make one.
    fn existing_endpoint(&self, session: &Session) -> Option<Arc<Endpoint>> {
        let id = session.work().session().id()?;
        self.endpoints.lock().unwrap().get(&id).cloned()
    }
}

/// One window's ContextVM state.
struct Endpoint {
    push: Option<Push>,
    servers: Mutex<HashMap<PublicKey, Arc<ServerSession>>>,
    /// Requests waiting, across this window's servers.
    pending: Arc<AtomicU64>,
}

impl Endpoint {
    fn shutdown(&self) {
        let servers: Vec<_> = self.servers.lock().unwrap().drain().collect();
        for (_, server) in servers {
            server.shutdown();
        }
    }
}

/// The server a napplet named: its key and where to reach it.
#[derive(Debug, Clone)]
struct ServerRef {
    pubkey: PublicKey,
    relays: Vec<String>,
}

impl ServerRef {
    fn to_json(&self) -> Value {
        let mut out = json!({ "pubkey": self.pubkey.to_hex() });
        if !self.relays.is_empty() {
            out["relays"] = json!(self.relays);
        }
        out
    }
}

/// Who a waiting request answers to.
#[derive(Debug, Clone)]
enum Owner {
    /// The napplet, with the JSON-RPC id it sent and its payment option.
    Napplet { id: Value, payment: Payment },
    /// The runtime itself: initialization.
    Shell,
}

/// What ends a waiting request.
#[derive(Debug)]
enum Answer {
    Message(Value),
    Fail(&'static str),
}

struct Pending {
    tx: oneshot::Sender<Answer>,
    owner: Owner,
}

#[derive(Default)]
struct State {
    closed: bool,
    /// Waiting requests, by the id of the event that carried them.
    pending: HashMap<EventId, Pending>,
    /// JSON-RPC ids the napplet has used for requests in this session.
    client_ids: HashSet<String>,
    /// Server requests delivered to the napplet and not yet answered: by
    /// JSON-RPC id, the id of the event that carried each, which the answer
    /// `e`-tags.
    server_requests: HashMap<String, EventId>,
    /// Requests that timed out or were closed: what still arrives for them
    /// is dropped.
    expired: VecDeque<EventId>,
    /// Whether to gift-wrap what is sent (CEP-4).
    encrypt: bool,
    /// Whether anything was sent yet — the first message carries the
    /// requested payment mode (CEP-8).
    sent_any: bool,
    initialized: bool,
}

/// A window's MCP session with one server.
struct ServerSession {
    /// The server, with the relays requests are published to.
    server: ServerRef,
    /// Where its answers are listened for: those, and its outbox relays.
    listen_relays: Vec<String>,
    keys: Keys,
    state: Mutex<State>,
    /// Serializes initialization: one `initialize` per session.
    init: tokio::sync::Mutex<()>,
    listen: Mutex<Option<CvmListen>>,
    push: Option<Push>,
    next_internal: AtomicU64,
}

impl ServerSession {
    fn shutdown(&self) {
        let pending: Vec<Pending> = {
            let mut state = self.state.lock().unwrap();
            state.closed = true;
            state.server_requests.clear();
            state.pending.drain().map(|(_, p)| p).collect()
        };
        for p in pending {
            let _ = p.tx.send(Answer::Fail(SESSION_CLOSED));
        }
        self.listen.lock().unwrap().take();
    }

    /// One event from the relays, on the transport key.
    fn receive(&self, event: Event) {
        let (inner, wrapped) = match event.kind.as_u16() {
            KIND_MESSAGE => (event, false),
            KIND_GIFT_WRAP | KIND_GIFT_WRAP_EPHEMERAL => {
                let Some(inner) = self.unwrap(&event) else {
                    return;
                };
                (inner, true)
            }
            _ => return,
        };
        // The relay delivering it proves nothing: only the server's own
        // signature does.
        if inner.kind.as_u16() != KIND_MESSAGE || inner.pubkey != self.server.pubkey {
            return;
        }
        let Ok(message) = serde_json::from_str::<Value>(&inner.content) else {
            return;
        };
        if message.get("jsonrpc") != Some(&json!("2.0")) || !message.is_object() {
            return;
        }
        let supports_encryption = inner
            .tags
            .iter()
            .any(|t| t.kind().as_str() == "support_encryption");
        let refers_to = inner
            .tags
            .iter()
            .find(|t| t.kind().as_str() == "e")
            .and_then(|t| t.content())
            .and_then(|id| EventId::from_hex(id).ok());

        let mut state = self.state.lock().unwrap();
        if state.closed {
            return;
        }
        if wrapped || supports_encryption {
            state.encrypt = true;
        }
        let has_id = message.get("id").is_some_and(|id| !id.is_null());
        let method = message.get("method").and_then(Value::as_str);
        match (method, has_id) {
            // A response: it ends the request it answers, or nothing.
            (None, true) => {
                let Some(request) = refers_to else { return };
                let Some(pending) = state.pending.remove(&request) else {
                    return;
                };
                remember_expired(&mut state, request);
                drop(state);
                let answer = match payment_gate_code(&message) {
                    Some(_) => Answer::Fail(payment_outcome(&pending.owner)),
                    None => Answer::Message(match &pending.owner {
                        Owner::Napplet { id, .. } => with_id(message, id.clone()),
                        Owner::Shell => message,
                    }),
                };
                let _ = pending.tx.send(answer);
            }
            // A notification.
            (Some(method), false) => {
                if let Some(request) = refers_to {
                    if state.expired.contains(&request) {
                        return;
                    }
                    if method == "notifications/payment_required" {
                        if let Some(pending) = state.pending.remove(&request) {
                            remember_expired(&mut state, request);
                            drop(state);
                            let _ = pending
                                .tx
                                .send(Answer::Fail(payment_outcome(&pending.owner)));
                        }
                        return;
                    }
                    if matches!(
                        state.pending.get(&request).map(|p| &p.owner),
                        Some(Owner::Shell)
                    ) {
                        return;
                    }
                }
                if method.starts_with("notifications/payment_") {
                    return;
                }
                drop(state);
                self.deliver(message);
            }
            // A request from the server: the napplet answers it.
            (Some(_), true) => {
                let key = id_key(&message["id"]);
                if !state.server_requests.contains_key(&key)
                    && state.server_requests.len() >= MAX_SERVER_REQUESTS
                {
                    return;
                }
                state.server_requests.insert(key, inner.id);
                drop(state);
                self.deliver(message);
            }
            (None, false) => {}
        }
    }

    fn unwrap(&self, wrap: &Event) -> Option<Event> {
        let plain = nip44::decrypt(self.keys.secret_key(), &wrap.pubkey, &wrap.content).ok()?;
        let inner = Event::from_json(plain).ok()?;
        inner.verify().ok()?;
        Some(inner)
    }

    /// A `cvm.event` to this window, and nobody else.
    fn deliver(&self, message: Value) {
        if let Some(push) = &self.push {
            push(
                Envelope::new("cvm.event")
                    .with_field("server", self.server.to_json())
                    .with_field("message", message),
            );
        }
    }

    /// The event carrying `message`, gift-wrapped when the server takes that.
    /// `answers` is the server request's event, for a response to one.
    fn seal(
        &self,
        message: &Value,
        answers: Option<EventId>,
    ) -> Result<(Event, EventId), &'static str> {
        let (encrypt, first) = {
            let mut state = self.state.lock().unwrap();
            let first = !state.sent_any;
            state.sent_any = true;
            (state.encrypt, first)
        };
        let mut tags = vec![Tag::public_key(self.server.pubkey)];
        if let Some(request) = answers {
            tags.push(Tag::event(request));
        }
        if first {
            // CEP-8: ask for explicit gating, so a priced call fails as an
            // error this runtime can see rather than waiting on a payment.
            tags.push(Tag::parse(["payment_interaction", "explicit_gating"]).unwrap());
        }
        let inner = EventBuilder::new(Kind::Custom(KIND_MESSAGE), message.to_string())
            .tags(tags)
            .sign_with_keys(&self.keys)
            .map_err(|_| INVALID_MESSAGE)?;
        let id = inner.id;
        if !encrypt {
            return Ok((inner, id));
        }
        let ephemeral = Keys::generate();
        let content = nip44::encrypt(
            ephemeral.secret_key(),
            &self.server.pubkey,
            inner.as_json(),
            nip44::Version::V2,
        )
        .map_err(|_| INVALID_MESSAGE)?;
        let wrap = EventBuilder::new(Kind::Custom(KIND_GIFT_WRAP), content)
            .tags([Tag::public_key(self.server.pubkey)])
            .sign_with_keys(&ephemeral)
            .map_err(|_| INVALID_MESSAGE)?;
        Ok((wrap, id))
    }
}

/// Removes a waiting request when its call ends however it ends — answered,
/// timed out, or dropped — and remembers it as expired.
struct PendingGuard {
    session: Arc<ServerSession>,
    request: EventId,
    count: Arc<AtomicU64>,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        let mut state = self.session.state.lock().unwrap();
        state.pending.remove(&self.request);
        remember_expired(&mut state, self.request);
        self.count.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A client request id taken for one call. Duplicates are refused while it
/// is held; if the call ends before the request was handed to the relays —
/// a failed initialization, a refused publish slot, a timeout before
/// sending — the id is given back, so the napplet may retry under it.
struct ClaimedId {
    session: Arc<ServerSession>,
    key: String,
    sent: AtomicBool,
}

impl ClaimedId {
    fn take(session: &Arc<ServerSession>, key: String) -> Result<Self, &'static str> {
        let mut state = session.state.lock().unwrap();
        if state.closed {
            return Err(SESSION_CLOSED);
        }
        if !state.client_ids.insert(key.clone()) {
            return Err(INVALID_MESSAGE);
        }
        Ok(Self {
            session: session.clone(),
            key,
            sent: AtomicBool::new(false),
        })
    }
}

impl Drop for ClaimedId {
    fn drop(&mut self) {
        if !self.sent.load(Ordering::SeqCst) {
            self.session
                .state
                .lock()
                .unwrap()
                .client_ids
                .remove(&self.key);
        }
    }
}

fn remember_expired(state: &mut State, request: EventId) {
    if state.expired.contains(&request) {
        return;
    }
    if state.expired.len() >= EXPIRED_MEMORY {
        state.expired.pop_front();
    }
    state.expired.push_back(request);
}

/// `-32042` (payment required) or `-32043` (payment pending), CEP-8's gates.
fn payment_gate_code(message: &Value) -> Option<i64> {
    let code = message.get("error")?.get("code")?.as_i64()?;
    matches!(code, -32042 | -32043).then_some(code)
}

/// What a payment gate ends a call with. Myco pays nothing: `deny` forbids
/// it, `allow` needs a standing allowance Myco has none of, and `prompt`
/// would need a payment handler Myco does not have.
fn payment_outcome(owner: &Owner) -> &'static str {
    match owner {
        Owner::Napplet {
            payment: Payment::Prompt,
            ..
        } => PAYMENT_REQUIRED,
        Owner::Napplet { .. } => PAYMENT_DENIED,
        Owner::Shell => INITIALIZATION_FAILED,
    }
}

fn with_id(mut message: Value, id: Value) -> Value {
    message["id"] = id;
    message
}

/// A JSON-RPC id as a set key: `1` and `"1"` are different ids.
fn id_key(id: &Value) -> String {
    id.to_string()
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Payment {
    Deny,
    Prompt,
    Allow,
}

/// What `CvmRequestOptions` (and the call half of `CvmRegistryCallOptions`)
/// asked for.
#[derive(Debug, Clone)]
struct RequestOptions {
    timeout: Duration,
    initialize: Option<bool>,
    payment: Payment,
}

/// The three forms an MCP message takes, per the spec's table.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Form {
    Request,
    Notification,
    Response,
}

/// Handle an inbound `cvm.*` message.
pub async fn handle(ctx: &NapContext, session: &Session, message: &Envelope) -> Outcome {
    let reply = match message.action() {
        "discover" => discover(ctx, message).await,
        "request" => request(ctx, session, message).await,
        "close" => close(ctx, session, message),
        "registry.list" => registry_list(message),
        "registry.has" => registry_has(message),
        "registry.describe" => registry_describe(message),
        "registry.call" => registry_call(ctx, message),
        // `cvm.event` travels the other way; anything else is not a NAP-CVM
        // request. NIP-5D: silence.
        _ => return Outcome::Ignore,
    };
    Outcome::Reply(vec![reply])
}

fn options_of(value: Option<&Value>, max_ms: u64) -> Result<RequestOptions, &'static str> {
    let empty = Map::new();
    let options = match value {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(o)) => o,
        Some(_) => return Err(INVALID_REQUEST),
    };
    let timeout_ms = match options.get("timeoutMs") {
        None | Some(Value::Null) => DEFAULT_TIMEOUT_MS,
        Some(v) => match v.as_u64() {
            Some(ms) if (1..=MAX_TIMEOUT_MS).contains(&ms) => ms,
            _ => return Err(INVALID_TIMEOUT),
        },
    };
    if timeout_ms > max_ms {
        return Err(TIMEOUT_OUT_OF_POLICY);
    }
    let initialize = match options.get("initialize") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err(INVALID_REQUEST),
    };
    let payment = match options.get("payment").map(|v| v.as_str()) {
        None => Payment::Prompt,
        Some(Some("deny")) => Payment::Deny,
        Some(Some("prompt")) => Payment::Prompt,
        Some(Some("allow")) => Payment::Allow,
        Some(_) => return Err(INVALID_REQUEST),
    };
    if let Some(cache) = options.get("cache") {
        if !matches!(cache.as_str(), Some("default" | "reload" | "no-store")) {
            return Err(INVALID_REQUEST);
        }
    }
    Ok(RequestOptions {
        timeout: Duration::from_millis(timeout_ms),
        initialize,
        payment,
    })
}

/// Relay hints the runtime would dial: `ws(s)://`, a public host, at most
/// [`MAX_RELAY_HINTS`]. The rest are dropped, not refused — a hint is a hint.
fn relay_hints(value: Option<&Value>) -> Result<Vec<String>, &'static str> {
    let list = match value {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(list)) => list,
        Some(_) => return Err(INVALID_REQUEST),
    };
    let mut out = Vec::new();
    for hint in list {
        let url = hint.as_str().ok_or(INVALID_REQUEST)?;
        if let Ok(RelayLane::Internet { url }) = super::outbox::validate_relay_url(url) {
            if !out.contains(&url) && out.len() < MAX_RELAY_HINTS {
                out.push(url);
            }
        }
    }
    Ok(out)
}

fn server_ref(value: Option<&Value>) -> Result<ServerRef, &'static str> {
    let server = value.and_then(Value::as_object).ok_or(INVALID_REQUEST)?;
    let hex = server
        .get("pubkey")
        .and_then(Value::as_str)
        .ok_or(INVALID_REQUEST)?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(INVALID_REQUEST);
    }
    let pubkey = PublicKey::from_hex(hex).map_err(|_| INVALID_REQUEST)?;
    Ok(ServerRef {
        pubkey,
        relays: relay_hints(server.get("relays"))?,
    })
}

fn valid_id(id: &Value) -> bool {
    match id {
        Value::String(_) => true,
        Value::Number(n) => n.is_i64() || n.is_u64(),
        _ => false,
    }
}

/// Which MCP form `message` is, or `invalid message`.
fn form_of(message: Option<&Value>) -> Result<Form, &'static str> {
    let m = message.and_then(Value::as_object).ok_or(INVALID_MESSAGE)?;
    if m.get("jsonrpc") != Some(&json!("2.0")) {
        return Err(INVALID_MESSAGE);
    }
    let id = m.get("id");
    if id.is_some_and(|id| !valid_id(id)) {
        return Err(INVALID_MESSAGE);
    }
    if m.get("params").is_some_and(|p| !p.is_object()) {
        return Err(INVALID_MESSAGE);
    }
    let method = m.get("method");
    if method.is_some_and(|m| !m.is_string()) {
        return Err(INVALID_MESSAGE);
    }
    let (result, error) = (m.get("result"), m.get("error"));
    match (
        method.is_some(),
        id.is_some(),
        result.is_some(),
        error.is_some(),
    ) {
        (true, true, false, false) => Ok(Form::Request),
        (true, false, false, false) => Ok(Form::Notification),
        (false, true, true, false) if m.get("params").is_none() => {
            result.filter(|r| r.is_object()).ok_or(INVALID_MESSAGE)?;
            Ok(Form::Response)
        }
        (false, true, false, true) if m.get("params").is_none() => {
            let error = error.and_then(Value::as_object).ok_or(INVALID_MESSAGE)?;
            let code_ok = error.get("code").is_some_and(|c| c.is_i64());
            let message_ok = error.get("message").is_some_and(Value::is_string);
            if code_ok && message_ok {
                Ok(Form::Response)
            } else {
                Err(INVALID_MESSAGE)
            }
        }
        _ => Err(INVALID_MESSAGE),
    }
}

async fn request(ctx: &NapContext, session: &Session, message: &Envelope) -> Envelope {
    let cvm = &ctx.cvm;
    // Everything checkable is checked before anything is published.
    let options = match options_of(message.field("options"), cvm.max_timeout_ms) {
        Ok(o) => o,
        Err(e) => return message.to_error(e),
    };
    let deadline = Instant::now() + options.timeout;
    let server = match server_ref(message.field("server")) {
        Ok(s) => s,
        Err(e) => return message.to_error(e),
    };
    let form = match form_of(message.field("message")) {
        Ok(f) => f,
        Err(e) => return message.to_error(e),
    };
    let mcp = message.field("message").cloned().unwrap_or_default();

    let work = async {
        let Some(endpoint) = cvm.endpoint(session) else {
            return Err(POLICY_DENIED);
        };
        // Cheap refusals before a session is opened for them.
        if !cvm.relays.available() {
            return Err(SERVER_NOT_FOUND);
        }
        let server_session = open(cvm, &endpoint, &server).await?;
        match form {
            Form::Request => {
                let id = mcp["id"].clone();
                let claim = ClaimedId::take(&server_session, id_key(&id))?;
                let napplet_initializes = mcp["method"] == "initialize";
                if !napplet_initializes && options.initialize != Some(false) {
                    initialize(cvm, &endpoint, &server_session).await?;
                }
                let owner = Owner::Napplet {
                    id,
                    payment: options.payment,
                };
                let answer = send_request(
                    cvm,
                    &endpoint,
                    &server_session,
                    &mcp,
                    owner,
                    Some(&claim.sent),
                )
                .await?;
                if napplet_initializes && answer.get("result").is_some() {
                    server_session.state.lock().unwrap().initialized = true;
                }
                Ok(Some(answer))
            }
            Form::Notification => {
                if mcp["method"] == "notifications/cancelled" {
                    // Only this napplet's own requests, in this session.
                    let target = &mcp["params"]["requestId"];
                    let state = server_session.state.lock().unwrap();
                    let ours = state
                        .pending
                        .values()
                        .any(|p| matches!(&p.owner, Owner::Napplet { id, .. } if id == target));
                    if !ours {
                        return Err(INVALID_MESSAGE);
                    }
                }
                if options.initialize == Some(true) {
                    initialize(cvm, &endpoint, &server_session).await?;
                }
                publish(cvm, &server_session, &mcp, None).await?;
                Ok(None)
            }
            Form::Response => {
                let key = id_key(&mcp["id"]);
                let Some(request) = server_session
                    .state
                    .lock()
                    .unwrap()
                    .server_requests
                    .remove(&key)
                else {
                    return Err(INVALID_MESSAGE);
                };
                // The server matches the answer by the event it answers.
                if let Err(e) = publish(cvm, &server_session, &mcp, Some(request)).await {
                    // Not delivered: still the napplet's to answer.
                    let mut state = server_session.state.lock().unwrap();
                    if !state.closed {
                        state.server_requests.insert(key, request);
                    }
                    return Err(e);
                }
                Ok(None)
            }
        }
    };
    match tokio::time::timeout_at(deadline, work).await {
        Ok(Ok(Some(answer))) => message.to_result().with_field("message", answer),
        Ok(Ok(None)) => message.to_result(),
        Ok(Err(e)) => message.to_error(e),
        Err(_) => message.to_error(RELAY_TIMEOUT),
    }
}

/// The window's session with `server`, opened — key made, listen live — if
/// it has none.
async fn open(
    cvm: &Arc<Cvm>,
    endpoint: &Arc<Endpoint>,
    server: &ServerRef,
) -> Result<Arc<ServerSession>, &'static str> {
    if let Some(existing) = endpoint.servers.lock().unwrap().get(&server.pubkey) {
        return Ok(existing.clone());
    }
    let (relays, listen_relays) = server_relays(cvm, server).await;
    if relays.is_empty() {
        return Err(SERVER_NOT_FOUND);
    }
    // A server that announced CEP-4 support gets nothing in plaintext, its
    // `initialize` included.
    let encrypt = cvm
        .encrypting
        .lock()
        .unwrap()
        .get(&server.pubkey)
        .is_some_and(|(_, yes)| *yes);
    let session = Arc::new(ServerSession {
        server: ServerRef {
            pubkey: server.pubkey,
            relays,
        },
        listen_relays,
        keys: Keys::generate(),
        state: Mutex::new(State {
            encrypt,
            ..State::default()
        }),
        init: tokio::sync::Mutex::new(()),
        listen: Mutex::new(None),
        push: endpoint.push.clone(),
        next_internal: AtomicU64::new(1),
    });
    let me = session.keys.public_key();
    let filters = vec![
        Filter::new()
            .kind(Kind::Custom(KIND_MESSAGE))
            .author(server.pubkey)
            .pubkey(me),
        // Gift wraps carry a random author and, under NIP-59, a backdated
        // timestamp: matched on the recipient alone.
        Filter::new()
            .kinds([
                Kind::Custom(KIND_GIFT_WRAP),
                Kind::Custom(KIND_GIFT_WRAP_EPHEMERAL),
            ])
            .pubkey(me),
    ];
    let weak = Arc::downgrade(&session);
    let deliver: Arc<dyn Fn(Event) + Send + Sync> = Arc::new(move |event| {
        if let Some(session) = weak.upgrade() {
            session.receive(event);
        }
    });
    let listen = cvm
        .relays
        .listen(&session.listen_relays, filters, deliver)
        .await;
    *session.listen.lock().unwrap() = Some(listen);

    let mut servers = endpoint.servers.lock().unwrap();
    // Opened twice at once: the first one in wins, the other is dropped.
    if let Some(existing) = servers.get(&server.pubkey) {
        return Ok(existing.clone());
    }
    if servers.len() >= MAX_SERVERS {
        return Err(POLICY_DENIED);
    }
    servers.insert(server.pubkey, session.clone());
    Ok(session)
}

/// Where to publish to `server`, and where to listen for its answers.
///
/// Relays the napplet named are used as given. Without them, the server's
/// CEP-17 relay list says: requests go to its inbox relays, and its outbox
/// relays are listened on too. A server with no list is reached on the
/// default relays.
async fn server_relays(cvm: &Cvm, server: &ServerRef) -> (Vec<String>, Vec<String>) {
    if !server.relays.is_empty() {
        return (server.relays.clone(), server.relays.clone());
    }
    match relay_list(cvm, server.pubkey).await {
        Some(list) if !list.inbox.is_empty() => {
            let mut listen = list.inbox.clone();
            for url in list.outbox {
                if !listen.contains(&url) {
                    listen.push(url);
                }
            }
            (list.inbox, listen)
        }
        _ => {
            let defaults = cvm.relays.default_relays();
            (defaults.clone(), defaults)
        }
    }
}

/// `server`'s relay list, from memory or read from the discovery relays.
async fn relay_list(cvm: &Cvm, server: PublicKey) -> Option<ServerRelays> {
    let now = std::time::Instant::now();
    if let Some((at, list)) = cvm.relay_lists.lock().unwrap().get(&server) {
        let ttl = if list.is_some() {
            RELAY_LIST_TTL
        } else {
            NO_RELAY_LIST_TTL
        };
        if now.duration_since(*at) < ttl {
            return list.as_ref().map(|(_, list)| list.clone());
        }
    }
    let relays = cvm.relays.discovery_relays().await;
    if relays.is_empty() {
        return None;
    }
    let lists = cvm
        .relays
        .query(
            &relays,
            vec![Filter::new()
                .kind(Kind::Custom(KIND_RELAY_LIST))
                .author(server)],
            RELAY_LIST_LOOKUP,
        )
        .await;
    let newest = lists
        .into_iter()
        .filter(|e| e.pubkey == server && e.kind.as_u16() == KIND_RELAY_LIST)
        .max_by_key(|e| e.created_at);
    let list = newest.map(|e| (e.created_at, parse_relay_list(&e)));
    remember_relay_list(cvm, server, list)
}

/// Remember what was read of `server`'s relay list, and return what is now
/// known. A list never gives way to an older one, nor to a read that found
/// none (a relay timing out is not the list going away): the newer list
/// stays, trusted for another while.
fn remember_relay_list(
    cvm: &Cvm,
    server: PublicKey,
    read: Option<(Timestamp, ServerRelays)>,
) -> Option<ServerRelays> {
    let mut lists = cvm.relay_lists.lock().unwrap();
    let kept = match (lists.get(&server).and_then(|(_, l)| l.clone()), read) {
        (Some(known), Some(read)) if known.0 > read.0 => Some(known),
        (_, Some(read)) => Some(read),
        (known, None) => known,
    };
    if lists.len() >= RELAY_LIST_MEMORY && !lists.contains_key(&server) {
        // Forget the oldest.
        if let Some(oldest) = lists.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| *k) {
            lists.remove(&oldest);
        }
    }
    lists.insert(server, (std::time::Instant::now(), kept.clone()));
    kept.map(|(_, list)| list)
}

/// Remember whether `server` announces CEP-4 support, as of `at`; an older
/// announcement never overrides a newer one.
fn remember_encryption(cvm: &Cvm, server: PublicKey, at: Timestamp, supports: bool) {
    let mut known = cvm.encrypting.lock().unwrap();
    if known.get(&server).is_some_and(|(seen, _)| *seen >= at) {
        return;
    }
    if known.len() >= RELAY_LIST_MEMORY && !known.contains_key(&server) {
        if let Some(oldest) = known.iter().min_by_key(|(_, (t, _))| *t).map(|(k, _)| *k) {
            known.remove(&oldest);
        }
    }
    known.insert(server, (at, supports));
}

/// A CEP-17 list's relays the runtime would dial, by direction.
fn parse_relay_list(list: &Event) -> ServerRelays {
    let mut out = ServerRelays {
        inbox: Vec::new(),
        outbox: Vec::new(),
    };
    for tag in list.tags.iter() {
        let v = tag.as_slice();
        if v.first().map(String::as_str) != Some("r") {
            continue;
        }
        let Some(Ok(RelayLane::Internet { url })) =
            v.get(1).map(|u| super::outbox::validate_relay_url(u))
        else {
            continue;
        };
        let marker = v.get(2).map(String::as_str);
        if matches!(marker, None | Some("read") | Some(""))
            && !out.inbox.contains(&url)
            && out.inbox.len() < MAX_RELAY_HINTS
        {
            out.inbox.push(url.clone());
        }
        if matches!(marker, None | Some("write") | Some(""))
            && !out.outbox.contains(&url)
            && out.outbox.len() < MAX_RELAY_HINTS
        {
            out.outbox.push(url);
        }
    }
    out
}

/// MCP initialization, once per session, on the call's deadline. The client
/// advertises no capabilities: it cannot know which server requests the
/// napplet will service, and forwarding one is not servicing it.
async fn initialize(
    cvm: &Arc<Cvm>,
    endpoint: &Arc<Endpoint>,
    session: &Arc<ServerSession>,
) -> Result<(), &'static str> {
    let _one = session.init.lock().await;
    if session.state.lock().unwrap().initialized {
        return Ok(());
    }
    let n = session.next_internal.fetch_add(1, Ordering::SeqCst);
    let init = json!({
        "jsonrpc": "2.0",
        "id": format!("myco-init-{n}"),
        "method": "initialize",
        "params": {
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": CLIENT_NAME, "version": CLIENT_VERSION },
        },
    });
    let answer = match send_request(cvm, endpoint, session, &init, Owner::Shell, None).await {
        Ok(answer) => answer,
        Err(SESSION_CLOSED) => return Err(SESSION_CLOSED),
        Err(_) => return Err(INITIALIZATION_FAILED),
    };
    if answer.get("result").is_none() {
        return Err(INITIALIZATION_FAILED);
    }
    // Initialized once the server has been told so: a session whose
    // notification never went out initializes again on the next call rather
    // than never sending it.
    let initialized = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    publish(cvm, session, &initialized, None).await?;
    session.state.lock().unwrap().initialized = true;
    Ok(())
}

/// Publish a request and wait for what ends it. The caller bounds the wait.
/// `sent` is set the moment the request is handed to the relays: from then
/// on the server may have seen its id.
async fn send_request(
    cvm: &Arc<Cvm>,
    endpoint: &Arc<Endpoint>,
    session: &Arc<ServerSession>,
    mcp: &Value,
    owner: Owner,
    sent: Option<&AtomicBool>,
) -> Result<Value, &'static str> {
    if endpoint.pending.fetch_add(1, Ordering::SeqCst) >= MAX_PENDING as u64 {
        endpoint.pending.fetch_sub(1, Ordering::SeqCst);
        return Err(POLICY_DENIED);
    }
    let (event, request) = match session.seal(mcp, None) {
        Ok(sealed) => sealed,
        Err(e) => {
            endpoint.pending.fetch_sub(1, Ordering::SeqCst);
            return Err(e);
        }
    };
    let (tx, rx) = oneshot::channel();
    {
        let mut state = session.state.lock().unwrap();
        if state.closed {
            endpoint.pending.fetch_sub(1, Ordering::SeqCst);
            return Err(SESSION_CLOSED);
        }
        // Registered before it is published: the answer can come back before
        // the publish's `OK` does.
        state.pending.insert(request, Pending { tx, owner });
    }
    let _guard = PendingGuard {
        session: session.clone(),
        request,
        count: endpoint.pending.clone(),
    };
    if let Some(sent) = sent {
        sent.store(true, Ordering::SeqCst);
    }
    if !cvm
        .relays
        .publish(&session.server.relays, &event, PUBLISH_TIMEOUT)
        .await
    {
        return Err(SERVER_NOT_FOUND);
    }
    match rx.await {
        Ok(Answer::Message(m)) => Ok(m),
        Ok(Answer::Fail(e)) => Err(e),
        Err(_) => Err(SESSION_CLOSED),
    }
}

/// Publish a notification or response; nothing is awaited but the relay.
/// `answers` is the server request a response answers.
async fn publish(
    cvm: &Arc<Cvm>,
    session: &Arc<ServerSession>,
    mcp: &Value,
    answers: Option<EventId>,
) -> Result<(), &'static str> {
    if session.state.lock().unwrap().closed {
        return Err(SESSION_CLOSED);
    }
    let (event, _) = session.seal(mcp, answers)?;
    if cvm
        .relays
        .publish(&session.server.relays, &event, PUBLISH_TIMEOUT)
        .await
    {
        Ok(())
    } else {
        Err(SERVER_NOT_FOUND)
    }
}

/// `cvm.close`: this window's session with the server, and nobody else's.
fn close(ctx: &NapContext, session: &Session, message: &Envelope) -> Envelope {
    let server = match server_ref(message.field("server")) {
        Ok(s) => s,
        Err(e) => return message.to_error(e),
    };
    if let Some(endpoint) = ctx.cvm.existing_endpoint(session) {
        let gone = endpoint.servers.lock().unwrap().remove(&server.pubkey);
        if let Some(gone) = gone {
            gone.shutdown();
        }
    }
    message.to_result()
}

/// `cvm.discover`: CEP-6 announcements, with CEP-17 relay lists.
async fn discover(ctx: &NapContext, message: &Envelope) -> Envelope {
    let empty = Map::new();
    let query = match message.field("query") {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(q)) => q,
        Some(_) => return message.to_error(INVALID_REQUEST),
    };
    let search = match query.get("search") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.to_lowercase()),
        Some(_) => return message.to_error(INVALID_REQUEST),
    };
    let kinds: Vec<u16> = match query.get("kinds") {
        None | Some(Value::Null) => ANNOUNCEMENT_KINDS.to_vec(),
        Some(Value::Array(list)) => {
            let mut kinds = Vec::new();
            for k in list {
                let Some(k) = k.as_u64() else {
                    return message.to_error(INVALID_REQUEST);
                };
                match u16::try_from(k) {
                    Ok(k) if ANNOUNCEMENT_KINDS.contains(&k) => kinds.push(k),
                    _ => return message.to_error(UNSUPPORTED_KIND),
                }
            }
            kinds
        }
        Some(_) => return message.to_error(INVALID_REQUEST),
    };
    let limit = match query.get("limit") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_u64() {
            Some(n) if n > 0 => Some(n as usize),
            _ => return message.to_error(INVALID_REQUEST),
        },
    };
    let hints = match relay_hints(query.get("relays")) {
        Ok(h) => h,
        Err(e) => return message.to_error(e),
    };
    if kinds.is_empty() || !ctx.cvm.relays.available() {
        return message.to_result().with_field("servers", json!([]));
    }
    // Only what the device's own discovery relays say is remembered for
    // other calls: a napplet's hints answer that napplet, and nobody else.
    let remember = hints.is_empty();
    let relays = if remember {
        ctx.cvm.relays.discovery_relays().await
    } else {
        hints
    };
    let filter = Filter::new()
        .kinds(kinds.iter().map(|k| Kind::Custom(*k)))
        .limit(500);
    let events = ctx
        .cvm
        .relays
        .query(&relays, vec![filter], DISCOVER_TIMEOUT)
        .await;
    let mut servers = servers_of(&kinds, events);
    if remember {
        for found in &servers {
            if let Some((at, supports)) = found.encryption {
                remember_encryption(&ctx.cvm, found.pubkey, at, supports);
            }
        }
    }

    if !servers.is_empty() {
        let authors: Vec<PublicKey> = servers.iter().map(|s| s.pubkey).take(200).collect();
        let lists = ctx
            .cvm
            .relays
            .query(
                &relays,
                vec![Filter::new()
                    .kind(Kind::Custom(KIND_RELAY_LIST))
                    .authors(authors)],
                DISCOVER_TIMEOUT,
            )
            .await;
        apply_relay_lists(&ctx.cvm, &mut servers, lists, remember);
    }

    let out: Vec<Value> = servers
        .into_iter()
        .filter(|s| search.as_deref().is_none_or(|q| s.matches(q)))
        .take(limit.unwrap_or(usize::MAX))
        .map(|s| s.to_json())
        .collect();
    message.to_result().with_field("servers", out)
}

/// One discovered server, assembled from its announcements.
#[derive(Debug)]
struct Found {
    pubkey: PublicKey,
    newest: Timestamp,
    relays: Vec<String>,
    relay_list_at: Option<Timestamp>,
    name: Option<String>,
    description: Option<String>,
    capabilities: Vec<String>,
    priced: bool,
    /// The server announcement's time, and whether it offers CEP-4.
    encryption: Option<(Timestamp, bool)>,
}

impl Found {
    fn matches(&self, q: &str) -> bool {
        [self.name.as_deref(), self.description.as_deref()]
            .into_iter()
            .flatten()
            .any(|s| s.to_lowercase().contains(q))
            || self.pubkey.to_hex().contains(q)
    }

    fn to_json(&self) -> Value {
        let mut out = json!({ "pubkey": self.pubkey.to_hex() });
        if !self.relays.is_empty() {
            out["relays"] = json!(self.relays);
        }
        if let Some(name) = &self.name {
            out["name"] = json!(name);
        }
        if let Some(d) = &self.description {
            out["description"] = json!(d);
        }
        if !self.capabilities.is_empty() {
            out["capabilities"] = json!(self.capabilities);
        }
        // Advisory and one-sided: `true` on a positive advertised price,
        // omitted otherwise — Myco does not claim a server is free.
        if self.priced {
            out["paymentRequired"] = json!(true);
        }
        out
    }

    fn add_capability(&mut self, name: &str) {
        if !self.capabilities.iter().any(|c| c == name) {
            self.capabilities.push(name.to_string());
        }
    }
}

fn tag_value<'a>(event: &'a Event, name: &str) -> Option<&'a str> {
    event
        .tags
        .iter()
        .find(|t| t.kind().as_str() == name)
        .and_then(|t| t.content())
        .filter(|v| !v.is_empty())
}

/// A positive price in a CEP-8 `cap` tag: `["cap", "tool:x", "100", "sats"]`,
/// or a range `"0-100"` whose top is positive.
fn priced(event: &Event) -> bool {
    event.tags.iter().any(|t| {
        let v = t.as_slice();
        v.first().map(String::as_str) == Some("cap")
            && v.get(2).is_some_and(|price| {
                price
                    .split('-')
                    .filter_map(|p| p.trim().parse::<f64>().ok())
                    .any(|p| p > 0.0)
            })
    })
}

/// Servers from announcement events, newest announcement first. Only the
/// kinds asked for were read, so only they contribute.
fn servers_of(kinds: &[u16], events: Vec<Event>) -> Vec<Found> {
    let mut by_key: HashMap<PublicKey, Found> = HashMap::new();
    // Newest per (author, kind): replaceable events, and relays disagree.
    let mut newest: HashMap<(PublicKey, u16), Event> = HashMap::new();
    for event in events {
        let k = event.kind.as_u16();
        if !kinds.contains(&k) {
            continue;
        }
        let slot = newest
            .entry((event.pubkey, k))
            .or_insert_with(|| event.clone());
        if event.created_at > slot.created_at {
            *slot = event;
        }
    }
    for ((pubkey, kind), event) in newest {
        let found = by_key.entry(pubkey).or_insert_with(|| Found {
            pubkey,
            newest: Timestamp::from(0),
            relays: Vec::new(),
            relay_list_at: None,
            name: None,
            description: None,
            capabilities: Vec::new(),
            priced: false,
            encryption: None,
        });
        found.newest = found.newest.max(event.created_at);
        found.priced |= priced(&event);
        let content: Value = serde_json::from_str(&event.content).unwrap_or(Value::Null);
        match kind {
            KIND_SERVER => {
                let supports = event
                    .tags
                    .iter()
                    .any(|t| t.kind().as_str() == "support_encryption");
                found.encryption = Some((event.created_at, supports));
                found.name = tag_value(&event, "name")
                    .map(str::to_string)
                    .or_else(|| content["serverInfo"]["name"].as_str().map(str::to_string));
                found.description = tag_value(&event, "about")
                    .map(str::to_string)
                    .or_else(|| content["instructions"].as_str().map(str::to_string));
                if let Some(caps) = content["capabilities"].as_object() {
                    for name in caps.keys() {
                        found.add_capability(name);
                    }
                }
            }
            11317 => found.add_capability("tools"),
            11318 | 11319 => found.add_capability("resources"),
            11320 => found.add_capability("prompts"),
            _ => {}
        }
    }
    let mut out: Vec<Found> = by_key.into_values().collect();
    out.sort_by(|a, b| b.newest.cmp(&a.newest).then(a.pubkey.cmp(&b.pubkey)));
    out
}

/// CEP-17: where each server reads its requests (unmarked and `read`
/// relays), returned as its hints — a napplet passing the server back to
/// `request` publishes there. With `remember`, the whole list is kept too, so
/// that request also listens on the `write` relays without a lookup.
fn apply_relay_lists(cvm: &Cvm, servers: &mut [Found], lists: Vec<Event>, remember: bool) {
    for list in lists {
        let Some(found) = servers.iter_mut().find(|s| s.pubkey == list.pubkey) else {
            continue;
        };
        if found.relay_list_at.is_some_and(|at| at >= list.created_at) {
            continue;
        }
        let relays = parse_relay_list(&list);
        found.relay_list_at = Some(list.created_at);
        found.relays = relays.inbox.clone();
        if remember {
            // A request to this server soon after needs no lookup of its own.
            remember_relay_list(cvm, found.pubkey, Some((list.created_at, relays)));
        }
    }
}

/// `CvmRegistrySelector`: a 64-hex `schemaHash`, a nonempty `family`, or both.
fn valid_selector(value: Option<&Value>) -> bool {
    let Some(s) = value.and_then(Value::as_object) else {
        return false;
    };
    let hash = s.get("schemaHash");
    let family = s.get("family");
    if hash.is_none() && family.is_none() {
        return false;
    }
    let hash_ok = hash.is_none_or(|h| h.as_str().is_some_and(is_hash));
    let family_ok = family.is_none_or(|f| f.as_str().is_some_and(|f| !f.is_empty()));
    hash_ok && family_ok
}

fn is_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_registry_options(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::Object(o)) => o.get("server").is_none_or(|s| server_ref(Some(s)).is_ok()),
        Some(_) => false,
    }
}

fn registry_list(message: &Envelope) -> Envelope {
    let ok = match message.field("query") {
        None | Some(Value::Null) => true,
        Some(Value::Object(q)) => {
            q.get("schemaHash")
                .is_none_or(|h| h.as_str().is_some_and(is_hash))
                && q.get("family")
                    .is_none_or(|f| f.as_str().is_some_and(|f| !f.is_empty()))
                && q.get("limit")
                    .is_none_or(|l| l.as_u64().is_some_and(|n| n > 0))
        }
        Some(_) => false,
    };
    message.to_error(if ok {
        UNSUPPORTED_METHOD
    } else {
        INVALID_REQUEST
    })
}

fn registry_has(message: &Envelope) -> Envelope {
    if !valid_selector(message.field("selector"))
        || !valid_registry_options(message.field("options"))
    {
        return message.to_error(INVALID_REQUEST);
    }
    // No registry here: nothing satisfies any selector.
    message.to_result().with_field("has", false)
}

fn registry_describe(message: &Envelope) -> Envelope {
    if !valid_selector(message.field("selector"))
        || !valid_registry_options(message.field("options"))
    {
        return message.to_error(INVALID_REQUEST);
    }
    message.to_error(UNSUPPORTED_METHOD)
}

fn registry_call(ctx: &NapContext, message: &Envelope) -> Envelope {
    if !valid_selector(message.field("selector"))
        || !message
            .field("tool")
            .and_then(Value::as_str)
            .is_some_and(|t| !t.is_empty())
        || message
            .field("args")
            .is_some_and(|a| !a.is_object() && !a.is_null())
        || !valid_registry_options(message.field("options"))
    {
        return message.to_error(INVALID_REQUEST);
    }
    if let Err(e) = options_of(message.field("options"), ctx.cvm.max_timeout_ms) {
        return message.to_error(e);
    }
    message.to_error(UNSUPPORTED_METHOD)
}

#[cfg(test)]
mod tests;
