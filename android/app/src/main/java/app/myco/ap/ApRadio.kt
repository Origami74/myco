package app.myco.ap

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import androidx.annotation.RequiresApi
import app.myco.core.MycoCore
import app.myco.core.NativeCore
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * The `!FIPS` access-point lane. When the phone associates with a FIPS AP's
 * open `!FIPS` SSID (or any LAN carrying a fips node), this connects the app's
 * node to the AP's fips node over the ordinary UDP transport:
 *
 *  1. watch infrastructure Wi-Fi via a [ConnectivityManager.NetworkCallback]
 *     (passive, permissionless);
 *  2. while any Wi-Fi is up, browse mDNS for `_fips._udp` — the advert fips
 *     LAN discovery publishes (`reference/fips` `src/discovery/lan`), whose
 *     TXT carries the node's `npub`;
 *  3. on resolve, push `(npub, addr)` into the core's platform peer queue
 *     ([NativeCore.awarePeerFound], transport `"udp"`) — the same seam the
 *     Wi-Fi Aware radio uses. The node dials the UDP transport bound `:4871`
 *     and Noise IK authenticates; the pushed npub is only a routing hint.
 *
 * The browse is deliberately **not** gated on the literal `!FIPS` SSID: on
 * API 33+ the SSID is redacted unless the app holds location permission, so a
 * name gate would kill the lane on modern devices — and browsing another LAN
 * is harmless (nothing advertises there). The SSID is still read best-effort
 * for the Dev panel.
 *
 * Address preference on resolve: link-local IPv6 first (never captured by the
 * mesh TUN, always on-link), then a global/non-ULA IPv6, then IPv4 as a
 * v4-mapped IPv6 (`[::ffff:a.b.c.d]` — the core's UDP socket is a dual-stack
 * `[::]` bind and its transport selection is family-aware, so a plain V4
 * address would find no compatible socket). `fd00::/8` addresses are skipped:
 * the VpnService routes that prefix into the mesh TUN, so dialing one would
 * blackhole the handshake.
 */
class ApRadio private constructor(private val context: Context) {
    private val connectivity =
        context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    private val nsd = context.getSystemService(Context.NSD_SERVICE) as NsdManager

    /** All mutable state below is confined to this thread. */
    private val thread = HandlerThread("myco-ap").apply { start() }
    private val handler = Handler(thread.looper)

    /** Live infrastructure Wi-Fi networks (usually 0 or 1). */
    private val wifiNets = HashSet<Network>()

    /** mDNS instance name → npub, learned at resolve time (a lost event
     *  carries only the instance name). */
    private val npubByService = HashMap<String, String>()

    /** npub → last pushed addr. */
    private val pushed = HashMap<String, String>()

    private val resolveQueue = ArrayDeque<NsdServiceInfo>()
    private var resolving = false
    private var browsing = false
    private var browseListener: NsdManager.DiscoveryListener? = null
    private var ssid: String? = null

    /** Own npub, to skip a self-advert (defensive — the app never publishes
     *  mDNS, only browses). Resolved lazily; the core is up by first resolve. */
    private val ownNpub: String by lazy {
        runCatching { MycoCore.client(context).state().ownNpub }.getOrDefault("")
    }

    private inner class WifiCallback : ConnectivityManager.NetworkCallback {
        constructor() : super()

        @RequiresApi(Build.VERSION_CODES.S)
        constructor(flags: Int) : super(flags)

        override fun onAvailable(network: Network) {
            if (wifiNets.add(network) && wifiNets.size == 1) startBrowse()
            publishWifi()
        }

        override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
            currentSsid(caps)?.let { ssid = it }
            publishWifi()
        }

