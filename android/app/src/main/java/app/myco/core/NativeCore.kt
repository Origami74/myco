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
}
