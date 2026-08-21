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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.InsertDriveFile
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Photo
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.myco.core.AppState
import app.myco.core.CircleContact
import app.myco.core.FileTransfer
import app.myco.share.ExternalShare
import app.myco.share.SharedItem
import app.myco.ui.TransferAvatar
import app.myco.ui.TransferProgressBar
import app.myco.ui.formatSize
import app.myco.ui.isLive
import app.myco.ui.peerLabel
import app.myco.ui.theme.StatusConnected
import app.myco.ui.theme.avatarColorFor
import app.myco.ui.transferStage

/**
 * The in-app destination after someone chooses Myco in Android's Sharesheet.
 * It intentionally shows only Circle contacts: a radio-nearby stranger is not
 * a valid file destination until both phones have paired.
 *
 * The send never leaves this sheet. Picking a peer swaps the list for that
 * transfer's progress in place, so the whole flow — choose, send, watch, done —
 * happens in one surface instead of handing off to a modal that covered the app
 * until the transfer finished.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PeerShareSheet(
    state: AppState,
    uris: List<Uri>,
    onDismiss: () -> Unit,
    onShare: (CircleContact) -> Unit,
    onCancelTransfer: (FileTransfer) -> Unit = {},
    /** Peer chosen before the files were — from a contact's sheet on the Circle tab. */
    preselectedNpub: String? = null,
) {
    val context = LocalContext.current
    val items = remember(uris) { uris.map { ExternalShare.describe(context, it) } }
    var selectedNpub by remember(uris) { mutableStateOf(preselectedNpub) }
    // Once a send starts, this is the peer whose progress the sheet follows.
    var sendingTo by remember(uris) { mutableStateOf<CircleContact?>(null) }
    val selected = state.circle.firstOrNull { it.npub == selectedNpub }

    val target = sendingTo
    val sending = target?.let { peer ->
        state.fileTransfers.filter { it.direction == "outgoing" && it.peerNpub == peer.npub }
    }.orEmpty()
    // An empty list means two different things: the core has not created the
    // row yet, or the transfer finished and cleared itself. Only the second is
    // "delivered", so remember whether a row was ever here.
    var everStarted by remember(uris) { mutableStateOf(false) }
    if (sending.isNotEmpty() && !everStarted) everStarted = true

    // Skip the half-height resting state, and scroll the content rather than the
    // sheet: at the default height the send button sat below the fold, which
    // read as a broken sheet rather than a scrollable one.
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(start = 20.dp, end = 20.dp, bottom = 28.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column {
                    Text(
                        if (target == null) "Share with Myco" else "Sending",
                        style = MaterialTheme.typography.headlineSmall,
                        fontWeight = FontWeight.ExtraBold,
                    )
                    Text(
                        if (target == null) {
                            "Choose a paired phone"
                        } else {
                            "to ${peerLabel(state, target.npub)}"
                        },
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
                Icon(
                    Icons.Filled.Share,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.primary,
                )
            }

            Spacer(Modifier.height(16.dp))

            if (target == null) {
                SharedItemsCard(items)
                Spacer(Modifier.height(20.dp))
                PairedPhonePicker(
                    state = state,
                    selectedNpub = selectedNpub,
                    onSelect = { selectedNpub = it },
                )
                Spacer(Modifier.height(20.dp))
                Button(
                    modifier = Modifier.fillMaxWidth().height(52.dp),
                    enabled = selected != null,
                    onClick = {
                        selected?.let {
                            sendingTo = it
                            onShare(it)
                        }
                    },
                ) {
                    Text(
                        selected?.let { "Share with ${peerLabel(state, it.npub)}" }
                            ?: "Choose a phone",
                    )
                }
                TextButton(modifier = Modifier.fillMaxWidth(), onClick = onDismiss) {
                    Text("Cancel")
                }
            } else {
                SendingProgress(sending, everStarted, onCancelTransfer)
                Spacer(Modifier.height(16.dp))
                TextButton(modifier = Modifier.fillMaxWidth(), onClick = onDismiss) {
                    Text(if (sending.any { it.isLive() }) "Hide" else "Done")
                }
            }
        }
    }
}

/**
 * Progress for the files just sent.
 *
 * The core drops an outgoing row only when the recipient acknowledges the
 * finished file — every other outcome leaves a row behind — so an empty list
 * *after* one existed is a confirmed delivery, and says so. Before the first
 * row appears it means the send has not been built yet, which is a different
 * message entirely.
 */
