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
import android.os.SystemClock
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.unit.dp
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.result.contract.ActivityResultContracts.RequestMultiplePermissions
import androidx.core.content.ContextCompat
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import androidx.lifecycle.lifecycleScope
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
import app.myco.share.MycoLink
import app.myco.share.NsiteShare
import app.myco.share.PendingDeepLinks
import app.myco.ui.MycoApp
import app.myco.ui.intro.IntroMode
import app.myco.ui.FirstRunNameDialog
import app.myco.ui.intro.IntroScreen
import app.myco.ui.theme.MycoTheme
import app.myco.vpn.MycoVpnService
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

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

    /** Hosts with a live [watchPendingLink] coroutine, so resumes don't stack them. */
    private val pendingWatchers = mutableSetOf<String>()

    private val permLauncher = registerForActivityResult(RequestMultiplePermissions()) {
        // BLE is enabled by default / remembered; (re)start it once perms land.
        if (prefs.getBoolean(PREF_BLE, true) && bleCorePermsGranted()) {
            BleService.start(this)
        }
        // The Wi-Fi Aware lane is ON by default — it is a peering transport, and
        // a lane the user never turned on is a lane that silently never carries
        // anyone. (Re)start it if its perms just landed.
        if (prefs.getBoolean(PREF_AWARE, true) && AwareRadio.isSupported(this) && awarePermsGranted()) {
            AwareService.start(this)
        }
    }

    private val vpnConsentLauncher =
        registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            if (result.resultCode == RESULT_OK) {
                startMeshNow()
            } else {
                // Consent declined: without the VPN slot no mesh traffic can
                // flow, so don't pretend — persist the mesh off. (The Settings
                // warning card explains and offers to retry.)
                prefs.edit().putBoolean(PREF_MESH, false).apply()
                Toast.makeText(
                    this,
                    "VPN permission declined — mesh disabled",
                    Toast.LENGTH_LONG,
                ).show()
            }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        // Black splash (Myco mark) until Compose draws its first frame; must be
        // installed before super.onCreate so the system hands the window over.
        installSplashScreen()
        super.onCreate(savedInstanceState)
        // Draw edge-to-edge with transparent system bars on every API level. The
        // default auto style follows Android's light/dark configuration, keeping
        // system icons legible when the AMOLED scheme is active.
        enableEdgeToEdge()
        core = MycoCore.client(this)
        // Restore the mesh-only (no IP fallback) preference into the core.
        core.dispatch(NativeActions.setOfflineOnly(prefs.getBoolean(PREF_OFFLINE_ONLY, false)))
        // (Device name is asserted in onResume, which also covers identity not yet
        // being ready at this point.)

        // Nothing starts and nothing is asked for until the intro has been seen
        // once. On a cold install this block would otherwise run before the
        // first frame: LAN browse up, the Bluetooth and Wi-Fi Aware permission
        // dialogs stacked, and the system's "Myco wants to set up a VPN
        // connection" prompt on top of them — four system dialogs over a splash
        // animation, before the app has said what it is. Every one of them now
        // arrives after the intro, which is the first thing that explains
        // anything. Returning launches are unchanged: the flag is set, so this
        // runs here exactly as it always did.
        if (prefs.getBoolean(PREF_INTRO_SEEN, false)) startEnabledLanes()

        setContent {
            MycoTheme {
                // The intro is an overlay, not a nav destination: the app is
                // composed and laid out underneath it from the first frame. The
                // pupil is a hole in that overlay, so the app is what shows
                // through it — live, from the moment it opens — and the dive is
                // that hole growing until there is no overlay left to see. A
                // nav destination would have had nothing behind it to reveal.
                // The full sequence is a first-launch event; after that the
                // intro is only the dive, so it never stands between someone
                // and the app they opened. Reset it from Dev.
                val introMode = remember {
                    if (prefs.getBoolean(PREF_INTRO_SEEN, false)) {
                        IntroMode.Returning
                    } else {
                        IntroMode.FirstRun
                    }
                }
                var introShowing by rememberSaveable { mutableStateOf(true) }
                // How frosted the pupil is. The intro washes the hole itself;
                // this blurs what shows through it by the same amount, so
                // early on you can see there is something behind the mark
                // without being able to read it. Modifier.blur is a no-op
                // below API 31, where the wash carries it alone.
                var frost by remember { mutableFloatStateOf(if (introShowing) 1f else 0f) }
                // The name question, asked exactly once. Raised here rather than
                // only from the intro's completion so an upgrade from a build
                // that never asked still gets it on its next launch, with the
                // intro already behind it.
                var askName by rememberSaveable {
                    mutableStateOf(
                        !prefs.getBoolean(PREF_NAME_CHOSEN, false) &&
                            prefs.getBoolean(PREF_INTRO_SEEN, false),
                    )
                }

                Box(Modifier.fillMaxSize()) {
                    Box(Modifier.blur(14.dp * frost)) {
                        MycoApp(
                            client = core,
                            onBleToggle = { enabled -> setBleEnabled(enabled) },
                            wifiAwareSupported = AwareRadio.isSupported(this@MainActivity),
                            onWifiAwareToggle = { enabled -> setWifiAwareEnabled(enabled) },
                            onLaunchNsite = { hostLabel, title -> launchNsite(hostLabel, title) },
                            onPinToHome = { hostLabel, title -> pinToHomeScreen(hostLabel, title) },
                            onScanned = { text -> handleScannedText(text) },
                            initialMeshEnabled = prefs.getBoolean(PREF_MESH, true),
                            onMeshToggle = { enabled -> setMeshEnabled(enabled) },
                            onOfflineOnlyToggle = { enabled -> setOfflineOnly(enabled) },
                            initialDeveloperMode = prefs.getBoolean(PREF_DEV, BuildConfig.DEBUG),
                            onDeveloperModeToggle = { enabled -> prefs.edit().putBoolean(PREF_DEV, enabled).apply() },
                            initialExitProxy = prefs.getString(PREF_EXIT_PROXY, "").orEmpty(),
                            onExitProxyChange = { spec -> setExitProxy(spec) },
                            onReplayIntro = {
                                prefs.edit().putBoolean(PREF_INTRO_SEEN, false).apply()
                            },
                        )
                    }

                    // Sits above the app and below nothing: the intro is gone
                    // by the time this can show.
                    if (askName && !introShowing) {
                        // Read once, not on every keystroke — state() crosses
                        // JNI and takes the core's locks.
                        val npub = remember { core.state().ownNpub }
                        FirstRunNameDialog(ownNpub = npub) { picked ->
                            DeviceName.set(this@MainActivity, picked)
                            core.dispatch(NativeActions.setDeviceName(picked))
                            prefs.edit().putBoolean(PREF_NAME_CHOSEN, true).apply()
                            askName = false
                            // Deferred from the intro on a first run; a no-op
                            // when the lanes are already up.
                            startEnabledLanes()
                        }
                    }

                    if (introShowing) {
                        IntroScreen(
                            mode = introMode,
                            onFrost = { frost = it },
                            onFinished = {
                                introShowing = false
                                val firstRun = !prefs.getBoolean(PREF_INTRO_SEEN, false)
                                prefs.edit().putBoolean(PREF_INTRO_SEEN, true).apply()
                                // First run: onCreate deliberately started
                                // nothing. Ask the name before the radios come
                                // up rather than after — the permission dialogs
                                // would sit on top of this one, and the name is
                                // what every later pair request carries, so it
                                // should be settled before anything can send
                                // one. A replayed intro has already answered it
                                // and falls through to starting the lanes,
                                // which is a no-op there since every call in
                                // startEnabledLanes is idempotent.
                                if (firstRun && !prefs.getBoolean(PREF_NAME_CHOSEN, false)) {
                                    askName = true
                                } else if (firstRun) {
                                    startEnabledLanes()
                                }
                            },
                        )
                    }
                }
            }
        }

        handleDeepLink(intent)
    }

    // --- mesh adapter (app-owned TUN) ---

    /**
     * Bring up every lane the user has left enabled, and ask for whatever
     * permission each one still needs.
     *
     * Called from `onCreate` on every launch after the first, and from the
     * intro's completion on the first — see the gate at its `onCreate` call
     * site for why. Idempotent: each service's `start` is, `ApRadio` is
     * process-wide, and `startNode` is a no-op on a running node.
     */
    private fun startEnabledLanes() {
        // The `!FIPS` AP lane: watch Wi-Fi and browse the LAN for fips-node
        // mDNS adverts, feeding them to the node (Dev panel shows results).
        // Passive and permissionless; process-wide, so idempotent across
        // Activity recreation.
        ApRadio.ensureStarted(this)

        // BLE on by default, and remembered thereafter.
        if (prefs.getBoolean(PREF_BLE, true)) {
            if (bleCorePermsGranted()) BleService.start(this) else requestBlePermissionsIfNeeded()
        }

        // Wi-Fi Aware is ON by default; resume it unless the user turned it off,
        // and only where the hardware supports it.
        if (prefs.getBoolean(PREF_AWARE, true) && AwareRadio.isSupported(this)) {
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
    }

    /**
     * The mesh master switch. Takes the node **and the BLE radio** with it.
     *
     * The radio has to follow, and this is the one place it does. The BLE
     * byte-bridge hands its scan and accept receivers to whichever transport
     * asks first, and never gets them back — so a node rebuilt underneath a
     * surviving radio comes up with a scanner that receives nothing and an
     * acceptor that accepts nothing: BLE looks alive (advertising, the radio
     * still scanning) while no advert ever reaches the new node. Rebuilding the
     * radio alongside the node gives the new transport a fresh bridge to take.
     *
     * This does not contradict [BleService]'s rule that starting a radio must
     * never bounce the node — that is about the *radio* toggle. Here the node is
     * going down regardless; the radio is following it, not driving it.
     */
    private fun setMeshEnabled(enabled: Boolean) {
        prefs.edit().putBoolean(PREF_MESH, enabled).apply()
        val bleOn = prefs.getBoolean(PREF_BLE, true)
        if (enabled) {
            // Node follows the master switch (radio toggles only gate radios).
            core.dispatch(NativeActions.startNode())
            startBleWhenNodeUp()
            // Android requires user consent before any app can run a VPN.
            val consent = VpnService.prepare(this)
            android.util.Log.i("MycoVpn", "setMeshEnabled(true): consent needed=${consent != null}")
            if (consent != null) vpnConsentLauncher.launch(consent) else startMeshNow()
        } else {
            MycoVpnService.stop(this)
            // Stop the node first: its drain wants to get a shutdown Disconnect
            // out over whatever links are still up, including BLE.
            core.dispatch(NativeActions.stopNode())
            if (bleOn) BleService.stop(this)
        }
    }

    /**
     * Start the BLE radio, but not before the node is actually running.
     *
     * The order is load-bearing. The byte-bridge's scan and accept receivers are
     * single-take: whichever transport resolves the bridge first keeps them for
     * good. Creating the radio while the previous node is still draining hands
     * the fresh bridge to the *dying* transport, and the node that replaces it
     * comes up deaf — advertising normally, scanning normally, and receiving
     * nothing. Waiting for `nodeRunning` means the new transport is the one that
     * takes them.
     *
     * Same retry budget as [startMeshNow], and the same reasoning: a stop that
     * has to drain first can hold the start back for a couple of seconds.
     */
    private fun startBleWhenNodeUp(attempt: Int = 0) {
        if (!prefs.getBoolean(PREF_BLE, true)) return
        if (!bleCorePermsGranted()) {
            requestBlePermissionsIfNeeded()
            return
        }
        // Out of retries: start anyway rather than leaving BLE off entirely. A
        // bridge that arrives late is picked up in place — the running scanner
        // re-resolves the slot and takes the new receivers.
        if (core.state().nodeRunning || attempt >= MESH_START_RETRIES) {
            BleService.start(this)
        } else {
            window.decorView.postDelayed({ startBleWhenNodeUp(attempt + 1) }, MESH_START_RETRY_MS)
        }
    }

    /**
     * Set (or clear) the mesh **exit proxy** — a `[fd00::exit]:port` HTTP proxy on
     * a mesh node. Persisted, then applied by re-establishing the VPN so the new
     * proxy takes effect (the service re-configures in place when the config
     * changes). Empty string turns exit mode off (back to mesh-only routing).
     */
    private fun setExitProxy(spec: String) {
        prefs.edit().putString(PREF_EXIT_PROXY, spec.trim()).apply()
        // Only re-establish if the mesh is currently up; otherwise the new value is
        // picked up the next time the mesh starts.
        if (prefs.getBoolean(PREF_MESH, true)) startMeshNow()
    }

    /** Toggle mesh-only (no IP fallback); persisted + applied to the core. */
    private fun setOfflineOnly(enabled: Boolean) {
        prefs.edit().putBoolean(PREF_OFFLINE_ONLY, enabled).apply()
        core.dispatch(NativeActions.setOfflineOnly(enabled))
    }

    private fun startMeshNow(attempt: Int = 0) {
        val state = core.state()
        val ula = state.fipsIpv6
        android.util.Log.i("MycoVpn", "startMeshNow: ula=$ula mtu=${state.fipsMtu} attempt=$attempt")
        if (ula.isNotEmpty()) {
            MycoVpnService.start(this, ula, state.fipsMtu, prefs.getString(PREF_EXIT_PROXY, "").orEmpty())
        } else if (attempt < MESH_START_RETRIES) {
            // The node is still coming up (common right after the VPN consent
            // dialog — e.g. when Myco just reclaimed the slot from another VPN
            // app). Bailing here used to leave the mesh silently down; retry
            // until the node has published its address.
            window.decorView.postDelayed({ startMeshNow(attempt + 1) }, MESH_START_RETRY_MS)
        } else {
            Toast.makeText(this, "Mesh address not ready — try toggling the mesh off and on", Toast.LENGTH_LONG).show()
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
        // Deep links followed before the app existed (possibly in a previous process)
        // get their chance every time Myco comes back to the foreground.
        reconcilePendingLinks()
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

    /** The intent that opens an nsite as its own fullscreen task (one per host),
     *  at [path] inside it (the root unless a deep link said otherwise). */
    private fun nsiteIntent(hostLabel: String, title: String, path: String = "/"): Intent =
        Intent(this, NsiteActivity::class.java).apply {
            action = Intent.ACTION_VIEW
            // Host-only, so a deep link re-surfaces the app's existing Recents card
            // instead of opening a second one per route.
            data = NsiteActivity.documentUri(hostLabel)
            putExtra(NsiteActivity.EXTRA_HOST, hostLabel)
            putExtra(NsiteActivity.EXTRA_TITLE, title)
            putExtra(NsiteActivity.EXTRA_PATH, path)
            addFlags(Intent.FLAG_ACTIVITY_NEW_DOCUMENT)
        }

    /**
     * Open an nsite. With no explicit [path], any deep link still waiting on this app
     * is spent here — so tapping it yourself in the Apps grid lands on the link you
     * followed days ago just as surely as Myco opening it for you would have.
     */
    private fun launchNsite(hostLabel: String, title: String, path: String? = null) {
        val target = path ?: PendingDeepLinks.take(this, hostLabel) ?: "/"
        startActivity(nsiteIntent(hostLabel, title, target))
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
        MycoLink.parseAppLink(text)?.let { link ->
            openAppLink(link)
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
     * Follow a `myco://app/<host>/<path>` deep link.
     *
     * Installed already → open it, there, now. Not installed → start retrieving it and
     * **remember the path**: the app may arrive in five seconds or next week, and
     * either way its first open belongs to the link that asked for it. Nothing about
     * the wait is shown as a modal — the app appears in the Apps grid with its
     * download ring, same as any other incoming app.
     *
     * No holder is passed because the link carries none by design (see [MycoLink]);
     * `open_site` falls through every Circle peer and then the public source anyway.
     */
    private fun openAppLink(link: MycoLink.AppLink) {
        val site = core.state().sites.firstOrNull { it.host == link.host }
        if (site != null && site.state == "ready") {
            launchNsite(link.host, site.title, link.path)
            PendingDeepLinks.remove(this, link.host)
            return
        }
        PendingDeepLinks.put(this, link.host, link.path)
        core.dispatch(NativeActions.openNsite(link.host))
        Toast.makeText(this, "Getting the app — it opens when it lands", Toast.LENGTH_LONG).show()
        watchPendingLink(link.host)
    }

    /**
     * Open the apps whose deep links have been waiting, and retry the ones that
     * haven't arrived.
     *
     * Runs on every resume, which is what makes the wait survivable: the watcher
     * coroutine started at tap time dies with the process, but this doesn't — a link
     * followed before a reboot still opens the first time Myco comes back up with the
     * app in hand. An `unreachable` site is re-dispatched rather than dropped: it means
     * nobody in range had it *then*, and someone who does may have walked in since.
     */
    private fun reconcilePendingLinks() {
        val hosts = PendingDeepLinks.hosts(this)
        if (hosts.isEmpty()) return
        lifecycleScope.launch {
            val sites = withContext(Dispatchers.IO) { core.state() }.sites
            for (host in hosts) {
                val path = PendingDeepLinks.peek(this@MainActivity, host) ?: continue
                val site = sites.firstOrNull { it.host == host }
                when (site?.state) {
                    "ready" -> {
                        PendingDeepLinks.remove(this@MainActivity, host)
                        launchNsite(host, site.title, path)
                    }
                    // Still coming — the watcher below opens it the moment it lands.
                    "syncing" -> watchPendingLink(host)
                    // Never started, or nobody had it last time. Ask again.
                    else -> {
                        core.dispatch(NativeActions.openNsite(host))
                        watchPendingLink(host)
                    }
                }
            }
        }
    }

    /**
     * Watch one pending app while Myco is open, so a sync that finishes in the next few
     * seconds — the common case, a peer right there in the room — opens the link
     * immediately instead of waiting for the next resume. Bounded and de-duplicated;
     * the durable half of the promise is [reconcilePendingLinks].
     */
    private fun watchPendingLink(hostLabel: String) {
        if (!pendingWatchers.add(hostLabel)) return
        lifecycleScope.launch {
            try {
                val deadline = SystemClock.elapsedRealtime() + PENDING_WATCH_MS
                while (SystemClock.elapsedRealtime() < deadline) {
                    delay(PENDING_WATCH_POLL_MS)
                    val path = PendingDeepLinks.peek(this@MainActivity, hostLabel) ?: return@launch
                    val site = withContext(Dispatchers.IO) { core.state() }
                        .sites.firstOrNull { it.host == hostLabel }
                    if (site != null && site.state == "ready") {
                        PendingDeepLinks.remove(this@MainActivity, hostLabel)
                        launchNsite(hostLabel, site.title, path)
                        return@launch
                    }
                }
            } finally {
                pendingWatchers.remove(hostLabel)
            }
        }
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
        offerHomeScreenWhenReady(info.nsiteHost)
    }

    /**
     * Offer to pin an app a peer just shared, once its download actually finishes.
     *
     * Deliberately **not** at scan time: a site that is still syncing may fail or
     * be unreachable, and a home-screen icon for something that never arrived is
     * worse than no icon. Waiting for `ready` means the offer only appears for an
     * app that works.
     *
     * The offer is the platform's own pin dialog — `requestPinShortcut` already
     * asks "add to home screen?" with a cancel — rather than a bespoke prompt
     * stacked in front of it, which would be two dialogs for one decision.
     *
     * Offered **once per site, ever** (remembered in prefs): a peer re-sharing an
     * app you already declined must not re-ask. Declining the system dialog is
     * indistinguishable from accepting it — the API reports neither — so "asked"
     * is what we record, not "added".
     */
    private fun offerHomeScreenWhenReady(hostLabel: String) {
        if (hostLabel.isEmpty()) return
        val asked = prefs.getStringSet(PREF_HOME_OFFERED, emptySet()).orEmpty()
        if (hostLabel in asked) return

        lifecycleScope.launch {
            // Give up rather than watch forever: a sync that has not landed in
            // this long is a failure, and the user has moved on either way.
            val deadline = SystemClock.elapsedRealtime() + HOME_OFFER_TIMEOUT_MS
            while (SystemClock.elapsedRealtime() < deadline) {
                val site = withContext(Dispatchers.IO) { core.state() }
                    .sites.firstOrNull { it.host == hostLabel }
                if (site != null && site.state == "ready") {
                    prefs.edit()
                        .putStringSet(PREF_HOME_OFFERED, asked + hostLabel)
                        .apply()
                    pinToHomeScreen(hostLabel, site.title)
                    return@launch
                }
                // "unreachable" is terminal for this attempt; stop asking.
                if (site != null && site.state == "unreachable") return@launch
                delay(HOME_OFFER_POLL_MS)
            }
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

    // Non-private: the radio services read PREF_MESH to gate node startup.
    companion object {
        /** startMeshNow: how many 500ms retries to wait for the node's mesh address. */
        private const val MESH_START_RETRIES = 20
        private const val MESH_START_RETRY_MS = 500L

        const val PREF_BLE = "ble_enabled"
        const val PREF_AWARE = "wifi_aware_enabled"
        const val PREF_MESH = "mesh_enabled"
        const val PREF_OFFLINE_ONLY = "offline_only"
        const val PREF_DEV = "developer_mode"

        /** Hosts we have already offered to pin, so a re-share never re-asks. */
        private const val PREF_HOME_OFFERED = "home_screen_offered"

        /** How long to wait for a peer-shared app to finish downloading before
         *  giving up on offering it — a sync that has not landed by now failed. */
        private const val HOME_OFFER_TIMEOUT_MS = 3 * 60 * 1000L
        private const val HOME_OFFER_POLL_MS = 1500L

        /** How long a foreground watcher waits for a deep-linked app to land before
         *  handing the wait back to the durable store (opened on a later resume). */
        private const val PENDING_WATCH_MS = 3 * 60 * 1000L
        private const val PENDING_WATCH_POLL_MS = 1000L
        const val PREF_EXIT_PROXY = "exit_proxy"
        /** Set once the intro has played all the way through. */
        const val PREF_INTRO_SEEN = "intro_seen"

        /** Set once the first-run name question has been answered. Separate from
         *  [PREF_INTRO_SEEN] so replaying the intro doesn't re-ask it, and so an
         *  upgrade from a build that never asked still gets the question once. */
        private const val PREF_NAME_CHOSEN = "name_chosen"
    }
}
