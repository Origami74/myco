package app.myco.onboarding

import android.Manifest
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.net.VpnService
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import app.myco.MainActivity
import app.myco.aware.AwareRadio

/**
 * What a [SystemAsk] says before Android shows its prompt: a title, one line
 * on what Android is about to ask, one note on what Myco does with it, and
 * the label of the button that asks — naming what it opens, so the tap that
 * puts Android's prompt up never reads as "next page".
 * Rendered only by `ExplainCard` — in the setup popup and in the small
 * explain dialog — so a prompt can't be shown with different, or no, words.
 */
data class Explanation(
    val title: String,
    val body: String,
    val note: String,
    val button: String = "Continue",
)

/** How Android is asked. */
enum class AskMechanism {
    /** Runtime permissions, in one request. */
    Permissions,

    /** `VpnService.prepare`'s consent activity. */
    VpnConsent,

    /** A system activity that asks on Myco's behalf ([SystemAsk.intentAction]). */
    Intent,
}

/**
 * Every prompt Android shows on Myco's behalf, in one place: what it asks
 * for, whether it is already granted, and what Myco says before it.
 *
 * This is the only file that names a prompt's permissions, consent or intent,
 * and the only one with its explanation; `SystemAsker` is the only code that
 * launches one, and only from a tap on an explanation (`ExplainCard`). A JVM
 * test (`SystemAskTest`) holds the rest of the source tree to that.
 *
 * [Hotspot] is its own entry, not [Nearby]: the local-only hotspot needs only
 * the Wi-Fi half of the nearby group, whatever the mesh lane switches say and
 * whether or not the phone has Wi-Fi Aware — asking [Nearby] for it would ask
 * for Bluetooth it doesn't use, and could leave out the one permission it does.
 * It shares [Nearby]'s Wi-Fi list ([wifiNearby]) rather than holding a copy.
 */
