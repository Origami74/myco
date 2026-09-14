package app.myco.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import app.myco.R
import app.myco.core.PeerDiagnostic
import app.myco.ui.theme.TransportBluetooth
import app.myco.ui.theme.TransportNetwork

/**
 * The transport a peer is reachable over, as an icon.
 *
 * Three lanes, three glyphs: the Bluetooth rune, the Wi-Fi Aware arcs, and a
 * globe for anything routed (LAN, the `!FIPS` AP, mDNS). An unknown or absent
 * transport draws nothing rather than guessing — a peer with no resolved link
 * is a real state and a wrong icon would assert a link that does not exist.
 *
 * Bluetooth keeps its brand blue and the routed lane the app's emerald;
 * Aware follows `onSurface`, so it reads as the plain radio in either theme.
 *
 * Shared between the Dev tab's peer list and the status panel behind the peers
 * pill, so "which of these is on Bluetooth" is the same glyph in both places.
 */
@Composable
fun TransportIcon(
    transport: String,
    modifier: Modifier = Modifier,
    size: Int = 26,
    active: Boolean = true,
) {
    val (res, fullTint, name) = when (transport) {
        "ble" -> Triple(R.drawable.ic_transport_bluetooth, TransportBluetooth, "Bluetooth")
        "aware" -> Triple(
            R.drawable.ic_transport_wifi_aware,
            MaterialTheme.colorScheme.onSurface,
            "Wi-Fi Aware",
        )
        "" -> Triple(0, MaterialTheme.colorScheme.onSurfaceVariant, "")
        // udp, tcp and anything else routed: it reached us over IP.
        else -> Triple(R.drawable.ic_transport_network, TransportNetwork, "Network")
    }
    if (res == 0) {
        Spacer(modifier.size(size.dp))
        return
    }
    // A standby path is a real link fips is not sending on: same glyph, faded,
    // so "what else could carry this peer" reads without a second legend.
    Icon(
        painter = painterResource(res),
        contentDescription = if (active) name else "$name (standby)",
        tint = if (active) fullTint else fullTint.copy(alpha = STANDBY_ALPHA),
        modifier = modifier.size(size.dp),
    )
}

/** Faded enough to read as "not carrying traffic", not so faint it vanishes. */
private const val STANDBY_ALPHA = 0.3f

/**
 * One icon per lane a peer has a path on, the active one lit and the rest
 * faded. Lanes come from the peer's `paths` (multi-path fips); a core that
 * reports none falls back to the single [PeerDiagnostic.transport], and a
 * peer with neither draws nothing.
 *
 * Dead paths are left out: they are history fips keeps for a fast return,
 * not a transport we have. Lanes are drawn in the fixed order Bluetooth,
 * Aware, Network so the row does not reshuffle as paths come and go.
 */
@Composable
fun PathIcons(peer: PeerDiagnostic?, size: Int = 18) {
    val lanes = peerLanes(peer)
    if (lanes.isEmpty()) return
    Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        for ((lane, active) in lanes) TransportIcon(lane, size = size, active = active)
    }
}

/** Fixed display order for lanes; `tcp` and anything else routed sort last. */
private val LANE_ORDER = listOf("ble", "aware", "udp")

/**
 * The lanes a peer has a non-dead path on, in [LANE_ORDER], each with whether
 * fips currently sends on it. The Aware pool can hold several paths to one
 * peer — that is one lane, lit if any of them is active.
 */
fun peerLanes(peer: PeerDiagnostic?): List<Pair<String, Boolean>> {
    if (peer == null) return emptyList()
    val live = peer.paths.filter { it.state != "dead" && it.lane.isNotEmpty() }
    if (live.isEmpty()) {
        return if (peer.transport.isEmpty()) emptyList() else listOf(peer.transport to true)
    }
    return live
        .groupBy { it.lane }
        .map { (lane, paths) -> lane to paths.any { it.active } }
        .sortedBy { (lane, _) -> LANE_ORDER.indexOf(lane).let { if (it < 0) LANE_ORDER.size else it } }
}
