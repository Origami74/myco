package app.myco.usb

import android.app.Activity
import android.content.Context
import android.hardware.usb.UsbAccessory
import android.hardware.usb.UsbManager
import android.os.Build
import android.os.Bundle
import android.util.Log
import app.myco.core.NativeCore

/**
 * Started by Android when a USB host switches this phone into accessory mode
 * as `FIPS`/`fips-link` (res/xml/accessory_filter.xml) — a desktop fips node,
 * or another phone holding the cable's host role. Being started by that
 * intent is the permission to open the accessory, so it is opened at once,
 * its descriptor handed to the node's USB transport, and [UsbService] started
 * to keep the process — and the mesh — up while the cable is in. Shows
 * nothing.
 *
 * This also runs when Myco is not running at all: plugging in is what starts
 * the app, with the screen locked too. The node may not exist yet; the core
 * holds the descriptor until it does.
 */
class UsbAccessoryActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val accessory = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(UsbManager.EXTRA_ACCESSORY, UsbAccessory::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableExtra(UsbManager.EXTRA_ACCESSORY)
        }
        if (accessory == null) {
            Log.w(TAG, "launched without an accessory")
        } else if (attach(this, accessory)) {
            UsbService.start(this)
        }
        finish()
    }

    companion object {
        private const val TAG = "MycoUsb"

        /** Open `accessory` and give it to the core. False if it could not be
         *  opened — already open elsewhere, or detached in the meantime. */
        fun attach(context: Context, accessory: UsbAccessory): Boolean {
            val usb = context.getSystemService(Context.USB_SERVICE) as UsbManager
            val pfd = runCatching { usb.openAccessory(accessory) }.getOrNull()
            if (pfd == null) {
                Log.w(TAG, "could not open accessory ${accessory.manufacturer}/${accessory.model}")
                return false
            }
            // Detach: the core owns the descriptor from here and closes it
            // when the link ends.
            val fd = pfd.detachFd()
            val taken = NativeCore.usbAttachAccessory(fd)
            Log.i(TAG, "accessory ${accessory.manufacturer}/${accessory.model} v${accessory.version} → core (taken=$taken)")
            return taken
        }
    }
}
