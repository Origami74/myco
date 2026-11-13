package app.myco.neighbor

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.NativeActions
import app.myco.share.NsiteShare
import app.myco.ui.SectionCard
import app.myco.ui.screens.QrCodeCard

/**
 * The **Neighbors** tab: the user's neighborhoods as a map, then as a list.
 * Each neighborhood and each peer opens a sheet with its settings. Everyone
 * is the Circle; its sheet opens the Circle screen for pairing, tap to
 * connect, nearby people and file sharing.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NeighborsScreen(state: AppState, client: AppCoreClient, onOpenCircle: () -> Unit) {
    val nh = state.neighborhoods
    var hood by remember { mutableStateOf<String?>(null) }
    var peer by remember { mutableStateOf<String?>(null) }
    var adding by remember { mutableStateOf(false) }
    var joining by remember { mutableStateOf(false) }

    LazyColumn(
        modifier = Modifier.fillMaxSize(),
        contentPadding = PaddingValues(20.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        item {
            Text("Neighbors", style = MaterialTheme.typography.displaySmall)
        }
        item {
            SectionCard {
                NeighborMap(
                    nh,
                    onNeighborhood = { hood = it.id },
                    onPeer = { peer = it.key },
                )
            }
        }
        item {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = { adding = true }) { Text("New neighborhood") }
                OutlinedButton(onClick = { joining = true }) { Text("Join") }
            }
        }
        if (nh.status.isNotEmpty()) {
            item { Text(nh.status, style = MaterialTheme.typography.bodySmall) }
        }
        items(nh.list, key = { it.id }) { n ->
            SectionCard {
                Row(
                    modifier = Modifier.fillMaxWidth().clickable { hood = n.id }.padding(16.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(Modifier.size(14.dp).background(bubbleColor(n), CircleShape))
                    Spacer(Modifier.size(12.dp))
                    Column(Modifier.weight(1f)) {
                        Text(n.name, fontWeight = FontWeight.SemiBold)
                        Text(
                            describe(n, nh),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
        val waiting = nh.links.flatMap { l -> l.pending.map { l to it } }
        if (waiting.isNotEmpty()) {
            item { Text("Waiting for you", fontWeight = FontWeight.Bold) }
            items(waiting, key = { (l, p) -> l.id + p.id }) { (l, p) ->
                SectionCard {
                    Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text("${l.name} wants to share ${p.name}", Modifier.weight(1f))
                        TextButton(onClick = { client.dispatch(NativeActions.answerLinkNeighborhood(l.id, p.id, true)) }) {
                            Text("Accept")
                        }
                        TextButton(onClick = { client.dispatch(NativeActions.answerLinkNeighborhood(l.id, p.id, false)) }) {
                            Text("Decline")
                        }
                    }
                }
            }
        }
    }

    nh.list.firstOrNull { it.id == hood }?.let { n ->
        ModalBottomSheet(onDismissRequest = {
            hood = null
            client.dispatch(NativeActions.clearLinkInvite())
        }) {
            NeighborhoodSheet(n, nh, client, onOpenCircle = { hood = null; onOpenCircle() }, onClose = { hood = null })
        }
    }
    nh.peers.firstOrNull { it.key == peer }?.let { p ->
        ModalBottomSheet(onDismissRequest = { peer = null }) {
            PeerSheet(p, nh, client, onOpenCircle = { peer = null; onOpenCircle() }, onClose = { peer = null })
        }
    }
    if (adding) {
        NewNeighborhoodDialog(client, onDone = { adding = false })
    }
    if (joining) {
        ModalBottomSheet(onDismissRequest = { joining = false }) {
            Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text("Join with an invitation", style = MaterialTheme.typography.titleLarge)
                Join(client)
                Spacer(Modifier.size(24.dp))
            }
        }
    }
}

private fun describe(n: NeighborhoodsState.Neighborhood, nh: NeighborhoodsState): String {
    val peers = nh.peers.count { n.id in it.neighborhoods }
    val reach = nh.peers.count { n.id in it.neighborhoods && it.reachable }
    val kind = when {
        n.everyone -> "the Circle"
        n.autoAccept -> "open"
        else -> "by invitation"
    }
    return "$kind · $peers people · $reach reachable"
}

@Composable
private fun NeighborhoodSheet(
    n: NeighborhoodsState.Neighborhood,
    nh: NeighborhoodsState,
    client: AppCoreClient,
    onOpenCircle: () -> Unit,
    onClose: () -> Unit,
) {
    var name by remember(n.id) { mutableStateOf(n.name) }
    var leaving by remember { mutableStateOf(false) }
    Column(
        Modifier.padding(horizontal = 20.dp).verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text(n.name, style = MaterialTheme.typography.headlineSmall)
        Text(describe(n, nh), color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (n.everyone) {
            Button(onClick = onOpenCircle) { Text("Pair, tap to connect, nearby & files") }
        }

        OutlinedTextField(
            value = name,
            onValueChange = { name = it },
            label = { Text("Your name for it") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        if (name.isNotBlank() && name != n.name) {
            TextButton(onClick = { client.dispatch(NativeActions.renameNeighborhood(n.id, name)) }) { Text("Rename") }
        }

        HorizontalDivider()
        Text("Reach", fontWeight = FontWeight.Bold)
        Stepper("Posts travel", n.publishHops, 0..7) {
            client.dispatch(NativeActions.setNeighborhoodPolicy(n.id, it, n.backlogHops, n.passOn))
        }
        Stepper("Catch-up asks", n.backlogHops, 0..7) {
            client.dispatch(NativeActions.setNeighborhoodPolicy(n.id, n.publishHops, it, n.passOn))
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("Pass on others' posts", Modifier.weight(1f))
            Switch(checked = n.passOn, onCheckedChange = {
                client.dispatch(NativeActions.setNeighborhoodPolicy(n.id, n.publishHops, n.backlogHops, it))
            })
        }
        Stepper("Push uploaded files", n.blobHops, 0..3) {
            client.dispatch(NativeActions.setNeighborhoodBlobHops(n.id, it))
        }
        Text(
            "Files are heavy: 0 keeps them here until someone asks. Raise it for your own devices.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )

        if (!n.everyone) {
            HorizontalDivider()
            Text("Share", fontWeight = FontWeight.Bold)
            val invite = nh.invite
            if (invite != null) {
                val bitmap = remember(invite) { NsiteShare.qrBitmap(invite) }
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                    QrCodeCard(bitmap, "Invitation to ${n.name}", size = 220.dp)
                }
                Text(
                    "Scan with Myco: Neighbors → Join. Works once, for 15 minutes.",
                    style = MaterialTheme.typography.bodySmall,
                )
            } else {
                Button(onClick = { client.dispatch(NativeActions.createLinkInvite(listOf(n.id))) }) {
                    Text("Invite someone")
                }
            }
            val elsewhere = nh.links.filter { n.id !in it.neighborhoods }
            if (elsewhere.isNotEmpty()) {
                Text("Or offer it to someone you're already linked with:", style = MaterialTheme.typography.bodySmall)
                for (l in elsewhere) {
                    TextButton(onClick = { client.dispatch(NativeActions.shareOnLink(l.id, n.id)) }) { Text(l.name) }
                }
            }
            HorizontalDivider()
            TextButton(onClick = { leaving = true }) {
                Text("Leave ${n.name}", color = MaterialTheme.colorScheme.error)
            }
        }
        Spacer(Modifier.size(24.dp))
    }
    if (leaving) {
        AlertDialog(
            onDismissRequest = { leaving = false },
            title = { Text("Leave ${n.name}?") },
            text = { Text("You stop carrying it with everyone. What you received stays where it is.") },
            confirmButton = {
                TextButton(onClick = {
                    client.dispatch(NativeActions.leaveNeighborhood(n.id))
                    leaving = false
                    onClose()
                }) { Text("Leave") }
            },
            dismissButton = { TextButton(onClick = { leaving = false }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun PeerSheet(
    p: NeighborhoodsState.PeerNode,
    nh: NeighborhoodsState,
    client: AppCoreClient,
    onOpenCircle: () -> Unit,
    onClose: () -> Unit,
) {
    Column(
        Modifier.padding(horizontal = 20.dp).verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text(p.name, style = MaterialTheme.typography.headlineSmall)
        Text(
            if (p.reachable) "Reachable now" else "Not reachable right now",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        val linkId = p.linkId
        if (linkId == null) {
            Text("Paired with you: in Everyone.")
            Button(onClick = onOpenCircle) { Text("Open in Circle") }
        } else {
            Text("Carried with them", fontWeight = FontWeight.Bold)
            for (id in p.neighborhoods) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(nh.nameOf(id), Modifier.weight(1f))
                    TextButton(onClick = { client.dispatch(NativeActions.dropFromLink(linkId, id)) }) { Text("Stop") }
                }
            }
            val others = nh.list.filter { !it.everyone && it.id !in p.neighborhoods }
            if (others.isNotEmpty()) {
                Text("Offer", fontWeight = FontWeight.Bold)
                for (n in others) {
                    TextButton(onClick = { client.dispatch(NativeActions.shareOnLink(linkId, n.id)) }) { Text(n.name) }
                }
            }
            HorizontalDivider()
            TextButton(onClick = {
                client.dispatch(NativeActions.endLink(linkId))
                onClose()
            }) { Text("End link", color = MaterialTheme.colorScheme.error) }
        }
        Spacer(Modifier.size(24.dp))
    }
}

@Composable
private fun Stepper(label: String, value: Int, range: IntRange, onChange: (Int) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(label, Modifier.weight(1f))
        TextButton(enabled = value > range.first, onClick = { onChange(value - 1) }) { Text("−") }
        Text(if (value == 0) "off" else "$value hop${if (value == 1) "" else "s"}")
        TextButton(enabled = value < range.last, onClick = { onChange(value + 1) }) { Text("+") }
    }
}

@Composable
private fun NewNeighborhoodDialog(client: AppCoreClient, onDone: () -> Unit) {
    var name by remember { mutableStateOf("") }
    var open by remember { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = onDone,
        title = { Text("New neighborhood") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it },
                    label = { Text("Name") },
                    singleLine = true,
                )
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text("Open")
                        Text(
                            "Anyone using this exact name joins the same one — a festival, a city.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    Switch(checked = open, onCheckedChange = { open = it })
                }
            }
        },
        confirmButton = {
            TextButton(enabled = name.isNotBlank(), onClick = {
                client.dispatch(NativeActions.createNeighborhood(name, open))
                onDone()
            }) { Text("Create") }
        },
        dismissButton = { TextButton(onClick = onDone) { Text("Cancel") } },
    )
}
