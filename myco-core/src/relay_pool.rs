//! The internet relay pool: **one shared, multiplexed connection per relay**,
//! for every read and write Myco makes to a public or configured relay.
//!
//! Built on [`rustic_applesauce`]'s `RelayPool`. Before it, each query, each
//! subscription stream and each publish dialled its own WebSocket, so a feed
//! opening could hold several sockets to one relay at once — and relays that
//! count connections answered with 503s and refused handshakes. Now a relay
//! gets one socket, every `REQ` and `EVENT` rides it, it opens when something
//! asks and closes a minute after the last asker is done.
//!
//! What Myco keeps of its own, in the connector the pool dials through:
//!
//! - **Relay health.** Every dial is checked against the skip list first and
//!   its outcome recorded, with the typed error, exactly as before
//!   (`relay_health`).
//! - **Liveness.** The socket is pinged every [`PING_EVERY`] and given up
//!   after [`SILENT_FOR`] without a frame: a phone that changed networks
//!   leaves sockets that never error, they just never speak again. The pool
//!   then reconnects on its own backoff, and a live subscription resumes.
//!
//! The library has its own `Event` and `Filter` — plain NIP-01 JSON shapes.
//! They are converted here, and only here: everything outside this module
//! speaks `nostr` types. Every event that comes in is parsed into a
//! [`nostr::Event`] and signature-checked, which is also what rejects a
//! malformed one; the library checks neither.
//!
//! The private-address guard stays with the callers that dial relays someone
//! else named (the outbox), before they reach this pool.

use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::future::BoxFuture;
use futures_util::{Sink, Stream, StreamExt};
use nostr::Event;
use rustic_applesauce::relay::{
    Nip01Codec, PoolConfig, PublishAck, Relay, RelayConfig, RelayFactory, RelayIdentity, RelayPool,
    RequestEvent, SubscriptionEvent,
};
use rustic_applesauce::streams::{Driver, DriverError};
use rustic_applesauce::transport::{
    managed_socket_with_connector, BoxTransport, ConnectionConfig, ConnectionError, ManagedSocket,
    SocketFrame, TransportConnector,
};
use tokio_tungstenite::tungstenite::Message;

/// How often an open relay socket is pinged.
pub(crate) const PING_EVERY: Duration = Duration::from_secs(30);

/// A socket that has heard nothing — no event, no pong — for this long is
/// given up on, and the pool reconnects.
pub(crate) const SILENT_FOR: Duration = Duration::from_secs(90);

/// How long one dial gets. Counted against the relay when it runs out, as a
/// connect-phase timeout always was.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// How many events a slow reader may fall behind by before it is told it
/// lagged. A feed's backlog arrives in hundreds.
const DELIVERY_CAPACITY: usize = 4096;

type Pool = RelayPool<MycoFactory>;

/// A finite read of `filters` on `url`: each verified event is handed to
/// `each` as the relay sends it, until `EOSE` or `CLOSED`. `Ok` once the
/// relay said its stored events were over; `Err` if it could not be reached
/// or failed first. Bound it with a timeout at the call site — what `each`
/// was given before the timeout is the caller's to keep.
pub(crate) async fn request(
    url: &str,
    filters: Vec<serde_json::Value>,
    mut each: impl FnMut(Event),
) -> anyhow::Result<()> {
    crate::relay_health::check(url)?;
    let relay = relay(url)?;
    let mut dials = DialWatch::new(url);
    let mut op = relay.request(filters_in(filters)?).activate();
    loop {
        let item = tokio::select! {
            item = op.next() => match item { Some(item) => item, None => break },
            failed = dials.failed() => anyhow::bail!("{failed}"),
        };
        dials.heard();
        match item {
            Ok(RequestEvent::Event(event)) => {
                if let Some(event) = event_out(event) {
                    each(event);
                }
            }
            Ok(RequestEvent::Eose) => return Ok(()),
            Ok(RequestEvent::Closed { message }) => {
                tracing::debug!(url, message, "relay closed the request");
                return Ok(());
            }
            Err(e) => anyhow::bail!("relay request failed: {e:?}"),
        }
    }
    Ok(())
}

