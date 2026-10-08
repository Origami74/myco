package app.myco.usb

import android.app.Activity
import android.content.Context
import android.hardware.usb.UsbDevice
import android.hardware.usb.UsbManager
import android.os.Build
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.util.Log
import app.myco.core.NativeCore

/**
 * Started by Android when this phone holds a USB cable's host role and a
 * device matching res/xml/usb_device_filter.xml is plugged in — another phone,
 * or a phone that has already switched into accessory mode. Being started by
 * that intent is the permission to open the device, so it is opened at once,
 * a duplicate of its descriptor handed to the node, and [UsbService] started
 * to keep the process up while the cable is in. Shows nothing.
 *
 * The node does the rest: a phone not yet in accessory mode is asked to
 * switch, leaves the bus, and comes back as an accessory device — which starts
 * this activity again, and that time the node links with it. Android asks the
 * user once for each of the two devices ("Always" remembers the answer).
 */
class UsbDeviceActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val device = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(UsbManager.EXTRA_DEVICE, UsbDevice::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableExtra(UsbManager.EXTRA_DEVICE)
        }
        if (device == null) {
            Log.w(TAG, "launched without a device")
        } else if (attach(this, device)) {
            UsbService.start(this)
        }
        finish()
    }

    companion object {
        private const val TAG = "MycoUsb"

        /** Open `device` and give a duplicate of its descriptor to the core.
         *  The connection itself is closed again: the duplicate stays valid,
         *  and the core claims what it needs through it. */
        fun attach(context: Context, device: UsbDevice): Boolean {
            if (!UsbHandoff.claim(device)) {
                Log.i(TAG, "${device.deviceName} already handed to the core")
                return false
            }
            val usb = context.getSystemService(Context.USB_SERVICE) as UsbManager
            if (!usb.hasPermission(device)) {
                Log.w(TAG, "no permission for ${device.deviceName}")
                return false
            }
            val conn = runCatching { usb.openDevice(device) }.getOrNull()
            if (conn == null) {
                Log.w(TAG, "could not open ${device.deviceName}")
                return false
            }
            val taken = try {
                val dup = ParcelFileDescriptor.fromFd(conn.fileDescriptor)
                NativeCore.usbAttachHostDevice(dup.detachFd())
            } catch (t: Throwable) {
                Log.w(TAG, "could not hand ${device.deviceName} to the core", t)
                false
            } finally {
                conn.close()
            }
            Log.i(
                TAG,
                "device %04x:%04x ${device.deviceName} → core (taken=$taken)"
                    .format(device.vendorId, device.productId),
            )
            return taken
        }
    }
}
