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
import androidx.compose.material3.Divider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
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
import kotlin.math.pow
import kotlinx.coroutines.delay

/**
 * P1 developer screen: device identity, embedded-node status, and the BLE
 * peering diagnostics (adapter/scanning + discovered/connected peers). Polls the
 * core once a second (== Tick) so BLE state advances and peers appear.
 */
@Composable
fun DeveloperScreen(client: AppCoreClient, onBleToggle: (Boolean) -> Unit) {
    var state by remember { mutableStateOf(client.state()) }
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

            Field("Version / rev", "${state.appVersion} / rev ${state.rev}")
        }
        }
    }
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