@Composable
private fun SendingProgress(
    sending: List<FileTransfer>,
    everStarted: Boolean,
    onCancel: (FileTransfer) -> Unit,
) {
    if (sending.isEmpty()) {
        Surface(
            modifier = Modifier.fillMaxWidth(),
            shape = RoundedCornerShape(18.dp),
            color = if (everStarted) {
                MaterialTheme.colorScheme.secondaryContainer
            } else {
                MaterialTheme.colorScheme.surfaceVariant
            },
        ) {
            Row(
                modifier = Modifier.padding(16.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (everStarted) {
                    Icon(
                        Icons.Filled.CheckCircle,
                        contentDescription = null,
                        tint = MaterialTheme.colorScheme.primary,
                    )
                    Spacer(Modifier.width(12.dp))
                    Text("Sent", fontWeight = FontWeight.Bold)
                } else {
                    CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                    Spacer(Modifier.width(12.dp))
                    Text("Preparing…", color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
        return
    }
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        sending.forEach { transfer ->
            Surface(
                modifier = Modifier.fillMaxWidth(),
                shape = RoundedCornerShape(18.dp),
                color = MaterialTheme.colorScheme.surfaceVariant,
            ) {
                Column(Modifier.padding(14.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        TransferAvatar(transfer)
                        Spacer(Modifier.width(12.dp))
                        Column(Modifier.weight(1f)) {
                            Text(
                                transfer.name,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                                fontWeight = FontWeight.Bold,
                            )
                            Text(
                                transferStage(transfer),
                                color = if (transfer.isLive()) {
                                    MaterialTheme.colorScheme.onSurfaceVariant
                                } else {
                                    MaterialTheme.colorScheme.error
                                },
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                        if (transfer.isLive()) {
                            TextButton(onClick = { onCancel(transfer) }) { Text("Cancel") }
                        }
                    }
                    if (transfer.isLive()) {
                        Spacer(Modifier.height(10.dp))
                        TransferProgressBar(transfer)
                    }
                }
            }
        }
    }
}

/**
 * The paired phones, reachable ones first. Offline members collapse behind one
 * line — the same shape the status sheet's Circle list uses, and for the same
 * reason: they all say "still paired, not here", which does not need a row each.
 * A file cannot reach them anyway until they come back.
 */
@Composable
private fun PairedPhonePicker(
    state: AppState,
    selectedNpub: String?,
    onSelect: (String) -> Unit,
) {
    val (online, offline) = state.circle
        .sortedBy { it.name.ifEmpty { it.npub }.lowercase() }
        .partition { contact ->
            contact.npub in state.reachableNpubs ||
                state.blePeers.any { it.npub == contact.npub && it.connected }
        }
    var showOffline by rememberSaveable { mutableStateOf(false) }

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
        return
    }

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        online.forEach { contact ->
            PeerShareRow(state, contact, contact.npub == selectedNpub, true) {
                onSelect(contact.npub)
            }
        }
        if (online.isEmpty() && !showOffline) {
            Text(
                "Nobody is reachable right now.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        if (offline.isNotEmpty()) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { showOffline = !showOffline }
                    .padding(vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text(
                    "${offline.size} offline",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodyMedium,
                )
                Text(
                    if (showOffline) "⌃" else "⌄",
                    color = MaterialTheme.colorScheme.primary,
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
            if (showOffline) {
                offline.forEach { contact ->
                    PeerShareRow(state, contact, contact.npub == selectedNpub, false) {
                        onSelect(contact.npub)
                    }
                }
            }
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
                        if (item.mimeType.startsWith("image/")) {
                            Icons.Filled.Photo
                        } else {
                            Icons.AutoMirrored.Filled.InsertDriveFile
                        },
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
                        Text(
                            formatSize(item.size),
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            style = MaterialTheme.typography.labelSmall,
                        )
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
    online: Boolean,
    onClick: () -> Unit,
) {
    val name = peerLabel(state, contact.npub)
    Surface(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .clickable(onClick = onClick),
        shape = RoundedCornerShape(16.dp),
        color = if (selected) {
            MaterialTheme.colorScheme.secondaryContainer
        } else {
            MaterialTheme.colorScheme.surfaceVariant
        },
        border = if (selected) {
            androidx.compose.foundation.BorderStroke(1.dp, MaterialTheme.colorScheme.primary)
        } else {
            null
        },
    ) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Row(modifier = Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                BoxAvatar(name, contact.npub)
                Spacer(Modifier.width(12.dp))
                Column {
                    Text(
                        name,
                        fontWeight = FontWeight.Bold,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Text(
                        if (online) "online · ready to receive" else "paired · currently offline",
                        color = if (online) {
                            StatusConnected
                        } else {
                            MaterialTheme.colorScheme.onSurfaceVariant
                        },
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
        Text(
            name.firstOrNull()?.uppercase() ?: "?",
            color = Color.White,
            fontWeight = FontWeight.Bold,
        )
    }
}
