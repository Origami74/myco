package app.myco.usb

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.hardware.usb.UsbDevice
import android.hardware.usb.UsbManager
import android.os.Build
import android.os.SystemClock
import android.util.Log

/**
 * Catches phones plugged in while this phone holds the USB host role, for
 * the case the manifest route cannot cover.
 *
 * Android offers an attached device to the apps whose filter matches it
 * ([UsbDeviceActivity]), with one exception: a device exposing MTP — which
 * every Android phone does, in every USB mode — gets the system's own "USB
 * device connected" notification instead, unless the user has already made
 * an app its default. Stock Android (a Pixel) does this; Samsung's build
 * offers the apps. So on a Pixel holding the host role, another phone never
 * reached Myco at all.
 *
 * The `USB_DEVICE_ATTACHED` broadcast still goes to receivers registered at
 * runtime before the system decides what to show, so a running Myco hears
 * it here: a phone-like device is handed to the core if Myco may already use
 * it, and otherwise Myco asks for permission (one system dialog) and hands it
 * over once granted. The core switches it into accessory mode; it comes back
 * as an accessory device, which exposes no MTP, and the manifest route takes
 * it from there.
 *
 * Needs Myco running; with the mesh on it is. Process-wide and idempotent.
 */
object UsbHostWatcher {
    private const val TAG = "MycoUsb"
    private const val ACTION_PERMISSION = "app.myco.usb.HOST_PERMISSION"

    @Volatile
    private var started = false

    fun ensureStarted(context: Context) {
        if (started) return
        synchronized(this) {
            if (started) return
            val app = context.applicationContext
            val filter = IntentFilter(UsbManager.ACTION_USB_DEVICE_ATTACHED).apply {
                addAction(ACTION_PERMISSION)
            }
            val receiver = object : BroadcastReceiver() {
                override fun onReceive(ctx: Context, intent: Intent) = onBroadcast(ctx, intent)
            }
            // Not exported: the system's attach broadcast and Myco's own
            // permission PendingIntent still arrive, and no other app can send
            // the permission action with a device of its choosing.
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                app.registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
            } else {
                app.registerReceiver(receiver, filter)
            }
            started = true
            Log.i(TAG, "watching for USB devices while host")
        }
    }

    private fun onBroadcast(context: Context, intent: Intent) {
        val device = deviceOf(intent) ?: return
        when (intent.action) {
            UsbManager.ACTION_USB_DEVICE_ATTACHED -> {
                if (!looksLikePhone(device)) return
                val usb = context.getSystemService(Context.USB_SERVICE) as UsbManager
                if (usb.hasPermission(device)) {
                    handOver(context, device)
                } else {
                    Log.i(TAG, "asking to use ${device.deviceName} as USB host")
                    val pending = PendingIntent.getBroadcast(
                        context,
                        0,
                        Intent(ACTION_PERMISSION).setPackage(context.packageName),
                        // The system adds the device and the answer as extras.
                        PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                    )
                    usb.requestPermission(device, pending)
                }
            }
            ACTION_PERMISSION -> {
                if (intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)) {
                    handOver(context, device)
                } else {
                    Log.i(TAG, "permission for ${device.deviceName} declined")
                }
            }
        }
    }

    private fun handOver(context: Context, device: UsbDevice) {
        if (UsbDeviceActivity.attach(context, device)) {
            runCatching { UsbService.start(context) }
                .onFailure { Log.w(TAG, "could not start the USB service", it) }
        }
    }

    private fun deviceOf(intent: Intent): UsbDevice? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(UsbManager.EXTRA_DEVICE, UsbDevice::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableExtra(UsbManager.EXTRA_DEVICE)
        }

    /** The same rule as res/xml/usb_device_filter.xml: an MTP/PTP or adb
     *  interface, or a phone already in accessory mode. */
    private fun looksLikePhone(device: UsbDevice): Boolean {
        if (device.vendorId == AOA_VID && device.productId in AOA_PIDS) return true
        return (0 until device.interfaceCount).any { i ->
            val itf = device.getInterface(i)
            itf.interfaceClass == 6 ||
                (itf.interfaceClass == 255 && itf.interfaceSubclass == 66) ||
                (itf.interfaceClass == 255 && itf.interfaceSubclass == 255 && itf.interfaceProtocol == 0)
        }
    }

    private const val AOA_VID = 0x18d1
    private val AOA_PIDS = 0x2d00..0x2d05
}

/**
 * One hand-over per device attachment. A device can reach Myco by two routes
 * at once — the manifest's [UsbDeviceActivity] and [UsbHostWatcher]'s
 * broadcast — and handing the core the same device twice would have it
 * claim the accessory interface twice, or send the switch twice.
 */
internal object UsbHandoff {
    private const val WINDOW_MS = 5_000L
    private val recent = HashMap<String, Long>()

    /** True the first time `device` is claimed within [WINDOW_MS]. */
    @Synchronized
    fun claim(device: UsbDevice): Boolean {
        val now = SystemClock.elapsedRealtime()
        recent.entries.removeAll { now - it.value > WINDOW_MS }
        val key = "${device.deviceName}:${device.vendorId}:${device.productId}"
        if (recent.containsKey(key)) return false
        recent[key] = now
        return true
    }
}
