package app.myco.core

import android.content.Context

/**
 * JNI bindings to `libmyco_core.so`. The contract is a Redux-style reducer:
 * `dispatchJson(actionJson) -> stateJson`, with a monotonic `rev` in the state.
 * See docs/reference/ffi-surface.md.
 */
internal object NativeCore {
    init {
        System.loadLibrary("myco_core")
    }

    external fun initializeAndroidContext(context: Context)
    external fun appNew(dataDir: String, appVersion: String): Long
    external fun appFree(handle: Long)
    external fun stateJson(handle: Long): String
    external fun refreshJson(handle: Long): String
    external fun dispatchJson(handle: Long, actionJson: String): String

    /**
     * Serve one nsite request for the in-app WebView's `shouldInterceptRequest`
     * (the TUN-independent serve path). Returns a framed byte array:
     * `[u32 BE header-len][header JSON][body]`, where the header JSON is
     * `{status, contentType, headers}`. `range` is the request's `Range` header
     * (empty string if none). Blocks while the in-process gateway serves direct
     * from the local relay + Blossom.
     */
    external fun gatewayGet(
        handle: Long,
        host: String,
        path: String,
        range: String,
        allowSync: Boolean,
    ): ByteArray

    // --- BLE byte-bridge (see docs/reference/ffi-surface.md "BLE bridge") ---
    // The Kotlin radio (BleRadio) calls these to push inbound bytes/events and
    // pull outbound bytes. The Rust core calls back into the BleRadio object for
    // control (listen/connect/advertise/scan/close).

    /** Create the bridge over a BleRadio, inject it into the core, return a handle. */
    external fun bleBridgeNew(appHandle: Long, radio: Any): Long
    external fun bleBridgeFree(bridgeHandle: Long)

    /** Kotlin → Rust pushes (non-blocking). */
    external fun bleDeliverInbound(bridgeHandle: Long, addr: String, sendMtu: Int, recvMtu: Int): Long
    external fun bleDeliverConnectResult(
        bridgeHandle: Long, connectId: Long, ok: Boolean, addr: String, sendMtu: Int, recvMtu: Int,
    ): Long
    external fun bleDeliverScan(bridgeHandle: Long, addr: String, psm: Int, rssi: Int)
    external fun bleChannelDeliverRecv(bridgeHandle: Long, chId: Long, data: ByteArray, len: Int): Boolean
    external fun bleChannelClosed(bridgeHandle: Long, chId: Long)

    /** Whether the scan loop is live right now, pushed from the scan
     *  callback's own start/stop/retry-failure sites. Observed radio state for
     *  the developer diagnostics UI only. */
    external fun bleDeliverScanningState(bridgeHandle: Long, on: Boolean)

    /** Whether the advertiser is live right now, pushed from the advertise
     *  callback's own install/clear sites. Observed radio state for the
     *  developer diagnostics UI only. */
    external fun bleDeliverAdvertisingState(bridgeHandle: Long, on: Boolean)

    /** Rust → Kotlin pull (blocks up to timeoutMs): >0 len, 0 timeout, -1 closed. */
    external fun bleChannelNextSend(bridgeHandle: Long, chId: Long, out: ByteArray, timeoutMs: Int): Int

    // --- Wi-Fi Aware control bridge (see docs/design/wifi-aware-interop.md) ---
    // Control-plane only: no byte bridge. The AwareRadio drives discovery
    // itself and pushes peer reachability into the core's platform peer queue;
    // the bytes ride the ordinary UDP transport over the Aware data-path
    // interface.

    /** Aware data path up: peer `npub` reachable at `addr` ("[fe80::x%ifindex]:port"),
     *  observed on `lane` ("aware" for the Wi-Fi Aware radio, "udp" for the
     *  LAN/AP radio). Both lanes ride the same fips UDP transport and are
     *  otherwise indistinguishable, so `lane` is the disambiguation the Dev
     *  tab renders — it is recorded core-side and never reaches fips. */
    external fun awarePeerFound(npub: String, addr: String, lane: String)

    /** Aware data path to `npub` lost: close the pooled UDP session. `lane`
     *  must match the lane that pushed the corresponding [awarePeerFound],
     *  so a stale loss from one lane cannot clobber a fresher record from
     *  the other. */
    external fun awarePeerLost(npub: String, lane: String)

    /** Whether the Aware publish/subscribe session pair is live right now —
     *  the Aware analogue of a BLE scan. Observed radio state for the
     *  developer diagnostics UI only. */
    external fun awareSetDiscovering(on: Boolean)

    /** The underlying network's real DNS servers, comma-separated. The mesh
     *  tunnel advertises only its own sentinel resolver, so these are where the
     *  core relays every non-`.fips` query — without them the device resolves
     *  nothing but mesh names. */
    external fun setUpstreamDns(servers: String)

    /** Raw fd of the node's UDP transport socket, once it has opened. Blocks
     *  up to `timeoutMs`; returns -1 on timeout. Sent once per node lifetime —
     *  poll this once at startup, then bind it to whichever local-only
     *  network (Wi-Fi Aware NDP, the `!FIPS` AP) is carrying a
     *  platform-pushed peer via `Network.bindSocket`, so handshake replies
     *  aren't lost to a competing validated default network (e.g. cellular). */
    external fun nextUdpTransportFd(timeoutMs: Int): Int

    // --- TUN packet bridge (the app-owned TUN; the VpnService pumps these) ---
    /** Kotlin → Rust: route an IPv6 packet read from the TUN fd into the mesh. */
    external fun tunSendPacket(packet: ByteArray, len: Int): Boolean

    /** Rust → Kotlin: pull the next IPv6 packet for the TUN fd, blocking up to
     *  timeoutMs. Returns bytes written into `out`, or 0 on timeout. */
    external fun tunNextPacket(out: ByteArray, timeoutMs: Int): Int
}
