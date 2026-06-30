package app.myco.ui.screens

import android.graphics.Bitmap
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
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