enum class SystemAsk(
    val mechanism: AskMechanism,
    val explanation: Explanation,
    /** The system activity's action, for [AskMechanism.Intent]. */
    val intentAction: String? = null,
) {
    /** Bluetooth and Nearby Wi-Fi devices (location below API 31/33) for the mesh lanes left on. */
    Nearby(
        AskMechanism.Permissions,
        Explanation(
            title = "Nearby devices",
            body = "Next, Android asks to find nearby devices.",
            note = "Myco uses Bluetooth and Wi-Fi to find nearby mesh devices. It doesn’t record where you are.",
            button = "Allow nearby devices",
        ),
    ),

    /** The VPN consent behind the mesh adapter (app-owned TUN). */
    Vpn(
        AskMechanism.VpnConsent,
        Explanation(
            title = "Mesh connection",
            body = "Next, Android asks to set up a VPN.",
            note = "The VPN connects this phone to the FIPS mesh, so any app can reach devices on it. " +
                "Your regular internet traffic doesn’t go through it.",
            button = "Allow VPN",
        ),
    ),

    /** POST_NOTIFICATIONS on API 33+; below that, the app's notification settings. */
    Notifications(
        AskMechanism.Permissions,
        Explanation(
            title = "Notifications",
            body = "Next, Android asks to allow notifications.",
            note = "Myco tells you when someone sends you a file.",
        ),
    ),

    /**
     * Leave Myco out of battery optimisation (Doze app standby), so phones
     * that suspend background apps don't cut the radios while the screen is
     * off.
     *
     * The direct request needs `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` in the
     * manifest. Google Play only allows that permission for a short list of
     * app types; Myco ships through GitHub Releases and Zapstore, which have
     * no such rule (F-Droid doesn't either). Should a store ever object, the
     * fallback in `SystemAsker` — Android's own list of apps — needs no
     * permission at all.
     */
    Battery(
        AskMechanism.Intent,
        Explanation(
            title = "Keep running",
            body = "Next, Android asks to let Myco run in the background.",
            note = "Myco stays connected to the mesh while the screen is off.",
        ),
        intentAction = Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS,
    ),

    /** The QR scanner's camera. */
    Camera(
        AskMechanism.Permissions,
        Explanation(
            title = "Camera",
            body = "Next, Android asks to use the camera.",
            note = "Myco uses the camera only to scan codes.",
        ),
    ),

    /** What `WifiManager.startLocalOnlyHotspot` gates on, for file sharing with any phone. */
    Hotspot(
        AskMechanism.Permissions,
        Explanation(
            title = "Share over hotspot",
            body = "Next, Android asks to find nearby devices.",
            note = "Android needs it to open the Wi-Fi hotspot other phones join. It doesn’t record where you are.",
        ),
    ),

    /** Android's "turn Bluetooth on?" dialog, from the "Bluetooth is off" warning. */
    BluetoothOn(
        AskMechanism.Intent,
        Explanation(
            title = "Turn on Bluetooth",
            body = "Next, Android asks to turn Bluetooth on.",
            note = "Myco uses Bluetooth to find and link nearby mesh devices.",
        ),
        intentAction = BluetoothAdapter.ACTION_REQUEST_ENABLE,
    ),
    ;

    /**
     * The runtime permissions this asks for on [sdk]; empty for the
     * consent and intent prompts, and for notifications below API 33 (no
     * runtime permission there). For [Nearby], [bleLane] and [awareLane] are
     * the lanes the user has left on (and the phone has).
     */
    fun permissions(sdk: Int, bleLane: Boolean = true, awareLane: Boolean = true): List<String> = when (this) {
        Nearby -> buildList {
            if (bleLane) addAll(ble(sdk))
            if (awareLane) addAll(wifiNearby(sdk))
        }.distinct()
        Hotspot -> wifiNearby(sdk)
        Camera -> listOf(Manifest.permission.CAMERA)
        Notifications ->
            if (sdk >= Build.VERSION_CODES.TIRAMISU) listOf(Manifest.permission.POST_NOTIFICATIONS) else emptyList()
        Vpn, Battery, BluetoothOn -> emptyList()
    }

    /** [permissions] for this phone, with [Nearby] following the lane switches. */
    fun permissions(context: Context): List<String> {
        val prefs = context.getSharedPreferences("myco_prefs", Context.MODE_PRIVATE)
        return permissions(
            sdk = Build.VERSION.SDK_INT,
            bleLane = prefs.getBoolean(MainActivity.PREF_BLE, true),
            awareLane = prefs.getBoolean(MainActivity.PREF_AWARE, true) && AwareRadio.isSupported(context),
        )
    }

    /**
     * The activity Android asks through, for the consent and intent prompts;
     * null when there is nothing to ask ([Vpn] already Myco's) or the prompt
     * is a permission request.
     */
    fun intent(context: Context): Intent? = when (mechanism) {
        AskMechanism.VpnConsent -> vpnConsent(context)
        AskMechanism.Intent -> when (this) {
            Battery -> Intent(intentAction, Uri.parse("package:${context.packageName}"))
            else -> Intent(intentAction)
        }
        AskMechanism.Permissions -> null
    }

    /** Whether Android has nothing left to ask. Some of these are binder calls: off the main thread where it matters. */
    fun granted(context: Context): Boolean = when (this) {
        Nearby, Hotspot, Camera -> granted(context, permissions(context))
        Notifications -> NotificationManagerCompat.from(context).areNotificationsEnabled()
        Battery -> context.getSystemService(PowerManager::class.java)
            ?.isIgnoringBatteryOptimizations(context.packageName) == true
        Vpn -> vpnConsent(context) == null
        BluetoothOn -> (context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager)
            ?.adapter?.isEnabled == true
    }

    companion object {
        /** The BLE lane's runtime permissions — the Bluetooth half of [Nearby]. */
        fun ble(sdk: Int = Build.VERSION.SDK_INT): List<String> =
            if (sdk >= Build.VERSION_CODES.S) {
                listOf(
                    Manifest.permission.BLUETOOTH_SCAN,
                    Manifest.permission.BLUETOOTH_ADVERTISE,
                    Manifest.permission.BLUETOOTH_CONNECT,
                )
            } else {
                location()
            }

        /**
         * NEARBY_WIFI_DEVICES on API 33+ (declared neverForLocation), location
         * on 29–32: what Wi-Fi Aware and the local-only hotspot gate on — the
         * Wi-Fi half of [Nearby], and all of [Hotspot].
         */
        fun wifiNearby(sdk: Int = Build.VERSION.SDK_INT): List<String> =
            if (sdk >= Build.VERSION_CODES.TIRAMISU) {
                listOf(Manifest.permission.NEARBY_WIFI_DEVICES)
            } else {
                location()
            }

        /**
         * Fine location, always with coarse: from API 31 Android ignores a
         * request for fine alone (logcat: "ACCESS_FINE_LOCATION must be
         * requested with ACCESS_COARSE_LOCATION"), so on Android 12 the Wi-Fi
         * half of [Nearby] could never be granted. Harmless below 31.
         */
        private fun location(): List<String> =
            listOf(Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION)

        fun granted(context: Context, permissions: List<String>): Boolean = granted(permissions) {
            ContextCompat.checkSelfPermission(context, it) == PackageManager.PERMISSION_GRANTED
        }

        /**
         * Whether [permissions] are all held, by [isGranted]. Coarse location
         * counts as held when fine is: below API 31 a phone upgrading from a
         * build that never declared coarse holds fine alone, and that is all
         * its radios check — it shouldn't read as "not allowed", or be asked
         * again. (From 31 on, fine is never held without coarse.)
         */
        fun granted(permissions: List<String>, isGranted: (String) -> Boolean): Boolean = permissions.all {
            isGranted(it) ||
                (it == Manifest.permission.ACCESS_COARSE_LOCATION && isGranted(Manifest.permission.ACCESS_FINE_LOCATION))
        }

        /**
         * The one `VpnService.prepare` call: null when the VPN consent is
         * Myco's, otherwise the consent activity to launch. A binder call.
         */
        private fun vpnConsent(context: Context): Intent? = VpnService.prepare(context)
    }
}
