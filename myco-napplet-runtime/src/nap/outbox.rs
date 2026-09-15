//! NAP-OUTBOX — outbox-aware relay access.
//!
//! A napplet can already reach relays through NAP-RELAY; then every napplet
//! has to discover NIP-65 lists, pick an author's write relays, fall back when
//! a list is missing and deduplicate what comes back. NAP-OUTBOX moves that
//! into the shell: the napplet hands over filters and intent, the shell
//! resolves the relays, asks them, deduplicates by id and says how sure it is.
//!
//! ## The three lanes
//!
//! A plan is a list of [`RelayLane`]s — this device's relay, a Circle peer's
//! relay over the mesh, or an internet relay — because a NIP-65 list can name
//! `ws://<npub>.fips:4870` beside `wss://` relays and the outbox model works
//! unmodified (design §7.4). A mesh lane here is a *directed* connection to
//! one peer's relay. It is not the flood: that is NAP-MESH's, behind its own
//! grant and a hop budget, and nothing in this domain reaches it.
//!
//! ## What is awaited and what is not
//!
//! `getEvent`, `query` and `publish` wait for their lanes, bounded — a napplet
//! asked for relay-selected results and gets them, with `incomplete` when a
//! lane never answered. `subscribe` does not: it answers the local backlog and
//! pulls the remote lanes *into* the local relay, which delivers whatever
//! arrives as `outbox.event` the same way a live event is delivered. The spec
//! has no `outbox.eose` for exactly this reason.

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use nostr::{Event, EventId, Filter, PublicKey};

use crate::dispatch::NapContext;
use crate::nap::relay::{event_json, filters_from, sign_template};
use crate::seams::{Direction, Envelope, RelayLane, RelayPlan};
use crate::session::Session;

/// How long a read waits for its lanes when the napplet did not say.
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a publish waits for its lanes. Not a napplet option in the spec.
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(8);
/// Bounds on a napplet-supplied `timeoutMs`: below the floor a relay across
/// the mesh cannot answer, above the ceiling the session loop is hostage.
const MIN_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_TIMEOUT: Duration = Duration::from_secs(30);

/// Handle an inbound `outbox.*` message.
pub async fn handle(ctx: &NapContext, session: &mut Session, message: &Envelope) -> Vec<Envelope> {
    match message.action() {
        "getEvent" => vec![get_event(ctx, message).await],
        "query" => vec![query(ctx, message).await],
        "subscribe" => subscribe(ctx, session, message).await,
        "close" => {
            let Some(sub_id) = message.field("subId").and_then(|v| v.as_str()) else {
                return Vec::new();
            };
            session.unsubscribe_in("outbox", sub_id);
            // A lifecycle message answers the request, as the spec asks; the
            // napplet's shim has already forgotten the id and ignores it.
            vec![Envelope::new("outbox.closed")
                .with_field("subId", sub_id)
                .with_field("reason", "closed")]
        }
        "publish" => vec![publish(ctx, message).await],
        "resolveRelays" => vec![resolve_relays(ctx, message).await],
        _ => Vec::new(),
    }
}

