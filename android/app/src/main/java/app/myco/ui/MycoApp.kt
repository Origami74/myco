package app.myco.ui

import androidx.compose.foundation.background
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
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
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.NativeActions
import app.myco.ui.screens.AddScreen
import app.myco.ui.screens.AppsScreen
import app.myco.ui.screens.CircleScreen
import app.myco.ui.screens.DevScreen
import app.myco.ui.screens.DiscoverScreen
import app.myco.ui.screens.PairScreen
import app.myco.ui.screens.SettingsScreen
import app.myco.ui.theme.EmeraldSoft
import app.myco.ui.theme.Slate
import app.myco.ui.theme.StatusConnected
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
    onLaunchNsite: (host: String, title: String) -> Unit,
    onPinToHome: (host: String, title: String) -> Unit,
    onScanned: (String) -> Unit,
    initialMeshEnabled: Boolean,
    onMeshToggle: (Boolean) -> Unit,
    onOfflineOnlyToggle: (Boolean) -> Unit,
) {
    var state by remember { mutableStateOf(client.state()) }
    // Mesh toggle is hoisted here so it survives tab switches.
    var meshEnabled by remember { mutableStateOf(initialMeshEnabled) }
    // BLE advertiser exhaustion (set by the radio, read here for the Settings badge).
    var bleExhausted by remember { mutableStateOf(BleHealth.advertiserExhausted) }
    // Re-poll the native state each second off the main thread (it crosses JNI and
    // walks the blob dir for cache counts). Pure read — does not bump `rev`.
    androidx.compose.runtime.LaunchedEffect(Unit) {
        while (true) {
            delay(1000)
            state = withContext(Dispatchers.IO) { client.state() }
            bleExhausted = BleHealth.advertiserExhausted
        }
    }

    val nav = rememberNavController()
    val onPair = { nav.navigate("pair") }
    val onAddApp = { nav.navigate("add") }
    Scaffold(
        bottomBar = {
            val current by nav.currentBackStackEntryAsState()
            // The full-screen Pair / Add surfaces hide the bottom bar.
            val route = current?.destination?.route
            if (route != "pair" && route != "add") {
                NavigationBar(containerColor = MaterialTheme.colorScheme.surface) {
                    TABS.forEach { tab ->
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
                                if (tab.route == "settings" && bleExhausted) {
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
                                indicatorColor = EmeraldSoft,
                                unselectedIconColor = Slate,
                                unselectedTextColor = Slate,
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
                    AppsScreen(state, client, onLaunchNsite = onLaunchNsite, onPinToHome = onPinToHome, onAddApp = onAddApp)
                }
                composable("circle") { CircleScreen(state, client, onPair = onPair) }
                composable("discover") { DiscoverScreen(state, client, onLaunchNsite = onLaunchNsite) }
                composable("settings") {
                    SettingsScreen(
                        state = state,
                        client = client,
                        onBleToggle = onBleToggle,
                        meshEnabled = meshEnabled,
                        onMeshToggle = { on -> meshEnabled = on; onMeshToggle(on) },
                        onOfflineOnlyToggle = onOfflineOnlyToggle,
                        bleExhausted = bleExhausted,
                    )
                }
                composable("dev") { DevScreen(state) }
                composable("pair") {
                    PairScreen(
                        state = state,
                        onScanned = { text -> nav.popBackStack(); onScanned(text) },
                        onBack = { nav.popBackStack() },
                    )
                }
                composable("add") {
                    AddScreen(
                        state = state,
                        onScanned = { text -> nav.popBackStack(); onScanned(text) },
                        onBack = { nav.popBackStack() },
                    )
                }
            }
        }
    }

    // Incoming pair request → pop-up to accept/decline (mutual pairing).
    PairRequestDialog(state, client) { newState -> state = newState }

    // Loud warning if our local relay couldn't bind 4869 (another relay holds it):
    // nsites would talk to that foreign relay and show messages that aren't yours.
    var relayWarnDismissed by remember { mutableStateOf(false) }
    if (state.error.contains("4869") && !relayWarnDismissed) {
        AlertDialog(
            onDismissRequest = { relayWarnDismissed = true },
            title = { Text("Another relay is running") },
            text = {
                Text(
                    "Port 4869 is in use by another app, so Myco's own relay couldn't " +
                        "start. Your apps may talk to the wrong relay — including showing " +
                        "messages that aren't yours. Close the other app and restart Myco.\n\n" +
                        state.error,
                )
            },
            confirmButton = { TextButton(onClick = { relayWarnDismissed = true }) { Text("OK") } },
        )
    }
}

/**
 * Modal shown when another device has sent a pair request (it scanned our QR).
 * Accept adds them to the Circle and signals them to add us back; Decline drops it.
 */
@Composable
private fun PairRequestDialog(
    state: AppState,
    client: AppCoreClient,
    onStateChange: (AppState) -> Unit,
) {
    val pending = state.pendingPairRequests.firstOrNull() ?: return
    val name = pending.name.ifEmpty { "A nearby device" }
    AlertDialog(
        onDismissRequest = { onStateChange(client.dispatch(NativeActions.declinePairRequest(pending.npub))) },
        title = { Text("Pair with $name?") },
        text = { Text("They scanned your code. Accepting adds them to your Circle so apps and chats flow both ways.") },
        confirmButton = {
            TextButton(onClick = {
                onStateChange(client.dispatch(NativeActions.acceptPairRequest(pending.npub, pending.name)))
            }) { Text("Accept") }
        },
        dismissButton = {
            TextButton(onClick = {
                onStateChange(client.dispatch(NativeActions.declinePairRequest(pending.npub)))
            }) { Text("Decline") }
        },
    )
}

// ----- shared UI building blocks used across screens -----

/**
 * The green pill shown top-right on every screen: live BLE peers ("N peers") plus
 * the total size of your Circle (paired contacts, online or not) after a divider.
 */
@Composable
fun PeersPill(state: AppState) {
    val connected = state.blePeers.count { it.connected }
    val circle = state.circle.size
    Surface(
        shape = CircleShape,
        color = MaterialTheme.colorScheme.primaryContainer,
        contentColor = MaterialTheme.colorScheme.onPrimaryContainer,
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp),
        ) {
            StatusDot(if (connected > 0) StatusConnected else Slate)
            Text(
                "$connected ${if (connected == 1) "peer" else "peers"}",
                fontWeight = FontWeight.SemiBold,
                style = MaterialTheme.typography.labelLarge,
            )
            Box(
                Modifier
                    .padding(start = 2.dp)
                    .size(width = 1.dp, height = 14.dp)
                    .background(MaterialTheme.colorScheme.onPrimaryContainer.copy(alpha = 0.25f)),
            )
            Icon(
                Icons.Filled.People,
                contentDescription = "in Circle",
                modifier = Modifier.size(16.dp),
            )
            Text(
                "$circle",
                fontWeight = FontWeight.SemiBold,
                style = MaterialTheme.typography.labelLarge,
            )
        }
    }
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
            Text(subtitle, color = Slate, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

/** A rounded grouped card (settings rows, dev sections). */
@Composable
fun SectionCard(content: @Composable () -> Unit) {
    Surface(
        shape = RoundedCornerShape(18.dp),
        color = MaterialTheme.colorScheme.surfaceVariant,
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
        color = Slate,
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

/** A monospace label: value row (dev diagnostics). */
@Composable
fun KeyVal(label: String, value: String, valueColor: Color = MaterialTheme.colorScheme.onSurface) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(label, color = Slate, style = MaterialTheme.typography.bodyMedium)
        Spacer(Modifier.width(12.dp))
        Text(
            value,
            color = valueColor,
            style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
        )
    }
}
