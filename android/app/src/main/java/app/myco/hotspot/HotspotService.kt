package app.myco.hotspot

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import androidx.core.app.NotificationCompat
import app.myco.MainActivity
import app.myco.R
import app.myco.aware.AwareRadio
import app.myco.aware.AwareService
import app.myco.nfc.PairPresent
import java.net.Inet4Address
import java.net.NetworkInterface
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * Foreground service owning the **file-share hotspot**: a
 * [WifiManager.startLocalOnlyHotspot] reservation (the OS picks the SSID and
 * WPA2 passphrase) plus the [FileShareServer] a joined guest browses to.
 *
 * A local-only hotspot dies with its reservation, so the reservation must live
 * somewhere that survives the Circle sheet being dismissed and the app being
 * backgrounded — that is this service. `connectedDevice` is the right FGS type:
 * the whole point is a live link to a nearby device, and the app holds
 * CHANGE_WIFI_STATE, which qualifies it for that type.
 *
 * State is published like the radios do it ([app.myco.ap.ApRadio]): a
 * process-wide [StateFlow] the Circle sheet collects.
 */
class HotspotService : Service() {

    private var reservation: WifiManager.LocalOnlyHotspotReservation? = null
    private var server: FileShareServer? = null
    private var starting = false
    private var pausedAware = false
    private var lohsRetries = 0
    private val handler = Handler(Looper.getMainLooper())
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var approvalsJob: Job? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                shutdown()
                stopSelf()
                return START_NOT_STICKY
            }
            else -> begin()
        }
        // NOT_STICKY: a hotspot the system killed is gone (the reservation died
        // with the process) — restarting the service alone would show a lying
        // "on" notification, and re-opening a hotspot unasked is not our call.
        return START_NOT_STICKY
    }

    private fun begin() {
        if (reservation != null || starting) return
        startForegroundCompat()
        starting = true
        publish(HotspotView(phase = HotspotPhase.STARTING))
        // The Wi-Fi chip cannot host an AP interface next to the Aware (NAN)
        // session the mesh's Aware lane holds — seen on Pixel 9 as HalDevMgr
        // "bestIfaceCreationProposal is null" followed by ERROR_INCOMPATIBLE_MODE.
        // Pause Aware for the hotspot's lifetime (shutdown restores it) and give
        // the HAL a beat to actually release the interface before asking for AP.
        val prefs = getSharedPreferences("myco_prefs", MODE_PRIVATE)
        val awareOn = prefs.getBoolean(MainActivity.PREF_AWARE, true) && AwareRadio.isSupported(this)
        if (awareOn) {
            Log.i(TAG, "pausing Wi-Fi Aware while the hotspot runs")
            AwareService.stop(this)
            pausedAware = true
        }
        lohsRetries = LOHS_RETRIES
        handler.postDelayed({ requestLohs() }, if (awareOn) AWARE_RELEASE_MS else 0)
    }

    private fun requestLohs() {
        if (!starting) return // stopped while waiting
        val wifi = getSystemService(Context.WIFI_SERVICE) as WifiManager
        try {
            wifi.startLocalOnlyHotspot(object : WifiManager.LocalOnlyHotspotCallback() {
                override fun onStarted(res: WifiManager.LocalOnlyHotspotReservation) {
                    starting = false
                    reservation = res
                    onHotspotUp(res)
                }

                override fun onFailed(reason: Int) {
                    // INCOMPATIBLE_MODE right after releasing Aware usually means
                    // the NAN interface is still tearing down — retry, don't die.
                    if (reason == WifiManager.LocalOnlyHotspotCallback.ERROR_INCOMPATIBLE_MODE &&
                        lohsRetries-- > 0
                    ) {
                        Log.i(TAG, "hotspot refused (mode conflict), retrying…")
                        handler.postDelayed({ requestLohs() }, LOHS_RETRY_MS)
                        return
                    }
                    starting = false
                    fail("The system refused to start a hotspot (code $reason). Turn off tethering and try again.")
                }

                override fun onStopped() {
                    // The system tore it down (e.g. real tethering started).
                    Log.i(TAG, "local-only hotspot stopped by the system")
                    shutdown()
                    stopSelf()
                }
            }, handler)
        } catch (e: Exception) {
            // SecurityException (permission/location off) or IllegalStateException.
            starting = false
            fail("Couldn't start the hotspot: ${e.message ?: e.javaClass.simpleName}")
        }
    }

    private fun onHotspotUp(res: WifiManager.LocalOnlyHotspotReservation) {
        val (ssid, pass) = credentials(res)
        if (ssid == null || pass == null) {
            fail("The hotspot started but its name/password could not be read.")
            return
        }
        val srv = startServer() ?: run {
            fail("The hotspot is up but the file page could not bind a port.")
            return
        }
        server = srv
        Log.i(TAG, "hotspot '$ssid' up, file page on port ${srv.listeningPort}")
        publish(HotspotView(phase = HotspotPhase.ON, ssid = ssid, passphrase = pass))
        updateNotification("Hotspot “$ssid” is on")
        watchApprovals(ssid)
        awaitOwnAddress(srv.listeningPort, tries = IP_POLL_TRIES)
    }

    /** Keep the notification pointing at the most urgent thing: a transfer
     *  waiting for the owner's OK beats the idle "hotspot is on" line, because
     *  the guest's request times out to a deny if nobody notices it. */
    private fun watchApprovals(ssid: String) {
        approvalsJob?.cancel()
        approvalsJob = scope.launch {
            TransferGate.pending.collect { reqs ->
                val first = reqs.firstOrNull()
                updateNotification(
                    when {
                        first == null -> "Hotspot “$ssid” is on"
                        first.direction == TransferGate.Direction.DOWNLOAD ->
                            "Guest asks for “${first.name}” — open Myco to allow"
                        else -> "Guest sends “${first.name}” — open Myco to accept"
                    },
                )
            }
        }
    }

    /** Bind the file page, walking a few ports in case one is taken. */
    private fun startServer(): FileShareServer? {
        for (port in BASE_PORT until BASE_PORT + PORT_TRIES) {
            val srv = FileShareServer(SharedFiles.get(this), Outbox.get(this), port)
            try {
                srv.start(SOCKET_READ_TIMEOUT_MS, false)
                return srv
            } catch (e: Exception) {
                Log.w(TAG, "port $port unavailable", e)
            }
        }
        return null
    }

    /**
     * The URL guests browse to needs this phone's address *on the hotspot
     * interface*, which the reservation does not expose — poll the interface
     * table until it shows up. It normally exists within a tick or two of
     * [WifiManager.LocalOnlyHotspotCallback.onStarted].
     */
    private fun awaitOwnAddress(port: Int, tries: Int) {
        val ip = hotspotIpv4()
        if (ip != null) {
            val url = "http://$ip:$port"
            publish(_view.value.copy(url = url))
            // From here a bump hands any phone the page: the emulated NFC tag
            // serves the URL and the reader's OS opens it in its browser.
            PairPresent.beginUrl(url)
            return
        }
        if (tries <= 0 || reservation == null) {
            Log.w(TAG, "hotspot interface address never appeared")
            return
        }
        handler.postDelayed({ awaitOwnAddress(port, tries - 1) }, IP_POLL_MS)
    }

    private fun hotspotIpv4(): String? {
        // Every *other* private IPv4 on this phone — client Wi-Fi, cell, VPN —
        // belongs to a Network the app can see through ConnectivityManager; the
        // local-only hotspot is deliberately not surfaced as one. So the hotspot
        // address is the site-local IPv4 on an up interface that no known
        // Network owns. Robust where name/address guessing is not: on Pixel 9
        // the AP interface is plain "wlan2", and Android 13+ randomizes the
        // subnet *and* the host part (seen: 10.221.103.240 — not a ".1").
        val known = knownNetworkV4()
        val candidates = runCatching {
            NetworkInterface.getNetworkInterfaces().toList()
                .filter { runCatching { it.isUp }.getOrDefault(false) }
                .flatMap { i -> i.inetAddresses.toList().filterIsInstance<Inet4Address>().map { i.name to it } }
                .filter { (_, a) -> a.isSiteLocalAddress && a.hostAddress.let { it != null && it !in known } }
        }.getOrDefault(emptyList())
        // Several survivors would be pathological; prefer a softap-ish name.
        val pick = candidates.minByOrNull { (name, _) ->
            if (AP_IFACE_PREFIXES.any(name::startsWith)) 0 else 1
        } ?: return null
        Log.i(TAG, "hotspot address ${pick.second.hostAddress} on ${pick.first}")
        return pick.second.hostAddress
    }

    /** IPv4 addresses of every Network the app can see (see [hotspotIpv4]). */
    private fun knownNetworkV4(): Set<String> {
        val cm = getSystemService(Context.CONNECTIVITY_SERVICE) as android.net.ConnectivityManager
        @Suppress("DEPRECATION") // enumerating all networks is exactly the point
        return cm.allNetworks.flatMap { n ->
            cm.getLinkProperties(n)?.linkAddresses.orEmpty()
                .mapNotNull { (it.address as? Inet4Address)?.hostAddress }
        }.toSet()
    }

    private fun credentials(res: WifiManager.LocalOnlyHotspotReservation): Pair<String?, String?> =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            val c = res.softApConfiguration
            val ssid = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                c.wifiSsid?.toString()?.removeSurrounding("\"")
            } else {
                @Suppress("DEPRECATION") c.ssid
            }
            ssid to c.passphrase
        } else {
            @Suppress("DEPRECATION") // pre-R: WifiConfiguration is all there is
            res.wifiConfiguration.let { c ->
                c?.SSID?.removeSurrounding("\"") to c?.preSharedKey
            }
        }

    private fun fail(message: String) {
        Log.w(TAG, "hotspot failed: $message")
        shutdown(HotspotView(phase = HotspotPhase.ERROR, error = message))
        stopSelf()
    }

    private fun shutdown(finalView: HotspotView = HotspotView()) {
        handler.removeCallbacksAndMessages(null)
        approvalsJob?.cancel()
        approvalsJob = null
        PairPresent.stopUrl()
        // Fail waiting transfers first, so their server threads unblock and the
        // server's stop() isn't held up by sockets mid-approval.
        TransferGate.denyAll()
        // The session's outgoing offers die with it.
        Outbox.get(this).clear()
        server?.let { runCatching { it.stop() } }
        server = null
        reservation?.let { runCatching { it.close() } }
        reservation = null
        starting = false
        if (pausedAware) {
            pausedAware = false
            val prefs = getSharedPreferences("myco_prefs", MODE_PRIVATE)
            if (prefs.getBoolean(MainActivity.PREF_AWARE, true)) {
                Log.i(TAG, "hotspot done — resuming Wi-Fi Aware")
                AwareService.start(this)
            }
        }
        publish(finalView)
        stopForeground(STOP_FOREGROUND_REMOVE)
        Log.i(TAG, "hotspot service stopped")
    }

    override fun onDestroy() {
        if (reservation != null || server != null || starting) shutdown()
        scope.cancel()
        super.onDestroy()
    }

    // --- notification ---

    private fun startForegroundCompat() {
        startForeground(
            NOTIF_ID,
            buildNotification("Starting file-share hotspot…"),
            ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE,
        )
    }

    private fun updateNotification(text: String) {
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        nm.notify(NOTIF_ID, buildNotification(text))
    }

    private fun buildNotification(text: String): Notification {
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        val channel = NotificationChannel(CHANNEL_ID, "Myco hotspot", NotificationManager.IMPORTANCE_LOW)
        channel.description = "File-share hotspot"
        nm.createNotificationChannel(channel)
        val stop = Intent(this, HotspotService::class.java).setAction(ACTION_STOP)
        val stopPi = PendingIntent.getService(
            this, 0, stop,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("Myco")
            .setContentText(text)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setOngoing(true)
            .addAction(0, "Stop", stopPi)
            .build()
    }

    companion object {
        private const val TAG = "MycoHotspot"
        private const val NOTIF_ID = 4
        private const val CHANNEL_ID = "myco_hotspot"
        const val ACTION_START = "app.myco.hotspot.START"
        const val ACTION_STOP = "app.myco.hotspot.STOP"

        /** First port tried for the file page — high, memorable, and clear of
         *  the embedded relay (4870), Blossom (24243), and the UDP lane (4871). */
        private const val BASE_PORT = 8080
        private const val PORT_TRIES = 4

        /** NanoHTTPD per-socket read timeout. Generous: a phone camera-to-
         *  browser flow can sit on the form a while between requests. */
        private const val SOCKET_READ_TIMEOUT_MS = 30_000

        private const val IP_POLL_MS = 500L
        private const val IP_POLL_TRIES = 20

        /** Grace for the HAL to release the Aware NAN interface before the AP
         *  request, and the retry cadence if it is still holding it. */
        private const val AWARE_RELEASE_MS = 1_000L
        private const val LOHS_RETRY_MS = 1_500L
        private const val LOHS_RETRIES = 2

        private val AP_IFACE_PREFIXES = listOf("ap", "swlan", "softap", "wlan1")

        private val _view = MutableStateFlow(HotspotView())

        /** Hotspot + file-page state, for the Circle sheet. */
        val view: StateFlow<HotspotView> = _view.asStateFlow()

        private fun publish(v: HotspotView) {
            _view.value = v
        }

        fun start(context: Context) {
            context.startForegroundService(
                Intent(context, HotspotService::class.java).setAction(ACTION_START),
            )
        }

        fun stop(context: Context) {
            context.startService(
                Intent(context, HotspotService::class.java).setAction(ACTION_STOP),
            )
        }

        /** What [WifiManager.startLocalOnlyHotspot] gates on: NEARBY_WIFI_DEVICES
         *  on 33+ (declared neverForLocation), fine location below — the same
         *  split as the Wi-Fi Aware lane. */
        fun permissions(): List<String> =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                listOf(Manifest.permission.NEARBY_WIFI_DEVICES)
            } else {
                listOf(Manifest.permission.ACCESS_FINE_LOCATION)
            }
    }
}

enum class HotspotPhase { OFF, STARTING, ON, ERROR }

/** What the Circle sheet renders. `url` arrives a beat after `ON` (the softap
 *  interface has to surface its address first). */
data class HotspotView(
    val phase: HotspotPhase = HotspotPhase.OFF,
    val ssid: String? = null,
    val passphrase: String? = null,
    val url: String? = null,
    val error: String? = null,
)
