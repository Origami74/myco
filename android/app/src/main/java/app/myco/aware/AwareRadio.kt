package app.myco.aware

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.wifi.aware.AttachCallback
import android.net.wifi.aware.DiscoverySession
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
import android.os.SystemClock
import android.util.Log
import app.myco.core.NativeCore
import app.myco.core.UdpSocketPin
import app.myco.ble.BleRadio
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
 * the NDP's [Network] ([pins]). Both halves matter. An NDP is a network of
 * its own with its own routing table, so a socket marked with any other
 * network — infrastructure Wi-Fi, as the AP lane marks it — cannot reach an
 * Aware peer at all: the address is well-formed, the send reports success, and
 * nothing arrives. That was the whole of the "Aware discovers everything and
 * peers with nothing" fault. See [UdpSocketPin].
 *
 * # One socket per peer, not one per lane
 *
 * That exclusivity does not stop at the lane boundary: each NDP is a separate
 * [Network] too, so one socket cannot serve two peers either — the most recent
 * bind wins and the rest go dark with their data paths still up. The core
 * therefore binds a pool of UDP transport instances (`aware0`…) and this class
 * holds a pin per instance, giving each peer a [slotPool] slot and pinning that
 * slot's socket to that peer's data path. The chipsets always had the headroom
 * (a Pixel 7 Pro advertises 8 concurrent data paths, a Galaxy A52s 2); the
 * single socket was the ceiling. See `reference/aware-multipeer-limit.md`.
 *
 * Flow, per peer:
 *  1. publish + subscribe the Myco service (symmetric, no group owner).
 *  2. on a subscribe match, exchange identities over Aware `sendMessage`
 *     (the analog of BLE's in-band pubkey exchange — no identity is in the
 *     advert itself). The payload is `"<npub>|<port>"`, where the port is the
 *     one *this* peer's slot listens on.
 *  3. the smaller-npub side requests the NDP (the cross-probe tiebreaker,
 *     applied before spending a scarce data-path slot; the core backstops it).
 *  4. read the peer's scoped link-local IPv6 from [WifiAwareNetworkInfo] and
 *     push `awarePeerFound(npub, "[fe80::x%ifindex]:port", "aware<slot>")`.
 *
 * The listener port is **per peer**, not a global constant — and now doubly so.
 * Each side advertises, in the identity exchange, the port of the socket it has
 * pinned to *that* peer's data path (slot *i* listens on
 * [app.myco.core.AppState.wifiAwarePort]` + i`, `runtime.rs`'s
 * `AWARE_UDP_BASE_PORT`), and we dial each peer at the port *it* named.
 * Dialling our own port instead is what broke interop the moment this lane
 * moved off the LAN port: a peer on an older build still listens on
 * [LEGACY_UDP_PORT], so the dial lands on a dead port, the handshake never
 * completes, and Android tears the idle NDP down ~35s later — forever. An
 * identity payload with no port is exactly that peer, and is dialled at
 * [LEGACY_UDP_PORT]; there is no flag day.
 *
 * A peer discovered before its identity is known is told the base port, which
 * is slot 0 — so two phones need no correction at all, and a third only learns
 * its port a message later ([updatePeerPort] re-announces if its data path beat
 * the correction).
 *
 * The NDP is left **open** (no PSK) — fips authenticates with Noise IK.
 */
