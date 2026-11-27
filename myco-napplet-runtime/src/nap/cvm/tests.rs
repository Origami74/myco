//! NAP-CVM against an in-memory relay with a scripted ContextVM server: the
//! spec's conformance cases, as far as they reach the runtime.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use nostr::filter::MatchEventOptions;
use nostr::nips::nip44;
use nostr::{Event, EventBuilder, Filter, JsonUtil, Keys, Kind, PublicKey, Tag};
use serde_json::{json, Value};

use super::*;
use crate::dispatch::{dispatch, NapContext, Outcome};
use crate::seams::{CvmListen, CvmRelays, Envelope};
use crate::session::{NappletIdentity, Session, Streamer};
use crate::testing::test_context;

const RELAY: &str = "wss://relay.example.com";

type Listener = (Vec<Filter>, Arc<dyn Fn(Event) + Send + Sync>, Arc<()>);

/// One relay and the ContextVM servers behind it. What a server answers is
/// up to the test: it reads what was published and calls `reply`.
#[derive(Default)]
struct Net {
    listeners: Mutex<Vec<Listener>>,
    /// Every event published, as sent (wrapped or not).
    published: Mutex<Vec<Event>>,
    /// Stored events, for `query`.
    stored: Mutex<Vec<Event>>,
    /// Answer `initialize` (and nothing else) automatically.
    auto_init: Mutex<Option<(Keys, bool)>>,
    offline: Mutex<bool>,
    /// The relays each call was made on: query, publish, listen.
    queried_on: Mutex<Vec<Vec<String>>>,
    published_on: Mutex<Vec<Vec<String>>>,
    listened_on: Mutex<Vec<Vec<String>>>,
    /// Relays discovery adds to the default (the user's own).
    user_relays: Mutex<Vec<String>>,
    /// No relay takes a message with this method (to the auto-initializing
    /// server): published nowhere.
    refuse_method: Mutex<Option<String>>,
}

impl Net {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// `server` answers every `initialize` on its own, advertising encryption
    /// when `encryption` is set.
    fn auto_initialize(&self, server: &Keys, encryption: bool) {
        *self.auto_init.lock().unwrap() = Some((server.clone(), encryption));
    }

    /// The inner ContextVM messages sent to `server`, unwrapped, in order.
    fn sent_to(&self, server: &Keys) -> Vec<(Event, Value)> {
        self.published
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| open_for(server, e))
            .map(|inner| {
                let m = serde_json::from_str(&inner.content).unwrap();
                (inner, m)
            })
            .collect()
    }

    /// Send `message` from `server` to the client behind `request`,
    /// `e`-tagging it unless `correlated` is false.
    fn reply(&self, server: &Keys, request: &Event, message: Value, extra: Vec<Tag>) {
        let mut tags = vec![Tag::public_key(request.pubkey), Tag::event(request.id)];
        tags.extend(extra);
        let event = EventBuilder::new(Kind::Custom(KIND_MESSAGE), message.to_string())
            .tags(tags)
            .sign_with_keys(server)
            .unwrap();
        self.deliver(event);
    }

    /// Send `message` from `server` to `client`, about no request.
    fn push(&self, server: &Keys, client: PublicKey, message: Value) {
        let event = EventBuilder::new(Kind::Custom(KIND_MESSAGE), message.to_string())
            .tags([Tag::public_key(client)])
            .sign_with_keys(server)
            .unwrap();
        self.deliver(event);
    }

    fn deliver(&self, event: Event) {
        let listeners: Vec<_> = self
            .listeners
            .lock()
            .unwrap()
            .iter()
            .filter(|(filters, _, _)| {
                filters
                    .iter()
                    .any(|f| f.match_event(&event, MatchEventOptions::default()))
            })
            .map(|(_, deliver, _)| deliver.clone())
            .collect();
        for deliver in listeners {
            deliver(event.clone());
        }
    }

    fn live_listeners(&self) -> usize {
        self.listeners
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, _, alive)| Arc::strong_count(alive) > 1)
            .count()
    }
}

/// The inner event of `event` if it is addressed to `server`.
fn open_for(server: &Keys, event: &Event) -> Option<Event> {
    let addressed = event
        .tags
        .iter()
        .any(|t| t.kind().as_str() == "p" && t.content() == Some(&server.public_key().to_hex()));
    if !addressed {
        return None;
    }
    match event.kind.as_u16() {
        KIND_MESSAGE => Some(event.clone()),
        KIND_GIFT_WRAP => {
            let plain = nip44::decrypt(server.secret_key(), &event.pubkey, &event.content).ok()?;
            Event::from_json(plain).ok()
        }
        _ => None,
    }
}

#[async_trait]
impl CvmRelays for Net {
    fn available(&self) -> bool {
        !*self.offline.lock().unwrap()
    }
    fn default_relays(&self) -> Vec<String> {
        vec![RELAY.to_string()]
    }
    async fn discovery_relays(&self) -> Vec<String> {
        let mut out = self.default_relays();
        out.extend(self.user_relays.lock().unwrap().iter().cloned());
        out
    }
    async fn query(&self, relays: &[String], filters: Vec<Filter>, _: Duration) -> Vec<Event> {
        self.queried_on.lock().unwrap().push(relays.to_vec());
        self.stored
            .lock()
            .unwrap()
            .iter()
            .filter(|e| {
                filters
                    .iter()
                    .any(|f| f.match_event(e, MatchEventOptions::default()))
            })
            .cloned()
            .collect()
    }
    async fn publish(&self, relays: &[String], event: &Event, _: Duration) -> bool {
        let auto = self.auto_init.lock().unwrap().clone();
        let refused = self.refuse_method.lock().unwrap().clone();
        if let (Some((server, _)), Some(refused)) = (&auto, refused) {
            if let Some(inner) = open_for(server, event) {
                let m: Value = serde_json::from_str(&inner.content).unwrap();
                if m["method"] == refused.as_str() {
                    return false;
                }
            }
        }
        self.published_on.lock().unwrap().push(relays.to_vec());
        self.published.lock().unwrap().push(event.clone());
        if let Some((server, encryption)) = auto {
            if let Some(inner) = open_for(&server, event) {
                let m: Value = serde_json::from_str(&inner.content).unwrap();
                if m["method"] == "initialize" {
                    let extra = if encryption {
                        vec![Tag::parse(["support_encryption"]).unwrap()]
                    } else {
                        vec![]
                    };
                    self.reply(
                        &server,
                        &inner,
                        json!({"jsonrpc": "2.0", "id": m["id"], "result": {
                            "protocolVersion": "2025-11-25",
                            "capabilities": {"tools": {}},
                            "serverInfo": {"name": "test", "version": "1"}
                        }}),
                        extra,
                    );
                }
            }
        }
        true
    }
    async fn listen(
        &self,
        relays: &[String],
        filters: Vec<Filter>,
        deliver: Arc<dyn Fn(Event) + Send + Sync>,
    ) -> CvmListen {
        self.listened_on.lock().unwrap().push(relays.to_vec());
        let alive = Arc::new(());
        self.listeners
            .lock()
            .unwrap()
            .push((filters, deliver, alive.clone()));
        CvmListen::new(alive)
    }
}

