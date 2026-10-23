package app.myco.ui.screens

import android.graphics.Bitmap
import android.content.Intent
import android.os.Build
import android.widget.Toast
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.ContentPaste
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/**
 * Shared building blocks for the modal bottom-sheet menus (app long-press,
 * circle-person long-press) and the QR surfaces (pair / share).
 */

@OptIn(ExperimentalFoundationApi::class)
internal fun Modifier.combinedClickableSafe(onClick: () -> Unit): Modifier =
    this.combinedClickable(onClick = onClick)

/**
 * The head of an app's long-press sheet: its [tile], name and one quiet
 * [subtitle] line, with Open as the sheet's one button — the thing most people
 * long-pressed for is a tap away without reading a list.
 */
@Composable
internal fun AppSheetHeader(
    tile: @Composable () -> Unit,
    title: String,
    subtitle: String,
    subtitleColor: Color = MaterialTheme.colorScheme.onSurfaceVariant,
    onOpen: () -> Unit,
) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        tile()
        Spacer(Modifier.size(12.dp))
        Column(Modifier.weight(1f)) {
            Text(
                title,
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.Bold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                subtitle,
                color = subtitleColor,
                style = MaterialTheme.typography.bodySmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Spacer(Modifier.size(12.dp))
        Button(onClick = onOpen, contentPadding = PaddingValues(start = 14.dp, end = 18.dp)) {
            Icon(Icons.Filled.PlayArrow, contentDescription = null, modifier = Modifier.size(18.dp))
            Spacer(Modifier.size(6.dp))
            Text("Open")
        }
    }
}

/**
 * One action in a sheet's icon row: the icon on a tonal circle, a short label
 * under it. Give each one the same [modifier] weight so a row of three or four
 * spreads evenly.
 */
@Composable
internal fun SheetIconAction(
    icon: ImageVector,
    label: String,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        modifier = modifier
            .clip(RoundedCornerShape(12.dp))
            .combinedClickableSafe(onClick)
            .padding(vertical = 6.dp),
    ) {
        Surface(
            shape = CircleShape,
            color = MaterialTheme.colorScheme.secondaryContainer,
            modifier = Modifier.size(48.dp),
        ) {
            Row(horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
                Icon(icon, contentDescription = null, tint = MaterialTheme.colorScheme.onSecondaryContainer)
            }
        }
        Spacer(Modifier.height(6.dp))
        Text(
            label,
            style = MaterialTheme.typography.bodySmall,
            textAlign = TextAlign.Center,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** A single tappable row in a [androidx.compose.material3.ModalBottomSheet] menu:
 *  leading icon + label. Destructive actions pass the error color as [tint]. */
@Composable
internal fun SheetAction(
    icon: ImageVector,
    label: String,
    tint: Color = MaterialTheme.colorScheme.primary,
    onClick: () -> Unit,
) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .combinedClickableSafe(onClick)
            .padding(vertical = 12.dp),
    ) {
        Icon(icon, contentDescription = null, tint = tint)
        Spacer(Modifier.size(14.dp))
        Text(label, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/**
 * A share link as text, for the people who can't scan or tap: the [uri],
 * cut to one line, with Copy and Share. Both hand over the bare link —
 * "Paste a link" on the other phone reads exactly that, not a sentence
 * around it.
 */
@Composable
internal fun ShareLinkRow(uri: String, title: String) {
    val context = LocalContext.current
    val clipboard = LocalClipboardManager.current
    Surface(
        shape = RoundedCornerShape(14.dp),
        color = MaterialTheme.colorScheme.surface,
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier.padding(start = 14.dp, end = 4.dp),
        ) {
            Text(
                uri,
                style = MaterialTheme.typography.bodySmall,
                fontFamily = FontFamily.Monospace,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            IconButton(onClick = {
                clipboard.setText(AnnotatedString(uri))
                // Android 13+ confirms a copy itself; a toast on top doubles it.
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
                    Toast.makeText(context, "Link copied", Toast.LENGTH_SHORT).show()
                }
            }) {
                Icon(Icons.Filled.ContentCopy, contentDescription = "Copy link", tint = MaterialTheme.colorScheme.primary)
            }
            IconButton(onClick = {
                val send = Intent(Intent.ACTION_SEND)
                    .setType("text/plain")
                    .putExtra(Intent.EXTRA_TEXT, uri)
                    .putExtra(Intent.EXTRA_SUBJECT, title)
                context.startActivity(Intent.createChooser(send, "Share $title"))
            }) {
                Icon(Icons.Filled.Share, contentDescription = "Share link", tint = MaterialTheme.colorScheme.primary)
            }
        }
    }
}

/**
 * The white, rounded QR "card" shown on the pair and share surfaces — a QR
 * bitmap padded on white so it scans well against the soft panel behind it.
 *
 * [badge] punches a glyph through the middle of the code, which is how a screen
 * full of otherwise-identical black squares tells the reader what it *is* — a
 * Wi-Fi network versus a link. Only pass one for a bitmap encoded at
 * [com.google.zxing.qrcode.decoder.ErrorCorrectionLevel.H]: the badge covers
 * real modules, and only H's ~30% redundancy reconstructs them. The badge sits
 * on its own white plate so the scanner sees a clean quiet zone around the
 * glyph rather than a glyph bleeding into the surrounding modules.
 */
@Composable
internal fun QrCodeCard(
    bitmap: Bitmap,
    contentDescription: String,
    size: Dp = 180.dp,
    badge: ImageVector? = null,
    onClick: (() -> Unit)? = null,
) {
    androidx.compose.foundation.layout.Box(
        modifier = Modifier
            .clip(RoundedCornerShape(16.dp))
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .background(Color.White)
            .padding(14.dp),
        contentAlignment = Alignment.Center,
    ) {
        Image(remember(bitmap) { bitmap.asImageBitmap() }, contentDescription = contentDescription, modifier = Modifier.size(size))
        if (badge != null) {
            // ~22% of the code: big enough to read across a table, small enough
            // that H-level recovery still has margin to spare.
            val plate = size * 0.22f
            androidx.compose.foundation.layout.Box(
                modifier = Modifier
                    .size(plate)
                    .clip(RoundedCornerShape(plate * 0.28f))
                    .background(Color.White)
                    .padding(plate * 0.14f),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    badge,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.size(plate * 0.72f),
                )
            }
        }
    }
}

/** The outlined "paste a code/link from the clipboard" button used under the
 *  scanners (QR screen, add-app sheet). Empty clipboard shows a toast. */
@Composable
internal fun PasteCodeButton(label: String, onPaste: (String) -> Unit) {
    val clipboard = LocalClipboardManager.current
    val context = LocalContext.current
    Surface(
        shape = RoundedCornerShape(14.dp),
        color = MaterialTheme.colorScheme.surface,
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
        modifier = Modifier.fillMaxWidth().height(48.dp).clickable {
            val text = clipboard.getText()?.text?.trim().orEmpty()
            if (text.isNotEmpty()) onPaste(text)
            else Toast.makeText(context, "Clipboard is empty", Toast.LENGTH_SHORT).show()
        },
    ) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.Center,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Filled.ContentPaste, contentDescription = null, tint = MaterialTheme.colorScheme.primary, modifier = Modifier.size(18.dp))
            Spacer(Modifier.size(8.dp))
            Text(label, color = MaterialTheme.colorScheme.onSurface, fontWeight = FontWeight.Bold, style = MaterialTheme.typography.labelLarge)
        }
    }
}
