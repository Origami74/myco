package app.myco.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.Bluetooth
import androidx.compose.material.icons.filled.CloudOff
import androidx.compose.material.icons.filled.Code
import androidx.compose.material.icons.filled.DeveloperMode
import androidx.compose.material.icons.filled.Lan
import androidx.compose.material.icons.filled.Person
import androidx.compose.material.icons.filled.Public
import androidx.compose.material.icons.filled.Storage
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material.icons.filled.Wifi
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import kotlin.system.exitProcess
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.NativeActions
import app.myco.share.DeviceName
import app.myco.ui.GroupLabel
import app.myco.ui.NameSuggestions
import app.myco.ui.applyDeviceName
import app.myco.ui.RadioAction
import app.myco.ui.RadioWarning
import app.myco.ui.ScreenHeader
import app.myco.ui.SectionCard
import app.myco.ui.radioWarnings


/** The Settings surfaces: the root list and its three drill-in sub-pages. */
private enum class SettingsPage { Root, Identity, Storage, Developer }

/** Cap used for the storage gauge (matches the LRU target in the core). */
private const val STORAGE_CAP_BYTES = 2_000_000_000.0

/**
 * **Settings** — a root list of categories (Identity, Storage, the Mesh + its
 * transports, and Advanced) that drill into focused sub-pages. Developer-only
 * controls (mesh-only, raw identity) live behind the Advanced → Developer settings
 * page, shown only when developer mode is on.
 */
@Composable
fun SettingsScreen(
    state: AppState,
    client: AppCoreClient,
    onBleToggle: (Boolean) -> Unit,
    wifiAwareSupported: Boolean,
    onWifiAwareToggle: (Boolean) -> Unit,
    meshEnabled: Boolean,
    onMeshToggle: (Boolean) -> Unit,
    onOfflineOnlyToggle: (Boolean) -> Unit,
    developerMode: Boolean,
    onDeveloperModeToggle: (Boolean) -> Unit,
    bleExhausted: Boolean = false,
    initialExitProxy: String = "",
    onExitProxyChange: (String) -> Unit = {},
    onReplayIntro: () -> Unit = {},
) {
    var page by remember { mutableStateOf(SettingsPage.Root) }

    // Sub-pages are local state, not NavHost destinations, so the system back
    // gesture would pop straight to the Apps start destination. Intercept it while
    // drilled in so back returns to the Settings root first.
    BackHandler(enabled = page != SettingsPage.Root) { page = SettingsPage.Root }

    when (page) {
        SettingsPage.Root -> RootSettings(
            state = state,
            onBleToggle = onBleToggle,
            wifiAwareSupported = wifiAwareSupported,
            onWifiAwareToggle = onWifiAwareToggle,
            meshEnabled = meshEnabled,
            onMeshToggle = onMeshToggle,
            developerMode = developerMode,
            onDeveloperModeToggle = onDeveloperModeToggle,
            bleExhausted = bleExhausted,
            onOpenIdentity = { page = SettingsPage.Identity },
            onOpenStorage = { page = SettingsPage.Storage },
            onOpenDeveloper = { page = SettingsPage.Developer },
        )
        SettingsPage.Identity -> IdentitySettings(state, client, onBack = { page = SettingsPage.Root })
        SettingsPage.Storage -> StorageSettings(state, client, onBack = { page = SettingsPage.Root })
        SettingsPage.Developer -> DeveloperSettings(
            state = state,
            onOfflineOnlyToggle = onOfflineOnlyToggle,
            initialExitProxy = initialExitProxy,
            onExitProxyChange = onExitProxyChange,
            onReplayIntro = onReplayIntro,
            onBack = { page = SettingsPage.Root },
        )
    }
}

// ----------------------------------------------------------------------------
// Root
// ----------------------------------------------------------------------------

