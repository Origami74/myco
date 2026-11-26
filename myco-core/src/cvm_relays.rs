//! NAP-CVM's relays: ContextVM traffic over the shared internet relay pool.
//!
//! The runtime decides what to send and to which relays
//! (`myco_napplet_runtime::nap::cvm`); this is only the carrying. Every relay
//! goes through [`crate::relay_pool`] — one socket per relay, shared with the
//! rest of Myco — under the same two rules as every other dial a napplet
//! causes: nothing while the user has the internet off, and no relay whose
//! name resolves to this device or its LAN ([`crate::outbox::dials_public`]).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use myco_napplet_runtime::{CvmListen, CvmRelays, Direction, OutboxResolver, RelayLane, Signer};
use nostr::{Event, Filter};
use tokio::sync::mpsc;

/// Where ContextVM servers announce themselves and listen by default.
pub(crate) const DEFAULT_RELAYS: &[&str] = &["wss://relay.contextvm.org"];

/// The most relays discovery reads: the defaults first, then the user's.
const MAX_DISCOVERY_RELAYS: usize = 8;

/// How long `listen` waits for a relay to say its subscription is live
/// before letting the first request go anyway.
const LISTEN_READY: Duration = Duration::from_secs(5);

/// Back-off between a failed listen and the next attempt on that relay.
const RELISTEN_AFTER: Duration = Duration::from_secs(5);

pub(crate) struct PoolCvmRelays {
    content: Arc<crate::content::Content>,
    /// The user, whose own relays discovery reads too.
    signer: Arc<dyn Signer>,
    /// Where the user's relays come from: their NIP-65 list, as the outbox
    /// holds it.
    resolver: Arc<dyn OutboxResolver>,
}

impl PoolCvmRelays {
    pub(crate) fn new(
        content: Arc<crate::content::Content>,
        signer: Arc<dyn Signer>,
        resolver: Arc<dyn OutboxResolver>,
    ) -> Self {
        Self {
            content,
            signer,
            resolver,
        }
    }
}

/// The relays among `relays` that may be dialled now.
async fn dialable(relays: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for url in relays {
        if crate::outbox::dials_public(url).await {
            out.push(url.clone());
        } else {
            tracing::debug!(url, "cvm: relay refused (private or unresolvable)");
        }
    }
    out
}

fn filters_json(filters: &[Filter]) -> Vec<serde_json::Value> {
    filters
        .iter()
        .filter_map(|f| serde_json::to_value(f).ok())
        .collect()
}

#[async_trait]
impl CvmRelays for PoolCvmRelays {
    fn available(&self) -> bool {
        !self.content.is_offline_only()
    }

    fn default_relays(&self) -> Vec<String> {
        DEFAULT_RELAYS.iter().map(|s| s.to_string()).collect()
    }

    /// The ContextVM defaults, then the relays the user publishes to — a
    /// server announced where the user is finds them there. Planned from
    /// what the outbox holds now, never waiting on a lookup; logged out, the
    /// defaults alone.
    async fn discovery_relays(&self) -> Vec<String> {
        let mut out = self.default_relays();
        let Ok(me) = self.signer.public_key().await else {
            return out;
        };
        let plan = self.resolver.plan_stored(Direction::Read, &[me], &[]).await;
        for lane in plan.lanes {
            if out.len() >= MAX_DISCOVERY_RELAYS {
                break;
            }
            if let RelayLane::Internet { url } = lane {
                if !out.contains(&url) {
                    out.push(url);
                }
            }
        }
        out
    }

    async fn query(
        &self,
        relays: &[String],
        filters: Vec<Filter>,
        timeout: Duration,
    ) -> Vec<Event> {
        if !self.available() {
            return Vec::new();
        }
        let json = filters_json(&filters);
        let reads = dialable(relays).await.into_iter().map(|url| {
            let json = json.clone();
            async move {
                let mut got = Vec::new();
                let _ = tokio::time::timeout(
                    timeout,
                    crate::relay_pool::request(&url, json, |e| got.push(e)),
                )
                .await;
                got
            }
        });
        let mut seen = std::collections::HashSet::new();
        futures_util::future::join_all(reads)
            .await
            .into_iter()
            .flatten()
            .filter(|e| seen.insert(e.id))
            .collect()
    }

    async fn publish(&self, relays: &[String], event: &Event, timeout: Duration) -> bool {
        if !self.available() {
            return false;
        }
        let writes = dialable(relays).await.into_iter().map(|url| async move {
            matches!(
                tokio::time::timeout(timeout, crate::relay_pool::publish(&url, event)).await,
                Ok(Ok(true))
            )
        });
        futures_util::future::join_all(writes)
            .await
            .into_iter()
            .any(|accepted| accepted)
    }

