package app.myco.usb

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.hardware.usb.UsbManager
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import app.myco.MainActivity
import app.myco.R
import app.myco.core.MycoCore
import app.myco.core.NativeActions

/**
 * Foreground service for the length of a USB link: it keeps the process alive
 * while a cable connects this phone to another fips node, and starts the mesh
 * node when the cable is what started the app.
 *
 * The link itself lives in the core — the descriptor was handed over by
 * [UsbAccessoryActivity] or [UsbDeviceActivity] — so all this does is hold the
 * process up and leave when the cable goes away. Like the radio services, it follows the mesh
 * master switch: with mesh off it starts no node, and a node is never bounced
 * to adopt the link.
 */
class UsbService : Service() {
    private var detachWatch: BroadcastReceiver? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        startForegroundCompat()
        watchDetach()

        val meshOn = getSharedPreferences("myco_prefs", MODE_PRIVATE)
            .getBoolean(MainActivity.PREF_MESH, true)
        val client = MycoCore.client(this)
        if (meshOn && !client.state().nodeRunning) {
            Log.i(TAG, "USB link with the node down; starting it")
            client.dispatch(NativeActions.startNode())
        }

        // The cable may already be out by the time this runs.
        if (!cableIn()) {
            Log.i(TAG, "no USB peer attached; stopping")
            stopSelf()
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        detachWatch?.let { runCatching { unregisterReceiver(it) } }
        detachWatch = null
        super.onDestroy()
    }

    /** Whether a USB peer is attached on either side of a cable: an accessory
     *  (this phone is the accessory) or a device (this phone is host). */
    private fun cableIn(): Boolean {
        val usb = getSystemService(Context.USB_SERVICE) as UsbManager
        return !usb.accessoryList.isNullOrEmpty() || usb.deviceList.isNotEmpty()
    }

    /** Leave when the cable is pulled; the core sees the link end on its own.
     *  A host-role switch detaches the device it switched, so a detach only
     *  stops the service once nothing is attached at all. */
    private fun watchDetach() {
        if (detachWatch != null) return
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                if (cableIn()) return
                Log.i(TAG, "USB peer detached")
                stopSelf()
            }
        }
        val filter = IntentFilter(UsbManager.ACTION_USB_ACCESSORY_DETACHED).apply {
            addAction(UsbManager.ACTION_USB_DEVICE_DETACHED)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            registerReceiver(receiver, filter, RECEIVER_NOT_EXPORTED)
        } else {
            registerReceiver(receiver, filter)
        }
        detachWatch = receiver
    }

    private fun startForegroundCompat() {
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(CHANNEL_ID, "Myco USB", NotificationManager.IMPORTANCE_LOW)
            channel.description = "Mesh link over a USB cable"
            nm.createNotificationChannel(channel)
        }
        val notif: Notification = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("Myco")
            .setContentText("Mesh link over USB")
            .setSmallIcon(R.mipmap.ic_launcher)
            .setOngoing(true)
            .build()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(NOTIF_ID, notif, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
        } else {
            startForeground(NOTIF_ID, notif)
        }
    }

    companion object {
        private const val TAG = "MycoUsbService"
        private const val NOTIF_ID = 5
        private const val CHANNEL_ID = "myco_usb"

        fun start(context: Context) {
            context.startForegroundService(Intent(context, UsbService::class.java))
        }
    }
}
