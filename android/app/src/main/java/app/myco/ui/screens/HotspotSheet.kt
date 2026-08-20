package app.myco.ui.screens

import android.os.Build
import android.widget.Toast
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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.Link
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.Wifi
import androidx.compose.material.icons.filled.WifiTethering
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.dp
import app.myco.hotspot.HotspotPhase
import app.myco.hotspot.HotspotView
import app.myco.hotspot.Outbox
import app.myco.hotspot.SharedFiles
import app.myco.hotspot.WifiQr
import app.myco.share.NsiteShare
import com.google.zxing.qrcode.decoder.ErrorCorrectionLevel

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
    onShareFiles: () -> Unit,
    onDismiss: () -> Unit,
) {
    // Skip the half-height resting state: the two steps plus the file lists are
    // taller than a partially-expanded sheet, so at the default height step 2
    // sits below the fold and reads as a cut-off sheet rather than a scrollable
    // one. The flow only works if the guest can see both steps at once.
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
    ) {
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

                HotspotPhase.ON -> HotspotOn(view, shared, onStop, onShareFiles)
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun HotspotOn(
    view: HotspotView,
    shared: List<SharedFiles.Entry>,
    onStop: () -> Unit,
    onShareFiles: () -> Unit,
) {
    val ssid = view.ssid.orEmpty()
    val pass = view.passphrase.orEmpty()
    // H-level so the Wi-Fi badge in the middle stays recoverable.
    val wifiQr = remember(ssid, pass) {
        NsiteShare.qrBitmap(WifiQr.payload(ssid, pass), ecc = ErrorCorrectionLevel.H)
    }
    val urlQr = remember(view.url) {
        view.url?.let { NsiteShare.qrBitmap(it, ecc = ErrorCorrectionLevel.H) }
    }
    val context = LocalContext.current
    val offers by Outbox.get(context).offers.collectAsState()
    val clipboard = LocalClipboardManager.current
    // No universal plain-text format for Wi-Fi credentials exists — `WIFI:…` is a
    // QR payload that no messenger renders as anything but a string to decode by
    // hand. Two labelled lines are what someone can actually text a friend.
    val wifiText = "Wi-Fi: $ssid\nPass: $pass"
    val copy: (String, String) -> Unit = { label, text ->
        clipboard.setText(AnnotatedString(text))
        // Android 13+ pops its own clipboard preview; a second toast is noise.
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            Toast.makeText(context, "$label copied", Toast.LENGTH_SHORT).show()
        }
    }
    // The two steps happen one after the other — the page QR is useless until the
    // guest is already on the network — so only one code is ever on screen. The
    // other is a tap away, which keeps the whole sheet inside one screen.
    var showPageQr by remember { mutableStateOf(false) }
    val onPage = showPageQr && urlQr != null

    // A plain "show the other QR" button read as an optional extra, so nothing
    // told the guest that joining the Wi-Fi was only half of it. Both steps are
    // named up front instead: the sequence, and the fact that one is still ahead,
    // are visible without spending the height a second QR would cost.
    SingleChoiceSegmentedButtonRow(modifier = Modifier.fillMaxWidth()) {
        SegmentedButton(
            selected = !onPage,
            onClick = { showPageQr = false },
            shape = SegmentedButtonDefaults.itemShape(index = 0, count = 2),
        ) {
            Text("1 · Join Wi-Fi", style = MaterialTheme.typography.labelLarge)
        }
        SegmentedButton(
            selected = onPage,
            onClick = { showPageQr = true },
            // Nothing to show until the softap interface surfaces its address.
            enabled = urlQr != null,
            shape = SegmentedButtonDefaults.itemShape(index = 1, count = 2),
        ) {
            Text("2 · Share files", style = MaterialTheme.typography.labelLarge)
        }
    }
    Spacer(Modifier.height(12.dp))
    if (showPageQr && urlQr != null) {
        // The bump leads: it is the only route with nothing to scan and nothing to
        // type. It belongs to step 2 rather than beside the join QR, where it
        // invites tapping phones before the guest is on the network and the
        // address it hands over resolves to nothing. The QR under it is the
        // fallback.
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            NfcPulseBubble(size = 44.dp)
            Column {
                Text(
                    "Bump to open the page",
                    fontWeight = FontWeight.ExtraBold,
                    color = MaterialTheme.colorScheme.onSurface,
                    style = MaterialTheme.typography.titleSmall,
                )
                Text(
                    "Hold the backs together",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
        Spacer(Modifier.height(14.dp))
        QrCodeCard(
            urlQr,
            contentDescription = "Scan to open the file page",
            badge = Icons.Filled.Link,
            onClick = { copy("Link", view.url.orEmpty()) },
        )
        Spacer(Modifier.height(8.dp))
        // The scheme is the browser's job to add, and every character shown here
        // is one someone may end up typing.
        Mono(
            view.url.orEmpty().removePrefix("http://"),
            onClick = { copy("Link", view.url.orEmpty()) },
        )
    } else {
        QrCodeCard(
            wifiQr,
            contentDescription = "Scan to join the hotspot",
            badge = Icons.Filled.Wifi,
            onClick = { copy("Wi-Fi details", wifiText) },
        )
        Spacer(Modifier.height(8.dp))
        Mono("$ssid · $pass", onClick = { copy("Wi-Fi details", wifiText) })
        if (urlQr == null) {
            Spacer(Modifier.height(6.dp))
            Text(
                "Finding this phone's address…",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall,
            )
        }
    }

    Spacer(Modifier.height(16.dp))
    TransferSummary(offers, shared)
    Spacer(Modifier.height(10.dp))
    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        OutlinedButton(onClick = onShareFiles) {
            Icon(Icons.Filled.Share, contentDescription = null, modifier = Modifier.size(16.dp))
            Spacer(Modifier.width(6.dp))
            // Not "Share files" — that is what step 2 is called now, and this
            // button does the narrower thing of picking what to offer.
            Text("Add files")
        }
        TextButton(onClick = onStop) {
            Text("Stop hotspot", color = MaterialTheme.colorScheme.error, fontWeight = FontWeight.Bold)
        }
    }
}

/**
 * Transfers as a single line — "2 offered · 1 received" — that opens into the
 * per-file detail only when asked. Idle is the common case (the guest is still
 * connecting), and idle should cost one row rather than four lines of prose.
 */
@Composable
private fun TransferSummary(offers: List<Outbox.Offer>, shared: List<SharedFiles.Entry>) {
    var expanded by remember { mutableStateOf(false) }
    val hasAny = offers.isNotEmpty() || shared.isNotEmpty()
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .then(if (hasAny) Modifier.clickable { expanded = !expanded } else Modifier)
            .padding(vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.Center,
    ) {
        Text(
            if (hasAny) "${offers.size} offered · ${shared.size} received" else "No files yet",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
            fontWeight = if (hasAny) FontWeight.Bold else FontWeight.Normal,
        )
        if (hasAny) {
            Icon(
                if (expanded) Icons.Filled.ExpandLess else Icons.Filled.ExpandMore,
                contentDescription = if (expanded) "Hide files" else "Show files",
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.size(18.dp),
            )
        }
    }
    if (hasAny && expanded) {
        Spacer(Modifier.height(4.dp))
        Text(
            buildList {
                offers.forEach { offer ->
                    val status = when (offer.status) {
                        Outbox.Status.WAITING -> "waiting"
                        Outbox.Status.SENT -> "sent"
                        Outbox.Status.DECLINED -> "declined"
                    }
                    add("↑ ${offer.name} — $status")
                }
                shared.forEach { add("↓ ${it.name}") }
            }.joinToString("\n"),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
            textAlign = TextAlign.Center,
        )
    }
}


@Composable
private fun Mono(text: String, big: Boolean = false, onClick: (() -> Unit)? = null) {
    Surface(
        shape = RoundedCornerShape(10.dp),
        color = MaterialTheme.colorScheme.surfaceVariant,
        modifier = if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier,
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
