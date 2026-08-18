package app.myco.aware

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.wifi.aware.AttachCallback
import android.net.wifi.aware.DiscoverySessionCallback
import android.net.wifi.aware.PeerHandle
import android.net.wifi.aware.PublishConfig
import android.net.wifi.aware.PublishDiscoverySession
import android.net.wifi.aware.SubscribeConfig
import android.net.wifi.aware.SubscribeDiscoverySession
import android.net.wifi.aware.WifiAwareManager
import android.net.wifi.aware.WifiAwareNetworkInfo
import android.net.wifi.aware.WifiAwareNetworkSpecifier
import android.net.wifi.aware.WifiAwareSession
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import app.myco.core.NativeCore
import app.myco.core.UdpSocketPin
import java.net.Inet6Address
import java.util.concurrent.ConcurrentHashMap
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * The Wi-Fi Aware (NAN) bulk-lane radio. Control-plane only: it discovers
 * peers, brings up a data path (NDP), and pushes "peer reachable / lost" into
 * the core ([NativeCore.awarePeerFound]/[NativeCore.awarePeerLost]). The bytes
 * ride a fips UDP transport over the `aware_dataN` interface — this class never
 * touches a payload byte. See docs/design/wifi-aware-interop.md.
 *
 * That transport is **this lane's own** UDP socket, and this class pins it to
 * the NDP's [Network] ([udpPin]). Both halves matter. An NDP is a network of
 * its own with its own routing table, so a socket marked with any other
 * network — infrastructure Wi-Fi, as the AP lane marks it — cannot reach an
 * Aware peer at all: the address is well-formed, the send reports success, and
 * nothing arrives. That was the whole of the "Aware discovers everything and
 * peers with nothing" fault. See [UdpSocketPin].
 *
 * Flow, per peer:
 *  1. publish + subscribe the Myco service (symmetric, no group owner).
 *  2. on a subscribe match, exchange device npubs over Aware `sendMessage`
 *     (the analog of BLE's in-band pubkey exchange — no identity is in the
 *     advert itself).
 *  3. the smaller-npub side requests the NDP (the cross-probe tiebreaker,
 *     applied before spending a scarce data-path slot; the core backstops it).
 *  4. read the peer's scoped link-local IPv6 from [WifiAwareNetworkInfo] and
 *     push `awarePeerFound(npub, "[fe80::x%ifindex]:port")`.
 *
 * The listener port is a fixed app constant carried in the state
 * ([app.myco.core.AppState.wifiAwarePort]); both peers bind it, so there is no
 * PSM-style discovery problem and no need for `setPort()` on a secured NDP.
 * It is **not** the LAN lane's port — the two lanes bind different ports as
 * well as different sockets — so two phones must run matching builds to peer
 * over Aware at all. See `runtime.rs`'s `AWARE_UDP_PORT`.
 * The NDP is left **open** (no PSK) — fips authenticates with Noise IK.
 */