/// A live subscription to `filters` on `url`: the stored events and then the
/// live ones, each verified and sent to `out`, until the relay closes the
/// subscription, the connection fails, or `out` is gone. `saw_eose` is set
/// when the relay said its stored events were over. Reconnecting is the
/// caller's, with its backoff.
pub(crate) async fn subscribe(
    url: &str,
    filters: Vec<serde_json::Value>,
    out: tokio::sync::mpsc::Sender<Event>,
    saw_eose: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<()> {
    crate::relay_health::check(url)?;
    let relay = relay(url)?;
    let mut dials = DialWatch::new(url);
    let mut op = relay.subscribe(filters_in(filters)?).activate();
    loop {
        let item = tokio::select! {
            item = op.next() => match item { Some(item) => item, None => break },
            failed = dials.failed() => anyhow::bail!("{failed}"),
        };
        dials.heard();
        match item {
            Ok(SubscriptionEvent::Event(event)) => {
                // A bounded channel: a relay flooding faster than the store
                // takes it waits here, not in memory.
                if let Some(event) = event_out(event) {
                    if out.send(event).await.is_err() {
                        return Ok(());
                    }
                }
            }
            Ok(SubscriptionEvent::Eose) => {
                saw_eose.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(SubscriptionEvent::Closed { message }) => {
                tracing::debug!(url, message, "relay closed the subscription");
                return Ok(());
            }
            Err(e) => anyhow::bail!("relay subscription ended: {e:?}"),
        }
    }
    Ok(())
}

/// Publish one signed event to `url` and wait for its `OK`. `Ok(true)` is
/// accepted, `Ok(false)` refused (the message is logged), `Err` no answer.
/// Bound it with a timeout at the call site.
pub(crate) async fn publish(url: &str, event: &Event) -> anyhow::Result<bool> {
    crate::relay_health::check(url)?;
    let relay = relay(url)?;
    let lib_event = serde_json::from_value(serde_json::to_value(event)?)?;
    let mut dials = DialWatch::new(url);
    let mut op = relay.publish(lib_event).activate();
    let answer = tokio::select! {
        answer = op.next() => answer,
        failed = dials.failed() => anyhow::bail!("{failed}"),
    };
    match answer {
        Some(Ok(PublishAck { accepted, message })) => {
            if !accepted {
                tracing::debug!(url, event = %event.id, message, "relay refused the event");
            }
            Ok(accepted)
        }
        Some(Err(e)) => anyhow::bail!("relay publish failed: {e:?}"),
        None => anyhow::bail!("relay closed without an OK"),
    }
}

/// The pooled handle for `url`, made on first use.
fn relay(
    url: &str,
) -> anyhow::Result<rustic_applesauce::relay::PooledRelay<Nip01Codec<ManagedSocket>>> {
    with_pool(|pool| {
        pool.relay(url)
            .map_err(|e| anyhow::anyhow!("relay {url} not usable: {e:?}"))
    })
}

/// NIP-01 filters, as JSON, into the library's filter. NIP-50 `search` has
/// no place in it and is dropped — no relay Myco asks by default serves it.
fn filters_in(filters: Vec<serde_json::Value>) -> anyhow::Result<Vec<rustic_applesauce::Filter>> {
    filters
        .into_iter()
        .map(|mut f| {
            if let Some(obj) = f.as_object_mut() {
                obj.remove("search");
            }
            serde_json::from_value(f).map_err(anyhow::Error::from)
        })
        .collect()
}

/// A relay's event, into a `nostr` event — `None` if it is malformed or its
/// signature does not check out. This is where a relay's events enter the
/// process, so callers downstream need not check. See
/// `reference/thinning-custom-relay.md` (D7).
fn event_out(event: rustic_applesauce::Event) -> Option<Event> {
    let event: Event = serde_json::to_value(event)
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())?;
    event.verify().is_ok().then_some(event)
}

/// Every dial the connector makes, as `(relay key, error)`: how an operation
/// still waiting on its connection learns the dial failed. The pool retries
/// on its own backoff for whoever asks next; the operation that asked now
/// fails at once, as a dial always did, rather than waiting out its deadline.
fn dial_outcomes() -> &'static tokio::sync::broadcast::Sender<(String, Option<String>)> {
    static DIALS: std::sync::OnceLock<tokio::sync::broadcast::Sender<(String, Option<String>)>> =
        std::sync::OnceLock::new();
    DIALS.get_or_init(|| tokio::sync::broadcast::channel(256).0)
}

