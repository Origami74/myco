package app.myco.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.snapshots.SnapshotStateList
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
import app.myco.core.PeerAttempt
import app.myco.core.PeerDiagnostic
import app.myco.share.DeviceName
import app.myco.ui.KeyVal
import app.myco.ui.ScreenHeader
import app.myco.ui.SectionCard
import app.myco.ui.StatusDot
import app.myco.ui.peerLabel
import app.myco.ui.TransportIcon
import app.myco.ui.locationServicesEnabled
import app.myco.ui.theme.StatusAlone
import app.myco.ui.theme.StatusConnected
import app.myco.ui.theme.StatusReachable
import app.myco.ui.theme.StatusThin
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
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

    // This screen refreshes faster than the rest of the app, because the peer
    // list is only useful if it converges visibly while you watch it (D-16), and
    // last-seen counts in exact seconds (D-18). It drives that itself rather
    // than raising the shell's cadence: `state()` takes many core locks, and
    // CONCERNS.md records that making every tab pay for this rate is not free.
    //
    // Seeded from the hoisted `state` so the first frame is real content — no
    // spinner, no skeleton. The lifecycle gate is the shell's, so leaving the
    // tab disposes the effect and backgrounding suspends it; the core keeps
    // recording either way, so history is complete when the screen returns.
    var devState by remember { mutableStateOf(state) }
    var firstReadLanded by remember { mutableStateOf(false) }
    // Re-read on the same tick as the core state rather than once at
    // composition: this is the row you watch while flipping the setting in
    // another app, so a value frozen at screen entry would be a lie.
    var locationOn by remember { mutableStateOf(locationServicesEnabled(context)) }
    val lifecycleOwner = LocalLifecycleOwner.current
    LaunchedEffect(Unit) {
        lifecycleOwner.repeatOnLifecycle(androidx.lifecycle.Lifecycle.State.STARTED) {
            while (true) {
                devState = withContext(Dispatchers.IO) { client.state() }
                locationOn = withContext(Dispatchers.IO) { locationServicesEnabled(context) }
                firstReadLanded = true
                delay(1000)
            }
        }
    }

    Column(
        modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        // The own npub leads the screen: it is the first thing needed when
        // comparing two devices side by side, and the identity card carrying it
        // sits several cards down.
        ScreenHeader(
            "Dev",
            devState,
            subtitle = devState.ownNpub.takeIf { it.isNotEmpty() }
                ?.let { "you: ${short(it)}" }
                ?: "Technical details — myco-core state.",
        )

        // Who am I, then who can I see, then why. Identity leads because it is
        // what a second device is compared against; the self-check sits under
        // the peer list rather than over it, since it is the follow-up question
        // once the list is empty or short, not the first thing read.
        IdentityCard(devState)

        PeersOverviewCard(devState, firstReadLanded)

        RadioSelfCheckCard(devState, awareSupported, awareAvailable, locationOn)

        PendingPairingsCard(devState, firstReadLanded)

        SelectionContainer {
            Column(verticalArrangement = Arrangement.spacedBy(14.dp)) {
                DevCard("NODE & FIPS") {
                    KeyValDot("Status", if (devState.nodeRunning) "running" else devState.nodeStatus, devState.nodeRunning)
                    KeyVal("node_addr", short(devState.nodeAddrHex))
                    KeyVal("fips ipv6", short(devState.fipsIpv6))
                    KeyVal("mtu", if (devState.fipsMtu > 0) devState.fipsMtu.toString() else "—")
                }
                DevCard("BLE") {
                    KeyVal("adapter", devState.bleAdapterName)
                    KeyValDot("scanning", if (devState.bleScanning) "active" else "idle", devState.bleScanning)
                    KeyVal("role", devState.bleRole)
                }
                DevCard("WI-FI AWARE") {
                    KeyValDot("supported", if (awareSupported) "yes" else "no", awareSupported)
                    KeyValDot(
                        "available",
                        if (awareAvailable) "yes" else "no — is Wi-Fi on?",
                        awareAvailable,
                    )
                    KeyValDot("lane", if (devState.wifiAwareEnabled) "enabled" else "off", devState.wifiAwareEnabled)
                    KeyVal("udp port", if (devState.wifiAwarePort > 0) devState.wifiAwarePort.toString() else "—")
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
                val advertsSorted = devState.bleAdverts.sortedBy { it.addr }
                DevCard("RADIO ADVERTS (${devState.bleAdverts.size})") {
                    if (devState.bleAdverts.isEmpty()) {
                        EmptyLine("none")
                    } else {
                        advertsSorted.forEach { AdvertRow(it) }
                    }
                }
            }
        }

        // D-04: the speedtest is content-layer, not peering — it sits below
        // every peering card rather than second from the top.
        SpeedtestCard(devState, client)

        Spacer(Modifier.height(8.dp))
    }
}