    async fn listen(
        &self,
        relays: &[String],
        filters: Vec<Filter>,
        deliver: Arc<dyn Fn(Event) + Send + Sync>,
    ) -> CvmListen {
        if !self.available() {
            return CvmListen::none();
        }
        let content = self.content.clone();
        listen_on(
            dialable(relays).await,
            &filters,
            deliver,
            Arc::new(move || content.is_offline_only()),
        )
        .await
    }
}

/// The listen itself, on relays already cleared for dialling: one task per
/// relay, each resubscribing after a drop until the listen is dropped or
/// `offline` says the internet went off. Returns once one relay said EOSE,
/// or after [`LISTEN_READY`].
async fn listen_on(
    urls: Vec<String>,
    filters: &[Filter],
    deliver: Arc<dyn Fn(Event) + Send + Sync>,
    offline: Arc<dyn Fn() -> bool + Send + Sync>,
) -> CvmListen {
    let json = filters_json(filters);
    let (ready_tx, mut ready_rx) = mpsc::channel::<()>(urls.len().max(1));
    let mut tasks = Vec::new();
    for url in urls {
        let json = json.clone();
        let deliver = deliver.clone();
        let ready = ready_tx.clone();
        let offline = offline.clone();
        tasks.push(tokio::spawn(async move {
            // Kept open until the listen is dropped (the task aborted),
            // reconnecting after a relay drops it.
            loop {
                let (tx, mut rx) = mpsc::channel::<Event>(256);
                let eose = AtomicBool::new(false);
                let deliver = deliver.clone();
                let forward = async {
                    while let Some(event) = rx.recv().await {
                        deliver(event);
                    }
                };
                let watch_ready = async {
                    // A relay answers EOSE once the subscription is live.
                    // Watched only while `listen` waits for it: a relay that
                    // never says EOSE costs no timer for the rest of the
                    // session.
                    let until = tokio::time::Instant::now() + LISTEN_READY;
                    while tokio::time::Instant::now() < until {
                        if eose.load(std::sync::atomic::Ordering::SeqCst) {
                            let _ = ready.try_send(());
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    std::future::pending::<()>().await
                };
                let sub = crate::relay_pool::subscribe(&url, json.clone(), tx, &eose);
                tokio::select! {
                    result = sub => {
                        if let Err(e) = result {
                            tracing::debug!(url, error = %e, "cvm: listen ended");
                        }
                    }
                    _ = forward => {}
                    _ = watch_ready => {}
                }
                if offline() {
                    return;
                }
                tokio::time::sleep(RELISTEN_AFTER).await;
            }
        }));
    }
    drop(ready_tx);
    // Live on one relay is enough to publish: the server answers on the
    // relays it was asked on, and the listen covers all of them.
    let _ = tokio::time::timeout(LISTEN_READY, ready_rx.recv()).await;
    CvmListen::new(AbortOnDrop(tasks))
}

/// Stops the listen's tasks when the session that holds it goes.
struct AbortOnDrop(Vec<tokio::task::JoinHandle<()>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};
    use std::sync::Mutex;

    async fn relay() -> String {
        let store = Arc::new(myco_relay::RelayStore::in_memory());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        tokio::spawn(crate::mesh_relay::serve_on(store, listener));
        url
    }

    fn addressed(to: &Keys, n: u32) -> Event {
        EventBuilder::new(Kind::TextNote, format!("{n}"))
            .tags([Tag::public_key(to.public_key())])
            .sign_with_keys(&Keys::generate())
            .unwrap()
    }

    async fn heard(got: &Mutex<Vec<Event>>, n: usize) -> bool {
        for _ in 0..100 {
            if got.lock().unwrap().len() >= n {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    /// The listen is live when it returns: an event published right after
    /// reaches it. Dropping it stops delivery.
    #[tokio::test]
    async fn a_listen_is_live_when_it_returns_and_ends_when_dropped() {
        let url = relay().await;
        let me = Keys::generate();
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = got.clone();
        let started = std::time::Instant::now();
        let listen = listen_on(
            vec![url.clone()],
            &[Filter::new().pubkey(me.public_key())],
            Arc::new(move |e| sink.lock().unwrap().push(e)),
            Arc::new(|| false),
        )
        .await;
        assert!(
            started.elapsed() < LISTEN_READY,
            "returned on EOSE, not on the wait running out"
        );

        let first = addressed(&me, 1);
        assert!(crate::relay_pool::publish(&url, &first).await.unwrap());
        assert!(heard(&got, 1).await, "published after the listen, missed");
        assert_eq!(got.lock().unwrap()[0].id, first.id);

        drop(listen);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(crate::relay_pool::publish(&url, &addressed(&me, 2))
            .await
            .unwrap());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(got.lock().unwrap().len(), 1, "heard after the drop");
    }
}
