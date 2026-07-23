package app.myco

import android.Manifest
import android.content.ComponentName
import android.content.Intent
import android.provider.Settings
import android.content.pm.PackageManager
import android.content.pm.ShortcutInfo
import android.content.pm.ShortcutManager
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.drawable.Icon
import android.net.VpnService
import android.nfc.NfcAdapter
import android.nfc.Tag
import android.nfc.cardemulation.CardEmulation
import android.nfc.tech.Ndef
import android.os.Build
import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.result.contract.ActivityResultContracts.RequestMultiplePermissions
import androidx.core.content.ContextCompat
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import app.myco.ap.ApRadio
import app.myco.aware.AwareRadio
import app.myco.aware.AwareService
import app.myco.ble.BleService
import app.myco.BuildConfig
import app.myco.core.AppCoreClient
import app.myco.core.MycoCore
import app.myco.core.NativeActions
import app.myco.nfc.NfcReader
import app.myco.nfc.PairPresent
import app.myco.share.DeviceName
import app.myco.share.NsiteShare
import app.myco.ui.MycoApp
import app.myco.ui.theme.MycoTheme
import app.myco.vpn.MycoVpnService

/**
 * Developer-UI entry point: device identity, node status, BLE diagnostics, and
 * the nsite list (paste a link / scan a QR / open / share / pin to home). The
 * node + radio are process-singletons ([MycoCore]); the BLE toggle starts/stops
 * the foreground [BleService] and its choice is remembered across restarts.
 */
class MainActivity : ComponentActivity() {
    private lateinit var core: AppCoreClient
    private val prefs by lazy { getSharedPreferences("myco_prefs", MODE_PRIVATE) }
    private val nfcAdapter by lazy { NfcAdapter.getDefaultAdapter(this) }

    private val permLauncher = registerForActivityResult(RequestMultiplePermissions()) {
        // BLE is enabled by default / remembered; (re)start it once perms land.
        if (prefs.getBoolean(PREF_BLE, true) && bleCorePermsGranted()) {
            BleService.start(this)
        }
        // The Wi-Fi Aware lane is opt-in (default off); (re)start it too if the
        // user had it on and its perms just landed.
        if (prefs.getBoolean(PREF_AWARE, false) && AwareRadio.isSupported(this) && awarePermsGranted()) {
            AwareService.start(this)
        }
    }

    private val vpnConsentLauncher =
        registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            if (result.resultCode == RESULT_OK) startMeshNow()
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        // Black splash (Myco mark) until Compose draws its first frame; must be
        // installed before super.onCreate so the system hands the window over.
        installSplashScreen()
        super.onCreate(savedInstanceState)
        // Draw edge-to-edge with transparent system bars on every API level. The
        // platform forces this on Android 15+, but pre-15 devices (e.g. Samsung on
        // One UI 6 / Android 14) otherwise keep opaque bars whose default color
        // differs from our white background — showing up as big top/bottom borders.
        // The Compose Scaffold applies the system-bar insets, so content stays clear.
        //
        // Force the *light* bar style (dark icons) on both bars: the shell is always
        // a white background, so the default `auto` style would draw white icons in
        // system dark mode — invisible white-on-white (seen on Pixel).
        val barStyle = SystemBarStyle.light(android.graphics.Color.TRANSPARENT, android.graphics.Color.TRANSPARENT)
        enableEdgeToEdge(statusBarStyle = barStyle, navigationBarStyle = barStyle)
        core = MycoCore.client(this)
        // Restore the mesh-only (no IP fallback) preference into the core.
        core.dispatch(NativeActions.setOfflineOnly(prefs.getBoolean(PREF_OFFLINE_ONLY, false)))
        // (Device name is asserted in onResume, which also covers identity not yet
        // being ready at this point.)

        // The `!FIPS` AP lane: watch Wi-Fi and browse the LAN for fips-node
        // mDNS adverts, feeding them to the node (Dev panel shows results).
        // Passive and permissionless; process-wide, so idempotent across
        // Activity recreation.
        ApRadio.ensureStarted(this)

        // BLE on by default, and remembered thereafter.
        if (prefs.getBoolean(PREF_BLE, true)) {
            if (bleCorePermsGranted()) BleService.start(this) else requestBlePermissionsIfNeeded()
        }

        // Wi-Fi Aware is opt-in (default off); resume it if the user left it on
        // and the hardware supports it.
        if (prefs.getBoolean(PREF_AWARE, false) && AwareRadio.isSupported(this)) {
            if (awarePermsGranted()) AwareService.start(this) else requestAwarePermissionsIfNeeded()
        }

