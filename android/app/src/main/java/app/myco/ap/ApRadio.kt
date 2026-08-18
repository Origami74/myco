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
import app.myco.core.UdpSocketPin
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
 *     ([NativeCore.awarePeerFound], lane `"udp"`) — the same seam the Wi-Fi
 *     Aware radio uses, labelled with this lane's own name rather than
 *     masquerading as Aware. The core turns that label into the qualified
 *     transport `"udp/lan"`, so the node dials the LAN lane's own UDP socket
 *     (bound `:4871`, pinned below to this Wi-Fi [Network]) and never the
 *     Aware one; Noise IK authenticates, and the pushed npub is only a
 *     routing hint.
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
 *
 * The `!FIPS` AP never passes internet validation (it's local-only), so on a
 * phone with active mobile data the OS can route the handshake's replies to
 * a competing validated default network instead of back over this Wi-Fi —
 * the send succeeds locally but nothing ever comes back. [UdpSocketPin] pins
 * this lane's own UDP transport socket to this specific [Network] so replies
 * aren't lost that way — and, because that mark is exclusive, so that pinning
 * it here cannot cost the Wi-Fi Aware lane its own reachability.
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

    /** npub → every dialable address the advert carried, in preference order,
     *  and which one we are currently trying. A fips node advertises one
     *  address per interface, and only the interface facing us is actually
     *  on-link — the others fail neighbour discovery — so the right one can
     *  only be found by trying them ([rotate]). */
    private val candidates = HashMap<String, List<String>>()
    private val candidateIdx = HashMap<String, Int>()

    /** npub → the address that last produced a session, and whether it currently
     *  has one. A peer that drops is almost always reachable at the same address
     *  it just used, so retrying that before cycling turns a reconnect into one
     *  dial instead of a walk through every candidate at [REPUSH_MS] apart. */
    private val lastGood = HashMap<String, String>()
    private val wasConnected = HashSet<String>()

    private val resolveQueue = ArrayDeque<NsdServiceInfo>()
    private var resolving = false
    private var browsing = false
    private var browseListener: NsdManager.DiscoveryListener? = null
    private var ssid: String? = null

    /** Pins this lane's UDP transport socket to the current Wi-Fi [Network] —
     *  see [UdpSocketPin] for why each lane needs a socket of its own. Asks
     *  for [LANE] by name, so it can only ever receive the LAN lane's socket,
     *  never Wi-Fi Aware's. */
    private val udpPin = UdpSocketPin(LANE, handler, TAG)

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
            udpPin.bindTo(network)
            publishWifi()
        }

        override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
            currentSsid(caps)?.let { ssid = it }
            publishWifi()
        }

        override fun onLost(network: Network) {
            udpPin.clearTarget(network)
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

        // Learn this lane's UDP transport socket as soon as the node opens it
        // (only once mesh is toggled on — see runtime.rs's start_node) and pin
        // it to the Wi-Fi network the browse is running over.
        udpPin.start()
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
        for (npub in pushed.keys) NativeCore.awarePeerLost(npub, LANE)
        pushed.clear()
        candidates.clear()
        candidateIdx.clear()
        npubByService.clear()
        publishNodes()
        publishWifi()
    }

    /** Periodic tick while the browse is live: advance an *unconnected* peer to
     *  its next candidate address.
     *
     *  A connected peer is deliberately left alone. Pushing a peer the core has
     *  already authenticated is not free — it starts an alternate-path handshake
     *  — so re-pushing on a timer meant a healthy session was continuously
     *  raced by a fresh one, which showed up as endless
     *  `Stale handshake connection timed out` churn and link ids in the
     *  hundreds. A dropped session still recovers: it turns the peer
     *  unconnected, and this tick resumes rotating. */
    private val repush = object : Runnable {
        override fun run() {
            if (browseListener == null) return
            for (npub in candidates.keys.toList()) {
                if (connected(npub)) {
                    pushed[npub]?.let { lastGood[npub] = it }
                    wasConnected.add(npub)
                    continue
                }
                // First tick after a drop: go back to the address that worked
                // rather than resuming the cycle wherever it left off.
                if (wasConnected.remove(npub) && retryLastGood(npub)) continue
                rotate(npub)
            }
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
        val addrs = pickAddrs(info)
        if (addrs.isEmpty()) {
            Log.w(TAG, "no dialable address for ${short(npub)} (${info.serviceName})")
            return
        }
        npubByService[info.serviceName] = npub
        candidates[npub] = addrs
        candidateIdx.putIfAbsent(npub, 0)
        push(npub)
        publishNodes()
    }

    /** Push this peer's current candidate address to the core. */
    private fun push(npub: String) {
        val addrs = candidates[npub] ?: return
        val addr = addrs[(candidateIdx[npub] ?: 0) % addrs.size]
        if (pushed[npub] == addr) return
        Log.i(TAG, "fips node ${short(npub)} at $addr — pushing to core")
        NativeCore.awarePeerFound(npub, addr, LANE)
        pushed[npub] = addr
    }

    /** True once the core reports an authenticated session with `npub`. */
    private fun connected(npub: String): Boolean = runCatching {
        MycoCore.client(context).state().blePeers.any { it.npub == npub && it.connected }
    }.getOrDefault(false)

    /** Advance an unconnected peer to its next candidate address. Only the
     *  interface facing us answers neighbour discovery, and the advert doesn't
     *  say which that is, so this cycles until one handshakes. */
    /** Re-push the address that last held a session, if we still have it as a
     *  candidate. Returns false when there is nothing to fall back to. */
    private fun retryLastGood(npub: String): Boolean {
        val good = lastGood[npub] ?: return false
        val i = candidates[npub]?.indexOf(good) ?: -1
        if (i < 0) return false
        candidateIdx[npub] = i
        // `push` skips a no-op re-push, so clear the record to force this one:
        // the core dropped the peer and needs the address again.
        pushed.remove(npub)
        push(npub)
        return true
    }

    private fun rotate(npub: String) {
        val addrs = candidates[npub] ?: return
        if (addrs.size < 2) return
        candidateIdx[npub] = ((candidateIdx[npub] ?: 0) + 1) % addrs.size
        push(npub)
    }

    private fun serviceLost(serviceName: String) {
        val npub = npubByService.remove(serviceName) ?: return
        candidates.remove(npub)
        candidateIdx.remove(npub)
        val wasPushed = pushed.remove(npub) != null
        // An mDNS advert lapsing does not mean the peer is gone — NSD drops and
        // re-adds a service routinely, several times an hour here. Telling the
        // core it was lost closes the pooled connection, so acting on this while
        // a session is live tore down a perfectly healthy peer every few minutes.
        // A session that has genuinely died is detected by its own keepalive.
        if (wasPushed && !connected(npub)) {
            Log.i(TAG, "fips node ${short(npub)} gone from LAN")
            NativeCore.awarePeerLost(npub, LANE)
        } else if (wasPushed) {
            Log.i(TAG, "fips node ${short(npub)} advert lapsed, session still up — keeping it")
        }
        publishNodes()
    }

    /**
     * Every dialable address the advert carries, in the preference order given
     * in the class doc. Formats match what the core's address parser accepts:
     * numeric-scope link-local (`[fe80::x%3]:4871`), plain `[v6]:port`,
     * v4-mapped v6.
     */
    private fun pickAddrs(info: NsdServiceInfo): List<String> {
        val hosts: List<InetAddress> =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                info.hostAddresses
            } else {
                @Suppress("DEPRECATION")
                listOfNotNull(info.host)
            }
        val port = info.port.takeIf { it > 0 } ?: return emptyList()
        val v6 = hosts.filterIsInstance<Inet6Address>()
        return buildList {
            // IPv4 first, as a v4-mapped address. On a LAN this is the node's
            // address *on the network we joined* — the one interface certain to
            // face us. The link-locals below are one per interface the node has,
            // and nothing in the advert says which is which, so trying them
            // first means dialling addresses that are not on our link at all:
            // measured here, three dead link-locals ahead of an IPv4 address
            // that handshook in half a second. Safe to prefer because the mesh
            // tunnel routes no IPv4, so this cannot be swallowed by it, and the
            // core's UDP socket is a dual-stack `[::]` bind.
            hosts.filterIsInstance<Inet4Address>()
                .forEach { add("[::ffff:${it.hostAddress}]:$port") }
            // Then link-local — the only option on a link with no IPv4 (a Wi-Fi
            // Aware data path), and still correct here, just slower to find.
            v6.filter { it.isLinkLocalAddress && it.scopeId != 0 }
                .forEach { add("[${bare(it)}%${it.scopeId}]:$port") }
            // fd00::/8 is routed into the mesh TUN — dialing it would blackhole.
            v6.filter { !it.isLinkLocalAddress && it.address[0] != 0xfd.toByte() }
                .forEach { add("[${bare(it)}]:$port") }
        }
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

        /** The lane label pushed to [NativeCore.awarePeerFound]/[NativeCore.awarePeerLost],
         *  and asked of [NativeCore.nextUdpTransportFd] — distinguishes this radio from
         *  [app.myco.aware.AwareRadio], which shares the same JNI seams and pushes
         *  "aware". Both ride UDP, but each lane is its own transport instance with
         *  its own socket, and this label is what selects between them. */
        private const val LANE = "udp"

        /** Candidate-rotation interval (see [rotate]).
         *
         *  Must stay **longer than the core's handshake timeout** (30s), so only
         *  one dial is ever in flight. At 10s we pushed a new address every time
         *  while the previous two were still retrying, and the node happily ran
         *  all three: whichever completed last promoted and *evicted the session
         *  that had already succeeded*. The peer then re-handshook, and the cycle
         *  repeated every couple of minutes — a delayed session-replacement
         *  planted by every rotation.
         *
         *  The cost is reaching a peer whose first candidate is wrong more
         *  slowly. That is the right trade: the addresses only need cycling when
         *  nothing is connected, and an established session is worth far more
         *  than shaving seconds off finding one. */
        private const val REPUSH_MS = 35_000L

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
