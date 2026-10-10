//! NAP-MESH — hop-limited publish and subscribe over the mesh.
//!
//! Myco's own capability, in the registry's form (`docs/design/napplet/NAP-MESH.md`).
//! Where NAP-RELAY puts an event on the user's relays, NAP-MESH floods it to
//! the people around the user — Circle peers, over BLE, Wi-Fi Aware or the
//! LAN — with a hop budget, and lets a subscription reach out the same way for
//! backlog. The budget is the whole difference, and it is the one thing a
//! napplet may choose here that it may choose nowhere else.
//!
//! ## What the napplet does not get to choose
//!
//! Everything NAP-RELAY withholds — `pubkey`, `created_at`, `id`, `sig` — is
//! withheld here too, through the same template parser. On top of that the hop
//! budget is **clamped, never refused**: a request above the user's cap gets
//! the cap, and the result says what it got. A napplet cannot turn a phone
//! into an amplifier by asking, and it does not have to guess the cap to make a
//! working call.
//!
//! ## Why `eose` is local
//!
//! `mesh.eose` marks the end of *this device's* stored backlog. Peers' backlog
//! streams in afterwards as `mesh.event`, because a peer two hops away answers
//! in seconds, not milliseconds, and a napplet's other calls must not queue
//! behind that. The mesh is asynchronous; the wire says so rather than hiding
//! it behind a long wait.
//!
//! ## Why `blobs` answers with counts
//!
//! `mesh.blobs` tells a napplet whether the phones around it can serve a
//! file — an app store showing only apps that would actually load here. It
//! answers "how many peers hold it", never which ones: `mesh.info` exposes no
//! peer identity, and this does not start. It only asks (a `HEAD` per peer),
//! only over the mesh, and the asking is bounded by the shell, not by how
//! often the napplet calls.

use crate::dispatch::NapContext;
use crate::nap::relay::{event_json, filters_from, signed_or_template};
use crate::seams::{Envelope, MAX_BLOB_HASHES};
use crate::session::Session;

/// Handle an inbound `mesh.*` message.
pub async fn handle(ctx: &NapContext, session: &mut Session, message: &Envelope) -> Vec<Envelope> {
    match message.action() {
        "info" => vec![info(ctx, message).await],
        "publish" => vec![publish(ctx, session, message).await],
        "subscribe" => subscribe(ctx, session, message).await,
        "blobs" => vec![blobs(ctx, message).await],
        "close" => {
            if let Some(sub_id) = message.field("subId").and_then(|v| v.as_str()) {
                session.unsubscribe_in("mesh", sub_id);
            }
            Vec::new()
        }
        // Unrecognized within the domain: silent, per NIP-5D.
        _ => Vec::new(),
    }
}

/// `mesh.info` — whether the mesh is up, how many Circle peers are reachable,
/// and the user's caps. A napplet reads the caps to show what "everyone
/// nearby" means on this phone; it never needs them to make a call.
async fn info(ctx: &NapContext, message: &Envelope) -> Envelope {
    let limits = ctx.mesh.limits().await;
    let reach = ctx.mesh.reach().await.unwrap_or_default();
    message
        .to_result()
        .with_field("online", reach.online)
        .with_field("peers", reach.peers)
        .with_field(
            "limits",
            serde_json::json!({
                "publishTtl": limits.publish_ttl,
                "subscribeTtl": limits.subscribe_ttl,
            }),
        )
}

/// `mesh.publish` — sign the template as the user, store it, and flood it
/// `ttl` hops out. Given an event the napplet was delivered, keep it and flood
/// it again as it is — any time, seen before or not (NAP-LOCAL).
async fn publish(ctx: &NapContext, session: &Session, message: &Envelope) -> Envelope {
    let requested = match ttl_from(message) {
        Ok(ttl) => ttl,
        Err(e) => return failed(message, e),
    };
    let (signed, as_is) = match signed_or_template(ctx, session, message, true).await {
        Ok(event) => event,
        Err(e) => return failed(message, e),
    };

    let ttl = ctx.mesh.limits().await.clamp_publish(requested);
    let sent = if as_is {
        ctx.mesh.rebroadcast(signed.clone(), ttl).await
    } else {
        ctx.mesh.publish(signed.clone(), ttl).await
    };
    if let Err(e) = sent {
        return failed(message, format!("could not publish: {e}"));
    }

    let id = signed.id.to_hex();
    tracing::info!(kind = %signed.kind.as_u16(), event = %id, ttl, "napplet published to the mesh");
    message
        .to_result()
        .with_field("ok", true)
        .with_field("event", event_json(&signed))
        .with_field("eventId", id)
        .with_field("ttl", ttl)
}

