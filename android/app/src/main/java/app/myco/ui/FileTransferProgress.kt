package app.myco.ui

import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
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
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
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
import app.myco.core.FileTransfer
import app.myco.ui.theme.avatarColorFor

/**
 * App-root transfer feedback. It is intentionally phase-based for now: the
 * native transport currently reports protocol milestones, not byte counts.
 */
@Composable
fun FileTransferProgressOverlay(transfers: List<FileTransfer>) {
    var hiddenOutgoing by remember { mutableStateOf<Set<String>>(emptySet()) }
    val outgoing = transfers
        .filter { it.direction == "outgoing" && it.status in ACTIVE_OUTGOING && it.id !in hiddenOutgoing }
        .maxByOrNull { it.updatedAt }
    val incoming = transfers
        .filter { it.direction == "incoming" && it.status in ACTIVE_INCOMING }
        .maxByOrNull { it.updatedAt }

    Box(Modifier.fillMaxSize()) {
        incoming?.let { IncomingTransferBanner(it, Modifier.align(Alignment.TopCenter)) }
        outgoing?.let { transfer ->
            OutgoingTransferDialog(transfer, onHide = { hiddenOutgoing = hiddenOutgoing + transfer.id })
        }
    }
}

private val ACTIVE_OUTGOING = setOf("offered", "accepted", "ready")
private val ACTIVE_INCOMING = setOf("accepted", "downloading")

@Composable
private fun OutgoingTransferDialog(transfer: FileTransfer, onHide: () -> Unit) {
    val progress by animateFloatAsState(
        targetValue = transferProgress(transfer),
        animationSpec = tween(750, easing = FastOutSlowInEasing),
        label = "file-send-progress",
    )
    val peer = transfer.peerName.ifBlank { "your peer" }
    val stage = when (transfer.status) {
        "offered" -> "Waiting for $peer to accept"
        "accepted" -> "Preparing secure transfer"
        else -> "Sending securely to $peer"
    }

    AlertDialog(
        onDismissRequest = {},
        title = { Text("Sending to $peer") },
        text = {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                TransferAvatar(transfer)
                Spacer(Modifier.height(16.dp))
                Text(
                    transfer.name,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    fontWeight = FontWeight.Bold,
                )
                Spacer(Modifier.height(12.dp))
                CircularProgressIndicator(
                    progress = { progress },
                    modifier = Modifier.size(92.dp),
                    strokeWidth = 7.dp,
                    color = MaterialTheme.colorScheme.primary,
                    trackColor = MaterialTheme.colorScheme.surfaceVariant,
                )
                Spacer(Modifier.height(10.dp))
                Text(stage, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        },
        confirmButton = {
            TextButton(onClick = onHide) { Text("Hide") }
        },
    )
}

@Composable
private fun IncomingTransferBanner(transfer: FileTransfer, modifier: Modifier = Modifier) {
    val progress by animateFloatAsState(
        targetValue = transferProgress(transfer),
        animationSpec = tween(750, easing = FastOutSlowInEasing),
        label = "file-receive-progress",
    )
    val peer = transfer.peerName.ifBlank { "your peer" }
    Surface(
        modifier = modifier
            .statusBarsPadding()
            .padding(horizontal = 12.dp, vertical = 10.dp)
            .fillMaxWidth(),
        shape = RoundedCornerShape(20.dp),
        tonalElevation = 5.dp,
        color = MaterialTheme.colorScheme.secondaryContainer,
    ) {
        Column(Modifier.padding(horizontal = 16.dp, vertical = 12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                TransferAvatar(transfer, size = 42.dp)
                Column(Modifier.padding(start = 12.dp).weight(1f)) {
                    Text("Receiving from $peer", fontWeight = FontWeight.Bold)
                    Text(
                        "Getting ${transfer.name}",
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        color = MaterialTheme.colorScheme.onSecondaryContainer,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                Text(
                    "${(progress * 100).toInt()}%",
                    color = MaterialTheme.colorScheme.onSecondaryContainer,
                    style = MaterialTheme.typography.labelLarge,
                )
            }
            Spacer(Modifier.height(10.dp))
            LinearProgressIndicator(
                progress = { progress },
                modifier = Modifier.fillMaxWidth().height(6.dp).clip(CircleShape),
                color = MaterialTheme.colorScheme.primary,
                trackColor = MaterialTheme.colorScheme.surface.copy(alpha = 0.6f),
            )
            Spacer(Modifier.height(4.dp))
            Text(
                "Keep Myco open while the file arrives",
                color = MaterialTheme.colorScheme.onSecondaryContainer,
                style = MaterialTheme.typography.labelSmall,
            )
        }
    }
}

@Composable
private fun TransferAvatar(transfer: FileTransfer, size: androidx.compose.ui.unit.Dp = 112.dp) {
    val name = transfer.peerName.ifBlank { "Peer" }
    Box(
        modifier = Modifier
            .size(size)
            .clip(CircleShape)
            .background(avatarColorFor(transfer.peerNpub)),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            name.firstOrNull()?.uppercase() ?: "?",
            color = Color.White,
            style = if (size > 60.dp) MaterialTheme.typography.displaySmall else MaterialTheme.typography.titleMedium,
            fontWeight = FontWeight.ExtraBold,
        )
    }
}

private fun transferProgress(transfer: FileTransfer): Float = when (transfer.status) {
    "offered" -> 0.12f
    "accepted" -> 0.28f
    "ready" -> 0.72f
    "downloading" -> 0.72f
    "completed" -> 1f
    else -> 0f
}