/// One operation's view of its relay's dials, until the relay has said
/// anything to it — after that, a failure reaches it as the connection's.
struct DialWatch {
    key: String,
    dials: Option<tokio::sync::broadcast::Receiver<(String, Option<String>)>>,
}

impl DialWatch {
    fn new(url: &str) -> Self {
        Self {
            key: crate::relay_health::key(url),
            dials: Some(dial_outcomes().subscribe()),
        }
    }

    /// The relay answered this operation: its dials are no longer this
    /// operation's business.
    fn heard(&mut self) {
        self.dials = None;
    }

    /// Resolves with the error of this relay's next failed dial; never, once
    /// the relay has answered or a dial succeeded.
    async fn failed(&mut self) -> String {
        loop {
            let Some(dials) = self.dials.as_mut() else {
                return std::future::pending().await;
            };
            match dials.recv().await {
                Ok((key, outcome)) if key == self.key => match outcome {
                    Some(error) => return error,
                    None => self.dials = None,
                },
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => self.dials = None,
            }
        }
    }
}

fn pool_config() -> PoolConfig {
    let connection = ConnectionConfig {
        // The dial is bounded by the connector, which counts it; this is
        // only a backstop above that.
        connect_timeout: CONNECT_TIMEOUT + Duration::from_secs(5),
        event_capacity: DELIVERY_CAPACITY,
        ..ConnectionConfig::default()
    };
    PoolConfig::default()
        .with_connection_config(connection)
        .with_relay_config(
            // The callers bound every read and publish themselves; these only
            // stop a forgotten one from running forever.
            RelayConfig::default()
                .with_request_timeout(Duration::from_secs(60))
                .with_publish_timeout(Duration::from_secs(30)),
        )
        .with_operation_capacity(DELIVERY_CAPACITY)
}

/// The process's pool, with its driver on a runtime of its own — so it
/// outlives whichever runtime happened to ask first.
#[cfg(not(test))]
fn with_pool<T>(f: impl FnOnce(&Pool) -> T) -> T {
    static POOL: std::sync::OnceLock<(Pool, tokio::runtime::Runtime)> = std::sync::OnceLock::new();
    let (pool, _rt) = POOL.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("myco-relays")
            .enable_all()
            .build()
            .expect("relay pool runtime");
        let (pool, driver) = Pool::new(MycoFactory, pool_config()).expect("relay pool config");
        rt.spawn(async move {
            if let Err(e) = driver.await {
                tracing::error!(error = ?e, "relay pool driver stopped");
            }
        });
        (pool, rt)
    });
    f(pool)
}

/// In tests, one pool per test thread with its driver on that test's
/// runtime — as `relay_health` keeps one list per test thread.
#[cfg(test)]
fn with_pool<T>(f: impl FnOnce(&Pool) -> T) -> T {
    thread_local! {
        static POOL: Pool = {
            let (pool, driver) = Pool::new(MycoFactory, pool_config()).expect("relay pool config");
            tokio::spawn(async move {
                let _ = driver.await;
            });
            pool
        };
    }
    POOL.with(|pool| f(pool))
}

