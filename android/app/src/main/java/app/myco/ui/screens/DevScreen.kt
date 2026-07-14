package app.myco.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.myco.aware.AwareRadio
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.BleAdvert
import app.myco.core.BlePeer
import app.myco.core.NativeActions
import app.myco.share.DeviceName
import app.myco.ui.KeyVal
import app.myco.ui.ScreenHeader
import app.myco.ui.SectionCard
import app.myco.ui.StatusDot
import app.myco.ui.theme.Emerald
import app.myco.ui.theme.Slate
import app.myco.ui.theme.StatusConnected
import kotlin.math.pow

/**
 * **Dev** — technical diagnostics over the raw `myco-core` state: node/FIPS,
 * the BLE radio, connected peers, scan adverts, and cache counts. Read-only.
 */
@Composable
fun DevScreen(state: AppState, client: AppCoreClient) {
    val context = LocalContext.current
    val awareSupported = AwareRadio.isSupported(context)
    val awareAvailable = AwareRadio.isAvailable(context)
    Column(
        modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        ScreenHeader("Dev", state, subtitle = "Technical details — myco-core state.")

        SpeedtestCard(state, client)

        SelectionContainer {
            Column(verticalArrangement = Arrangement.spacedBy(14.dp)) {
                DevCard("NODE & FIPS") {
                    KeyValDot("Status", if (state.nodeRunning) "running" else state.nodeStatus, state.nodeRunning)
                    KeyVal("node_addr", short(state.nodeAddrHex))
                    KeyVal("fips ipv6", short(state.fipsIpv6))
                    KeyVal("mtu", if (state.fipsMtu > 0) state.fipsMtu.toString() else "—")
                }
                DevCard("BLE") {
                    KeyVal("adapter", state.bleAdapterName)
                    KeyValDot("scanning", if (state.bleScanning) "active" else "idle", state.bleScanning)
                    KeyVal("role", state.bleRole)
                }
                DevCard("WI-FI AWARE") {
                    KeyValDot("supported", if (awareSupported) "yes" else "no", awareSupported)
                    KeyValDot(
                        "available",
                        if (awareAvailable) "yes" else "no — is Wi-Fi on?",
                        awareAvailable,
                    )
                    KeyValDot("lane", if (state.wifiAwareEnabled) "enabled" else "off", state.wifiAwareEnabled)
                    KeyVal("udp port", if (state.wifiAwarePort > 0) state.wifiAwarePort.toString() else "—")
                }
                DevCard("PEERS (${state.blePeers.size})") {
                    if (state.blePeers.isEmpty()) {
                        EmptyLine("none connected")
                    } else {
                        state.blePeers.forEach { PeerRow(it) }
                    }
                }
                DevCard("RADIO ADVERTS (${state.bleAdverts.size})") {
                    if (state.bleAdverts.isEmpty()) {
                        EmptyLine("none")
                    } else {
                        state.bleAdverts.forEach { AdvertRow(it) }
                    }
                }
                DevCard("CACHE") {
                    KeyVal("events", state.cache.relayEvents.toString())
                    KeyVal("blobs", state.cache.blobCount.toString())
                    KeyVal("bytes", state.cache.usedBytes.toString())
                    KeyVal("rev", state.rev.toString())
                }
            }
        }
        Spacer(Modifier.height(8.dp))
    }
}

/**
 * A peer throughput test: pick a connected, handshaken peer and round-trip a
 * ~1 MiB payload through its mesh Blossom (PUT then GET), showing up/down
 * throughput (kbps under 1 Mbps, Mbps above).
 * Only works against a paired peer (their Blossom gates non-loopback by Circle).
 */
