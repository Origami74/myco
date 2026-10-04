//! NAP-IDENTITY — read-only queries about the user.
//!
//! Napplets learn *about* the user; they never act *as* the user through this
//! domain. There is no signing, no encryption and no key material here: acting
//! as the user goes through NAP-RELAY, where the runtime signs and the napplet
//! only asks.
//!
//! The identity reported is the **user key** (D3), not the mesh device key.
//! A napplet learns who someone is socially, never which hardware they are on.
//!
//! This is also distinct from the napplet's own session identity, which the
//! runtime assigns from verified bytes and the napplet never negotiates.

use nostr::{Filter, Kind};

use crate::dispatch::NapContext;
use crate::seams::Envelope;

/// Handle an inbound `identity.*` message.
pub async fn handle(ctx: &NapContext, message: &Envelope) -> Vec<Envelope> {
    match message.action() {
        "getPublicKey" => vec![get_public_key(ctx, message).await],
        "getProfile" => vec![get_profile(ctx, message).await],
        "getRelays" => vec![get_relays(ctx, message).await],
        "getFollows" => vec![get_p_tags(ctx, message, Kind::ContactList).await],
        "getMutes" => vec![get_p_tags(ctx, message, Kind::MuteList).await],
        // Defined by the NAP but not answered yet. An empty answer, in the
        // field the spec names for each, is the truthful one for a runtime
        // with nothing to say — and the shape the spec gives for "nothing
        // found", so a napplet handles it on a path it already has. A
        // generic `result` field would resolve the napplet's promise to
        // `undefined`, which it cannot iterate.
        "getBlocked" => vec![empty(message, "pubkeys")],
        "getList" => vec![empty(message, "entries")],
        "getZaps" => vec![empty(message, "zaps")],
        "getBadges" => vec![empty(message, "badges")],
        // Not an identity query at all: unrecognized, so silent (NIP-5D).
        _ => Vec::new(),
    }
}

/// The user's hex public key.
///
/// The one method every NAP-IDENTITY runtime must answer. An empty string means
/// no signer is connected, which is the spec's shape for "nobody is here" — so
/// a signer that cannot be reached reports absence rather than an error a
/// napplet would have to special-case.
async fn get_public_key(ctx: &NapContext, message: &Envelope) -> Envelope {
    let pubkey = match ctx.signer.public_key().await {
        Ok(pk) => pk.to_hex(),
        Err(_) => String::new(),
    };
    message.to_result().with_field("pubkey", pubkey)
}

/// The push NAP-IDENTITY requires when the shell-user changes: a hex pubkey
/// on login, `""` on logout.
pub fn changed(pubkey_hex: &str) -> Envelope {
    Envelope::new("identity.changed").with_field("pubkey", pubkey_hex)
}

fn empty(message: &Envelope, field: &str) -> Envelope {
    message
        .to_result()
        .with_field(field, serde_json::Value::Array(Vec::new()))
}

/// The `p` tags of the user's newest `kind` list — follows (3) or mutes
/// (10000) — as `pubkeys`, from the local store. Empty when logged out or
/// when this device has no such list.
async fn get_p_tags(ctx: &NapContext, message: &Envelope, kind: Kind) -> Envelope {
    let Ok(pubkey) = ctx.signer.public_key().await else {
        return empty(message, "pubkeys");
    };
    let filter = Filter::new().kind(kind).author(pubkey).limit(1);
    let newest = ctx
        .relay
        .query(&[filter])
        .await
        .ok()
        .and_then(|events| events.into_iter().max_by_key(|e| e.created_at));
    let mut seen = std::collections::HashSet::new();
    let pubkeys: Vec<serde_json::Value> = newest
        .map(|event| {
            event
                .tags
                .iter()
                .filter_map(|t| match t.as_slice() {
                    [k, v, ..] if k == "p" && v.len() == 64 => Some(v.to_ascii_lowercase()),
                    _ => None,
                })
                .filter(|v| seen.insert(v.clone()))
                .map(serde_json::Value::String)
                .collect()
        })
        .unwrap_or_default();
    message
        .to_result()
        .with_field("pubkeys", serde_json::Value::Array(pubkeys))
}

/// The user's kind 0, or `null` when there is none.
async fn get_profile(ctx: &NapContext, message: &Envelope) -> Envelope {
    let Ok(pubkey) = ctx.signer.public_key().await else {
        return message
            .to_result()
            .with_field("profile", serde_json::Value::Null);
    };

    let filter = Filter::new().kind(Kind::Metadata).author(pubkey).limit(1);
    let newest = ctx
        .relay
        .query(&[filter])
        .await
        .ok()
        .and_then(|events| events.into_iter().max_by_key(|e| e.created_at));

    // A kind 0's content is author-written JSON. Anything unparseable is
    // reported as no profile rather than passed along, so a napplet is never
    // handed something that is not a profile object.
    let profile = newest
        .and_then(|event| serde_json::from_str::<serde_json::Value>(&event.content).ok())
        .filter(|v| v.is_object())
        .unwrap_or(serde_json::Value::Null);

    message.to_result().with_field("profile", profile)
}

