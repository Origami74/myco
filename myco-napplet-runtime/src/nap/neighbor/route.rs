//! Addresses inside other NAPs: where `relay`, `outbox` and `resource` hand a
//! `neighborhood:` address to the host instead of a relay lane.
//!
//! Caught **before** lane validation. A lane is the same for every napplet;
//! an address is one napplet's own and means nothing to another, so only the
//! host that minted it may resolve it. Nothing here ever dials anything.
//!
//! Using an address needs the `neighbor` grant as well as the calling
//! domain's: revoking `neighbor` stops a napplet's remembered addresses, not
//! only its `neighbor.*` calls.

use nostr::{Event, Filter};

use super::address;
use super::model::NeighborError;
use crate::dispatch::NapContext;
use crate::nap::event_frame;
use crate::seams::Envelope;
use crate::session::{NappletIdentity, Session};

/// Whether this napplet may use addresses at all right now.
fn allowed(session: &Session) -> Result<(), NeighborError> {
    if session.is_granted("neighbor") {
        Ok(())
    } else {
        Err(NeighborError::Denied)
    }
}

/// `relay.subscribe` on an address: the host's backlog, `eose`, then what
/// the host streams, until closed.
///
/// Registered with no filters of its own, so the device-wide live path —
/// which delivers whatever lands in the local relay — never feeds it.
/// Everything this subscription sees comes through the host, which keeps the
/// neighborhood's view apart from everything else on the device.
pub async fn subscribe(
    ctx: &NapContext,
    session: &mut Session,
    sub_id: &str,
    filters: Vec<Filter>,
    addr: &str,
) -> Vec<Envelope> {
    let closed = |reason: &str| {
        vec![Envelope::new("relay.closed")
            .with_field("subId", sub_id)
            .with_field("reason", reason)]
    };
    if let Err(e) = allowed(session) {
        return closed(e.as_str());
    }
    let feed = match ctx
        .neighbor
        .subscribe(session.identity(), addr, filters.clone())
        .await
    {
        Ok(feed) => feed,
        Err(e) => return closed(e.as_str()),
    };
    if let Err(reason) = session.subscribe_in("relay", sub_id, Vec::new()) {
        return closed(&reason);
    }
    let handle = session.subscription_handle("relay", sub_id);

    let mut out = Vec::new();
    for event in matching(&filters, feed.backlog) {
        handle.deliver(&event.id, || out.push(event_frame("relay", sub_id, &event)));
    }
    out.push(Envelope::new("relay.eose").with_field("subId", sub_id));

    // With no streamer there is nowhere to push between calls; the backlog
    // was the answer.
    if let Some(streamer) = session.streamer().cloned() {
        let sub_id = sub_id.to_string();
        let mut live = feed.live;
        (streamer.spawn)(Box::pin(async move {
            while let Some(event) = futures_util::StreamExt::next(&mut live).await {
                if handle.is_closed() {
                    return;
                }
                // The host's view is trusted to be scoped, not to be
                // filtered: only what the napplet asked for.
                if !matches(&filters, &event) {
                    continue;
                }
                handle.deliver(&event.id, || {
                    (streamer.push)(event_frame("relay", sub_id.as_str(), &event))
                });
            }
        }));
    }
    out
}

/// Publish `event` to each address. One result per address, in order:
/// `Ok` once the host took it.
pub async fn publish(
    ctx: &NapContext,
    session: &Session,
    addresses: &[String],
    event: &Event,
) -> Vec<(String, Result<(), NeighborError>)> {
    let mut out = Vec::with_capacity(addresses.len());
    for addr in addresses {
        let r = match allowed(session) {
            Ok(()) => {
                ctx.neighbor
                    .publish(session.identity(), addr, event.clone())
                    .await
            }
            Err(e) => Err(e),
        };
        out.push((addr.clone(), r));
    }
    out
}

/// A blob from the first address whose scope has it, hash verified.
pub async fn blob(
    ctx: &NapContext,
    napplet: &NappletIdentity,
    granted: bool,
    addresses: &[String],
    sha256: &str,
) -> Option<Vec<u8>> {
    if !granted {
        return None;
    }
    for addr in addresses {
        match ctx.neighbor.blob(napplet, addr, sha256).await {
            Ok(Some(bytes)) if nsite_deck::sync::sha256_hex(&bytes) == sha256 => {
                return Some(bytes)
            }
            Ok(Some(_)) => {
                tracing::warn!(sha256, "neighborhood blob failed its hash; ignored");
            }
            Ok(None) | Err(_) => {}
        }
    }
    None
}

