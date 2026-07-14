package app.myco.aware

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
import app.myco.R
import app.myco.core.MycoCore
import app.myco.core.NativeActions

/**
 * Foreground service that keeps the Wi-Fi Aware bulk lane alive while the app
 * is backgrounded. It owns the [AwareRadio] and enables the lane's UDP
 * transport on the shared node:
 *
 * 1. enable the lane ([NativeActions.setWifiAwareEnabled]) — the node gains a
 *    bound UDP transport instance (rebuilding if it was already running),
 * 2. ensure the node is running (so the lane works standalone, without BLE),
 * 3. create the [AwareRadio] and let it drive discovery + data paths.
 *
 * On stop it disables the lane (dropping the UDP transport) and shuts the radio
 * down, but deliberately does **not** stop the node — [app.myco.ble.BleService]
 * or the app may still want it. Node-lifecycle coordination between the two
 * radio services is intentionally simple; see docs/design/wifi-aware-interop.md.
 */
class AwareService : Service() {
    private var radio: AwareRadio? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stopAware()
                stopSelf()
                return START_NOT_STICKY
            }
            else -> startAware()
        }
        return START_STICKY
    }

    private fun startAware() {
        if (radio != null) return
        startForegroundCompat()

        val client = MycoCore.client(this)
        val ownNpub = client.state().ownNpub
        if (ownNpub.isEmpty()) {
            Log.e(TAG, "no device identity; not starting Aware")
            stopSelf()
            return
        }

        // Enable the lane (adds the UDP transport, restarting a running node),
        // then ensure the node is up so the lane works even without BLE.
        client.dispatch(NativeActions.setWifiAwareEnabled(true))
        client.dispatch(NativeActions.startNode())

        if (!AwareRadio.isSupported(this)) {
            Log.w(TAG, "Wi-Fi Aware not supported on this device")
            client.dispatch(NativeActions.setWifiAwareEnabled(false))
            stopSelf()
            return
        }
        val port = client.state().wifiAwarePort
        // The radio attaches now if Aware is available, or waits for it to
        // become available (e.g. once the user turns Wi-Fi on) — it does not
        // bail, so the lane stays armed. The UI pops the Wi-Fi panel when needed.
        val r = AwareRadio(applicationContext, ownNpub, port)
        r.start()
        radio = r
        Log.i(TAG, "Aware service started (port $port, available=${r.isAvailable()})")
    }

    private fun stopAware() {
        runCatching {
            val client = MycoCore.client(this)
            // Drop the UDP transport; leave the node running for other users.
            client.dispatch(NativeActions.setWifiAwareEnabled(false))
        }
        radio?.shutdown()
        radio = null
        stopForeground(STOP_FOREGROUND_REMOVE)
        Log.i(TAG, "Aware service stopped")
    }

    override fun onDestroy() {
        if (radio != null) stopAware()
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
            val channel = NotificationChannel(CHANNEL_ID, "Myco Wi-Fi", NotificationManager.IMPORTANCE_LOW)
            channel.description = "Wi-Fi Aware bulk transfer"
            nm.createNotificationChannel(channel)
        }
        val stop = Intent(this, AwareService::class.java).setAction(ACTION_STOP)
        val stopPi = android.app.PendingIntent.getService(
            this, 0, stop,
            android.app.PendingIntent.FLAG_IMMUTABLE or android.app.PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("Myco")
            .setContentText("Wi-Fi Aware active")
            .setSmallIcon(R.mipmap.ic_launcher)
            .setOngoing(true)
            .addAction(0, "Stop", stopPi)
            .build()
    }

    companion object {
        private const val TAG = "MycoAwareService"
        private const val NOTIF_ID = 2
        private const val CHANNEL_ID = "myco_aware"
        const val ACTION_START = "app.myco.aware.START"
        const val ACTION_STOP = "app.myco.aware.STOP"

        fun start(context: Context) {
            val i = Intent(context, AwareService::class.java).setAction(ACTION_START)
            context.startForegroundService(i)
        }

        fun stop(context: Context) {
            val i = Intent(context, AwareService::class.java).setAction(ACTION_STOP)
            context.startService(i)
        }
    }
}
