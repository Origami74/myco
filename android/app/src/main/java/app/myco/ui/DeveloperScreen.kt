package app.myco.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Divider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import app.myco.core.AppCoreClient
import app.myco.core.BleAdvert
import app.myco.core.BlePeer
import app.myco.core.NativeActions
import app.myco.core.SiteStatus
import app.myco.share.NsiteShare
import kotlin.math.pow
import kotlinx.coroutines.delay

/**
 * P1 developer screen: device identity, embedded-node status, and the BLE
 * peering diagnostics (adapter/scanning + discovered/connected peers). Polls the
 * core once a second (== Tick) so BLE state advances and peers appear.
 */
@Composable
fun DeveloperScreen(
    client: AppCoreClient,
    onBleToggle: (Boolean) -> Unit,
    onLaunchNsite: (host: String, title: String) -> Unit = { _, _ -> },
    onPinToHome: (host: String, title: String) -> Unit = { _, _ -> },
    onScan: () -> Unit = {},
    initialMeshEnabled: Boolean = false,
    onMeshToggle: (Boolean) -> Unit = {},
) {
    var state by remember { mutableStateOf(client.state()) }
    var linkInput by remember { mutableStateOf("") }
    var shareUri by remember { mutableStateOf<String?>(null) }
    var meshEnabled by remember { mutableStateOf(initialMeshEnabled) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(1000)
            // Pure read: the node advances on its own background loop, so the UI
            // only needs to read fresh state (no Tick, so `rev` doesn't churn).
            state = client.state()
        }
    }

    Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        // SelectionContainer makes every Text long-press-selectable + copyable
        // (npub, node_addr, peer fields). The Switch stays interactive.
        SelectionContainer {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text("Myco", style = MaterialTheme.typography.headlineMedium)

            if (state.error.isNotEmpty()) {
                Text("Error: ${state.error}", color = MaterialTheme.colorScheme.error)
            }

            Field("Device npub", state.ownNpub)
            Field("node_addr", state.nodeAddrHex)
            Field("Node", state.nodeStatus)

            Divider()

            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("Bluetooth peering", style = MaterialTheme.typography.titleMedium)
                Switch(checked = state.bleEnabled, onCheckedChange = onBleToggle)
            }
            Field("Adapter", state.bleAdapterName)
            Field("Scanning", state.bleScanning.toString())
            Field("Role", state.bleRole)

            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(modifier = Modifier.weight(1f)) {
                    Text("Allow system-wide access to the mesh", style = MaterialTheme.typography.titleSmall)
                    Text(
                        "Routes fd00::/8 so this device can reach (and be reached by) peers. Uses the VPN slot.",
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                Switch(
                    checked = meshEnabled,
                    onCheckedChange = {
                        meshEnabled = it
                        onMeshToggle(it)
                    },
                )
            }
            Field("Mesh ULA", state.fipsIpv6)

            Text(
                "Peers (${state.blePeers.size})",
                style = MaterialTheme.typography.titleSmall,
                color = MaterialTheme.colorScheme.primary,
            )
            if (state.blePeers.isEmpty()) {
                Text("— none yet —", style = MaterialTheme.typography.bodyMedium)
            } else {
                state.blePeers.forEach { PeerRow(it) }
            }

            Text(
                "Discovered (radio) (${state.bleAdverts.size})",
                style = MaterialTheme.typography.titleSmall,
                color = MaterialTheme.colorScheme.primary,
            )
            if (state.bleAdverts.isEmpty()) {
                Text("— none —", style = MaterialTheme.typography.bodyMedium)
            } else {
                state.bleAdverts.forEach { AdvertRow(it) }
            }

            Divider()

            // --- nsites (P2): paste a link to fetch over IP, then open offline ---
            Text("nsites", style = MaterialTheme.typography.titleMedium)
            OutlinedTextField(
                value = linkInput,
                onValueChange = { linkInput = it },
                singleLine = true,
                label = { Text("Paste an nsite link (npub… / <npub>.nsite.lol)") },
                modifier = Modifier.fillMaxWidth(),
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(
                    onClick = {
                        if (linkInput.isNotBlank()) {
                            state = client.dispatch(NativeActions.openNsite(linkInput.trim()))
                        }
                    },
                ) { Text("Fetch") }
                OutlinedButton(onClick = onScan) { Text("Scan") }
                OutlinedButton(
                    onClick = { state = client.dispatch(NativeActions.wipeStores()) },
                ) { Text("Wipe") }
            }
            Mono(
                "cache",
                "${state.cache.relayEvents} events · ${state.cache.blobCount} blobs · ${state.cache.usedBytes} B",
            )

            if (state.sites.isEmpty()) {
                Text("— no sites yet —", style = MaterialTheme.typography.bodyMedium)
            } else {
                state.sites.forEach { site ->
                    SiteRow(
                        site,
                        onOpen = { onLaunchNsite(site.host, site.title) },
                        onPinToHome = { onPinToHome(site.host, site.title) },
                        onShare = {
                            // Combine the nsite id with this device's pairing info
                            // (npub + a fresh one-time secret) into one QR.
                            shareUri = NsiteShare.buildShareUri(
                                nsiteHost = site.host,
                                deviceNpub = state.ownNpub,
                                deviceName = NsiteShare.deviceName(state.ownNpub),
                                pairSecret = NsiteShare.newPairSecret(),
                            )
                        },
                    )
                }
            }

            Field("Version / rev", "${state.appVersion} / rev ${state.rev}")
        }
        }
    }

    shareUri?.let { uri -> ShareQrDialog(uri, onDismiss = { shareUri = null }) }
}