        override fun onLost(network: Network) {
            if (wifiNets.remove(network) && wifiNets.isEmpty()) {
                ssid = null
                stopBrowse()
            }
            publishWifi()
        }
    }

    private fun start() {
        val request = NetworkRequest.Builder()
            .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
            .build()
        // FLAG_INCLUDE_LOCATION_INFO (31+) asks for an unredacted SSID in the
        // callback's WifiInfo; still redacted without location permission —
        // that's fine, the SSID is display-only.
        val callback = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            WifiCallback(ConnectivityManager.NetworkCallback.FLAG_INCLUDE_LOCATION_INFO)
        } else {
            WifiCallback()
        }
        connectivity.registerNetworkCallback(request, callback, handler)
        Log.i(TAG, "AP lane armed (browsing $SERVICE_TYPE while Wi-Fi is up)")
    }

    // --- mDNS browse ---

    private fun startBrowse() {
        if (browseListener != null) return
        // A DiscoveryListener is single-use: a fresh one per browse session.
        val listener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) {
                handler.post {
                    browsing = true
                    publishWifi()
                    handler.postDelayed(repush, REPUSH_MS)
                    Log.i(TAG, "mDNS browse started")
                }
            }

            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                handler.post {
                    Log.w(TAG, "mDNS browse failed to start: $errorCode")
                    browseListener = null
                    browsing = false
                    publishWifi()
                }
            }

            override fun onServiceFound(info: NsdServiceInfo) {
                handler.post {
                    resolveQueue.add(info)
                    pumpResolve()
                }
            }

            override fun onServiceLost(info: NsdServiceInfo) {
                handler.post { serviceLost(info.serviceName) }
            }

            override fun onDiscoveryStopped(serviceType: String) {
                handler.post { browsing = false; publishWifi() }
            }

            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {
                handler.post { browsing = false; publishWifi() }
            }
        }
        browseListener = listener
        nsd.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, listener)
    }

    private fun stopBrowse() {
        browseListener?.let { runCatching { nsd.stopServiceDiscovery(it) } }
        browseListener = null
        browsing = false
        handler.removeCallbacks(repush)
        resolveQueue.clear()
        // The LAN is gone with the link: tell the core so it closes the pooled
        // UDP sessions instead of re-using dead sockets.
        for (npub in pushed.keys) NativeCore.awarePeerLost(npub)
        pushed.clear()
        npubByService.clear()
        publishNodes()
        publishWifi()
    }

    /** Periodic re-push of every resolved node while the browse is live: NSD
     *  fires onServiceFound only once per appearance, so a dropped UDP session
     *  would otherwise never get a fresh dial hint. The core's alternate-path
     *  gates make a redundant push a no-op for a healthy peer. */
    private val repush = object : Runnable {
        override fun run() {
            if (browseListener == null) return
            for ((npub, addr) in pushed) NativeCore.awarePeerFound(npub, addr)
            handler.postDelayed(this, REPUSH_MS)
        }
    }

    // --- resolve (NsdManager allows one in-flight resolve at a time) ---

    private fun pumpResolve() {
        if (resolving) return
        val next = resolveQueue.removeFirstOrNull() ?: return
        resolving = true
        @Suppress("DEPRECATION") // registerServiceInfoCallback is 34+; minSdk 29
        nsd.resolveService(next, object : NsdManager.ResolveListener {
            override fun onServiceResolved(info: NsdServiceInfo) {
                handler.post {
                    resolving = false
                    resolved(info)
                    pumpResolve()
                }
            }

            override fun onResolveFailed(info: NsdServiceInfo, errorCode: Int) {
                handler.post {
                    resolving = false
                    if (errorCode == NsdManager.FAILURE_ALREADY_ACTIVE) {
                        // Another app's resolve is in flight; retry shortly.
                        resolveQueue.add(info)
                        handler.postDelayed({ pumpResolve() }, 1_000)
                    } else {
                        Log.w(TAG, "resolve failed for ${info.serviceName}: $errorCode")
                        pumpResolve()
                    }
                }
            }
        })
    }

    private fun resolved(info: NsdServiceInfo) {
        val npub = info.attributes[TXT_NPUB]?.toString(Charsets.UTF_8)
            ?.takeIf { it.startsWith("npub1") }
            ?: run { Log.w(TAG, "advert ${info.serviceName} has no npub TXT"); return }
        if (npub == ownNpub) return
        val addr = pickAddr(info) ?: run {
            Log.w(TAG, "no dialable address for ${short(npub)} (${info.serviceName})")
            return
        }
        npubByService[info.serviceName] = npub
        if (pushed[npub] != addr) {
            Log.i(TAG, "fips node ${short(npub)} at $addr — pushing to core")
            NativeCore.awarePeerFound(npub, addr)
            pushed[npub] = addr
        }
        publishNodes()
    }

    private fun serviceLost(serviceName: String) {
        val npub = npubByService.remove(serviceName) ?: return
        if (pushed.remove(npub) != null) {
            Log.i(TAG, "fips node ${short(npub)} gone from LAN")
            NativeCore.awarePeerLost(npub)
        }
        publishNodes()
    }

    /**
     * Pick the address to dial, per the preference order in the class doc.
     * Formats match what the core's address parser accepts: numeric-scope
     * link-local (`[fe80::x%3]:4871`), plain `[v6]:port`, v4-mapped v6.
     */
    private fun pickAddr(info: NsdServiceInfo): String? {
        val hosts: List<InetAddress> =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                info.hostAddresses
            } else {
                @Suppress("DEPRECATION")
                listOfNotNull(info.host)
            }
        val port = info.port.takeIf { it > 0 } ?: return null
        val v6 = hosts.filterIsInstance<Inet6Address>()
        v6.firstOrNull { it.isLinkLocalAddress && it.scopeId != 0 }
            ?.let { return "[${bare(it)}%${it.scopeId}]:$port" }
        // fd00::/8 is routed into the mesh TUN — dialing it would blackhole.
        v6.firstOrNull { !it.isLinkLocalAddress && it.address[0] != 0xfd.toByte() }
            ?.let { return "[${bare(it)}]:$port" }
        hosts.filterIsInstance<Inet4Address>().firstOrNull()
            ?.let { return "[::ffff:${it.hostAddress}]:$port" }
        return null
    }

    private fun bare(a: InetAddress): String = a.hostAddress?.substringBefore('%') ?: ""

    // --- state for the Dev panel ---

    private fun publishWifi() {
        _wifi.value = WifiApView(
            connected = wifiNets.isNotEmpty(),
            ssid = currentSsid() ?: ssid,
            browsing = browsing,
        )
    }

    private fun publishNodes() {
        _nodes.value = npubByService.values.distinct().map { npub ->
            LanFipsNode(npub = npub, addr = pushed[npub] ?: "", pushed = pushed.containsKey(npub))
        }
    }

    /** Best-effort SSID: from the callback's [WifiInfo] on 31+ (unredacted only
     *  with location permission), from the deprecated [WifiManager] path below. */
    private fun currentSsid(caps: NetworkCapabilities? = null): String? {
        val raw = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            (caps?.transportInfo as? WifiInfo)?.ssid
        } else {
            @Suppress("DEPRECATION")
            runCatching {
                (context.getSystemService(Context.WIFI_SERVICE) as? WifiManager)
                    ?.connectionInfo?.ssid
            }.getOrNull()
        }
        return raw?.removeSurrounding("\"")
            ?.takeIf { it.isNotEmpty() && it != WifiManager.UNKNOWN_SSID }
    }

    private fun short(npub: String): String =
        if (npub.length > 12) npub.substring(0, 12) + "…" else npub

    companion object {
        private const val TAG = "MycoApRadio"

        /** fips LAN discovery's DNS-SD service type (Android form, no `.local.`). */
        private const val SERVICE_TYPE = "_fips._udp"

        /** TXT key carrying the advertising node's npub. */
        private const val TXT_NPUB = "npub"

        private const val REPUSH_MS = 60_000L

        private val _wifi = MutableStateFlow(WifiApView())
        private val _nodes = MutableStateFlow<List<LanFipsNode>>(emptyList())

        /** Wi-Fi + browse status, for the Dev screen. */
        val wifi: StateFlow<WifiApView> = _wifi.asStateFlow()

        /** fips nodes discovered on the current LAN, for the Dev screen. */
        val nodes: StateFlow<List<LanFipsNode>> = _nodes.asStateFlow()

        @Volatile
        private var instance: ApRadio? = null

        /** Start the process-wide watcher (idempotent; survives Activity
         *  recreation — it holds only the application context). */
        fun ensureStarted(context: Context) {
            if (instance != null) return
            synchronized(this) {
                if (instance == null) {
                    instance = ApRadio(context.applicationContext).also { it.start() }
                }
            }
        }
    }
}

/** Wi-Fi + mDNS-browse status for the Dev panel. `ssid` is best-effort (null
 *  when redacted — API 33+ without location permission). */
data class WifiApView(
    val connected: Boolean = false,
    val ssid: String? = null,
    val browsing: Boolean = false,
)

/** One fips node seen on the current LAN. `pushed` = handed to the core's
 *  platform peer queue (the node dials it over UDP). */
data class LanFipsNode(
    val npub: String,
    val addr: String,
    val pushed: Boolean,
)
