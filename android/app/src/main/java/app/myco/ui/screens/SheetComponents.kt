package app.myco.ui.screens

import android.graphics.Bitmap
import android.widget.Toast
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ContentPaste
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
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

/** The white, rounded QR "card" shown on the pair and share surfaces — a QR
 *  bitmap padded on white so it scans well against the soft panel behind it. */
@Composable
internal fun QrCodeCard(bitmap: Bitmap, contentDescription: String, size: Dp = 180.dp) {
    androidx.compose.foundation.layout.Box(
        modifier = Modifier.clip(RoundedCornerShape(16.dp)).background(Color.White).padding(14.dp),
    ) {
        Image(bitmap.asImageBitmap(), contentDescription = contentDescription, modifier = Modifier.size(size))
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
        color = Color.White,
        border = BorderStroke(1.dp, Color(0xFFCBD5E1)),
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
            Icon(Icons.Filled.ContentPaste, contentDescription = null, tint = Color(0xFF334155), modifier = Modifier.size(18.dp))
            Spacer(Modifier.size(8.dp))
            Text(label, color = Color(0xFF334155), fontWeight = FontWeight.Bold, style = MaterialTheme.typography.labelLarge)
        }
    }
}