/// A context whose ContextVM bridge reaches `net`.
fn ctx_on(net: &Arc<Net>, cvm: Cvm) -> NapContext {
    let (mut ctx, _signer) = test_context();
    let _ = net;
    ctx.cvm = Arc::new(cvm);
    ctx
}

fn bridge(net: &Arc<Net>) -> NapContext {
    ctx_on(net, Cvm::new(net.clone()))
}

/// An established window granted `cvm`, and what was pushed to it.
fn window() -> (Session, Arc<Mutex<Vec<Envelope>>>) {
    let pushed = Arc::new(Mutex::new(Vec::new()));
    let sink = pushed.clone();
    let mut s =
        Session::new(NappletIdentity::new("tools", "hash"), ["cvm"]).with_streamer(Streamer {
            push: Arc::new(move |e| sink.lock().unwrap().push(e)),
            spawn: Arc::new(|work| {
                tokio::spawn(work);
            }),
        });
    s.on_ready();
    (s, pushed)
}

fn server_json(server: &Keys) -> Value {
    json!({ "pubkey": server.public_key().to_hex() })
}

fn req(id: &str, server: &Keys, message: Value, options: Option<Value>) -> Envelope {
    let mut e = Envelope::new("cvm.request")
        .with_id(id)
        .with_field("server", server_json(server))
        .with_field("message", message);
    if let Some(o) = options {
        e = e.with_field("options", o);
    }
    e
}

async fn call(ctx: &NapContext, s: &mut Session, e: &Envelope) -> Value {
    match dispatch(ctx, s, e).await {
        Outcome::Reply(r) => serde_json::to_value(&r[0]).unwrap(),
        other => panic!("no reply: {other:?}"),
    }
}

/// Run a call in the background, the way a host runs one against a snapshot.
fn spawn_call(ctx: &NapContext, s: &Session, e: Envelope) -> tokio::task::JoinHandle<Value> {
    let ctx = ctx.clone();
    let mut s = s.clone();
    tokio::spawn(async move { call(&ctx, &mut s, &e).await })
}

/// Wait until `net` has seen `n` messages to `server`.
async fn sent(net: &Net, server: &Keys, n: usize) -> Vec<(Event, Value)> {
    for _ in 0..200 {
        let out = net.sent_to(server);
        if out.len() >= n {
            return out;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!(
        "server saw {} messages, wanted {n}",
        net.sent_to(server).len()
    );
}

fn tools_call(id: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
           "params": {"name": "echo", "arguments": {"x": 1}}})
}

#[tokio::test]
async fn a_tool_call_initializes_once_and_keeps_the_napplets_id() {
    let net = Net::new();
    let server = Keys::generate();
    net.auto_initialize(&server, false);
    let ctx = bridge(&net);
    let (mut s, _) = window();

    let pending = spawn_call(&ctx, &s, req("r1", &server, tools_call(json!(2)), None));
    let msgs = sent(&net, &server, 3).await;
    assert_eq!(msgs[0].1["method"], "initialize");
    assert_eq!(msgs[1].1["method"], "notifications/initialized");
    assert_eq!(msgs[2].1["method"], "tools/call");
    assert_eq!(msgs[2].1["id"], 2);
    // CEP-8's mode request rides the first message only.
    let has_mode = |e: &Event| {
        e.tags
            .iter()
            .any(|t| t.kind().as_str() == "payment_interaction")
    };
    assert!(has_mode(&msgs[0].0));
    assert!(!has_mode(&msgs[2].0));
    // A fresh transport key, not anyone's identity.
    let client = msgs[2].0.pubkey;
    assert!(msgs.iter().all(|(e, _)| e.pubkey == client));

    net.reply(
        &server,
        &msgs[2].0,
        json!({"jsonrpc": "2.0", "id": 2, "result": {"content": [], "isError": false}}),
        vec![],
    );
    let r = pending.await.unwrap();
    assert_eq!(r["type"], "cvm.request.result");
    assert_eq!(r["id"], "r1");
    assert_eq!(r["message"]["id"], 2);
    assert_eq!(r["message"]["result"]["isError"], false);
    assert!(r.get("error").is_none());

    // The second call goes straight out: the session is initialized.
    let pending = spawn_call(&ctx, &s, req("r2", &server, tools_call(json!(3)), None));
    let msgs = sent(&net, &server, 4).await;
    assert_eq!(msgs[3].1["method"], "tools/call");
    net.reply(
        &server,
        &msgs[3].0,
        json!({"jsonrpc": "2.0", "id": 3, "result": {}}),
        vec![],
    );
    assert_eq!(pending.await.unwrap()["message"]["id"], 3);

    // Reusing a request id in the session is refused before publication.
    let r = call(
        &ctx,
        &mut s,
        &req("r3", &server, tools_call(json!(2)), None),
    )
    .await;
    assert_eq!(r["error"], INVALID_MESSAGE);
    assert_eq!(net.sent_to(&server).len(), 4);
}

