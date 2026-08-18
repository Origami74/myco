package app.myco.ui.screens

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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.WifiTethering
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import app.myco.hotspot.HotspotPhase
import app.myco.hotspot.HotspotView
import app.myco.hotspot.SharedFiles
import app.myco.hotspot.WifiQr
import app.myco.share.NsiteShare

/**
 * The file-share hotspot sheet, opened from the Circle tab's hotspot bubble.
 *
 * Off: explains and offers to start. On: the two steps a guest walks —
 * (1) a `WIFI:` QR that joins this phone's hotspot, (2) the file page's
 * address — plus what's being shared and the stop button. The sheet only
 * *views* the hotspot; its lifetime belongs to
 * [app.myco.hotspot.HotspotService], so dismissing the sheet changes nothing.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HotspotSheet(
    view: HotspotView,
    shared: List<SharedFiles.Entry>,
    onStart: () -> Unit,
    onStop: () -> Unit,
    onAddFiles: () -> Unit,
    onDismiss: () -> Unit,
) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(start = 20.dp, end = 20.dp, bottom = 28.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    Icons.Filled.WifiTethering,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.primary,
                )
                Spacer(Modifier.width(8.dp))
                Text("Share files over hotspot", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
            }
            Spacer(Modifier.height(14.dp))

            when (view.phase) {
                HotspotPhase.OFF, HotspotPhase.ERROR -> {
                    if (view.phase == HotspotPhase.ERROR && view.error != null) {
                        Text(
                            view.error,
                            color = MaterialTheme.colorScheme.error,
                            style = MaterialTheme.typography.bodyMedium,
                            textAlign = TextAlign.Center,
                        )
                        Spacer(Modifier.height(12.dp))
                    }
                    Text(
                        "Opens a Wi-Fi hotspot on this phone and a web page on it. " +
                            "Anyone who joins — no Myco needed — can download the files " +
                            "you share and send you files back.",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.bodyMedium,
                        textAlign = TextAlign.Center,
                    )
                    Spacer(Modifier.height(16.dp))
                    Button(onClick = onStart) {
                        Text(if (view.phase == HotspotPhase.ERROR) "Try again" else "Start hotspot")
                    }
                }

                HotspotPhase.STARTING -> {
                    CircularProgressIndicator(modifier = Modifier.size(32.dp))
                    Spacer(Modifier.height(12.dp))
                    Text("Starting the hotspot…", color = MaterialTheme.colorScheme.onSurfaceVariant)
                }

                HotspotPhase.ON -> HotspotOn(view, shared, onStop, onAddFiles)
            }
        }
    }
}

@Composable
private fun HotspotOn(
    view: HotspotView,
    shared: List<SharedFiles.Entry>,
    onStop: () -> Unit,
    onAddFiles: () -> Unit,
) {
    val ssid = view.ssid.orEmpty()
    val pass = view.passphrase.orEmpty()
    val wifiQr = remember(ssid, pass) { NsiteShare.qrBitmap(WifiQr.payload(ssid, pass)) }

    StepLabel("1 · JOIN THIS PHONE'S WI-FI")
    Spacer(Modifier.height(10.dp))
    QrCodeCard(wifiQr, contentDescription = "Scan to join the hotspot")
    Spacer(Modifier.height(8.dp))
    Text(
        "Scan with the other phone's camera — or join manually:",
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        style = MaterialTheme.typography.bodySmall,
        textAlign = TextAlign.Center,
    )
    Spacer(Modifier.height(4.dp))
    Mono("$ssid · $pass")
    Spacer(Modifier.height(4.dp))
    Text(
        "This network is offline on purpose — if asked, choose to stay connected.",
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        style = MaterialTheme.typography.bodySmall,
        textAlign = TextAlign.Center,
    )

    Spacer(Modifier.height(18.dp))
    StepLabel("2 · OPEN THE FILE PAGE")
    Spacer(Modifier.height(8.dp))
    if (view.url != null) {
        Text(
            "In the browser there, go to:",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
        )
        Spacer(Modifier.height(4.dp))
        Mono(view.url, big = true)
    } else {
        Text(
            "Finding this phone's address…",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
        )
    }

    Spacer(Modifier.height(18.dp))
    StepLabel("SHARED FROM THIS PHONE")
    Spacer(Modifier.height(6.dp))
    Text(
        when {
            shared.isEmpty() -> "Nothing shared yet — files sent to you land in Download/Myco."
            else -> shared.joinToString(", ") { it.name }
        },
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        style = MaterialTheme.typography.bodySmall,
        textAlign = TextAlign.Center,
        maxLines = 3,
    )
    Spacer(Modifier.height(10.dp))
    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        OutlinedButton(onClick = onAddFiles) {
            Icon(Icons.Filled.Add, contentDescription = null, modifier = Modifier.size(16.dp))
            Spacer(Modifier.width(6.dp))
            Text("Add files")
        }
        TextButton(onClick = onStop) {
            Text("Stop hotspot", color = MaterialTheme.colorScheme.error, fontWeight = FontWeight.Bold)
        }
    }
}

@Composable
private fun StepLabel(text: String) {
    Text(
        text,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        fontWeight = FontWeight.Bold,
        style = MaterialTheme.typography.labelMedium,
    )
}

@Composable
private fun Mono(text: String, big: Boolean = false) {
    Surface(
        shape = RoundedCornerShape(10.dp),
        color = MaterialTheme.colorScheme.surfaceVariant,
    ) {
        Text(
            text,
            modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
            style = (if (big) MaterialTheme.typography.titleMedium else MaterialTheme.typography.bodyMedium)
                .copy(fontFamily = FontFamily.Monospace),
            fontWeight = FontWeight.Bold,
            textAlign = TextAlign.Center,
        )
    }
}
