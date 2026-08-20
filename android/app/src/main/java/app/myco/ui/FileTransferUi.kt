package app.myco.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.myco.core.FileTransfer
import app.myco.ui.theme.avatarColorFor

/**
 * Statuses a transfer can still move on from. Everything else is terminal and
 * belongs to a dismissable row rather than a live one.
 */
val ACTIVE_TRANSFER_STATUSES = setOf("offered", "accepted", "ready", "downloading")

/** Terminal states that carry something the user still needs to see. */
private val REPORTABLE_STATUSES = setOf("failed", "denied", "cancelled")

/** Whether this row is worth showing in the Circle tab's transfer section. */
fun FileTransfer.isLive(): Boolean = status in ACTIVE_TRANSFER_STATUSES

fun FileTransfer.needsAttention(): Boolean = status in REPORTABLE_STATUSES

/**
 * The incoming-offer prompt: a notification-style card that drops in from the
 * top rather than a modal dialog.
 *
 * A file offer is an interruption, not a decision the app is blocked on — the
 * modal it replaces covered whatever the user was already doing and could not
 * be dismissed with the back button. Sliding in over the status bar says the
 * same thing the platform's own notifications say, and leaves the app usable
 * underneath while it waits.
 */
@Composable
fun FileOfferBanner(
    offer: FileTransfer?,
    onAccept: (FileTransfer) -> Unit,
    onDeny: (FileTransfer) -> Unit,
    modifier: Modifier = Modifier,
) {
    AnimatedVisibility(
        visible = offer != null,
        enter = slideInVertically(animationSpec = tween(320)) { -it } + fadeIn(tween(320)),
        exit = slideOutVertically(animationSpec = tween(220)) { -it } + fadeOut(tween(220)),
        modifier = modifier,
    ) {
        // Kept after the offer clears so the exit animation has something to
        // draw; `visible` is what actually decides whether it is on screen.
        val shown = offer ?: return@AnimatedVisibility
        val peer = shown.peerName.ifBlank { "A paired phone" }
        Surface(
            modifier = Modifier
                .statusBarsPadding()
                .padding(horizontal = 12.dp, vertical = 8.dp)
                .fillMaxWidth(),
            shape = RoundedCornerShape(24.dp),
            tonalElevation = 6.dp,
            shadowElevation = 8.dp,
            color = MaterialTheme.colorScheme.secondaryContainer,
        ) {
            Column(Modifier.padding(horizontal = 16.dp, vertical = 14.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    TransferAvatar(shown, size = 40.dp)
                    Spacer(Modifier.width(12.dp))
                    Column(Modifier.weight(1f)) {
                        Text(
                            "$peer wants to send you a file",
                            fontWeight = FontWeight.Bold,
                            style = MaterialTheme.typography.bodyMedium,
                        )
                        Text(
                            transferSubtitle(shown),
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            color = MaterialTheme.colorScheme.onSecondaryContainer,
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
                Spacer(Modifier.height(6.dp))
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.End,
                ) {
                    TextButton(onClick = { onDeny(shown) }) { Text("Deny") }
                    TextButton(onClick = { onAccept(shown) }) {
                        Text("Accept", fontWeight = FontWeight.Bold)
                    }
                }
            }
        }
    }
}

/** Hosts [FileOfferBanner] above whatever else is on screen. */
@Composable
fun FileOfferLayer(
    offer: FileTransfer?,
    onAccept: (FileTransfer) -> Unit,
    onDeny: (FileTransfer) -> Unit,
) {
    Box(Modifier.fillMaxSize()) {
        FileOfferBanner(offer, onAccept, onDeny, Modifier.align(Alignment.TopCenter))
    }
}

/**
 * One transfer as it appears in the Circle tab, next to pairing requests —
 * transfers are the other thing that happens between two paired phones, and
 * like a pending invite they need somewhere durable to live once whatever
 * surface started them is gone.
 *
 * A live transfer offers cancel; a finished one that went wrong offers dismiss.
 */
@Composable
fun TransferCard(
    transfer: FileTransfer,
    onCancel: () -> Unit,
    onDismiss: () -> Unit,
) {
    val live = transfer.isLive()
    Surface(
        shape = RoundedCornerShape(18.dp),
        color = MaterialTheme.colorScheme.surfaceVariant,
        border = androidx.compose.foundation.BorderStroke(
            1.dp,
            if (live) MaterialTheme.colorScheme.outline else MaterialTheme.colorScheme.error,
        ),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(modifier = Modifier.padding(14.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                TransferAvatar(transfer, size = 40.dp)
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f)) {
                    Text(
                        transfer.name.ifBlank { "File" },
                        fontWeight = FontWeight.ExtraBold,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.titleSmall,
                    )
                    Text(
                        transferStage(transfer),
                        color = if (live) {
                            MaterialTheme.colorScheme.onSurfaceVariant
                        } else {
                            MaterialTheme.colorScheme.error
                        },
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                TextButton(onClick = if (live) onCancel else onDismiss) {
                    Text(if (live) "Cancel" else "Dismiss")
                }
            }
            if (live) {
                Spacer(Modifier.height(10.dp))
                TransferProgressBar(transfer)
            }
        }
    }
}

/**
 * Phase-based rather than byte-based: the native transport reports protocol
 * milestones, not a byte count, so an honest coarse bar beats a fake smooth one.
 */
@Composable
fun TransferProgressBar(transfer: FileTransfer, modifier: Modifier = Modifier) {
    LinearProgressIndicator(
        progress = { transferProgress(transfer) },
        modifier = modifier
            .fillMaxWidth()
            .height(6.dp)
            .clip(CircleShape),
        color = MaterialTheme.colorScheme.primary,
        trackColor = MaterialTheme.colorScheme.surface.copy(alpha = 0.6f),
    )
}

@Composable
fun TransferAvatar(transfer: FileTransfer, size: Dp = 40.dp) {
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
            style = if (size > 60.dp) {
                MaterialTheme.typography.displaySmall
            } else {
                MaterialTheme.typography.titleMedium
            },
            fontWeight = FontWeight.ExtraBold,
        )
    }
}

