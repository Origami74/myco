//! Client for the fips node's Unix-domain control socket.
//!
//! The node no longer exposes an in-process read handle or a Rust API for
//! pushing platform-discovered peers; both are control-socket commands now
//! (`show_peers`, `connect`). This module is Myco's only way to talk to a node
//! that `run_rx_loop` has borrowed for its whole life.
//!
//! The wire format is fips's own (`docs/reference/control-socket.md`):
//! newline-delimited JSON, one request line per connection, the write half shut
//! down so the server stops waiting for more, one response line back, envelope
//! `{"status":"ok","data":…}` / `{"status":"error","message":…}`. The reference
//! client lives in fips's `src/bin/`, not its library, so it cannot be imported.
//!
//! **Never call this from the FFI thread.** A connect + write + read with a 5s
//! timeout is not a substitute for the lock-free `peer_views()` read it
//! replaces; `AppRuntime::state()` reads a cached snapshot the 8s tick writes.

use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::timeout;

/// File name of the control socket inside the app-private data dir.
///
/// The default fips path resolves `/run/fips` → `$XDG_RUNTIME_DIR` → `/tmp`,
/// none of which an Android app UID can write, so Myco always sets this
/// explicitly. `/data/user/0/app.myco/files/fips-control.sock` is 45 of the
/// 108 `sun_path` bytes bionic allows — verified on device.
pub const SOCKET_FILE_NAME: &str = "fips-control.sock";

/// Where the control socket lives for a given app data dir.
pub fn socket_path(data_dir: &str) -> String {
    format!("{}/{}", data_dir.trim_end_matches('/'), SOCKET_FILE_NAME)
}

/// Matches the 5s both fips's server and its reference client use.
const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// One peer, as Myco needs it — the fields it consumes out of a `show_peers`
/// row, not the whole row (which carries MMP link quality, tree position, noise
/// counters and more).
///
/// Named after the `fips::control::read_handle::PeerView` it replaces so the
/// merge in [`crate::peer_diagnostics`] keeps its shape, but Myco-owned now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PeerView {
    /// The peer's `node_addr`, hex-encoded (`node_addr` on the wire — the value
    /// is hex, the key is not).
    pub node_addr_hex: String,
    /// The peer's npub. `show_peers` returns authenticated peers only, so this
    /// is always a real npub rather than an address stand-in.
    pub npub: String,
    /// Whether Myco should treat this peer as usable right now — see
    /// [`is_connected`].
    pub connected: bool,
    /// Milliseconds since the epoch this peer was last heard from.
    pub last_seen_ms: u64,
    /// Transport type carrying the peer's link (`"ble"`, `"udp"`, …).
    pub transport: String,
    /// fips's render-ready name. **This is an abbreviated npub**
    /// (`"npub1qrjr...msuc"`), not a profile name — observed on device.
    pub display_name: String,
}

/// Myco's notion of "connected", derived from fips's `connectivity` string.
///
/// `connectivity` is `ConnectivityState`'s `Display` — a closed set of exactly
/// four values (`connected`, `stale`, `reconnecting`, `disconnected`) — and
/// fips has three disagreeing predicates over it. This one is `can_send()`:
/// `connected` **or** `stale`. A stale peer is still routable, so matching only
/// the literal `"connected"` would silently drop usable peers out of the
/// content sync tick and leave their relay dial backoff un-reset.
pub fn is_connected(connectivity: &str) -> bool {
    matches!(connectivity, "connected" | "stale")
}

/// A client for one control socket path.
///
/// Stateless and cheap to clone: fips's server reads exactly one request per
/// connection, so there is no session to keep and each call dials afresh.
#[derive(Clone, Debug)]
pub struct ControlClient {
    socket_path: String,
}