/// Makes each relay's socket, dialling through [`MycoConnector`].
struct MycoFactory;

impl RelayFactory for MycoFactory {
    type Socket = Nip01Codec<ManagedSocket>;
    type Error = std::io::Error;

    fn create(
        &self,
        identity: &RelayIdentity,
        config: &PoolConfig,
    ) -> Result<(Relay<Self::Socket>, Driver), Self::Error> {
        let (socket, driver) = managed_socket_with_connector(
            Arc::<str>::from(identity.as_str()),
            config.connection_config().clone(),
            Arc::new(MycoConnector),
        )
        .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
        let relay = Relay::with_config(Nip01Codec::new(socket), config.relay_config());
        let driver: Driver =
            Box::pin(async move { driver.await.map_err(|e| DriverError(format!("{e:?}"))) });
        Ok((relay, driver))
    }
}

/// Dials a relay the way Myco always has — skip list first, the dial timed
/// and its outcome recorded — and hands the pool a socket that watches its
/// own liveness.
struct MycoConnector;

impl TransportConnector for MycoConnector {
    fn connect(&self, url: Arc<str>) -> BoxFuture<'static, Result<BoxTransport, ConnectionError>> {
        Box::pin(async move {
            if let Err(e) = crate::relay_health::check(&url) {
                let _ = dial_outcomes().send((crate::relay_health::key(&url), Some(e.to_string())));
                return Err(ConnectionError::Connect(e.to_string()));
            }
            let connected = match crate::relay_health::timeout(
                &url,
                CONNECT_TIMEOUT,
                tokio_tungstenite::connect_async(url.as_ref()),
            )
            .await
            {
                Ok(connected) => connected.map_err(anyhow::Error::from),
                Err(_) => Err(anyhow::anyhow!("relay did not connect in time")),
            };
            crate::relay_health::record_ws(&url, &connected);
            let _ = dial_outcomes().send((
                crate::relay_health::key(&url),
                connected.as_ref().err().map(|e| e.to_string()),
            ));
            let (ws, _) = connected.map_err(|e| ConnectionError::Connect(e.to_string()))?;
            Ok(Box::pin(Watched::new(ws, &url)) as BoxTransport)
        })
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A relay socket that pings and gives up on silence. See [`SILENT_FOR`].
/// Counted open in `relay_health` for as long as it lives.
struct Watched {
    ws: Ws,
    ping: tokio::time::Interval,
    heard: tokio::time::Instant,
    url: Arc<str>,
}

impl Watched {
    fn new(ws: Ws, url: &Arc<str>) -> Self {
        let mut ping = tokio::time::interval(PING_EVERY);
        ping.reset(); // the first tick after a full interval, not at once
        crate::relay_health::socket_opened(url);
        Self {
            ws,
            ping,
            heard: tokio::time::Instant::now(),
            url: url.clone(),
        }
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        crate::relay_health::socket_closed(&self.url);
    }
}

impl Stream for Watched {
    type Item = Result<SocketFrame, ConnectionError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        while this.ping.poll_tick(cx).is_ready() {
            if this.heard.elapsed() >= SILENT_FOR {
                return Poll::Ready(Some(Err(ConnectionError::Read(
                    "relay silent too long".into(),
                ))));
            }
            // Best effort: a socket not ready to write is not pinged this
            // time; silence is what decides, not a missed ping.
            let mut ws = std::pin::Pin::new(&mut this.ws);
            if let Poll::Ready(Ok(())) = ws.as_mut().poll_ready(cx) {
                if ws.as_mut().start_send(Message::Ping(Vec::new())).is_ok() {
                    let _ = ws.as_mut().poll_flush(cx);
                }
            }
        }
        loop {
            match std::pin::Pin::new(&mut this.ws).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(ConnectionError::Read(e.to_string()))))
                }
                Poll::Ready(Some(Ok(msg))) => {
                    this.heard = tokio::time::Instant::now();
                    match msg {
                        Message::Text(text) => {
                            return Poll::Ready(Some(Ok(SocketFrame::Text(text))))
                        }
                        Message::Binary(bytes) => {
                            return Poll::Ready(Some(Ok(SocketFrame::Binary(bytes))))
                        }
                        Message::Close(_) => return Poll::Ready(None),
                        // Ping, pong, raw frames: liveness noted above.
                        _ => continue,
                    }
                }
            }
        }
    }
}

