package app.myco.neighbor

import android.content.Intent
import android.widget.Toast
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
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
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
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
 * **Invite people** — the "Share this app" sheet's shape. Pick which
 * neighborhoods the invitation carries (chips), then: a QR panel with the
 * link to copy or send, and tap-phones-together, which presents the same
 * invitation over NFC while the sheet is up. [byLink] leads with sending the
 * link instead, for someone who isn't here. Shows how long it still works,
 * waits visibly, and closes with "<name> joined" once someone does.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun InviteSheet(
    state: AppState,
    client: AppCoreClient,
    byLink: Boolean,
    preselect: String?,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val from = remember(state.ownNpub) { DeviceName.current(context, state.ownNpub) }
    val choices = state.neighborhoods.list.filter { !it.everyone }
    val picked = remember {
        mutableStateListOf<String>().also { l ->
            (preselect ?: choices.firstOrNull()?.id)?.let { l.add(it) }
        }
    }
    // A fresh one-time invitation whenever what it carries changes.
    LaunchedEffect(picked.toList()) {
        if (picked.isNotEmpty()) client.dispatch(NativeActions.createLinkInvite(picked.toList(), from))
        else client.dispatch(NativeActions.clearLinkInvite())
    }
    val uri = state.neighborhoods.invite.takeIf { picked.isNotEmpty() }
    val preview = remember(uri) { uri?.let { InvitePreview.read(it) } }
    if (!byLink) {
        DisposableEffect(uri) {
            if (uri != null) PairPresent.beginRaw(uri)
            onDispose { PairPresent.stop() }
        }
    }
    // Ticks the countdown.
    var now by remember { mutableLongStateOf(System.currentTimeMillis() / 1000) }
    LaunchedEffect(Unit) {
        while (true) {
            kotlinx.coroutines.delay(1000)
            now = System.currentTimeMillis() / 1000
        }
    }
    // Someone joining is a new connection: say who, and close.
    val baseline = remember { state.neighborhoods.links.map { it.id }.toSet() }
    LaunchedEffect(state.neighborhoods.links) {
        val joined = state.neighborhoods.links.firstOrNull { it.id !in baseline }
        if (joined != null) {
            Toast.makeText(context, "${joined.name} joined", Toast.LENGTH_SHORT).show()
            client.dispatch(NativeActions.clearLinkInvite())
            onDismiss()
        }
    }
    val dismiss = {
        client.dispatch(NativeActions.clearLinkInvite())
        onDismiss()
    }
    val names = choices.filter { it.id in picked }.joinToString(", ") { it.name }
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(onDismissRequest = dismiss, sheetState = sheetState) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 20.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(
                if (byLink) "Send an invite" else "Invite in person",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.Bold,
            )
            Spacer(Modifier.height(12.dp))
            Text(
                "To",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.fillMaxWidth(),
            )
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
                for (n in choices) {
                    FilterChip(
                        selected = n.id in picked,
                        onClick = { if (n.id in picked) picked.remove(n.id) else picked.add(n.id) },
                        label = { Text(n.name) },
                    )
                }
            }
            Spacer(Modifier.height(8.dp))
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
                    when {
                        picked.isEmpty() -> Text("Pick at least one neighborhood.", Modifier.padding(24.dp))
                        uri == null -> Box(Modifier.size(if (byLink) 48.dp else 210.dp), contentAlignment = Alignment.Center) {
                            CircularProgressIndicator()
                        }
                        !byLink -> {
                            val qr = remember(uri) { NsiteShare.qrBitmap(uri) }
                            QrCodeCard(qr, contentDescription = "Invitation to $names", size = 210.dp)
                        }
                    }
                    if (uri != null) {
                        Spacer(Modifier.height(10.dp))
                        Text(names, fontWeight = FontWeight.ExtraBold, style = MaterialTheme.typography.titleMedium, textAlign = TextAlign.Center)
                        Spacer(Modifier.height(2.dp))
                        Text(
                            if (byLink) "They open it in Myco to join" else "Scan with Myco to join",
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            style = MaterialTheme.typography.bodySmall,
                        )
                        val left = (preview?.exp ?: 0) - now
                        Spacer(Modifier.height(6.dp))
                        Text(
                            if (left > 0) "Works once · %d:%02d left".format(left / 60, left % 60) else "Expired — pick again to renew",
                            color = Color(0xFFB45309),
                            fontWeight = FontWeight.Bold,
                            style = MaterialTheme.typography.labelMedium,
                        )
                        Spacer(Modifier.height(12.dp))
                        ShareLinkRow(uri = uri, title = "Join $names")
                    }
                }
            }
            Spacer(Modifier.height(12.dp))
            if (byLink && uri != null) {
                Button(
                    onClick = {
                        val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, uri)
                        context.startActivity(Intent.createChooser(send, "Send invite"))
                    },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text("Send invite…") }
            } else if (!byLink) {
                TapCard("Or tap phones together", "Hold the backs together — no scan needed")
            }
            if (uri != null) {
                Spacer(Modifier.height(12.dp))
                Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
                    CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
                    Spacer(Modifier.size(10.dp))
                    Text(
                        if (byLink) "Waiting for them to open it…" else "Waiting for them to scan…",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            }
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
                "Pick what to join. You'll be connected with them.",
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
            Surface(
                shape = RoundedCornerShape(16.dp),
                color = Color(0xFFFFFBEB),
                border = BorderStroke(1.dp, Color(0xFFFDE68A)),
                modifier = Modifier.fillMaxWidth(),
            ) {
                Column(Modifier.padding(14.dp)) {
                    Text("Not private", fontWeight = FontWeight.Bold, color = Color(0xFF92400E))
                    Text(
                        "People in a neighborhood can pass posts on, even outside it. Share like you're in a public room.",
                        style = MaterialTheme.typography.bodySmall,
                        color = Color(0xFF92400E),
                    )
                }
            }
            Spacer(Modifier.height(4.dp))
            val chosen = invite.offered.filter { it.id in accept }
            Button(
                onClick = {
                    client.dispatch(NativeActions.joinLink(invite.raw, accept.toList()))
                    onDismiss()
                },
                enabled = invite.offered.isEmpty() || chosen.isNotEmpty(),
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text(
                    when (chosen.size) {
                        0 -> if (invite.offered.isEmpty()) "Connect" else "Pick at least one"
                        1 -> "Join ${chosen.first().name}"
                        else -> "Join ${chosen.size} neighborhoods"
                    },
                )
            }
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