/// Whether any of `addresses` could supply a blob, without transferring it:
/// `Some(true)` if one can, `Some(false)` if every one said no, `None` when
/// none could tell.
pub async fn has(
    ctx: &NapContext,
    napplet: &NappletIdentity,
    granted: bool,
    addresses: &[String],
    sha256: &str,
) -> Option<bool> {
    if !granted {
        return Some(false);
    }
    let mut unknown = false;
    for addr in addresses {
        match ctx.neighbor.has(napplet, addr, sha256).await {
            Ok(Some(true)) => return Some(true),
            Ok(Some(false)) | Err(_) => {}
            Ok(None) => unknown = true,
        }
    }
    if unknown {
        None
    } else {
        Some(false)
    }
}

/// Split `urls` into neighborhood addresses and everything else.
pub fn split(urls: Vec<String>) -> (Vec<String>, Vec<String>) {
    urls.into_iter().partition(|u| address::is_address(u))
}

fn matches(filters: &[Filter], event: &Event) -> bool {
    let options = nostr::filter::MatchEventOptions::new();
    filters.iter().any(|f| f.match_event(event, options))
}

fn matching(filters: &[Filter], events: Vec<Event>) -> impl Iterator<Item = Event> + '_ {
    events.into_iter().filter(move |e| matches(filters, e))
}

#[cfg(test)]
mod tests {
    use super::super::testing::MemNeighbors;
    use crate::dispatch::{dispatch, NapContext};
    use crate::seams::Envelope;
    use crate::session::{NappletIdentity, Session};
    use crate::testing::test_context;
    use serde_json::{json, Value};
    use std::sync::Arc;

    const HOME: &str = "neighborhood://h2c";

    fn session(granted: &[&str]) -> Session {
        let mut s = Session::new(NappletIdentity::new("chat", "aggregate"), granted.to_vec());
        s.on_ready();
        s
    }

    fn ctx() -> (NapContext, Arc<MemNeighbors>) {
        let (mut ctx, _signer) = test_context();
        let host = Arc::new(MemNeighbors::household());
        ctx.neighbor = host.clone();
        (ctx, host)
    }

    async fn send(ctx: &NapContext, s: &mut Session, e: Envelope) -> Vec<Value> {
        dispatch(ctx, s, &e)
            .await
            .envelopes()
            .iter()
            .map(|e| serde_json::to_value(e).unwrap())
            .collect()
    }

    fn publish(relays: Value) -> Envelope {
        Envelope::new("outbox.publish")
            .with_id("p1")
            .with_field("event", json!({"kind": 1, "content": "hi home"}))
            .with_field("options", json!({"relays": relays, "toOutbox": false}))
    }

    fn subscribe(relay: &str) -> Envelope {
        Envelope::new("relay.subscribe")
            .with_id("s")
            .with_field("subId", "s1")
            .with_field("filters", json!([{"kinds": [1]}]))
            .with_field("relay", relay)
    }

    #[tokio::test]
    async fn publish_then_subscribe_through_an_address() {
        let (ctx, host) = ctx();
        let mut s = session(&["neighbor", "outbox", "relay"]);
        let r = send(&ctx, &mut s, publish(json!([HOME]))).await;
        assert_eq!(r[0]["ok"], true, "{}", r[0]);
        assert_eq!(r[0]["relays"][HOME], true);
        assert_eq!(host.published.lock().unwrap().len(), 1);

        let out = send(&ctx, &mut s, subscribe(HOME)).await;
        assert_eq!(out[0]["type"], "relay.event");
        assert_eq!(out[0]["result"]["event"]["content"], "hi home");
        assert_eq!(out.last().unwrap()["type"], "relay.eose");
    }

    #[tokio::test]
    async fn an_address_subscription_sees_nothing_else_on_the_device() {
        let (ctx, _) = ctx();
        let mut s = session(&["neighbor", "outbox", "relay"]);
        // Published to the user's own store, not the neighborhood.
        let mine = Envelope::new("relay.publish")
            .with_id("p0")
            .with_field("event", json!({"kind": 1, "content": "mine"}));
        send(&ctx, &mut s, mine).await;
        let out = send(&ctx, &mut s, subscribe(HOME)).await;
        assert_eq!(out.len(), 1, "only eose: {out:?}");
        assert_eq!(out[0]["type"], "relay.eose");
    }