/**
 * **The first card, always** (D-07). Seven observed radio facts in a fixed order
 * that never varies with data, so the person holding the phone can answer "is it
 * me or is it them" before scrolling.
 *
 * Every fact is tri-state. A radio value the app could not actually observe —
 * bridge absent, radio never started — renders "unknown" in the neutral colour,
 * never a confident `false`. That is this phase's standing prohibition: an
 * unobservable fact is a fact, not a failure, so it never uses the error colour
 * and never blocks the screen.
 */
@Composable
private fun RadioSelfCheckCard(
    state: AppState,
    awareSupported: Boolean,
    awareAvailable: Boolean,
    locationOn: Boolean,
) {
    DevCard("RADIO SELF-CHECK") {
        KeyValTri("ble enabled", state.bleEnabled, "on", "off")
        KeyValTri(
            "ble scanning",
            if (state.bleScanningKnown) state.bleScanning else null,
            "active",
            "idle",
        )
        KeyValTri(
            "ble advertising",
            if (state.bleAdvertisingKnown) state.bleAdvertising else null,
            "active",
            "idle",
        )
        // Sits with the BLE rows because that is what it explains. Some vendor
        // stacks gate BLE scan callbacks on the location master switch even
        // though Myco declares `neverForLocation` and asks for no location
        // permission at all; the device then advertises and accepts inbound
        // connections normally while receiving nothing, which reads as
        // "ble scanning: active" and an empty peer list. Off here, with the
        // rows above green, is the whole answer.
        KeyValTri("location services", locationOn, "on", "off")
        KeyValTri("aware supported", awareSupported, "yes", "no")
        KeyValTri("aware available", awareAvailable, "yes", "no")
        KeyValTri(
            "aware discovering",
            if (state.wifiAwareScanningKnown) state.wifiAwareScanning else null,
            "active",
            "idle",
        )
    }
}

/**
 * Incoming pair requests and outbound invites, so a pairing that is waiting is
 * visible rather than inferred from its absence elsewhere (DIAG-06).
 *
 * Requester names arrive from an untrusted mesh peer, so they go through the
 * same shortening helper as every other peer-supplied string; the npub is the
 * row's identity and the name is decoration beside it, never in its place.
 */
@Composable
private fun PendingPairingsCard(state: AppState, firstReadLanded: Boolean) {
    val incoming = state.pendingPairRequests
    val outbound = state.outboundPairs
    DevCard("PENDING PAIRINGS (${incoming.size + outbound.size})") {
        if (incoming.isEmpty() && outbound.isEmpty()) {
            // Same rule as the peer list: do not claim there are no pending
            // requests until this screen has actually read once.
            EmptyLine(if (firstReadLanded) "none" else "reading…")
        } else {
            incoming.forEach { PairingRow("incoming", it.npub, it.name) }
            outbound.forEach { PairingRow("outbound", it.npub, it.name) }
        }
    }
}

/** One pending-pairing row: direction, then the shortened npub with any name. */
@Composable
private fun PairingRow(direction: String, npub: String, name: String) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
    ) {
        Text(direction, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium)
        Text(
            if (name.isEmpty()) short(npub) else "${short(name)}  ${short(npub)}",
            style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
        )
    }
}

/**
 * This device's own identity: the npub peers address it by, and the Circle name
 * they see it as (DIAG-07). Either value being empty renders an em-dash — never
 * a blank card, never a placeholder sentinel.
 */
