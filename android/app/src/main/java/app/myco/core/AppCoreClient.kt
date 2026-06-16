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

/** Per-site sync/readiness for an `OpenNsite` (keyed by the `<host>` label). */
data class SiteStatus(
    val host: String,
    val authorNpub: String,
    val dTag: String?,
    val title: String,
    /** "syncing" | "ready" | "unreachable" | "incomplete". */
    val state: String,
    val filesPulled: Long,
    val filesTotal: Long,
    val message: String,
)

/** A pinned/opened Library entry. */
data class LibraryItem(
    val authorNpub: String,
    val dTag: String?,
    val title: String,
    val urlHost: String,
    val pinned: Boolean,
)

/** Local relay/Blossom counts. */
data class CacheStatus(
    val relayEvents: Long,
    val blobCount: Long,
    val usedBytes: Long,
)

/** Parsed slice of the core's state snapshot (P1 BLE surface + P2 content). */
data class AppState(
    val rev: Long,
    val error: String,
    val appVersion: String,
    val ownNpub: String,
    val ownPubkeyHex: String,
    val nodeAddrHex: String,
    val fipsAddr: String,
    val fipsIpv6: String,
    val nodeRunning: Boolean,
    val nodeStatus: String,
    val bleEnabled: Boolean,
    val bleRole: String,
    val bleScanning: Boolean,
    val bleAdapterName: String,
    val blePeers: List<BlePeer>,
    val bleAdverts: List<BleAdvert>,
    val sites: List<SiteStatus>,
    val library: List<LibraryItem>,
    val cache: CacheStatus,
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
            val sitesJson = o.optJSONArray("sites")
            val sites = buildList {
                if (sitesJson != null) {
                    for (i in 0 until sitesJson.length()) {
                        val s = sitesJson.optJSONObject(i) ?: continue
                        add(
                            SiteStatus(
                                host = s.optString("host"),
                                authorNpub = s.optString("authorNpub"),
                                dTag = if (s.isNull("dTag")) null else s.optString("dTag"),
                                title = s.optString("title"),
                                state = s.optString("state"),
                                filesPulled = s.optLong("filesPulled"),
                                filesTotal = s.optLong("filesTotal"),
                                message = s.optString("message"),
                            )
                        )
                    }
                }
            }
            val libraryJson = o.optJSONArray("library")
            val library = buildList {
                if (libraryJson != null) {
                    for (i in 0 until libraryJson.length()) {
                        val l = libraryJson.optJSONObject(i) ?: continue
                        add(
                            LibraryItem(
                                authorNpub = l.optString("authorNpub"),
                                dTag = if (l.isNull("dTag")) null else l.optString("dTag"),
                                title = l.optString("title"),
                                urlHost = l.optString("urlHost"),
                                pinned = l.optBoolean("pinned"),
                            )
                        )
                    }
                }
            }
            val cacheJson = o.optJSONObject("cache") ?: JSONObject()
            val cache = CacheStatus(
                relayEvents = cacheJson.optLong("relayEvents"),
                blobCount = cacheJson.optLong("blobCount"),
                usedBytes = cacheJson.optLong("usedBytes"),
            )
            return AppState(
                rev = o.optLong("rev"),
                error = o.optString("error"),
                appVersion = o.optString("appVersion"),
                ownNpub = id.optString("ownNpub"),
                ownPubkeyHex = id.optString("ownPubkeyHex"),
                nodeAddrHex = id.optString("nodeAddrHex"),
                fipsAddr = id.optString("fipsAddr"),
                fipsIpv6 = id.optString("fipsIpv6"),
                nodeRunning = node.optBoolean("running"),
                nodeStatus = node.optString("statusText"),
                bleEnabled = ble.optBoolean("enabled"),
                bleRole = ble.optString("role"),
                bleScanning = ble.optBoolean("scanning"),
                bleAdapterName = ble.optString("adapterName"),
                blePeers = peers,
                bleAdverts = adverts,
                sites = sites,
                library = library,
                cache = cache,
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

    /**
     * Serve one nsite request through the in-process gateway (for the WebView's
     * `shouldInterceptRequest`). Decodes the framed `[u32 header-len][header][body]`
     * the native side returns. `range` is the request's `Range` header (or "").
     */
    fun gatewayGet(host: String, path: String, range: String): GatewayResult {
        val framed = NativeCore.gatewayGet(requireHandle(), host, path, range)
        return GatewayResult.decode(framed)
    }

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

/**
 * A decoded gateway response: status, content-type, extra headers, and the body
 * bytes. Decoded from the native `[u32 BE header-len][header JSON][body]` frame.
 */
data class GatewayResult(
    val status: Int,
    val contentType: String,
    val headers: List<Pair<String, String>>,
    val body: ByteArray,
) {
    /** MIME type without the `; charset=…` suffix (what WebResourceResponse wants). */
    val mimeType: String get() = contentType.substringBefore(';').trim().ifEmpty { "application/octet-stream" }

    /** Charset from the content-type, or null. */
    val encoding: String? get() =
        contentType.substringAfter("charset=", "").trim().ifEmpty { null }

    companion object {
        fun decode(framed: ByteArray): GatewayResult {
            if (framed.size < 4) {
                return GatewayResult(502, "text/plain", emptyList(), ByteArray(0))
            }
            val headerLen = ((framed[0].toInt() and 0xff) shl 24) or
                ((framed[1].toInt() and 0xff) shl 16) or
                ((framed[2].toInt() and 0xff) shl 8) or
                (framed[3].toInt() and 0xff)
            val header = JSONObject(String(framed, 4, headerLen, Charsets.UTF_8))
            val headersJson = header.optJSONArray("headers")
            val headers = buildList {
                if (headersJson != null) {
                    for (i in 0 until headersJson.length()) {
                        val pair = headersJson.optJSONArray(i) ?: continue
                        add(pair.optString(0) to pair.optString(1))
                    }
                }
            }
            val body = framed.copyOfRange(4 + headerLen, framed.size)
            return GatewayResult(
                status = header.optInt("status", 200),
                contentType = header.optString("contentType", "application/octet-stream"),
                headers = headers,
                body = body,
            )
        }
    }

    override fun equals(other: Any?): Boolean =
        this === other || (other is GatewayResult && status == other.status &&
            contentType == other.contentType && headers == other.headers && body.contentEquals(other.body))

    override fun hashCode(): Int = status * 31 + body.contentHashCode()
}

/** Builders for the reducer actions (see docs/reference/ffi-surface.md). */
object NativeActions {
    fun tick(): JSONObject = JSONObject().put("type", "tick")
    fun startNode(): JSONObject = JSONObject().put("type", "start_node")
    fun stopNode(): JSONObject = JSONObject().put("type", "stop_node")
    fun setBleEnabled(enabled: Boolean): JSONObject =
        JSONObject().put("type", "set_ble_enabled").put("enabled", enabled)

    // --- content (P2) ---
    /** `holder` is a sharer's device npub (from a share QR) to pull from over the
     *  mesh first; null for a plain pasted link. */
    fun openNsite(link: String, holder: String? = null): JSONObject =
        JSONObject().put("type", "open_nsite").put("link", link)
            .apply { if (!holder.isNullOrEmpty()) put("holder", holder) }
    fun importNsite(dir: String): JSONObject =
        JSONObject().put("type", "import_nsite").put("dir", dir)
    fun addToLibrary(link: String): JSONObject =
        JSONObject().put("type", "add_to_library").put("link", link)
    fun removeFromLibrary(link: String): JSONObject =
        JSONObject().put("type", "remove_from_library").put("link", link)
    fun wipeStores(): JSONObject = JSONObject().put("type", "wipe_stores")
}