        // The mesh adapter (app-owned TUN) is ON by default — it's how this device
        // reaches the mesh, so it's effectively required. Bring it up at launch,
        // prompting for the one-time VPN consent the first time it's needed.
        // The fips node's lifecycle follows this master "Enable" switch (the
        // radio toggles only gate their radios), so start it here too — the
        // dispatch is idempotent with the radio services' own startNode calls.
        if (prefs.getBoolean(PREF_MESH, true)) {
            core.dispatch(NativeActions.startNode())
            val consent = VpnService.prepare(this)
            if (consent == null) startMeshNow() else vpnConsentLauncher.launch(consent)
        }

        setContent {
            MycoTheme {
                MycoApp(
                    client = core,
                    onBleToggle = { enabled -> setBleEnabled(enabled) },
                    wifiAwareSupported = AwareRadio.isSupported(this),
                    onWifiAwareToggle = { enabled -> setWifiAwareEnabled(enabled) },
                    onLaunchNsite = { hostLabel, title -> launchNsite(hostLabel, title) },
                    onPinToHome = { hostLabel, title -> pinToHomeScreen(hostLabel, title) },
                    onScanned = { text -> handleScannedText(text) },
                    initialMeshEnabled = prefs.getBoolean(PREF_MESH, true),
                    onMeshToggle = { enabled -> setMeshEnabled(enabled) },
                    onOfflineOnlyToggle = { enabled -> setOfflineOnly(enabled) },
                    initialDeveloperMode = prefs.getBoolean(PREF_DEV, BuildConfig.DEBUG),
                    onDeveloperModeToggle = { enabled -> prefs.edit().putBoolean(PREF_DEV, enabled).apply() },
                )
            }
        }