impl ControlClient {
    pub fn new(socket_path: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    /// Issue one command and return its `data` object.
    ///
    /// Every failure mode — the socket not existing (the bind failed, or the
    /// node has not started), a timeout, an `{"status":"error"}` envelope — is
    /// an `Err` with a human-readable reason, because Myco has to be able to
    /// tell "no peers" from "no peer feed" (fips's bind failure is non-fatal
    /// and warns only, so an unbound socket is otherwise invisible).
    pub async fn request(&self, command: &str, params: Option<Value>) -> Result<Value, String> {
        let mut req = json!({ "command": command });
        if let Some(params) = params {
            req["params"] = params;
        }
        let mut line = req.to_string();
        line.push('\n');

        let stream = timeout(
            IO_TIMEOUT,
            tokio::net::UnixStream::connect(&self.socket_path),
        )
        .await
        .map_err(|_| "connect timed out".to_string())?
        .map_err(|e| format!("connect {}: {e}", self.socket_path))?;

        let (reader, mut writer) = tokio::io::split(stream);

        timeout(IO_TIMEOUT, writer.write_all(line.as_bytes()))
            .await
            .map_err(|_| "write timed out".to_string())?
            .map_err(|e| format!("write: {e}"))?;
        // The server reads exactly one line and would otherwise keep waiting on
        // a half-open stream.
        writer
            .shutdown()
            .await
            .map_err(|e| format!("shutdown: {e}"))?;

        let mut response = String::new();
        timeout(IO_TIMEOUT, BufReader::new(reader).read_line(&mut response))
            .await
            .map_err(|_| "read timed out".to_string())?
            .map_err(|e| format!("read: {e}"))?;
        if response.is_empty() {
            return Err("server closed the connection without a response".to_string());
        }

        let value: Value =
            serde_json::from_str(response.trim()).map_err(|e| format!("bad response JSON: {e}"))?;
        match value.get("status").and_then(Value::as_str) {
            Some("ok") => Ok(value.get("data").cloned().unwrap_or(Value::Null)),
            Some("error") => Err(value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unspecified error")
                .to_string()),
            other => Err(format!("unexpected status: {other:?}")),
        }
    }

    /// The node's currently authenticated peers.
    ///
    /// Note the semantic delta from the read handle this replaces: a peer that
    /// was seen and is now gone **disappears from the list** rather than
    /// appearing with `connected: false`.
    pub async fn show_peers(&self) -> Result<Vec<PeerView>, String> {
        let data = self.request("show_peers", None).await?;
        let peers = data
            .get("peers")
            .and_then(Value::as_array)
            .ok_or_else(|| "show_peers response has no peers array".to_string())?;
        Ok(peers.iter().map(peer_from_json).collect())
    }

    /// Tell the node a platform-discovered peer is reachable.
    ///
    /// `address` is a fully-formatted socket address (Wi-Fi Aware link-locals
    /// carry a numeric scope, `"[fe80::x%3]:4871"`). The npub is only a routing
    /// hint — Noise IK is what authenticates — but fips pre-seeds its identity
    /// cache from it, which warms the route for free.
    ///
    /// This is a *mutation*, so it takes the rx-loop path rather than being
    /// served off a snapshot: it is serialised behind whatever the packet loop
    /// is doing. Call it only from the drain task in
    /// [`crate::platform_peers`], never from a radio callback thread.
    pub async fn connect_peer(
        &self,
        npub: &str,
        address: &str,
        transport: &str,
    ) -> Result<(), String> {
        self.request(
            "connect",
            Some(json!({
                "npub": npub,
                "address": address,
                "transport": transport,
            })),
        )
        .await
        .map(|_| ())
    }
}

/// Map one `show_peers` row onto [`PeerView`]. Every field is optional on the
/// wire (`transport_addr`/`transport_type` are conditional keys), so a missing
/// one degrades to empty rather than dropping the peer.
fn peer_from_json(peer: &Value) -> PeerView {
    let s = |key: &str| {
        peer.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    PeerView {
        node_addr_hex: s("node_addr"),
        npub: s("npub"),
        connected: is_connected(
            peer.get("connectivity")
                .and_then(Value::as_str)
                .unwrap_or(""),
        ),
        last_seen_ms: peer
            .get("last_seen_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        transport: s("transport_type"),
        display_name: s("display_name"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four `connectivity` values, against `can_send()` semantics. A peer
    /// that is `stale` is still routable; dropping it here is what would leave
    /// a returning Circle member waiting out a 180s dial backoff.
    #[test]
    fn connectivity_maps_with_can_send_semantics() {
        assert!(is_connected("connected"));
        assert!(is_connected("stale"));
        assert!(!is_connected("reconnecting"));
        assert!(!is_connected("disconnected"));
        assert!(
            !is_connected(""),
            "an absent key must not read as connected"
        );
    }

    /// Field mapping against a row observed verbatim on device (probe
    /// 260811-urj), trimmed to the keys Myco reads. The key names are the
    /// non-obvious part: `node_addr` not `node_addr_hex`, `transport_type` not
    /// `transport`, and `connectivity` (a string) not `connected` (a bool).
    #[test]
    fn maps_an_observed_peer_row() {
        let row = serde_json::json!({
            "authenticated_at_ms": 1786483829884u64,
            "connectivity": "connected",
            "display_name": "npub1qrjr...msuc",
            "ipv6_addr": "fdad:9d5c:b1a2:48d4:ff21:e9f3:c10:b3ea",
            "last_seen_ms": 1786484197793u64,
            "node_addr": "ad9d5cb1a248d4ff21e9f30c10b3ea40",
            "npub": "npub1qrjrvpelneupkjnk5nmkxxjfyxkyp5yg5l38t3e8fxs75lzwtgqqfqmsuc",
            "transport_addr": "[::ffff:192.168.8.238]:2121",
            "transport_type": "udp",
        });
        let view = peer_from_json(&row);
        assert_eq!(view.node_addr_hex, "ad9d5cb1a248d4ff21e9f30c10b3ea40");
        assert_eq!(
            view.npub,
            "npub1qrjrvpelneupkjnk5nmkxxjfyxkyp5yg5l38t3e8fxs75lzwtgqqfqmsuc"
        );
        assert!(view.connected);
        assert_eq!(view.last_seen_ms, 1786484197793);
        assert_eq!(view.transport, "udp");
        assert_eq!(view.display_name, "npub1qrjr...msuc");
    }

    #[test]
    fn a_row_missing_conditional_keys_still_maps() {
        let row = serde_json::json!({ "npub": "npub1x", "connectivity": "stale" });
        let view = peer_from_json(&row);
        assert!(view.connected);
        assert!(view.transport.is_empty());
        assert_eq!(view.last_seen_ms, 0);
    }

    fn temp_socket(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("myco-ctl-{}-{tag}.sock", std::process::id()))
    }

    /// Serve exactly one request the way fips does — read a line, write a line
    /// — and hand the request back so the test can assert on it.
    async fn serve_once(path: std::path::PathBuf, response: &'static str) -> String {
        let listener = tokio::net::UnixListener::bind(&path).expect("bind");
        let (stream, _) = listener.accept().await.expect("accept");
        let (reader, mut writer) = tokio::io::split(stream);
        let mut request = String::new();
        BufReader::new(reader)
            .read_line(&mut request)
            .await
            .expect("read");
        writer.write_all(response.as_bytes()).await.expect("write");
        writer.flush().await.expect("flush");
        request
    }

    #[tokio::test]
    async fn show_peers_round_trips() {
        let path = temp_socket("peers");
        let _ = std::fs::remove_file(&path);
        let server = tokio::spawn(serve_once(
            path.clone(),
            "{\"status\":\"ok\",\"data\":{\"peers\":[\
             {\"npub\":\"npub1a\",\"connectivity\":\"connected\",\"node_addr\":\"aa\"},\
             {\"npub\":\"npub1b\",\"connectivity\":\"disconnected\",\"node_addr\":\"bb\"}]}}\n",
        ));
        // The listener binds inside the task; retry the dial until it is up.
        let client = ControlClient::new(path.to_str().unwrap());
        let peers = loop {
            match client.show_peers().await {
                Ok(p) => break p,
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        };
        let request = server.await.expect("server task");
        assert!(request.contains("\"command\":\"show_peers\""));
        assert_eq!(peers.len(), 2, "both peers are returned, connected or not");
        assert!(peers[0].connected);
        assert!(!peers[1].connected);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn connect_sends_the_three_params_and_surfaces_an_error_envelope() {
        let path = temp_socket("connect");
        let _ = std::fs::remove_file(&path);
        let server = tokio::spawn(serve_once(
            path.clone(),
            "{\"status\":\"error\",\"message\":\"no such transport\"}\n",
        ));
        let client = ControlClient::new(path.to_str().unwrap());
        let err = loop {
            match client
                .connect_peer("npub1x", "[fe80::1%3]:4871", "udp")
                .await
            {
                Err(e) if e.starts_with("connect ") => {
                    tokio::time::sleep(Duration::from_millis(10)).await
                }
                Err(e) => break e,
                Ok(()) => panic!("an error envelope must not read as success"),
            }
        };
        assert_eq!(err, "no such transport");
        let request = server.await.expect("server task");
        assert!(request.contains("\"command\":\"connect\""));
        assert!(request.contains("\"npub\":\"npub1x\""));
        assert!(request.contains("\"address\":\"[fe80::1%3]:4871\""));
        assert!(request.contains("\"transport\":\"udp\""));
        let _ = std::fs::remove_file(&path);
    }

    /// An absent socket is the shape a failed bind takes, and it must be an
    /// error rather than an empty peer list.
    #[tokio::test]
    async fn an_absent_socket_is_an_error_not_an_empty_list() {
        let client = ControlClient::new(temp_socket("absent").to_str().unwrap());
        assert!(client.show_peers().await.is_err());
    }

    #[test]
    fn socket_path_sits_in_the_data_dir() {
        assert_eq!(
            socket_path("/data/user/0/app.myco/files"),
            "/data/user/0/app.myco/files/fips-control.sock"
        );
        assert_eq!(socket_path("/tmp/"), "/tmp/fips-control.sock");
    }
}