/// The user's NIP-65 relay list, in the NIP-07 shape:
/// `{ "<url>": { "read": bool, "write": bool } }`, `{}` when there is none.
///
/// Read from the user's newest kind 10002 — the one the runtime publishes at
/// first use and `outbox.resolveRelays` plans by — so a napplet that asks
/// here first sees the same relays the outbox will use, not an empty map
/// that says the user has none. An `r` tag with no marker is read and
/// write; `read` or `write` narrows it to that side.
async fn get_relays(ctx: &NapContext, message: &Envelope) -> Envelope {
    let Ok(pubkey) = ctx.signer.public_key().await else {
        return message
            .to_result()
            .with_field("relays", serde_json::json!({}));
    };

    let filter = Filter::new().kind(Kind::RelayList).author(pubkey).limit(1);
    let newest = ctx
        .relay
        .query(&[filter])
        .await
        .ok()
        .and_then(|events| events.into_iter().max_by_key(|e| e.created_at));

    let mut relays = serde_json::Map::new();
    if let Some(event) = newest {
        for tag in event.tags.iter() {
            let parts = tag.as_slice();
            if parts.first().map(String::as_str) != Some("r") {
                continue;
            }
            let Some(url) = parts.get(1).filter(|u| !u.is_empty()) else {
                continue;
            };
            let (read, write) = match parts.get(2).map(String::as_str) {
                Some("read") => (true, false),
                Some("write") => (false, true),
                _ => (true, true),
            };
            relays.insert(
                url.clone(),
                serde_json::json!({ "read": read, "write": write }),
            );
        }
    }

    message
        .to_result()
        .with_field("relays", serde_json::Value::Object(relays))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{dispatch, NapContext};
    use crate::session::{NappletIdentity, Session};
    use crate::testing::{test_context, AbsentSigner, TestSigner};
    use nostr::{EventBuilder, Keys};
    use nsite_deck::seams::RelayBackend;
    use nsite_deck::testing::MemRelay;
    use std::sync::Arc;

    fn granted_session() -> Session {
        let mut s = Session::new(NappletIdentity::new("chat", "aggregate"), ["identity"]);
        s.on_ready();
        s
    }

    async fn call(ctx: &NapContext, action: &str) -> Envelope {
        let mut session = granted_session();
        let msg = Envelope::new(format!("identity.{action}")).with_id("i1");
        let replies = dispatch(ctx, &mut session, &msg).await.envelopes().to_vec();
        assert_eq!(replies.len(), 1, "{action} gave no answer");
        replies[0].clone()
    }

    /// The one method every NAP-IDENTITY runtime must answer.
    #[tokio::test]
    async fn reports_the_user_key() {
        let (ctx, signer) = test_context();
        let reply = call(&ctx, "getPublicKey").await;

        assert_eq!(reply.msg_type, "identity.getPublicKey.result");
        assert_eq!(reply.id.as_deref(), Some("i1"));
        assert_eq!(
            reply.field("pubkey").unwrap().as_str().unwrap(),
            signer.public_key().to_hex()
        );
    }

    /// A device that has never run a napplet has no user key. The spec's shape
    /// for "nobody is here" is an empty string, so absence is reported on the
    /// path a napplet already handles rather than as an error it must
    /// special-case.
    #[tokio::test]
    async fn no_user_key_reads_as_nobody_rather_than_an_error() {
        let relay: Arc<dyn nsite_deck::seams::RelayBackend> = Arc::new(MemRelay::new());
        let ctx = NapContext {
            signer: Arc::new(AbsentSigner),
            relay: relay.clone(),
            sink: Arc::new(crate::seams::StoreOnlySink(relay.clone())),
            mesh: Arc::new(crate::testing::MemMesh::new(
                relay.clone(),
                crate::seams::MeshLimits {
                    publish_ttl: 3,
                    subscribe_ttl: 2,
                },
            )),
            outbox: Arc::new(crate::testing::OutboxFixture::new(relay.clone())),
            lanes: Arc::new(crate::testing::OutboxFixture::new(relay)),
            blobs: Arc::new(nsite_deck::testing::MemBlobs::new()),
            kept_blobs: Arc::new(nsite_deck::testing::MemBlobs::new()),
            fetcher: Arc::new(crate::seams::NoFetcher),
            intents: Arc::new(crate::nap::intent::NoIntents),
            uploads: Arc::new(crate::seams::NoUploads),
        };
        let reply = call(&ctx, "getPublicKey").await;
        assert_eq!(reply.field("pubkey").unwrap().as_str().unwrap(), "");
        assert!(reply.field("error").is_none(), "absence is not a failure");
    }

    /// The spec names the field `pubkey`; the reference shim reads exactly
    /// that, and a napplet handed `publicKey` sees nobody logged in.
    #[tokio::test]
    async fn the_public_key_travels_in_the_field_the_spec_names() {
        let (ctx, _) = test_context();
        let reply = call(&ctx, "getPublicKey").await;
        assert!(reply.field("pubkey").is_some());
        assert!(reply.field("publicKey").is_none());
        assert_eq!(changed("ab").field("pubkey").unwrap().as_str(), Some("ab"));
        assert_eq!(changed("").msg_type, "identity.changed");
    }

    #[tokio::test]
    async fn follows_come_from_the_users_newest_contact_list() {
        let keys = Keys::generate();
        let relay = Arc::new(MemRelay::new());
        let friend = Keys::generate().public_key();
        let contacts = EventBuilder::new(Kind::ContactList, "")
            .tags([
                nostr::Tag::public_key(friend),
                nostr::Tag::public_key(friend),
            ])
            .sign_with_keys(&keys)
            .unwrap();
        relay.publish(contacts).await.unwrap();
        let (mut ctx, _) = test_context();
        ctx.signer = Arc::new(TestSigner::with_keys(keys));
        ctx.relay = relay;

        let reply = call(&ctx, "getFollows").await;
        assert_eq!(
            reply.field("pubkeys").unwrap(),
            &serde_json::json!([friend.to_hex()])
        );
        let mutes = call(&ctx, "getMutes").await;
        assert_eq!(mutes.field("pubkeys").unwrap(), &serde_json::json!([]));
    }

    /// Every list query answers in the spec's field — an empty list when
    /// there is nothing to say — never a generic `result` a napplet cannot
    /// iterate.
    #[tokio::test]
    async fn identity_queries_use_the_spec_field_names() {
        let (ctx, _) = test_context();
        for (action, field) in [
            ("getFollows", "pubkeys"),
            ("getBlocked", "pubkeys"),
            ("getList", "entries"),
            ("getZaps", "zaps"),
            ("getBadges", "badges"),
        ] {
            let reply = call(&ctx, action).await;
            assert_eq!(
                reply.field(field),
                Some(&serde_json::json!([])),
                "{action} answered {reply:?}"
            );
            assert!(reply.field("result").is_none());
        }
    }

    #[tokio::test]
    async fn reads_the_users_profile_from_the_relay() {
        let keys = Keys::generate();
        let relay = Arc::new(MemRelay::new());
        let profile = EventBuilder::metadata(&nostr::Metadata::new().name("Myco Guest 01234"))
            .sign_with_keys(&keys)
            .unwrap();
        relay.publish(profile).await.unwrap();

        let ctx = NapContext {
            signer: Arc::new(TestSigner::with_keys(keys)),
            relay: relay.clone(),
            sink: Arc::new(crate::seams::StoreOnlySink(relay.clone())),
            mesh: Arc::new(crate::testing::MemMesh::new(
                relay.clone(),
                crate::seams::MeshLimits {
                    publish_ttl: 3,
                    subscribe_ttl: 2,
                },
            )),
            outbox: Arc::new(crate::testing::OutboxFixture::new(relay.clone())),
            lanes: Arc::new(crate::testing::OutboxFixture::new(relay)),
            blobs: Arc::new(nsite_deck::testing::MemBlobs::new()),
            kept_blobs: Arc::new(nsite_deck::testing::MemBlobs::new()),
            fetcher: Arc::new(crate::seams::NoFetcher),
            intents: Arc::new(crate::nap::intent::NoIntents),
            uploads: Arc::new(crate::seams::NoUploads),
        };
        let reply = call(&ctx, "getProfile").await;
        assert_eq!(reply.field("profile").unwrap()["name"], "Myco Guest 01234");
    }

    /// `getRelays` answers the user's kind 10002 in the NIP-07 shape, and
    /// `{}` when none was published — L7 of the PR #52 review, where it
    /// always answered `{}` and a napplet concluded the user had no relays.
    #[tokio::test]
    async fn get_relays_reflects_the_users_relay_list() {
        let keys = Keys::generate();
        let relay = Arc::new(MemRelay::new());
        let ctx = NapContext {
            signer: Arc::new(TestSigner::with_keys(keys.clone())),
            relay: relay.clone(),
            sink: Arc::new(crate::seams::StoreOnlySink(relay.clone())),
            mesh: Arc::new(crate::testing::MemMesh::new(
                relay.clone(),
                crate::seams::MeshLimits {
                    publish_ttl: 3,
                    subscribe_ttl: 2,
                },
            )),
            outbox: Arc::new(crate::testing::OutboxFixture::new(relay.clone())),
            lanes: Arc::new(crate::testing::OutboxFixture::new(relay.clone())),
            blobs: Arc::new(nsite_deck::testing::MemBlobs::new()),
            kept_blobs: Arc::new(nsite_deck::testing::MemBlobs::new()),
            fetcher: Arc::new(crate::seams::NoFetcher),
            intents: Arc::new(crate::nap::intent::NoIntents),
            uploads: Arc::new(crate::seams::NoUploads),
        };

        let reply = call(&ctx, "getRelays").await;
        assert_eq!(reply.field("relays").unwrap(), &serde_json::json!({}));

        let list = EventBuilder::new(nostr::Kind::RelayList, "")
            .tags([
                nostr::Tag::parse(["r", "wss://a"]).unwrap(),
                nostr::Tag::parse(["r", "wss://b", "read"]).unwrap(),
            ])
            .sign_with_keys(&keys)
            .unwrap();
        relay.publish(list).await.unwrap();
        // Somebody else's list is not the user's.
        let other = EventBuilder::new(nostr::Kind::RelayList, "")
            .tags([nostr::Tag::parse(["r", "wss://theirs"]).unwrap()])
            .sign_with_keys(&Keys::generate())
            .unwrap();
        relay.publish(other).await.unwrap();

        let reply = call(&ctx, "getRelays").await;
        assert_eq!(
            reply.field("relays").unwrap(),
            &serde_json::json!({
                "wss://a": { "read": true, "write": true },
                "wss://b": { "read": true, "write": false },
            })
        );
    }

    #[tokio::test]
    async fn no_profile_reads_as_null() {
        let (ctx, _signer) = test_context();
        let reply = call(&ctx, "getProfile").await;
        assert!(reply.field("profile").unwrap().is_null());
    }

    /// A kind 0's content is author-written text. Anything that is not a
    /// profile object is reported as no profile, so a napplet is never handed
    /// something it would have to guess about.
    #[tokio::test]
    async fn a_malformed_profile_reads_as_none() {
        let keys = Keys::generate();
        let relay = Arc::new(MemRelay::new());
        let junk = EventBuilder::new(nostr::Kind::Metadata, "not json at all")
            .sign_with_keys(&keys)
            .unwrap();
        relay.publish(junk).await.unwrap();

        let ctx = NapContext {
            signer: Arc::new(TestSigner::with_keys(keys)),
            relay: relay.clone(),
            sink: Arc::new(crate::seams::StoreOnlySink(relay.clone())),
            mesh: Arc::new(crate::testing::MemMesh::new(
                relay.clone(),
                crate::seams::MeshLimits {
                    publish_ttl: 3,
                    subscribe_ttl: 2,
                },
            )),
            outbox: Arc::new(crate::testing::OutboxFixture::new(relay.clone())),
            lanes: Arc::new(crate::testing::OutboxFixture::new(relay)),
            blobs: Arc::new(nsite_deck::testing::MemBlobs::new()),
            kept_blobs: Arc::new(nsite_deck::testing::MemBlobs::new()),
            fetcher: Arc::new(crate::seams::NoFetcher),
            intents: Arc::new(crate::nap::intent::NoIntents),
            uploads: Arc::new(crate::seams::NoUploads),
        };
        assert!(call(&ctx, "getProfile")
            .await
            .field("profile")
            .unwrap()
            .is_null());
    }

    /// Identity is read-only: nothing here signs, and there is no method that
    /// would hand over key material.
    #[tokio::test]
    async fn there_is_no_way_to_reach_the_key() {
        let (ctx, _signer) = test_context();
        let mut session = granted_session();
        for action in ["getPrivateKey", "sign", "nip04Encrypt", "getSecretKey"] {
            let msg = Envelope::new(format!("identity.{action}")).with_id("x");
            let replies = dispatch(&ctx, &mut session, &msg)
                .await
                .envelopes()
                .to_vec();
            assert!(replies.is_empty(), "{action} was answered");
        }
    }

    /// An ungranted napplet still has `window.napplet.identity` — and still
    /// gets refused when it calls.
    #[tokio::test]
    async fn the_grant_is_checked_on_the_call() {
        let (ctx, _signer) = test_context();
        let mut session = Session::new(
            NappletIdentity::new("chat", "aggregate"),
            Vec::<String>::new(),
        );
        session.on_ready();

        assert!(
            session.offers("identity"),
            "the API is available to everyone"
        );
        let msg = Envelope::new("identity.getPublicKey").with_id("i1");
        let replies = dispatch(&ctx, &mut session, &msg)
            .await
            .envelopes()
            .to_vec();
        assert_eq!(replies.len(), 1);
        assert!(replies[0].field("error").is_some());
        assert!(
            replies[0].field("pubkey").is_none(),
            "a refusal leaked the key"
        );
    }
}