impl Sink<SocketFrame> for Watched {
    type Error = ConnectionError;

    fn poll_ready(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        std::pin::Pin::new(&mut self.ws)
            .poll_ready(cx)
            .map_err(|e| ConnectionError::Write(e.to_string()))
    }

    fn start_send(
        mut self: std::pin::Pin<&mut Self>,
        frame: SocketFrame,
    ) -> Result<(), Self::Error> {
        let msg = match frame {
            SocketFrame::Text(text) => Message::Text(text),
            SocketFrame::Binary(bytes) => Message::Binary(bytes),
        };
        std::pin::Pin::new(&mut self.ws)
            .start_send(msg)
            .map_err(|e| ConnectionError::Write(e.to_string()))
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        std::pin::Pin::new(&mut self.ws)
            .poll_flush(cx)
            .map_err(|e| ConnectionError::Write(e.to_string()))
    }

    fn poll_close(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        std::pin::Pin::new(&mut self.ws)
            .poll_close(cx)
            .map_err(|e| ConnectionError::Write(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsite_deck::seams::RelayBackend as _;

    async fn relay_holding(events: Vec<Event>) -> String {
        let store = Arc::new(myco_relay::RelayStore::in_memory());
        for e in events {
            store.publish(e).await.unwrap();
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        tokio::spawn(crate::mesh_relay::serve_on(store, listener));
        url
    }

    /// Reads to one relay share one connection, however many run at once.
    #[tokio::test]
    async fn reads_share_one_connection() {
        use futures_util::SinkExt;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio_tungstenite::tungstenite::Message;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let accepted = Arc::new(AtomicUsize::new(0));
        let counted = accepted.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                    while let Some(Ok(msg)) = ws.next().await {
                        if let Some(sub) = crate::ip_source::tests::req_sub_id(&msg) {
                            let eose = serde_json::json!(["EOSE", sub]).to_string();
                            let _ = ws.send(Message::Text(eose)).await;
                        }
                    }
                });
            }
        });

        let reads = (0..5).map(|i| {
            let url = url.clone();
            async move { request(&url, vec![serde_json::json!({ "kinds": [i] })], |_| {}).await }
        });
        for r in futures_util::future::join_all(reads).await {
            r.unwrap();
        }
        request(&url, vec![serde_json::json!({ "kinds": [9] })], |_| {})
            .await
            .unwrap();
        assert_eq!(
            accepted.load(Ordering::SeqCst),
            1,
            "a read opened its own socket"
        );
    }

    /// A relay that refuses the connection fails the read at once, not at
    /// the read's deadline.
    #[tokio::test]
    async fn a_refused_relay_fails_at_once() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let started = std::time::Instant::now();
        let r = request(
            &format!("ws://127.0.0.1:{port}"),
            vec![serde_json::json!({})],
            |_| {},
        )
        .await;
        assert!(r.is_err());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    /// A read over the pool reaches a relay and hands back its events.
    #[tokio::test]
    async fn a_request_reads_a_relay() {
        let note = nostr::EventBuilder::text_note("hi")
            .sign_with_keys(&nostr::Keys::generate())
            .unwrap();
        let url = relay_holding(vec![note.clone()]).await;
        let mut got = Vec::new();
        let r = request(&url, vec![serde_json::json!({"kinds": [1]})], |e| {
            got.push(e)
        })
        .await;
        r.unwrap();
        assert_eq!(got.len(), 1);
    }
}
