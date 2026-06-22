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
    /** A staged newer version finished downloading but isn't active yet. */
    val updateAvailable: Boolean = false,
    /** Download progress of a staging update (0/0 when none). */
    val updatePulled: Long = 0,
    val updateTotal: Long = 0,
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

/** A Circle contact: a paired peer we pull nsites from over the mesh. */
data class CircleContact(
    val npub: String,
    val name: String,
    val addedAt: Long,
)

/** An nsite discovered on a Circle peer's relay ("nsites around me"). */
data class DiscoveredNsite(
    val host: String,
    val authorNpub: String,
    val dTag: String?,
    val title: String,
    /** Unix seconds of the manifest version seen (its `created_at`); 0 if unknown. */
    val updatedAt: Long,
    /** The Circle peer who has it — the holder to pull from on open. */
    val holderNpub: String,
    val holderName: String,
)

/** An incoming pair request awaiting accept/decline (shown as a pop-up). */
data class PairRequest(
    val npub: String,
    val name: String,
    val secret: String,
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
    val fipsMtu: Int,
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
    val circle: List<CircleContact>,
    val discovered: List<DiscoveredNsite>,
    val pendingPairRequests: List<PairRequest>,
    val offlineOnly: Boolean,
    val updateCheck: UpdateCheck = UpdateCheck(),
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
                                updateAvailable = s.optBoolean("updateAvailable"),
                                updatePulled = s.optLong("updatePulled"),
                                updateTotal = s.optLong("updateTotal"),
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
            val circleJson = o.optJSONArray("circle")
            val circle = buildList {
                if (circleJson != null) {
                    for (i in 0 until circleJson.length()) {
                        val c = circleJson.optJSONObject(i) ?: continue
                        add(
                            CircleContact(
                                npub = c.optString("npub"),
                                name = c.optString("name"),
                                addedAt = c.optLong("addedAt"),
                            )
                        )
                    }
                }
            }
            val discoveredJson = o.optJSONArray("discovered")
            val discovered = buildList {
                if (discoveredJson != null) {
                    for (i in 0 until discoveredJson.length()) {
                        val d = discoveredJson.optJSONObject(i) ?: continue
                        add(
                            DiscoveredNsite(
                                host = d.optString("host"),
                                authorNpub = d.optString("authorNpub"),
                                dTag = if (d.isNull("dTag")) null else d.optString("dTag"),
                                title = d.optString("title"),
                                updatedAt = d.optLong("updatedAt"),
                                holderNpub = d.optString("holderNpub"),
                                holderName = d.optString("holderName"),
                            )
                        )
                    }
                }
            }
            val pairsJson = o.optJSONArray("pendingPairRequests")
            val pendingPairRequests = buildList {
                if (pairsJson != null) {
                    for (i in 0 until pairsJson.length()) {
                        val p = pairsJson.optJSONObject(i) ?: continue
                        add(
                            PairRequest(
                                npub = p.optString("npub"),
                                name = p.optString("name"),
                                secret = p.optString("secret"),
                            )
                        )
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
                fipsIpv6 = id.optString("fipsIpv6"),
                fipsMtu = id.optInt("fipsMtu"),
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
                circle = circle,
                discovered = discovered,
                pendingPairRequests = pendingPairRequests,
                offlineOnly = o.optBoolean("offlineOnly"),
                updateCheck = o.optJSONObject("updateCheck")?.let { u ->
                    UpdateCheck(
                        checking = u.optBoolean("checking"),
                        message = u.optString("message"),
                        generation = u.optLong("generation"),
                    )
                } ?: UpdateCheck(),
            )
        }
    }
}

/** Feedback for the "Check for updates" action: in-progress + last result. */
data class UpdateCheck(
    val checking: Boolean = false,
    val message: String = "",
    val generation: Long = 0,
)

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
    fun forgetNsite(link: String): JSONObject =
        JSONObject().put("type", "forget_nsite").put("link", link)
    fun checkNsiteUpdates(): JSONObject = JSONObject().put("type", "check_nsite_updates")
    fun wipeStores(): JSONObject = JSONObject().put("type", "wipe_stores")

    // --- circle (paired peers) ---
    /** Add a paired peer (from a scanned share QR) to the Circle. */
    fun addToCircle(npub: String, name: String): JSONObject =
        JSONObject().put("type", "add_to_circle").put("npub", npub).put("name", name)
    /** Forget a paired peer. */
    fun removeFromCircle(npub: String): JSONObject =
        JSONObject().put("type", "remove_from_circle").put("npub", npub)

    // --- mutual pairing ---
    fun sendPairRequest(npub: String, name: String, secret: String): JSONObject =
        JSONObject().put("type", "send_pair_request").put("npub", npub).put("name", name).put("secret", secret)

    fun acceptPairRequest(npub: String, name: String): JSONObject =
        JSONObject().put("type", "accept_pair_request").put("npub", npub).put("name", name)

    fun declinePairRequest(npub: String): JSONObject =
        JSONObject().put("type", "decline_pair_request").put("npub", npub)

    /** Discover nsites on connected Circle peers' relays ("nsites around me"). */
    fun searchNsites(): JSONObject = JSONObject().put("type", "search_nsites")

    /** Toggle mesh-only: when enabled, don't use the public IP relay/Blossom fallback. */
    fun setOfflineOnly(enabled: Boolean): JSONObject =
        JSONObject().put("type", "set_offline_only").put("enabled", enabled)
}
