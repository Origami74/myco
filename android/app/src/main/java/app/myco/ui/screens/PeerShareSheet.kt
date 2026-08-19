package app.myco.ui.screens

import android.net.Uri
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.InsertDriveFile
import androidx.compose.material.icons.filled.Photo
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.RadioButton
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.myco.core.AppState
import app.myco.core.CircleContact
import app.myco.share.ExternalShare
import app.myco.share.SharedItem
import app.myco.ui.peerLabel
import app.myco.ui.theme.StatusConnected
import app.myco.ui.theme.avatarColorFor

/**
 * The in-app destination after someone chooses Myco in Android's Sharesheet.
 * It intentionally shows only Circle contacts: a radio-nearby stranger is not
 * a valid file destination until both phones have paired.
 */
@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
fun PeerShareSheet(
    state: AppState,
    uris: List<Uri>,
    onDismiss: () -> Unit,
    onShare: (CircleContact) -> Unit,
) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val items = remember(uris) { uris.map { ExternalShare.describe(context, it) } }
    var selectedNpub by remember(uris) { mutableStateOf<String?>(null) }
    val selected = state.circle.firstOrNull { it.npub == selectedNpub }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(start = 20.dp, end = 20.dp, bottom = 28.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column {
                    Text("Share with Myco", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.ExtraBold)
                    Text(
                        "Choose a paired phone",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
                Icon(Icons.Filled.Share, contentDescription = null, tint = MaterialTheme.colorScheme.primary)
            }

            Spacer(Modifier.height(16.dp))
            SharedItemsCard(items)
            Spacer(Modifier.height(20.dp))

            Text(
                "YOUR PAIRED PHONES",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontWeight = FontWeight.Bold,
                style = MaterialTheme.typography.labelMedium,
            )
            Spacer(Modifier.height(8.dp))

            if (state.circle.isEmpty()) {
                Surface(
                    modifier = Modifier.fillMaxWidth(),
                    shape = RoundedCornerShape(16.dp),
                    color = MaterialTheme.colorScheme.surfaceVariant,
                ) {
                    Column(Modifier.padding(16.dp)) {
                        Text("No paired phones yet", fontWeight = FontWeight.Bold)
                        Spacer(Modifier.height(4.dp))
                        Text(
                            "Pair a phone in Circle first. It will appear here as a share destination.",
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            } else {
                LazyColumn(
                    modifier = Modifier.fillMaxWidth().height((state.circle.size * 68).coerceAtMost(272).dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    items(state.circle, key = { it.npub }) { contact ->
                        PeerShareRow(
                            state = state,
                            contact = contact,
                            selected = contact.npub == selectedNpub,
                            onClick = { selectedNpub = contact.npub },
                        )
                    }
                }
            }

            Spacer(Modifier.height(20.dp))
            Button(
                modifier = Modifier.fillMaxWidth().height(52.dp),
                enabled = selected != null,
                onClick = { selected?.let(onShare) },
            ) {
                Text(selected?.let { "Share with ${peerLabel(state, it.npub)}" } ?: "Choose a phone")
            }
            TextButton(modifier = Modifier.fillMaxWidth(), onClick = onDismiss) { Text("Cancel") }
        }
    }
}

@Composable
private fun SharedItemsCard(items: List<SharedItem>) {
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(18.dp),
        color = MaterialTheme.colorScheme.primaryContainer,
    ) {
        Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            items.take(3).forEach { item ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Icon(
                        if (item.mimeType.startsWith("image/")) Icons.Filled.Photo else Icons.AutoMirrored.Filled.InsertDriveFile,
                        contentDescription = null,
                        tint = MaterialTheme.colorScheme.primary,
                        modifier = Modifier.size(22.dp),
                    )
                    Spacer(Modifier.width(10.dp))
                    Text(
                        item.name,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f),
                        fontWeight = FontWeight.SemiBold,
                    )
                    if (item.size > 0) {
                        Spacer(Modifier.width(8.dp))
                        Text(formatBytes(item.size), color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.labelSmall)
                    }
                }
            }
            if (items.size > 3) {
                Text(
                    "+${items.size - 3} more",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.labelSmall,
                )
            }
        }
    }
}

@Composable
private fun PeerShareRow(
    state: AppState,
    contact: CircleContact,
    selected: Boolean,
    onClick: () -> Unit,
) {
    val online = contact.npub in state.reachableNpubs ||
        state.blePeers.any { it.npub == contact.npub && it.connected }
    val name = peerLabel(state, contact.npub)
    Surface(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .clickable(onClick = onClick),
        shape = RoundedCornerShape(16.dp),
        color = if (selected) MaterialTheme.colorScheme.secondaryContainer else MaterialTheme.colorScheme.surfaceVariant,
        border = if (selected) androidx.compose.foundation.BorderStroke(1.dp, MaterialTheme.colorScheme.primary) else null,
    ) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Row(modifier = Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                BoxAvatar(name, contact.npub)
                Spacer(Modifier.width(12.dp))
                Column {
                    Text(name, fontWeight = FontWeight.Bold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(
                        if (online) "online · ready to receive" else "paired · currently offline",
                        color = if (online) StatusConnected else MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.labelSmall,
                    )
                }
            }
            RadioButton(selected = selected, onClick = onClick)
        }
    }
}

@Composable
private fun BoxAvatar(name: String, npub: String) {
    androidx.compose.foundation.layout.Box(
        modifier = Modifier.size(42.dp).clip(CircleShape).background(avatarColorFor(npub)),
        contentAlignment = Alignment.Center,
    ) {
        Text(name.firstOrNull()?.uppercase() ?: "?", color = Color.White, fontWeight = FontWeight.Bold)
    }
}

private fun formatBytes(bytes: Long): String = when {
    bytes < 1024 -> "$bytes B"
    bytes < 1024 * 1024 -> "${bytes / 1024} KB"
    bytes < 1024 * 1024 * 1024 -> "${bytes / (1024 * 1024)} MB"
    else -> "${bytes / (1024 * 1024 * 1024)} GB"
}
