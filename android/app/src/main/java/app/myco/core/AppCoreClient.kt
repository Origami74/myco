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
    /** A custom relay is configured, so the built-in event store is not serving. */
    val externalRelay: Boolean = false,
    /** A custom Blossom is configured, so the built-in blob store is not serving. */
    val externalBlobs: Boolean = false,
)

/** The configured custom relay, and why it is unreachable if it is. */
data class RelayBackendHealth(
    /** Empty when the built-in store is in use. */
    val url: String = "",
    /** Empty when reachable, or when there is no custom relay. */
    val error: String = "",
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

/** An invite we sent that nobody has accepted yet. */
data class OutboundPair(
    val npub: String,
    val name: String,
    val since: Long,
)

/** An incoming pair request awaiting accept/decline (shown as a pop-up). */
data class PairRequest(
    val npub: String,
    val name: String,
    val secret: String,
)

/** Native paired-peer file transfer, including incoming offers awaiting a decision. */
data class FileTransfer(
    val id: String,
    val direction: String,
    val peerNpub: String,
    val peerName: String,
    val name: String,
    val mime: String,
    val size: Long,
    val status: String,
    val blobHash: String,
    val receivedPath: String,
    val publishPending: Boolean,
    val error: String,
    val updatedAt: Long,
)

/**
 * One merged, npub-or-address-keyed peer diagnostics row (DIAG-01/03/04/06),
 * built once in Rust from `ble_peers` / `ble_adverts` / `circle` / pending
 * pairings — never re-joined here. `key` is the `npub` when resolved, else
 * `nodeAddrHex`, else the raw BLE address of an unresolved advert.
 */
data class PeerDiagnostic(
    val key: String,
    val npub: String,
    val nodeAddrHex: String,
    val bleAddr: String,
    val name: String,
    /** "connected" | "reachable-via-relay" | "seen-unidentified" | "paired-offline" | "unreachable". */
    val state: String,
    /** Transport carrying this row when connected ("ble", "aware", "udp", "tcp"); empty otherwise. */
    val transport: String,
    val alsoReachableVia: List<String> = emptyList(),
    /** 0 when never heard from — renders as an em-dash, never "0s". */
    val lastSeenMs: Long,
    /** Epoch ms the FMP session authenticated; 0 when there is no session.
     *  The age from this is the session's, not the current FMP index's — the
     *  index rotates on every rekey (~120s) while the session survives. */
    val authenticatedAtMs: Long = 0L,
    /** The name this peer broadcasts for itself in its BLE scan response; empty
     *  when it advertised none or was not found over BLE.
     *
     *  **Unauthenticated** — anyone in radio range can forge it. Ranks below
     *  every name learned from signed pair traffic, never above. */
    val advertisedName: String = "",
    /** Smoothed round-trip time over this peer's link in milliseconds, as MMP
     *  measured it; null when the link has never been timed (renders as an
     *  em-dash, never "0ms"). */
    val srttMs: Double? = null,
    val rssi: Int?,
    val psm: Int,
    /** "" | "incoming-waiting" | "outbound-waiting" | "paired". */
    val pairState: String,
    val inCircle: Boolean,
    /** BLE role this device chose on the most recent recorded attempt:
     *  "central" | "peripheral" | "" when nothing has been recorded. Never a guess. */
    val role: String = "",
    /** Discovery-to-resolution milliseconds for the newest attempt; 0 when unmeasured. */
    val discoveryMs: Long = 0L,
    /** Link-level send failures to this peer. Excludes MTU rejections. */
    val sendDrops: Long = 0L,
    /** Recorded connect attempts, newest first, capped at 20 by the core. */
    val attempts: List<PeerAttempt> = emptyList(),
)

/**
 * One recorded BLE connect attempt (DIAG-01/03), as surfaced on a peer row.
 *
 * Every field is an observed fact from the core's attempt log — a peer with no
 * recorded history carries an empty list, never a fabricated entry.
 */
data class PeerAttempt(
    val atMs: Long,
    /** "central" | "peripheral". */
    val role: String,
    val discoveryMs: Long,
    /** "connected" | "connect-timeout" | "connect-error" | "pubkey-exchange-failed"
     *  | "lost-tiebreaker" | "pool-rejected" | "duplicate-node". */
    val outcome: String,
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
    /** False when [bleScanning] could not actually be observed (bridge absent,
     *  radio never started, or a non-Android build) — render unknown, not a
     *  value. */
    val bleScanningKnown: Boolean = false,
    val bleAdvertising: Boolean = false,
    /** False when [bleAdvertising] could not actually be observed. */
    val bleAdvertisingKnown: Boolean = false,
    val bleAdapterName: String,
    val blePeers: List<BlePeer>,
    val bleAdverts: List<BleAdvert>,
    val wifiAwareEnabled: Boolean,
    /** Base port of the Aware socket pool: slot *i* listens on `port + i`, and
     *  a peer is told the port of the slot pinned to its own data path. */
    val wifiAwarePort: Int,
    /** How many peers the Aware lane can carry at once — the number of UDP
     *  transport instances the core bound. One socket can be marked for only
     *  one data path, so this is the pool size, not a chipset limit. */
    val wifiAwareSlots: Int = 1,
    /** Whether the Aware lane is actively discovering right now, sourced from
     *  the publish/subscribe session lifecycle. */
    val wifiAwareScanning: Boolean = false,
    /** False when [wifiAwareScanning] could not actually be observed. */
    val wifiAwareScanningKnown: Boolean = false,
    val sites: List<SiteStatus>,
    val library: List<LibraryItem>,
    val cache: CacheStatus,
    val circle: List<CircleContact>,
    /** Circle members with a live mesh relay connection right now — reachable
     *  at any hop count, not just direct neighbours. */
    val reachableNpubs: Set<String> = emptySet(),
    val discovered: List<DiscoveredNsite>,
    val pendingPairRequests: List<PairRequest>,
    /** Invites we sent that are still waiting to be accepted. */
    val outboundPairs: List<OutboundPair> = emptyList(),
    val offlineOnly: Boolean,
    /** The configured custom relay and whether it can be reached. */
    val relayBackend: RelayBackendHealth = RelayBackendHealth(),
    /** The custom relay URL as last saved; may differ from the one in use until restart. */
    val pendingRelayUrl: String = "",
    /** The configured custom Blossom and whether it can be reached. */
    val blobBackend: RelayBackendHealth = RelayBackendHealth(),
    /** The custom Blossom URL as last saved. */
    val pendingBlossomUrl: String = "",
    val updateCheck: UpdateCheck = UpdateCheck(),
    val fileTransfers: List<FileTransfer> = emptyList(),
    val speedtest: SpeedtestStatus = SpeedtestStatus(),
    /** Merged per-identity peer diagnostics rows (DIAG-01/03/04/06). Built once
     *  in Rust; the UI renders this directly, it never re-joins blePeers/circle. */
    val peers: List<PeerDiagnostic> = emptyList(),
) {
    companion object {
        fun parse(json: String): AppState {
            val o = JSONObject(json)
            val id = o.optJSONObject("identity") ?: JSONObject()
            val node = o.optJSONObject("node") ?: JSONObject()
            val ble = o.optJSONObject("ble") ?: JSONObject()
            val wifiAware = o.optJSONObject("wifiAware") ?: JSONObject()
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
                externalRelay = cacheJson.optBoolean("externalRelay"),
                externalBlobs = cacheJson.optBoolean("externalBlobs"),
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
            val peerDiagnosticsJson = o.optJSONArray("peers")
            val peerDiagnostics = buildList {
                if (peerDiagnosticsJson != null) {
                    for (i in 0 until peerDiagnosticsJson.length()) {
                        val p = peerDiagnosticsJson.optJSONObject(i) ?: continue
                        val alsoReachableVia = buildList {
                            p.optJSONArray("alsoReachableVia")?.let { arr ->
                                for (j in 0 until arr.length()) add(arr.optString(j))
                            }
                        }
                        // Attempts arrive newest-first from the core, already
                        // capped per peer. A payload predating plan 01-03 simply
                        // has no `attempts` key and parses to an empty list.
                        val attempts = buildList {
                            p.optJSONArray("attempts")?.let { arr ->
                                for (j in 0 until arr.length()) {
                                    val a = arr.optJSONObject(j) ?: continue
                                    add(
                                        PeerAttempt(
                                            atMs = a.optLong("atMs"),
                                            role = a.optString("role"),
                                            discoveryMs = a.optLong("discoveryMs"),
                                            outcome = a.optString("outcome"),
                                        )
                                    )
                                }
                            }
                        }
                        add(
                            PeerDiagnostic(
                                key = p.optString("key"),
                                npub = p.optString("npub"),
                                nodeAddrHex = p.optString("nodeAddrHex"),
                                bleAddr = p.optString("bleAddr"),
                                name = p.optString("name"),
                                state = p.optString("state"),
                                transport = p.optString("transport"),
                                alsoReachableVia = alsoReachableVia,
                                lastSeenMs = p.optLong("lastSeenMs"),
                                authenticatedAtMs = p.optLong("authenticatedAtMs"),
                                advertisedName = p.optString("advertisedName"),
                                srttMs = if (p.isNull("srttMs")) null else p.optDouble("srttMs"),
                                rssi = if (p.isNull("rssi")) null else p.optInt("rssi"),
                                psm = p.optInt("psm"),
                                pairState = p.optString("pairState"),
                                inCircle = p.optBoolean("inCircle"),
                                role = p.optString("role"),
                                discoveryMs = p.optLong("discoveryMs"),
                                sendDrops = p.optLong("sendDrops"),
                                attempts = attempts,
                            )
                        )
                    }
                }
            }
            val outboundJson = o.optJSONArray("outboundPairs")
            val outboundPairs = buildList {
                if (outboundJson != null) {
                    for (i in 0 until outboundJson.length()) {
                        val p = outboundJson.optJSONObject(i) ?: continue
                        add(
                            OutboundPair(
                                npub = p.optString("npub"),
                                name = p.optString("name"),
                                since = p.optLong("since"),
                            )
                        )
                    }
                }
            }
            val transfersJson = o.optJSONArray("fileTransfers")
            val fileTransfers = buildList {
                if (transfersJson != null) {
                    for (i in 0 until transfersJson.length()) {
                        val t = transfersJson.optJSONObject(i) ?: continue
                        add(
                            FileTransfer(
                                id = t.optString("id"),
                                direction = t.optString("direction"),
                                peerNpub = t.optString("peerNpub"),
                                peerName = t.optString("peerName"),
                                name = t.optString("name"),
                                mime = t.optString("mime"),
                                size = t.optLong("size"),
                                status = t.optString("status"),
                                blobHash = t.optString("blobHash"),
                                receivedPath = t.optString("receivedPath"),
                                publishPending = t.optBoolean("publishPending", false),
                                error = t.optString("error"),
                                updatedAt = t.optLong("updatedAt"),
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
                bleScanningKnown = ble.optBoolean("scanningKnown"),
                bleAdvertising = ble.optBoolean("advertising"),
                bleAdvertisingKnown = ble.optBoolean("advertisingKnown"),
                bleAdapterName = ble.optString("adapterName"),
                blePeers = peers,
                bleAdverts = adverts,
                wifiAwareEnabled = wifiAware.optBoolean("enabled"),
                wifiAwarePort = wifiAware.optInt("port"),
                wifiAwareSlots = wifiAware.optInt("slots", 1).coerceAtLeast(1),
                wifiAwareScanning = wifiAware.optBoolean("scanning"),
                wifiAwareScanningKnown = wifiAware.optBoolean("scanningKnown"),
                sites = sites,
                library = library,
                cache = cache,
                circle = circle,
                reachableNpubs = buildSet {
                    o.optJSONArray("reachableNpubs")?.let { arr ->
                        for (i in 0 until arr.length()) add(arr.optString(i))
                    }
                },
                discovered = discovered,
                pendingPairRequests = pendingPairRequests,
                outboundPairs = outboundPairs,
                offlineOnly = o.optBoolean("offlineOnly"),
                relayBackend = o.optJSONObject("relayBackend").let { rb ->
                    RelayBackendHealth(
                        url = rb?.optString("url").orEmpty(),
                        error = rb?.optString("error").orEmpty(),
                    )
                },
                pendingRelayUrl = o.optString("pendingRelayUrl"),
                blobBackend = o.optJSONObject("blobBackend").let { bb ->
                    RelayBackendHealth(
                        url = bb?.optString("url").orEmpty(),
                        error = bb?.optString("error").orEmpty(),
                    )
                },
                pendingBlossomUrl = o.optString("pendingBlossomUrl"),
                speedtest = o.optJSONObject("speedtest")?.let { s ->
                    SpeedtestStatus(
                        running = s.optBoolean("running"),
                        peerNpub = s.optString("peerNpub"),
                        bytes = s.optLong("bytes"),
                        upMbps = s.optDouble("upMbps", 0.0),
                        downMbps = s.optDouble("downMbps", 0.0),
                        error = s.optString("error"),
                        generation = s.optLong("generation"),
                    )
                } ?: SpeedtestStatus(),
                updateCheck = o.optJSONObject("updateCheck")?.let { u ->
                    UpdateCheck(
                        checking = u.optBoolean("checking"),
                        message = u.optString("message"),
                        generation = u.optLong("generation"),
                    )
                } ?: UpdateCheck(),
                fileTransfers = fileTransfers,
                peers = peerDiagnostics,
            )
        }
    }
}

/** Result of the dev-menu peer speedtest (a Blossom upload+download round-trip). */
data class SpeedtestStatus(
    val running: Boolean = false,
    val peerNpub: String = "",
    val bytes: Long = 0,
    val upMbps: Double = 0.0,
    val downMbps: Double = 0.0,
    val error: String = "",
    val generation: Long = 0,
)

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
     *
     * [allowSync] must stay `true` for WebView loads — the user asked for that
     * site, so a missing one should start pulling. Pass `false` for a **passive
     * probe** the user did not ask for, such as a favicon behind a Discover
     * tile: a sync there downloads and pins every site merely rendered on
     * screen.
     */
    fun gatewayGet(
        host: String,
        path: String,
        range: String,
        allowSync: Boolean = true,
    ): GatewayResult {
        val framed = NativeCore.gatewayGet(requireHandle(), host, path, range, allowSync)
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
    fun setWifiAwareEnabled(enabled: Boolean): JSONObject =
        JSONObject().put("type", "set_wifi_aware_enabled").put("enabled", enabled)

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
    /** Clear cached relay/Blossom data but keep pinned nsites (Storage → "Delete cache"). */
    fun wipeCache(): JSONObject = JSONObject().put("type", "wipe_cache")

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

    /** Withdraw an invite still waiting to be accepted. */
    fun cancelPairInvite(npub: String): JSONObject =
        JSONObject().put("type", "cancel_pair_invite").put("npub", npub)

    /** Discover nsites on connected Circle peers' relays ("nsites around me"). */
    fun searchNsites(): JSONObject = JSONObject().put("type", "search_nsites")

    /**
     * Point the event store at [url], or back at the built-in store with an
     * empty string. Applied on the next launch — the backend is chosen when the
     * content layer is built.
     */
    fun setCustomRelay(url: String): JSONObject =
        JSONObject().put("type", "set_custom_relay").put("url", url)

    /**
     * Point the blob store at [url], or back at the built-in one with an empty
     * string. Applied on the next launch, like [setCustomRelay].
     */
    fun setCustomBlossom(url: String): JSONObject =
        JSONObject().put("type", "set_custom_blossom").put("url", url)

    /** Toggle mesh-only: when enabled, don't use the public IP relay/Blossom fallback. */
    fun setOfflineOnly(enabled: Boolean): JSONObject =
        JSONObject().put("type", "set_offline_only").put("enabled", enabled)

    /** Set this device's memorable name; stamped on outgoing pair events so peers
     *  see the chosen name. The app owns the value and re-applies it on launch. */
    fun setDeviceName(name: String): JSONObject =
        JSONObject().put("type", "set_device_name").put("name", name)

    /** Dev-menu speedtest: round-trip a payload through `npub`'s mesh Blossom. */
    fun speedtestPeer(npub: String): JSONObject =
        JSONObject().put("type", "speedtest_peer").put("npub", npub)

    /** Encrypt a local file and send a private offer to a paired peer. */
    fun shareFile(path: String, name: String, mime: String, peerNpub: String): JSONObject =
        JSONObject().put("type", "share_file")
            .put("path", path)
            .put("name", name)
            .put("mime", mime)
            .put("peerNpub", peerNpub)

    fun acceptFileTransfer(transferId: String): JSONObject =
        JSONObject().put("type", "accept_file_transfer").put("transferId", transferId)

    fun declineFileTransfer(transferId: String): JSONObject =
        JSONObject().put("type", "decline_file_transfer").put("transferId", transferId)

    /** Stop a transfer that is still in flight and tell the other phone. */
    fun cancelFileTransfer(transferId: String): JSONObject =
        JSONObject().put("type", "cancel_file_transfer").put("transferId", transferId)

    fun forgetFileTransfer(transferId: String): JSONObject =
        JSONObject().put("type", "forget_file_transfer").put("transferId", transferId)
}
