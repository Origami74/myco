package app.myco.vpn

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.ProxyInfo
import android.net.VpnService
import android.os.ParcelFileDescriptor
import android.util.Log
import app.myco.MainActivity
import app.myco.R
import app.myco.core.NativeCore
import java.io.FileInputStream
import java.io.FileOutputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The **app-owned TUN**: a [VpnService] that owns the TUN fd and pumps IPv6 packet
 * bytes between it and the FIPS node over the JNI bridge ([NativeCore.tunSendPacket]
 * / [NativeCore.tunNextPacket]). It routes **only `fd00::/8`** (the mesh ULA), so
 * normal internet traffic is untouched — this is not a full-tunnel VPN.
 *
 * The node installs the bridge channels when it starts (BLE on); this service just
 * moves bytes. With it up, a native socket to `[fd00::peer]:4870/:24243` routes
 * over the mesh, so the sync engine can pull a shared nsite from a peer's device.
 *
 * It also advertises the in-mesh sentinel resolver `fd00::53` as the VPN's DNS
 * server and leaves every app on the tunnel (no [Builder.addAllowedApplication]
 * restriction), so **any app** — not just Myco — can resolve `<npub>.fips` names
 * and reach mesh addresses directly. The native TUN pump answers those DNS
 * queries by pure computation (see `myco-core`'s `dns_intercept`); everything
 * else stays off this route, so normal internet is untouched.
 *
 * ### Exit mode
 * Given an [EXTRA_EXIT_PROXY] address, the service also advertises an HTTP proxy
 * to every app on the tunnel, so their web traffic egresses through a proxy
 * running on a mesh **exit node** — letting a phone with no internet of its own
 * browse the public web over the mesh. Android's [ProxyInfo] is unreliable with
 * an IPv6 literal, so the proxy is advertised as `127.0.0.1:<port>` and a small
 * loopback relay ([ExitRelay]) carries those connections to the exit's mesh
 * address. Because the exit is named by npub, `<npub>.fips:8080` works and the
 * exit need not peer this device directly — FIPS forwards multi-hop.
 */
class MycoVpnService : VpnService() {
    private var tun: ParcelFileDescriptor? = null
    private val running = AtomicBoolean(false)
    @Volatile private var readerThread: Thread? = null
    @Volatile private var writerThread: Thread? = null
    @Volatile private var relay: ExitRelay? = null

    // Live config; a start intent carrying a different one re-establishes.
    @Volatile private var curUla: String = ""
    @Volatile private var curMtu: Int = 0
    @Volatile private var curExit: String = ""

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopTun()
            stopSelf()
            return START_NOT_STICKY
        }
        startTun(
            intent?.getStringExtra(EXTRA_ULA).orEmpty(),
            intent?.getIntExtra(EXTRA_MTU, 0) ?: 0,
            intent?.getStringExtra(EXTRA_EXIT_PROXY).orEmpty(),
        )
        return START_STICKY
    }

    private fun startTun(ula: String, mtuHint: Int, exitProxy: String) {
        if (ula.isEmpty()) {
            stopSelf()
            return
        }
        // Already up on this exact config — nothing to do. A changed one (the
        // user set or cleared the exit) tears down and re-establishes.
        if (running.get()) {
            if (ula == curUla && mtuHint == curMtu && exitProxy == curExit) return
            Log.i(TAG, "reconfiguring TUN (exit='$exitProxy')")
            teardown()
        }
        startForegroundCompat()

        // The TUN MTU must be >= the IPv6 minimum (1280); FIPS's effective MTU
        // (transport_mtu - 77) is usually below that, so the real fit is the MSS
        // clamp (effective - 60) applied in the native bridge. Use the FIPS hint
        // only when it's already >= 1280 (a larger-MTU transport).
        val mtu = if (mtuHint in 1280..1500) mtuHint else 1280

        // In exit mode, stand the loopback relay up first — we need its port to
        // advertise the proxy below.
        val exit = parseExit(exitProxy)
        val relayPort: Int = if (exit != null) {
            val r = try {
                ExitRelay(exit.first, exit.second).also { it.start() }
            } catch (t: Throwable) {
                Log.e(TAG, "exit relay failed to start", t)
                null
            }
            relay = r
            r?.localPort ?: -1
        } else {
            -1
        }

        val builder = Builder()
            .setSession("Myco mesh")
            .setMtu(mtu)
            .addAddress(ula, 128) // this node's IPv6 ULA
            // A dummy IPv4 address (no IPv4 route) keeps the IPv4 family
            // "configured" so Myco's own IPv4 (the online fallback) bypasses the
            // VPN instead of being blacked out by an IPv6-only tunnel.
            .addAddress("10.255.255.254", 32)
            .addRoute("fd00::", 8) // the mesh ULA range — the only traffic we carry
            // An IPv6 default route as well, purely so the OS reports this network
            // as IPv6-capable. Chromium (and others) gate AAAA queries on an IPv6
            // reachability probe, and a tunnel offering only a unique-local address
            // fails it — so `<npub>.fips` was never even *asked* for as AAAA, and
            // resolving it in a browser failed while `ping6` worked. Non-mesh
            // packets that arrive because of this route are dropped in the pump
            // (see myco-core's `tun_bridge`), never forwarded into the mesh.
            .addRoute("::", 0)
            .setConfigureIntent(configIntent())
        // Advertise the in-mesh sentinel resolver so every app on the VPN can
        // resolve `<npub>.fips` names system-wide. The native TUN pump answers
        // queries to this address (see dns_intercept); it never leaves the
        // device. No addAllowedApplication restriction — leaving every app on
        // the VPN is what lets e.g. a browser reach a resolved fd00:: address,
        // not just Myco's own sync engine. Only the fd00::/8 route above is
        // captured, so normal internet is unaffected for all apps.
        try {
            builder.addDnsServer(DNS_SENTINEL)
        } catch (e: Exception) {
            Log.w(TAG, "addDnsServer($DNS_SENTINEL) failed", e)
        }
        if (relayPort > 0) {
            // Point every app's web traffic at the loopback relay, which carries
            // it to the exit's proxy over the mesh. Proxied requests go to
            // 127.0.0.1 — loopback, reachable regardless of routes — so this
            // needs no default route of its own, and Myco's own transports (the
            // AP UDP lane, mDNS, BLE) stay on the real network.
            builder.setHttpProxy(ProxyInfo.buildDirectProxy("127.0.0.1", relayPort))
            Log.i(TAG, "exit mode: proxy 127.0.0.1:$relayPort -> mesh $exitProxy")
        }
        val pfd = try {
            builder.establish()
        } catch (t: Throwable) {
            Log.e(TAG, "establish failed", t)
            null
        }
        if (pfd == null) {
            Log.e(TAG, "establish() returned null — VPN not consented or Builder rejected (ula=$ula)")
            relay?.close()
            relay = null
            android.widget.Toast.makeText(
                this,
                "Couldn't start the mesh adapter — another VPN may be active. " +
                    "Turn off any always-on VPN, then try again.",
                android.widget.Toast.LENGTH_LONG,
            ).show()
            stopSelf()
            return
        }
        tun = pfd
        curUla = ula
        curMtu = mtuHint
        curExit = exitProxy
        running.set(true)
        readerThread = Thread({ readLoop(pfd) }, "myco-tun-read").apply { start() }
        writerThread = Thread({ writeLoop(pfd) }, "myco-tun-write").apply { start() }
        Log.i(
            TAG,
            "mesh TUN up at $ula (route fd00::/8, dns $DNS_SENTINEL" +
                "${if (relayPort > 0) ", exit on" else ""})",
        )
    }

    /** TUN fd → mesh: read IPv6 packets and hand them to FIPS. */
    private fun readLoop(pfd: ParcelFileDescriptor) {
        val input = FileInputStream(pfd.fileDescriptor)
        val buf = ByteArray(2048)
        while (running.get()) {
            val n = try {
                input.read(buf)
            } catch (_: Exception) {
                break
            }
            if (n < 0) break
            if (n > 0) NativeCore.tunSendPacket(buf, n)
        }
    }

    /** mesh → TUN fd: pull IPv6 packets from FIPS and write them. */
    private fun writeLoop(pfd: ParcelFileDescriptor) {
        val output = FileOutputStream(pfd.fileDescriptor)
        val buf = ByteArray(2048)
        while (running.get()) {
            val n = NativeCore.tunNextPacket(buf, 1000)
            if (n > 0) {
                try {
                    output.write(buf, 0, n)
                } catch (_: Exception) {
                    break
                }
            }
        }
    }

    /** Tear the tun + relay down without stopping the service (used on reconfig). */
    private fun teardown() {
        running.set(false)
        readerThread?.interrupt()
        writerThread?.interrupt()
        try {
            tun?.close()
        } catch (_: Exception) {
        }
        tun = null
        relay?.close()
        relay = null
        curUla = ""
        curMtu = 0
        curExit = ""
    }

    private fun stopTun() {
        val wasRunning = running.get()
        teardown()
        stopForeground(STOP_FOREGROUND_REMOVE)
        if (wasRunning) Log.i(TAG, "mesh TUN down")
    }

    override fun onDestroy() {
        stopTun()
        super.onDestroy()
    }

    private fun startForegroundCompat() {
        val mgr = getSystemService(NotificationManager::class.java)
        mgr.createNotificationChannel(
            NotificationChannel(CHANNEL, "Myco mesh", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Mesh network adapter"
            },
        )
        val notif: Notification = Notification.Builder(this, CHANNEL)
            .setContentTitle("Myco")
            .setContentText("Mesh adapter active")
            .setSmallIcon(R.mipmap.ic_launcher)
            .setOngoing(true)
            .build()
        startForeground(NOTIF_ID, notif)
    }

    private fun configIntent(): PendingIntent =
        PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE,
        )

    /**
     * A loopback TCP relay: accepts on `127.0.0.1:0` and forwards each connection
     * verbatim to the exit node's HTTP proxy at `[host]:port` over the mesh. A
     * dumb byte pipe — the browser speaks the proxy protocol (CONNECT/GET)
     * end-to-end with the exit's proxy; we only carry the bytes, so the exit does
     * the DNS and the egress and a phone with no internet still works.
     *
     * The upstream socket is deliberately NOT [protect]ed: it must ride this same
     * VPN (route `fd00::/8`) into FIPS and out over the mesh. Replies come back
     * on that socket, never into the listener, so there is no loop.
     */
    private inner class ExitRelay(private val host: String, private val port: Int) {
        private val server = ServerSocket().apply {
            reuseAddress = true
            bind(InetSocketAddress(InetAddress.getByName("127.0.0.1"), 0))
        }
        val localPort: Int get() = server.localPort
        private val alive = AtomicBoolean(true)
        private var acceptThread: Thread? = null

        fun start() {
            acceptThread = Thread({ acceptLoop() }, "myco-exit-accept").apply { start() }
        }

        private fun acceptLoop() {
            while (alive.get()) {
                val client = try {
                    server.accept()
                } catch (_: Exception) {
                    break
                }
                Thread({ handle(client) }, "myco-exit-conn").start()
            }
        }

        private fun handle(client: Socket) {
            val upstream = try {
                Socket().apply {
                    connect(InetSocketAddress(InetAddress.getByName(host), port), 10_000)
                }
            } catch (t: Throwable) {
                Log.w(TAG, "exit upstream connect [$host]:$port failed", t)
                try { client.close() } catch (_: Exception) {}
                return
            }
            client.tcpNoDelay = true
            upstream.tcpNoDelay = true
            Thread({ pipe(client, upstream) }, "myco-exit-up").start()
            Thread({ pipe(upstream, client) }, "myco-exit-down").start()
        }

        private fun pipe(from: Socket, to: Socket) {
            val buf = ByteArray(8192)
            try {
                val ins = from.getInputStream()
                val outs = to.getOutputStream()
                while (true) {
                    val n = ins.read(buf)
                    if (n < 0) break
                    outs.write(buf, 0, n)
                    outs.flush()
                }
            } catch (_: Exception) {
            } finally {
                try { to.shutdownOutput() } catch (_: Exception) {}
                try { from.shutdownInput() } catch (_: Exception) {}
                if (from.isClosed || to.isClosed) {
                    try { from.close() } catch (_: Exception) {}
                    try { to.close() } catch (_: Exception) {}
                }
            }
        }

        fun close() {
            alive.set(false)
            try { server.close() } catch (_: Exception) {}
            acceptThread?.interrupt()
        }
    }

    companion object {
        const val EXTRA_ULA = "app.myco.extra.ULA"
        const val EXTRA_MTU = "app.myco.extra.MTU"
        const val EXTRA_EXIT_PROXY = "app.myco.extra.EXIT_PROXY"
        // In-mesh sentinel DNS resolver (matches myco-core's dns_intercept). Inside
        // the routed fd00::/8 range but never a node's own address, so query
        // packets reach the app-owned-TUN pump instead of being delivered locally.
        private const val DNS_SENTINEL = "fd00::53"
        private const val ACTION_STOP = "app.myco.vpn.STOP"
        private const val CHANNEL = "myco_mesh"
        private const val NOTIF_ID = 42
        private const val TAG = "MycoVpn"

        /**
         * Parse an exit-proxy spec into (host, port). Accepts `<npub>.fips:8080`,
         * `[fd00::ab]:8080`, `fd00::ab 8080`, `host:8080`, or a bare host
         * (default port 8080), and tolerates a pasted `http(s)://…/` URL.
         * Returns null when [spec] is blank or unparseable.
         */
        fun parseExit(spec: String): Pair<String, Int>? {
            var s = spec.trim()
            s = s.removePrefix("https://").removePrefix("http://")
            // Drop a path but never a port — only cut at '/' (IPv6 literals use
            // brackets, so they carry no slashes).
            val slash = s.indexOf('/')
            if (slash >= 0) s = s.substring(0, slash)
            s = s.trim()
            if (s.isEmpty()) return null
            return try {
                when {
                    s.startsWith("[") -> {
                        val close = s.indexOf(']')
                        val host = s.substring(1, close)
                        val port = s.substring(close + 1).removePrefix(":").ifEmpty { "8080" }
                        host to port.toInt()
                    }
                    ' ' in s -> {
                        val (h, p) = s.split(Regex("\\s+"), limit = 2)
                        h to p.toInt()
                    }
                    s.count { it == ':' } == 1 -> {
                        val (h, p) = s.split(":", limit = 2)
                        h to p.toInt()
                    }
                    else -> s to 8080 // bare host, or a bracket-less IPv6 literal
                }
            } catch (_: Exception) {
                null
            }
        }

        fun start(context: Context, ula: String, mtu: Int, exitProxy: String = "") {
            context.startService(
                Intent(context, MycoVpnService::class.java)
                    .putExtra(EXTRA_ULA, ula)
                    .putExtra(EXTRA_MTU, mtu)
                    .putExtra(EXTRA_EXIT_PROXY, exitProxy),
            )
        }

        fun stop(context: Context) {
            context.startService(
                Intent(context, MycoVpnService::class.java).setAction(ACTION_STOP),
            )
        }
    }
}