@Composable
private fun IdentityCard(state: AppState) {
    val context = LocalContext.current
    val circleName = if (state.ownNpub.isEmpty()) "" else DeviceName.current(context, state.ownNpub)
    DevCard("IDENTITY") {
        KeyVal("own npub", state.ownNpub.ifEmpty { "—" }.let { if (it == "—") it else short(it) })
        KeyVal("circle name", circleName.ifEmpty { "—" }.let { if (it == "—") it else short(it) })
    }
}

/**
 * The two-state [KeyValDot] row extended to three: `true`, `false`, and `null`
 * for a fact the app could not observe.
 *
 * `null` renders the literal "unknown" against the neutral variant colour. It is
 * deliberately not the error colour — a radio whose state cannot be read is
 * reporting honestly, and colouring that as an error would tell the user to act
 * on something that may be fine.
 */
@Composable
private fun KeyValTri(label: String, state: Boolean?, yes: String, no: String) {
    val value = when (state) {
        true -> yes
        false -> no
        null -> "unknown"
    }
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
    ) {
        Text(label, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium)
        Row(
            verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            StatusDot(if (state == true) StatusConnected else MaterialTheme.colorScheme.onSurfaceVariant)
            Text(
                value,
                color = if (state == true) StatusConnected else MaterialTheme.colorScheme.onSurfaceVariant,
                fontWeight = FontWeight.SemiBold,
                style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
            )
        }
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
                val name = peerLabel(state, peer.npub)
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
            st.running -> "Testing ${peerLabel(state, st.peerNpub)}…"
            st.generation == 0L -> null
            st.error.isNotEmpty() -> "✗ ${st.error}"
            else -> "↑ %s   ↓ %s   (%s, %s)".format(
                rate(st.upMbps), rate(st.downMbps), peerLabel(state, st.peerNpub), size(st.bytes),
            )
        }
        if (resultLine != null) {
            Text(
                resultLine,
                color = if (st.error.isNotEmpty() && !st.running) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
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
 * Every peer the node knows, in one place: state, last-heard and carrying
 * transport — the merged `state.peers` row Rust already built (D-19), never
 * re-joined here against `blePeers` / the radio lists. The full five-state
 * grouping and expand-in-place detail (D-05/D-11) land in plan 01-04; this is
 * the tracer slice that proves the row end-to-end for a connected peer.
 */
@Composable
private fun PeersOverviewCard(state: AppState, firstReadLanded: Boolean) {
    val connectedCount = state.peers.count { it.state == "connected" }
    // Survives both the 1s refresh and a configuration change, so a row a
    // developer opened to read stays open while the list underneath it updates.
    val expanded = rememberSaveable(saver = expandedKeysSaver) { mutableStateListOf<String>() }
    DevCard("PEERS ($connectedCount)") {
        if (state.peers.isEmpty()) {
            // "No peers" is an assertion of absence; it must not be made before
            // this screen's own first read lands, or a cold open flashes it
            // before real data arrives.
            if (!firstReadLanded) {
                EmptyLine("reading…")
            } else {
                Text(
                    "No peers yet",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 16.dp),
                )
                Text(
                    "Check the radio status above — a peer will show up here as soon as one is heard.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
                )
            }
        }
        // Ruled between rows, not merely spaced: a two-line row against a
        // two-line neighbour reads as one four-line block without them, and the
        // expanded body makes that worse.
        state.peers.forEachIndexed { i, p ->
            if (i > 0) {
                HorizontalDivider(
                    color = MaterialTheme.colorScheme.outline,
                    modifier = Modifier.padding(start = 16.dp),
                )
            }
            val isOpen = p.key in expanded
            PeerDiagnosticRow(p, isOpen) {
                if (isOpen) expanded.remove(p.key) else expanded.add(p.key)
            }
        }
        Spacer(Modifier.height(6.dp))
        Text(
            "aware = Wi-Fi Aware · udp = LAN/AP lane · ble = Bluetooth",
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(horizontal = 16.dp),
        )
    }
}

/**
 * One merged peer diagnostics row, over two lines: state dot + monospace
 * identity with the expand caret on the first, transport and the two link
 * clocks on the second.
 *
 * "seen" is the exact-seconds last-heard counter (D-18); "up" is the FMP
 * session age. They answer different questions — a link heard from a second
 * ago that is only ever a few seconds "up" is re-establishing, which reads as
 * healthy if only the last-heard value is shown. The session survives rekeys
 * (`receiver_idx` rotates roughly every 120s), so a long "up" means the link
 * has held rather than that a handshake went stale.
 *
 * `lastSeenMs == 0L`, `authenticatedAtMs == 0L` and an empty transport all
 * render as an em-dash rather than a false "0s"/blank value.
 */
@Composable
private fun PeerDiagnosticRow(peer: PeerDiagnostic, expanded: Boolean, onToggle: () -> Unit) {
    val dotColor = when (peer.state) {
        "connected" -> StatusConnected
        "reachable-via-relay" -> StatusReachable
        "paired-offline" -> StatusThin
        "unreachable" -> StatusAlone
        // "seen-unidentified" and any unrecognized state: neutral, matching
        // the pre-existing "○ seen" convention.
        else -> MaterialTheme.colorScheme.onSurfaceVariant
    }
    // An unidentified peer shows its address only — never a fabricated name.
    val identity = if (peer.state == "seen-unidentified") {
        peer.bleAddr.ifEmpty { peer.nodeAddrHex }
    } else {
        peer.name.ifEmpty { peer.npub.ifEmpty { peer.nodeAddrHex.ifEmpty { peer.bleAddr } } }
    }
    Column(modifier = Modifier.fillMaxWidth().clickable(onClick = onToggle)) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
        ) {
        // Which radio carried this peer, read at a glance down the left edge.
        // Deliberately larger than the text beside it: scanning the column for
        // "which of these is on Bluetooth" is the common question, and it
        // should not require reading a word on the second line.
        TransportIcon(peer.transport, Modifier.padding(start = 14.dp, end = 2.dp))
        Column(modifier = Modifier.weight(1f)) {
        // Line 1: who. The caret is the affordance — a row that opens has to
        // look like one before it is tapped, and the dot alone never said so.
        Row(
            modifier = Modifier.fillMaxWidth().padding(start = 2.dp, end = 16.dp, top = 8.dp),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
        ) {
            Row(
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
            ) {
                StatusDot(dotColor)
                Text(
                    short(identity),
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = FontFamily.Monospace,
                )
            }
            Text(
                if (expanded) "⌃" else "⌄",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.primary,
            )
        }
        // Line 2: the link, at a glance. Session age rides here rather than
        // only in the expanded body — it is the number that separates a link
        // that keeps re-establishing from one that is simply holding, and that
        // is worth seeing without opening every row.
        Row(
            modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 1.dp, bottom = 8.dp),
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Text(
                peer.transport.ifEmpty { "—" },
                style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                "seen ${elapsedExact(peer.lastSeenMs)} · up ${elapsedExact(peer.authenticatedAtMs)}",
                style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        }
        }
        if (expanded) {
            PeerForensics(peer)
        }
    }
}

