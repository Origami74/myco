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
#[derive(Clone, Debug, Default, PartialEq)]
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
    /// Milliseconds since the epoch the FMP session with this peer completed
    /// authentication. The age derived from this is the *session's* lifetime,
    /// not the current FMP index's: `receiver_idx` rotates on every rekey
    /// (`node.rekey.after_secs`, default 120s) while the session survives, so
    /// a long age here is the link holding rather than a stale handshake.
    pub authenticated_at_ms: u64,
    /// Transport type carrying the peer's link (`"ble"`, `"udp"`, …).
    pub transport: String,
    /// The transport-level address the link currently runs over, exactly as
    /// fips formats it: `adapter/AA:BB:CC:DD:EE:FF` for BLE, `[addr]:port` for
    /// the IP transports. Empty when the peer has no resolved address.
    ///
    /// For BLE this is the one thing that ties an authenticated peer to the
    /// address its scan adverts arrive on. Without it a peer that connected
    /// *inbound* has no recorded address at all — the connect-attempt log only
    /// covers dials we made — so its adverts, and the RSSI and self-advertised
    /// name they carry, could never be attributed to it.
    pub transport_addr: String,
    /// Smoothed round-trip time over this peer's link, milliseconds, as MMP
    /// measured it. `None` when MMP has taken no measurement yet — fips omits
    /// the key entirely in that case, and an unmeasured link must render as
    /// "no ping", never as a confident `0`.
    pub srtt_ms: Option<f64>,
    /// fips's render-ready name. **This is an abbreviated npub**
    /// (`"npub1qrjr...msuc"`), not a profile name — observed on device.
    pub display_name: String,
    /// Every path fips holds to this peer, in the order `show_peers` lists
    /// them. Empty on a daemon that predates multi-path, or on a peer whose
    /// paths have not been probed yet — never a fabricated single entry.
    pub paths: Vec<PeerPath>,
}

/// One transport path to a peer, as fips's multi-path layer tracks it. A peer
/// can hold several at once (BLE + Aware + LAN) and fips sends on exactly one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PeerPath {
    /// Myco's lane name: `ble`, `aware`, `udp` (the LAN/AP lane) or `tcp`.
    /// See [`lane_for_path`] for how the UDP instance name decides.
    pub lane: String,
    /// fips's path lifecycle: `probing`, `live`, `suspect` or `dead`.
    pub state: String,
    /// Whether this is the path fips currently sends on.
    pub active: bool,
    /// `normal` or `backup` — a backup path carries traffic only while no
    /// normal path is eligible.
    pub role: String,
    /// Minimum probe round trip inside fips's window, ms; `None` until one
    /// has been measured. This, not srtt, is what selection scores on.
    pub min_rtt_ms: Option<u64>,
    /// RTT samples inside the window. Below `node.path.min_samples` the path
    /// is not yet selectable.
    pub rtt_samples: u32,
    /// Per-path expected transmission count, smoothed.
    pub etx: f64,
    /// `etx × (1 + min_rtt/100)`, lower is better; `None` until measured.
    pub score: Option<f64>,
}

/// The lane a path belongs to. `transport_type` is the answer for every
/// transport but UDP, where Wi-Fi Aware and the LAN/AP lane share the type
/// and only the instance name tells them apart: the node binds the Aware
/// pool as `aware0`…`aware7` and the LAN lane as `lan` (see `runtime.rs`).
/// This is the instance the path really runs over, not a guess from the
/// address shape.
fn lane_for_path(transport_type: &str, instance: &str) -> String {
    if transport_type == "udp" && instance.starts_with("aware") {
        "aware".to_string()
    } else {
        transport_type.to_string()
    }
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
        authenticated_at_ms: peer
            .get("authenticated_at_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        transport: s("transport_type"),
        transport_addr: s("transport_addr"),
        srtt_ms: peer
            .get("mmp")
            .and_then(|mmp| mmp.get("srtt_ms"))
            .and_then(Value::as_f64),
        display_name: s("display_name"),
        paths: peer
            .get("paths")
            .and_then(Value::as_array)
            .map(|paths| paths.iter().map(path_from_json).collect())
            .unwrap_or_default(),
    }
}