class AwareRadio(
    private val context: Context,
    /** This device's npub, sent in the pubkey exchange and used for the tiebreaker. */
    private val ownNpub: String,
    /** The **base** port of this device's Aware socket pool: slot *i* is bound
     *  at `port + i`. Each peer is told its own slot's port in the identity
     *  exchange, and is itself dialled at *its* advertised port. */
    private val port: Int,
    /** How many peers the lane can carry at once — the number of UDP transport
     *  instances the core bound, read from the state rather than assumed, so
     *  this class can never name an instance the node does not have. */
    private val slots: Int,
) {
    private val manager: WifiAwareManager? =
        context.getSystemService(Context.WIFI_AWARE_SERVICE) as? WifiAwareManager
    private val connectivity =
        context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager

    private val thread = HandlerThread("myco-aware").apply { start() }
    private val handler = Handler(thread.looper)

    /** One pin per pooled UDP transport instance: pin *i* holds the socket the
     *  core bound as `aware<i>` and marks it for the data path of whichever
     *  peer holds slot *i*. Each asks for its own instance by name, so it can
     *  only ever receive that socket — never the AP lane's, and never another
     *  slot's. See the class doc and [UdpSocketPin]. */
    private val pins: List<UdpSocketPin> = List(slots) { UdpSocketPin(laneFor(it), handler, TAG) }

    /** Which pooled socket carries which peer. */
    private val slotPool = AwareSlotPool(slots)

    private var session: WifiAwareSession? = null
    private var publishSession: PublishDiscoverySession? = null
    private var subscribeSession: SubscribeDiscoverySession? = null

    /** Peers we have exchanged identities with, keyed by the (session-scoped) handle. */
    private val peerIdentities = ConcurrentHashMap<PeerHandle, AwarePeer>()

    /** Live NDP requests, keyed by peer npub, so we can tear them down on stop.
     *  Also the "one outstanding request per peer" lock: entries go in with
     *  [ConcurrentHashMap.putIfAbsent], so a rediscovery and a retry landing at
     *  the same moment (on different threads) cannot both request. */
    private val ndpCallbacks = ConcurrentHashMap<String, ConnectivityManager.NetworkCallback>()

    /** What a peer's NDP would be re-requested against: the discovery session
     *  and handle it was last seen on, plus the port it advertised. Kept after
     *  the NDP drops so recovery does not have to wait for rediscovery. */
    private val ndpTargets = ConcurrentHashMap<String, NdpTarget>()

    /** Peers whose NDP is established right now — the coexistence signal the
     *  BLE radio reads, and the honest count of what the lane is carrying.
     *  [logResources] is not a substitute: it reported seven of eight "data
     *  paths" free throughout a run of instant refusals. */
    private val liveNdps: MutableSet<String> = ConcurrentHashMap.newKeySet()

    /** Each peer's scoped link-local IPv6, kept from the moment its data path
     *  came up so the address can be re-formatted if the peer later announces a
     *  different port — see [updatePeerPort]. */
    private val peerAddrs = ConcurrentHashMap<String, Inet6Address>()

    /** The port we last told each peer to dial us on, and the port each peer
     *  last named for itself. Together they decide whether an identity is worth
     *  answering — see [announceSlot]. Cleared with the peer's slot, so a
     *  re-established data path re-announces. */
    private val announcedPorts = ConcurrentHashMap<String, Int>()
    private val heardPorts = ConcurrentHashMap<String, Int>()

    /** Per-peer NDP retry state. Mutated only on [handler]; the map is
     *  concurrent because [ConnectivityManager.NetworkCallback]s (which arrive
     *  on the framework's own thread) hop onto [handler] to touch it. */
    private val retries = ConcurrentHashMap<String, NdpRetry>()

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
        pins.forEach { it.start() }
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
        for ((_, st) in retries) st.pending?.let { handler.removeCallbacks(it) }
        retries.clear()
        liveNdps.clear()
        publishCoexState()
        ndpTargets.clear()
        peerIdentities.clear()
        peerAddrs.clear()
        announcedPorts.clear()
        heardPorts.clear()
        slotPool.clear()
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
        // On the handler thread, where the pins' state lives. Releases our dups
        // of the sockets; the core's own descriptors, and their bindings, are
        // untouched — a stale mark on a network that has gone away is harmless,
        // and the next NDP re-pins.
        handler.post { pins.forEach { it.stop() } }
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

            // A subscriber reached us. Reply with our npub and the port of the
            // slot we just gave it, so it can label the NDP and dial the socket
            // that will actually be marked for its data path. Then, if WE are
            // the responder for this pair (larger npub), request the data path
            // on the publish session. Exactly one side is responder and one is
            // initiator — an NDP needs both, complementary.
            override fun onMessageReceived(peer: PeerHandle, message: ByteArray) {
                val remote = parsePeer(message) ?: return
                peerIdentities[peer] = remote
                onPeerIdentity(publishSession, peer, remote, initiate = ownNpub > remote.npub)
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
            //
            // We cannot name a slot yet — a slot is per npub and the match
            // carries no identity — so this advertises the base port, which is
            // slot 0. That is exactly right for the first peer, and corrected
            // one message later for any other (see [onPeerIdentity]).
            override fun onServiceDiscovered(
                peer: PeerHandle,
                serviceSpecificInfo: ByteArray?,
                matchFilter: MutableList<ByteArray>?,
            ) {
                Log.i(TAG, "discovered a peer; sending our identity")
                subscribeSession?.sendMessage(peer, MSG_ID_NPUB, identityPayload(port))
            }

            // The publisher replied with its npub — now we know who it is, so
            // it gets a slot and, with it, the corrected port to dial us on. If
            // WE are the initiator for this pair (smaller npub), request the
            // NDP on the subscribe session; the peer's publish side requests as
            // responder.
            override fun onMessageReceived(peer: PeerHandle, message: ByteArray) {
                val remote = parsePeer(message) ?: return
                peerIdentities[peer] = remote
                onPeerIdentity(subscribeSession, peer, remote, initiate = ownNpub < remote.npub)
            }
        }, handler)
    }

    /**
     * A peer has told us who it is, on `session`. Give it a slot, tell it which
     * port that slot listens on, take note of the port *it* named, and request
     * its data path if we are the side that does the requesting.
     *
     * The reply is conditional — see [announceSlot] for why, and why that is
     * what stops two sides answering each other forever.
     *
     * A full pool is answered with silence rather than the base port: the port
     * we would name belongs to a socket marked for somebody else's data path,
     * so the peer's packets would arrive and our replies would leave down the
     * wrong network. Silence leaves it to retry when a slot frees.
     */
    private fun onPeerIdentity(
        session: DiscoverySession?,
        peer: PeerHandle,
        remote: AwarePeer,
        initiate: Boolean,
    ) {
        val slot = slotPool.acquire(remote.npub)
        if (slot == null) {
            Log.i(
                TAG,
                "no free Aware slot for ${short(remote.npub)} " +
                    "(${slotPool.inUse()}/$slots held); not answering",
            )
            return
        }
        announceSlot(remote.npub, slot, remote.port, session, peer)
        updatePeerPort(remote)
        if (initiate) {
            Log.i(TAG, "initiator for ${short(remote.npub)}; requesting NDP")
            requestDataPath(session, peer, remote)
        }
    }

    /**
     * Tell `npub` which port to dial us on — the one belonging to the socket we
     * will pin to its data path.
     *
     * **Sent only when something has actually changed**, and that is what makes
     * the exchange terminate. Both sides now answer an identity (the subscriber
     * used to stay silent), so an unconditional reply would have each answering
     * the other's answer forever. Two conditions, and each is load-bearing:
     *
     *  - *our* port changed — first contact, or a slot that moved because the
     *    data path was re-established somewhere else;
     *  - *their* port changed since we last heard it — which is how a peer that
     *    restarted its radio, and is introducing itself at the base port again,
     *    gets an answer instead of the silence its stale entry would otherwise
     *    earn it.
     *
     * A message that changes neither is an echo, and gets nothing back. The
     * gate lives here rather than in a marker in the payload because the
     * payload's shape has to stay parseable by builds from before the pool.
     *
     * `peerPort` is what the peer just advertised, or null when re-announcing
     * outside the identity exchange (a retry claiming a new slot).
     */
    private fun announceSlot(
        npub: String,
        slot: Int,
        peerPort: Int?,
        session: DiscoverySession?,
        peer: PeerHandle,
    ) {
        val ours = portForSlot(slot)
        val oursChanged = announcedPorts.put(npub, ours) != ours
        val theirsChanged = peerPort != null && heardPorts.put(npub, peerPort) != peerPort
        if (!oursChanged && !theirsChanged) return
        Log.i(TAG, "telling ${short(npub)} to dial us on $ours (slot $slot)")
        session?.sendMessage(peer, MSG_ID_NPUB, identityPayload(ours))
    }

    /**
     * The peer named a different port than the one we are dialling it at — it
     * has given us a slot of its own, and the message arrived after we had
     * already started (or finished) bringing its data path up.
     *
     * Re-announce rather than negotiate: `platform_peers` suppresses a re-push
     * by `(npub, address)`, so a changed port is not swallowed, and the core
     * refreshes the peer's path instead of standing up a second one. Costs
     * nothing when the ports already agree, which is every two-device pairing.
     */
    private fun updatePeerPort(remote: AwarePeer) {
        val target = ndpTargets[remote.npub] ?: return // not requested yet; the request will use it
        if (target.port == remote.port) return
        ndpTargets[remote.npub] = NdpTarget(target.session, target.peer, remote.port)
        val slot = slotPool.slotOf(remote.npub) ?: return
        val addr = peerAddrs[remote.npub]?.let { formatPeerAddr(it, remote.port) } ?: return
        Log.i(TAG, "${short(remote.npub)} moved to port ${remote.port}; re-announcing at $addr")
        setLink(remote.npub, addr, up = true)
        NativeCore.awarePeerFound(remote.npub, addr, laneFor(slot))
    }

    /** The port slot *i* listens on: the pool is contiguous from [port]. */
    private fun portForSlot(slot: Int): Int = port + slot

    /** This device's identity as it goes on the wire: `"<npub>|<port>"`, where
     *  the port is the one the peer being addressed should dial. The shape is
     *  unchanged by the pool, so a build from before it still parses this. */
    private fun identityPayload(dialPort: Int): ByteArray =
        "$ownNpub$FIELD_SEP$dialPort".toByteArray()

    /**
     * Parse an identity payload into the peer's npub and the UDP port to dial
     * it at. The wire format is `"<npub>|<port>"` in UTF-8 — one delimiter, and
     * unambiguous because an npub is fixed-shape bech32 (`npub1` + 58 chars
     * from an alphabet that excludes `|`).
     *
     * A bare npub with no delimiter is a peer on a build from before the Aware
     * lane got its own socket, which listens on the LAN lane's
     * [LEGACY_UDP_PORT] — parse it as such rather than rejecting it, so old and
     * new builds still peer.
     *
     * Anything else is ignored: a payload we cannot parse is never dialled at a
     * guessed port, because an NDP to a peer we cannot reach holds the
     * chipset's only data-path slot for ~35s and starves peers we can.
     */
    private fun parsePeer(message: ByteArray): AwarePeer? {
        val text = message.toString(Charsets.UTF_8)
        val sep = text.indexOf(FIELD_SEP)
        val npub = if (sep < 0) text else text.substring(0, sep)
        if (!NPUB_RE.matches(npub)) {
            Log.w(TAG, "ignoring malformed identity payload (${message.size} bytes)")
            return null
        }
        if (sep < 0) {
            Log.i(TAG, "${short(npub)} advertised no port (legacy build); dialling $LEGACY_UDP_PORT")
            return AwarePeer(npub, LEGACY_UDP_PORT)
        }
        val peerPort = text.substring(sep + 1).toIntOrNull()
        if (peerPort == null || peerPort !in 1..65535) {
            Log.w(TAG, "ignoring identity from ${short(npub)}: bad port field")
            return null
        }
        return AwarePeer(npub, peerPort)
    }

    /**
     * Request an open NDP toward `peer` on the given discovery `session`. Both
     * ends request (initiator on its subscribe session, responder on its
     * publish session) — an NDP forms only when both do. Both ends then get
     * [android.net.ConnectivityManager.NetworkCallback.onCapabilitiesChanged]
     * and push `awarePeerFound`; FIPS's cross-connection resolution dedups the
     * two resulting UDP links to one Noise session.
     */
    private fun requestDataPath(
        session: DiscoverySession?,
        peer: PeerHandle,
        remote: AwarePeer,
        isRetry: Boolean = false,
    ): Boolean {
        val sess = session ?: return false
        val peerNpub = remote.npub
        // Remember what to re-request against if this NDP later drops, and
        // drop any queued retry: this request supersedes it.
        ndpTargets[peerNpub] = NdpTarget(sess, peer, remote.port)
        if (isRetry) cancelPendingRetry(peerNpub) else clearRetry(peerNpub)
        // A socket to pin comes first. Without one, the data path could come up
        // and still carry nothing: the peer would have no port of ours to dial.
        // This is also where a retry gets its slot back — the previous one was
        // handed in when the data path dropped, so another peer could use it
        // while this one was down.
        val slot = slotPool.acquire(peerNpub)
        if (slot == null) {
            Log.i(
                TAG,
                "not requesting NDP to ${short(peerNpub)}: " +
                    "all $slots Aware slots in use (${logResources()})",
            )
            deferNdpRetry(peerNpub)
            return false
        }
        // The slot may differ from the one this peer was last told about, so
        // re-announce before the data path can come up under a stale port.
        announceSlot(peerNpub, slot, peerPort = null, session = sess, peer = peer)
        // Don't spend a request the framework has already said it cannot
        // provision. Queue instead — see [deferNdpRetry], which does not touch
        // the retry budget.
        dataPathBlockedBy()?.let { why ->
            Log.i(TAG, "not requesting NDP to ${short(peerNpub)}: $why (${logResources()})")
            deferNdpRetry(peerNpub)
            return false
        }
        // Open (unencrypted) NDP: no security setter. Noise IK is the trust
        // layer; a PSK here would be a redundant credential under it.
        val specifier = WifiAwareNetworkSpecifier.Builder(sess, peer).build()
        val request = NetworkRequest.Builder()
            .addTransportType(NetworkCapabilities.TRANSPORT_WIFI_AWARE)
            .setNetworkSpecifier(specifier)
            .build()

        // When the request went out, so [ConnectivityManager.NetworkCallback.onUnavailable]
        // can tell an instant refusal from a negotiation that ran its timeout.
        var requestedAt = SystemClock.elapsedRealtime()

        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
                val info = caps.transportInfo as? WifiAwareNetworkInfo ?: return
                val ipv6 = info.peerIpv6Addr ?: return
                // The port the peer last named, not the one it named when this
                // request went out — a slot-qualified identity can overtake the
                // data path it belongs to.
                val dialPort = ndpTargets[peerNpub]?.port ?: remote.port
                val addr = formatPeerAddr(ipv6, dialPort) ?: return
                val slot = slotPool.slotOf(peerNpub) ?: run {
                    Log.w(TAG, "NDP up to ${short(peerNpub)} with no slot held; not announcing")
                    return
                }
                Log.i(TAG, "Aware NDP up to ${short(peerNpub)} at $addr (slot $slot)")
                liveNdps.add(peerNpub)
                peerAddrs[peerNpub] = ipv6
                publishCoexState()
                // The link is good: hand the peer a fresh retry budget.
                if (retries.containsKey(peerNpub)) handler.post { clearRetry(peerNpub) }
                // Pin BEFORE announcing the peer: the core dials as soon as it
                // is told, and a dial from an unpinned (or wrong-network)
                // socket is what used to time out. This callback repeats for
                // the life of the NDP, so the re-pin is idempotent and cheap.
                //
                // One socket, one mark — so this pins *this peer's* socket, the
                // one whose port it was told to dial. Binding a shared socket
                // here is what used to make the most recent data path the only
                // reachable one.
                pins[slot].bindTo(network)
                setLink(peerNpub, addr, up = true)
                NativeCore.awarePeerFound(peerNpub, addr, laneFor(slot))
            }

            // An NDP is otherwise only requested on *rediscovery*, so a peer
            // whose data path drops sits dark until the discovery cycle comes
            // round again — minutes. Re-request it ourselves; see
            // [scheduleNdpRetry] for the backoff and the give-up rule.
            override fun onLost(network: Network) {
                Log.i(TAG, "Aware NDP lost to ${short(peerNpub)}")
                val slot = slotPool.slotOf(peerNpub)
                slot?.let { pins[it].clearTarget(network) }
                NativeCore.awarePeerLost(peerNpub, laneFor(slot ?: 0))
                releaseNdp(peerNpub)
                handler.post { scheduleNdpRetry(peerNpub, "NDP lost") }
            }

            // Fired when the request can't be provisioned within the timeout
            // below — typically because the chipset's data-path slots are all
            // in use. Releasing the request frees the slot (an un-timed-out
            // request would hold it indefinitely), and dropping the map entry
            // lets a later rediscovery retry.
            // Two very different failures arrive here, and telling them apart
            // is what keeps the retry budget honest. A refusal comes back in
            // milliseconds: the framework had no `aware_data` interface to give
            // (`WifiAwareDataPathStMgr: NdpInfos[] - no interfaces available!`)
            // and the peer had no say in it — so it is deferred, not counted.
            // A genuine failure to negotiate takes the full [NDP_TIMEOUT_MS]
            // and does say something about the peer, so it costs a retry.
            override fun onUnavailable() {
                val elapsed = SystemClock.elapsedRealtime() - requestedAt
                releaseNdp(peerNpub)
                if (elapsed < NDP_REFUSAL_MS) {
                    Log.w(
                        TAG,
                        "NDP to ${short(peerNpub)} refused outright after ${elapsed}ms — " +
                            "no data-path interface free (${logResources()})",
                    )
                    handler.post { deferNdpRetry(peerNpub, refundAttempt = true) }
                } else {
                    Log.w(TAG, "Aware NDP request to ${short(peerNpub)} gave up after ${elapsed}ms (${logResources()})")
                    handler.post { scheduleNdpRetry(peerNpub, "request unavailable") }
                }
            }
        }
        // The one-outstanding-request lock. onLost arrives on the framework's
        // thread and rediscovery on ours, so the check and the claim have to be
        // one operation or both paths can request the same peer at once —
        // two requests for one peer against a chipset with one data-path slot.
        if (ndpCallbacks.putIfAbsent(peerNpub, callback) != null) {
            Log.d(TAG, "NDP request to ${short(peerNpub)} already outstanding; not re-requesting")
            return false
        }
        Log.i(TAG, "requesting NDP to ${short(peerNpub)} (dial port ${remote.port}, ${logResources()})")
        setLink(peerNpub, addr = null, up = false)
        requestedAt = SystemClock.elapsedRealtime()
        // Timed request: on failure to provision within NDP_TIMEOUT_MS the
        // framework calls onUnavailable and releases it, so a stuck negotiation
        // never leaks a data-path slot (the root cause of "works fresh, dies
        // after a few restarts" — slots pile up and never free).
        connectivity.requestNetwork(request, callback, NDP_TIMEOUT_MS)
        return true
    }

    /**
     * Queue a re-request of `peerNpub`'s NDP after a loss or a failed request.
     *
     * The policy, and each clause earns its place:
     *  - **Backoff**, [NDP_RETRY_BASE_MS] doubling to [NDP_RETRY_MAX_MS]: an
     *    immediate unconditional retry against a peer that has walked out of
     *    the room thrashes the radio for as long as it is gone.
     *  - **Give up** after [MAX_NDP_RETRIES] attempts and fall back to waiting
     *    for rediscovery — which is the correct signal that the peer is back.
     *  - **One outstanding request per peer**: guarded here (a queued retry and
     *    a live request both bar a new one) and again, atomically, in
     *    [requestDataPath]. A success resets the budget.
     *  - **A free slot**: see [deferNdpRetry], which queues a try that could not
     *    have worked without charging it to the budget.
     *
     * Handler thread only.
     */
    private fun scheduleNdpRetry(peerNpub: String, why: String) {
        if (!running) return
        if (ndpTargets[peerNpub] == null) return
        if (ndpCallbacks.containsKey(peerNpub)) return
        val state = retries.getOrPut(peerNpub) { NdpRetry() }
        if (state.pending != null) return
        if (state.attempts >= MAX_NDP_RETRIES) {
            Log.w(
                TAG,
                "giving up on ${short(peerNpub)} after $MAX_NDP_RETRIES NDP retries ($why); " +
                    "waiting for rediscovery",
            )
            clearRetry(peerNpub)
            surrenderSlot(peerNpub)
            return
        }
        val delay = backoffMs(state.attempts)
        Log.i(
            TAG,
            "re-requesting NDP to ${short(peerNpub)} in ${delay}ms " +
                "($why, attempt ${state.attempts + 1}/$MAX_NDP_RETRIES)",
        )
        queueRetry(peerNpub, state, delay)
    }

    /** Fire a queued retry. The attempt is only spent if a request actually
     *  went out — [requestDataPath] defers instead when the data-path
     *  interface is held, which is nothing to do with this peer. Handler
     *  thread only. */
    private fun attemptNdpRetry(peerNpub: String) {
        val state = retries[peerNpub] ?: return
        state.pending = null
        if (!running) return
        val target = ndpTargets[peerNpub] ?: return
        if (ndpCallbacks.containsKey(peerNpub)) return
        val issued = requestDataPath(
            target.session,
            target.peer,
            AwarePeer(peerNpub, target.port),
            isRetry = true,
        )
        if (issued) state.attempts += 1
    }

    /**
     * Queue another try *without* spending a retry, because the last one could
     * not have succeeded: the chipset's single `aware_data` interface was
     * carrying another peer's NDP. Counting these as retries would let one
     * unreachable peer eat the budget of a reachable one. Bounded all the same
     * by [MAX_NDP_SLOT_DEFERRALS], so an interface held for good does not leave
     * us polling forever. Handler thread only.
     */
    private fun deferNdpRetry(peerNpub: String, refundAttempt: Boolean = false) {
        if (!running) return
        if (ndpTargets[peerNpub] == null) return
        if (ndpCallbacks.containsKey(peerNpub)) return
        val state = retries.getOrPut(peerNpub) { NdpRetry() }
        if (state.pending != null) return
        // A request that was refused outright still went through
        // [attemptNdpRetry], which counted it. It never reached the peer, so
        // give it back — otherwise a held interface silently drains the budget
        // meant for real attempts, and the counters stop meaning anything.
        if (refundAttempt) state.attempts = (state.attempts - 1).coerceAtLeast(0)
        state.deferrals += 1
        if (state.deferrals > MAX_NDP_SLOT_DEFERRALS) {
            Log.w(
                TAG,
                "still no room after $MAX_NDP_SLOT_DEFERRALS deferrals; " +
                    "leaving ${short(peerNpub)} to rediscovery",
            )
            clearRetry(peerNpub)
            surrenderSlot(peerNpub)
            return
        }
        val delay = backoffMs(state.deferrals - 1)
        Log.i(
            TAG,
            "deferring NDP to ${short(peerNpub)} by ${delay}ms " +
                "(deferral ${state.deferrals}/$MAX_NDP_SLOT_DEFERRALS, " +
                "retries still ${state.attempts}/$MAX_NDP_RETRIES)",
        )
        queueRetry(peerNpub, state, delay)
    }

    /**
     * Hand a slot back for a peer we have stopped trying to reach.
     *
     * A slot is claimed when a data path is *requested*, not when it comes up,
     * so a peer that walks out of the room mid-negotiation would otherwise hold
     * a socket until the lane restarts — and with four of them, two such peers
     * are half the room's capacity. The peer keeps its [ndpTargets] entry:
     * rediscovery still brings it back, and takes a slot again then.
     */
    private fun surrenderSlot(peerNpub: String) {
        slotPool.release(peerNpub)
        announcedPorts.remove(peerNpub)
        heardPorts.remove(peerNpub)
    }

    private fun queueRetry(peerNpub: String, state: NdpRetry, delay: Long) {
        val runnable = Runnable { attemptNdpRetry(peerNpub) }
        state.pending = runnable
        handler.postDelayed(runnable, delay)
    }

    /** [NDP_RETRY_BASE_MS] doubled per attempt, capped at [NDP_RETRY_MAX_MS]. */
    private fun backoffMs(attempts: Int): Long =
        (NDP_RETRY_BASE_MS shl attempts.coerceAtMost(8)).coerceAtMost(NDP_RETRY_MAX_MS)

    /** Cancel a queued retry but keep the peer's counters (the request that
     *  replaces it is part of the same escalating chain). */
    private fun cancelPendingRetry(peerNpub: String) {
        retries[peerNpub]?.let { st ->
            st.pending?.let { handler.removeCallbacks(it) }
            st.pending = null
        }
    }

    /** Forget a peer's retry state entirely: it either connected or is out of
     *  budget, and either way the next request starts from scratch. */
    private fun clearRetry(peerNpub: String) {
        retries.remove(peerNpub)?.pending?.let { handler.removeCallbacks(it) }
    }

    /**
     * Why a request would be refused out of hand by the *framework*, or null if
     * it has a chance.
     *
     * This used to refuse whenever any other peer held a data path, which was
     * right only because the lane had one socket to pin: a second NDP would
     * have unbound the first. With a socket per slot that reason is gone, and
     * what remains is our own pool (checked where the slot is acquired) and the
     * framework's count — kept as a backstop but never trusted alone, since
     * [android.net.wifi.aware.AwareResources] reported `dataPaths=7` free
     * throughout a run of instant refusals.
     */
    private fun dataPathBlockedBy(): String? {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val r = manager?.availableAwareResources
            if (r != null && r.availableDataPathsCount <= 0) return "no data paths free"
        }
        return null
    }

    /** Unregister and forget a peer's NDP request, freeing its data-path slot. */
    /** Publish the node_addr prefixes of peers Aware is carrying, so the BLE
     *  radio can refuse to dial them. The core otherwise re-establishes one
     *  peer alternately over both transports, and that churn tears the NAN
     *  data path down every ~60s.
     *
     *  node_addr is the only identity both radios can compute: BLE reads a
     *  prefix of it from the peer's scan response, and it is
     *  `SHA-256(x-only pubkey)[..16]`, which is derivable from the npub here.
     *  An npub that fails to decode is simply left out — the peer then keeps
     *  its BLE lane, which is the pre-existing behaviour. */
    private fun publishCoexState() {
        BleRadio.awareNodePrefixes = liveNdps.mapNotNull { nodeAddrPrefix(it) }.toSet()
    }

    /** `SHA-256(pubkey)` truncated to the same prefix width BLE advertises,
     *  hex-encoded. Mirrors `NodeAddr::from_pubkey` in fips. */
    private fun nodeAddrPrefix(npub: String): String? {
        val pubkey = decodeNpub(npub) ?: return null
        val hash = java.security.MessageDigest.getInstance("SHA-256").digest(pubkey)
        return hash.take(BleRadio.NODE_PREFIX_BYTES).joinToString("") { "%02x".format(it) }
    }

    /** bech32 npub -> 32-byte x-only pubkey. Checksum is not verified: these
     *  npubs come from our own core, not from the wire. */
    private fun decodeNpub(npub: String): ByteArray? {
        val sep = npub.lastIndexOf('1')
        if (sep < 0) return null
        val data = ArrayList<Int>(npub.length - sep)
        for (c in npub.substring(sep + 1)) {
            val v = BECH32_CHARSET.indexOf(c)
            if (v < 0) return null
            data.add(v)
        }
        if (data.size < 7) return null
        val payload = data.subList(0, data.size - 6) // strip checksum
        var acc = 0
        var bits = 0
        val out = ArrayList<Byte>(32)
        for (v in payload) {
            acc = (acc shl 5) or v
            bits += 5
            while (bits >= 8) {
                bits -= 8
                out.add(((acc shr bits) and 0xff).toByte())
            }
        }
        return if (out.size == 32) out.toByteArray() else null
    }

    private fun releaseNdp(peerNpub: String) {
        liveNdps.remove(peerNpub)
        publishCoexState()
        ndpCallbacks.remove(peerNpub)?.let {
            runCatching { connectivity.unregisterNetworkCallback(it) }
        }
        // Hand the socket back so a peer waiting on a full pool can have it.
        // The announced port goes with it: a re-established data path may land
        // on a different slot, and the peer has to be told the new one —
        // [requestDataPath] re-announces when the retry claims a slot again.
        surrenderSlot(peerNpub)
        peerAddrs.remove(peerNpub)
        removeLink(peerNpub)
    }

    /** Best-effort snapshot of free Aware data-path/session slots (API 31+),
     *  next to our own pool occupancy — the two disagree, and which one is
     *  refusing a request is the thing worth knowing. */
    private fun logResources(): String {
        val ours = "slots=${slotPool.inUse()}/$slots"
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return "$ours resources n/a"
        val r = manager?.availableAwareResources ?: return "$ours resources unknown"
        return "$ours dataPaths=${r.availableDataPathsCount} pub=${r.availablePublishSessionsCount} sub=${r.availableSubscribeSessionsCount}"
    }

    /**
     * Format the peer's link-local IPv6 as `"[fe80::x%ifindex]:peerPort"` — the
 * port the *peer* advertised, not ours, so a peer on a different build is
 * dialled where it actually listens — with a
     * **numeric** scope — the only form fips-core's address parser accepts
     * (interface-name scopes do not parse). The [Inet6Address] handed back by
     * [WifiAwareNetworkInfo] is already scoped to the local `aware_dataN`
     * interface, so its `scopeId` is the ifindex we need.
     */
    private fun formatPeerAddr(ipv6: Inet6Address?, peerPort: Int): String? {
        if (ipv6 == null) return null
        val scopeId = ipv6.scopeId
        if (scopeId == 0) {
            Log.w(TAG, "peer IPv6 has no scope id; cannot dial")
            return null
        }
        // hostAddress may render as "fe80::x%aware_data0" or "fe80::x%3";
        // strip any scope suffix and re-append the numeric ifindex.
        val bare = ipv6.hostAddress?.substringBefore('%') ?: return null
        return "[$bare%$scopeId]:$peerPort"
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

        /** Prefix of the lane label pushed to [NativeCore.awarePeerFound]/
         *  [NativeCore.awarePeerLost] and asked of [NativeCore.nextUdpTransportFd]
         *  — distinguishes this radio from [app.myco.ap.ApRadio], which pushes
         *  "udp" through the same seams. Both ride UDP, but each transport
         *  instance has its own socket and this label is what selects between
         *  them. */
        private const val LANE = "aware"

        /** The lane label for one pooled socket: `"aware2"`. The core maps it to
         *  the transport instance of the same name and qualifies the peer's
         *  address as `"udp/aware2"`, which is what makes fips dial the socket
         *  pinned to *that* peer's data path rather than another peer's — or
         *  the LAN lane's. */
        private fun laneFor(slot: Int): String = "$LANE$slot"

        /** Message id for the identity-exchange `sendMessage`. */
        private const val MSG_ID_NPUB = 1

        /** Separator between npub and port in the identity payload. Not in the
         *  bech32 alphabet, so it cannot occur inside an npub. */
        private const val FIELD_SEP = '|'

        /** Shape of a valid npub: bech32, `npub1` + 58 chars of the bech32
         *  alphabet (no `1`, `b`, `i`, `o`). Anything else is not dialled. */
        private const val BECH32_CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"

        private val NPUB_RE = Regex("^npub1[023456789acdefghjklmnpqrstuvwxyz]{58}$")

        /** Where a peer that advertises no port listens: builds from before the
         *  Aware lane got its own socket shared the LAN lane's port. */
        private const val LEGACY_UDP_PORT = 4871

        /** NDP request timeout: if the data path isn't provisioned within this,
         *  onUnavailable fires and the request (and its slot) is released. */
        private const val NDP_TIMEOUT_MS = 20_000

        /** First NDP retry delay after a loss; doubles per attempt. */
        private const val NDP_RETRY_BASE_MS = 2_000L

        /** Ceiling on the retry backoff, and the deferral poll interval. */
        private const val NDP_RETRY_MAX_MS = 30_000L

        /** Retries before falling back to waiting for rediscovery. With the
         *  backoff above that is 2+4+8+16+30s ≈ 1 minute of trying. */
        private const val MAX_NDP_RETRIES = 5

        /** An `onUnavailable` faster than this is the framework refusing the
         *  request outright (no `aware_data` interface), not a negotiation that
         *  ran out of time — the two are told apart by nothing else. */
        private const val NDP_REFUSAL_MS = 2_000L

        /** How many times a try may be deferred for want of a data-path
         *  interface before the peer is left to rediscovery. Deferrals use the
         *  same escalating backoff as retries, so this is ~2.5 minutes — long
         *  enough to outlast the ~40s Android takes to reclaim the interface
         *  behind a dropped NDP, short enough not to poll a dead room. */
        private const val MAX_NDP_SLOT_DEFERRALS = 8
    }
}

/** A peer's identity as advertised over Aware: who it is, and the UDP port it
 *  listens on. The port is per peer — see [AwareRadio.parsePeer]. */
private data class AwarePeer(val npub: String, val port: Int)

/** Everything needed to re-request a peer's NDP without waiting for it to be
 *  rediscovered: the discovery session and handle it was last seen on, and the
 *  port it advertised. */
private class NdpTarget(
    val session: DiscoverySession,
    val peer: PeerHandle,
    val port: Int,
)

/** A peer's NDP retry budget. Mutated only on the radio's handler thread;
 *  [pending] is volatile because teardown cancels it from another one. */
private class NdpRetry {
    /** Retries actually issued since the last successful NDP. */
    var attempts: Int = 0

    /** Retries skipped because the chipset's data-path interface was held.
     *  Counted separately: the peer did nothing wrong, so these must not eat
     *  the retry budget — but they are bounded too. */
    var deferrals: Int = 0

    @Volatile
    var pending: Runnable? = null
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
