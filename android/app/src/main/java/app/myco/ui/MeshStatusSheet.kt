package app.myco.ui

import androidx.compose.foundation.clickable
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
import androidx.compose.runtime.saveable.rememberSaveable
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
import app.myco.core.CircleContact
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

/**
 * Who you're paired with, online first.
 *
 * Ordering is reachable-then-name, never last-heard: a Circle of five phones in
 * one room re-ranks on every poll if the order carries any live measurement,
 * and a list that reshuffles under your thumb is unreadable. Two buckets and an
 * alphabetical sort inside each is stable for as long as reachability is.
 *
 * Nothing spells out "reachable" or "offline" — the dot's colour is the whole
 * status, and repeating it in words on every row was noise. Offline members
 * collapse behind one line, because the answer they give ("still paired, not
 * here") is the same for all of them and does not need five rows.
 */
@Composable
private fun CircleSection(state: AppState, nowMs: Long) {
    val (online, offline) = state.circle
        .sortedBy { it.name.ifEmpty { it.npub }.lowercase() }
        .partition { it.npub in state.reachableNpubs }
    GroupLabel("CIRCLE — ${online.size}/${state.circle.size} REACHABLE")
    if (state.circle.isEmpty()) {
        Hint("Nobody paired yet. Pair a device from the Circle tab.")
        return
    }
    // Survives the 1Hz tick — this is a list you open and then read.
    var showOffline by rememberSaveable { mutableStateOf(false) }
    SectionCard {
        online.forEachIndexed { i, member ->
            if (i > 0) Divider()
            CircleLine(state, member, StatusReachable, nowMs)
        }
        if (offline.isEmpty()) {
            if (online.isEmpty()) EmptyRow("nobody reachable right now")
            return@SectionCard
        }
        if (online.isNotEmpty()) Divider()
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .clickable { showOffline = !showOffline }
                .padding(start = 14.dp, end = 14.dp, top = 10.dp, bottom = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                StatusDot(StatusAlone, size = 8)
                Text(
                    "${offline.size} offline",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Text(
                if (showOffline) "\u2303" else "\u2304",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.primary,
            )
        }
        if (showOffline) {
            offline.forEach { member ->
                Divider()
                CircleLine(state, member, StatusAlone, nowMs)
            }
        }
    }
}

/**
 * One Circle member. Reachability is the Circle's own fact (a live mesh relay
 * at any hop count); the peer row, when there is one, supplies the link
 * numbers. A member reachable over several hops has no direct peer row at all,
 * and that is not a fault — the row simply carries no numbers.
 */
@Composable
private fun CircleLine(state: AppState, member: CircleContact, dot: Color, nowMs: Long) {
    val peer = state.peers.firstOrNull { it.npub == member.npub && it.npub.isNotEmpty() }
    PeerLine(
        name = peerLabel(state, member.npub),
        transport = peer?.transport.orEmpty(),
        dot = dot,
        peer = peer,
        nowMs = nowMs,
    )
}

@Composable
private fun EmptyRow(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.labelMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(start = 14.dp, top = 8.dp, bottom = 8.dp),
    )
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
            state = state,
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
        // A radio this phone does not have is not a lane you can act on, so it
        // gets no row at all — an "unsupported" line is a permanent dead entry
        // on every device that lacks Aware, which is most of them.
        if (awareSupported) {
            Divider()
            LaneBlock(
                state = state,
                transport = "aware",
                label = "Wi-Fi Aware",
                scanning = when {
                    !meshEnabled || !state.wifiAwareEnabled -> false
                    state.wifiAwareScanningKnown -> state.wifiAwareScanning
                    else -> null
                },
                off = !meshEnabled || !state.wifiAwareEnabled,
                peers = connected.filter { it.transport == "aware" },
                nowMs = nowMs,
            )
        }
        Divider()
        LaneBlock(
            state = state,
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
    state: AppState,
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
                "no peers",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(start = 44.dp, top = 4.dp),
            )
        } else {
            peers.forEach { p ->
                PeerLine(
                    // `p.name` is fips's own label, which is an abbreviated
                    // npub rather than anything a person chose — resolve
                    // through the names we have actually been told first, and
                    // fall back to an address only for a row with no npub yet.
                    name = if (p.npub.isNotEmpty()) {
                        peerLabel(state, p.npub)
                    } else {
                        p.nodeAddrHex.ifEmpty { p.bleAddr }
                    },
                    transport = "",
                    dot = StatusConnected,
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
 * The dot is the status — there is no status word. Green/teal/red across a
 * short list reads faster than the same three labels repeated down it.
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
    peer: PeerDiagnostic?,
    nowMs: Long,
    indent: Int = 14,
) {
    Column(modifier = Modifier.fillMaxWidth().padding(start = indent.dp, end = 14.dp, top = 6.dp, bottom = 6.dp)) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            StatusDot(dot, size = 8)
            if (transport.isNotEmpty()) TransportIcon(transport, size = 18)
            Text(shortLabel(name), style = MaterialTheme.typography.bodyMedium)
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
