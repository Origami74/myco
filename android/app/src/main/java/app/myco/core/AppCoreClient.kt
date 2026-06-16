package app.myco.core

import org.json.JSONObject

/** A peer seen/connected over BLE (keyed by node_addr, not MAC). */
data class BlePeer(
    val nodeAddrHex: String,
    val npub: String,
    val connected: Boolean,
    val psm: Int,
    val rssi: Int?,
)

/** A raw scan advert (radio-level view, keyed by BLE address). */
data class BleAdvert(
    val addr: String,
    val psm: Int,
    val rssi: Int,
)

/** Parsed slice of the core's state snapshot (P1 surface: identity + node + BLE). */
data class AppState(
    val rev: Long,
    val error: String,
    val appVersion: String,
    val ownNpub: String,
    val ownPubkeyHex: String,
    val nodeAddrHex: String,
    val fipsAddr: String,
    val nodeRunning: Boolean,
    val nodeStatus: String,
    val bleEnabled: Boolean,
    val bleRole: String,
    val bleScanning: Boolean,
    val bleAdapterName: String,
    val blePeers: List<BlePeer>,
    val bleAdverts: List<BleAdvert>,
) {
    companion object {
        fun parse(json: String): AppState {
            val o = JSONObject(json)
            val id = o.optJSONObject("identity") ?: JSONObject()
            val node = o.optJSONObject("node") ?: JSONObject()
            val ble = o.optJSONObject("ble") ?: JSONObject()
            val peersJson = o.optJSONArray("blePeers")
            val peers = buildList {
                if (peersJson != null) {
                    for (i in 0 until peersJson.length()) {
                        val p = peersJson.optJSONObject(i) ?: continue
                        add(
                            BlePeer(
                                nodeAddrHex = p.optString("nodeAddrHex"),
                                npub = p.optString("npub"),
                                connected = p.optBoolean("connected"),
                                psm = p.optInt("psm"),
                                rssi = if (p.isNull("rssi")) null else p.optInt("rssi"),
                            )
                        )
                    }
                }
            }
            val advertsJson = o.optJSONArray("bleAdverts")
            val adverts = buildList {
                if (advertsJson != null) {
                    for (i in 0 until advertsJson.length()) {
                        val a = advertsJson.optJSONObject(i) ?: continue
                        add(BleAdvert(addr = a.optString("addr"), psm = a.optInt("psm"), rssi = a.optInt("rssi")))
                    }
                }
            }
            return AppState(
                rev = o.optLong("rev"),
                error = o.optString("error"),
                appVersion = o.optString("appVersion"),
                ownNpub = id.optString("ownNpub"),
                ownPubkeyHex = id.optString("ownPubkeyHex"),
                nodeAddrHex = id.optString("nodeAddrHex"),
                fipsAddr = id.optString("fipsAddr"),
                nodeRunning = node.optBoolean("running"),
                nodeStatus = node.optString("statusText"),
                bleEnabled = ble.optBoolean("enabled"),
                bleRole = ble.optString("role"),
                bleScanning = ble.optBoolean("scanning"),
                bleAdapterName = ble.optString("adapterName"),
                blePeers = peers,
                bleAdverts = adverts,
            )
        }
    }
}

/**
 * Thin AutoCloseable wrapper over the opaque native handle. Guards against
 * double-free by zeroing the stored handle on close.
 */
class AppCoreClient(dataDir: String, appVersion: String) : AutoCloseable {
    private var handle: Long = NativeCore.appNew(dataDir, appVersion)

    fun state(): AppState = AppState.parse(NativeCore.stateJson(requireHandle()))

    fun refresh(): AppState = AppState.parse(NativeCore.refreshJson(requireHandle()))

    fun dispatch(action: JSONObject): AppState =
        AppState.parse(NativeCore.dispatchJson(requireHandle(), action.toString()))

    override fun close() {
        val current = handle
        if (current != 0L) {
            NativeCore.appFree(current)
            handle = 0
        }
    }

    /** The opaque native handle (passed to bleBridgeNew). 0 if closed. */
    fun handle(): Long = handle

    private fun requireHandle(): Long {
        check(handle != 0L) { "native app core is closed" }
        return handle
    }
}

/** Builders for the reducer actions (see docs/reference/ffi-surface.md). */
object NativeActions {
    fun tick(): JSONObject = JSONObject().put("type", "tick")
    fun startNode(): JSONObject = JSONObject().put("type", "start_node")
    fun stopNode(): JSONObject = JSONObject().put("type", "stop_node")
    fun setBleEnabled(enabled: Boolean): JSONObject =
        JSONObject().put("type", "set_ble_enabled").put("enabled", enabled)
}
