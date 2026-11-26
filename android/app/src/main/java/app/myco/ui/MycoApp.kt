package app.myco.ui

import android.app.ActivityManager
import android.content.Context
import android.content.pm.PackageManager
import android.content.res.Configuration
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.GridView
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.filled.People
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Terminal
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.NavigationRail
import androidx.compose.material3.NavigationRailItem
import androidx.compose.material3.NavigationRailItemDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.draw.scale
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.navigation.NavDestination.Companion.hierarchy
import androidx.navigation.NavGraph.Companion.findStartDestination
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import app.myco.ble.BleHealth
import app.myco.ui.screens.backendErrors
import androidx.compose.ui.platform.LocalContext
import app.myco.ui.radioWarnings
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.forShell
import app.myco.hotspot.TransferGate
import app.myco.nfc.PairPresent
import app.myco.onboarding.SetupNotice
import app.myco.ui.onboarding.SetupSnackbar
import app.myco.ui.onboarding.SetupSnackbarVisuals
import app.myco.share.DeviceName
import app.myco.ui.screens.AppsScreen
import app.myco.ui.screens.DevScreen
import app.myco.update.rememberSoftwareUpdates
import app.myco.update.SoftwareUpdateBanner
import app.myco.ui.screens.SettingsScreen

import app.myco.ui.theme.StatusAlone
import app.myco.ui.theme.StatusConnected
import app.myco.ui.theme.StatusThin
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

/**
 * The consumer shell: a bottom-nav app over four surfaces — **Apps** (the nsite
 * launcher), **Neighbors** (neighborhoods),
 * **Settings**, and **Dev** (diagnostics).
 * The Rust `AppState` is polled once a second here and handed down to every tab,
 * so all screens share one read: the tabs get its [forShell] projection, and
 * the diagnostics screens the full snapshot through [LocalLiveState].
 */
private data class Tab(val route: String, val label: String, val icon: ImageVector)

private val TABS = listOf(
    Tab("apps", "Apps", Icons.Filled.GridView),
    Tab("neighbors", "Neighbors", Icons.Filled.People),
    Tab("settings", "Settings", Icons.Filled.Settings),
    Tab("dev", "Dev", Icons.Filled.Terminal),
)