@Composable
private fun SpeedtestCard(state: AppState, client: AppCoreClient) {
    val peers = state.blePeers.filter { it.connected && it.npub.isNotEmpty() }
    val st = state.speedtest
    DevCard("SPEEDTEST") {
        if (peers.isEmpty()) {
            EmptyLine("no connected peer to test")
        } else {
            peers.forEach { peer ->
                val name = DeviceName.generated(peer.npub)
                Row(
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
                ) {
                    Text(name, color = MaterialTheme.colorScheme.onSurface, style = MaterialTheme.typography.bodyMedium)
                    // "Retry" once a finished run for this peer failed; "Run" otherwise.
                    val lastForPeer = st.peerNpub == peer.npub && st.generation > 0L
                    val label = when {
                        st.running && st.peerNpub == peer.npub -> "running…"
                        lastForPeer && st.error.isNotEmpty() -> "Retry"
                        else -> "Run"
                    }
                    TextButton(
                        enabled = !st.running,
                        onClick = { client.dispatch(NativeActions.speedtestPeer(peer.npub)) },
                    ) { Text(label) }
                }
            }
        }

        val resultLine = when {
            st.running -> "Testing ${DeviceName.generated(st.peerNpub)}…"
            st.generation == 0L -> null
            st.error.isNotEmpty() -> "✗ ${st.error}"
            else -> "↑ %s   ↓ %s   (%s, %s)".format(
                rate(st.upMbps), rate(st.downMbps), DeviceName.generated(st.peerNpub), size(st.bytes),
            )
        }
        if (resultLine != null) {
            Text(
                resultLine,
                color = if (st.error.isNotEmpty() && !st.running) MaterialTheme.colorScheme.error else Slate,
                style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp),
            )
        }
    }
}

/** Format a throughput in Mbps, dropping to kbps under 1 Mbps where most BLE
 *  runs land (0.5 Mbps reads as "500 kbps", not "0.5 Mbps"). */
private fun rate(mbps: Double): String =
    if (mbps < 1.0) "%.0f kbps".format(mbps * 1000) else "%.1f Mbps".format(mbps)

/** Payload size as KB or MB (the adaptive speedtest climbs from 256 KB to 16 MB). */
private fun size(bytes: Long): String =
    if (bytes >= 1024 * 1024) "%.0f MB".format(bytes / (1024.0 * 1024.0)) else "%d KB".format(bytes / 1024)

@Composable
private fun DevCard(title: String, content: @Composable () -> Unit) {
    Column {
        Text(
            title,
            color = Emerald,
            fontWeight = FontWeight.Bold,
            style = MaterialTheme.typography.titleSmall,
            modifier = Modifier.padding(start = 4.dp, bottom = 6.dp),
        )
        SectionCard {
            Column(modifier = Modifier.padding(vertical = 6.dp)) { content() }
        }
    }
}

@Composable
private fun KeyValDot(label: String, value: String, ok: Boolean) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
    ) {
        Text(label, color = Slate, style = MaterialTheme.typography.bodyMedium)
        Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            StatusDot(if (ok) StatusConnected else Slate)
            Text(
                value,
                color = if (ok) StatusConnected else MaterialTheme.colorScheme.onSurface,
                fontWeight = FontWeight.SemiBold,
                style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
            )
        }
    }
}

@Composable
private fun PeerRow(peer: BlePeer) {
    Column(modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) {
        Text(
            if (peer.connected) "● connected" else "○ seen",
            color = if (peer.connected) StatusConnected else Slate,
            style = MaterialTheme.typography.labelMedium,
        )
        Text(
            "${short(peer.nodeAddrHex)}  ${peer.npub.ifEmpty { "(handshake pending)" }.let { if (it.length > 18) it.take(14) + "…" else it }}",
            style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
        )
    }
}

@Composable
private fun AdvertRow(a: BleAdvert) {
    Text(
        "${a.addr}  psm=${a.psm}  ${a.rssi}dBm  ~${"%.1f".format(approxMeters(a.rssi))}m",
        style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
    )
}

@Composable
private fun EmptyLine(text: String) {
    Text(text, color = Slate, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp))
}

private fun short(hex: String): String =
    if (hex.length > 18) "${hex.take(10)}…${hex.takeLast(4)}" else hex

private const val TX_POWER_AT_1M = -59.0
private const val PATH_LOSS_N = 2.0
private fun approxMeters(rssi: Int): Double = 10.0.pow((TX_POWER_AT_1M - rssi) / (10.0 * PATH_LOSS_N))