/// Map one entry of a `show_peers` row's `paths` array. `transport` (the
/// instance name) is nullable on the wire; `transport_type` is absent when
/// fips could not find the transport handle, which leaves the lane empty.
fn path_from_json(path: &Value) -> PeerPath {
    let s = |key: &str| path.get(key).and_then(Value::as_str).unwrap_or_default();
    PeerPath {
        lane: lane_for_path(s("transport_type"), s("transport")),
        state: s("state").to_string(),
        active: path.get("active").and_then(Value::as_bool).unwrap_or(false),
        role: s("role").to_string(),
        min_rtt_ms: path.get("min_rtt_ms").and_then(Value::as_u64),
        rtt_samples: path.get("rtt_samples").and_then(Value::as_u64).unwrap_or(0) as u32,
        etx: path.get("etx").and_then(Value::as_f64).unwrap_or(0.0),
        score: path.get("score").and_then(Value::as_f64),
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
            "mmp": {
                "mode": "reliable",
                "srtt_ms": 42.5,
                "loss_rate": 0.0,
            },
        });
        let view = peer_from_json(&row);
        assert_eq!(view.node_addr_hex, "ad9d5cb1a248d4ff21e9f30c10b3ea40");
        assert_eq!(
            view.npub,
            "npub1qrjrvpelneupkjnk5nmkxxjfyxkyp5yg5l38t3e8fxs75lzwtgqqfqmsuc"
        );
        assert!(view.connected);
        assert_eq!(view.last_seen_ms, 1786484197793);
        assert_eq!(view.authenticated_at_ms, 1786483829884);
        assert_eq!(view.transport, "udp");
        assert_eq!(view.transport_addr, "[::ffff:192.168.8.238]:2121");
        assert_eq!(view.display_name, "npub1qrjr...msuc");
        assert_eq!(view.srtt_ms, Some(42.5));
    }

    /// fips omits `srtt_ms` from the inline MMP block until MMP has taken a
    /// measurement, and omits the whole block on a peer with no MMP session.
    /// Both must read as "no measurement" — a link that has never been timed
    /// must never render as a 0ms ping.
    #[test]
    fn an_unmeasured_link_carries_no_ping() {
        let no_srtt = serde_json::json!({
            "npub": "npub1x",
            "connectivity": "connected",
            "mmp": { "mode": "reliable", "loss_rate": 0.0 },
        });
        assert_eq!(peer_from_json(&no_srtt).srtt_ms, None);

        let no_mmp = serde_json::json!({ "npub": "npub1x", "connectivity": "connected" });
        assert_eq!(peer_from_json(&no_mmp).srtt_ms, None);
    }

    #[test]
    fn a_row_missing_conditional_keys_still_maps() {
        let row = serde_json::json!({ "npub": "npub1x", "connectivity": "stale" });
        let view = peer_from_json(&row);
        assert!(view.connected);
        assert!(view.transport.is_empty());
        assert_eq!(view.last_seen_ms, 0);
        // A peer row without it must read as "unknown", never as "aged zero" —
        // the UI shows a dash rather than an age computed from the epoch.
        assert_eq!(view.authenticated_at_ms, 0);
    }

    /// The `paths` array from the multi-path branch: the UDP instance name is
    /// what separates Aware from the LAN lane, and `active` marks the one path
    /// fips sends on.
    #[test]
    fn maps_every_path_with_its_lane_and_active_flag() {
        let row = serde_json::json!({
            "npub": "npub1x",
            "connectivity": "connected",
            "transport_type": "ble",
            "paths": [
                { "transport_id": 1, "transport": "ble", "transport_type": "ble",
                  "addr": "hci0/AA:BB:CC:DD:EE:FF", "state": "live", "active": true,
                  "role": "backup", "min_rtt_ms": 33, "rtt_samples": 12, "etx": 1.1, "score": 1.463 },
                { "transport_id": 3, "transport": "aware2", "transport_type": "udp",
                  "addr": "[fe80::1]:4872", "state": "probing", "active": false },
                { "transport_id": 2, "transport": "lan", "transport_type": "udp",
                  "addr": "[::ffff:192.168.8.2]:4871", "state": "dead", "active": false },
                { "transport_id": 4, "transport": null, "transport_type": "tcp",
                  "addr": "[::1]:1", "state": "live", "active": false },
            ],
        });
        let paths = peer_from_json(&row).paths;
        let lanes: Vec<&str> = paths.iter().map(|p| p.lane.as_str()).collect();
        assert_eq!(lanes, ["ble", "aware", "udp", "tcp"]);
        let states: Vec<&str> = paths.iter().map(|p| p.state.as_str()).collect();
        assert_eq!(states, ["live", "probing", "dead", "live"]);
        assert_eq!(
            paths
                .iter()
                .filter(|p| p.active)
                .map(|p| p.lane.as_str())
                .collect::<Vec<_>>(),
            ["ble"]
        );
        assert_eq!(paths[0].role, "backup");
        assert_eq!(paths[0].min_rtt_ms, Some(33));
        assert_eq!(paths[0].rtt_samples, 12);
        assert_eq!(paths[0].etx, 1.1);
        assert_eq!(paths[0].score, Some(1.463));
        // An unmeasured standby: no min RTT, no score — never a confident 0.
        assert_eq!(paths[1].min_rtt_ms, None);
        assert_eq!(paths[1].score, None);
        assert_eq!(paths[1].rtt_samples, 0);
    }

    /// A daemon without multi-path has no `paths` key; that is "no paths",
    /// not one path made up from the row's `transport_type`.
    #[test]
    fn a_row_without_paths_maps_to_none() {
        let row = serde_json::json!({ "npub": "npub1x", "connectivity": "connected", "transport_type": "ble" });
        assert!(peer_from_json(&row).paths.is_empty());
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
