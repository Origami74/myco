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
        // Every other query is defined by the NAP but not answered yet. An
        // empty answer is the truthful one for a runtime with nothing to say —
        // and it is the shape the spec gives for "nothing found", so a napplet
        // handles it on a path it already has.
        "getRelays" => vec![message
            .to_result()
            .with_field("relays", serde_json::json!({}))],
        "getFollows" | "getMutes" | "getBlocked" | "getBadges" | "getZaps" | "getList" => {
            vec![message
                .to_result()
                .with_field("result", serde_json::Value::Array(Vec::new()))]
        }
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
    message.to_result().with_field("publicKey", pubkey)
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
            reply.field("publicKey").unwrap().as_str().unwrap(),
            signer.public_key().to_hex()
        );
    }

    /// A device that has never run a napplet has no user key. The spec's shape
    /// for "nobody is here" is an empty string, so absence is reported on the
    /// path a napplet already handles rather than as an error it must
    /// special-case.
    #[tokio::test]
    async fn no_user_key_reads_as_nobody_rather_than_an_error() {
        let ctx = NapContext {
            signer: Arc::new(AbsentSigner),
            relay: Arc::new(MemRelay::new()),
        };
        let reply = call(&ctx, "getPublicKey").await;
        assert_eq!(reply.field("publicKey").unwrap().as_str().unwrap(), "");
        assert!(reply.field("error").is_none(), "absence is not a failure");
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
            relay,
        };
        let reply = call(&ctx, "getProfile").await;
        assert_eq!(reply.field("profile").unwrap()["name"], "Myco Guest 01234");
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
            relay,
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
            replies[0].field("publicKey").is_none(),
            "a refusal leaked the key"
        );
    }
}