/// `mesh.subscribe` — register the filters for live delivery, answer from the
/// local store, ask peers `ttl` hops out for theirs, and mark the local
/// backlog's end.
///
/// The filters are registered **before** anything is read, for the same
/// reason NAP-RELAY does it: an event landing between the two is delivered
/// live rather than lost in the gap. Peer backlog then arrives on that same
/// live path, which is what lets this call return at local-store speed.
async fn subscribe(ctx: &NapContext, session: &mut Session, message: &Envelope) -> Vec<Envelope> {
    let sub_id = match message.field("subId").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return vec![message.to_error("subscribe needs a subId")],
    };
    let requested = match ttl_from(message) {
        Ok(ttl) => ttl,
        Err(e) => return vec![message.to_error(e)],
    };
    let filters = match filters_from(message) {
        Ok(filters) => filters,
        Err(e) => return vec![message.to_error(e)],
    };

    // Peers are asked with the raw filters: hops ride the envelope, and the
    // filters stay canonical NIP-01 all the way out.
    let ttl = ctx.mesh.limits().await.clamp_subscribe(requested);
    // The local backlog's end follows the backlog, streamed or not.
    let eose = Envelope::new("mesh.eose")
        .with_field("subId", sub_id.clone())
        .with_field("ttl", ttl);
    let out =
        match crate::nap::open_subscription(ctx, session, "mesh", &sub_id, &filters, vec![eose])
            .await
        {
            Ok(backlog) => backlog,
            Err(reason) => {
                return vec![Envelope::new("mesh.closed")
                    .with_field("subId", sub_id)
                    .with_field("reason", reason)]
            }
        };

    if ttl > 0 {
        let raw = match message.field("filters") {
            Some(serde_json::Value::Array(items)) => items.clone(),
            Some(object) => vec![object.clone()],
            None => Vec::new(),
        };
        if let Err(e) = ctx.mesh.pull(raw, ttl).await {
            // The local backlog was already answered; a pull that could not
            // even start is worth a line, not a closed subscription.
            tracing::warn!(sub_id, error = %e, "mesh pull could not be started");
        }
    }

    out
}

/// `mesh.blobs` — how many reachable mesh peers hold each named blob.
///
/// The hashes are checked here, before anything is asked: each a sha256 in
/// hex (read case-blind, answered lowercase), at most [`MAX_BLOB_HASHES`] of
/// them. Too many is refused rather than cut short — a napplet that silently
/// got answers for the first 64 of 100 would read the rest as "nobody has
/// it".
async fn blobs(ctx: &NapContext, message: &Envelope) -> Envelope {
    let hashes = match hashes_from(message) {
        Ok(hashes) => hashes,
        Err(e) => return message.to_error(e),
    };
    let Some(reach) = ctx.mesh.blob_holders(&hashes).await else {
        return message.to_error("this shell cannot ask the mesh for blobs");
    };
    let mut blobs = serde_json::Map::new();
    for hash in &hashes {
        let held = reach.holders.get(hash).copied().unwrap_or(0);
        blobs.insert(hash.clone(), held.into());
    }
    message
        .to_result()
        .with_field("peers", reach.peers)
        .with_field("blobs", serde_json::Value::Object(blobs))
}

/// The `hashes` field: a list of sha256 hex strings, deduplicated in order.
fn hashes_from(message: &Envelope) -> Result<Vec<String>, String> {
    let Some(serde_json::Value::Array(items)) = message.field("hashes") else {
        return Err("blobs needs a list of sha256 hashes".to_string());
    };
    if items.len() > MAX_BLOB_HASHES {
        return Err(format!("at most {MAX_BLOB_HASHES} hashes per call"));
    }
    let mut hashes: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let hash = item
            .as_str()
            .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| format!("not a sha256 hash: {item}"))?
            .to_ascii_lowercase();
        if !hashes.contains(&hash) {
            hashes.push(hash);
        }
    }
    Ok(hashes)
}