/// `outbox.getEvent` — one event by id, from the author's relays when an
/// author is hinted, from policy relays otherwise.
async fn get_event(ctx: &NapContext, message: &Envelope) -> Envelope {
    let id = match message.field("eventId").and_then(|v| v.as_str()) {
        Some(hex) => match EventId::from_hex(hex) {
            Ok(id) => id,
            Err(_) => return message.to_error("invalid event id"),
        },
        None => return message.to_error("getEvent needs an eventId"),
    };
    let options = options_of(message);
    let authors = match pubkeys_in(options, "author") {
        Ok(authors) => authors,
        Err(e) => return message.to_error(e),
    };
    let hints = match hint_lanes(options) {
        Ok(hints) => hints,
        Err(e) => return message.to_error(e),
    };
    let timeout = timeout_in(options, DEFAULT_READ_TIMEOUT);

    let plan = ctx.outbox.plan(Direction::Read, &authors).await;
    let lanes = dedupe(
        std::iter::once(RelayLane::Local)
            .chain(plan.lanes.iter().cloned())
            .chain(hints),
    );
    let answers = ctx
        .lanes
        .query(&lanes, &[Filter::new().id(id)], timeout)
        .await;

    let mut found: Option<Event> = None;
    let mut hints_for: Vec<String> = Vec::new();
    let mut incomplete = !plan.missing_authors.is_empty();
    for (lane, events) in answers {
        let Some(events) = events else {
            incomplete = true;
            continue;
        };
        // The shell MUST check the id matches what was asked for.
        if let Some(event) = events.into_iter().find(|e| e.id == id) {
            if let Some(url) = lane.url() {
                hints_for.push(url.to_string());
            }
            found.get_or_insert(event);
        }
    }

    let mut out = message.to_result();
    match found {
        Some(event) => {
            out = out.with_field("result", result_with_hints(&event, &hints_for));
        }
        None => {
            out = out.with_field("error", "not found");
        }
    }
    if incomplete {
        out = out.with_field("incomplete", true);
    }
    out
}

