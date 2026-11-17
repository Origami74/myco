package app.myco.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.myco.core.AppState
import app.myco.neighbor.IncomingInvite
import app.myco.nfc.NfcReader

/**
 * **Scan** — the one sheet behind the scan bubble on Apps and on Neighbors.
 * A live camera scanner, tap phones together, or paste: an app share and a
 * neighborhood invitation alike go through [onScanned] (MainActivity's one
 * router for codes), which knows what each is.
 *
 * Closes on a scan or paste, and also when an NFC tap (read by the Activity,
 * not here) brought an invitation to confirm or a new app.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ScanSheet(state: AppState, onScanned: (String) -> Unit, onDismiss: () -> Unit) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    // Claim foreground NFC reader mode while we're up — a modal sheet's own window
    // misses the OS's passive tap dispatch, so the Activity reads the tag directly.
    DisposableEffect(Unit) {
        NfcReader.begin()
        onDispose { NfcReader.stop() }
    }
    val apps = state.sites.size + state.library.size
    val baseline = remember { apps }
    LaunchedEffect(apps) { if (apps > baseline) onDismiss() }
    val incoming = IncomingInvite.pending.value
    LaunchedEffect(incoming) { if (incoming != null) onDismiss() }
    val take: (String) -> Unit = { text ->
        onDismiss()
        onScanned(text)
    }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 20.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text("Scan a code", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
            Text(
                "An app someone shares, or an invitation to a neighborhood",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(12.dp))
            Box(modifier = Modifier.fillMaxWidth().height(300.dp)) {
                ScanPanel(onScanned = take)
            }
            Spacer(Modifier.height(12.dp))
            Surface(
                shape = RoundedCornerShape(16.dp),
                color = MaterialTheme.colorScheme.primaryContainer,
                modifier = Modifier.fillMaxWidth(),
            ) {
                Row(
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(14.dp),
                ) {
                    NfcPulseBubble(size = 44.dp)
                    Column {
                        Text(
                            "Or tap a friend's phone",
                            fontWeight = FontWeight.ExtraBold,
                            color = MaterialTheme.colorScheme.onPrimaryContainer,
                            style = MaterialTheme.typography.titleSmall,
                        )
                        Text(
                            "Hold the backs together",
                            color = MaterialTheme.colorScheme.onPrimaryContainer,
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            }
            Spacer(Modifier.height(12.dp))
            PasteCodeButton(label = "Paste a link or invitation", onPaste = take)
        }
    }
}
