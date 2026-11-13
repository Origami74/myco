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
import app.myco.ui.SectionCard

/**
 * The **Neighbors** tab: the user's neighborhoods as a map, then as a list.
 * Each neighborhood and each peer opens a sheet with its settings. Everyone
 * is the Circle; its sheet opens the Circle screen for pairing, tap to
 * connect, nearby people and file sharing.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NeighborsScreen(
    state: AppState,
    client: AppCoreClient,
    /** MainActivity's router for scanned, tapped and pasted `myco://` codes. */
    onScanned: (String) -> Unit,
    /** Pick files and offer them to a paired device (by npub). */
    onSendFile: (String) -> Unit,
) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val prefs = remember { context.getSharedPreferences("neighbors", android.content.Context.MODE_PRIVATE) }
    var noticeSeen by remember { mutableStateOf(prefs.getBoolean("circle_notice_seen", false)) }
    val nh = state.neighborhoods
    var hood by remember { mutableStateOf<String?>(null) }
    var peer by remember { mutableStateOf<String?>(null) }
    var adding by remember { mutableStateOf(false) }
    var joining by remember { mutableStateOf(false) }
    var inviting by remember { mutableStateOf<String?>(null) }

    LazyColumn(
        modifier = Modifier.fillMaxSize(),
        contentPadding = PaddingValues(20.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        item {
            Text("Neighbors", style = MaterialTheme.typography.displaySmall)
        }
        if (!noticeSeen) {
            item {
                SectionCard {
                    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text("The Circle is now Neighbors", fontWeight = FontWeight.Bold)
                        Text(
                            "Everyone you paired with is in Everyone, as before. New people join a " +
                                "neighborhood with an invitation: tap a neighborhood, then Invite someone.",
                            style = MaterialTheme.typography.bodySmall,
                        )
                        TextButton(onClick = {
                            prefs.edit().putBoolean("circle_notice_seen", true).apply()
                            noticeSeen = true
                        }) { Text("Got it") }
                    }
                }
            }
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
        ModalBottomSheet(onDismissRequest = { hood = null }) {
            NeighborhoodSheet(
                n,
                nh,
                client,
                onInvite = { hood = null; inviting = n.id },
                onClose = { hood = null },
            )
        }
    }
    nh.peers.firstOrNull { it.key == peer }?.let { p ->
        ModalBottomSheet(onDismissRequest = { peer = null }) {
            PeerSheet(
                p,
                nh,
                client,
                onSendFile = { peer = null; onSendFile(p.key) },
                onClose = { peer = null },
            )
        }
    }
    if (adding) {
        NewNeighborhoodDialog(client, onDone = { adding = false })
    }
    if (joining) {
        JoinSheet(onScanned = onScanned, onDismiss = { joining = false })
    }
    nh.list.firstOrNull { it.id == inviting }?.let { n ->
        InviteSheet(n, state, client, onDismiss = { inviting = null })
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
    onInvite: () -> Unit,
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
            Text(
                "The devices you paired with before neighborhoods. New people join by invitation " +
                    "to another neighborhood.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
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
            Button(onClick = onInvite) { Text("Invite someone") }
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
    onSendFile: () -> Unit,
    onClose: () -> Unit,
) {
    var removing by remember { mutableStateOf(false) }
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
            Button(onClick = onSendFile) { Text("Send a file") }
            HorizontalDivider()
            TextButton(onClick = { removing = true }) {
                Text("Remove from Everyone", color = MaterialTheme.colorScheme.error)
            }
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
    if (removing) {
        AlertDialog(
            onDismissRequest = { removing = false },
            title = { Text("Remove ${p.name}?") },
            text = { Text("They leave Everyone on this phone. To connect again, invite them to a neighborhood.") },
            confirmButton = {
                TextButton(onClick = {
                    client.dispatch(NativeActions.removeFromCircle(p.key))
                    removing = false
                    onClose()
                }) { Text("Remove") }
            },
            dismissButton = { TextButton(onClick = { removing = false }) { Text("Cancel") } },
        )
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
