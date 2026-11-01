package app.myco.ui.screens

import android.content.ActivityNotFoundException
import android.util.Log
import android.widget.Toast
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.WifiTethering
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import app.myco.share.MycoApk
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * The Apps tab's Share Myco bubble: a labelled pill rather than a bare icon,
 * so "you can give this app to someone" reads without a tap.
 */
@Composable
fun ShareMycoBubble(onClick: () -> Unit, modifier: Modifier = Modifier) {
    Surface(
        shape = CircleShape,
        color = MaterialTheme.colorScheme.primary,
        shadowElevation = 6.dp,
        modifier = modifier.clickable(onClick = onClick),
    ) {
        Row(
            modifier = Modifier.padding(start = 16.dp, end = 20.dp, top = 14.dp, bottom = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(
                Icons.Filled.Share,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onPrimary,
                modifier = Modifier.size(22.dp),
            )
            Spacer(Modifier.width(8.dp))
            Text(
                "Share Myco",
                color = MaterialTheme.colorScheme.onPrimary,
                fontWeight = FontWeight.Bold,
                style = MaterialTheme.typography.labelLarge,
            )
        }
    }
}

/**
 * Give Myco itself to a phone that doesn't have it: over this phone's
 * hotspot (no internet; the guest downloads it from the hotspot page), or as
 * a file through the system share menu.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ShareMycoSheet(onHotspot: () -> Unit, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val apk = remember { MycoApk.info(context) }
    val scope = rememberCoroutineScope()
    var preparing by remember { mutableStateOf(false) }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(start = 20.dp, end = 20.dp, bottom = 28.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text("Share Myco", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
            Spacer(Modifier.height(6.dp))
            Text(
                "Give the app to someone nearby who doesn't have it yet. " +
                    "Their phone needs Android 10 or newer" +
                    (if (apk?.only64Bit == true) ", 64-bit." else "."),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodyMedium,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(18.dp))

            if (apk == null) {
                // A split install has no single file to hand over.
                Text(
                    "This copy of Myco was installed in parts, so it can't be passed on " +
                        "as one file. Get Myco from Zapstore or GitHub instead.",
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodyMedium,
                    textAlign = TextAlign.Center,
                )
                return@Column
            }

            ShareMycoOption(
                icon = Icons.Filled.WifiTethering,
                title = "Over a hotspot",
                detail = "No internet needed. They join this phone's Wi-Fi and " +
                    "download Myco in their browser.",
                onClick = onHotspot,
            )
            Spacer(Modifier.height(10.dp))
            ShareMycoOption(
                icon = Icons.Filled.Share,
                title = "Send the app file",
                detail = "Quick Share, Bluetooth, a messenger — whatever this phone offers.",
                busy = preparing,
                onClick = {
                    if (preparing) return@ShareMycoOption
                    preparing = true
                    scope.launch {
                        // Copying ~100 MB into the cache; not on the main thread.
                        val intent = withContext(Dispatchers.IO) {
                            runCatching { MycoApk.shareIntent(context) }
                                .onFailure { Log.w("MycoShare", "preparing the APK to share failed", it) }
                                .getOrNull()
                        }
                        preparing = false
                        try {
                            if (intent == null) error("no intent")
                            context.startActivity(intent)
                            onDismiss()
                        } catch (e: Exception) {
                            val msg = if (e is ActivityNotFoundException) "Nothing on this phone can share it"
                            else "Couldn't prepare the app file"
                            Toast.makeText(context, msg, Toast.LENGTH_SHORT).show()
                        }
                    }
                },
            )
            Spacer(Modifier.height(14.dp))
            Text(
                "Myco ${apk.version} · ${"%.0f MB".format(apk.size / 1_048_576.0)}",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall,
            )
            if (!apk.release) {
                // Not signed with the release key: whoever installs it can't
                // update from Zapstore or GitHub over it.
                Spacer(Modifier.height(4.dp))
                Text(
                    "Development build — it won't update from Zapstore or GitHub.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                    textAlign = TextAlign.Center,
                )
            }
        }
    }
}

@Composable
private fun ShareMycoOption(
    icon: ImageVector,
    title: String,
    detail: String,
    onClick: () -> Unit,
    busy: Boolean = false,
) {
    Surface(
        shape = RoundedCornerShape(16.dp),
        color = MaterialTheme.colorScheme.surfaceVariant,
        modifier = Modifier.fillMaxWidth().clickable(onClick = onClick),
    ) {
        Row(
            modifier = Modifier.padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Box(Modifier.size(28.dp), contentAlignment = Alignment.Center) {
                if (busy) {
                    CircularProgressIndicator(modifier = Modifier.size(22.dp), strokeWidth = 2.dp)
                } else {
                    Icon(icon, contentDescription = null, tint = MaterialTheme.colorScheme.primary)
                }
            }
            Column {
                Text(title, fontWeight = FontWeight.Bold, style = MaterialTheme.typography.titleSmall)
                Text(
                    if (busy) "Preparing the app file…" else detail,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
    }
}