        handleDeepLink(intent)
    }

    // --- mesh adapter (app-owned TUN) ---

    private fun setMeshEnabled(enabled: Boolean) {
        prefs.edit().putBoolean(PREF_MESH, enabled).apply()
        if (enabled) {
            // Node follows the master switch (radio toggles only gate radios).
            core.dispatch(NativeActions.startNode())
            // Android requires user consent before any app can run a VPN.
            val consent = VpnService.prepare(this)
            android.util.Log.i("MycoVpn", "setMeshEnabled(true): consent needed=${consent != null}")
            if (consent != null) vpnConsentLauncher.launch(consent) else startMeshNow()
        } else {
            MycoVpnService.stop(this)
            core.dispatch(NativeActions.stopNode())
        }
    }

    /** Toggle mesh-only (no IP fallback); persisted + applied to the core. */
    private fun setOfflineOnly(enabled: Boolean) {
        prefs.edit().putBoolean(PREF_OFFLINE_ONLY, enabled).apply()
        core.dispatch(NativeActions.setOfflineOnly(enabled))
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

    // --- NFC tap-to-pair (numo-style: we are the card; the other phone reads) ---
    //
    // While the QR/present screen is open we claim foreground HCE for the standard
    // NDEF AID and emulate a URI tag (myco://pair/…). The *other* phone needs no
    // special mode: its OS tag dispatch reads the URI and delivers it to us as an
    // NDEF_DISCOVERED intent (see the manifest filter) → handleScannedText.

    override fun onResume() {
        super.onResume()
        // Presenting is owned by the Circle screen (it's the only place we emulate a
        // card). Here we just (re)apply the current presenting state — re-claiming
        // the foreground HCE service after a background→foreground while on Circle.
        PairPresent.onChanged = { runOnUiThread { updateNfcPresent() } }
        updateNfcPresent()
        // Foreground reader mode is owned by the add-app sheet (a modal window where
        // passive NDEF dispatch can't reach us). Re-apply its current state here.
        NfcReader.onChanged = { runOnUiThread { updateNfcReader() } }
        updateNfcReader()
        // (Re)assert our memorable name into the core so pair events carry it. Done
        // here (not just onCreate) in case the device identity wasn't ready yet at
        // first launch; set_device_name is idempotent.
        core.state().ownNpub.takeIf { it.isNotEmpty() }?.let {
            core.dispatch(NativeActions.setDeviceName(DeviceName.current(this, it)))
        }
    }

    override fun onPause() {
        super.onPause()
        // Drop the callbacks so the process-global NFC state doesn't pin this
        // Activity while backgrounded (they're re-set on the next onResume).
        PairPresent.onChanged = null
        NfcReader.onChanged = null
        nfcAdapter?.let { adapter ->
            runCatching { CardEmulation.getInstance(adapter).unsetPreferredService(this) }
            runCatching { adapter.disableReaderMode(this) }
        }
    }

    /** Claim (or release) foreground HCE for our NDEF service while presenting.
     *  We deliberately do NOT suppress polling: leaving the default poll+listen
     *  loop on means a presenting phone also *reads* the other phone's tag, so two
     *  phones in the pairing flow pair symmetrically on a single bump. */
    private fun updateNfcPresent() {
        val adapter = nfcAdapter ?: return
        val cardEmu = runCatching { CardEmulation.getInstance(adapter) }.getOrNull() ?: return
        val svc = ComponentName(this, "app.myco.nfc.PairHostApduService")
        runCatching {
            if (PairPresent.presenting) cardEmu.setPreferredService(this, svc)
            else cardEmu.unsetPreferredService(this)
        }.onFailure { android.util.Log.w("MycoNfc", "nfc present setup failed", it) }
        android.util.Log.i("MycoNfc", "present=${PairPresent.presenting}")
    }

    /** Claim (or release) foreground NFC reader mode while the add-app sheet is up.
     *  Reader mode reads a tapped peer's emulated tag in-foreground, which a modal
     *  sheet's separate window otherwise misses (passive dispatch skips it). */
    private fun updateNfcReader() {
        val adapter = nfcAdapter ?: return
        runCatching {
            if (NfcReader.active) {
                val flags = NfcAdapter.FLAG_READER_NFC_A or
                    NfcAdapter.FLAG_READER_NFC_B or
                    NfcAdapter.FLAG_READER_NO_PLATFORM_SOUNDS
                adapter.enableReaderMode(this, { tag -> handleNfcTag(tag) }, flags, null)
            } else {
                adapter.disableReaderMode(this)
            }
        }.onFailure { android.util.Log.w("MycoNfc", "nfc reader setup failed", it) }
        android.util.Log.i("MycoNfc", "reading=${NfcReader.active}")
    }

    /** Read the NDEF URI record off a tapped tag (the peer's emulated share/pair
     *  tag) and route it through the same handler as a scan. Runs on a binder
     *  thread — hop to the main thread to touch the core / show toasts. */
    private fun handleNfcTag(tag: Tag) {
        val ndef = Ndef.get(tag) ?: return
        val uri = runCatching {
            val msg = ndef.cachedNdefMessage ?: run {
                ndef.connect()
                try { ndef.ndefMessage } finally { runCatching { ndef.close() } }
            }
            msg?.records?.firstNotNullOfOrNull { it.toUri() }
        }.getOrNull() ?: return
        // Only act on our own scheme. Reader mode reads *any* tapped NDEF tag, so
        // without this an unrelated URL tag would fall through to openNsite(). Raw
        // nsite links are entered via QR/paste, never NFC.
        if (uri.scheme != NsiteShare.SCHEME) return
        runOnUiThread { handleScannedText(uri.toString()) }
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

    // --- Wi-Fi Aware toggle (remembered) ---

    private fun setWifiAwareEnabled(enabled: Boolean) {
        prefs.edit().putBoolean(PREF_AWARE, enabled).apply()
        if (enabled) {
            if (!awarePermsGranted()) {
                requestAwarePermissionsIfNeeded()
                return
            }
            AwareService.start(this)
            // Wi-Fi Aware needs Wi-Fi on, and an app cannot toggle Wi-Fi since
            // API 29 — so if it isn't available right now (Wi-Fi off), pop the
            // system Wi-Fi panel. The armed radio attaches once Wi-Fi comes on.
            if (!AwareRadio.isAvailable(this)) openWifiPanel()
        } else {
            AwareService.stop(this)
        }
    }

    /** Slide-up system Wi-Fi panel (API 29+) so the user can turn Wi-Fi on
     *  without leaving Myco. */
    private fun openWifiPanel() {
        runCatching { startActivity(Intent(Settings.Panel.ACTION_WIFI)) }
            .onFailure { startActivity(Intent(Settings.ACTION_WIFI_SETTINGS)) }
    }

    private fun requestAwarePermissionsIfNeeded() {
        val needed = awarePermissions().filter {
            ContextCompat.checkSelfPermission(this, it) != PackageManager.PERMISSION_GRANTED
        }
        if (needed.isNotEmpty()) permLauncher.launch(needed.toTypedArray())
    }

    private fun awarePermsGranted(): Boolean = awarePermissions().all {
        ContextCompat.checkSelfPermission(this, it) == PackageManager.PERMISSION_GRANTED
    }

    /** NEARBY_WIFI_DEVICES on API 33+; ACCESS_FINE_LOCATION gates Aware on 29–32. */
    private fun awarePermissions(): List<String> =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            listOf(Manifest.permission.NEARBY_WIFI_DEVICES)
        } else {
            listOf(Manifest.permission.ACCESS_FINE_LOCATION)
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
            val bmp = NsiteIcons.fetch(MycoCore.client(this), "$hostLabel.localhost")
            val icon = if (bmp != null) {
                // An *adaptive* bitmap fills the launcher's icon shape; a plain
                // bitmap is treated as legacy and shrunk onto a padded white
                // circle (the "tiny icon" look). Pre-compose the favicon onto a
                // full-bleed canvas so it renders big.
                Icon.createWithAdaptiveBitmap(adaptiveShortcutIcon(bmp))
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

    /**
     * Compose a favicon into a full-bleed adaptive-icon bitmap so the home-screen
     * shortcut renders large instead of a tiny image floating in a white circle.
     * The favicon is scaled to *cover* the whole 108dp layer (edge-to-edge); the
     * launcher then masks it to its shape, trimming only the corners — the same
     * full-bleed look as a normal app icon. White only shows through where the
     * source itself is transparent.
     */
    private fun adaptiveShortcutIcon(src: Bitmap): Bitmap {
        val size = 432 // 108dp @ xxhdpi
        val out = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(out)
        canvas.drawColor(Color.WHITE)
        // Cover: scale by the *smaller* dimension so the image fills the canvas
        // with no bars; any overflow on the long axis is centered and clipped.
        val scale = size / minOf(src.width, src.height).toFloat()
        val w = src.width * scale
        val h = src.height * scale
        val left = (size - w) / 2f
        val top = (size - h) / 2f
        val paint = Paint(Paint.FILTER_BITMAP_FLAG or Paint.ANTI_ALIAS_FLAG)
        canvas.drawBitmap(src, null, RectF(left, top, left + w, top + h), paint)
        return out
    }

    // --- share QR: scan (in-app PairScreen) + deep-link ---

    /** A scanned QR is a `myco://pair/…` pairing code, a `myco://share/…` shared
     *  nsite, or a raw nsite link. */
    private fun handleScannedText(text: String) {
        NsiteShare.parsePairUri(text)?.let { pair ->
            // Mutual pairing: send a request to the scanned device; it pops up there
            // to accept, and only then do both sides add each other.
            core.dispatch(NativeActions.sendPairRequest(pair.npub, pair.name, pair.secret))
            Toast.makeText(this, "Pair request sent to ${pair.name}…", Toast.LENGTH_SHORT).show()
            return
        }
        NsiteShare.parseShareUri(text)?.let { info ->
            openSharedNsite(info)
            return
        }
        // Fall back to treating it as a pasteable nsite link.
        core.dispatch(NativeActions.openNsite(text))
        Toast.makeText(this, "Opening app...", Toast.LENGTH_SHORT).show()
    }

    private fun handleDeepLink(intent: Intent?) {
        val data = intent?.data ?: return
        if (data.scheme != NsiteShare.SCHEME) return
        handleScannedText(data.toString())
    }

    /**
     * Receive a shared nsite: add the sharer to your Circle and kick off its sync.
     * We do **not** open the fullscreen view here — the app appears in the Apps
     * drawer with a download ring (iOS-install style) and opens when tapped. The
     * sharer's device becomes a paired peer we pull from (holder = their npub).
     */
    private fun openSharedNsite(info: NsiteShare.ShareInfo) {
        if (info.npub.isNotEmpty()) {
            // Mutual pairing: request to pair (the sharer accepts on their device);
            // the nsite can still download from them as a holder meanwhile.
            core.dispatch(NativeActions.sendPairRequest(info.npub, info.name, info.secret))
        }
        core.dispatch(NativeActions.openNsite(info.nsiteHost, holder = info.npub))
        val who = info.name.ifEmpty { "a peer" }
        Toast.makeText(this, "Downloading from $who — find it in Apps", Toast.LENGTH_SHORT).show()
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

    // Non-private: the radio services read PREF_MESH to gate node startup.
    companion object {
        const val PREF_BLE = "ble_enabled"
        const val PREF_AWARE = "wifi_aware_enabled"
        const val PREF_MESH = "mesh_enabled"
        const val PREF_OFFLINE_ONLY = "offline_only"
        const val PREF_DEV = "developer_mode"
    }
}
