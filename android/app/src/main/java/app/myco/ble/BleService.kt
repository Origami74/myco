package app.myco.ble

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import app.myco.R
import app.myco.core.MycoCore
import app.myco.core.NativeActions
import app.myco.core.NativeCore

/**
 * Foreground service that keeps BLE peering alive while the app is backgrounded.
 * It owns the [BleRadio] and starts the embedded node's BLE transport:
 *
 * 1. create the radio, inject the byte-bridge into the core ([NativeCore.bleBridgeNew]),
 * 2. dispatch StartNode — the node's BLE transport then drives the radio
 *    (listen/advertise/scan) over the bridge.
 *
 * The node + radio are process-singletons ([MycoCore]); the developer UI reads
 * the same node's state.
 */
class BleService : Service() {
    private var radio: BleRadio? = null
    private var bridgeHandle: Long = 0

    // App-visibility observer: while no activity is visible (home button, screen
    // off), drop BLE discovery from LOW_LATENCY to a duty-cycled LOW_POWER scan —
    // the dominant background battery cost. ProcessLifecycleOwner's onStop fires
    // for both cases; onStart restores full intensity. Established connections
    // are untouched either way.
    private val visibilityObserver = object : DefaultLifecycleObserver {
        override fun onStart(owner: LifecycleOwner) {
            radio?.setBackgroundMode(false)
        }

        override fun onStop(owner: LifecycleOwner) {
            radio?.setBackgroundMode(true)
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stopBle()
                stopSelf()
                return START_NOT_STICKY
            }
            else -> startBle()
        }
        return START_STICKY
    }

    private fun startBle() {
        if (radio != null) return
        startForegroundCompat()

        val client = MycoCore.client(this)
        val r = BleRadio(this)
        val handle = NativeCore.bleBridgeNew(client.handle(), r)
        if (handle == 0L) {
            Log.e(TAG, "bleBridgeNew failed")
            stopSelf()
            return
        }
        r.bindBridge(handle)
        radio = r
        bridgeHandle = handle

        // Inject-then-start: the node's BLE transport picks up the bridge and
        // begins driving the radio (listen → advertise → scan).
        client.dispatch(NativeActions.setBleEnabled(true))
        // Node lifecycle follows the mesh "Enable" master switch, not this
        // toggle. When it's on, (re)start the node: a byte-bridge is only bound
        // at node start, so a node that was already running must bounce to pick
        // up the fresh bridge injected above.
        val meshOn = getSharedPreferences("myco_prefs", MODE_PRIVATE)
            .getBoolean(app.myco.MainActivity.PREF_MESH, true)
        if (meshOn) {
            if (client.state().nodeRunning) client.dispatch(NativeActions.stopNode())
            client.dispatch(NativeActions.startNode())
        }
        // Registration replays the current state (onStart fires immediately if the
        // app is already visible), so the radio starts in the right mode.
        ProcessLifecycleOwner.get().lifecycle.addObserver(visibilityObserver)
        Log.i(TAG, "BLE service started")
    }

    private fun stopBle() {
        ProcessLifecycleOwner.get().lifecycle.removeObserver(visibilityObserver)
        // Node lifecycle belongs to the mesh "Enable" master switch — never stop
        // the node here. Clearing the BLE flag tells the node's BLE backend to
        // stop driving the radio; shutting the radio down closes its channels,
        // which the core observes as channel closures.
        runCatching {
            val client = MycoCore.client(this)
            client.dispatch(NativeActions.setBleEnabled(false))
        }
        radio?.shutdown()
        radio = null
        // Intentionally NOT freeing the bridge handle: the radio's I/O threads may
        // still reference it as they wind down, and the rebuilt node will inject a
        // fresh bridge on the next start. Freeing here risks a use-after-free; the
        // bridge is small, so leaking it on stop is the safe trade.
        bridgeHandle = 0
        stopForeground(STOP_FOREGROUND_REMOVE)
        Log.i(TAG, "BLE service stopped")
    }

    override fun onDestroy() {
        if (radio != null) stopBle()
        super.onDestroy()
    }

    private fun startForegroundCompat() {
        val notif = buildNotification()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(NOTIF_ID, notif, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
        } else {
            startForeground(NOTIF_ID, notif)
        }
    }

    private fun buildNotification(): Notification {
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(CHANNEL_ID, "Myco BLE", NotificationManager.IMPORTANCE_LOW)
            channel.description = "Bluetooth peering"
            nm.createNotificationChannel(channel)
        }
        val stop = Intent(this, BleService::class.java).setAction(ACTION_STOP)
        val stopPi = android.app.PendingIntent.getService(
            this, 0, stop,
            android.app.PendingIntent.FLAG_IMMUTABLE or android.app.PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("Myco")
            .setContentText("BLE peering active")
            .setSmallIcon(R.mipmap.ic_launcher)
            .setOngoing(true)
            .addAction(0, "Stop", stopPi)
            .build()
    }

    companion object {
        private const val TAG = "MycoBleService"
        private const val NOTIF_ID = 1
        private const val CHANNEL_ID = "myco_ble"
        const val ACTION_START = "app.myco.ble.START"
        const val ACTION_STOP = "app.myco.ble.STOP"

        fun start(context: Context) {
            val i = Intent(context, BleService::class.java).setAction(ACTION_START)
            context.startForegroundService(i)
        }

        fun stop(context: Context) {
            val i = Intent(context, BleService::class.java).setAction(ACTION_STOP)
            context.startService(i)
        }
    }
}
