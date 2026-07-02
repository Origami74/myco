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
import java.net.Inet6Address
import java.util.concurrent.ConcurrentHashMap

/**
 * The Wi-Fi Aware (NAN) bulk-lane radio. Control-plane only: it discovers
 * peers, brings up a data path (NDP), and pushes "peer reachable / lost" into
 * the core ([NativeCore.awarePeerFound]/[NativeCore.awarePeerLost]). The bytes
 * ride the ordinary fips UDP transport over the `aware_dataN` interface — this
 * class never touches a payload byte. See docs/design/wifi-aware-interop.md.
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
        val mgr = manager ?: run { Log.w(TAG, "no Wi-Fi Aware service"); return }
        running = true
        registerAvailability(mgr)
        if (mgr.isAvailable) {
            attach(mgr)
        } else {
            Log.i(TAG, "Aware not available yet (is Wi-Fi on?); waiting for it")
        }
    }

    private fun attach(mgr: WifiAwareManager) {
        if (session != null) return
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
        runCatching { publishSession?.close() }
        runCatching { subscribeSession?.close() }
        runCatching { session?.close() }
        publishSession = null
        subscribeSession = null
        session = null
    }

    fun stop() {
        running = false
        availabilityReceiver?.let { runCatching { context.unregisterReceiver(it) } }
        availabilityReceiver = null
        closeSessions()
    }

    fun shutdown() {
        stop()
        thread.quitSafely()
    }

    private fun startPublish(s: WifiAwareSession) {
        // No service-specific info: the advert carries no identity, exactly
        // like the UUID-only BLE advert. Identity is exchanged post-match.
        val config = PublishConfig.Builder().setServiceName(SERVICE_NAME).build()
        s.publish(config, object : DiscoverySessionCallback() {
            override fun onPublishStarted(session: PublishDiscoverySession) {
                Log.i(TAG, "publish started")
                publishSession = session
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
        s.subscribe(config, object : DiscoverySessionCallback() {
            override fun onSubscribeStarted(session: SubscribeDiscoverySession) {
                Log.i(TAG, "subscribe started")
                subscribeSession = session
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
                NativeCore.awarePeerFound(peerNpub, addr)
            }

            override fun onLost(network: Network) {
                Log.i(TAG, "Aware NDP lost to ${short(peerNpub)}")
                NativeCore.awarePeerLost(peerNpub)
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

        /** Message id for the npub-exchange `sendMessage`. */
        private const val MSG_ID_NPUB = 1

        /** NDP request timeout: if the data path isn't provisioned within this,
         *  onUnavailable fires and the request (and its slot) is released. */
        private const val NDP_TIMEOUT_MS = 20_000
    }
}
