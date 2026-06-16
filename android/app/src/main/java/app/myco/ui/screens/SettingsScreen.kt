package app.myco.ui.screens

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
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.Bluetooth
import androidx.compose.material.icons.filled.Key
import androidx.compose.material.icons.filled.Lan
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.NativeActions
import app.myco.ui.GroupLabel
import app.myco.ui.ScreenHeader
import app.myco.ui.SectionCard
import app.myco.ui.theme.Slate

/**
 * **Settings** — grouped Device (identity, storage) and Mesh (Bluetooth mesh,
 * system-wide VPN access) controls, plus a destructive "Clear all content".
 */
@Composable
fun SettingsScreen(
    state: AppState,
    client: AppCoreClient,
    onBleToggle: (Boolean) -> Unit,
    meshEnabled: Boolean,
    onMeshToggle: (Boolean) -> Unit,
) {
    var showIdentity by remember { mutableStateOf(false) }
    var confirmWipe by remember { mutableStateOf(false) }

    val capBytes = 2_000_000_000.0
    val used = state.cache.usedBytes.toDouble()

    Column(
        modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        ScreenHeader("Settings", state)
        Spacer(Modifier.height(8.dp))

        GroupLabel("DEVICE")
        SectionCard {
            SettingRow(
                icon = Icons.Filled.Key,
                title = "Device identity",
                subtitle = shortNpub(state.ownNpub),
                onClick = { showIdentity = true },
            )
            RowDivider()
            StorageRow(usedBytes = state.cache.usedBytes, fraction = (used / capBytes).coerceIn(0.0, 1.0).toFloat(),
                blobs = state.cache.blobCount, events = state.cache.relayEvents)
        }

        Spacer(Modifier.height(8.dp))
        GroupLabel("MESH")
        SectionCard {
            ToggleRow(
                icon = Icons.Filled.Bluetooth,
                title = "Bluetooth mesh",
                subtitle = "Find & link nearby peers offline",
                checked = state.bleEnabled,
                onToggle = onBleToggle,
            )
            RowDivider()
            ToggleRow(
                icon = Icons.Filled.Lan,
                title = "System-wide mesh",
                subtitle = "Let the whole device reach the mesh (uses the VPN slot)",
                checked = meshEnabled,
                onToggle = onMeshToggle,
            )
        }

        Spacer(Modifier.height(8.dp))
        GroupLabel("DATA")
        SectionCard {
            SettingRow(
                icon = null,
                title = "Clear all content",
                subtitle = "Wipe local relay + Blossom (keeps identity & Circle)",
                titleColor = MaterialTheme.colorScheme.error,
                onClick = { confirmWipe = true },
            )
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

    if (showIdentity) {
        AlertDialog(
            onDismissRequest = { showIdentity = false },
            confirmButton = { TextButton(onClick = { showIdentity = false }) { Text("Close") } },
            title = { Text("Device identity") },
            text = {
                SelectionContainer {
                    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                        IdField("npub", state.ownNpub)
                        IdField("node_addr", state.nodeAddrHex)
                        IdField(".fips", state.fipsAddr)
                        IdField("mesh ULA", state.fipsIpv6)
                    }
                }
            },
        )
    }

    if (confirmWipe) {
        AlertDialog(
            onDismissRequest = { confirmWipe = false },
            confirmButton = {
                TextButton(onClick = { client.dispatch(NativeActions.wipeStores()); confirmWipe = false }) {
                    Text("Clear", color = MaterialTheme.colorScheme.error)
                }
            },
            dismissButton = { TextButton(onClick = { confirmWipe = false }) { Text("Cancel") } },
            title = { Text("Clear all content?") },
            text = { Text("Removes every downloaded nsite (relay events + blobs). Your identity and Circle stay.") },
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
) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        LeadingIcon(icon)
        Spacer(Modifier.size(14.dp))
        Column(modifier = Modifier.weight(1f)) {
            Text(title, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.titleMedium)
            Text(subtitle, color = Slate, style = MaterialTheme.typography.bodySmall)
        }
        Switch(checked = checked, onCheckedChange = onToggle)
    }
}

@Composable
private fun StorageRow(usedBytes: Long, fraction: Float, blobs: Long, events: Long) {
    Column(modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 14.dp)) {
        Text("Storage", fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.titleMedium)
        Spacer(Modifier.height(8.dp))
        LinearProgressIndicator(
            progress = { fraction },
            modifier = Modifier.fillMaxWidth().height(8.dp),
            trackColor = MaterialTheme.colorScheme.outline,
        )
        Spacer(Modifier.height(6.dp))
        Text(
            "${humanBytes(usedBytes)} of 2 GB used · $blobs blobs · $events events",
            color = Slate,
            style = MaterialTheme.typography.bodySmall,
        )
    }
}

@Composable
private fun LeadingIcon(icon: ImageVector) {
    Box(
        modifier = Modifier
            .size(38.dp)
            .background(MaterialTheme.colorScheme.background, androidx.compose.foundation.shape.CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, contentDescription = null, tint = MaterialTheme.colorScheme.onSurface)
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

private fun shortNpub(npub: String): String =
    if (npub.length > 16) "${npub.take(12)}…${npub.takeLast(4)}" else npub
