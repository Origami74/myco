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
    external fun gatewayGet(handle: Long, host: String, path: String, range: String): ByteArray

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

    /** Rust → Kotlin pull (blocks up to timeoutMs): >0 len, 0 timeout, -1 closed. */
    external fun bleChannelNextSend(bridgeHandle: Long, chId: Long, out: ByteArray, timeoutMs: Int): Int

    // --- Wi-Fi Aware control bridge (see docs/design/wifi-aware-interop.md) ---
    // Control-plane only: no byte bridge. The AwareRadio drives discovery
    // itself and pushes peer reachability into the core's platform peer queue;
    // the bytes ride the ordinary UDP transport over the Aware data-path
    // interface.

    /** Aware data path up: peer `npub` reachable at `addr` ("[fe80::x%ifindex]:port"). */
    external fun awarePeerFound(npub: String, addr: String)

    /** Aware data path to `npub` lost: close the pooled UDP session. */
    external fun awarePeerLost(npub: String)

    // --- TUN packet bridge (the app-owned TUN; the VpnService pumps these) ---
    /** Kotlin → Rust: route an IPv6 packet read from the TUN fd into the mesh. */
    external fun tunSendPacket(packet: ByteArray, len: Int): Boolean

    /** Rust → Kotlin: pull the next IPv6 packet for the TUN fd, blocking up to
     *  timeoutMs. Returns bytes written into `out`, or 0 on timeout. */
    external fun tunNextPacket(out: ByteArray, timeoutMs: Int): Int
}