@Composable
fun MycoApp(
    client: AppCoreClient,
    onBleToggle: (Boolean) -> Unit,
    wifiAwareSupported: Boolean,
    onWifiAwareToggle: (Boolean) -> Unit,
    /** The LAN lane's mDNS discovery switch, as last persisted. */
    initialLanEnabled: Boolean = true,
    onLanToggle: (Boolean) -> Unit = {},
    onLaunchNsite: (host: String, title: String) -> Unit,
    onLaunchNapplet: (pointer: String, title: String, intentToken: String) -> Unit,
    onPinNappletToHome: (pointer: String, title: String) -> Unit,
    onPinToHome: (host: String, title: String) -> Unit,
    onScanned: (String) -> Unit,
    /** The mesh master switch; owned by the Activity, which the setup popup reports to. */
    meshEnabled: Boolean,
    onMeshToggle: (Boolean) -> Unit,
    /** Settings › Permissions "Fix" on Nearby devices: reopens the setup popup there. */
    onFixNearby: () -> Unit = {},
    /** "Fix" on the mesh connection (Permissions, the VPN warning card). */
    onFixConnection: () -> Unit = {},
    shareMesh: Boolean = false,
    onShareMeshToggle: (Boolean) -> Unit = {},
    /** What the setup popup ended with, to show as a snackbar once. */
    setupNotice: SetupNotice? = null,
    onSetupNoticeShown: () -> Unit = {},
    onOfflineOnlyToggle: (Boolean) -> Unit,
    initialDeveloperMode: Boolean,
    onDeveloperModeToggle: (Boolean) -> Unit,
    initialExitProxy: String = "",
    onExitProxyChange: (String) -> Unit = {},
    /** Clears the intro's "already seen" flag so it plays in full again. */
    onReplayIntro: () -> Unit = {},
) {
    // Two views of one poll. `live` is the full latest snapshot, provided as
    // [LocalLiveState] to the screens that draw per-second diagnostics. `state`
    // is its [forShell] projection, handed to every screen: assigning an equal
    // projection is a no-op under mutableStateOf's structural policy, so polls
    // that only moved RTTs and counters recompose nothing but those readers.
    // Seeded from one plain read, not from `live.value`: reading that here
    // would subscribe this whole composable to every poll.
    val initial = remember { client.state() }
    val live = remember { mutableStateOf(initial) }
    var state by remember { mutableStateOf(initial.forShell()) }
    fun publish(full: AppState) {
        live.value = full
        state = full.forShell()
    }
    val softwareUpdates = rememberSoftwareUpdates(state.offlineOnly)
    val softwareUpdateState by softwareUpdates.state.collectAsState()
    // The 1 Hz loop below outlives any one value of the switch.
    val currentMeshEnabled by rememberUpdatedState(meshEnabled)
    // Developer mode gates the Dev tab; hoisted so toggling it rebuilds the nav bar.
    var developerMode by remember { mutableStateOf(initialDeveloperMode) }
    // Kotlin-owned like developerMode: the LAN browse is an Android NsdManager
    // affair the core never sees, so there is no core state to read it from.
    var lanEnabled by remember { mutableStateOf(initialLanEnabled) }
    // BLE advertiser exhaustion (set by the radio, read here for the Settings badge).
    var bleExhausted by remember { mutableStateOf(BleHealth.advertiserExhausted) }
    val context = LocalContext.current
    // Re-poll the native state each second (every two on a low-RAM device) off
    // the main thread (it crosses JNI into the Rust core). Pure read — does not
    // bump `rev`. Gated on STARTED:
    // with the app backgrounded the UI can't show the result anyway, and the
    // 1Hz JNI wakeup was a measurable battery cost. repeatOnLifecycle cancels
    // the loop on onStop and restarts it (immediate first poll) on onStart.
    // Radio/VPN misconfigurations (VPN slot lost, Bluetooth/Wi-Fi off under an
    // enabled transport) — drives the red dot on the Settings tab.
    var radioAlert by remember { mutableStateOf(false) }
    val lifecycleOwner = androidx.lifecycle.compose.LocalLifecycleOwner.current
    val pollMs = remember { shellPollMs(context) }
    androidx.compose.runtime.LaunchedEffect(Unit) {
        lifecycleOwner.repeatOnLifecycle(androidx.lifecycle.Lifecycle.State.STARTED) {
            while (true) {
                val full = withContext(Dispatchers.IO) { client.state() }
                publish(full)
                bleExhausted = BleHealth.advertiserExhausted
                radioAlert = withContext(Dispatchers.IO) {
                    radioWarnings(context, full, currentMeshEnabled).isNotEmpty()
                }
                delay(pollMs)
            }
        }
    }

    val nav = rememberNavController()
    // The setup popup's parting word, or a one-off notice. A "Settings" tap
    // goes to Settings › Permissions, where everything setup skipped can be
    // fixed — or, for news, to Settings itself.
    val snackbars = remember { SnackbarHostState() }
    var openPermissions by remember { mutableStateOf(false) }
    androidx.compose.runtime.LaunchedEffect(setupNotice) {
        val notice = setupNotice ?: return@LaunchedEffect
        val visuals = SetupSnackbarVisuals(notice)
        val result = snackbars.showSnackbar(visuals)
        if (result == SnackbarResult.ActionPerformed) {
            openPermissions = visuals.opensPermissions
            nav.navigate("settings") {
                popUpTo(nav.graph.findStartDestination().id) { saveState = true }
                launchSingleTop = true
            }
        }
        // Last: clearing it changes this effect's key and cancels it.
        onSetupNoticeShown()
    }
    // Keep the navigation host and all transient surfaces in one app-root layer.
    Box(Modifier.fillMaxSize()) {
        androidx.compose.runtime.CompositionLocalProvider(
            LocalMeshControl provides MeshControl(meshEnabled, onMeshToggle),
            LocalLiveState provides live,
        ) {
    val current by nav.currentBackStackEntryAsState()
    // The full-screen Add surface hides the tab bar.
    val showTabs = current?.destination?.route != "qr"
    val tabs = if (developerMode) TABS else TABS.filterNot { it.route == "dev" }
    val isSelected = { tab: Tab ->
        current?.destination?.hierarchy?.any { it.route == tab.route } == true ||
            // A neighborhood's page belongs to the Neighbors tab.
            (tab.route == "neighbors" && current?.destination?.route?.startsWith("neighborhood/") == true)
    }
    val select = { tab: Tab ->
        nav.navigate(tab.route) {
            popUpTo(nav.graph.findStartDestination().id) { saveState = true }
            launchSingleTop = true
            restoreState = true
        }
    }
    val tabIcon: @Composable (Tab) -> Unit = { tab ->
        val badged = (
            tab.route == "settings" &&
                (bleExhausted || radioAlert || backendErrors(state).isNotEmpty() || softwareUpdateState.availableTag != null)
            ) ||
            (tab.route == "neighbors" &&
                state.neighborhoods.links.any { it.pending.isNotEmpty() })
        if (badged) {
            BadgedBox(badge = { Badge() }) {
                Icon(tab.icon, contentDescription = tab.label)
            }
        } else {
            Icon(tab.icon, contentDescription = tab.label)
        }
    }
    // In landscape the tabs move to a side rail, out of the content's way and
    // within one D-pad press of it. Phones are locked to portrait, so this is
    // Android TV, or a large screen where Android 16 ignores the lock.
    val rail = LocalConfiguration.current.orientation == Configuration.ORIENTATION_LANDSCAPE
    Scaffold(
        topBar = { SoftwareUpdateBanner(softwareUpdates) },
        snackbarHost = { SnackbarHost(snackbars) { SetupSnackbar(it) } },
        bottomBar = {
            if (showTabs && !rail) {
                NavigationBar(containerColor = MaterialTheme.colorScheme.surface) {
                    tabs.forEach { tab ->
                        NavigationBarItem(
                            selected = isSelected(tab),
                            onClick = { select(tab) },
                            icon = { tabIcon(tab) },
                            label = { Text(tab.label) },
                            colors = NavigationBarItemDefaults.colors(
                                selectedIconColor = MaterialTheme.colorScheme.primary,
                                selectedTextColor = MaterialTheme.colorScheme.primary,
                                indicatorColor = MaterialTheme.colorScheme.primaryContainer,
                                unselectedIconColor = MaterialTheme.colorScheme.onSurfaceVariant,
                                unselectedTextColor = MaterialTheme.colorScheme.onSurfaceVariant,
                            ),
                        )
                    }
                }
            }
        },
    ) { padding ->
        Row(Modifier.padding(padding)) {
        if (showTabs && rail) {
            NavigationRail(containerColor = MaterialTheme.colorScheme.surface) {
                tabs.forEach { tab ->
                    NavigationRailItem(
                        selected = isSelected(tab),
                        onClick = { select(tab) },
                        icon = { tabIcon(tab) },
                        label = { Text(tab.label) },
                        colors = NavigationRailItemDefaults.colors(
                            selectedIconColor = MaterialTheme.colorScheme.primary,
                            selectedTextColor = MaterialTheme.colorScheme.primary,
                            indicatorColor = MaterialTheme.colorScheme.primaryContainer,
                            unselectedIconColor = MaterialTheme.colorScheme.onSurfaceVariant,
                            unselectedTextColor = MaterialTheme.colorScheme.onSurfaceVariant,
                        ),
                    )
                }
            }
        }
        Surface(modifier = Modifier.weight(1f), color = MaterialTheme.colorScheme.background) {
            // Files shared into Myco from another app, for Nearby.
            app.myco.neighbor.NearbyFiles.sharing.value?.let { uris ->
                app.myco.neighbor.NearbyShareSheet(uris, state, client) {
                    app.myco.neighbor.NearbyFiles.sharing.value = null
                }
            }
            if (app.myco.neighbor.NearbyFiles.showing.value) {
                app.myco.neighbor.NearbyFilesSheet(client) { app.myco.neighbor.NearbyFiles.showing.value = false }
            }
            // A neighborhood invitation from any tab's scan, a tap or a link.
            app.myco.neighbor.IncomingInvite.pending.value?.let { invite ->
                // A second invitation arriving gets a fresh sheet, not this
                // one's state swapped under the user's thumb.
                androidx.compose.runtime.key(invite.raw) {
                    app.myco.neighbor.JoinConfirmSheet(invite, client, state.ownNpub) {
                        app.myco.neighbor.IncomingInvite.pending.value = null
                    }
                }
            }
            NavHost(navController = nav, startDestination = "apps") {
                composable("apps") {
                    AppsScreen(
                        state,
                        client,
                        onLaunchNsite = onLaunchNsite,
                        onLaunchNapplet = onLaunchNapplet,
                        onPinNappletToHome = onPinNappletToHome,
                        onPinToHome = onPinToHome,
                        onScanned = onScanned,
                    )
                }
                composable("neighbors") {
                    app.myco.neighbor.NeighborsScreen(
                        state,
                        client,
                        onOpenNeighborhood = { id -> nav.navigate("neighborhood/$id") },
                        onScanned = onScanned,
                    )
                }
                composable("neighborhood/{id}") { entry ->
                    app.myco.neighbor.NeighborhoodPage(
                        id = entry.arguments?.getString("id").orEmpty(),
                        state = state,
                        client = client,
                        // Leaving pops back, and the page then notices the
                        // neighborhood is gone and asks again: only pop while
                        // a neighborhood page is actually on top.
                        onBack = {
                            if (nav.currentDestination?.route?.startsWith("neighborhood/") == true) {
                                nav.popBackStack()
                            }
                        },
                    )
                }
                composable("settings") {
                    SettingsScreen(
                        softwareUpdates = softwareUpdates,
                        state = state,
                        client = client,
                        onBleToggle = onBleToggle,
                        wifiAwareSupported = wifiAwareSupported,
                        onWifiAwareToggle = onWifiAwareToggle,
                        lanEnabled = lanEnabled,
                        onLanToggle = { on -> lanEnabled = on; onLanToggle(on) },
                        meshEnabled = meshEnabled,
                        onMeshToggle = onMeshToggle,
                        onFixNearby = onFixNearby,
                        onFixConnection = onFixConnection,
                        shareMesh = shareMesh,
                        onShareMeshToggle = onShareMeshToggle,
                        openPermissions = openPermissions,
                        onPermissionsOpened = { openPermissions = false },
                        onOfflineOnlyToggle = onOfflineOnlyToggle,
                        developerMode = developerMode,
                        onDeveloperModeToggle = { on -> developerMode = on; onDeveloperModeToggle(on) },
                        bleExhausted = bleExhausted,
                        initialExitProxy = initialExitProxy,
                        onExitProxyChange = onExitProxyChange,
                        onReplayIntro = onReplayIntro,
                    )
                }
                composable("dev") { DevScreen() }
            }
        }
        } // Row
    }
        } // CompositionLocalProvider(LocalMeshControl)

    // A hotspot guest started a transfer — pop the AirDrop-style accept/reject
    // dialog wherever in the app you are (their browser is blocked on this
    // answer; unanswered requests deny themselves after 90s). One at a time,
    // oldest first; deciding it reveals the next.
    val pendingTransfers by TransferGate.pending.collectAsState()
    pendingTransfers.firstOrNull()?.let { req ->
        val size = if (req.size > 0) " (${formatSize(req.size)})" else ""
        AlertDialog(
            // No outside-tap/back dismissal: an accidental swipe must not
            // silently deny (or leave a guest hanging) — the buttons decide.
            onDismissRequest = {},
            title = {
                Text(
                    when (req.direction) {
                        TransferGate.Direction.UPLOAD -> "Incoming file"
                        TransferGate.Direction.DOWNLOAD -> "Share this file?"
                    },
                )
            },
            text = {
                Text(
                    when (req.direction) {
                        TransferGate.Direction.UPLOAD ->
                            "The other phone wants to send you “${req.name}”$size. " +
                                "Accepted files land in Download/Myco."
                        TransferGate.Direction.DOWNLOAD ->
                            "The other phone asks to download “${req.name}”$size."
                    },
                )
            },
            confirmButton = {
                TextButton(onClick = { TransferGate.decide(req.id, true) }) {
                    Text(if (req.direction == TransferGate.Direction.UPLOAD) "Accept" else "Send")
                }
            },
            dismissButton = {
                TextButton(onClick = { TransferGate.decide(req.id, false) }) {
                    Text("Decline", color = MaterialTheme.colorScheme.error)
                }
            },
        )
    }

    // Loud warning if our local relay couldn't bind 4870 (another relay holds it):
    // nsites would talk to that foreign relay and show messages that aren't yours.
    var relayWarnDismissed by remember { mutableStateOf(false) }
    if (state.error.contains("4870") && !relayWarnDismissed) {
        AlertDialog(
            onDismissRequest = { relayWarnDismissed = true },
            title = { Text("Another relay is running") },
            text = {
                Text(
                    "Port 4870 is in use by another app, so Myco's own relay couldn't " +
                        "start. Your apps may talk to the wrong relay — including showing " +
                        "messages that aren't yours. Close the other app and restart Myco.\n\n" +
                        state.error,
                )
            },
            confirmButton = { TextButton(onClick = { relayWarnDismissed = true }) { Text("OK") } },
        )
    }
    }
}