/// The `mesh.event` frames a session should receive for an arriving event.
pub fn deliveries_for(session: &Session, event: &nostr::Event) -> Vec<Envelope> {
    crate::nap::deliveries_in(session, "mesh", event)
}

/// The optional `ttl` field. Absent means "the cap"; present, it must be a
/// small non-negative integer. Anything else is refused rather than read as
/// zero — a napplet that asked for reach and silently got none would have no
/// way to tell.
fn ttl_from(message: &Envelope) -> Result<Option<u8>, String> {
    match message.field("ttl") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(n)) => match n.as_u64() {
            Some(v) if v <= u8::MAX as u64 => Ok(Some(v as u8)),
            _ => Err("ttl must be a whole number of hops from 0 to 255".to_string()),
        },
        Some(_) => Err("ttl must be a whole number of hops".to_string()),
    }
}

/// A publish failure, in NAP-RELAY's shape: `ok` false beside the reason.
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
    use crate::seams::MeshLimits;
    use crate::session::NappletIdentity;
    use crate::testing::test_context_with_mesh;
    use nostr::{EventBuilder, Keys};
    use serde_json::json;

    fn granted() -> Session {
        let mut s = Session::new(NappletIdentity::new("chat", "aggregate"), ["relay", "mesh"]);
        s.on_ready();
        s
    }

    async fn call(ctx: &NapContext, session: &mut Session, envelope: Envelope) -> Vec<Envelope> {
        dispatch(ctx, session, &envelope).await.envelopes().to_vec()
    }

    fn template() -> serde_json::Value {
        json!({"kind": 20666, "content": "ding", "tags": [["t", "doorbell"]]})
    }

    #[tokio::test]
    async fn publishes_as_the_user_with_the_requested_hops() {
        let (ctx, mesh, _signer) = test_context_with_mesh(MeshLimits {
            publish_ttl: 3,
            subscribe_ttl: 2,
        });
        let mut s = granted();
        let out = call(
            &ctx,
            &mut s,
            Envelope::new("mesh.publish")
                .with_id("p1")
                .with_field("event", template())
                .with_field("ttl", 2),
        )
        .await;
        assert_eq!(out.len(), 1);
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["type"], "mesh.publish.result");
        assert_eq!(r["id"], "p1");
        assert_eq!(r["ok"], true);
        assert_eq!(r["ttl"], 2);
        assert_eq!(r["event"]["kind"], 20666);

        let published = mesh.published();
        assert_eq!(published.len(), 1);
        assert_eq!(
            published[0].1, 2,
            "the sink got a different budget than the napplet"
        );
        assert_eq!(published[0].0.id.to_hex(), r["eventId"]);
    }

    /// The cap is the user's, and it wins: asking for more gets the cap, and
    /// the result says so rather than pretending.
    #[tokio::test]
    async fn a_request_above_the_cap_is_clamped_and_reported() {
        let (ctx, mesh, _signer) = test_context_with_mesh(MeshLimits {
            publish_ttl: 1,
            subscribe_ttl: 0,
        });
        let mut s = granted();
        let out = call(
            &ctx,
            &mut s,
            Envelope::new("mesh.publish")
                .with_id("p1")
                .with_field("event", template())
                .with_field("ttl", 200),
        )
        .await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["ttl"], 1);
        assert_eq!(mesh.published()[0].1, 1);
    }

    /// No `ttl` means "as far as the user allows" — the working default, so a
    /// napplet need not know the cap to reach everyone nearby.
    #[tokio::test]
    async fn an_omitted_ttl_is_the_cap() {
        let (ctx, mesh, _signer) = test_context_with_mesh(MeshLimits {
            publish_ttl: 3,
            subscribe_ttl: 2,
        });
        let mut s = granted();
        let out = call(
            &ctx,
            &mut s,
            Envelope::new("mesh.publish")
                .with_id("p1")
                .with_field("event", template()),
        )
        .await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["ttl"], 3);
        assert_eq!(mesh.published()[0].1, 3);
    }

    /// A budget that is not a small whole number is refused, not read as zero.
    #[tokio::test]
    async fn a_malformed_ttl_is_refused() {
        let (ctx, mesh, _signer) = test_context_with_mesh(MeshLimits {
            publish_ttl: 3,
            subscribe_ttl: 2,
        });
        let mut s = granted();
        for bad in [json!(-1), json!("far"), json!(2.5), json!(300)] {
            let out = call(
                &ctx,
                &mut s,
                Envelope::new("mesh.publish")
                    .with_id("p1")
                    .with_field("event", template())
                    .with_field("ttl", bad.clone()),
            )
            .await;
            let r = serde_json::to_value(&out[0]).unwrap();
            assert_eq!(r["ok"], false, "accepted ttl {bad}");
            assert!(r["error"].is_string());
        }
        assert!(
            mesh.published().is_empty(),
            "a refused publish reached the sink"
        );
    }

    /// Same notary rules as NAP-RELAY: the template's author and time are
    /// ignored, the user's are used.
    #[tokio::test]
    async fn the_napplet_cannot_choose_the_author() {
        let (ctx, mesh, signer) = test_context_with_mesh(MeshLimits {
            publish_ttl: 3,
            subscribe_ttl: 2,
        });
        let mut s = granted();
        let imposter = Keys::generate().public_key().to_hex();
        let out = call(
            &ctx,
            &mut s,
            Envelope::new("mesh.publish").with_id("p1").with_field(
                "event",
                json!({"kind": 1, "content": "hi", "pubkey": imposter, "created_at": 1}),
            ),
        )
        .await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["ok"], true);
        let published = mesh.published();
        assert_eq!(published[0].0.pubkey, signer.public_key());
        assert_ne!(published[0].0.created_at.as_secs(), 1);
    }

    /// An event the napplet was delivered can be flooded again as it is — any
    /// number of times — and one it was not delivered cannot.
    #[tokio::test]
    async fn a_delivered_event_is_rebroadcast_as_it_is() {
        let (ctx, mesh, _signer) = test_context_with_mesh(MeshLimits {
            publish_ttl: 3,
            subscribe_ttl: 2,
        });
        let theirs = EventBuilder::text_note("worth passing on")
            .sign_with_keys(&Keys::generate())
            .unwrap();
        let unseen = EventBuilder::text_note("never shown")
            .sign_with_keys(&Keys::generate())
            .unwrap();
        ctx.relay.publish(theirs.clone()).await.unwrap();
        let mut s = granted();
        call(
            &ctx,
            &mut s,
            Envelope::new("relay.query")
                .with_id("q")
                .with_field("filters", json!({"ids": [theirs.id.to_hex()]})),
        )
        .await;

        for n in 0..2 {
            let out = call(
                &ctx,
                &mut s,
                Envelope::new("mesh.publish")
                    .with_id(format!("r{n}"))
                    .with_field("event", event_json(&theirs))
                    .with_field("ttl", 2),
            )
            .await;
            let r = serde_json::to_value(&out[0]).unwrap();
            assert_eq!(r["ok"], true, "{r}");
        }
        let passed_on = mesh.rebroadcasts();
        assert_eq!(passed_on.len(), 2);
        assert!(passed_on
            .iter()
            .all(|(e, ttl)| e.id == theirs.id && *ttl == 2));
        assert!(
            mesh.published().is_empty(),
            "passed on through the publish door"
        );

        let out = call(
            &ctx,
            &mut s,
            Envelope::new("mesh.publish")
                .with_id("x")
                .with_field("event", event_json(&unseen)),
        )
        .await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["ok"], false);
        assert_eq!(mesh.rebroadcasts().len(), 2);
    }

    #[tokio::test]
    async fn subscribe_answers_locally_then_asks_peers_then_eose() {
        let (ctx, mesh, _signer) = test_context_with_mesh(MeshLimits {
            publish_ttl: 3,
            subscribe_ttl: 2,
        });
        let keys = Keys::generate();
        let stored = EventBuilder::new(nostr::Kind::from(20666u16), "earlier")
            .sign_with_keys(&keys)
            .unwrap();
        ctx.relay.publish(stored.clone()).await.unwrap();

        let mut s = granted();
        let out = call(
            &ctx,
            &mut s,
            Envelope::new("mesh.subscribe")
                .with_id("s1")
                .with_field("subId", "sub-1")
                .with_field("filters", json!([{"kinds": [20666]}]))
                .with_field("ttl", 1),
        )
        .await;
        assert_eq!(out.len(), 2);
        let first = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(first["type"], "mesh.event");
        assert_eq!(first["subId"], "sub-1");
        assert_eq!(first["result"]["event"]["id"], stored.id.to_hex());
        let last = serde_json::to_value(&out[1]).unwrap();
        assert_eq!(
            last,
            json!({"type": "mesh.eose", "subId": "sub-1", "ttl": 1})
        );

        let pulls = mesh.pulled();
        assert_eq!(pulls.len(), 1);
        assert_eq!(pulls[0].1, 1);
        assert_eq!(pulls[0].0, vec![json!({"kinds": [20666]})]);
        assert_eq!(s.subscription_count(), 1);
    }

    /// Zero hops is a local subscription: nobody is asked.
    #[tokio::test]
    async fn a_zero_hop_subscribe_asks_no_peer() {
        let (ctx, mesh, _signer) = test_context_with_mesh(MeshLimits {
            publish_ttl: 3,
            subscribe_ttl: 2,
        });
        let mut s = granted();
        let out = call(
            &ctx,
            &mut s,
            Envelope::new("mesh.subscribe")
                .with_id("s1")
                .with_field("subId", "sub-1")
                .with_field("filters", json!({"kinds": [20666]}))
                .with_field("ttl", 0),
        )
        .await;
        let last = serde_json::to_value(out.last().unwrap()).unwrap();
        assert_eq!(last["ttl"], 0);
        assert!(mesh.pulled().is_empty());
    }

    /// The subscribe cap is separate from the publish cap, and lower by
    /// default — a flooded read costs more than a flooded write.
    #[tokio::test]
    async fn the_subscribe_cap_is_its_own() {
        let (ctx, mesh, _signer) = test_context_with_mesh(MeshLimits {
            publish_ttl: 3,
            subscribe_ttl: 1,
        });
        let mut s = granted();
        let out = call(
            &ctx,
            &mut s,
            Envelope::new("mesh.subscribe")
                .with_id("s1")
                .with_field("subId", "sub-1")
                .with_field("filters", json!({"kinds": [20666]}))
                .with_field("ttl", 3),
        )
        .await;
        let last = serde_json::to_value(out.last().unwrap()).unwrap();
        assert_eq!(last["ttl"], 1);
        assert_eq!(mesh.pulled()[0].1, 1);
    }

    /// An event arriving later — published here or carried in from a peer —
    /// reaches a live mesh subscription as `mesh.event`, and a relay
    /// subscription with the same id is not confused with it.
    #[tokio::test]
    async fn live_events_are_delivered_to_mesh_subscriptions() {
        let (ctx, _mesh, _signer) = test_context_with_mesh(MeshLimits {
            publish_ttl: 3,
            subscribe_ttl: 2,
        });
        let mut s = granted();
        call(
            &ctx,
            &mut s,
            Envelope::new("mesh.subscribe")
                .with_id("s1")
                .with_field("subId", "sub-1")
                .with_field("filters", json!({"kinds": [20666]}))
                .with_field("ttl", 0),
        )
        .await;
        call(
            &ctx,
            &mut s,
            Envelope::new("relay.subscribe")
                .with_id("s2")
                .with_field("subId", "sub-1")
                .with_field("filters", json!({"kinds": [1]})),
        )
        .await;

        let keys = Keys::generate();
        let doorbell = EventBuilder::new(nostr::Kind::from(20666u16), "ding")
            .sign_with_keys(&keys)
            .unwrap();
        let frames = crate::nap::deliveries_for(&s, &doorbell);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].msg_type, "mesh.event");
        assert_eq!(frames[0].field("subId").unwrap(), "sub-1");

        let note = EventBuilder::text_note("hi").sign_with_keys(&keys).unwrap();
        let frames = crate::nap::deliveries_for(&s, &note);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].msg_type, "relay.event");

        // Closing the mesh one leaves the relay one alone.
        call(
            &ctx,
            &mut s,
            Envelope::new("mesh.close").with_field("subId", "sub-1"),
        )
        .await;
        let again = EventBuilder::new(nostr::Kind::from(20666u16), "dong")
            .sign_with_keys(&keys)
            .unwrap();
        assert!(crate::nap::deliveries_for(&s, &again).is_empty());
        // A new note (a subscription is handed each event once).
        let another = EventBuilder::text_note("hi again")
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(crate::nap::deliveries_for(&s, &another).len(), 1);
    }

    /// Revoking `mesh` stops deliveries on the next event, as with `relay`.
    #[tokio::test]
    async fn an_ungranted_napplet_receives_no_mesh_deliveries() {
        let mut s = Session::new(NappletIdentity::new("chat", "aggregate"), ["relay"]);
        s.on_ready();
        s.subscribe_in(
            "mesh",
            "sub-1",
            vec![nostr::Filter::new().kind(nostr::Kind::from(20666u16))],
        )
        .unwrap();
        let keys = Keys::generate();
        let doorbell = EventBuilder::new(nostr::Kind::from(20666u16), "ding")
            .sign_with_keys(&keys)
            .unwrap();
        assert!(deliveries_for(&s, &doorbell).is_empty());
    }

    #[tokio::test]
    async fn info_reports_reach_and_caps() {
        let (ctx, mesh, _signer) = test_context_with_mesh(MeshLimits {
            publish_ttl: 3,
            subscribe_ttl: 2,
        });
        mesh.set_reach(true, 4);
        let mut s = granted();
        let out = call(&ctx, &mut s, Envelope::new("mesh.info").with_id("i1")).await;
        assert_eq!(
            serde_json::to_value(&out[0]).unwrap(),
            json!({
                "type": "mesh.info.result",
                "id": "i1",
                "online": true,
                "peers": 4,
                "limits": {"publishTtl": 3, "subscribeTtl": 2},
            })
        );
    }

    /// The permission boundary, for this domain: implemented and offered, yet
    /// refused on the call when it was not granted.
    #[tokio::test]
    async fn an_ungranted_napplet_cannot_publish_to_the_mesh() {
        let (ctx, mesh, _signer) = test_context_with_mesh(MeshLimits {
            publish_ttl: 3,
            subscribe_ttl: 2,
        });
        let mut s = Session::new(NappletIdentity::new("chat", "aggregate"), ["relay"]);
        s.on_ready();
        let out = call(
            &ctx,
            &mut s,
            Envelope::new("mesh.publish")
                .with_id("p1")
                .with_field("event", template()),
        )
        .await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["type"], "mesh.publish.result");
        assert!(r["error"].as_str().unwrap().contains("not granted"));
        assert!(mesh.published().is_empty());
    }

    const HELD: &str = "b1674191a88ec5cdd733e4240a81803105dc412d6c6708d53ab94fc248f4f553";
    const MISSING: &str = "0000000000000000000000000000000000000000000000000000000000000001";

    fn blobs_call(hashes: serde_json::Value) -> Envelope {
        Envelope::new("mesh.blobs")
            .with_id("b1")
            .with_field("hashes", hashes)
    }

    fn blobs_ctx() -> (NapContext, std::sync::Arc<crate::testing::MemMesh>) {
        let (ctx, mesh, _signer) = test_context_with_mesh(MeshLimits {
            publish_ttl: 3,
            subscribe_ttl: 2,
        });
        mesh.set_blob_holders(3, &[(HELD, 2)]);
        (ctx, mesh)
    }

    /// A blob two of three peers hold is counted, one nobody holds is zero,
    /// and the hash is answered as asked — lowercase, under its own key.
    #[tokio::test]
    async fn blobs_counts_the_peers_that_hold_each_hash() {
        let (ctx, mesh) = blobs_ctx();
        let mut s = granted();
        let out = call(
            &ctx,
            &mut s,
            blobs_call(json!([HELD.to_uppercase(), MISSING, HELD])),
        )
        .await;
        assert_eq!(
            serde_json::to_value(&out[0]).unwrap(),
            json!({
                "type": "mesh.blobs.result",
                "id": "b1",
                "peers": 3,
                "blobs": {HELD: 2, MISSING: 0},
            })
        );
        // Deduplicated before the seam: each blob is asked about once.
        assert_eq!(
            mesh.blob_asks(),
            vec![vec![HELD.to_string(), MISSING.to_string()]]
        );
    }

    /// A miss is a zero, not an error and not a missing key.
    #[tokio::test]
    async fn blobs_reports_a_miss_as_zero() {
        let (ctx, _mesh) = blobs_ctx();
        let mut s = granted();
        let out = call(&ctx, &mut s, blobs_call(json!([MISSING]))).await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["blobs"], json!({MISSING: 0}));
        assert!(r.get("error").is_none());
    }

    /// More than the cap is refused whole, before any peer is asked.
    #[tokio::test]
    async fn blobs_refuses_more_than_the_cap() {
        let (ctx, mesh) = blobs_ctx();
        let mut s = granted();
        let many: Vec<String> = (0..=MAX_BLOB_HASHES).map(|i| format!("{i:064x}")).collect();
        let out = call(&ctx, &mut s, blobs_call(json!(many))).await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["type"], "mesh.blobs.result");
        assert!(r["error"].as_str().unwrap().contains("at most 64"));

        let at_cap: Vec<String> = (0..MAX_BLOB_HASHES).map(|i| format!("{i:064x}")).collect();
        let out = call(&ctx, &mut s, blobs_call(json!(at_cap))).await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["blobs"].as_object().unwrap().len(), MAX_BLOB_HASHES);
        assert_eq!(
            mesh.blob_asks().len(),
            1,
            "the refused call reached the seam"
        );
    }

    /// Anything that is not a list of sha256 hashes is refused, not guessed at.
    #[tokio::test]
    async fn blobs_refuses_what_is_not_a_hash() {
        let (ctx, mesh) = blobs_ctx();
        let mut s = granted();
        for bad in [
            json!(HELD),
            json!(["abc"]),
            json!([format!("{}zz", &HELD[..62])]),
            json!([7]),
            json!(["https://blossom.example/".to_string() + HELD]),
        ] {
            let out = call(&ctx, &mut s, blobs_call(bad.clone())).await;
            let r = serde_json::to_value(&out[0]).unwrap();
            assert!(r["error"].is_string(), "accepted {bad}");
        }
        let out = call(&ctx, &mut s, Envelope::new("mesh.blobs").with_id("b1")).await;
        assert!(serde_json::to_value(&out[0]).unwrap()["error"].is_string());
        assert!(mesh.blob_asks().is_empty());
    }

    /// Same grant as every other NAP-MESH call: without `mesh`, refused, and
    /// nobody is asked.
    #[tokio::test]
    async fn blobs_needs_the_mesh_grant() {
        let (ctx, mesh) = blobs_ctx();
        let mut s = Session::new(NappletIdentity::new("store", "aggregate"), ["relay"]);
        s.on_ready();
        let out = call(&ctx, &mut s, blobs_call(json!([HELD]))).await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(r["type"], "mesh.blobs.result");
        assert!(r["error"].as_str().unwrap().contains("not granted"));
        assert!(mesh.blob_asks().is_empty());
    }

    /// Asking who holds a blob is not fetching it: the blob fetcher (the path
    /// that reaches the internet) is never asked, and nothing is stored.
    #[tokio::test]
    async fn blobs_fetches_nothing() {
        let (mut ctx, _mesh) = blobs_ctx();
        let fetcher = std::sync::Arc::new(crate::testing::MemFetcher::default());
        fetcher.hold(b"index.html bytes");
        ctx.fetcher = fetcher.clone();
        let mut s = granted();
        let out = call(&ctx, &mut s, blobs_call(json!([HELD, MISSING]))).await;
        assert!(serde_json::to_value(&out[0]).unwrap()["error"].is_null());
        assert!(fetcher.asked().is_empty(), "mesh.blobs fetched a body");
        assert!(!ctx.blobs.has(HELD).await);
        assert!(!ctx.blobs.has(MISSING).await);
    }

    /// A shell that cannot ask says so, rather than answering "nobody".
    #[tokio::test]
    async fn blobs_without_a_prober_is_an_error_not_zero() {
        let (ctx, _mesh, _signer) = test_context_with_mesh(MeshLimits {
            publish_ttl: 3,
            subscribe_ttl: 2,
        });
        let mut s = granted();
        let out = call(&ctx, &mut s, blobs_call(json!([HELD]))).await;
        let r = serde_json::to_value(&out[0]).unwrap();
        assert!(r["error"].is_string());
        assert!(r.get("blobs").is_none());
    }
}