@Composable
private fun RootSettings(
    state: AppState,
    onBleToggle: (Boolean) -> Unit,
    wifiAwareSupported: Boolean,
    onWifiAwareToggle: (Boolean) -> Unit,
    meshEnabled: Boolean,
    onMeshToggle: (Boolean) -> Unit,
    developerMode: Boolean,
    onDeveloperModeToggle: (Boolean) -> Unit,
    bleExhausted: Boolean,
    onOpenIdentity: () -> Unit,
    onOpenStorage: () -> Unit,
    onOpenDeveloper: () -> Unit,
) {
    val context = LocalContext.current
    val deviceName = DeviceName.current(context, state.ownNpub)
    val used = state.cache.usedBytes.toDouble()
    val pct = (used / STORAGE_CAP_BYTES * 100).coerceIn(0.0, 100.0)
    val free = (STORAGE_CAP_BYTES - used).coerceAtLeast(0.0).toLong()

    SettingsColumn {
        ScreenHeader("Settings", state)
        Spacer(Modifier.height(8.dp))

        GroupLabel("DEVICE")
        SectionCard {
            SettingRow(
                icon = Icons.Filled.Person,
                title = "Identity",
                subtitle = deviceName,
                onClick = onOpenIdentity,
            )
            RowDivider()
            SettingRow(
                icon = Icons.Filled.Storage,
                title = "Storage",
                subtitle = "${"%.0f".format(pct)}% used · ${humanBytes(free)} free",
                onClick = onOpenStorage,
            )
        }

        Spacer(Modifier.height(8.dp))
        GroupLabel("MESH")
        SectionCard {
            // The master switch (an app-owned VPN/TUN under the hood). Required for
            // this device to reach the mesh; its transports below ride on top of it,
            // so they grey out when the mesh is off.
            ToggleRow(
                icon = Icons.Filled.Lan,
                title = "Enable",
                subtitle = "Connect this device to the mesh",
                checked = meshEnabled,
                onToggle = onMeshToggle,
            )
            RowDivider()
            ToggleRow(
                icon = Icons.Filled.Bluetooth,
                title = "Bluetooth",
                subtitle = "Find & link nearby peers offline",
                checked = state.bleEnabled,
                onToggle = onBleToggle,
                enabled = meshEnabled,
            )
            RowDivider()
            ToggleRow(
                icon = Icons.Filled.Wifi,
                title = "Wi-Fi Aware",
                subtitle = if (wifiAwareSupported) {
                    "Faster transfers to nearby peers"
                } else {
                    "Not supported on your device"
                },
                checked = state.wifiAwareEnabled,
                onToggle = onWifiAwareToggle,
                enabled = meshEnabled && wifiAwareSupported,
            )
            RowDivider()
            SoonRow(
                icon = Icons.Filled.Public,
                title = "Internet",
                subtitle = "Mesh over the internet",
            )
        }

        // Radio/VPN misconfigurations that silently break peering — recomputed
        // on every state poll (the `state` param changes each second).
        radioWarnings(context, state, meshEnabled).forEach { warning ->
            Spacer(Modifier.height(8.dp))
            RadioWarningCard(warning) {
                when (warning.action) {
                    RadioAction.FIX_VPN -> onMeshToggle(true) // re-runs the VPN consent flow
                    RadioAction.ENABLE_BLUETOOTH -> runCatching {
                        context.startActivity(
                            android.content.Intent(
                                android.bluetooth.BluetoothAdapter.ACTION_REQUEST_ENABLE,
                            ),
                        )
                    }
                    RadioAction.ENABLE_WIFI -> runCatching {
                        context.startActivity(
                            android.content.Intent(android.provider.Settings.Panel.ACTION_WIFI),
                        )
                    }
                    RadioAction.GRANT_AWARE_PERMISSION -> runCatching {
                        context.startActivity(
                            android.content.Intent(
                                android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                                android.net.Uri.parse("package:${context.packageName}"),
                            ),
                        )
                    }
                    RadioAction.ENABLE_LOCATION -> runCatching {
                        context.startActivity(
                            android.content.Intent(
                                android.provider.Settings.ACTION_LOCATION_SOURCE_SETTINGS,
                            ),
                        )
                    }
                }
            }
        }

        if (bleExhausted) {
            Spacer(Modifier.height(8.dp))
            BleExhaustedCard()
        }

        Spacer(Modifier.height(8.dp))
        GroupLabel("ADVANCED")
        SectionCard {
            ToggleRow(
                icon = Icons.Filled.DeveloperMode,
                title = "Developer mode",
                subtitle = "Show the Dev diagnostics tab",
                checked = developerMode,
                onToggle = onDeveloperModeToggle,
            )
            if (developerMode) {
                RowDivider()
                SettingRow(
                    icon = Icons.Filled.Code,
                    title = "Developer settings",
                    subtitle = "Mesh-only mode, raw identity",
                    onClick = onOpenDeveloper,
                )
            }
        }

        if (state.error.isNotEmpty()) {
            Spacer(Modifier.height(8.dp))
            Text("⚠ ${state.error}", color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
        }
        Text(
            "Myco ${state.appVersion}",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
            modifier = Modifier.padding(top = 8.dp, start = 4.dp),
        )
    }
}

// ----------------------------------------------------------------------------
// Identity sub-page — the memorable name shown to peers when pairing.
// ----------------------------------------------------------------------------

@Composable
private fun IdentitySettings(state: AppState, client: AppCoreClient, onBack: () -> Unit) {
    val context = LocalContext.current
    var name by remember { mutableStateOf(DeviceName.current(context, state.ownNpub)) }
    val saved = DeviceName.current(context, state.ownNpub)

    SettingsColumn {
        SubHeader("Identity", onBack)
        Spacer(Modifier.height(4.dp))

        GroupLabel("DEVICE NAME")
        SectionCard {
            Column(modifier = Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(
                    "Nearby devices see this name when you pair, so they can confirm " +
                        "they're connecting to the right device.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it.take(DeviceName.MAX_LENGTH) },
                    label = { Text("Name") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                // Both defaults, one tap each — picking one saves immediately,
                // since there is nothing left to confirm about a name you chose
                // off a list rather than typed.
                NameSuggestions(state.ownNpub, name) { picked ->
                    name = applyDeviceName(context, client, state.ownNpub, picked)
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Spacer(Modifier.weight(1f))
                    TextButton(
                        enabled = name.isNotBlank() && name.trim() != saved,
                        onClick = {
                            applyDeviceName(context, client, state.ownNpub, name.trim())
                            onBack()
                        },
                    ) { Text("Save") }
                }
            }
        }
    }
}

// ----------------------------------------------------------------------------
// Storage sub-page — usage + the two destructive deletes.
// ----------------------------------------------------------------------------

@Composable
private fun StorageSettings(state: AppState, client: AppCoreClient, onBack: () -> Unit) {
    var confirmCache by remember { mutableStateOf(false) }
    var confirmAll by remember { mutableStateOf(false) }
    var editRelay by remember { mutableStateOf(false) }
    var editBlossom by remember { mutableStateOf(false) }
    // Both settings are read when the core starts, so a save only takes hold on
    // the next launch. Offer the restart rather than leaving the user to guess
    // why nothing changed.
    var restartPrompt by remember { mutableStateOf(false) }
    val context = LocalContext.current

    val used = state.cache.usedBytes
    val fraction = (used.toDouble() / STORAGE_CAP_BYTES).coerceIn(0.0, 1.0).toFloat()
    val free = (STORAGE_CAP_BYTES - used).coerceAtLeast(0.0).toLong()

    SettingsColumn {
        SubHeader("Storage", onBack)
        Spacer(Modifier.height(4.dp))

        GroupLabel("USAGE")
        SectionCard {
            Column(modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 14.dp)) {
                Text(
                    "${"%.0f".format(fraction * 100)}% used",
                    fontWeight = FontWeight.SemiBold,
                    style = MaterialTheme.typography.titleMedium,
                )
                Spacer(Modifier.height(8.dp))
                LinearProgressIndicator(
                    progress = { fraction },
                    modifier = Modifier.fillMaxWidth().height(8.dp),
                    trackColor = MaterialTheme.colorScheme.outline,
                )
                Spacer(Modifier.height(6.dp))
                Text(
                    "${humanBytes(used)} of 2 GB · ${humanBytes(free)} free · " +
                        "${state.cache.blobCount} blobs · ${state.cache.relayEvents} events",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
                // These figures are always the built-in store's. With a custom
                // relay or Blossom configured they still describe what is taking
                // up room here, but no longer what is serving — say so, rather
                // than letting the bar read as the whole picture.
                val notInUse = when {
                    state.cache.externalRelay && state.cache.externalBlobs ->
                        "Not in use — a custom relay and Blossom are configured below."
                    state.cache.externalRelay ->
                        "Events not in use — a custom relay is configured below."
                    state.cache.externalBlobs ->
                        "Blobs not in use — a custom Blossom is configured below."
                    else -> null
                }
                if (notInUse != null) {
                    Spacer(Modifier.height(6.dp))
                    Text(
                        notInUse,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
        }

        Spacer(Modifier.height(8.dp))
        GroupLabel("ADVANCED")
        // A backend that has gone away otherwise looks like an app with no
        // content: every site missing, no explanation. Warn the way the radio
        // warnings do, since the cause is equally invisible from the app.
        if (state.relayBackend.error.isNotEmpty()) {
            BackendUnreachableCard("Can't reach your relay", state.relayBackend.error)
            Spacer(Modifier.height(8.dp))
        }
        if (state.blobBackend.error.isNotEmpty()) {
            BackendUnreachableCard("Can't reach your blob store", state.blobBackend.error)
            Spacer(Modifier.height(8.dp))
        }
        SectionCard {
            SettingRow(
                icon = null,
                title = "Custom relay",
                subtitle = when {
                    state.pendingRelayUrl.isNotEmpty() -> state.pendingRelayUrl
                    else -> "Use another Nostr relay instead of the built-in one"
                },
                onClick = { editRelay = true },
            )
            RowDivider()
            SettingRow(
                icon = null,
                title = "Custom Blossom",
                subtitle = when {
                    state.pendingBlossomUrl.isNotEmpty() -> state.pendingBlossomUrl
                    else -> "Use another Blossom server instead of the built-in one"
                },
                onClick = { editBlossom = true },
            )
        }

        Spacer(Modifier.height(8.dp))
        GroupLabel("DELETE")
        SectionCard {
            SettingRow(
                icon = null,
                title = "Delete cache",
                subtitle = "Free up space — keeps your pinned apps, clears everything else",
                titleColor = MaterialTheme.colorScheme.error,
                onClick = { confirmCache = true },
            )
            RowDivider()
            SettingRow(
                icon = null,
                title = "Delete all data, including apps",
                subtitle = "Wipe entirely (keeps identity & Circle)",
                titleColor = MaterialTheme.colorScheme.error,
                onClick = { confirmAll = true },
            )
        }
    }

    if (editRelay) {
        CustomRelayDialog(
            current = state.pendingRelayUrl,
            onSave = { url ->
                client.dispatch(NativeActions.setCustomRelay(url))
                editRelay = false
                restartPrompt = true
            },
            onDismiss = { editRelay = false },
        )
    }
    if (editBlossom) {
        CustomBlossomDialog(
            current = state.pendingBlossomUrl,
            onSave = { url ->
                client.dispatch(NativeActions.setCustomBlossom(url))
                editBlossom = false
                restartPrompt = true
            },
            onDismiss = { editBlossom = false },
        )
    }
    if (restartPrompt) {
        AlertDialog(
            onDismissRequest = { restartPrompt = false },
            confirmButton = {
                TextButton(onClick = { restartApp(context) }) { Text("Restart now") }
            },
            dismissButton = {
                TextButton(onClick = { restartPrompt = false }) { Text("Later") }
            },
            title = { Text("Saved") },
            text = {
                Text(
                    "Myco needs to restart to use the new setting. Until then it keeps " +
                        "using the current store.",
                )
            },
        )
    }
    if (confirmCache) {
        ConfirmDialog(
            title = "Delete cache?",
            body = "Clears all downloaded relay events and blobs except your pinned apps, " +
                "which keep working offline.",
            confirmLabel = "Delete cache",
            onConfirm = { client.dispatch(NativeActions.wipeCache()); confirmCache = false },
            onDismiss = { confirmCache = false },
        )
    }
    if (confirmAll) {
        ConfirmDialog(
            title = "Delete all data?",
            body = "Removes every downloaded nsite, including pinned apps (relay events + blobs). " +
                "Your identity and Circle stay.",
            confirmLabel = "Delete all",
            onConfirm = { client.dispatch(NativeActions.wipeStores()); confirmAll = false },
            onDismiss = { confirmAll = false },
        )
    }
}

// ----------------------------------------------------------------------------
// Developer sub-page — mesh-only + raw identity (gated by developer mode).
// ----------------------------------------------------------------------------

@Composable
private fun DeveloperSettings(
    state: AppState,
    onOfflineOnlyToggle: (Boolean) -> Unit,
    initialExitProxy: String,
    onExitProxyChange: (String) -> Unit,
    onReplayIntro: () -> Unit,
    onBack: () -> Unit,
) {
    SettingsColumn {
        SubHeader("Developer settings", onBack)
        Spacer(Modifier.height(4.dp))

        GroupLabel("NETWORK")
        SectionCard {
            ToggleRow(
                icon = Icons.Filled.CloudOff,
                title = "Mesh-only",
                subtitle = "Never use the internet relay/Blossom fallback — pull only over the mesh",
                checked = state.offlineOnly,
                onToggle = onOfflineOnlyToggle,
            )
        }

        Spacer(Modifier.height(8.dp))
        GroupLabel("EXIT NODE")
        SectionCard {
            Column(
                modifier = Modifier.fillMaxWidth().padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    "Route web traffic through an HTTP proxy on a mesh exit node. " +
                        "Enter the exit as <npub>.fips:8080 (or [fd00::…]:8080). Blank = off.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                // Init once — NOT keyed on initialExitProxy, which MainActivity
                // re-reads from prefs on every recomposition; keying on it would
                // wipe the user's typing mid-edit (and before Apply persists).
                var field by rememberSaveable { mutableStateOf(initialExitProxy) }
                OutlinedTextField(
                    value = field,
                    onValueChange = { field = it },
                    singleLine = true,
                    label = { Text("Exit proxy") },
                    placeholder = { Text("<npub>.fips:8080") },
                    modifier = Modifier.fillMaxWidth(),
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(onClick = { onExitProxyChange(field.trim()) }) { Text("Apply") }
                    TextButton(onClick = { field = ""; onExitProxyChange("") }) { Text("Turn off") }
                }
            }
        }

        Spacer(Modifier.height(8.dp))
        GroupLabel("INTRO")
        SectionCard {
            var replayed by rememberSaveable { mutableStateOf(false) }
            Column(
                modifier = Modifier.fillMaxWidth().padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    "The intro plays in full on first launch; after that it is only the " +
                        "dive. This puts it back to first-launch state, without clearing " +
                        "app data.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Row(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Button(onClick = { onReplayIntro(); replayed = true }) { Text("Play again") }
                    if (replayed) {
                        Text(
                            "Plays in full next launch.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }

        Spacer(Modifier.height(8.dp))
        GroupLabel("IDENTITY")
        SectionCard {
            SelectionContainer {
                Column(
                    modifier = Modifier.fillMaxWidth().padding(16.dp),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    IdField("npub", state.ownNpub)
                    IdField("node_addr", state.nodeAddrHex)
                    IdField(".fips", state.fipsAddr)
                    IdField("mesh ULA", state.fipsIpv6)
                }
            }
        }
    }
}

// ----------------------------------------------------------------------------
// Shared building blocks
// ----------------------------------------------------------------------------

@Composable
private fun SettingsColumn(content: @Composable () -> Unit) {
    Column(
        modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) { content() }
}

/** A sub-page header: a back arrow + the page title. */
@Composable
private fun SubHeader(title: String, onBack: () -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = onBack) {
            Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
        }
        Spacer(Modifier.size(4.dp))
        Text(title, style = MaterialTheme.typography.headlineSmall)
    }
}

@Composable
private fun ConfirmDialog(
    title: String,
    body: String,
    confirmLabel: String,
    onConfirm: () -> Unit,
    onDismiss: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        confirmButton = {
            TextButton(onClick = onConfirm) { Text(confirmLabel, color = MaterialTheme.colorScheme.error) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
        title = { Text(title) },
        text = { Text(body) },
    )
}

/**
 * An actionable radio/VPN misconfiguration (see [radioWarnings]): same visual
 * language as [BleExhaustedCard], but tappable to jump to the fix.
 *
 * The trailing chevron is what carries that difference. Without it the two
 * cards are pixel-for-pixel the same object — one that takes you to the fix and
 * one that cannot be tapped at all — and the only hint that this one is
 * interactive was a "Tap to…" sentence buried at the end of the detail text.
 * The same chevron marks every other navigating row on this screen.
 */
@Composable
private fun RadioWarningCard(warning: RadioWarning, onClick: () -> Unit) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(
                MaterialTheme.colorScheme.errorContainer,
                RoundedCornerShape(14.dp),
            )
            // Labelled as a button rather than left as an anonymous clickable:
            // a screen reader otherwise announces the warning text with no
            // indication that acting on it is possible from here.
            //
            // The label names the *action* only. `clickable` merges the
            // semantics of its descendants, so both the title and the detail
            // are already announced as this node's description — the title
            // alone is thin ("Location is off" says nothing about the
            // consequence), but the detail that follows it carries that, and
            // repeating either one here would only say it twice.
            .clickable(
                onClickLabel = "fix",
                role = androidx.compose.ui.semantics.Role.Button,
                onClick = onClick,
            )
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(
                Icons.Filled.Warning,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onErrorContainer,
                modifier = Modifier.size(20.dp),
            )
            Spacer(Modifier.size(10.dp))
            Text(
                warning.title,
                color = MaterialTheme.colorScheme.onErrorContainer,
                fontWeight = FontWeight.SemiBold,
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.weight(1f),
            )
            Icon(
                Icons.AutoMirrored.Filled.KeyboardArrowRight,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onErrorContainer,
                modifier = Modifier.size(20.dp),
            )
        }
        Text(
            warning.detail,
            color = MaterialTheme.colorScheme.onErrorContainer,
            style = MaterialTheme.typography.bodySmall,
        )
    }
}

/** Warning shown when the OS denied our BLE advertiser (TOO_MANY_ADVERTISERS):
 *  other apps hold every advertising slot, so peers can't discover this device.
 *  The radio keeps retrying on a backoff; this tells the user how to free a slot. */
/**
 * Relaunch the app, process and all.
 *
 * Restarting the activity is not enough: the settings are read once when the
 * native core is constructed, so the process itself has to go. `makeRestartActivityTask`
 * queues a fresh launch first, so exiting hands control straight back to a new
 * process rather than dropping the user to the launcher.
 */
private fun restartApp(context: android.content.Context) {
    val launch = context.packageManager.getLaunchIntentForPackage(context.packageName)
    val component = launch?.component
    if (component == null) {
        // Nothing sane left to do but stop; the next manual launch picks up the
        // setting anyway.
        exitProcess(0)
    }
    context.startActivity(android.content.Intent.makeRestartActivityTask(component))
    exitProcess(0)
}

@Composable
private fun BackendUnreachableCard(title: String, detail: String) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(
                MaterialTheme.colorScheme.errorContainer,
                RoundedCornerShape(14.dp),
            )
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(
                Icons.Filled.Warning,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onErrorContainer,
                modifier = Modifier.size(20.dp),
            )
            Spacer(Modifier.size(10.dp))
            Text(
                title,
                color = MaterialTheme.colorScheme.onErrorContainer,
                fontWeight = FontWeight.SemiBold,
                style = MaterialTheme.typography.titleMedium,
            )
        }
        Text(
            "$detail\n\nYour apps are stored there, so they won't load until it's " +
                "reachable again. Check that it's running and on the same network, " +
                "or clear the setting below to go back to the built-in store.",
            color = MaterialTheme.colorScheme.onErrorContainer,
            style = MaterialTheme.typography.bodySmall,
        )
    }
}

/**
 * Enter (or clear) the custom relay URL.
 *
 * Carries the trust warning at the point of the decision rather than in a help
 * page: reads are not re-verified, so whoever runs the relay decides what this
 * device believes.
 */
@Composable
private fun CustomRelayDialog(
    current: String,
    onSave: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    var url by remember { mutableStateOf(current) }
    AlertDialog(
        onDismissRequest = onDismiss,
        confirmButton = { TextButton(onClick = { onSave(url.trim()) }) { Text("Save") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
        title = { Text("Custom relay") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(
                    value = url,
                    onValueChange = { url = it },
                    singleLine = true,
                    label = { Text("Relay URL") },
                    placeholder = { Text("ws://192.168.1.10:4869") },
                )
                Text(
                    "Your apps and messages will be stored on this relay instead of on " +
                        "this device. Myco trusts it to check signatures, so whoever runs " +
                        "it decides what this device believes — only use one you control.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    "Leave it empty to go back to the built-in store. Either way it takes " +
                        "effect the next time Myco starts.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        },
    )
}

/**
 * Enter (or clear) the custom Blossom URL.
 *
 * Carries a blunter warning than the relay's. Blobs are the bulk of an nsite, so
 * moving them off the device means a peer pulling an app from you needs your
 * connection to the server — which is the opposite of what the mesh is for.
 */
@Composable
private fun CustomBlossomDialog(
    current: String,
    onSave: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    var url by remember { mutableStateOf(current) }
    AlertDialog(
        onDismissRequest = onDismiss,
        confirmButton = { TextButton(onClick = { onSave(url.trim()) }) { Text("Save") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
        title = { Text("Custom Blossom") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(
                    value = url,
                    onValueChange = { url = it },
                    singleLine = true,
                    label = { Text("Blossom URL") },
                    placeholder = { Text("http://192.168.1.10:24242") },
                )
                Text(
                    "App files will be stored on this server instead of on this device. " +
                        "If it's not on your own network, sharing an app with someone " +
                        "nearby will need your internet connection — Myco won't work " +
                        "offline the way it does now.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    "Leave it empty to go back to the built-in store. Either way it takes " +
                        "effect the next time Myco starts.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        },
    )
}

@Composable
private fun BleExhaustedCard() {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(
                MaterialTheme.colorScheme.errorContainer,
                RoundedCornerShape(14.dp),
            )
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(
                Icons.Filled.Warning,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onErrorContainer,
                modifier = Modifier.size(20.dp),
            )
            Spacer(Modifier.size(10.dp))
            Text(
                "Can't advertise to nearby peers",
                color = MaterialTheme.colorScheme.onErrorContainer,
                fontWeight = FontWeight.SemiBold,
                style = MaterialTheme.typography.titleMedium,
            )
        }
        Text(
            "Another app is using up all of Android's Bluetooth advertising slots, " +
                "so other devices can't discover this one. To fix it: restart the device, " +
                "or turn off Nearby Share / Quick Share / Fast Pair. Myco keeps retrying " +
                "automatically.",
            color = MaterialTheme.colorScheme.onErrorContainer,
            style = MaterialTheme.typography.bodySmall,
        )
    }
}

@Composable
private fun SettingRow(
    icon: ImageVector?,
    title: String,
    subtitle: String,
    titleColor: androidx.compose.ui.graphics.Color = MaterialTheme.colorScheme.onSurface,
    onClick: () -> Unit,
) {
    Row(
        modifier = Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (icon != null) {
            LeadingIcon(icon)
            Spacer(Modifier.size(14.dp))
        }
        Column(modifier = Modifier.weight(1f)) {
            Text(title, color = titleColor, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.titleMedium)
            Text(subtitle, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
        }
        Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun ToggleRow(
    icon: ImageVector,
    title: String,
    subtitle: String,
    checked: Boolean,
    onToggle: (Boolean) -> Unit,
    enabled: Boolean = true,
) {
    val contentColor = if (enabled) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        LeadingIcon(icon, tint = contentColor)
        Spacer(Modifier.size(14.dp))
        Column(modifier = Modifier.weight(1f)) {
            Text(title, color = contentColor, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.titleMedium)
            Text(subtitle, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
        }
        Switch(checked = checked, onCheckedChange = onToggle, enabled = enabled)
    }
}

/** A disabled row standing in for a not-yet-shipped option, tagged "SOON". */
@Composable
private fun SoonRow(icon: ImageVector, title: String, subtitle: String) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        LeadingIcon(icon, tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Spacer(Modifier.size(14.dp))
        Column(modifier = Modifier.weight(1f)) {
            Text(title, color = MaterialTheme.colorScheme.onSurfaceVariant, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.titleMedium)
            Text(subtitle, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
        }
        Surface(shape = RoundedCornerShape(8.dp), color = MaterialTheme.colorScheme.surface) {
            Text(
                "SOON",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontWeight = FontWeight.Bold,
                style = MaterialTheme.typography.labelSmall,
                modifier = Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
            )
        }
    }
}

@Composable
private fun LeadingIcon(icon: ImageVector, tint: androidx.compose.ui.graphics.Color = MaterialTheme.colorScheme.onSurface) {
    Box(
        modifier = Modifier
            .size(38.dp)
            .background(MaterialTheme.colorScheme.background, androidx.compose.foundation.shape.CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, contentDescription = null, tint = tint)
    }
}

@Composable
private fun RowDivider() {
    HorizontalDivider(color = MaterialTheme.colorScheme.outline, modifier = Modifier.padding(start = 16.dp))
}

@Composable
private fun IdField(label: String, value: String) {
    Column {
        Text(label, color = MaterialTheme.colorScheme.onSurfaceVariant, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.labelMedium)
        Text(
            value.ifEmpty { "—" },
            style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
        )
    }
}

private fun humanBytes(b: Long): String = when {
    b >= 1_000_000_000 -> "%.1f GB".format(b / 1_000_000_000.0)
    b >= 1_000_000 -> "%.1f MB".format(b / 1_000_000.0)
    b >= 1_000 -> "%.0f KB".format(b / 1_000.0)
    else -> "$b B"
}