// ----- shared UI building blocks used across screens -----

/** Mesh master-switch state + toggle, provided by [MycoApp] so the status pill
 *  (rendered inside every screen's header) can flip the mesh without threading
 *  a callback through each screen's signature. */
data class MeshControl(val enabled: Boolean, val toggle: (Boolean) -> Unit)

val LocalMeshControl = androidx.compose.runtime.compositionLocalOf { MeshControl(true) {} }

/**
 * The status pill shown top-right on every screen: mesh on/off toggle ·
 * live peer count.
 *
 * Tapping anywhere but the switch opens [MeshStatusSheet] — the pill is a
 * summary, and the summary is only useful if the thing it summarises is one
 * tap away. With the mesh off the whole pill goes red rather than merely
 * showing an unchecked switch: "off" is a state worth noticing from across the
 * room, and a grey slider is not.
 */
@Composable
fun PeersPill(state: AppState) {
    val mesh = LocalMeshControl.current
    val connected = state.blePeers.count { it.connected }
    var sheetOpen by remember { mutableStateOf(false) }

    if (sheetOpen) {
        MeshStatusSheet(meshEnabled = mesh.enabled, onDismiss = { sheetOpen = false })
    }

    Surface(
        shape = CircleShape,
        color = if (mesh.enabled) {
            MaterialTheme.colorScheme.primaryContainer
        } else {
            MaterialTheme.colorScheme.errorContainer
        },
        contentColor = if (mesh.enabled) {
            MaterialTheme.colorScheme.onPrimaryContainer
        } else {
            MaterialTheme.colorScheme.onErrorContainer
        },
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
            modifier = Modifier.padding(start = 4.dp, end = 4.dp),
        ) {
            // 1 — mesh master switch: the same slider as the Settings rows,
            // scaled down to pill height.
            //
            // The whole left block toggles, not the slider. Sizing the Box was
            // not enough on its own: the Switch keeps its own hit rect and
            // swallows everything inside it, so taps landing in the Box but
            // beside the slider did nothing — which is what made this fiddly.
            // Handing the click to the Box and passing the Switch a null
            // onCheckedChange makes the slider a pure indicator and the entire
            // 72×48 block the target. `scale` is a draw transform only, so the
            // slider stays small while the target does not.
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier
                    .height(40.dp)
                    .toggleable(
                        value = mesh.enabled,
                        onValueChange = { mesh.toggle(it) },
                        role = Role.Switch,
                        // No ripple: a bounded indication on a block wider than
                        // the thing it draws reads as a misaligned button.
                        indication = null,
                        interactionSource = remember { MutableInteractionSource() },
                    )
                    .padding(start = 6.dp),
            ) {
                // Named, so the slider is not a mystery switch on every screen.
                Text(
                    "mesh",
                    fontWeight = FontWeight.SemiBold,
                    style = MaterialTheme.typography.labelMedium,
                )
                Box(
                    modifier = Modifier.size(width = 52.dp, height = 40.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    androidx.compose.material3.Switch(
                        checked = mesh.enabled,
                        onCheckedChange = null,
                        modifier = Modifier.scale(0.7f),
                        colors = androidx.compose.material3.SwitchDefaults.colors(
                            // Off is a fault state here, not a neutral one.
                            uncheckedTrackColor = MaterialTheme.colorScheme.error,
                            uncheckedBorderColor = MaterialTheme.colorScheme.error,
                            uncheckedThumbColor = MaterialTheme.colorScheme.onError,
                        ),
                    )
                }
            }
            // 2/3 — the counts, and the whole of them is the panel affordance.
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(7.dp),
                modifier = Modifier
                    .clickable(
                        onClick = { sheetOpen = true },
                        onClickLabel = "Show mesh status",
                    )
                    .padding(start = 2.dp, end = 8.dp, top = 6.dp, bottom = 6.dp),
            ) {
                PillDivider()
                // Live mesh peers, coloured by how much mesh you actually have:
                // none is a fault (and pulses, since it is the one state you
                // want noticed from across the room), one works but has no
                // redundancy, two or more is healthy.
                PeerCountDot(connected)
                Text(
                    "$connected",
                    fontWeight = FontWeight.SemiBold,
                    style = MaterialTheme.typography.titleSmall,
                )
                // The invitation: the counts are a summary, and this says
                // there is more behind them.
                Icon(
                    Icons.Outlined.Info,
                    contentDescription = null,
                    modifier = Modifier.size(16.dp).padding(start = 1.dp),
                    tint = LocalContentColor.current.copy(alpha = 0.7f),
                )
            }
        }
    }
}