/**
 * The expanded body of a peer row: why a connection failed, in place (D-05).
 *
 * Metric lines first — they render even when there is no attempt history, so a
 * peer that has never resolved an attempt still says what it knows. Then the
 * newest [MAX_ATTEMPTS_SHOWN] attempts, newest first, matching the core store's
 * per-peer retention exactly so this list can never grow unbounded.
 *
 * An empty, unreadable or partially corrupted log all degrade to the same
 * neutral no-history line — never red, never a dialog, and nothing here ever
 * rewrites or deletes the file behind it (D-13).
 */
@Composable
private fun PeerForensics(peer: PeerDiagnostic) {
    Column(
        modifier = Modifier.padding(start = 30.dp, end = 16.dp, bottom = 8.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        // The name the peer broadcasts for itself, as received — never the
        // resolved label. This row is what separates "they advertised this" from
        // "we generated it from their npub", which the peer list alone cannot
        // show when the two happen to agree.
        ForensicLine("advert name", peer.advertisedName.ifEmpty { "—" })
        ForensicLine("role", peer.role.ifEmpty { "—" })
        ForensicLine("discovery", if (peer.discoveryMs > 0) "${peer.discoveryMs}ms" else "—")
        ForensicLine("send drops", peer.sendDrops.toString())
        ForensicLine("rssi", peer.rssi?.let { "${it}dBm" } ?: "—")
        Spacer(Modifier.height(4.dp))
        if (peer.attempts.isEmpty()) {
            Text(
                "No history for this peer",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        } else {
            peer.attempts.take(MAX_ATTEMPTS_SHOWN).forEach { AttemptRow(it) }
        }
    }
}

/** One compact label/value line inside an expanded peer row. */
@Composable
private fun ForensicLine(label: String, value: String) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(label, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
        Text(
            value,
            style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
            color = MaterialTheme.colorScheme.onSurface,
        )
    }
}

/**
 * One recorded attempt. Every value here is generated in-repo — a fixed-width
 * clock time, an enum role and an enum outcome — so no peer-supplied text
 * reaches this row.
 */
@Composable
private fun AttemptRow(a: PeerAttempt) {
    Text(
        "${clockTime(a.atMs)}  ${a.role.take(4).padEnd(4)}  ${a.discoveryMs}ms  ${a.outcome}",
        style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

/** Wall-clock `HH:mm:ss` for an attempt timestamp; an unset stamp renders as dashes. */
private fun clockTime(atMs: Long): String {
    if (atMs <= 0L) return "--:--:--"
    val c = java.util.Calendar.getInstance().apply { timeInMillis = atMs }
    return "%02d:%02d:%02d".format(
        c.get(java.util.Calendar.HOUR_OF_DAY),
        c.get(java.util.Calendar.MINUTE),
        c.get(java.util.Calendar.SECOND),
    )
}

/** Matches `MAX_ATTEMPTS_PER_PEER` in the core's attempt store (plan 01-03). */
private const val MAX_ATTEMPTS_SHOWN = 20

/**
 * Persists which peer rows are open across configuration changes.
 *
 * Saves an `ArrayList`, not a `List`: `rememberSaveable`'s default registry only
 * accepts Bundle-storable types, and `toList()` on an empty collection returns
 * Kotlin's `EmptyList` singleton, which is not one — so an empty expansion set
 * (the state on every cold open) crashed the screen at composition.
 */
private val expandedKeysSaver: Saver<SnapshotStateList<String>, ArrayList<String>> = Saver(
    save = { ArrayList(it) },
    restore = { mutableStateListOf<String>().apply { addAll(it) } },
)

/**
 * Exact seconds counting up (`3s`, `47s`, `4m 12s`) — D-18: at the 1s Dev-tab
 * poll cadence this is how a live link is told from a stale one. `0L` (never
 * heard from) renders an em-dash, never a decades-long elapsed time.
 */
private fun elapsedExact(lastSeenMs: Long): String {
    if (lastSeenMs <= 0L) return "—"
    val elapsedMs = (System.currentTimeMillis() - lastSeenMs).coerceAtLeast(0)
    val totalSecs = elapsedMs / 1000
    val mins = totalSecs / 60
    val secs = totalSecs % 60
    return if (mins > 0) "${mins}m ${secs}s" else "${secs}s"
}

@Composable
private fun DevCard(title: String, content: @Composable () -> Unit) {
    Column {
        Text(
            title,
            color = MaterialTheme.colorScheme.primary,
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
        Text(label, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium)
        Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            StatusDot(if (ok) StatusConnected else MaterialTheme.colorScheme.onSurfaceVariant)
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
            color = if (peer.connected) StatusConnected else MaterialTheme.colorScheme.onSurfaceVariant,
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
            color = if (l.up) StatusConnected else MaterialTheme.colorScheme.onSurfaceVariant,
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
            color = if (n.pushed) StatusConnected else MaterialTheme.colorScheme.onSurfaceVariant,
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
    Text(text, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp))
}

private fun short(hex: String): String =
    if (hex.length > 18) "${hex.take(10)}…${hex.takeLast(4)}" else hex

private const val TX_POWER_AT_1M = -59.0
private const val PATH_LOSS_N = 2.0
private fun approxMeters(rssi: Int): Double = 10.0.pow((TX_POWER_AT_1M - rssi) / (10.0 * PATH_LOSS_N))
