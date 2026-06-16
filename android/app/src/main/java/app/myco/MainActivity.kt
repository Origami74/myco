package app.myco

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ShortcutInfo
import android.content.pm.ShortcutManager
import android.graphics.drawable.Icon
import android.net.VpnService
import android.os.Build
import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.result.contract.ActivityResultContracts.RequestMultiplePermissions
import androidx.core.content.ContextCompat
import app.myco.ble.BleService
import app.myco.core.AppCoreClient
import app.myco.core.MycoCore
import app.myco.core.NativeActions
import app.myco.share.NsiteShare
import app.myco.ui.DeveloperScreen
import app.myco.vpn.MycoVpnService
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions

/**
 * Developer-UI entry point: device identity, node status, BLE diagnostics, and
 * the nsite list (paste a link / scan a QR / open / share / pin to home). The
 * node + radio are process-singletons ([MycoCore]); the BLE toggle starts/stops
 * the foreground [BleService] and its choice is remembered across restarts.
 */
class MainActivity : ComponentActivity() {
    private lateinit var core: AppCoreClient
    private val prefs by lazy { getSharedPreferences("myco_prefs", MODE_PRIVATE) }

    private val permLauncher = registerForActivityResult(RequestMultiplePermissions()) {
        // BLE is enabled by default / remembered; (re)start it once perms land.
        if (prefs.getBoolean(PREF_BLE, true) && bleCorePermsGranted()) {
            BleService.start(this)
        }
    }

    private val scanLauncher = registerForActivityResult(ScanContract()) { result ->
        result.contents?.let { handleScannedText(it) }
    }

    private val vpnConsentLauncher =
        registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            if (result.resultCode == RESULT_OK) startMeshNow()
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        core = MycoCore.client(this)

        // BLE on by default, and remembered thereafter.
        if (prefs.getBoolean(PREF_BLE, true)) {
            if (bleCorePermsGranted()) BleService.start(this) else requestBlePermissionsIfNeeded()
        }

        // The mesh adapter (app-owned TUN) is opt-in; if it was on and consent is
        // still granted, bring it back up.
        if (prefs.getBoolean(PREF_MESH, false) && VpnService.prepare(this) == null) {
            startMeshNow()
        }

        setContent {
            DeveloperScreen(
                client = core,
                onBleToggle = { enabled -> setBleEnabled(enabled) },
                onLaunchNsite = { hostLabel, title -> launchNsite(hostLabel, title) },
                onPinToHome = { hostLabel, title -> pinToHomeScreen(hostLabel, title) },
                onScan = { startScan() },
                initialMeshEnabled = prefs.getBoolean(PREF_MESH, false),
                onMeshToggle = { enabled -> setMeshEnabled(enabled) },
            )
        }

