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
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.NativeActions
import app.myco.share.DeviceName
import app.myco.ui.GroupLabel
import app.myco.ui.RadioAction
import app.myco.ui.RadioWarning
import app.myco.ui.ScreenHeader
import app.myco.ui.SectionCard
import app.myco.ui.radioWarnings
import app.myco.ui.theme.Slate

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
            color = Slate,
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
                    color = Slate,
                    style = MaterialTheme.typography.bodySmall,
                )
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it.take(40) },
                    label = { Text("Name") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    TextButton(
                        onClick = {
                            // Empty resets to the npub-derived default.
                            DeviceName.set(context, "")
                            name = DeviceName.generated(state.ownNpub)
                            client.dispatch(NativeActions.setDeviceName(name))
                        },
                    ) { Text("Reset") }
                    Spacer(Modifier.weight(1f))
                    TextButton(
                        enabled = name.isNotBlank() && name.trim() != saved,
                        onClick = {
                            val trimmed = name.trim()
                            DeviceName.set(context, trimmed)
                            client.dispatch(NativeActions.setDeviceName(trimmed))
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
                    color = Slate,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
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
private fun DeveloperSettings(state: AppState, onOfflineOnlyToggle: (Boolean) -> Unit, onBack: () -> Unit) {
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

/** An actionable radio/VPN misconfiguration (see [radioWarnings]): same visual
 *  language as [BleExhaustedCard], but tappable to jump to the fix. */
@Composable
private fun RadioWarningCard(warning: RadioWarning, onClick: () -> Unit) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(
                MaterialTheme.colorScheme.errorContainer,
                RoundedCornerShape(14.dp),
            )
            .clickable(onClick = onClick)
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
            Text(subtitle, color = Slate, style = MaterialTheme.typography.bodySmall)
        }
        Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, contentDescription = null, tint = Slate)
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
    val contentColor = if (enabled) MaterialTheme.colorScheme.onSurface else Slate
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        LeadingIcon(icon, tint = contentColor)
        Spacer(Modifier.size(14.dp))
        Column(modifier = Modifier.weight(1f)) {
            Text(title, color = contentColor, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.titleMedium)
            Text(subtitle, color = Slate, style = MaterialTheme.typography.bodySmall)
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
        LeadingIcon(icon, tint = Slate)
        Spacer(Modifier.size(14.dp))
        Column(modifier = Modifier.weight(1f)) {
            Text(title, color = Slate, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.titleMedium)
            Text(subtitle, color = Slate, style = MaterialTheme.typography.bodySmall)
        }
        Surface(shape = RoundedCornerShape(8.dp), color = MaterialTheme.colorScheme.surface) {
            Text(
                "SOON",
                color = Slate,
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
        Text(label, color = Slate, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.labelMedium)
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
