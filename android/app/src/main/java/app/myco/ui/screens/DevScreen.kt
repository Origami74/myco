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
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.myco.ap.ApRadio
import app.myco.ap.LanFipsNode
import app.myco.aware.AwareLink
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
    val awareLinks by AwareRadio.links.collectAsState()
    val apWifi by ApRadio.wifi.collectAsState()
    val apNodes by ApRadio.nodes.collectAsState()
    Column(
        modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        ScreenHeader("Dev", state, subtitle = "Technical details — myco-core state.")

        PeersOverviewCard(state, awareLinks, apNodes)

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
                    if (awareLinks.isEmpty()) {
                        EmptyLine("no data paths")
                    } else {
                        awareLinks.forEach { AwareLinkRow(it) }
                    }
                }
                DevCard("WI-FI AP (!FIPS)") {
                    KeyValDot(
                        "wi-fi",
                        apWifi.ssid ?: if (apWifi.connected) "connected" else "off",
                        apWifi.connected,
                    )
                    KeyValDot("mdns browse", if (apWifi.browsing) "active" else "idle", apWifi.browsing)
                    if (apNodes.isEmpty()) {
                        EmptyLine("no fips node on this lan")
                    } else {
                        apNodes.forEach { ApNodeRow(it) }
                    }
                }
                // Stable alphabetical order — the state arrays arrive in snapshot
                // order, which reshuffles between polls and makes the rows flap.
                val advertsSorted = state.bleAdverts.sortedBy { it.addr }
                DevCard("RADIO ADVERTS (${state.bleAdverts.size})") {
                    if (state.bleAdverts.isEmpty()) {
                        EmptyLine("none")
                    } else {
                        advertsSorted.forEach { AdvertRow(it) }
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

/**
 * Every peer the node knows, in one place, so the transport sections below
 * don't have to be cross-referenced by hand: who is connected, over which
 * lane(s), and for how long.
 *
 * Lanes are attributed from **our own radios**, not from the node: Wi-Fi Aware
 * rides the ordinary UDP transport, so the node reports "udp" for both Aware
 * and the AP lane and cannot tell them apart. A peer can carry more than one
 * marker while lanes overlap.
 *
 * Uptime is measured from when this screen first observed the peer connected,
 * so it resets on app restart and reads "just now" for a session that predates
 * the process. It is a diagnostic, not an SLA.
 */
@Composable
private fun PeersOverviewCard(
    state: AppState,
    awareLinks: List<AwareLink>,
    apNodes: List<LanFipsNode>,
) {
    // npub → epoch millis first seen connected; cleared when it drops, so a
    // reconnect restarts the clock rather than reporting the older session.
    val since = remember { mutableStateMapOf<String, Long>() }
    val now = System.currentTimeMillis()
    val connected = state.blePeers.filter { it.connected && it.npub.isNotEmpty() }
    for (p in connected) since.putIfAbsent(p.npub, now)
    since.keys.retainAll(connected.map { it.npub }.toSet())

    val awareNpubs = awareLinks.filter { it.up }.map { it.npub }.toSet()
    val udpNpubs = apNodes.filter { it.pushed }.map { it.npub }.toSet()

    DevCard("PEERS (${connected.size})") {
        if (state.blePeers.isEmpty()) {
            Text(
                "No peers yet.",
                style = MaterialTheme.typography.bodySmall,
                color = Slate,
            )
        }
        for (p in state.blePeers.sortedBy { it.npub }) {
            val lanes = buildList {
                if (p.npub in awareNpubs) add("aware")
                if (p.npub in udpNpubs) add("udp")
                // Nothing claimed it: BLE is the lane with no radio-side npub
                // list of its own, so an otherwise-unattributed peer is one.
                if (isEmpty() && p.connected) add("ble")
            }
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    StatusDot(if (p.connected) StatusConnected else Slate)
                    Text(
                        short(p.npub.ifEmpty { p.nodeAddrHex }),
                        style = MaterialTheme.typography.bodySmall,
                        fontFamily = FontFamily.Monospace,
                    )
                }
                Text(
                    if (p.connected) "${lanes.joinToString("+")} · ${uptime(now - (since[p.npub] ?: now))}"
                    else "offline",
                    style = MaterialTheme.typography.bodySmall,
                    color = if (p.connected) Emerald else Slate,
                )
            }
        }
        Spacer(Modifier.height(6.dp))
        Text(
            "aware = Wi-Fi Aware · udp = LAN/AP lane · ble = Bluetooth · uptime since first seen here",
            style = MaterialTheme.typography.labelSmall,
            color = Slate,
        )
    }
}

/** Compact duration for the peers card: `42s`, `7m`, `3h12m`. */
private fun uptime(ms: Long): String {
    val secs = (ms / 1000).coerceAtLeast(0)
    return when {
        secs < 60 -> "${secs}s"
        secs < 3600 -> "${secs / 60}m"
        else -> "${secs / 3600}h${(secs % 3600) / 60}m"
    }
}

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
private fun AwareLinkRow(l: AwareLink) {
    Column(modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) {
        Text(
            if (l.up) "● ndp up" else "○ ndp requested",
            color = if (l.up) StatusConnected else Slate,
            style = MaterialTheme.typography.labelMedium,
        )
        Text(
            "${short(l.npub)}  ${l.addr ?: ""}",
            style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
        )
    }
}

@Composable
private fun ApNodeRow(n: LanFipsNode) {
    Column(modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) {
        Text(
            if (n.pushed) "● pushed to node" else "○ resolved",
            color = if (n.pushed) StatusConnected else Slate,
            style = MaterialTheme.typography.labelMedium,
        )
        Text(
            "${short(n.npub)}  ${n.addr}",
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
