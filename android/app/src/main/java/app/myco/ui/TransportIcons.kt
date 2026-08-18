package app.myco.ui

import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import app.myco.R
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
fun TransportIcon(transport: String, modifier: Modifier = Modifier, size: Int = 26) {
    val (res, tint, label) = when (transport) {
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
    Icon(
        painter = painterResource(res),
        contentDescription = label,
        tint = tint,
        modifier = modifier.size(size.dp),
    )
}
