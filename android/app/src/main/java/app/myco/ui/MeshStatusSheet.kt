package app.myco.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.myco.ap.ApRadio
import app.myco.aware.AwareRadio
import app.myco.core.AppState
import app.myco.core.PeerDiagnostic
import app.myco.ui.theme.StatusAlone
import app.myco.ui.theme.StatusConnected
import app.myco.ui.theme.StatusReachable
import app.myco.ui.theme.StatusThin
import kotlinx.coroutines.delay

/**
 * The panel behind the peers pill: what the mesh is actually doing, right now.
 *
 * Two questions, in the order they get asked. **Circle** answers "can I reach
 * my people" — the reason the mesh exists. **Mesh** answers "why not" — one
 * block per radio lane, each saying whether that lane is looking for anyone and
 * which peers it is currently carrying.
 *
 * Every number here is observed, never inferred: a lane whose scan state the
 * app could not read says "unknown" rather than "idle", and a link MMP has
 * never timed shows no ping rather than `0ms`.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MeshStatusSheet(state: AppState, meshEnabled: Boolean, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val apWifi by ApRadio.wifi.collectAsState()
    val awareSupported = remember { AwareRadio.isSupported(context) }

    // The shell's poll replaces `state` once a second, but two equal snapshots
    // don't recompose — and the ages on this panel are derived from the wall
    // clock, not from the snapshot, so they would freeze on a quiet mesh. This
    // ticker is what keeps "seen 4s" counting while nothing else changes.
    var nowMs by remember { mutableStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) {
        while (true) {
            nowMs = System.currentTimeMillis()
            delay(1000)
        }
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(start = 20.dp, end = 20.dp, bottom = 28.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            if (!meshEnabled) {
                Text(
                    "Mesh is off — no radio is scanning and no peer can reach you.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.error,
                    modifier = Modifier.padding(bottom = 6.dp),
                )
            }

            CircleSection(state, nowMs)
            Spacer(Modifier.height(10.dp))
            MeshSection(state, meshEnabled, awareSupported, apWifi.browsing, apWifi.connected, nowMs)
        }
    }
}

// ----- Circle -----

/** Who you're paired with, and whether the mesh can reach them right now. */
@Composable
private fun CircleSection(state: AppState, nowMs: Long) {
    val reachable = state.circle.count { it.npub in state.reachableNpubs }
    GroupLabel("CIRCLE — $reachable/${state.circle.size} REACHABLE")
    if (state.circle.isEmpty()) {
        Hint("Nobody paired yet. Pair a device from the Circle tab.")
        return
    }
    SectionCard {
        state.circle.forEachIndexed { i, member ->
            if (i > 0) Divider()
            // Reachability is the Circle's own fact (a live mesh relay at any
            // hop count); the peer row, when there is one, is what supplies the
            // link numbers. A member reachable over several hops has no direct
            // peer row at all, and that is not a fault.
            val peer = state.peers.firstOrNull { it.npub == member.npub && it.npub.isNotEmpty() }
            PeerLine(
                name = member.name.ifEmpty { "a device" },
                transport = peer?.transport.orEmpty(),
                dot = if (member.npub in state.reachableNpubs) StatusReachable else StatusAlone,
                status = if (member.npub in state.reachableNpubs) "reachable" else "offline",
                peer = peer,
                nowMs = nowMs,
            )
        }
    }
}

// ----- Mesh -----

/**
 * One block per radio lane. The lane header carries its scan state, so "nobody
 * is here" and "we are not looking" are never the same line.
 */
@Composable
private fun MeshSection(
    state: AppState,
    meshEnabled: Boolean,
    awareSupported: Boolean,
    lanBrowsing: Boolean,
    wifiConnected: Boolean,
    nowMs: Long,
) {
    val connected = state.peers.filter { it.state == "connected" }
    GroupLabel("MESH — ${connected.size} PEER${if (connected.size == 1) "" else "S"}")
    SectionCard {
        LaneBlock(
            transport = "ble",
            label = "Bluetooth",
            // Tri-state on purpose: the bridge can be absent or the radio never
            // started, and that is "unknown", not a confident "idle".
            scanning = when {
                !meshEnabled || !state.bleEnabled -> false
                state.bleScanningKnown -> state.bleScanning
                else -> null
            },
            off = !meshEnabled || !state.bleEnabled,
            peers = connected.filter { it.transport == "ble" },
            nowMs = nowMs,
        )
        Divider()
        LaneBlock(
            transport = "aware",
            label = "Wi-Fi Aware",
            scanning = when {
                !meshEnabled || !state.wifiAwareEnabled -> false
                state.wifiAwareScanningKnown -> state.wifiAwareScanning
                else -> null
            },
            off = !meshEnabled || !state.wifiAwareEnabled || !awareSupported,
            offLabel = if (!awareSupported) "unsupported" else "off",
            peers = connected.filter { it.transport == "aware" },
            nowMs = nowMs,
        )
        Divider()
        LaneBlock(
            transport = "udp",
            label = "Network",
            // The routed lane's "scanning" is the mDNS browse that finds fips
            // nodes on the LAN or the !FIPS AP.
            scanning = if (!meshEnabled) false else lanBrowsing,
            off = !meshEnabled || !wifiConnected,
            offLabel = if (!wifiConnected) "no wi-fi" else "off",
            peers = connected.filter { it.transport !in setOf("ble", "aware", "") },
            nowMs = nowMs,
        )
    }
}

