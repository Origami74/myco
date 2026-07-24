package app.myco.ui

import android.bluetooth.BluetoothManager
import android.content.Context
import android.net.VpnService
import android.net.wifi.WifiManager
import app.myco.aware.AwareHealth
import app.myco.core.AppState

/** What tapping a [RadioWarning] should do. Dispatched in SettingsScreen. */
enum class RadioAction { FIX_VPN, ENABLE_BLUETOOTH, ENABLE_WIFI, GRANT_AWARE_PERMISSION }

/** One actionable radio/VPN misconfiguration to surface to the user. */
data class RadioWarning(val title: String, val detail: String, val action: RadioAction)

/**
 * Cross-check the app's transport toggles against the phone's actual radio /
 * VPN state, and return every mismatch that silently breaks peering. Cheap
 * enough to recompute on each 1s state poll: three service lookups and (when
 * the mesh is on) one `VpnService.prepare` binder call.
 */
fun radioWarnings(context: Context, state: AppState, meshEnabled: Boolean): List<RadioWarning> {
    val warnings = mutableListOf<RadioWarning>()

    // The mesh rides an app-owned VPN/TUN. prepare() != null means the VPN
    // slot is NOT ours (consent revoked, or another VPN app took the slot) —
    // the node may look healthy but no mesh traffic can flow.
    if (meshEnabled && VpnService.prepare(context) != null) {
        warnings += RadioWarning(
            title = "Mesh has no VPN slot",
            detail = "Another app holds the VPN slot (or access was revoked), so no mesh " +
                "traffic can flow. Tap to re-assign the VPN to Myco.",
            action = RadioAction.FIX_VPN,
        )
    }

    if (state.bleEnabled) {
        val adapter = (context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager)?.adapter
        if (adapter?.isEnabled != true) {
            warnings += RadioWarning(
                title = "Bluetooth is off",
                detail = "The Bluetooth transport is enabled, but the phone's Bluetooth is " +
                    "turned off — nearby peers can't be found. Tap to turn Bluetooth on.",
                action = RadioAction.ENABLE_BLUETOOTH,
            )
        }
    }

    if (state.wifiAwareEnabled) {
        val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
        if (wifi?.isWifiEnabled != true) {
            warnings += RadioWarning(
                title = "Wi-Fi is off",
                detail = "Wi-Fi Aware is enabled, but the phone's Wi-Fi is turned off — the " +
                    "fast transfer lane can't run. Tap to turn Wi-Fi on.",
                action = RadioAction.ENABLE_WIFI,
            )
        }
        if (AwareHealth.permissionDenied) {
            warnings += RadioWarning(
                title = "Wi-Fi Aware permission denied",
                detail = "The system refused Wi-Fi Aware for lack of the nearby-devices " +
                    "permission. Tap to open app settings and grant it, then re-enable " +
                    "Wi-Fi Aware.",
                action = RadioAction.GRANT_AWARE_PERMISSION,
            )
        }
    }

    return warnings
}
