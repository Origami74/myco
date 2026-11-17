package app.myco.neighbor

import android.widget.Toast
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
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
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
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
 * **Invite** — one sheet, no steps before the code: it opens on the
 * neighborhood you invited to last (or the one you came from), and shows the
 * QR, the link to copy or send, and
 * tap-phones-together all at once. With no neighborhood yet it asks for a
 * name first and goes straight to the code. Counts down while it works,
 * waits visibly, and closes with "<name> joined". What the invitation
 * carries is a list of big tick rows at the bottom, in thumb reach.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun InviteSheet(
    state: AppState,
    client: AppCoreClient,
    /** The neighborhoods it starts out carrying; empty means the last used. */
    preselect: List<String>,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val prefs = remember { context.getSharedPreferences("neighbors", android.content.Context.MODE_PRIVATE) }
    val from = remember(state.ownNpub) { DeviceName.current(context, state.ownNpub) }
    // Nearby travels on its own links, never in an invitation.
    val choices = state.neighborhoods.list.filter { !it.nearby }
    val picked = remember {
        mutableStateListOf<String>().also { l ->
            val last = prefs.getString("last_invite", null)?.split(',').orEmpty()
            val start = preselect.ifEmpty { last.filter { id -> choices.any { it.id == id } } }
            l.addAll(start.ifEmpty { listOfNotNull(choices.firstOrNull()?.id) })
        }
    }
    // A fresh one-time invitation whenever what it carries changes.
    LaunchedEffect(picked.toList()) {
        if (picked.isNotEmpty()) {
            client.dispatch(NativeActions.createLinkInvite(picked.toList(), from))
            prefs.edit().putString("last_invite", picked.joinToString(",")).apply()
        } else {
            client.dispatch(NativeActions.clearLinkInvite())
        }
    }
    val uri = state.neighborhoods.invite.takeIf { picked.isNotEmpty() }
    val preview = remember(uri) { uri?.let { InvitePreview.read(it) } }
    DisposableEffect(uri) {
        if (uri != null) PairPresent.beginRaw(uri)
        onDispose { PairPresent.stop() }
    }
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
    var newName by remember { mutableStateOf("") }
    var adding by remember { mutableStateOf(false) }
    val create = {
        val name = newName.trim()
        if (name.isNotEmpty()) {
            val before = choices.map { it.id }.toSet()
            val after = client.dispatch(NativeActions.createNeighborhood(name, false)).neighborhoods.list
            after.firstOrNull { it.id !in before }?.let { picked.add(it.id) }
            newName = ""
            adding = false
        }
    }
    val names = choices.filter { it.id in picked }.joinToString(", ") { it.name }
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(onDismissRequest = dismiss, sheetState = sheetState) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(start = 20.dp, end = 20.dp, bottom = 20.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text("Invite", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
            Spacer(Modifier.height(12.dp))
            if (choices.isEmpty() || adding) {
                // Nothing to invite to yet: name one, and the code follows.
                OutlinedTextField(
                    value = newName,
                    onValueChange = { newName = it },
                    label = { Text("Neighborhood name") },
                    placeholder = { Text("Household, Sports club…") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(8.dp))
                Button(onClick = create, enabled = newName.isNotBlank(), modifier = Modifier.fillMaxWidth()) {
                    Text("Create and show code")
                }
                if (choices.isNotEmpty()) {
                    TextButton(onClick = { adding = false }) { Text("Cancel") }
                }
                return@Column
            }
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
                        picked.isEmpty() -> Text(
                            "Tick at least one neighborhood below.",
                            textAlign = TextAlign.Center,
                            modifier = Modifier.padding(24.dp),
                        )
                        uri == null -> Box(Modifier.size(210.dp), contentAlignment = Alignment.Center) {
                            CircularProgressIndicator()
                        }
                        else -> {
                            val qr = remember(uri) { NsiteShare.qrBitmap(uri) }
                            QrCodeCard(qr, contentDescription = "Invitation to $names", size = 210.dp)
                            Spacer(Modifier.height(10.dp))
                            Text(
                                "Joins " + names,
                                fontWeight = FontWeight.ExtraBold,
                                style = MaterialTheme.typography.titleMedium,
                                textAlign = TextAlign.Center,
                            )
                            Text(
                                "They'll see you as $from",
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                style = MaterialTheme.typography.bodySmall,
                            )
                            val left = (preview?.exp ?: 0) - now
                            Text(
                                if (left > 0) "Scan with Myco · works once · %d:%02d left".format(left / 60, left % 60)
                                else "Expired — change the neighborhoods below to renew",
                                color = if (left > 0) MaterialTheme.colorScheme.onSurfaceVariant else Color(0xFFB45309),
                                style = MaterialTheme.typography.bodySmall,
                            )
                            Spacer(Modifier.height(12.dp))
                            ShareLinkRow(uri = uri, title = "Join $names")
                        }
                    }
                }
            }
            Spacer(Modifier.height(12.dp))
            TapCard("Or tap phones together", "Hold the backs together — no scan needed")
            if (uri != null) {
                Spacer(Modifier.height(12.dp))
                Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
                    CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
                    Spacer(Modifier.size(10.dp))
                    Text("Waiting for them to join…", style = MaterialTheme.typography.bodyMedium)
                }
            }
            // What they join, near the thumb: one big row each.
            Spacer(Modifier.height(20.dp))
            Text(
                "INVITING THEM TO",
                style = MaterialTheme.typography.labelMedium,
                fontWeight = FontWeight.Bold,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.fillMaxWidth(),
            )
            Spacer(Modifier.height(8.dp))
            for (n in choices) {
                InviteChoiceRow(
                    name = n.name,
                    picked = n.id in picked,
                    onToggle = { if (n.id in picked) picked.remove(n.id) else picked.add(n.id) },
                )
                Spacer(Modifier.height(8.dp))
            }
            TextButton(onClick = { adding = true }, modifier = Modifier.fillMaxWidth().height(52.dp)) {
                Text("+ New neighborhood")
            }
        }
    }
}

/**
 * The invitation that just arrived: who it's from and which neighborhoods
 * it carries, each kept or not, then Join.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun JoinConfirmSheet(invite: InvitePreview, client: AppCoreClient, ownNpub: String, onDismiss: () -> Unit) {
    val context = LocalContext.current
    // Sent to the inviter with the join, and to nobody else.
    val myName = remember(ownNpub) { DeviceName.current(context, ownNpub) }
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
                "Pick what to join. They'll see you as $myName.",
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
                    client.dispatch(NativeActions.joinLink(invite.raw, accept.toList(), myName))
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

@Composable
private fun InviteChoiceRow(name: String, picked: Boolean, onToggle: () -> Unit) {
    Surface(
        shape = RoundedCornerShape(16.dp),
        color = if (picked) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceVariant,
        border = if (picked) BorderStroke(2.dp, MaterialTheme.colorScheme.primary) else null,
        modifier = Modifier
            .fillMaxWidth()
            .toggleable(value = picked, role = Role.Checkbox, onValueChange = { onToggle() }),
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier.padding(horizontal = 12.dp).height(60.dp),
        ) {
            Checkbox(checked = picked, onCheckedChange = null)
            Spacer(Modifier.size(12.dp))
            Text(
                name,
                style = MaterialTheme.typography.titleMedium,
                fontWeight = if (picked) FontWeight.Bold else FontWeight.Normal,
                modifier = Modifier.weight(1f),
            )
            Text(
                if (picked) "Inviting" else "Not included",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}
