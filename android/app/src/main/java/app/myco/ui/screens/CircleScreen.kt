package app.myco.ui.screens

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
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
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.QrCodeScanner
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.CircleContact
import app.myco.core.NativeActions
import app.myco.ui.ScreenHeader
import app.myco.ui.StatusDot
import app.myco.ui.theme.Slate
import app.myco.ui.theme.StatusConnected
import app.myco.ui.theme.avatarColorFor

/**
 * The **Circle**: paired peers — also the relays you pull from. Each card shows
 * connection status, a `relay` badge, and how many nsites that peer is hosting
 * (from Discovery). Scan a friend's share QR to add them; long-press to forget.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun CircleScreen(
    state: AppState,
    client: AppCoreClient,
    onPair: () -> Unit,
) {
    var forget by remember { mutableStateOf<CircleContact?>(null) }
    val connected = state.blePeers.filter { it.connected }.map { it.npub }.toSet()

    LazyColumn(
        modifier = Modifier.fillMaxSize(),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(20.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item {
            ScreenHeader("Circle", state, subtitle = "Paired peers — also the relays you pull from.")
            Spacer(Modifier.height(8.dp))
            OutlinedButton(onClick = onPair) {
                Icon(Icons.Filled.QrCodeScanner, contentDescription = null)
                Spacer(Modifier.size(8.dp))
                Text("Pair a device")
            }
        }
        if (state.circle.isEmpty()) {
            item {
                Text(
                    "No one in your Circle yet. Scan a friend's share QR to pair with their device.",
                    color = Slate,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(top = 12.dp),
                )
            }
        } else {
            items(state.circle, key = { it.npub }) { c ->
                CircleCard(
                    contact = c,
                    online = c.npub in connected,
                    siteCount = state.discovered.count { it.holderNpub == c.npub },
                    onLongPress = { forget = c },
                )
            }
        }
    }

    forget?.let { c ->
        AlertDialog(
            onDismissRequest = { forget = null },
            confirmButton = {
                TextButton(onClick = {
                    client.dispatch(NativeActions.removeFromCircle(c.npub)); forget = null
                }) { Text("Forget") }
            },
            dismissButton = { TextButton(onClick = { forget = null }) { Text("Cancel") } },
            title = { Text("Forget ${c.name.ifEmpty { "this peer" }}?") },
            text = { Text("They'll be removed from your Circle. You can re-pair anytime.") },
        )
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun CircleCard(
    contact: CircleContact,
    online: Boolean,
    siteCount: Int,
    onLongPress: () -> Unit,
) {
    Surface(
        shape = RoundedCornerShape(18.dp),
        color = MaterialTheme.colorScheme.surfaceVariant,
        modifier = Modifier
            .fillMaxWidth()
            .combinedClickable(onClick = {}, onLongClick = onLongPress),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                modifier = Modifier.size(52.dp).background(avatarColorFor(contact.npub), CircleShape),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    contact.name.firstOrNull()?.uppercase() ?: "?",
                    color = Color.White,
                    fontWeight = FontWeight.Bold,
                    style = MaterialTheme.typography.titleLarge,
                )
            }
            Spacer(Modifier.size(14.dp))
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    contact.name.ifEmpty { "Unknown device" },
                    fontWeight = FontWeight.Bold,
                    style = MaterialTheme.typography.titleMedium,
                )
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    StatusDot(if (online) StatusConnected else Slate)
                    Text(
                        if (online) "Connected · BLE" else "Offline",
                        color = if (online) StatusConnected else Slate,
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
                Text(
                    shortNpub(contact.npub),
                    color = Slate,
                    style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                )
            }
            Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                RelayBadge()
                if (siteCount > 0) {
                    Text(
                        "$siteCount ${if (siteCount == 1) "site" else "sites"}",
                        color = Slate,
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            }
        }
    }
}

@Composable
private fun RelayBadge() {
    Surface(
        shape = RoundedCornerShape(50),
        color = MaterialTheme.colorScheme.secondaryContainer,
        contentColor = MaterialTheme.colorScheme.onSecondaryContainer,
    ) {
        Text(
            "relay",
            fontWeight = FontWeight.SemiBold,
            style = MaterialTheme.typography.labelMedium,
            modifier = Modifier.padding(horizontal = 12.dp, vertical = 5.dp),
        )
    }
}

private fun shortNpub(npub: String): String =
    if (npub.length > 16) "${npub.take(10)}…${npub.takeLast(3)}" else npub
