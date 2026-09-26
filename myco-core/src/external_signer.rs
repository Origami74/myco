//! The **external signer** bridge: signing with a key that never enters
//! Myco (NIP-55 — Amber and the like).
//!
//! The signer is an Android app, so only Kotlin can talk to it: a content
//! resolver query when the user let the signer remember its answer, an
//! intent the user approves otherwise. Rust queues a request here and waits;
//! Kotlin long-polls the queue (`signerNextRequest`, the same shape as the
//! napplet frame pump), does the Android half, and answers (`signerRespond`).
//!
//! Nothing Kotlin hands back is trusted: a signed event must be the event
//! that was asked for — same id, same pubkey — with a valid signature
//! ([`sign_via`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use nostr::{Event, PublicKey, UnsignedEvent};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

/// How long a request may wait for its answer. Long: the user may be
/// reading an approval screen in the signer app.
///
/// Kept well under the napplet prelude's publish wait
/// ([`myco_napplet_runtime::SIGNING_TIMEOUT`], 5 min): this is the bound
/// that answers the napplet — a clean `ok: false`, "the signer app did not
/// answer" — and the prelude's timer is only the backstop behind it. The
/// other way round, a napplet would see "timed out" while the user was
/// still approving, and the event could then go out anyway.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(120);

/// One request for Kotlin to carry to the signer app.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SignerRequest {
    pub id: String,
    /// NIP-55 method: `sign_event`, `nip44_encrypt`, …
    #[serde(rename = "type")]
    pub method: String,
    /// The method's payload: the unsigned event JSON for `sign_event`.
    pub payload: String,
    /// The logged-in user, hex.
    pub current_user: String,
    /// The signer app's package, from login.
    pub package: String,
}

pub struct SignerBridge {
    queue_tx: mpsc::UnboundedSender<SignerRequest>,
    queue_rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<SignerRequest>>,
    pending: Mutex<HashMap<String, oneshot::Sender<Result<String, String>>>>,
    next_id: AtomicU64,
}

impl Default for SignerBridge {
    fn default() -> Self {
        let (queue_tx, queue_rx) = mpsc::unbounded_channel();
        Self {
            queue_tx,
            queue_rx: tokio::sync::Mutex::new(queue_rx),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }
}

impl SignerBridge {
    /// Queue a request and wait for its answer: the signer's `result`, or
    /// the signed event JSON for `sign_event`.
    pub async fn request(
        &self,
        method: &str,
        payload: String,
        current_user: &PublicKey,
        package: &str,
        timeout: Duration,
    ) -> anyhow::Result<String> {
        let id = format!("s{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        let _ = self.queue_tx.send(SignerRequest {
            id: id.clone(),
            method: method.to_string(),
            payload,
            current_user: current_user.to_hex(),
            package: package.to_string(),
        });
        let answer = tokio::time::timeout(timeout, rx).await;
        self.pending.lock().unwrap().remove(&id);
        match answer {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(why))) => Err(anyhow::anyhow!("{why}")),
            Ok(Err(_)) => Err(anyhow::anyhow!("the signer request was dropped")),
            Err(_) => Err(anyhow::anyhow!("the signer app did not answer")),
        }
    }

    /// The next request for Kotlin, waiting up to `timeout`. `None` means
    /// the wait expired.
    pub async fn next_request(&self, timeout: Duration) -> Option<SignerRequest> {
        let mut rx = self.queue_rx.lock().await;
        tokio::time::timeout(timeout, rx.recv())
            .await
            .ok()
            .flatten()
    }

    /// Kotlin's answer to request `id`. An answer for a request that already
    /// timed out goes nowhere.
    pub fn respond(&self, id: &str, answer: Result<String, String>) {
        if let Some(tx) = self.pending.lock().unwrap().remove(id) {
            let _ = tx.send(answer);
        }
    }
}

