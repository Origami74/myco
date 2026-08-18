package app.myco.ui

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
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.GridView
import androidx.compose.material.icons.filled.People
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Terminal
import androidx.compose.material.icons.filled.TravelExplore
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.draw.scale
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
import app.myco.core.NativeActions
import app.myco.hotspot.TransferGate
import app.myco.nfc.PairPresent
import app.myco.share.DeviceName
import app.myco.share.PairSecrets
import app.myco.ui.screens.PairConnectedDialog
import app.myco.ui.screens.PairPendingDialog
import app.myco.ui.screens.AppsScreen
import app.myco.ui.screens.CircleScreen
import app.myco.ui.screens.DevScreen
import app.myco.ui.screens.DiscoverScreen
import app.myco.ui.screens.QrScreen
import app.myco.ui.screens.SettingsScreen

import app.myco.ui.theme.StatusAlone
import app.myco.ui.theme.StatusConnected
import app.myco.ui.theme.StatusThin
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

/**
 * The consumer shell: a bottom-nav app over five surfaces — **Apps** (the nsite
 * launcher), **Circle** (paired peers), **Discover** ("nsites around me"),
 * **Settings**, and **Dev** (diagnostics). The Rust `AppState` is polled once a
 * second here and handed down to every tab, so all screens share one read.
 */
private data class Tab(val route: String, val label: String, val icon: ImageVector)

private val TABS = listOf(
    Tab("apps", "Apps", Icons.Filled.GridView),
    Tab("circle", "Circle", Icons.Filled.People),
    Tab("discover", "Discover", Icons.Filled.TravelExplore),
    Tab("settings", "Settings", Icons.Filled.Settings),
    Tab("dev", "Dev", Icons.Filled.Terminal),
)

