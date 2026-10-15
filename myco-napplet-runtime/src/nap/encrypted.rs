//! Private events the user keeps for themselves: NIP-44, encrypted to the
//! user's own key.
//!
//! `relay.publishEncrypted` takes a template with plaintext `content`; the
//! runtime encrypts it to the user and signs it. The napplet never holds a
//! key. Relays and the mesh carry the result like any other event: what is
//! private is the content, not where it travels.
//!
//! Reading it back, the runtime decrypts an event for a napplet when it is
//! the user's own event, its signature verifies, and its content is a NIP-44
//! payload the user encrypted to themselves. Any kind, any napplet that
//! reads events (`relay`, `outbox`, `mesh`), whichever app wrote it:
//! NAP-RELAY's "the shell MUST decrypt incoming encrypted events; napplets
//! receive plaintext content". Which kinds a protocol keeps
//! private (NIP-78 app data, say) is that protocol's business, not the
//! shell's. That includes the private halves of the user's NIP-51 lists.
//!
//! Recipients are the user only, for now. Encrypting to someone else, and
//! decrypting what others encrypted to the user (DMs), are a different
//! feature with their own rules for who may read what.

use base64::Engine as _;
use nostr::PublicKey;

use crate::seams::{Envelope, Signer};

/// Whether `content` has the shape of a NIP-44 v2 payload: base64 of a
/// version byte 2, a nonce, at least one padded block and a MAC.
///
/// A cheap gate in front of the signer. Without it every note the user ever
/// wrote would cost a decrypt attempt, and with a signer app each attempt is
/// a round trip.
fn looks_like_nip44(content: &str) -> bool {
    // 1 version + 32 nonce + 32 minimum padded plaintext (+2 length) + 32 MAC.
    const MIN_DECODED: usize = 99;
    if content.len() < 132 || content.starts_with('#') {
        return false;
    }
    match base64::engine::general_purpose::STANDARD.decode(content) {
        Ok(bytes) => bytes.len() >= MIN_DECODED && bytes[0] == 2,
        Err(_) => false,
    }
}

/// Decrypt, in `frame`, every event the user encrypted to themselves.
///
/// Walks the places an event sits on this wire: `event` (publish results),
/// `result.event` (subscription deliveries) and `events[].event` (query
/// results). Anything else, and any event that does not qualify, is left as
/// it is.
pub async fn reveal(signer: &dyn Signer, mut frame: Envelope) -> Envelope {
    let mut user: Option<Option<PublicKey>> = None;

    if let Some(event) = frame.fields.get_mut("event") {
        reveal_one(signer, &mut user, event).await;
    }
    if let Some(event) = frame
        .fields
        .get_mut("result")
        .and_then(|r| r.get_mut("event"))
    {
        reveal_one(signer, &mut user, event).await;
    }
    if let Some(serde_json::Value::Array(items)) = frame.fields.get_mut("events") {
        for item in items {
            if let Some(event) = item.get_mut("event") {
                reveal_one(signer, &mut user, event).await;
            }
        }
    }
    frame
}

/// Decrypt one event's `content` in place, if it qualifies.
///
/// `user` caches the signer's public key across one frame, asked for only
/// once a candidate turns up.
async fn reveal_one(
    signer: &dyn Signer,
    user: &mut Option<Option<PublicKey>>,
    event: &mut serde_json::Value,
) {
    if !event
        .get("content")
        .and_then(|c| c.as_str())
        .is_some_and(looks_like_nip44)
    {
        return;
    }
    let user = match user {
        Some(known) => *known,
        None => *user.insert(signer.public_key().await.ok()),
    };
    let Some(user) = user else { return };
    if event.get("pubkey").and_then(|p| p.as_str()) != Some(user.to_hex().as_str()) {
        return;
    }
    // Verified last, being the dearest check: a forged event naming the user
    // could otherwise carry someone else's ciphertext here to be read.
    let Ok(parsed) = serde_json::from_value::<nostr::Event>(event.clone()) else {
        return;
    };
    if parsed.verify().is_err() {
        return;
    }
    match signer.nip44_decrypt(&user, &parsed.content).await {
        Ok(plaintext) => event["content"] = serde_json::Value::String(plaintext),
        Err(e) => tracing::debug!(event = %parsed.id, error = %e, "private event left encrypted"),
    }
}