@Composable
private fun PillDivider() {
    Box(
        Modifier
            .size(width = 1.dp, height = 16.dp)
            .background(LocalContentColor.current.copy(alpha = 0.25f)),
    )
}

/** A screen's big title + the peers pill, with an optional subtitle underneath. */
@Composable
fun ScreenHeader(title: String, state: AppState, subtitle: String? = null) {
    Column {
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(title, style = MaterialTheme.typography.displaySmall)
            PeersPill(state)
        }
        if (subtitle != null) {
            Spacer(Modifier.size(6.dp))
            Text(subtitle, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

/** A rounded grouped card (settings rows, dev sections). */
@Composable
fun SectionCard(content: @Composable () -> Unit) {
    Surface(
        shape = RoundedCornerShape(18.dp),
        color = MaterialTheme.colorScheme.surfaceVariant,
        border = androidx.compose.foundation.BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(modifier = Modifier.padding(vertical = 4.dp)) { content() }
    }
}

/** An uppercase group label, e.g. "DEVICE" / "MESH". */
@Composable
fun GroupLabel(text: String) {
    Text(
        text,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        fontWeight = FontWeight.Bold,
        style = MaterialTheme.typography.labelMedium,
        modifier = Modifier.padding(start = 4.dp),
    )
}

/** A small filled status dot. */
@Composable
fun StatusDot(color: Color, size: Int = 9) {
    Box(modifier = Modifier.size(size.dp).background(color, CircleShape))
}

/**
 * The mesh-peer dot: red and pulsing with no peers, amber with one, green with
 * two or more.
 *
 * Only the zero case animates. A pulse is an attention-getter, so spending it
 * on the states you can't act on would train people to ignore it — one peer is
 * a working mesh, just a fragile one, and it says that in colour alone.
 *
 * The pulse is read only in the draw phase (`graphicsLayer`), so it redraws the
 * dot each frame without recomposing the header. Low-RAM devices and TVs don't
 * pulse at all: they show the static fault colour.
 */
@Composable
fun PeerCountDot(peers: Int) {
    val color = when {
        peers == 0 -> StatusAlone
        peers == 1 -> StatusThin
        else -> StatusConnected
    }
    val context = LocalContext.current
    val canPulse = remember(context) { pulseAllowed(context) }
    if (peers > 0 || !canPulse) {
        StatusDot(color)
        return
    }
    val pulse = rememberInfiniteTransition(label = "no-peers")
    val alpha = pulse.animateFloat(
        initialValue = 1f,
        targetValue = 0.25f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 900, easing = FastOutSlowInEasing),
            repeatMode = RepeatMode.Reverse,
        ),
        label = "alpha",
    )
    Box(
        modifier = Modifier
            .size(9.dp)
            .graphicsLayer { this.alpha = alpha.value }
            .background(color, CircleShape),
    )
}

/** False on low-RAM devices and TVs, where a never-ending animation costs too much. */
private fun pulseAllowed(context: Context): Boolean {
    val am = context.getSystemService(Context.ACTIVITY_SERVICE) as? ActivityManager
    if (am?.isLowRamDevice == true) return false
    return !context.packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK)
}

/** A monospace label: value row (dev diagnostics). */
@Composable
fun KeyVal(label: String, value: String, valueColor: Color = MaterialTheme.colorScheme.onSurface) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(label, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium)
        Spacer(Modifier.width(12.dp))
        Text(
            value,
            color = valueColor,
            style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
        )
    }
}

/** Human file size. The one byte formatter for the whole `ui` package. */
fun formatSize(bytes: Long): String = when {
    bytes >= 1L shl 20 -> "%.1f MB".format(bytes.toDouble() / (1L shl 20))
    bytes >= 1L shl 10 -> "%.0f kB".format(bytes.toDouble() / (1L shl 10))
    else -> "$bytes B"
}