/// Sign `unsigned` through the signer app, and check what comes back.
///
/// The answer is either the signed event (Amber sends it) or a bare
/// signature; both are accepted, and both are checked: the event must have
/// the requested id and pubkey and a valid signature, so a confused or
/// hostile signer cannot hand a napplet something else to publish.
pub async fn sign_via(
    bridge: &SignerBridge,
    unsigned: UnsignedEvent,
    pubkey: &PublicKey,
    package: &str,
) -> anyhow::Result<Event> {
    anyhow::ensure!(
        unsigned.pubkey == *pubkey,
        "the event is not for the logged-in user"
    );
    let mut unsigned = unsigned;
    let id = unsigned.id();
    let payload = serde_json::to_string(&unsigned)?;
    let answer = bridge
        .request("sign_event", payload, pubkey, package, ANSWER_TIMEOUT)
        .await?;
    let answer = answer.trim();

    let event = match serde_json::from_str::<Event>(answer) {
        Ok(event) => event,
        Err(_) => {
            let sig = answer.parse().map_err(|_| {
                anyhow::anyhow!("the signer answered with neither an event nor a signature")
            })?;
            unsigned.add_signature(sig)?
        }
    };
    anyhow::ensure!(event.id == id, "the signer signed a different event");
    anyhow::ensure!(event.pubkey == *pubkey, "the signer signed as someone else");
    event
        .verify()
        .map_err(|e| anyhow::anyhow!("the signer's signature is invalid: {e}"))?;
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys};
    use std::sync::Arc;

    /// A stand-in for the Kotlin loop: answers each request with `answer`.
    fn answer_with(
        bridge: Arc<SignerBridge>,
        answer: impl Fn(&SignerRequest) -> Result<String, String> + Send + 'static,
    ) {
        tokio::spawn(async move {
            while let Some(req) = bridge.next_request(Duration::from_secs(5)).await {
                bridge.respond(&req.id, answer(&req));
            }
        });
    }

    #[tokio::test]
    async fn a_signed_event_from_the_signer_is_checked_and_returned() {
        let keys = Keys::generate();
        let bridge = Arc::new(SignerBridge::default());
        let signer_keys = keys.clone();
        answer_with(bridge.clone(), move |req| {
            assert_eq!(req.method, "sign_event");
            assert_eq!(req.package, "com.greenart7c3.nostrsigner");
            let unsigned: UnsignedEvent = serde_json::from_str(&req.payload).unwrap();
            let signed = unsigned.sign_with_keys(&signer_keys).unwrap();
            Ok(serde_json::to_string(&signed).unwrap())
        });
        let unsigned = EventBuilder::text_note("gg").build(keys.public_key());
        let event = sign_via(
            &bridge,
            unsigned,
            &keys.public_key(),
            "com.greenart7c3.nostrsigner",
        )
        .await
        .unwrap();
        assert_eq!(event.pubkey, keys.public_key());
        assert!(event.verify().is_ok());
    }

    #[tokio::test]
    async fn a_bare_signature_is_accepted() {
        let keys = Keys::generate();
        let bridge = Arc::new(SignerBridge::default());
        let signer_keys = keys.clone();
        answer_with(bridge.clone(), move |req| {
            let unsigned: UnsignedEvent = serde_json::from_str(&req.payload).unwrap();
            Ok(unsigned
                .sign_with_keys(&signer_keys)
                .unwrap()
                .sig
                .to_string())
        });
        let unsigned = EventBuilder::text_note("gg").build(keys.public_key());
        assert!(sign_via(&bridge, unsigned, &keys.public_key(), "p")
            .await
            .is_ok());
    }

    /// A signer that signs something else, or as someone else, is refused.
    #[tokio::test]
    async fn a_substituted_event_is_refused() {
        let keys = Keys::generate();
        let bridge = Arc::new(SignerBridge::default());
        let signer_keys = keys.clone();
        answer_with(bridge.clone(), move |_| {
            let other = EventBuilder::text_note("something else")
                .sign_with_keys(&signer_keys)
                .unwrap();
            Ok(serde_json::to_string(&other).unwrap())
        });
        let unsigned = EventBuilder::text_note("gg").build(keys.public_key());
        let err = sign_via(&bridge, unsigned, &keys.public_key(), "p")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("different event"), "{err}");
    }

    #[tokio::test]
    async fn a_rejection_is_an_error() {
        let keys = Keys::generate();
        let bridge = Arc::new(SignerBridge::default());
        answer_with(bridge.clone(), |_| Err("rejected".to_string()));
        let unsigned = EventBuilder::text_note("gg").build(keys.public_key());
        let err = sign_via(&bridge, unsigned, &keys.public_key(), "p")
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(err, "rejected");
    }

    #[tokio::test]
    async fn nobody_answering_times_out() {
        let bridge = SignerBridge::default();
        let pk = Keys::generate().public_key();
        let err = bridge
            .request(
                "sign_event",
                "{}".into(),
                &pk,
                "p",
                Duration::from_millis(50),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("did not answer"), "{err}");
        assert!(
            bridge.pending.lock().unwrap().is_empty(),
            "a timed-out request was kept"
        );
    }

    /// The signer wait answers the napplet; the prelude's publish timer is
    /// only a backstop behind it. With room for the relay write after
    /// signing, the backstop must not fire first.
    #[test]
    fn the_signer_gives_up_before_the_napplet_prelude_does() {
        let relay_write_budget = Duration::from_secs(60);
        assert!(
            ANSWER_TIMEOUT + relay_write_budget <= myco_napplet_runtime::SIGNING_TIMEOUT,
            "a napplet would see its publish time out while the signer is still waiting"
        );
    }
}