/** A lane header — icon, name, scan state — over the peers it is carrying. */
@Composable
private fun LaneBlock(
    transport: String,
    label: String,
    scanning: Boolean?,
    off: Boolean,
    peers: List<PeerDiagnostic>,
    nowMs: Long,
    offLabel: String = "off",
) {
    val (word, color) = when {
        off -> offLabel to MaterialTheme.colorScheme.onSurfaceVariant
        scanning == null -> "unknown" to MaterialTheme.colorScheme.onSurfaceVariant
        scanning -> "scanning" to StatusConnected
        else -> "idle" to StatusThin
    }
    Column(modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp)) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                TransportIcon(transport, size = 22)
                Text(label, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold)
            }
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                StatusDot(color, size = 7)
                Text(word, style = MaterialTheme.typography.labelMedium, color = color)
            }
        }
        if (peers.isEmpty()) {
            Text(
                "no peers on this lane",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(start = 44.dp, top = 4.dp),
            )
        } else {
            peers.forEach { p ->
                PeerLine(
                    name = p.name.ifEmpty { p.npub.ifEmpty { p.nodeAddrHex.ifEmpty { p.bleAddr } } },
                    transport = "",
                    dot = StatusConnected,
                    status = null,
                    peer = p,
                    nowMs = nowMs,
                    indent = 44,
                )
            }
        }
    }
}

// ----- one peer, two lines -----

/**
 * A peer as this panel states it: who, then the three link numbers.
 *
 * `peer` being null (a Circle member reachable over relay with no direct row)
 * collapses to the identity line alone — the numbers are link facts and there
 * is no link to state them about.
 */
@Composable
private fun PeerLine(
    name: String,
    transport: String,
    dot: Color,
    status: String?,
    peer: PeerDiagnostic?,
    nowMs: Long,
    indent: Int = 14,
) {
    Column(modifier = Modifier.fillMaxWidth().padding(start = indent.dp, end = 14.dp, top = 6.dp, bottom = 6.dp)) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.weight(1f, fill = false),
            ) {
                StatusDot(dot, size = 8)
                if (transport.isNotEmpty()) TransportIcon(transport, size = 18)
                Text(shortLabel(name), style = MaterialTheme.typography.bodyMedium)
            }
            if (status != null) {
                Text(
                    status,
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        if (peer != null) {
            Text(
                "ping ${ping(peer.srttMs)} · up ${age(peer.authenticatedAtMs, nowMs)} · seen ${seen(peer.lastSeenMs, nowMs)}",
                style = MaterialTheme.typography.labelMedium.copy(fontFamily = FontFamily.Monospace),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(start = 16.dp, top = 2.dp),
            )
        }
    }
}

@Composable
private fun Divider() {
    HorizontalDivider(
        color = MaterialTheme.colorScheme.outline,
        modifier = Modifier.padding(horizontal = 14.dp),
    )
}

@Composable
private fun Hint(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(start = 4.dp, top = 2.dp),
    )
}

/** MMP's smoothed RTT. Never "0ms" for a link that was simply never timed. */
private fun ping(srttMs: Double?): String =
    srttMs?.let { if (it < 10) "%.1fms".format(it) else "%.0fms".format(it) } ?: "—"

/** How long the FMP session has held. `0` means there is none — an em-dash. */
private fun age(sinceMs: Long, nowMs: Long): String =
    if (sinceMs <= 0L) "—" else duration((nowMs - sinceMs).coerceAtLeast(0L) / 1000)

/**
 * Last heard. Anything under ten seconds is "now": the exact second only
 * matters once a link has gone quiet long enough to worry about, and a counter
 * flickering 1s/2s/3s reads as a problem when it is the healthy case.
 */
private fun seen(lastSeenMs: Long, nowMs: Long): String {
    if (lastSeenMs <= 0L) return "—"
    val secs = (nowMs - lastSeenMs).coerceAtLeast(0L) / 1000
    return if (secs < 10) "now" else duration(secs)
}

private fun duration(secs: Long): String = when {
    secs < 60 -> "${secs}s"
    secs < 3600 -> "${secs / 60}m"
    else -> "${secs / 3600}h"
}

/** Trim an npub/hex fallback to something that fits one line. */
private fun shortLabel(s: String): String =
    if (s.length > 18) "${s.take(10)}…${s.takeLast(4)}" else s
