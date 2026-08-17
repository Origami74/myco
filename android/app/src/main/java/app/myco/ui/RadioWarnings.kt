package app.myco.ui

import android.bluetooth.BluetoothManager
import android.content.Context
import android.location.LocationManager
import android.net.VpnService
import android.net.wifi.WifiManager
import app.myco.aware.AwareHealth
import app.myco.ble.BleHealth
import app.myco.core.AppState

/** What tapping a [RadioWarning] should do. Dispatched in SettingsScreen. */
enum class RadioAction { FIX_VPN, ENABLE_BLUETOOTH, ENABLE_WIFI, GRANT_AWARE_PERMISSION, ENABLE_LOCATION }

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
        } else if (scanBlindedByLocation(context)) {
            warnings += RadioWarning(
                title = "Location is off",
                detail = "Others can find Myco, but Myco can't find them. Bluetooth " +
                    "scanning needs location. Tap to turn it on.",
                action = RadioAction.ENABLE_LOCATION,
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

/**
 * Whether the phone's **location services** master switch is on — a plain
 * Android fact, read straight off [LocationManager] the same way
 * [app.myco.aware.AwareRadio.isAvailable] reads Aware's, never routed through
 * `myco-core`. `isLocationEnabled` is API 28 and this app is minSdk 29, so
 * there is no legacy provider fallback to keep.
 *
 * Myco does not want the user's location and never asks for it: `BLUETOOTH_SCAN`
 * is declared `neverForLocation` and `ACCESS_FINE_LOCATION` is capped at
 * API 32. This is read only to explain a scanner that has gone silent — see
 * [scanBlindedByLocation].
 */
fun locationServicesEnabled(context: Context): Boolean =
    (context.getSystemService(Context.LOCATION_SERVICE) as? LocationManager)?.isLocationEnabled == true

/**
 * Whether this phone looks like it is in the state that cost a full day of
 * diagnosis on a DC-1 tablet: **scanning silently returns nothing because
 * location services are off.**
 *
 * On AOSP, `neverForLocation` means the master location switch has nothing to
 * do with BLE scanning from API 31 up. Some vendor stacks ignore that and gate
 * scan callbacks on it anyway. The failure is completely silent — permissions
 * are granted, `startScan` reports success, advertising works and inbound
 * connections land — the device simply never receives a scan result, so it can
 * never learn a peer's PSM and never dials anyone.
 *
 * The test is therefore the *symptom* plus its likely cause, never the setting
 * alone: warning on "location is off" would be a false alarm on every
 * compliant device, which is most of them. [BleHealth.scannerConfirmedSilent]
 * is the symptom — a full minute of listening that produced not one advert —
 * and it clears itself the moment a real advert arrives, which is exactly what
 * happens on this hardware within seconds of location being switched on.
 */
private fun scanBlindedByLocation(context: Context): Boolean =
    BleHealth.scannerConfirmedSilent && !locationServicesEnabled(context)