/** What this transfer is doing, in the user's terms. */
fun transferStage(transfer: FileTransfer): String {
    val peer = transfer.peerName.ifBlank { "your peer" }
    val outgoing = transfer.direction == "outgoing"
    return when (transfer.status) {
        "offered" -> "Waiting for $peer to accept"
        "waiting_user" -> "Waiting for you to decide"
        "accepted" -> if (outgoing) "Preparing secure transfer" else "Waiting for $peer's file"
        "ready" -> "Sending securely to $peer"
        "downloading" -> "Receiving from $peer"
        "completed" -> "Done"
        "denied" -> transfer.error.ifBlank { "Declined" }
        "cancelled" -> transfer.error.ifBlank { "Cancelled" }
        "failed" -> transfer.error.ifBlank { "Transfer failed" }
        else -> transfer.status
    }
}

/**
 * "photo.jpg · JPEG image · 2.4 MB".
 *
 * The kind is spelled out next to the name because the name alone is what the
 * bait-and-switch relies on: a prompt that only ever showed `holiday.jpg` gave
 * the user no way to notice they were agreeing to something else. App packages
 * are refused outright in the core, so this is the second layer, not the only
 * one.
 */
fun transferSubtitle(transfer: FileTransfer): String =
    listOfNotNull(
        transfer.name.takeIf { it.isNotBlank() },
        fileKindLabel(transfer),
        formatSize(transfer.size).takeIf { transfer.size > 0 },
    ).joinToString(" · ")

/** A short, human name for what this file is. */
fun fileKindLabel(transfer: FileTransfer): String {
    val extension = transfer.name.substringAfterLast('.', "").uppercase()
    return when {
        transfer.mime.startsWith("image/") -> "${extension.ifBlank { "Image" }} image"
        transfer.mime.startsWith("video/") -> "${extension.ifBlank { "Video" }} video"
        transfer.mime.startsWith("audio/") -> "${extension.ifBlank { "Audio" }} audio"
        transfer.mime == "application/pdf" -> "PDF document"
        transfer.mime.startsWith("text/") -> "${extension.ifBlank { "Text" }} text"
        extension.isNotBlank() -> "$extension file"
        else -> "File"
    }
}

fun transferProgress(transfer: FileTransfer): Float = when (transfer.status) {
    "offered", "waiting_user" -> 0.12f
    "accepted" -> 0.28f
    "ready", "downloading" -> 0.72f
    "completed" -> 1f
    else -> 0f
}
