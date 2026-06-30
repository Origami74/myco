//! A pool of **persistent** WebSocket connections to peers' mesh relays.
//!
//! Fan-out used to open a fresh `ws://[fd00::peer]:4869` per message — over BLE a
//! TCP+WS handshake is several round-trips, and two concurrent connects contend
//! for the one radio, so the second peer lagged ~1-2s. Here each peer gets a
//! long-lived writer task fed by an unbounded channel: the first frame pays the
//! connect, every later frame is just a socket write. A dead connection is
//! dropped and lazily reopened on the next send.

use std::collections::HashMap;
use std::sync::Mutex;

use futures_util::SinkExt;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

#[derive(Default)]
pub struct PeerRelayPool {
    conns: Mutex<HashMap<String, mpsc::UnboundedSender<String>>>,
}

impl PeerRelayPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue `frame` to the peer's relay at `url`, keeping the connection open for
    /// the next send. Non-blocking; must be called from within the Tokio runtime
    /// (the gossiper's task is). A frame sent before the socket finishes
    /// connecting is buffered in the channel and flushed on connect.
    pub fn send(&self, npub: &str, url: &str, frame: String) {
        let mut conns = self.conns.lock().unwrap();
        // Reuse a live writer; on a closed channel (task gone) recover the frame
        // and fall through to reopen.
        let frame = match conns.get(npub) {
            Some(tx) => match tx.send(frame) {
                Ok(()) => return,
                Err(err) => {
                    conns.remove(npub);
                    err.0
                }
            },
            None => frame,
        };

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let _ = tx.send(frame);
        conns.insert(npub.to_string(), tx);

        let url = url.to_string();
        tokio::spawn(async move {
            let Ok((mut ws, _)) = tokio_tungstenite::connect_async(&url).await else {
                return; // channel drops with this task → next send reopens
            };
            while let Some(f) = rx.recv().await {
                if ws.send(Message::Text(f)).await.is_err() {
                    break;
                }
            }
            let _ = ws.send(Message::Close(None)).await;
        });
    }
}
