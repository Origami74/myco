package app.myco.neighbor

import android.widget.Toast
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.NativeActions
import app.myco.nfc.NfcReader
import app.myco.nfc.PairPresent
import app.myco.share.DeviceName
import app.myco.share.NsiteShare
import app.myco.ui.screens.NfcPulseBubble
import app.myco.ui.screens.PasteCodeButton
import app.myco.ui.screens.QrCodeCard
import app.myco.ui.screens.ScanPanel
import app.myco.ui.screens.ShareLinkRow

/**
 * **Invite to a neighborhood** — the "Share this app" sheet's shape: a QR
 * panel with the link to copy or send, and tap-phones-together, which
 * presents the same `myco://link/…` invitation over NFC while the sheet is up.
 * Closes itself once someone joins.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun InviteSheet(
    neighborhood: NeighborhoodsState.Neighborhood,
    state: AppState,
    client: AppCoreClient,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val from = remember(state.ownNpub) { DeviceName.current(context, state.ownNpub) }
    // A fresh one-time invitation each time the sheet opens.
    LaunchedEffect(neighborhood.id) {
        client.dispatch(NativeActions.createLinkInvite(listOf(neighborhood.id), from))
    }
    val uri = state.neighborhoods.invite
    DisposableEffect(uri) {
        if (uri != null) PairPresent.beginRaw(uri)
        onDispose { PairPresent.stop() }
    }
    // Someone joining is a new link: the invitation has done its job.
    val baselineLinks = remember { state.neighborhoods.links.size }
    LaunchedEffect(state.neighborhoods.links.size) {
        if (state.neighborhoods.links.size > baselineLinks) onDismiss()
    }
    val dismiss = {
        client.dispatch(NativeActions.clearLinkInvite())
        onDismiss()
    }
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(onDismissRequest = dismiss, sheetState = sheetState) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 20.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text("Invite to ${neighborhood.name}", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
            Spacer(Modifier.height(12.dp))
            Surface(
                shape = RoundedCornerShape(20.dp),
                color = MaterialTheme.colorScheme.surfaceVariant,
                border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
                modifier = Modifier.fillMaxWidth(),
            ) {
                Column(
                    modifier = Modifier.fillMaxWidth().padding(16.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    if (uri == null) {
                        Box(Modifier.size(210.dp), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
                    } else {
                        val qr = remember(uri) { NsiteShare.qrBitmap(uri) }
                        QrCodeCard(qr, contentDescription = "Invitation to ${neighborhood.name}", size = 210.dp)
                    }
                    Spacer(Modifier.height(10.dp))
                    Text(neighborhood.name, fontWeight = FontWeight.ExtraBold, style = MaterialTheme.typography.titleMedium)
                    Spacer(Modifier.height(2.dp))
                    Text(
                        "Scan to join — works once, for 15 minutes",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.bodySmall,
                        textAlign = TextAlign.Center,
                    )
                    if (uri != null) {
                        Spacer(Modifier.height(12.dp))
                        ShareLinkRow(uri = uri, title = "Join ${neighborhood.name}")
                    }
                }
            }
            Spacer(Modifier.height(12.dp))
            TapCard("Tap phones together to invite", "Hold the backs together — no scan needed")
        }
    }
}

/**
 * **Join a neighborhood** — the "Add an app" sheet's shape: a live scanner,
 * tap a friend's phone, or paste an invitation. Whatever arrives goes
 * through [onScanned] (MainActivity's one router for `myco://` codes), which
 * opens the confirm sheet.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun JoinSheet(onScanned: (String) -> Unit, onDismiss: () -> Unit) {
    val context = LocalContext.current
    DisposableEffect(Unit) {
        NfcReader.begin()
        onDispose { NfcReader.stop() }
    }
    // An invitation read by NFC lands in MainActivity, not here: close when
    // one is waiting to be confirmed.
    val incoming = IncomingInvite.pending.value
    LaunchedEffect(incoming) { if (incoming != null) onDismiss() }
    val take: (String) -> Unit = { text ->
        if (InvitePreview.read(text) != null) {
            onScanned(text)
        } else {
            Toast.makeText(context, "That isn't a neighborhood invitation", Toast.LENGTH_SHORT).show()
        }
    }
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 20.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text("Join a neighborhood", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
            Spacer(Modifier.height(12.dp))
            Box(modifier = Modifier.fillMaxWidth().height(300.dp)) {
                ScanPanel(onScanned = take)
            }
            Spacer(Modifier.height(12.dp))
            TapCard("Or tap a friend's phone", "Hold the backs together to join")
            Spacer(Modifier.height(12.dp))
            PasteCodeButton(label = "Paste an invitation", onPaste = take)
        }
    }
}

/**
 * The invitation that just arrived: who it's from and which neighborhoods
 * it carries, each kept or not, then Join.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun JoinConfirmSheet(invite: InvitePreview, client: AppCoreClient, onDismiss: () -> Unit) {
    val accept = remember(invite) { mutableStateListOf<String>().also { l -> invite.offered.forEach { l.add(it.id) } } }
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                if (invite.from.isNotBlank()) "${invite.from} invites you" else "You're invited",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.Bold,
                modifier = Modifier.align(Alignment.CenterHorizontally),
            )
            Text(
                "What you join is carried with them; they and whoever they share it with can pass posts on.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth(),
            )
            Surface(
                shape = RoundedCornerShape(20.dp),
                color = MaterialTheme.colorScheme.surfaceVariant,
                border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
                modifier = Modifier.fillMaxWidth(),
            ) {
                Column(Modifier.padding(8.dp)) {
                    if (invite.offered.isEmpty()) {
                        Text("Just a link with them, no neighborhoods yet.", Modifier.padding(8.dp))
                    }
                    for (o in invite.offered) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Checkbox(checked = o.id in accept, onCheckedChange = { on ->
                                if (on) accept.add(o.id) else accept.remove(o.id)
                            })
                            Text(o.name, fontWeight = FontWeight.SemiBold)
                        }
                    }
                }
            }
            Spacer(Modifier.height(4.dp))
            Button(
                onClick = {
                    client.dispatch(NativeActions.joinLink(invite.raw, accept.toList()))
                    onDismiss()
                },
                modifier = Modifier.fillMaxWidth(),
            ) { Text("Join") }
            TextButton(onClick = onDismiss, modifier = Modifier.align(Alignment.CenterHorizontally)) { Text("Not now") }
        }
    }
}

/** The breathing NFC card both sheets end on, as on the app share sheets. */
@Composable
private fun TapCard(title: String, subtitle: String) {
    Surface(shape = RoundedCornerShape(16.dp), color = MaterialTheme.colorScheme.primaryContainer, modifier = Modifier.fillMaxWidth()) {
        Row(
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            NfcPulseBubble(size = 44.dp)
            Column {
                Text(title, fontWeight = FontWeight.ExtraBold, color = MaterialTheme.colorScheme.onPrimaryContainer, style = MaterialTheme.typography.titleSmall)
                Text(subtitle, color = MaterialTheme.colorScheme.onPrimaryContainer, style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}
