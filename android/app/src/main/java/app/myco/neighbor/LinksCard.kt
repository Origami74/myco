package app.myco.neighbor

import android.content.Intent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.height
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.NativeActions
import app.myco.share.NsiteShare
import app.myco.ui.SectionCard
import app.myco.ui.screens.QrCodeCard
import app.myco.ui.screens.ScanPanel

/**
 * Bare-bones neighborhood links, for trying link setup between two phones
 * before the Neighbors tab exists. Start or join a neighborhood, invite with
 * a QR code, scan or paste an invitation, answer shares, end links.
 */
@Composable
fun LinksCard(state: AppState, client: AppCoreClient) {
    val nh = state.neighborhoods
    Column {
        Text(
            "NEIGHBORHOODS (PREVIEW)",
            color = MaterialTheme.colorScheme.primary,
            fontWeight = FontWeight.Bold,
            style = MaterialTheme.typography.titleSmall,
            modifier = Modifier.padding(start = 4.dp, bottom = 6.dp),
        )
        SectionCard {
            Column(
                modifier = Modifier.padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Neighborhoods(nh, client)
                HorizontalDivider()
                Invite(nh, client)
                HorizontalDivider()
                Join(client)
                if (nh.status.isNotEmpty()) {
                    Text(nh.status, style = MaterialTheme.typography.bodySmall)
                }
                HorizontalDivider()
                Links(nh, client)
            }
        }
    }
}

@Composable
private fun Neighborhoods(nh: NeighborhoodsState, client: AppCoreClient) {
    for (n in nh.list) {
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
            Column(modifier = Modifier.weight(1f)) {
                Text(n.name, fontWeight = FontWeight.SemiBold)
                val kind = when {
                    n.everyone -> "the Circle"
                    n.autoAccept -> "open, by name"
                    else -> "shared by link"
                }
                Text(
                    "$kind · ${n.links} links · hops ${n.publishHops}/${n.backlogHops} · blobs ${n.blobHops}",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            TextButton(onClick = {
                client.dispatch(NativeActions.setNeighborhoodBlobHops(n.id, (n.blobHops + 1) % 4))
            }) { Text("blobs ${n.blobHops}") }
        }
    }
    var name by remember { mutableStateOf("") }
    var open by remember { mutableStateOf(false) }
    OutlinedTextField(
        value = name,
        onValueChange = { name = it },
        label = { Text("New neighborhood") },
        singleLine = true,
        modifier = Modifier.fillMaxWidth(),
    )
    Row(verticalAlignment = Alignment.CenterVertically) {
        Checkbox(checked = open, onCheckedChange = { open = it })
        Text("Open: anyone using this name joins", modifier = Modifier.weight(1f))
        Button(enabled = name.isNotBlank(), onClick = {
            client.dispatch(NativeActions.createNeighborhood(name, open))
            name = ""
        }) { Text("Add") }
    }
}

@Composable
internal fun Invite(nh: NeighborhoodsState, client: AppCoreClient) {
    val context = LocalContext.current
    val invite = nh.invite
    if (invite != null) {
        val bitmap = remember(invite) { NsiteShare.qrBitmap(invite) }
        Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.fillMaxWidth()) {
            QrCodeCard(bitmap, "Link invitation", size = 240.dp)
            Row {
                TextButton(onClick = {
                    val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, invite)
                    context.startActivity(Intent.createChooser(send, "Send invitation"))
                }) { Text("Send as text") }
                TextButton(onClick = { client.dispatch(NativeActions.clearLinkInvite()) }) { Text("Done") }
            }
            Text(
                "Works once, for 15 minutes.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        return
    }
    val picked = remember { mutableStateListOf<String>() }
    Text("Invite someone, carrying:", style = MaterialTheme.typography.bodyMedium)
    for (n in nh.list.filter { !it.everyone }) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Checkbox(checked = n.id in picked, onCheckedChange = { on ->
                if (on) picked.add(n.id) else picked.remove(n.id)
            })
            Text(n.name)
        }
    }
    Button(onClick = { client.dispatch(NativeActions.createLinkInvite(picked.toList())) }) {
        Text("Show invitation QR")
    }
}

@Composable
internal fun Join(client: AppCoreClient) {
    var preview by remember { mutableStateOf<InvitePreview?>(null) }
    var pasted by remember { mutableStateOf("") }
    val accept = remember { mutableStateListOf<String>() }
    var scanning by remember { mutableStateOf(false) }
    val current = preview
    if (current == null) {
        if (scanning) {
            // The pairing scanner: the camera is asked through SystemAsker.
            Box(modifier = Modifier.fillMaxWidth().height(280.dp)) {
                ScanPanel(onScanned = { text ->
                    InvitePreview.read(text)?.let { p ->
                        preview = p
                        accept.clear()
                        p.offered.forEach { accept.add(it.id) }
                        scanning = false
                    }
                })
            }
            TextButton(onClick = { scanning = false }) { Text("Stop scanning") }
            return
        }
        OutlinedButton(onClick = { scanning = true }) { Text("Scan invitation") }
        OutlinedTextField(
            value = pasted,
            onValueChange = {
                pasted = it
                InvitePreview.read(it)?.let { p ->
                    preview = p
                    accept.clear()
                    p.offered.forEach { o -> accept.add(o.id) }
                }
            },
            label = { Text("…or paste one") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        return
    }
    Text("Join, carrying:", style = MaterialTheme.typography.bodyMedium)
    for (o in current.offered) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Checkbox(checked = o.id in accept, onCheckedChange = { on ->
                if (on) accept.add(o.id) else accept.remove(o.id)
            })
            Text(o.name)
        }
    }
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Button(onClick = {
            client.dispatch(NativeActions.joinLink(current.raw, accept.toList()))
            preview = null
            pasted = ""
        }) { Text("Join") }
        TextButton(onClick = { preview = null; pasted = "" }) { Text("Cancel") }
    }
}

@Composable
private fun Links(nh: NeighborhoodsState, client: AppCoreClient) {
    if (nh.links.isEmpty()) {
        Text("No links yet.", color = MaterialTheme.colorScheme.onSurfaceVariant)
        return
    }
    for (link in nh.links) {
        Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
                Text(link.name, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                TextButton(onClick = { client.dispatch(NativeActions.endLink(link.id)) }) { Text("End") }
            }
            Text(
                "Carries: " + link.neighborhoods.joinToString { nh.nameOf(it) }.ifEmpty { "nothing yet" },
                style = MaterialTheme.typography.bodySmall,
            )
            for (p in link.pending) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("Wants to share ${p.name}", modifier = Modifier.weight(1f))
                    TextButton(onClick = {
                        client.dispatch(NativeActions.answerLinkNeighborhood(link.id, p.id, true))
                    }) { Text("Accept") }
                    TextButton(onClick = {
                        client.dispatch(NativeActions.answerLinkNeighborhood(link.id, p.id, false))
                    }) { Text("Decline") }
                }
            }
            val shareable = nh.list.filter { !it.everyone && it.id !in link.neighborhoods }
            if (shareable.isNotEmpty()) {
                Row {
                    for (n in shareable) {
                        TextButton(onClick = { client.dispatch(NativeActions.shareOnLink(link.id, n.id)) }) {
                            Text("Share ${n.name}")
                        }
                    }
                }
            }
        }
    }
}