class AwareRadio(
    private val context: Context,
    /** This device's npub, sent in the pubkey exchange and used for the tiebreaker. */
    private val ownNpub: String,
    /** The fixed UDP port both peers bind. */
    private val port: Int,
) {
    private val manager: WifiAwareManager? =
        context.getSystemService(Context.WIFI_AWARE_SERVICE) as? WifiAwareManager
    private val connectivity =
        context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager

    private val thread = HandlerThread("myco-aware").apply { start() }
    private val handler = Handler(thread.looper)

    /** Pins this lane's UDP transport socket to the NDP [Network] — see the
     *  class doc, and [UdpSocketPin] for why the lane needs a socket of its
     *  own. Asks for [LANE] by name, so it can only ever receive the Aware
     *  lane's socket, never the AP lane's. */
    private val udpPin = UdpSocketPin(LANE, handler, TAG)

    private var session: WifiAwareSession? = null
    private var publishSession: PublishDiscoverySession? = null
    private var subscribeSession: SubscribeDiscoverySession? = null

    /** Peers we have exchanged npubs with, keyed by the (session-scoped) handle. */
    private val peerNpubs = ConcurrentHashMap<PeerHandle, String>()

    /** Live NDP requests, keyed by peer npub, so we can tear them down on stop. */
    private val ndpCallbacks = ConcurrentHashMap<String, ConnectivityManager.NetworkCallback>()

    @Volatile
    private var running = false

    /** True if Aware is present AND currently usable (Wi-Fi on, radio free). */
    fun isAvailable(): Boolean = manager?.isAvailable == true

    private var availabilityReceiver: android.content.BroadcastReceiver? = null

    /**
     * Start the lane. If Aware is available now, attach immediately; otherwise
     * register for [WifiAwareManager.ACTION_WIFI_AWARE_STATE_CHANGED] and attach
     * as soon as it becomes available — this is what makes the toggle "stick"
     * when the user enables it before turning Wi-Fi on (an app cannot turn
     * Wi-Fi on itself since API 29; [AwareService]/the UI pops the Wi-Fi panel).
     */
    fun start() {
        if (running) return
        val mgr = manager ?: run {
            Log.w(TAG, "no Wi-Fi Aware service")
            NativeCore.awareSetDiscovering(false)
            return
        }
        running = true
        udpPin.start()
        registerAvailability(mgr)
        if (mgr.isAvailable) {
            attach(mgr)
        } else {
            Log.i(TAG, "Aware not available yet (is Wi-Fi on?); waiting for it")
        }
    }

    /** The observed discovering state: live iff either session is up — the two
     *  sessions start and stop together in this lifecycle, so a single boolean
     *  does not under-report. */
    private fun discovering(): Boolean = publishSession != null || subscribeSession != null

    private fun attach(mgr: WifiAwareManager) {
        if (session != null) return
        try {
            mgr.attach(object : AttachCallback() {
                override fun onAttached(s: WifiAwareSession) {
                    if (!running) { s.close(); return }
                    session = s
                    startPublish(s)
                    startSubscribe(s)
                    Log.i(TAG, "Aware attached")
                }

                override fun onAttachFailed() {
                    Log.e(TAG, "Aware attach failed")
                }
            }, handler)
        } catch (e: SecurityException) {
            onPermissionDenied("attach", e)
        }
    }

    /**
     * The platform refused an Aware call for lack of NEARBY_WIFI_DEVICES /
     * fine-location permission. This can happen even after our own permission
     * check passed — GrapheneOS and secondary (non-admin) users enforce
     * differently — and the calls run on the Aware handler thread, where an
     * uncaught SecurityException kills the whole process. Flag it for the UI
     * and shut the lane down instead of crashing.
     */
    private fun onPermissionDenied(where: String, e: SecurityException) {
        Log.e(TAG, "Aware $where denied by platform (missing nearby/location permission)", e)
        AwareHealth.permissionDenied = true
        stop()
    }

    private fun registerAvailability(mgr: WifiAwareManager) {
        if (availabilityReceiver != null) return
        val receiver = object : android.content.BroadcastReceiver() {
            override fun onReceive(c: Context?, i: Intent?) {
                if (!running) return
                if (mgr.isAvailable) {
                    if (session == null) {
                        Log.i(TAG, "Aware became available; attaching")
                        attach(mgr)
                    }
                } else if (session != null) {
                    Log.i(TAG, "Aware became unavailable; dropping sessions")
                    closeSessions()
                }
            }
        }
        availabilityReceiver = receiver
        context.registerReceiver(
            receiver,
            android.content.IntentFilter(WifiAwareManager.ACTION_WIFI_AWARE_STATE_CHANGED),
        )
    }

    /** Drop NDPs + discovery + attach, but keep the availability watch (so the
     *  lane re-attaches if Aware flaps back). Called on availability loss. */
    private fun closeSessions() {
        for ((_, cb) in ndpCallbacks) runCatching { connectivity.unregisterNetworkCallback(cb) }
        ndpCallbacks.clear()
        peerNpubs.clear()
        _links.value = emptyList()
        runCatching { publishSession?.close() }
        runCatching { subscribeSession?.close() }
        runCatching { session?.close() }
        publishSession = null
        subscribeSession = null
        session = null
        NativeCore.awareSetDiscovering(discovering())
    }

    fun stop() {
        running = false
        availabilityReceiver?.let { runCatching { context.unregisterReceiver(it) } }
        availabilityReceiver = null
        closeSessions()
        // On the handler thread, where the pin's state lives. Releases our dup
        // of the socket; the core's own descriptor, and its binding, are
        // untouched — a stale mark on a network that has gone away is harmless,
        // and the next NDP re-pins.
        handler.post { udpPin.stop() }
    }

    fun shutdown() {
        stop()
        thread.quitSafely()
    }

    private fun startPublish(s: WifiAwareSession) {
        // No service-specific info: the advert carries no identity, exactly
        // like the UUID-only BLE advert. Identity is exchanged post-match.
        val config = PublishConfig.Builder().setServiceName(SERVICE_NAME).build()
        try {
            publish(s, config)
        } catch (e: SecurityException) {
            onPermissionDenied("publish", e)
        }
    }

    private fun publish(s: WifiAwareSession, config: PublishConfig) {
        s.publish(config, object : DiscoverySessionCallback() {
            override fun onPublishStarted(session: PublishDiscoverySession) {
                Log.i(TAG, "publish started")
                publishSession = session
                NativeCore.awareSetDiscovering(discovering())
            }

            // A subscriber reached us. Reply with our npub so it can label the
            // NDP. Then, if WE are the responder for this pair (larger npub),
            // request the data path on the publish session. Exactly one side is
            // responder and one is initiator — an NDP needs both, complementary.
            override fun onMessageReceived(peer: PeerHandle, message: ByteArray) {
                val peerNpub = parseNpub(message) ?: return
                peerNpubs[peer] = peerNpub
                publishSession?.sendMessage(peer, MSG_ID_NPUB, ownNpub.toByteArray())
                if (ownNpub > peerNpub) {
                    Log.i(TAG, "publish: responder for ${short(peerNpub)}; requesting NDP")
                    requestDataPath(publishSession, peer, peerNpub)
                }
            }
        }, handler)
    }

    private fun startSubscribe(s: WifiAwareSession) {
        val config = SubscribeConfig.Builder().setServiceName(SERVICE_NAME).build()
        try {
            subscribe(s, config)
        } catch (e: SecurityException) {
            onPermissionDenied("subscribe", e)
        }
    }

    private fun subscribe(s: WifiAwareSession, config: SubscribeConfig) {
        s.subscribe(config, object : DiscoverySessionCallback() {
            override fun onSubscribeStarted(session: SubscribeDiscoverySession) {
                Log.i(TAG, "subscribe started")
                subscribeSession = session
                NativeCore.awareSetDiscovering(discovering())
            }

            // We discovered a publisher: we are the INITIATOR toward it.
            // Introduce ourselves; it replies with its npub (below).
            override fun onServiceDiscovered(
                peer: PeerHandle,
                serviceSpecificInfo: ByteArray?,
                matchFilter: MutableList<ByteArray>?,
            ) {
                Log.i(TAG, "discovered a peer; sending our npub")
                subscribeSession?.sendMessage(peer, MSG_ID_NPUB, ownNpub.toByteArray())
            }

            // The publisher replied with its npub. If WE are the initiator for
            // this pair (smaller npub), request the NDP on the subscribe
            // session; the peer's publish side requests as responder.
            override fun onMessageReceived(peer: PeerHandle, message: ByteArray) {
                val peerNpub = parseNpub(message) ?: return
                peerNpubs[peer] = peerNpub
                if (ownNpub < peerNpub) {
                    Log.i(TAG, "subscribe: initiator for ${short(peerNpub)}; requesting NDP")
                    requestDataPath(subscribeSession, peer, peerNpub)
                }
            }
        }, handler)
    }

    private fun parseNpub(message: ByteArray): String? =
        message.toString(Charsets.UTF_8).takeIf { it.startsWith("npub1") }

    /**
     * Request an open NDP toward `peer` on the given discovery `session`. Both
     * ends request (initiator on its subscribe session, responder on its
     * publish session) — an NDP forms only when both do. Both ends then get
     * [android.net.ConnectivityManager.NetworkCallback.onCapabilitiesChanged]
     * and push `awarePeerFound`; FIPS's cross-connection resolution dedups the
     * two resulting UDP links to one Noise session.
     */
    private fun requestDataPath(session: android.net.wifi.aware.DiscoverySession?, peer: PeerHandle, peerNpub: String) {
        val sess = session ?: return
        if (ndpCallbacks.containsKey(peerNpub)) return
        Log.i(TAG, "requesting NDP to ${short(peerNpub)} (${logResources()})")
        // Open (unencrypted) NDP: no security setter. Noise IK is the trust
        // layer; a PSK here would be a redundant credential under it.
        val specifier = WifiAwareNetworkSpecifier.Builder(sess, peer).build()
        val request = NetworkRequest.Builder()
            .addTransportType(NetworkCapabilities.TRANSPORT_WIFI_AWARE)
            .setNetworkSpecifier(specifier)
            .build()

        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
                val info = caps.transportInfo as? WifiAwareNetworkInfo ?: return
                val addr = formatPeerAddr(info.peerIpv6Addr) ?: return
                Log.i(TAG, "Aware NDP up to ${short(peerNpub)} at $addr")
                // Pin BEFORE announcing the peer: the core dials as soon as it
                // is told, and a dial from an unpinned (or wrong-network)
                // socket is what used to time out. This callback repeats for
                // the life of the NDP, so the re-pin is idempotent and cheap.
                //
                // One socket, one mark: with several concurrent NDPs the most
                // recent one wins. Each NDP is a separate Network, so a single
                // socket cannot serve them all — the pair this milestone has to
                // get right is two phones, and a fan-out (a socket per NDP)
                // would need a transport instance per peer, which fips has no
                // way to configure at runtime.
                udpPin.bindTo(network)
                setLink(peerNpub, addr, up = true)
                NativeCore.awarePeerFound(peerNpub, addr, LANE)
            }

            override fun onLost(network: Network) {
                Log.i(TAG, "Aware NDP lost to ${short(peerNpub)}")
                udpPin.clearTarget(network)
                NativeCore.awarePeerLost(peerNpub, LANE)
                releaseNdp(peerNpub)
            }

            // Fired when the request can't be provisioned within the timeout
            // below — typically because the chipset's data-path slots are all
            // in use. Releasing the request frees the slot (an un-timed-out
            // request would hold it indefinitely), and dropping the map entry
            // lets a later rediscovery retry.
            override fun onUnavailable() {
                Log.w(TAG, "Aware NDP request unavailable for ${short(peerNpub)} — slots full? (${logResources()})")
                releaseNdp(peerNpub)
            }
        }
        ndpCallbacks[peerNpub] = callback
        setLink(peerNpub, addr = null, up = false)
        // Timed request: on failure to provision within NDP_TIMEOUT_MS the
        // framework calls onUnavailable and releases it, so a stuck negotiation
        // never leaks a data-path slot (the root cause of "works fresh, dies
        // after a few restarts" — slots pile up and never free).
        connectivity.requestNetwork(request, callback, NDP_TIMEOUT_MS)
    }

    /** Unregister and forget a peer's NDP request, freeing its data-path slot. */
    private fun releaseNdp(peerNpub: String) {
        ndpCallbacks.remove(peerNpub)?.let {
            runCatching { connectivity.unregisterNetworkCallback(it) }
        }
        removeLink(peerNpub)
    }

    /** Best-effort snapshot of free Aware data-path/session slots (API 31+),
     *  for logging why an NDP request might be refused. */
    private fun logResources(): String {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return "resources n/a"
        val r = manager?.availableAwareResources ?: return "resources unknown"
        return "dataPaths=${r.availableDataPathsCount} pub=${r.availablePublishSessionsCount} sub=${r.availableSubscribeSessionsCount}"
    }

    /**
     * Format the peer's link-local IPv6 as `"[fe80::x%ifindex]:port"` with a
     * **numeric** scope — the only form fips-core's address parser accepts
     * (interface-name scopes do not parse). The [Inet6Address] handed back by
     * [WifiAwareNetworkInfo] is already scoped to the local `aware_dataN`
     * interface, so its `scopeId` is the ifindex we need.
     */
    private fun formatPeerAddr(ipv6: Inet6Address?): String? {
        if (ipv6 == null) return null
        val scopeId = ipv6.scopeId
        if (scopeId == 0) {
            Log.w(TAG, "peer IPv6 has no scope id; cannot dial")
            return null
        }
        // hostAddress may render as "fe80::x%aware_data0" or "fe80::x%3";
        // strip any scope suffix and re-append the numeric ifindex.
        val bare = ipv6.hostAddress?.substringBefore('%') ?: return null
        return "[$bare%$scopeId]:$port"
    }

    private fun short(npub: String): String =
        if (npub.length > 12) npub.substring(0, 12) + "…" else npub

    companion object {
        private const val TAG = "MycoAwareRadio"

        private val _links = MutableStateFlow<List<AwareLink>>(emptyList())

        /** Live NDP links (requested + up), for the Dev screen. There is one
         *  radio per process (owned by [AwareService]), so a companion flow is
         *  safe; [closeSessions]/[stop] clear it. */
        val links: StateFlow<List<AwareLink>> = _links.asStateFlow()

        private fun setLink(npub: String, addr: String?, up: Boolean) {
            _links.value = _links.value.filter { it.npub != npub } + AwareLink(npub, addr, up)
        }

        private fun removeLink(npub: String) {
            _links.value = _links.value.filter { it.npub != npub }
        }

        /**
         * Whether this device has Wi-Fi Aware hardware at all — a static
         * capability the UI can read to gray out the toggle and show
         * "not supported on your device". Distinct from [isAvailable], which
         * is the *runtime* state (Aware hardware present but currently off
         * because Wi-Fi/Location is disabled or the radio is busy).
         */
        fun isSupported(context: Context): Boolean =
            context.packageManager.hasSystemFeature(PackageManager.FEATURE_WIFI_AWARE)

        /**
         * Whether Aware is usable *right now* — hardware present and the radio
         * available (which requires Wi-Fi to be on). Used to decide whether to
         * pop the Wi-Fi panel, and shown on the Dev screen.
         */
        fun isAvailable(context: Context): Boolean {
            if (!isSupported(context)) return false
            val mgr = context.getSystemService(Context.WIFI_AWARE_SERVICE) as? WifiAwareManager
            return mgr?.isAvailable == true
        }

        /** The Myco Wi-Fi Aware service name (the analog of the FIPS service UUID). */
        private const val SERVICE_NAME = "myco.fips.v1"

        /** The lane label pushed to [NativeCore.awarePeerFound]/[NativeCore.awarePeerLost],
         *  and asked of [NativeCore.nextUdpTransportFd] — distinguishes this radio from
         *  [app.myco.ap.ApRadio], which pushes "udp" through the same seams. Both ride
         *  UDP, but each lane is its own transport instance with its own socket, and
         *  this label is what selects between them. The core turns it into the
         *  qualified transport `"udp/aware"`, which is what makes fips dial this
         *  lane's socket rather than the LAN lane's. */
        private const val LANE = "aware"

        /** Message id for the npub-exchange `sendMessage`. */
        private const val MSG_ID_NPUB = 1

        /** NDP request timeout: if the data path isn't provisioned within this,
         *  onUnavailable fires and the request (and its slot) is released. */
        private const val NDP_TIMEOUT_MS = 20_000
    }
}

/** One Wi-Fi Aware NDP as the radio sees it: requested (no addr yet) or up
 *  (peer's scoped link-local + port). For the Dev screen. */
data class AwareLink(
    val npub: String,
    val addr: String?,
    val up: Boolean,
)

/** Process-global Wi-Fi Aware health flags read directly by the UI (no
 *  AppState round-trip) — the mirror of [app.myco.ble.BleHealth]. */
object AwareHealth {
    /** True when the platform refused an Aware call for lack of nearby-devices
     *  / fine-location permission (seen on GrapheneOS and secondary users even
     *  when our own permission check passed). The lane is stopped; the user
     *  must grant the permission and re-toggle Wi-Fi Aware. */
    @Volatile
    var permissionDenied: Boolean = false
}
