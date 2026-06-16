package app.myco.vpn

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.VpnService
import android.os.ParcelFileDescriptor
import android.util.Log
import app.myco.MainActivity
import app.myco.R
import app.myco.core.NativeCore
import java.io.FileInputStream
import java.io.FileOutputStream
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The **app-owned TUN**: a [VpnService] that owns the TUN fd and pumps IPv6 packet
 * bytes between it and the FIPS node over the JNI bridge ([NativeCore.tunSendPacket]
 * / [NativeCore.tunNextPacket]). It routes **only `fd00::/8`** (the mesh ULA), so
 * normal internet traffic is untouched — this is not a full-tunnel VPN.
 *
 * The node installs the bridge channels when it starts (BLE on); this service just
 * moves bytes. With it up, a native socket to `[fd00::peer]:4869/:24242` routes
 * over the mesh, so the sync engine can pull a shared nsite from a peer's device.
 */
class MycoVpnService : VpnService() {
    private var tun: ParcelFileDescriptor? = null
    private val running = AtomicBoolean(false)
    @Volatile private var readerThread: Thread? = null
    @Volatile private var writerThread: Thread? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopTun()
            stopSelf()
            return START_NOT_STICKY
        }
        startTun(intent?.getStringExtra(EXTRA_ULA).orEmpty())
        return START_STICKY
    }

    private fun startTun(ula: String) {
        if (running.get()) return
        if (ula.isEmpty()) {
            stopSelf()
            return
        }
        startForegroundCompat()

        val pfd = try {
            Builder()
                .setSession("Myco mesh")
                .setMtu(MTU)
                .addAddress(ula, 128) // this node's ULA
                .addRoute("fd00::", 8) // route ONLY the mesh ULA range
                .setConfigureIntent(configIntent())
                .establish()
        } catch (t: Throwable) {
            Log.e(TAG, "establish failed", t)
            null
        }
        if (pfd == null) {
            stopSelf()
            return
        }
        tun = pfd
        running.set(true)
        readerThread = Thread({ readLoop(pfd) }, "myco-tun-read").apply { start() }
        writerThread = Thread({ writeLoop(pfd) }, "myco-tun-write").apply { start() }
        Log.i(TAG, "mesh TUN up at $ula (route fd00::/8)")
    }

    /** TUN fd → mesh: read IPv6 packets and hand them to FIPS. */
    private fun readLoop(pfd: ParcelFileDescriptor) {
        val input = FileInputStream(pfd.fileDescriptor)
        val buf = ByteArray(MTU + 64)
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
        val buf = ByteArray(MTU + 64)
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

    private fun stopTun() {
        if (!running.compareAndSet(true, false)) {
            try {
                tun?.close()
            } catch (_: Exception) {
            }
            tun = null
            stopForeground(STOP_FOREGROUND_REMOVE)
            return
        }
        readerThread?.interrupt()
        writerThread?.interrupt()
        try {
            tun?.close()
        } catch (_: Exception) {
        }
        tun = null
        stopForeground(STOP_FOREGROUND_REMOVE)
        Log.i(TAG, "mesh TUN down")
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

    companion object {
        const val EXTRA_ULA = "app.myco.extra.ULA"
        private const val ACTION_STOP = "app.myco.vpn.STOP"
        private const val CHANNEL = "myco_mesh"
        private const val NOTIF_ID = 42
        private const val MTU = 1280
        private const val TAG = "MycoVpn"

        fun start(context: Context, ula: String) {
            context.startService(
                Intent(context, MycoVpnService::class.java).putExtra(EXTRA_ULA, ula),
            )
        }

        fun stop(context: Context) {
            context.startService(
                Intent(context, MycoVpnService::class.java).setAction(ACTION_STOP),
            )
        }
    }
}