@Composable
fun MycoApp(
    client: AppCoreClient,
    onBleToggle: (Boolean) -> Unit,
    wifiAwareSupported: Boolean,
    onWifiAwareToggle: (Boolean) -> Unit,
    onLaunchNsite: (host: String, title: String) -> Unit,
    onPinToHome: (host: String, title: String) -> Unit,
    onScanned: (String) -> Unit,
    initialMeshEnabled: Boolean,
    onMeshToggle: (Boolean) -> Unit,
    onOfflineOnlyToggle: (Boolean) -> Unit,
    initialDeveloperMode: Boolean,
    onDeveloperModeToggle: (Boolean) -> Unit,
    initialExitProxy: String = "",
    onExitProxyChange: (String) -> Unit = {},
    /** Clears the intro's "already seen" flag so it plays in full again. */
    onReplayIntro: () -> Unit = {},
) {
    var state by remember { mutableStateOf(client.state()) }
    // Mesh toggle is hoisted here so it survives tab switches.
    var meshEnabled by remember { mutableStateOf(initialMeshEnabled) }
    // Developer mode gates the Dev tab; hoisted so toggling it rebuilds the nav bar.
    var developerMode by remember { mutableStateOf(initialDeveloperMode) }
    // BLE advertiser exhaustion (set by the radio, read here for the Settings badge).
    var bleExhausted by remember { mutableStateOf(BleHealth.advertiserExhausted) }
    // Name of a peer we just connected to (drives the "connected" celebration).
    var justConnected by remember { mutableStateOf<String?>(null) }
    // Name of a peer we just invited who hasn't accepted yet — drives the
    // "waiting" dialog, so a bump that can't be delivered says so.
    var justInvited by remember { mutableStateOf<String?>(null) }
    val knownInvites = remember { mutableStateOf(state.outboundPairs.map { it.npub }.toSet()) }
    // Circle members we already knew about — anything new means a fresh pairing.
    val knownCircle = remember { mutableStateOf(state.circle.map { it.npub }.toSet()) }
    val context = LocalContext.current
    // Re-poll the native state each second off the main thread (it crosses JNI
    // into the Rust core). Pure read — does not bump `rev`. Gated on STARTED:
    // with the app backgrounded the UI can't show the result anyway, and the
    // 1Hz JNI wakeup was a measurable battery cost. repeatOnLifecycle cancels
    // the loop on onStop and restarts it (immediate first poll) on onStart.
    // Radio/VPN misconfigurations (VPN slot lost, Bluetooth/Wi-Fi off under an
    // enabled transport) — drives the red dot on the Settings tab.
    var radioAlert by remember { mutableStateOf(false) }
    val lifecycleOwner = androidx.lifecycle.compose.LocalLifecycleOwner.current
    androidx.compose.runtime.LaunchedEffect(Unit) {
        lifecycleOwner.repeatOnLifecycle(androidx.lifecycle.Lifecycle.State.STARTED) {
            while (true) {
                state = withContext(Dispatchers.IO) { client.state() }
                bleExhausted = BleHealth.advertiserExhausted
                radioAlert = withContext(Dispatchers.IO) {
                    radioWarnings(context, state, meshEnabled).isNotEmpty()
                }
                delay(1000)
            }
        }
    }

    // Auto-accept while presenting (QR/NFC): a request whose secret is one we just
    // issued is proof the peer read our live code. Consuming it is single-use, so a
    // replayed code can't pair again; those fall through to the manual inbox.
    androidx.compose.runtime.LaunchedEffect(state.pendingPairRequests) {
        // Auto-accept is a *pairing* behavior: while the file-share hotspot owns
        // the NFC surface no pair code is presented, so nothing may pair silently.
        if (!PairPresent.presenting || PairPresent.hotspotActive) return@LaunchedEffect
        for (req in state.pendingPairRequests) {
            if (PairSecrets.consume(context, req.secret)) {
                state = client.dispatch(NativeActions.acceptPairRequest(req.npub, req.name))
                PairPresent.rotate(context, state.ownNpub, DeviceName.current(context, state.ownNpub))
            }
        }
    }

    // Celebrate any new Circle member — fires on BOTH sides of a pairing (the one
    // who accepted and the one whose request was accepted), so both see "connected".
    androidx.compose.runtime.LaunchedEffect(state.circle) {
        val current = state.circle.map { it.npub }.toSet()
        val added = current - knownCircle.value
        if (added.isNotEmpty()) {
            state.circle.firstOrNull { it.npub in added }?.let { justConnected = it.name.ifEmpty { "a device" } }
        }
        knownCircle.value = current
    }

    // An invite that just started waiting. Only *new* ones raise the dialog:
    // the list persists until they accept, so keying on its contents alone
    // would re-open it on every poll.
    androidx.compose.runtime.LaunchedEffect(state.outboundPairs) {
        val current = state.outboundPairs.map { it.npub }.toSet()
        val added = current - knownInvites.value
        if (added.isNotEmpty()) {
            state.outboundPairs.firstOrNull { it.npub in added }?.let {
                justInvited = it.name.ifEmpty { "them" }
            }
        }
        knownInvites.value = current
    }

    val nav = rememberNavController()
    androidx.compose.runtime.CompositionLocalProvider(
        LocalMeshControl provides MeshControl(meshEnabled) { on ->
            meshEnabled = on
            onMeshToggle(on)
        },
    ) {
    Scaffold(
        bottomBar = {
            val current by nav.currentBackStackEntryAsState()
            // The full-screen pairing / Add surfaces hide the bottom bar.
            val route = current?.destination?.route
            if (route != "qr") {
                NavigationBar(containerColor = MaterialTheme.colorScheme.surface) {
                    val tabs = if (developerMode) TABS else TABS.filterNot { it.route == "dev" }
                    tabs.forEach { tab ->
                        val selected = current?.destination?.hierarchy?.any { it.route == tab.route } == true
                        NavigationBarItem(
                            selected = selected,
                            onClick = {
                                nav.navigate(tab.route) {
                                    popUpTo(nav.graph.findStartDestination().id) { saveState = true }
                                    launchSingleTop = true
                                    restoreState = true
                                }
                            },
                            icon = {
                                val badged = (
                                    tab.route == "settings" &&
                                        (bleExhausted || radioAlert || backendErrors(state).isNotEmpty())
                                    ) ||
                                    (tab.route == "circle" && state.pendingPairRequests.isNotEmpty())
                                if (badged) {
                                    BadgedBox(badge = { Badge() }) {
                                        Icon(tab.icon, contentDescription = tab.label)
                                    }
                                } else {
                                    Icon(tab.icon, contentDescription = tab.label)
                                }
                            },
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
        Surface(modifier = Modifier.padding(padding), color = MaterialTheme.colorScheme.background) {
            NavHost(navController = nav, startDestination = "apps") {
                composable("apps") {
                    AppsScreen(state, client, onLaunchNsite = onLaunchNsite, onPinToHome = onPinToHome, onScanned = onScanned)
                }
                composable("circle") {
                    CircleScreen(state, client, onOpenQr = { nav.navigate("qr") })
                }
                composable("discover") { DiscoverScreen(state, client, onLaunchNsite = onLaunchNsite) }
                composable("settings") {
                    SettingsScreen(
                        state = state,
                        client = client,
                        onBleToggle = onBleToggle,
                        wifiAwareSupported = wifiAwareSupported,
                        onWifiAwareToggle = onWifiAwareToggle,
                        meshEnabled = meshEnabled,
                        onMeshToggle = { on -> meshEnabled = on; onMeshToggle(on) },
                        onOfflineOnlyToggle = onOfflineOnlyToggle,
                        developerMode = developerMode,
                        onDeveloperModeToggle = { on -> developerMode = on; onDeveloperModeToggle(on) },
                        bleExhausted = bleExhausted,
                        initialExitProxy = initialExitProxy,
                        onExitProxyChange = onExitProxyChange,
                        onReplayIntro = onReplayIntro,
                    )
                }
                composable("dev") { DevScreen(state, client) }
                composable("qr") {
                    QrScreen(
                        state = state,
                        onScanned = { text -> nav.popBackStack(); onScanned(text) },
                        onBack = { nav.popBackStack() },
                    )
                }
            }
        }
    }
    } // CompositionLocalProvider(LocalMeshControl)

    // Incoming pair requests now live in the persistent Requests inbox (badged on
    // the Circle tab + surfaced on the pairing home), not a transient pop-up.
    justInvited?.let { name ->
        // Suppressed once they are actually in the Circle — an invite accepted
        // before this was dismissed should show the celebration, not the wait.
        if (state.circle.none { it.name == name }) {
            PairPendingDialog(theirName = name, onDone = { justInvited = null })
        }
    }
    justConnected?.let { name ->
        PairConnectedDialog(theirName = name, onDone = { justConnected = null })
    }

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

    // A request only auto-accepts while you're on the Circle tab (presenting). If
    // it arrives while you're elsewhere — another tab, or Myco was launched by the
    // tap — prompt to accept/ignore instead of silently pairing.
    if (!PairPresent.presenting) {
        state.pendingPairRequests.firstOrNull()?.let { req ->
            IncomingRequestDialog(
                name = req.name.ifEmpty { "A nearby device" },
                onAccept = { state = client.dispatch(NativeActions.acceptPairRequest(req.npub, req.name)) },
                onIgnore = { state = client.dispatch(NativeActions.declinePairRequest(req.npub)) },
            )
        }
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

/** Accept/ignore prompt for an incoming pair request that didn't auto-accept. */
@Composable
private fun IncomingRequestDialog(name: String, onAccept: () -> Unit, onIgnore: () -> Unit) {
    AlertDialog(
        onDismissRequest = onIgnore,
        title = { Text("Connect with $name?") },
        text = { Text("They want to join your circle. Check the name matches what they tell you, then accept to pair (it's mutual).") },
        confirmButton = { TextButton(onClick = onAccept) { Text("Accept") } },
        dismissButton = { TextButton(onClick = onIgnore) { Text("Ignore") } },
    )
}

// ----- shared UI building blocks used across screens -----

/** Mesh master-switch state + toggle, provided by [MycoApp] so the status pill
 *  (rendered inside every screen's header) can flip the mesh without threading
 *  a callback through each screen's signature. */
data class MeshControl(val enabled: Boolean, val toggle: (Boolean) -> Unit)

val LocalMeshControl = androidx.compose.runtime.compositionLocalOf { MeshControl(true) {} }

/**
 * The status pill shown top-right on every screen. Three segments:
 * mesh on/off toggle · Circle reachable/total · live peer count.
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
    // Reachability is "we hold a live mesh relay to them", not "they are an
    // adjacent node" — a Circle member many hops away is just as reachable, and
    // counting only direct peers showed 0 while sync was working fine.
    val reachable = state.reachableNpubs.size
    val circle = state.circle.size
    var sheetOpen by remember { mutableStateOf(false) }

    if (sheetOpen) {
        MeshStatusSheet(state, meshEnabled = mesh.enabled, onDismiss = { sheetOpen = false })
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
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.padding(start = 6.dp, end = 4.dp),
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
            Box(
                modifier = Modifier
                    .size(width = 72.dp, height = 48.dp)
                    .toggleable(
                        value = mesh.enabled,
                        onValueChange = { mesh.toggle(it) },
                        role = Role.Switch,
                        // No ripple: a bounded indication on a block wider than
                        // the thing it draws reads as a misaligned button.
                        indication = null,
                        interactionSource = remember { MutableInteractionSource() },
                    ),
                contentAlignment = Alignment.Center,
            ) {
                androidx.compose.material3.Switch(
                    checked = mesh.enabled,
                    onCheckedChange = null,
                    modifier = Modifier.scale(0.75f),
                    colors = androidx.compose.material3.SwitchDefaults.colors(
                        // Off is a fault state here, not a neutral one.
                        uncheckedTrackColor = MaterialTheme.colorScheme.error,
                        uncheckedBorderColor = MaterialTheme.colorScheme.error,
                        uncheckedThumbColor = MaterialTheme.colorScheme.onError,
                    ),
                )
            }
            // 2/3 — the counts, and the whole of them is the panel affordance.
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(7.dp),
                modifier = Modifier
                    .clickable(
                        onClick = { sheetOpen = true },
                        onClickLabel = "Show mesh and circle status",
                    )
                    .padding(start = 2.dp, end = 10.dp, top = 10.dp, bottom = 10.dp),
            ) {
                PillDivider()
                // Circle: reachable now / total paired.
                Icon(
                    Icons.Filled.People,
                    contentDescription = "Circle reachable / total",
                    modifier = Modifier.size(18.dp),
                )
                Text(
                    "$reachable/$circle",
                    fontWeight = FontWeight.SemiBold,
                    style = MaterialTheme.typography.titleSmall,
                )
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
 */
@Composable
fun PeerCountDot(peers: Int) {
    val color = when {
        peers == 0 -> StatusAlone
        peers == 1 -> StatusThin
        else -> StatusConnected
    }
    if (peers > 0) {
        StatusDot(color)
        return
    }
    val pulse = rememberInfiniteTransition(label = "no-peers")
    val alpha by pulse.animateFloat(
        initialValue = 1f,
        targetValue = 0.25f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 900, easing = FastOutSlowInEasing),
            repeatMode = RepeatMode.Reverse,
        ),
        label = "alpha",
    )
    StatusDot(color.copy(alpha = alpha))
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

/** Human file size for the transfer-consent dialog. */
private fun formatSize(bytes: Long): String = when {
    bytes >= 1L shl 20 -> "%.1f MB".format(bytes.toDouble() / (1L shl 20))
    bytes >= 1L shl 10 -> "%.0f kB".format(bytes.toDouble() / (1L shl 10))
    else -> "$bytes B"
}