    #[tokio::test]
    async fn an_unknown_address_closes_the_subscription() {
        let (ctx, _) = ctx();
        let mut s = session(&["neighbor", "relay"]);
        let out = send(&ctx, &mut s, subscribe("neighborhood://nope")).await;
        assert_eq!(out[0]["type"], "relay.closed");
        assert_eq!(out[0]["reason"], "not found");
    }

    #[tokio::test]
    async fn without_the_neighbor_grant_addresses_are_refused() {
        let (ctx, host) = ctx();
        let mut s = session(&["outbox", "relay"]);
        let out = send(&ctx, &mut s, subscribe(HOME)).await;
        assert_eq!(out[0]["reason"], "denied");
        let r = send(&ctx, &mut s, publish(json!([HOME]))).await;
        assert_eq!(r[0]["relays"][HOME], false);
        assert!(host.published.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn outbox_reads_refuse_addresses_rather_than_ignore_them() {
        let (ctx, _) = ctx();
        let mut s = session(&["neighbor", "outbox"]);
        let q = Envelope::new("outbox.query")
            .with_id("q1")
            .with_field("filters", json!([{"kinds": [1]}]))
            .with_field("options", json!({"relays": [HOME]}));
        let r = send(&ctx, &mut s, q).await;
        assert!(
            r[0]["error"].as_str().unwrap().contains("relay.subscribe"),
            "{}",
            r[0]
        );
    }

    #[tokio::test]
    async fn a_blob_comes_from_the_address_and_is_hash_checked() {
        let (ctx, host) = ctx();
        let mut s = session(&["neighbor", "resource"]);
        let bytes = b"\x89PNG\r\n\x1a\nneighborhood".to_vec();
        let sha = nsite_deck::sync::sha256_hex(&bytes);
        let get = |sha: &str| {
            Envelope::new("resource.bytes").with_id("b1").with_field(
                "url",
                format!("blossom:{sha}.png?xs=neighborhood%3A%2F%2Fh2c"),
            )
        };

        let r = send(&ctx, &mut s, get(&sha)).await;
        assert_eq!(
            r[0]["type"], "resource.bytes.error",
            "nobody has it: {}",
            r[0]
        );

        host.blobs
            .lock()
            .unwrap()
            .insert(sha.clone(), bytes.clone());
        let r = send(&ctx, &mut s, get(&sha)).await;
        assert_eq!(r[0]["type"], "resource.bytes.result", "{}", r[0]);

        // A host answering with other bytes is not believed.
        let other = "0".repeat(64);
        host.blobs.lock().unwrap().insert(other.clone(), bytes);
        let r = send(&ctx, &mut s, get(&other)).await;
        assert_eq!(r[0]["type"], "resource.bytes.error");
    }

    fn upload(destination: Value) -> Envelope {
        use base64::Engine as _;
        let data =
            base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nneighborhood");
        Envelope::new("upload.upload").with_id("u1").with_field(
            "request",
            json!({"dataBase64": data, "destination": destination}),
        )
    }

    #[tokio::test]
    async fn an_upload_into_a_neighborhood_is_kept_there_and_readable_back() {
        let (ctx, host) = ctx();
        let mut s = session(&["neighbor", "upload", "resource"]);
        let r = send(&ctx, &mut s, upload(json!(HOME))).await;
        let result = &r[0]["result"];
        assert_eq!(result["ok"], true, "{}", r[0]);
        assert_eq!(result["rail"], "blossom-neighborhood");
        let sha = result["sha256"].as_str().unwrap().to_string();
        assert!(host.calls().contains(&format!("upload {sha}")));

        let get = Envelope::new("resource.bytes").with_id("b1").with_field(
            "url",
            format!("blossom:{sha}.png?xs=neighborhood%3A%2F%2Fh2c"),
        );
        let r = send(&ctx, &mut s, get).await;
        assert_eq!(r[0]["type"], "resource.bytes.result", "{}", r[0]);
    }

    #[tokio::test]
    async fn an_upload_destination_must_be_a_granted_address() {
        let (ctx, host) = ctx();
        let mut s = session(&["upload"]);
        let r = send(&ctx, &mut s, upload(json!(HOME))).await;
        assert_eq!(r[0]["result"]["error"], "policy denied", "{}", r[0]);

        let mut s = session(&["neighbor", "upload"]);
        let r = send(&ctx, &mut s, upload(json!("https://blossom.example"))).await;
        assert!(
            r[0]["error"].is_string(),
            "a server is never named: {}",
            r[0]
        );
        assert!(host.calls().is_empty());
    }
}
