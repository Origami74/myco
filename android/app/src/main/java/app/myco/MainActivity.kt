package app.myco

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts.RequestMultiplePermissions
import androidx.core.content.ContextCompat
import app.myco.ble.BleService
import app.myco.core.AppCoreClient
import app.myco.core.MycoCore
import app.myco.ui.DeveloperScreen

/**
 * Developer-UI entry point: shows the device identity, node status, and the BLE
 * peering diagnostics. The node + radio are process-singletons ([MycoCore]); the
 * BLE toggle starts/stops the foreground [BleService] that owns the radio.
 */
class MainActivity : ComponentActivity() {
    private lateinit var core: AppCoreClient
    private val permLauncher = registerForActivityResult(RequestMultiplePermissions()) {}

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        core = MycoCore.client(this)
        requestBlePermissionsIfNeeded()

        setContent {
            DeveloperScreen(
                client = core,
                onBleToggle = { enabled ->
                    if (enabled) {
                        requestBlePermissionsIfNeeded()
                        BleService.start(this)
                    } else {
                        BleService.stop(this)
                    }
                },
            )
        }
    }

    private fun requestBlePermissionsIfNeeded() {
        val needed = blePermissions().filter {
            ContextCompat.checkSelfPermission(this, it) != PackageManager.PERMISSION_GRANTED
        }
        if (needed.isNotEmpty()) permLauncher.launch(needed.toTypedArray())
    }

    private fun blePermissions(): List<String> = buildList {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            add(Manifest.permission.BLUETOOTH_SCAN)
            add(Manifest.permission.BLUETOOTH_ADVERTISE)
            add(Manifest.permission.BLUETOOTH_CONNECT)
        } else {
            add(Manifest.permission.ACCESS_FINE_LOCATION)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            add(Manifest.permission.POST_NOTIFICATIONS)
        }
    }
}