@Composable
private fun SiteRow(
    site: SiteStatus,
    onOpen: () -> Unit,
    onShare: () -> Unit,
    onPinToHome: () -> Unit,
) {
    val ready = site.state == "ready"
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(
            site.title.ifEmpty { site.host },
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.primary,
        )
        val detail = when (site.state) {
            "syncing" -> "syncing… ${site.filesPulled}/${site.filesTotal}"
            "ready" -> "ready"
            "unreachable" -> "unreachable — ${site.message}"
            "incomplete" -> "incomplete — ${site.message}"
            else -> site.state
        }
        Mono("status", detail)
        Mono("host", site.host)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(onClick = onOpen, enabled = ready) { Text("Open") }
            OutlinedButton(onClick = onShare) { Text("Share") }
        }
        if (ready) {
            OutlinedButton(onClick = onPinToHome) { Text("Add to home screen") }
        }
    }
}

/** Shows the share-an-nsite QR (nsite id + this device's pairing info). */
@Composable
private fun ShareQrDialog(uri: String, onDismiss: () -> Unit) {
    val qr = remember(uri) { NsiteShare.qrBitmap(uri) }
    AlertDialog(
        onDismissRequest = onDismiss,
        confirmButton = { TextButton(onClick = onDismiss) { Text("Close") } },
        title = { Text("Share this nsite") },
        text = {
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Image(
                    bitmap = qr.asImageBitmap(),
                    contentDescription = "nsite share QR",
                    modifier = Modifier.size(260.dp),
                )
                Text(
                    "Scan with another Myco to open this app — and pair with this device if you haven't.",
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        },
    )
}

@Composable
private fun PeerRow(peer: BlePeer) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(
            if (peer.connected) "● connected" else "○ seen",
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.primary,
        )
        Mono("node_addr", peer.nodeAddrHex)
        Mono("npub", peer.npub.ifEmpty { "(handshake pending)" })
        Mono("psm / rssi", "${peer.psm}${peer.rssi?.let { " / $it dBm" } ?: ""}")
    }
}

@Composable
private fun AdvertRow(a: BleAdvert) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Mono("addr", a.addr)
        Mono("psm / rssi / dist", "${a.psm} / ${a.rssi} dBm / ~${"%.1f".format(approxMeters(a.rssi))} m")
    }
}

/**
 * Very rough distance estimate from RSSI via the log-distance path-loss model:
 * d ≈ 10^((txPower@1m − rssi) / (10·n)). BLE RSSI→distance is unreliable
 * (multipath, orientation, chipset), so this is a ballpark, not a measurement.
 */
private const val TX_POWER_AT_1M = -59.0 // typical BLE reference RSSI at 1 m
private const val PATH_LOSS_N = 2.0      // free-space-ish exponent

private fun approxMeters(rssi: Int): Double =
    10.0.pow((TX_POWER_AT_1M - rssi) / (10.0 * PATH_LOSS_N))

@Composable
private fun Mono(label: String, value: String) {
    Text(
        "$label: $value",
        style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
    )
}

@Composable
private fun Field(label: String, value: String) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(label, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary)
        Text(
            value.ifEmpty { "—" },
            style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
        )
    }
}