/// `outbox.query` — a one-shot, relay-selected query, deduplicated by id.
async fn query(ctx: &NapContext, message: &Envelope) -> Envelope {
    let filters = match filters_from(message) {
        Ok(filters) => filters,
        Err(e) => return message.to_error(e),
    };
    let options = options_of(message);
    let mut authors = match pubkeys_in(options, "authors") {
        Ok(authors) => authors,
        Err(e) => return message.to_error(e),
    };
    for filter in &filters {
        if let Some(set) = &filter.authors {
            authors.extend(set.iter().copied());
        }
    }
    let authors = dedupe_keys(authors);
    let hints = match hint_lanes(options) {
        Ok(hints) => hints,
        Err(e) => return message.to_error(e),
    };
    let timeout = timeout_in(options, DEFAULT_READ_TIMEOUT);
    let limit = options
        .and_then(|o| o.get("limit"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    let plan = ctx.outbox.plan(Direction::Read, &authors).await;
    let lanes = dedupe(
        std::iter::once(RelayLane::Local)
            .chain(plan.lanes.iter().cloned())
            .chain(hints),
    );
    let answers = ctx.lanes.query(&lanes, &filters, timeout).await;

    let mut incomplete = !plan.missing_authors.is_empty();
    let (events, hints_by_id) = merge(answers, &mut incomplete);
    let events: Vec<serde_json::Value> = events
        .into_iter()
        .take(limit.unwrap_or(usize::MAX))
        .map(|e| {
            let hints = hints_by_id.get(&e.id).cloned().unwrap_or_default();
            result_with_hints(&e, &hints)
        })
        .collect();

    let mut out = message.to_result().with_field("events", events);
    if incomplete {
        out = out.with_field("incomplete", true);
    }
    out
}

/// `outbox.subscribe` — register the filters, answer the local backlog, and
/// pull the planned lanes into the local relay for live delivery.
async fn subscribe(ctx: &NapContext, session: &mut Session, message: &Envelope) -> Vec<Envelope> {
    let sub_id = match message.field("subId").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return vec![message.to_error("subscribe needs a subId")],
    };
    let closed = |reason: String| {
        vec![Envelope::new("outbox.closed")
            .with_field("subId", sub_id.clone())
            .with_field("reason", reason)]
    };
    let filters = match filters_from(message) {
        Ok(filters) => filters,
        Err(e) => return closed(e),
    };
    let options = options_of(message);
    let mut authors = match pubkeys_in(options, "authors") {
        Ok(authors) => authors,
        Err(e) => return closed(e),
    };
    for filter in &filters {
        if let Some(set) = &filter.authors {
            authors.extend(set.iter().copied());
        }
    }
    let authors = dedupe_keys(authors);
    let hints = match hint_lanes(options) {
        Ok(hints) => hints,
        Err(e) => return closed(e),
    };

    let out = match crate::nap::open_subscription(ctx, session, "outbox", &sub_id, filters.clone())
        .await
    {
        Ok(backlog) => backlog,
        Err(reason) => return closed(reason),
    };

    let plan = ctx.outbox.plan(Direction::Read, &authors).await;
    let remote: Vec<RelayLane> = dedupe(plan.lanes.into_iter().chain(hints))
        .into_iter()
        .filter(|lane| *lane != RelayLane::Local)
        .collect();
    if !remote.is_empty() {
        if let Err(e) = ctx.lanes.pull_into_local(&remote, &filters).await {
            tracing::warn!(sub_id, error = %e, "outbox pull could not be started");
        }
    }
    out
}

/// `outbox.publish` — sign once, fan out to the user's outbox, the named
/// inboxes and any validated explicit relays, and say per relay how it went.
async fn publish(ctx: &NapContext, message: &Envelope) -> Envelope {
    let options = options_of(message);
    let explicit = match hint_lanes(options) {
        Ok(hints) => hints,
        Err(e) => return failed(message, e),
    };
    let to_outbox = options
        .and_then(|o| o.get("toOutbox"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let inboxes = match pubkeys_in(options, "toInboxes") {
        Ok(keys) => dedupe_keys(keys),
        Err(e) => return failed(message, e),
    };

    // Plans first, template second: a fanout that cannot be satisfied is
    // reported before anything is signed, so a refused publish leaves no
    // event behind.
    let mut lanes: Vec<RelayLane> = vec![RelayLane::Local];
    if to_outbox {
        let Ok(user) = ctx.signer.public_key().await else {
            return failed(message, "there is no user key on this device yet");
        };
        // The user's own outbox is where their events are read from.
        lanes.extend(ctx.outbox.plan(Direction::Read, &[user]).await.lanes);
    }
    if !inboxes.is_empty() {
        let plan = ctx.outbox.plan(Direction::Write, &inboxes).await;
        if !plan.missing_authors.is_empty() {
            return failed(message, "relay list unavailable");
        }
        lanes.extend(plan.lanes);
    }
    lanes.extend(explicit);
    let lanes = dedupe(lanes);

    let signed = match sign_template(ctx, message).await {
        Ok(event) => event,
        Err(e) => return failed(message, e),
    };

    let outcomes = ctx.lanes.publish(&lanes, &signed, PUBLISH_TIMEOUT).await;
    let mut relays = serde_json::Map::new();
    let mut stored = false;
    for (lane, ok) in outcomes {
        match lane.url() {
            None => stored = ok,
            Some(url) => {
                relays.insert(url.to_string(), serde_json::Value::Bool(ok));
            }
        }
    }
    if !stored {
        return failed(message, "could not store the event");
    }

    let id = signed.id.to_hex();
    tracing::info!(kind = %signed.kind.as_u16(), event = %id, relays = relays.len(), "napplet published via outbox");
    message
        .to_result()
        .with_field("ok", true)
        .with_field("event", event_json(&signed))
        .with_field("eventId", id)
        .with_field("relays", serde_json::Value::Object(relays))
}

/// `outbox.resolveRelays` — the plan the shell would use, for diagnostics.
async fn resolve_relays(ctx: &NapContext, message: &Envelope) -> Envelope {
    let target = message.field("target").and_then(|v| v.as_object());
    let mut authors = match pubkeys_in(target, "authors") {
        Ok(keys) => keys,
        Err(e) => return message.to_error(e),
    };
    match pubkeys_in(target, "pubkey") {
        Ok(keys) => authors.extend(keys),
        Err(e) => return message.to_error(e),
    }
    let authors = dedupe_keys(authors);
    let direction = match target
        .and_then(|t| t.get("direction"))
        .and_then(|v| v.as_str())
        .unwrap_or("read")
    {
        "read" => Direction::Read,
        "write" => Direction::Write,
        _ => return message.to_error("direction must be read or write"),
    };

    let plan = ctx.outbox.plan(direction, &authors).await;
    message.to_result().with_field("plan", plan_json(&plan))
}

/// The `outbox.event` frames a session should receive for an arriving event.
pub fn deliveries_for(session: &Session, event: &Event) -> Vec<Envelope> {
    crate::nap::deliveries_in(session, "outbox", event)
}

// --- helpers ---------------------------------------------------------------

fn options_of(message: &Envelope) -> Option<&serde_json::Map<String, serde_json::Value>> {
    message.field("options").and_then(|v| v.as_object())
}

/// Public keys under `key` in `object`: a hex string, or a list of them.
/// Absent is empty; present and unreadable is an error, because an author a
/// napplet named and the shell silently dropped would route its query to the
/// wrong relays with no way to tell.
fn pubkeys_in(
    object: Option<&serde_json::Map<String, serde_json::Value>>,
    key: &str,
) -> Result<Vec<PublicKey>, String> {
    let Some(value) = object.and_then(|o| o.get(key)) else {
        return Ok(Vec::new());
    };
    let items: Vec<&serde_json::Value> = match value {
        serde_json::Value::Null => return Ok(Vec::new()),
        serde_json::Value::Array(items) => items.iter().collect(),
        single => vec![single],
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(hex) = item.as_str() else {
            return Err(format!("{key} must be hex public keys"));
        };
        match PublicKey::from_hex(hex) {
            Ok(pk) => out.push(pk),
            Err(_) => return Err(format!("{key}: invalid public key")),
        }
    }
    Ok(out)
}

fn dedupe_keys(keys: Vec<PublicKey>) -> Vec<PublicKey> {
    let mut seen = BTreeSet::new();
    keys.into_iter().filter(|k| seen.insert(*k)).collect()
}

fn dedupe(lanes: impl IntoIterator<Item = RelayLane>) -> Vec<RelayLane> {
    let mut seen = std::collections::HashSet::new();
    lanes
        .into_iter()
        .filter(|l| seen.insert(l.clone()))
        .collect()
}

/// `options.relays`, validated. A napplet may name relays; it may not make the
/// shell connect to a loopback or private-network address.
fn hint_lanes(
    options: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<Vec<RelayLane>, String> {
    let Some(raw) = options.and_then(|o| o.get("relays")) else {
        return Ok(Vec::new());
    };
    let Some(items) = raw.as_array() else {
        return Err("relays must be a list of URLs".to_string());
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(url) = item.as_str() else {
            return Err("relays must be a list of URLs".to_string());
        };
        out.push(validate_relay_url(url)?);
    }
    Ok(out)
}

/// Accept `ws://` and `wss://` URLs to public hosts and to `.fips` mesh
/// peers; refuse anything that would point the shell at itself or at a
/// private network.
pub fn validate_relay_url(url: &str) -> Result<RelayLane, String> {
    let rest = url
        .strip_prefix("wss://")
        .or_else(|| url.strip_prefix("ws://"))
        .ok_or_else(|| format!("relay URL must be ws:// or wss://: {url}"))?;
    let authority = rest.split('/').next().unwrap_or("");
    let host = authority
        .strip_prefix('[')
        .and_then(|h| h.split(']').next())
        .unwrap_or_else(|| authority.split(':').next().unwrap_or(""));
    if host.is_empty() {
        return Err(format!("relay URL has no host: {url}"));
    }
    if host.ends_with(".fips") {
        return Ok(RelayLane::Mesh {
            url: url.to_string(),
        });
    }
    if is_private_host(host) {
        return Err(format!("relay URL is not allowed: {url}"));
    }
    Ok(RelayLane::Internet {
        url: url.to_string(),
    })
}

fn is_private_host(host: &str) -> bool {
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") || lower.ends_with(".local") {
        return true;
    }
    if let Ok(v4) = lower.parse::<std::net::Ipv4Addr>() {
        return v4.is_loopback()
            || v4.is_private()
            || v4.is_link_local()
            || v4.is_unspecified()
            || v4.is_broadcast();
    }
    if let Ok(v6) = lower.parse::<std::net::Ipv6Addr>() {
        let segs = v6.segments();
        return v6.is_loopback()
            || v6.is_unspecified()
            || (segs[0] & 0xfe00) == 0xfc00 // unique local
            || (segs[0] & 0xffc0) == 0xfe80; // link local
    }
    false
}

fn timeout_in(
    options: Option<&serde_json::Map<String, serde_json::Value>>,
    default: Duration,
) -> Duration {
    options
        .and_then(|o| o.get("timeoutMs"))
        .and_then(|v| v.as_u64())
        .map(Duration::from_millis)
        .map(|d| d.clamp(MIN_TIMEOUT, MAX_TIMEOUT))
        .unwrap_or(default)
}

/// Merge per-lane answers: dedupe by id, newest first, remembering which
/// lanes returned each id for `relayHints`.
fn merge(
    answers: Vec<(RelayLane, Option<Vec<Event>>)>,
    incomplete: &mut bool,
) -> (Vec<Event>, HashMap<EventId, Vec<String>>) {
    let mut by_id: HashMap<EventId, Event> = HashMap::new();
    let mut hints: HashMap<EventId, Vec<String>> = HashMap::new();
    for (lane, events) in answers {
        let Some(events) = events else {
            *incomplete = true;
            continue;
        };
        for event in events {
            if let Some(url) = lane.url() {
                hints.entry(event.id).or_default().push(url.to_string());
            }
            by_id.entry(event.id).or_insert(event);
        }
    }
    let mut events: Vec<Event> = by_id.into_values().collect();
    events.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
    (events, hints)
}

/// A `RelayEventResult` with `sidecar.relayHints` when there is anything to
/// disclose. The local relay is never a hint: nothing outside this device
/// could use its address.
fn result_with_hints(event: &Event, hints: &[String]) -> serde_json::Value {
    let mut out = serde_json::json!({ "event": event_json(event) });
    if !hints.is_empty() {
        out["sidecar"] = serde_json::json!({ "relayHints": hints });
    }
    out
}

fn plan_json(plan: &RelayPlan) -> serde_json::Value {
    let relays: Vec<&str> = plan.lanes.iter().filter_map(RelayLane::url).collect();
    let mut out = serde_json::json!({
        "relays": relays,
        "source": plan.source.as_str(),
    });
    if !plan.missing_authors.is_empty() {
        out["missingAuthors"] = serde_json::json!(plan
            .missing_authors
            .iter()
            .map(|k| k.to_hex())
            .collect::<Vec<_>>());
    }
    out
}

/// A publish failure in the spec's shape: `ok` false beside the reason.
fn failed(message: &Envelope, error: impl Into<String>) -> Envelope {
    message
        .to_result()
        .with_field("ok", false)
        .with_field("error", error.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::dispatch;
    use crate::seams::{PlanSource, RelayBackend as _};
    use crate::session::NappletIdentity;
    use crate::testing::test_context_with_outbox;
    use nostr::{EventBuilder, Keys, Kind};
    use serde_json::json;

    fn granted() -> Session {
        let mut s = Session::new(
            NappletIdentity::new("feed", "aggregate"),
            ["relay", "outbox"],
        );
        s.on_ready();
        s
    }

    async fn call(ctx: &NapContext, s: &mut Session, e: Envelope) -> Vec<Envelope> {
        dispatch(ctx, s, &e).await.envelopes().to_vec()
    }

    fn note(keys: &Keys, text: &str) -> Event {
        EventBuilder::text_note(text).sign_with_keys(keys).unwrap()
    }

    /// The point of the domain: the napplet names an author, the shell finds
    /// that author's relay and asks it, and says the answer is complete.
    #[tokio::test]
    async fn query_reads_from_the_authors_nip65_relays() {
        let (ctx, fx, _signer) = test_context_with_outbox();
        let alice = Keys::generate();
        let alices_relay = "wss://alice.example";
        fx.set_plan(
            alice.public_key(),
            Direction::Read,
            &[alices_relay],
            PlanSource::Nip65,
        );
        let posted = note(&alice, "on my relay");
        fx.relay(alices_relay)
            .publish(posted.clone())
            .await
            .unwrap();

        let mut s = granted();
        let out = call(
            &ctx,
            &mut s,
            Envelope::new("outbox.query").with_id("q1").with_field(
                "filters",
                json!({"kinds": [1], "authors": [alice.public_key().to_hex()]}),
            ),
        )
        .await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["type"], "outbox.query.result");
        assert_eq!(r["id"], "q1");
        assert_eq!(r["events"].as_array().unwrap().len(), 1);
        assert_eq!(r["events"][0]["event"]["id"], posted.id.to_hex());
        assert_eq!(
            r["events"][0]["sidecar"]["relayHints"],
            json!([alices_relay])
        );
        assert!(
            r.get("incomplete").is_none(),
            "a complete answer was marked incomplete"
        );

        let asked = fx.queried();
        assert!(asked.contains(&RelayLane::Local));
        assert!(asked.contains(&RelayLane::Internet {
            url: alices_relay.to_string()
        }));
    }

    /// The same event on two relays is one result carrying both hints; a
    /// relay that never answered makes the answer `incomplete`, not empty.
    #[tokio::test]
    async fn query_dedupes_by_id_and_reports_a_dead_lane() {
        let (ctx, fx, _signer) = test_context_with_outbox();
        let alice = Keys::generate();
        fx.set_plan(
            alice.public_key(),
            Direction::Read,
            &["wss://a.example", "wss://b.example", "wss://dead.example"],
            PlanSource::Nip65,
        );
        let posted = note(&alice, "everywhere");
        fx.relay("wss://a.example")
            .publish(posted.clone())
            .await
            .unwrap();
        fx.relay("wss://b.example")
            .publish(posted.clone())
            .await
            .unwrap();
        fx.mark_dead("wss://dead.example");

        let mut s = granted();
        let out = call(
            &ctx,
            &mut s,
            Envelope::new("outbox.query")
                .with_id("q1")
                .with_field("filters", json!([{"kinds": [1]}]))
                .with_field("options", json!({"authors": [alice.public_key().to_hex()]})),
        )
        .await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["events"].as_array().unwrap().len(), 1);
        let mut hints: Vec<String> =
            serde_json::from_value(r["events"][0]["sidecar"]["relayHints"].clone()).unwrap();
        hints.sort();
        assert_eq!(hints, vec!["wss://a.example", "wss://b.example"]);
        assert_eq!(r["incomplete"], true);
    }

    /// No relay list for an author: the plan falls back and says whose list
    /// was missing, and the query is `incomplete` rather than silently narrow.
    #[tokio::test]
    async fn a_missing_relay_list_falls_back_and_says_so() {
        let (ctx, fx, _signer) = test_context_with_outbox();
        let nobody = Keys::generate();
        fx.set_fallback(&["wss://fallback.example"]);

        let mut s = granted();
        let out = call(
            &ctx,
            &mut s,
            Envelope::new("outbox.resolveRelays")
                .with_id("r1")
                .with_field(
                    "target",
                    json!({"authors": [nobody.public_key().to_hex()], "direction": "read"}),
                ),
        )
        .await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["plan"]["source"], "fallback");
        assert_eq!(r["plan"]["relays"], json!(["wss://fallback.example"]));
        assert_eq!(
            r["plan"]["missingAuthors"],
            json!([nobody.public_key().to_hex()])
        );

        let out = call(
            &ctx,
            &mut s,
            Envelope::new("outbox.query").with_id("q1").with_field(
                "filters",
                json!({"authors": [nobody.public_key().to_hex()]}),
            ),
        )
        .await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["events"], json!([]));
        assert_eq!(r["incomplete"], true);
    }

    #[tokio::test]
    async fn get_event_checks_the_id_and_uses_the_author_hint() {
        let (ctx, fx, _signer) = test_context_with_outbox();
        let alice = Keys::generate();
        fx.set_plan(
            alice.public_key(),
            Direction::Read,
            &["wss://alice.example"],
            PlanSource::Nip65,
        );
        let wanted = note(&alice, "this one");
        let other = note(&alice, "not this one");
        fx.relay("wss://alice.example")
            .publish(wanted.clone())
            .await
            .unwrap();
        fx.relay("wss://alice.example")
            .publish(other)
            .await
            .unwrap();

        let mut s = granted();
        let out = call(
            &ctx,
            &mut s,
            Envelope::new("outbox.getEvent")
                .with_id("g1")
                .with_field("eventId", wanted.id.to_hex())
                .with_field(
                    "options",
                    json!({"author": alice.public_key().to_hex(), "timeoutMs": 1000}),
                ),
        )
        .await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["type"], "outbox.getEvent.result");
        assert_eq!(r["result"]["event"]["id"], wanted.id.to_hex());
        assert_eq!(
            r["result"]["sidecar"]["relayHints"],
            json!(["wss://alice.example"])
        );

        let out = call(
            &ctx,
            &mut s,
            Envelope::new("outbox.getEvent")
                .with_id("g2")
                .with_field("eventId", "00".repeat(32)),
        )
        .await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["error"], "not found");
        assert!(r.get("result").is_none());
    }

    /// Publish: signed once, stored here, sent to the user's own outbox and
    /// to each named inbox, with a per-relay verdict.
    #[tokio::test]
    async fn publish_fans_out_to_own_outbox_and_named_inboxes() {
        let (ctx, fx, signer) = test_context_with_outbox();
        fx.set_plan(
            signer.public_key(),
            Direction::Read,
            &["wss://mine.example"],
            PlanSource::Nip65,
        );
        let bob = Keys::generate();
        fx.set_plan(
            bob.public_key(),
            Direction::Write,
            &["wss://bob-inbox.example"],
            PlanSource::Nip65,
        );
        fx.mark_refusing("wss://bob-inbox.example");

        let mut s = granted();
        let out = call(
            &ctx,
            &mut s,
            Envelope::new("outbox.publish")
                .with_id("p1")
                .with_field("event", json!({"kind": 1, "content": "hi bob", "tags": [["p", bob.public_key().to_hex()]]}))
                .with_field("options", json!({"toInboxes": [bob.public_key().to_hex()], "relays": ["wss://extra.example"]})),
        )
        .await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["type"], "outbox.publish.result");
        assert_eq!(r["ok"], true);
        assert_eq!(r["event"]["pubkey"], signer.public_key().to_hex());
        assert_eq!(
            r["relays"],
            json!({
                "wss://mine.example": true,
                "wss://bob-inbox.example": false,
                "wss://extra.example": true,
            })
        );
        // Stored here too, so this phone's own subscriptions see it.
        let here = ctx
            .relay
            .query(&[Filter::new().kind(Kind::TextNote)])
            .await
            .unwrap();
        assert_eq!(here.len(), 1);
        assert_eq!(fx.relay("wss://mine.example").len(), 1);
        assert_eq!(fx.relay("wss://extra.example").len(), 1);
    }

    /// An inbox whose relay list cannot be resolved is a refusal before
    /// signing: the spec makes `toInboxes` a delivery contract, not a hint.
    #[tokio::test]
    async fn an_unresolvable_inbox_refuses_the_publish_unsigned() {
        let (ctx, fx, _signer) = test_context_with_outbox();
        let stranger = Keys::generate();
        fx.set_fallback(&["wss://fallback.example"]);

        let mut s = granted();
        let out = call(
            &ctx,
            &mut s,
            Envelope::new("outbox.publish")
                .with_id("p1")
                .with_field("event", json!({"kind": 1, "content": "hello?"}))
                .with_field(
                    "options",
                    json!({"toInboxes": [stranger.public_key().to_hex()]}),
                ),
        )
        .await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["ok"], false);
        assert_eq!(r["error"], "relay list unavailable");
        assert!(ctx
            .relay
            .query(&[Filter::new().kind(Kind::TextNote)])
            .await
            .unwrap()
            .is_empty());
        assert!(fx.published().is_empty());
    }

    /// `toOutbox: false` with nothing else is a local publish and an empty map.
    #[tokio::test]
    async fn publish_without_outbox_stays_local() {
        let (ctx, fx, signer) = test_context_with_outbox();
        fx.set_plan(
            signer.public_key(),
            Direction::Read,
            &["wss://mine.example"],
            PlanSource::Nip65,
        );
        let mut s = granted();
        let out = call(
            &ctx,
            &mut s,
            Envelope::new("outbox.publish")
                .with_id("p1")
                .with_field("event", json!({"kind": 1, "content": "just here"}))
                .with_field("options", json!({"toOutbox": false})),
        )
        .await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["relays"], json!({}));
        assert_eq!(fx.relay("wss://mine.example").len(), 0);
    }

    /// A napplet cannot point the shell at itself or a private network.
    #[test]
    fn private_relay_urls_are_refused() {
        for bad in [
            "http://relay.example",
            "wss://localhost:4870",
            "ws://127.0.0.1:4870",
            "ws://10.0.0.5",
            "ws://192.168.1.2:4870",
            "ws://172.16.0.1",
            "ws://[::1]:4870",
            "ws://[fd00::1]:4870",
            "ws://[fe80::1]:4870",
            "wss://relay.local",
            "wss://",
        ] {
            assert!(validate_relay_url(bad).is_err(), "accepted {bad}");
        }
        assert_eq!(
            validate_relay_url("wss://relay.damus.io").unwrap(),
            RelayLane::Internet {
                url: "wss://relay.damus.io".into()
            }
        );
        assert_eq!(
            validate_relay_url("ws://npub1abc.fips:4870").unwrap(),
            RelayLane::Mesh {
                url: "ws://npub1abc.fips:4870".into()
            }
        );
    }

    /// Subscribe: local backlog now, remote lanes pulled into the local relay,
    /// later matches delivered as `outbox.event`; a relay subscription with
    /// the same id is untouched.
    #[tokio::test]
    async fn subscribe_answers_locally_pulls_remotely_and_delivers_live() {
        let (ctx, fx, _signer) = test_context_with_outbox();
        let alice = Keys::generate();
        fx.set_plan(
            alice.public_key(),
            Direction::Read,
            &["wss://alice.example"],
            PlanSource::Nip65,
        );
        let stored = note(&alice, "already here");
        ctx.relay.publish(stored.clone()).await.unwrap();

        let mut s = granted();
        let out = call(
            &ctx,
            &mut s,
            Envelope::new("outbox.subscribe")
                .with_id("s1")
                .with_field("subId", "feed")
                .with_field(
                    "filters",
                    json!({"kinds": [1], "authors": [alice.public_key().to_hex()]}),
                ),
        )
        .await;
        assert_eq!(out.len(), 1);
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["type"], "outbox.event");
        assert_eq!(r["subId"], "feed");
        assert_eq!(r["result"]["event"]["id"], stored.id.to_hex());

        let pulled = fx.pulled();
        assert_eq!(pulled.len(), 1);
        assert_eq!(
            pulled[0].0,
            vec![RelayLane::Internet {
                url: "wss://alice.example".into()
            }]
        );

        let later = note(&alice, "arrived later");
        let frames = crate::nap::deliveries_for(&s, &later);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].msg_type, "outbox.event");

        let out = call(
            &ctx,
            &mut s,
            Envelope::new("outbox.close")
                .with_id("c1")
                .with_field("subId", "feed"),
        )
        .await;
        assert_eq!(out[0].msg_type, "outbox.closed");
        assert!(crate::nap::deliveries_for(&s, &later).is_empty());
    }

    #[tokio::test]
    async fn an_ungranted_napplet_is_refused() {
        let (ctx, fx, _signer) = test_context_with_outbox();
        let mut s = Session::new(NappletIdentity::new("feed", "aggregate"), ["relay"]);
        s.on_ready();
        let out = call(
            &ctx,
            &mut s,
            Envelope::new("outbox.query")
                .with_id("q1")
                .with_field("filters", json!({"kinds": [1]})),
        )
        .await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert!(r["error"].as_str().unwrap().contains("not granted"));
        assert!(fx.queried().is_empty());
    }
}
