package app.myco.onboarding

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.net.VpnService
import android.os.Build
import android.os.PowerManager
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import app.myco.MainActivity
import app.myco.aware.AwareRadio

/**
 * What the mesh needs from Android, read in one place so the setup popup,
 * Settings › Permissions and the lane start paths agree on it.
 */
object MeshPermissions {

    /** The BLE radio's runtime permissions (notifications are separate). */
    fun bleCore(): List<String> =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            listOf(
                Manifest.permission.BLUETOOTH_SCAN,
                Manifest.permission.BLUETOOTH_ADVERTISE,
                Manifest.permission.BLUETOOTH_CONNECT,
            )
        } else {
            listOf(Manifest.permission.ACCESS_FINE_LOCATION)
        }

    /** NEARBY_WIFI_DEVICES on API 33+; ACCESS_FINE_LOCATION gates Aware on 29–32. */
    fun aware(): List<String> =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            listOf(Manifest.permission.NEARBY_WIFI_DEVICES)
        } else {
            listOf(Manifest.permission.ACCESS_FINE_LOCATION)
        }

    /**
     * "Nearby phones": what the lanes the user has left on still need, as one
     * list — Bluetooth when its switch is on, Wi-Fi Aware when its switch is on
     * and the phone has it. Asked for in a single request: Android shows one
     * permission request at a time and drops a second launched behind it.
     */
    fun nearby(context: Context): List<String> {
        val prefs = context.getSharedPreferences("myco_prefs", Context.MODE_PRIVATE)
        return buildList {
            if (prefs.getBoolean(MainActivity.PREF_BLE, true)) addAll(bleCore())
            if (prefs.getBoolean(MainActivity.PREF_AWARE, true) && AwareRadio.isSupported(context)) {
                addAll(aware())
            }
        }.distinct()
    }

    fun granted(context: Context, permissions: List<String>): Boolean = permissions.all {
        ContextCompat.checkSelfPermission(context, it) == PackageManager.PERMISSION_GRANTED
    }

    fun nearbyGranted(context: Context): Boolean = granted(context, nearby(context))

    /** The VPN consent is Myco's: `prepare()` has nothing left to ask. A binder call. */
    fun vpnPrepared(context: Context): Boolean = VpnService.prepare(context) == null

    fun notificationsEnabled(context: Context): Boolean =
        NotificationManagerCompat.from(context).areNotificationsEnabled()

    /** Android leaves Myco out of battery optimisation (Doze app standby). */
    fun ignoringBatteryOptimizations(context: Context): Boolean =
        context.getSystemService(PowerManager::class.java)
            ?.isIgnoringBatteryOptimizations(context.packageName) == true
}