#[tokio::test]
async fn malformed_messages_are_refused_without_publication() {
    let net = Net::new();
    let server = Keys::generate();
    let ctx = bridge(&net);
    let (mut s, _) = window();
    for bad in [
        Value::Null,
        json!([tools_call(json!(1))]),
        json!({"jsonrpc": "2.0", "id": null, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 1, "result": {}, "error": {"code": 1, "message": "x"}}),
        json!({"jsonrpc": "1.0", "id": 1, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 1.5, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": [1]}),
    ] {
        let mut e = Envelope::new("cvm.request")
            .with_id("m")
            .with_field("server", server_json(&server));
        if !bad.is_null() {
            e = e.with_field("message", bad.clone());
        }
        let r = call(&ctx, &mut s, &e).await;
        assert_eq!(r["error"], INVALID_MESSAGE, "{bad}");
        assert!(r.get("message").is_none());
    }
    // A bad server reference is a request error, not a message error.
    let r = call(
        &ctx,
        &mut s,
        &Envelope::new("cvm.request")
            .with_id("m")
            .with_field("server", json!({"pubkey": "ABC"}))
            .with_field("message", tools_call(json!(1))),
    )
    .await;
    assert_eq!(r["error"], INVALID_REQUEST);
    assert!(net.published.lock().unwrap().is_empty());
}

#[tokio::test]
async fn timeouts_are_bounded_and_never_clamped() {
    let net = Net::new();
    let server = Keys::generate();
    let ctx = ctx_on(&net, Cvm::new(net.clone()).with_max_timeout_ms(60_000));
    let (mut s, _) = window();
    for (ms, want) in [
        (json!(0), INVALID_TIMEOUT),
        (json!(300_001), INVALID_TIMEOUT),
        (json!(1.5), INVALID_TIMEOUT),
        (json!(120_000), TIMEOUT_OUT_OF_POLICY),
    ] {
        let r = call(
            &ctx,
            &mut s,
            &req(
                "t",
                &server,
                tools_call(json!(1)),
                Some(json!({"timeoutMs": ms})),
            ),
        )
        .await;
        assert_eq!(r["error"], want, "{ms}");
    }
    assert!(net.published.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_expired_call_answers_once_and_its_late_reply_goes_nowhere() {
    let net = Net::new();
    let server = Keys::generate();
    let ctx = bridge(&net);
    let (mut s, pushed) = window();
    let r = call(
        &ctx,
        &mut s,
        &req(
            "slow",
            &server,
            tools_call(json!(1)),
            Some(json!({"timeoutMs": 1000, "initialize": false})),
        ),
    )
    .await;
    assert_eq!(r["error"], RELAY_TIMEOUT);
    let msgs = net.sent_to(&server);
    net.reply(
        &server,
        &msgs[0].0,
        json!({"jsonrpc": "2.0", "id": 1, "result": {}}),
        vec![],
    );
    net.reply(
        &server,
        &msgs[0].0,
        json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progress": 1}}),
        vec![],
    );
    assert!(pushed.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_long_timeout_is_honoured() {
    let net = Net::new();
    let server = Keys::generate();
    let ctx = bridge(&net);
    let (s, _) = window();
    let pending = spawn_call(
        &ctx,
        &s,
        req(
            "long",
            &server,
            tools_call(json!(1)),
            Some(json!({"timeoutMs": 120_000, "initialize": false})),
        ),
    );
    let msgs = sent(&net, &server, 1).await;
    tokio::time::sleep(Duration::from_secs(45)).await;
    net.reply(
        &server,
        &msgs[0].0,
        json!({"jsonrpc": "2.0", "id": 1, "result": {}}),
        vec![],
    );
    assert!(pending.await.unwrap().get("message").is_some());
}

#[tokio::test]
async fn notifications_and_responses_are_acknowledged_without_a_message() {
    let net = Net::new();
    let server = Keys::generate();
    let ctx = bridge(&net);
    let (mut s, pushed) = window();

    let r = call(
        &ctx,
        &mut s,
        &req(
            "n1",
            &server,
            json!({"jsonrpc": "2.0", "method": "notifications/roots/list_changed"}),
            None,
        ),
    )
    .await;
    assert_eq!(r, json!({"type": "cvm.request.result", "id": "n1"}));
    let msgs = net.sent_to(&server);
    assert_eq!(
        msgs.len(),
        1,
        "a notification initializes nothing by default"
    );
    assert!(
        msgs[0].1.get("id").is_none(),
        "no id added to a notification"
    );

    // The server asks; only this window hears it, and may answer.
    let client = msgs[0].0.pubkey;
    net.push(
        &server,
        client,
        json!({"jsonrpc": "2.0", "id": "srv-7", "method": "ping"}),
    );
    let events = pushed.lock().unwrap().clone();
    assert_eq!(events.len(), 1);
    let ev = serde_json::to_value(&events[0]).unwrap();
    assert_eq!(ev["type"], "cvm.event");
    assert_eq!(ev["server"]["pubkey"], server.public_key().to_hex());
    assert_eq!(ev["message"]["id"], "srv-7");

    let answer = json!({"jsonrpc": "2.0", "id": "srv-7", "result": {}});
    let r = call(&ctx, &mut s, &req("a1", &server, answer.clone(), None)).await;
    assert_eq!(r, json!({"type": "cvm.request.result", "id": "a1"}));
    assert_eq!(net.sent_to(&server).last().unwrap().1["id"], "srv-7");
    // Answered once; a second answer matches nothing.
    let r = call(&ctx, &mut s, &req("a2", &server, answer, None)).await;
    assert_eq!(r["error"], INVALID_MESSAGE);

    // Cancelling a request that is not this napplet's is refused.
    let r = call(
        &ctx,
        &mut s,
        &req(
            "c1",
            &server,
            json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 9}}),
            None,
        ),
    )
    .await;
    assert_eq!(r["error"], INVALID_MESSAGE);
}

#[tokio::test]
async fn windows_are_isolated_and_close_ends_only_the_callers_work() {
    let net = Net::new();
    let server = Keys::generate();
    let ctx = bridge(&net);
    let (mut a, a_pushed) = window();
    let (b, b_pushed) = window();
    let opts = Some(json!({"initialize": false}));

    let a_call = spawn_call(
        &ctx,
        &a,
        req("a", &server, tools_call(json!(1)), opts.clone()),
    );
    sent(&net, &server, 1).await;
    let b_call = spawn_call(&ctx, &b, req("b", &server, tools_call(json!(1)), opts));
    let msgs = sent(&net, &server, 2).await;
    let (a_key, b_key) = (msgs[0].0.pubkey, msgs[1].0.pubkey);
    assert_ne!(a_key, b_key, "each window has its own transport key");

    // A server request to B reaches B alone.
    net.push(
        &server,
        b_key,
        json!({"jsonrpc": "2.0", "id": 5, "method": "ping"}),
    );
    assert!(a_pushed.lock().unwrap().is_empty());
    assert_eq!(b_pushed.lock().unwrap().len(), 1);

    let r = call(
        &ctx,
        &mut a,
        &Envelope::new("cvm.close")
            .with_id("x")
            .with_field("server", server_json(&server)),
    )
    .await;
    assert_eq!(r, json!({"type": "cvm.close.result", "id": "x"}));
    assert_eq!(a_call.await.unwrap()["error"], SESSION_CLOSED);

    net.reply(
        &server,
        &msgs[1].0,
        json!({"jsonrpc": "2.0", "id": 1, "result": {}}),
        vec![],
    );
    assert_eq!(b_call.await.unwrap()["message"]["id"], 1);

    // Closing again succeeds.
    let r = call(
        &ctx,
        &mut a,
        &Envelope::new("cvm.close")
            .with_id("y")
            .with_field("server", server_json(&server)),
    )
    .await;
    assert!(r.get("error").is_none());
}

#[tokio::test]
async fn closing_the_window_answers_its_calls_and_stops_listening() {
    let net = Net::new();
    let server = Keys::generate();
    let ctx = bridge(&net);
    let (s, _) = window();
    let pending = spawn_call(
        &ctx,
        &s,
        req(
            "w",
            &server,
            tools_call(json!(1)),
            Some(json!({"initialize": false})),
        ),
    );
    sent(&net, &server, 1).await;
    assert_eq!(net.live_listeners(), 1);
    // The host ends the window explicitly: the running call holds a
    // snapshot of the session, which keeps its work scope alive.
    ctx.cvm.end_window(s.work().id().unwrap());
    drop(s);
    let r = tokio::time::timeout(Duration::from_secs(1), pending)
        .await
        .expect("the call was answered at once");
    assert_eq!(r.unwrap()["error"], SESSION_CLOSED);
    assert_eq!(net.live_listeners(), 0);
}

/// Without the host's call, the last clone of the session going is the
/// window closing: its scope's end does the same.
#[tokio::test]
async fn the_sessions_scope_ending_ends_the_window() {
    let net = Net::new();
    let server = Keys::generate();
    let ctx = bridge(&net);
    let (mut s, _) = window();
    let r = call(
        &ctx,
        &mut s,
        &req(
            "n",
            &server,
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            None,
        ),
    )
    .await;
    assert!(r.get("error").is_none());
    assert_eq!(net.live_listeners(), 1);
    drop(s);
    assert_eq!(net.live_listeners(), 0);
}

#[tokio::test]
async fn payment_gates_end_the_call_by_the_napplets_option() {
    let net = Net::new();
    let server = Keys::generate();
    let ctx = bridge(&net);
    let (s, _) = window();
    for (n, payment, want) in [
        (1, "prompt", PAYMENT_REQUIRED),
        (2, "deny", PAYMENT_DENIED),
        (3, "allow", PAYMENT_DENIED),
    ] {
        let pending = spawn_call(
            &ctx,
            &s,
            req(
                &format!("p{n}"),
                &server,
                tools_call(json!(n)),
                Some(json!({"initialize": false, "payment": payment})),
            ),
        );
        let msgs = sent(&net, &server, n).await;
        net.reply(
            &server,
            &msgs[n - 1].0,
            json!({"jsonrpc": "2.0", "id": n, "error": {"code": -32042, "message": "Payment Required",
                   "data": {"payment_options": [{"amount": 10, "pay_req": "lnbc1", "pmi": "bitcoin-lightning-bolt11"}]}}}),
            vec![],
        );
        assert_eq!(pending.await.unwrap()["error"], want, "{payment}");
    }
    // Transparent mode's notification ends it the same way.
    let pending = spawn_call(
        &ctx,
        &s,
        req(
            "p4",
            &server,
            tools_call(json!(4)),
            Some(json!({"initialize": false})),
        ),
    );
    let msgs = sent(&net, &server, 4).await;
    net.reply(
        &server,
        &msgs[3].0,
        json!({"jsonrpc": "2.0", "method": "notifications/payment_required",
               "params": {"amount": 10, "pay_req": "lnbc1", "pmi": "bitcoin-lightning-bolt11"}}),
        vec![],
    );
    assert_eq!(pending.await.unwrap()["error"], PAYMENT_REQUIRED);
}

#[tokio::test]
async fn a_server_that_supports_encryption_gets_gift_wraps() {
    let net = Net::new();
    let server = Keys::generate();
    net.auto_initialize(&server, true);
    let ctx = bridge(&net);
    let (s, _) = window();
    let pending = spawn_call(&ctx, &s, req("e", &server, tools_call(json!(1)), None));
    let msgs = sent(&net, &server, 3).await;
    let published = net.published.lock().unwrap().clone();
    assert_eq!(
        published[0].kind.as_u16(),
        KIND_MESSAGE,
        "first contact is plain"
    );
    assert_eq!(published[2].kind.as_u16(), KIND_GIFT_WRAP, "then wrapped");
    assert_ne!(
        published[2].pubkey, msgs[2].0.pubkey,
        "the wrap hides the sender"
    );

    // A gift-wrapped answer is accepted too.
    let inner = EventBuilder::new(
        Kind::Custom(KIND_MESSAGE),
        json!({"jsonrpc": "2.0", "id": 1, "result": {"ok": true}}).to_string(),
    )
    .tags([Tag::public_key(msgs[2].0.pubkey), Tag::event(msgs[2].0.id)])
    .sign_with_keys(&server)
    .unwrap();
    let ephemeral = Keys::generate();
    let wrap = EventBuilder::new(
        Kind::Custom(KIND_GIFT_WRAP),
        nip44::encrypt(
            ephemeral.secret_key(),
            &msgs[2].0.pubkey,
            inner.as_json(),
            nip44::Version::V2,
        )
        .unwrap(),
    )
    .tags([Tag::public_key(msgs[2].0.pubkey)])
    .sign_with_keys(&ephemeral)
    .unwrap();
    net.deliver(wrap);
    assert_eq!(pending.await.unwrap()["message"]["result"]["ok"], true);
}

#[tokio::test]
async fn an_answer_signed_by_anyone_else_is_ignored() {
    let net = Net::new();
    let server = Keys::generate();
    let impostor = Keys::generate();
    let ctx = bridge(&net);
    let (s, _) = window();
    let pending = spawn_call(
        &ctx,
        &s,
        req(
            "i",
            &server,
            tools_call(json!(1)),
            Some(json!({"initialize": false, "timeoutMs": 200})),
        ),
    );
    let msgs = sent(&net, &server, 1).await;
    net.reply(
        &impostor,
        &msgs[0].0,
        json!({"jsonrpc": "2.0", "id": 1, "result": {}}),
        vec![],
    );
    assert_eq!(pending.await.unwrap()["error"], RELAY_TIMEOUT);
}

#[tokio::test]
async fn offline_is_refused_before_anything_is_sent() {
    let net = Net::new();
    *net.offline.lock().unwrap() = true;
    let server = Keys::generate();
    let ctx = bridge(&net);
    let (mut s, _) = window();
    let r = call(&ctx, &mut s, &req("o", &server, tools_call(json!(1)), None)).await;
    assert_eq!(r["error"], SERVER_NOT_FOUND);
    assert!(net.published.lock().unwrap().is_empty());
}

fn announcement(keys: &Keys, kind: u16, content: Value, tags: Vec<Tag>) -> Event {
    EventBuilder::new(Kind::Custom(kind), content.to_string())
        .tags(tags)
        .sign_with_keys(keys)
        .unwrap()
}

#[tokio::test]
async fn discovery_reads_announcements_kinds_and_relay_lists() {
    let net = Net::new();
    let relayvm = Keys::generate();
    let weather = Keys::generate();
    net.stored.lock().unwrap().extend([
        announcement(
            &relayvm,
            11316,
            json!({"protocolVersion": "2025-11-25", "capabilities": {"tools": {}, "resources": {}},
                   "serverInfo": {"name": "relayvm"}}),
            vec![
                Tag::parse(["name", "RelayVM"]).unwrap(),
                Tag::parse(["about", "Relay intelligence"]).unwrap(),
            ],
        ),
        announcement(
            &weather,
            11317,
            json!({"tools": []}),
            vec![Tag::parse(["cap", "tool:get_weather", "100", "sats"]).unwrap()],
        ),
        announcement(
            &relayvm,
            10002,
            json!(""),
            vec![
                Tag::parse(["r", "wss://in.example.com"]).unwrap(),
                Tag::parse(["r", "wss://out.example.com", "write"]).unwrap(),
                Tag::parse(["r", "ws://127.0.0.1:4870"]).unwrap(),
            ],
        ),
    ]);
    let ctx = bridge(&net);
    let (mut s, _) = window();

    let r = call(&ctx, &mut s, &Envelope::new("cvm.discover").with_id("d")).await;
    let servers = r["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 2);
    let rv = servers
        .iter()
        .find(|x| x["pubkey"] == relayvm.public_key().to_hex())
        .unwrap();
    assert_eq!(rv["name"], "RelayVM");
    assert_eq!(rv["description"], "Relay intelligence");
    assert_eq!(rv["relays"], json!(["wss://in.example.com"]));
    assert_eq!(rv["capabilities"], json!(["resources", "tools"]));
    assert!(
        rv.get("paymentRequired").is_none(),
        "unknown pricing is omitted"
    );
    let w = servers
        .iter()
        .find(|x| x["pubkey"] == weather.public_key().to_hex())
        .unwrap();
    assert_eq!(w["capabilities"], json!(["tools"]));
    assert_eq!(w["paymentRequired"], true);

    let discover = |q: Value| {
        Envelope::new("cvm.discover")
            .with_id("d")
            .with_field("query", q)
    };
    let r = call(&ctx, &mut s, &discover(json!({"kinds": [11316]}))).await;
    assert_eq!(r["servers"].as_array().unwrap().len(), 1);
    let r = call(&ctx, &mut s, &discover(json!({"kinds": []}))).await;
    assert_eq!(r["servers"], json!([]));
    let r = call(&ctx, &mut s, &discover(json!({"kinds": [1]}))).await;
    assert_eq!(r["error"], UNSUPPORTED_KIND);
    let r = call(&ctx, &mut s, &discover(json!({"search": "relay"}))).await;
    assert_eq!(r["servers"].as_array().unwrap().len(), 1);
    let r = call(&ctx, &mut s, &discover(json!({"limit": 1}))).await;
    assert_eq!(r["servers"].as_array().unwrap().len(), 1);
    let r = call(&ctx, &mut s, &discover(json!({"limit": 0}))).await;
    assert_eq!(r["error"], INVALID_REQUEST);
}

#[tokio::test]
async fn the_registry_is_absent_but_still_validates() {
    let net = Net::new();
    let ctx = bridge(&net);
    let (mut s, _) = window();
    let hash = "a".repeat(64);
    let r = call(
        &ctx,
        &mut s,
        &Envelope::new("cvm.registry.list").with_id("l"),
    )
    .await;
    assert_eq!(r["error"], UNSUPPORTED_METHOD);
    let r = call(
        &ctx,
        &mut s,
        &Envelope::new("cvm.registry.has")
            .with_id("h")
            .with_field("selector", json!({"family": "relatr"})),
    )
    .await;
    assert_eq!(
        r,
        json!({"type": "cvm.registry.has.result", "id": "h", "has": false})
    );
    let r = call(
        &ctx,
        &mut s,
        &Envelope::new("cvm.registry.describe")
            .with_id("s")
            .with_field("selector", json!({"schemaHash": hash})),
    )
    .await;
    assert_eq!(r["error"], UNSUPPORTED_METHOD);
    for bad in [
        json!({}),
        json!({"schemaHash": "ABC"}),
        json!({"family": ""}),
    ] {
        let r = call(
            &ctx,
            &mut s,
            &Envelope::new("cvm.registry.has")
                .with_id("h")
                .with_field("selector", bad.clone()),
        )
        .await;
        assert_eq!(r["error"], INVALID_REQUEST, "{bad}");
    }
    let r = call(
        &ctx,
        &mut s,
        &Envelope::new("cvm.registry.call")
            .with_id("c")
            .with_field("selector", json!({"schemaHash": hash}))
            .with_field("tool", "search")
            .with_field("options", json!({"timeoutMs": 0})),
    )
    .await;
    assert_eq!(r["error"], INVALID_TIMEOUT);
}

#[tokio::test]
async fn unknown_actions_are_silent_and_the_domain_is_gated() {
    let net = Net::new();
    let ctx = bridge(&net);
    let (mut s, _) = window();
    assert_eq!(
        dispatch(&ctx, &mut s, &Envelope::new("cvm.frobnicate").with_id("f")).await,
        Outcome::Ignore
    );
    let mut denied = Session::new(NappletIdentity::new("tools", "hash"), ["relay"]);
    denied.on_ready();
    let r = call(
        &ctx,
        &mut denied,
        &Envelope::new("cvm.discover").with_id("d"),
    )
    .await;
    assert_eq!(r["type"], "cvm.discover.result");
    assert!(r["error"].as_str().unwrap().contains("not granted"));
}

fn relay_list(keys: &Keys, tags: &[&[&str]]) -> Event {
    announcement(
        keys,
        10002,
        json!(""),
        tags.iter()
            .map(|t| Tag::parse(t.to_vec()).unwrap())
            .collect(),
    )
}

fn notify() -> Value {
    json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
}

/// A server named without relays is reached where its CEP-17 list says:
/// published to its inbox relays, answered on its outbox ones. The list is
/// looked up once.
#[tokio::test]
async fn a_server_without_hints_is_reached_through_its_relay_list() {
    let net = Net::new();
    let server = Keys::generate();
    net.stored.lock().unwrap().push(relay_list(
        &server,
        &[
            &["r", "wss://both.example.com"],
            &["r", "wss://in.example.com", "read"],
            &["r", "wss://out.example.com", "write"],
            &["r", "ws://192.168.1.2"],
        ],
    ));
    net.user_relays
        .lock()
        .unwrap()
        .push("wss://user.example.com".into());
    let ctx = bridge(&net);
    let (mut s, _) = window();
    let r = call(&ctx, &mut s, &req("n", &server, notify(), None)).await;
    assert!(r.get("error").is_none(), "{r}");

    // Looked up on the discovery relays: the default and the user's.
    assert_eq!(
        net.queried_on.lock().unwrap()[0],
        vec![RELAY.to_string(), "wss://user.example.com".to_string()]
    );
    assert_eq!(
        net.published_on.lock().unwrap()[0],
        vec!["wss://both.example.com", "wss://in.example.com"]
    );
    assert_eq!(
        net.listened_on.lock().unwrap()[0],
        vec![
            "wss://both.example.com",
            "wss://in.example.com",
            "wss://out.example.com"
        ]
    );

    // Another window, same server: the list is remembered.
    let (mut other, _) = window();
    call(&ctx, &mut other, &req("n", &server, notify(), None)).await;
    assert_eq!(net.queried_on.lock().unwrap().len(), 1);
    assert_eq!(
        net.published_on.lock().unwrap()[1],
        vec!["wss://both.example.com", "wss://in.example.com"]
    );
}

/// Relays the napplet names are used as given, with no lookup.
#[tokio::test]
async fn relays_the_napplet_names_need_no_lookup() {
    let net = Net::new();
    let server = Keys::generate();
    let ctx = bridge(&net);
    let (mut s, _) = window();
    let e = Envelope::new("cvm.request")
        .with_id("h")
        .with_field(
            "server",
            json!({"pubkey": server.public_key().to_hex(),
                   "relays": ["wss://hint.example.com", "ws://localhost:4870"]}),
        )
        .with_field("message", notify());
    let r = call(&ctx, &mut s, &e).await;
    assert!(r.get("error").is_none());
    assert!(net.queried_on.lock().unwrap().is_empty());
    assert_eq!(
        net.published_on.lock().unwrap()[0],
        vec!["wss://hint.example.com"]
    );
    assert_eq!(
        net.listened_on.lock().unwrap()[0],
        vec!["wss://hint.example.com"]
    );
}

/// No relay list: the defaults. Remembered as "none" too.
#[tokio::test]
async fn a_server_without_a_relay_list_is_reached_on_the_defaults() {
    let net = Net::new();
    let server = Keys::generate();
    let ctx = bridge(&net);
    let (mut s, _) = window();
    call(&ctx, &mut s, &req("n", &server, notify(), None)).await;
    assert_eq!(net.published_on.lock().unwrap()[0], vec![RELAY]);
    assert_eq!(net.listened_on.lock().unwrap()[0], vec![RELAY]);
    let (mut other, _) = window();
    call(&ctx, &mut other, &req("n", &server, notify(), None)).await;
    assert_eq!(net.queried_on.lock().unwrap().len(), 1);
}

/// Discovery reads the user's relays too, and what it learns of a server's
/// relays saves the request a lookup.
#[tokio::test]
async fn discovery_uses_the_users_relays_and_primes_the_lookup() {
    let net = Net::new();
    let server = Keys::generate();
    net.user_relays
        .lock()
        .unwrap()
        .push("wss://user.example.com".into());
    net.stored.lock().unwrap().extend([
        announcement(
            &server,
            11316,
            json!({"capabilities": {"tools": {}}}),
            vec![],
        ),
        relay_list(
            &server,
            &[
                &["r", "wss://in.example.com", "read"],
                &["r", "wss://out.example.com", "write"],
            ],
        ),
    ]);
    let ctx = bridge(&net);
    let (mut s, _) = window();
    let r = call(&ctx, &mut s, &Envelope::new("cvm.discover").with_id("d")).await;
    assert_eq!(r["servers"][0]["relays"], json!(["wss://in.example.com"]));
    assert!(net
        .queried_on
        .lock()
        .unwrap()
        .iter()
        .all(|relays| relays.contains(&"wss://user.example.com".to_string())));
    let queries = net.queried_on.lock().unwrap().len();

    call(&ctx, &mut s, &req("n", &server, notify(), None)).await;
    assert_eq!(
        net.queried_on.lock().unwrap().len(),
        queries,
        "no second lookup"
    );
    assert_eq!(
        net.listened_on.lock().unwrap()[0],
        vec!["wss://in.example.com", "wss://out.example.com"]
    );
}

/// A request from the server, carried by an event the test keeps.
fn server_request(server: &Keys, client: PublicKey, id: &str) -> Event {
    EventBuilder::new(
        Kind::Custom(KIND_MESSAGE),
        json!({"jsonrpc": "2.0", "id": id, "method": "ping"}).to_string(),
    )
    .tags([Tag::public_key(client)])
    .sign_with_keys(server)
    .unwrap()
}

/// The napplet's answer to a server request `e`-tags the event that carried
/// the request: that is how the server matches it.
#[tokio::test]
async fn an_answer_to_a_server_request_tags_its_event() {
    let net = Net::new();
    let server = Keys::generate();
    let ctx = bridge(&net);
    let (mut s, _) = window();
    call(&ctx, &mut s, &req("n", &server, notify(), None)).await;
    let client = net.sent_to(&server)[0].0.pubkey;

    let asked = server_request(&server, client, "srv-1");
    net.deliver(asked.clone());
    let answer = json!({"jsonrpc": "2.0", "id": "srv-1", "result": {}});
    let r = call(&ctx, &mut s, &req("a", &server, answer, None)).await;
    assert!(r.get("error").is_none(), "{r}");

    let (sent, message) = net.sent_to(&server).last().unwrap().clone();
    assert_eq!(message["id"], "srv-1", "the id is the server's, unchanged");
    let tagged: Vec<_> = sent
        .tags
        .iter()
        .filter(|t| t.kind().as_str() == "e")
        .filter_map(|t| t.content().map(str::to_string))
        .collect();
    assert_eq!(tagged, vec![asked.id.to_hex()]);
}

/// A server cannot make a window hold, or hear, more than
/// `MAX_SERVER_REQUESTS` unanswered requests.
#[tokio::test]
async fn server_requests_beyond_the_bound_are_dropped() {
    let net = Net::new();
    let server = Keys::generate();
    let ctx = bridge(&net);
    let (mut s, pushed) = window();
    call(&ctx, &mut s, &req("n", &server, notify(), None)).await;
    let client = net.sent_to(&server)[0].0.pubkey;

    for n in 0..MAX_SERVER_REQUESTS + 10 {
        net.deliver(server_request(&server, client, &format!("srv-{n}")));
    }
    assert_eq!(pushed.lock().unwrap().len(), MAX_SERVER_REQUESTS);

    // Answering one makes room for the next.
    let answer = json!({"jsonrpc": "2.0", "id": "srv-0", "result": {}});
    call(&ctx, &mut s, &req("a", &server, answer, None)).await;
    net.deliver(server_request(&server, client, "srv-late"));
    assert_eq!(pushed.lock().unwrap().len(), MAX_SERVER_REQUESTS + 1);
}

/// An id whose request never reached a relay — its initialization timed
/// out — is the napplet's to use again. One that went out is not.
#[tokio::test]
async fn an_id_whose_request_never_went_out_may_be_retried() {
    let net = Net::new();
    let server = Keys::generate();
    let ctx = bridge(&net);
    let (mut s, _) = window();

    // Nobody answers `initialize`: the call times out before sending.
    let r = call(
        &ctx,
        &mut s,
        &req(
            "t",
            &server,
            tools_call(json!(7)),
            Some(json!({"timeoutMs": 50})),
        ),
    )
    .await;
    assert_eq!(r["error"], RELAY_TIMEOUT);
    assert!(net
        .sent_to(&server)
        .iter()
        .all(|(_, m)| m["method"] == "initialize"));

    // The same id again, without initialization: accepted and sent.
    let pending = spawn_call(
        &ctx,
        &s,
        req(
            "t2",
            &server,
            tools_call(json!(7)),
            Some(json!({"initialize": false})),
        ),
    );
    let msgs = sent(&net, &server, 2).await;
    let (request, m) = msgs.last().unwrap().clone();
    assert_eq!(m["method"], "tools/call");
    net.reply(
        &server,
        &request,
        json!({"jsonrpc": "2.0", "id": 7, "result": {}}),
        vec![],
    );
    assert!(pending.await.unwrap().get("error").is_none());

    // Now it went out: the id is used.
    let r = call(
        &ctx,
        &mut s,
        &req(
            "t3",
            &server,
            tools_call(json!(7)),
            Some(json!({"initialize": false})),
        ),
    )
    .await;
    assert_eq!(r["error"], INVALID_MESSAGE);
}

/// A session is initialized only once the server was told
/// `notifications/initialized`; if that never went out, the next call
/// initializes again.
#[tokio::test]
async fn a_session_whose_initialized_notice_failed_initializes_again() {
    let net = Net::new();
    let server = Keys::generate();
    net.auto_initialize(&server, false);
    *net.refuse_method.lock().unwrap() = Some("notifications/initialized".into());
    let ctx = bridge(&net);
    let (mut s, _) = window();
    let r = call(&ctx, &mut s, &req("a", &server, tools_call(json!(1)), None)).await;
    assert_eq!(r["error"], SERVER_NOT_FOUND);

    *net.refuse_method.lock().unwrap() = None;
    let pending = spawn_call(&ctx, &s, req("b", &server, tools_call(json!(2)), None));
    let msgs = sent(&net, &server, 4).await;
    let methods: Vec<_> = msgs.iter().map(|(_, m)| m["method"].clone()).collect();
    assert_eq!(
        methods,
        vec![
            json!("initialize"),
            json!("initialize"),
            json!("notifications/initialized"),
            json!("tools/call")
        ]
    );
    let (request, _) = msgs[3].clone();
    net.reply(
        &server,
        &request,
        json!({"jsonrpc": "2.0", "id": 2, "result": {}}),
        vec![],
    );
    assert!(pending.await.unwrap().get("error").is_none());
}

fn at(event: EventBuilder, keys: &Keys, secs: u64) -> Event {
    event
        .custom_created_at(nostr::Timestamp::from(secs))
        .sign_with_keys(keys)
        .unwrap()
}

/// What a napplet's own relay hints bring back answers that napplet only:
/// another window's request still looks the server up for itself.
#[tokio::test]
async fn hinted_discovery_is_not_remembered_for_others() {
    let net = Net::new();
    let server = Keys::generate();
    net.stored.lock().unwrap().extend([
        announcement(&server, 11316, json!({}), vec![]),
        relay_list(&server, &[&["r", "wss://in.example.com"]]),
    ]);
    let ctx = bridge(&net);
    let (mut s, _) = window();
    let discover = Envelope::new("cvm.discover")
        .with_id("d")
        .with_field("query", json!({"relays": ["wss://napplets.example.com"]}));
    let r = call(&ctx, &mut s, &discover).await;
    assert_eq!(r["servers"][0]["relays"], json!(["wss://in.example.com"]));
    let queries = net.queried_on.lock().unwrap().len();

    let (mut other, _) = window();
    call(&ctx, &mut other, &req("n", &server, notify(), None)).await;
    assert_eq!(
        net.queried_on.lock().unwrap().len(),
        queries + 1,
        "looked up on the device's own discovery relays"
    );
    assert_eq!(
        net.queried_on.lock().unwrap().last().unwrap(),
        &vec![RELAY.to_string()]
    );
}

/// An old relay list, replayed, never replaces a newer one the device knows.
#[tokio::test]
async fn an_older_relay_list_never_replaces_a_newer_one() {
    let net = Net::new();
    let server = Keys::generate();
    let list = |url: &str| {
        EventBuilder::new(Kind::Custom(10002), "").tags([Tag::parse(["r", url]).unwrap()])
    };
    net.stored
        .lock()
        .unwrap()
        .push(at(list("wss://new.example.com"), &server, 2_000_000_000));
    let ctx = bridge(&net);
    let (mut s, _) = window();
    call(&ctx, &mut s, &req("n", &server, notify(), None)).await;
    assert_eq!(
        net.published_on.lock().unwrap()[0],
        vec!["wss://new.example.com"]
    );

    // Discovery now meets only an older list.
    *net.stored.lock().unwrap() = vec![
        announcement(&server, 11316, json!({}), vec![]),
        at(list("wss://old.example.com"), &server, 1_000_000_000),
    ];
    call(&ctx, &mut s, &Envelope::new("cvm.discover").with_id("d")).await;

    let (mut other, _) = window();
    call(&ctx, &mut other, &req("n", &server, notify(), None)).await;
    assert_eq!(
        net.published_on.lock().unwrap().last().unwrap(),
        &vec!["wss://new.example.com".to_string()]
    );
}

/// A server whose announcement offers CEP-4 gets nothing in plaintext, not
/// even `initialize`.
#[tokio::test]
async fn a_server_announcing_encryption_is_wrapped_from_the_first_message() {
    let net = Net::new();
    let server = Keys::generate();
    net.auto_initialize(&server, true);
    net.stored.lock().unwrap().push(announcement(
        &server,
        11316,
        json!({}),
        vec![Tag::parse(["support_encryption"]).unwrap()],
    ));
    let ctx = bridge(&net);
    let (mut s, _) = window();
    call(&ctx, &mut s, &Envelope::new("cvm.discover").with_id("d")).await;

    let pending = spawn_call(&ctx, &s, req("e", &server, tools_call(json!(1)), None));
    let msgs = sent(&net, &server, 3).await;
    assert!(net
        .published
        .lock()
        .unwrap()
        .iter()
        .all(|e| e.kind.as_u16() == KIND_GIFT_WRAP));
    let init = &msgs[0].1;
    assert_eq!(init["method"], "initialize");
    assert_eq!(init["params"]["clientInfo"]["name"], CLIENT_NAME);
    net.reply(
        &server,
        &msgs[2].0,
        json!({"jsonrpc": "2.0", "id": 1, "result": {}}),
        vec![],
    );
    assert!(pending.await.unwrap().get("error").is_none());
}
