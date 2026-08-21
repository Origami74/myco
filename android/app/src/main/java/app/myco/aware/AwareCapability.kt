package app.myco.aware

import android.content.Context
import android.content.pm.PackageManager
import android.net.wifi.aware.WifiAwareManager
import android.os.Build
import android.util.Log

/**
 * What the Wi-Fi Aware chipset says it can do — specifically, how many
 * concurrent data paths it supports, which is what the core sizes its UDP
 * socket pool to.
 *
 * # Why this is read separately from the radio
 *
 * The pool is bound when the **node** starts, which happens whether or not the
 * Aware lane is on and long before [AwareRadio] attaches. So the number cannot
 * come from the radio: it is read here, pushed to the core, and persisted, and
 * the *next* node start uses it. A running node is never rebuilt for it — that
 * would drop every live link, including BLE ones with nothing to do with Aware.
 *
 * # Why it is not always available
 *
 * `getNumberOfSupportedDataPaths()` is API 33, above this app's minSdk of 29,
 * and `getCharacteristics()` returns null while Wi-Fi is off. Either way the
 * answer is null and the core keeps its default until a launch that can read
 * it.
 *
 * # Not to be confused with availability
 *
 * `AwareResources.getAvailableDataPathsCount()` is what is free *right now*,
 * and it lies for this purpose: it reported 7 free throughout a run of instant
 * refusals on a device that carried one peer. Capability comes from
 * `Characteristics`, which matches `dumpsys wifiaware`'s `maxNdpSessions`.
 */
internal object AwareCapability {

    /**
     * Concurrent data paths this chipset supports, or null if it cannot be
     * asked (no Aware hardware, API below 33, or Wi-Fi off).
     */
    fun supportedDataPaths(context: Context): Int? {
        if (!context.packageManager.hasSystemFeature(PackageManager.FEATURE_WIFI_AWARE)) return null
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return null
        val mgr = context.getSystemService(Context.WIFI_AWARE_SERVICE) as? WifiAwareManager
        // Null while the radio is unavailable — Wi-Fi off, typically.
        val characteristics = runCatching { mgr?.characteristics }.getOrNull() ?: return null
        val paths = runCatching { characteristics.numberOfSupportedDataPaths }.getOrNull()
        if (paths == null || paths <= 0) return null
        Log.i(TAG, "chipset supports $paths concurrent Aware data paths")
        return paths
    }

    private const val TAG = "MycoAwareCap"
}