        handleDeepLink(intent)
    }

    // --- mesh adapter (app-owned TUN) ---

    private fun setMeshEnabled(enabled: Boolean) {
        prefs.edit().putBoolean(PREF_MESH, enabled).apply()
        if (enabled) {
            // Android requires user consent before any app can run a VPN.
            val consent = VpnService.prepare(this)
            android.util.Log.i("MycoVpn", "setMeshEnabled(true): consent needed=${consent != null}")
            if (consent != null) vpnConsentLauncher.launch(consent) else startMeshNow()
        } else {
            MycoVpnService.stop(this)
        }
    }

    private fun startMeshNow() {
        val state = core.state()
        val ula = state.fipsIpv6
        android.util.Log.i("MycoVpn", "startMeshNow: ula=$ula mtu=${state.fipsMtu}")
        if (ula.isNotEmpty()) {
            MycoVpnService.start(this, ula, state.fipsMtu)
        } else {
            Toast.makeText(this, "Mesh address not ready yet", Toast.LENGTH_SHORT).show()
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleDeepLink(intent)
    }

    // --- BLE toggle (remembered) ---

    private fun setBleEnabled(enabled: Boolean) {
        prefs.edit().putBoolean(PREF_BLE, enabled).apply()
        if (enabled) {
            if (bleCorePermsGranted()) BleService.start(this) else requestBlePermissionsIfNeeded()
        } else {
            BleService.stop(this)
        }
    }

    // --- nsite launching ---

    /** The intent that opens an nsite as its own fullscreen task (one per host). */
    private fun nsiteIntent(hostLabel: String, title: String): Intent =
        Intent(this, NsiteActivity::class.java).apply {
            action = Intent.ACTION_VIEW
            data = NsiteActivity.documentUri(hostLabel)
            putExtra(NsiteActivity.EXTRA_HOST, hostLabel)
            putExtra(NsiteActivity.EXTRA_TITLE, title)
            addFlags(Intent.FLAG_ACTIVITY_NEW_DOCUMENT)
        }

    private fun launchNsite(hostLabel: String, title: String) {
        startActivity(nsiteIntent(hostLabel, title))
    }

    /** Pin an nsite to the home screen as an app-like shortcut (favicon + title). */
    private fun pinToHomeScreen(hostLabel: String, title: String) {
        val sm = getSystemService(ShortcutManager::class.java)
        if (sm == null || !sm.isRequestPinShortcutSupported) {
            Toast.makeText(this, "Home-screen pinning isn't supported here", Toast.LENGTH_SHORT).show()
            return
        }
        Thread {
            val bmp = NsiteIcons.fetch(MycoCore.client(this), "$hostLabel.nsite")
            val icon = if (bmp != null) {
                Icon.createWithBitmap(bmp)
            } else {
                Icon.createWithResource(this, R.mipmap.ic_launcher)
            }
            val shortcut = ShortcutInfo.Builder(this, "nsite:$hostLabel")
                .setShortLabel(title.ifEmpty { "nsite" })
                .setLongLabel(title.ifEmpty { hostLabel })
                .setIcon(icon)
                .setIntent(nsiteIntent(hostLabel, title))
                .build()
            runOnUiThread { sm.requestPinShortcut(shortcut, null) }
        }.start()
    }

    // --- share QR: scan + deep-link ---

    private fun startScan() {
        scanLauncher.launch(
            ScanOptions()
                .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                .setPrompt("Scan a Myco share QR")
                .setBeepEnabled(false)
                .setOrientationLocked(true)
                .setCaptureActivity(PortraitCaptureActivity::class.java),
        )
    }

    /** A scanned QR is either a `myco://share/…` payload or a raw nsite link. */
    private fun handleScannedText(text: String) {
        val info = NsiteShare.parseShareUri(text)
        if (info != null) {
            openSharedNsite(info)
        } else {
            // Fall back to treating it as a pasteable nsite link.
            core.dispatch(NativeActions.openNsite(text))
            Toast.makeText(this, "Opening nsite…", Toast.LENGTH_SHORT).show()
        }
    }

    private fun handleDeepLink(intent: Intent?) {
        val data = intent?.data ?: return
        if (data.scheme != NsiteShare.SCHEME) return
        NsiteShare.parseShareUri(data.toString())?.let { openSharedNsite(it) }
    }

    /**
     * Open a shared nsite: add the sharer to your Circle, kick off its sync, and
     * open the fullscreen view. The sharer's device becomes a paired peer we pull
     * from over the mesh (holder = their npub); the public IP source is the fallback.
     */
    private fun openSharedNsite(info: NsiteShare.ShareInfo) {
        if (info.npub.isNotEmpty()) {
            // Scanning someone's share QR adds them to your Circle (the contact list
            // of peers whose devices we pull nsites from).
            core.dispatch(NativeActions.addToCircle(info.npub, info.name))
        }
        core.dispatch(NativeActions.openNsite(info.nsiteHost, holder = info.npub))
        launchNsite(info.nsiteHost, info.name)
        if (info.npub.isNotEmpty()) {
            Toast.makeText(this, "${info.name} added to your Circle — pulling over the mesh", Toast.LENGTH_SHORT).show()
        }
    }

    // --- permissions ---

    private fun requestBlePermissionsIfNeeded() {
        val needed = blePermissions().filter {
            ContextCompat.checkSelfPermission(this, it) != PackageManager.PERMISSION_GRANTED
        }
        if (needed.isNotEmpty()) permLauncher.launch(needed.toTypedArray())
    }

    /** The BLE radio's core permissions are granted (notifications are separate). */
    private fun bleCorePermsGranted(): Boolean = bleCorePermissions().all {
        ContextCompat.checkSelfPermission(this, it) == PackageManager.PERMISSION_GRANTED
    }

    private fun bleCorePermissions(): List<String> =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            listOf(
                Manifest.permission.BLUETOOTH_SCAN,
                Manifest.permission.BLUETOOTH_ADVERTISE,
                Manifest.permission.BLUETOOTH_CONNECT,
            )
        } else {
            listOf(Manifest.permission.ACCESS_FINE_LOCATION)
        }

    private fun blePermissions(): List<String> = buildList {
        addAll(bleCorePermissions())
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            add(Manifest.permission.POST_NOTIFICATIONS)
        }
    }

    private companion object {
        const val PREF_BLE = "ble_enabled"
        const val PREF_MESH = "mesh_enabled"
    }
}
