package app.myco.neighbor

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Add
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Surface
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.NativeActions

/**
 * One neighborhood: who you're connected with in it, how far posts go, and
 * — out of the way — the finer settings and Leave. The people list is only
 * this phone's own connections; a neighborhood has no member list.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NeighborhoodPage(
    id: String,
    state: AppState,
    client: AppCoreClient,
    onBack: () -> Unit,
    onSendFile: (String) -> Unit,
) {
    val nh = state.neighborhoods
    val n = nh.list.firstOrNull { it.id == id }
    if (n == null) {
        // Left, or not there: nothing to show.
        androidx.compose.runtime.LaunchedEffect(Unit) { onBack() }
        return
    }
    var person by remember { mutableStateOf<String?>(null) }
    var inviting by remember { mutableStateOf(false) }
    var more by remember { mutableStateOf(false) }
    var leaving by remember { mutableStateOf(false) }
    val people = nh.peopleIn(n.id)
    val color = bubbleColor(n)

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
        // Header, in the neighborhood's colour.
        Column(
            Modifier
                .fillMaxWidth()
                .background(color.copy(alpha = 0.18f))
                .padding(start = 8.dp, end = 20.dp, top = 8.dp, bottom = 20.dp),
        ) {
            IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") }
            Row(Modifier.padding(start = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(titleOf(n), style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.ExtraBold)
                    Text(kindOf(n), style = MaterialTheme.typography.bodyMedium)
                }
                if (!n.everyone) {
                    Button(
                        onClick = { inviting = true },
                        colors = ButtonDefaults.buttonColors(containerColor = color, contentColor = Color.White),
                    ) {
                        Icon(Icons.Filled.Add, contentDescription = null)
                        Spacer(Modifier.width(4.dp))
                        Text("Invite")
                    }
                }
            }
        }

        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Label("PEOPLE YOU'RE CONNECTED WITH HERE")
            if (people.isEmpty()) {
                Text(
                    if (n.autoAccept) "Nobody yet. Anyone nearby using this name will show up here."
                    else "Just you so far. Invite someone to start.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            for (p in people) {
                Row(
                    Modifier.fillMaxWidth().clickable { person = p.key }.padding(vertical = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Avatar(p.name, p.key, here = p.reachable, size = 40.dp)
                    Spacer(Modifier.width(12.dp))
                    Column {
                        Text(p.name, fontWeight = FontWeight.SemiBold)
                        Text(
                            if (p.reachable) "Here now" else "Not here now",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
            if (!n.everyone) {
                Text(
                    "People they invite can reach you through them too — you only see who you're connected with.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }

            HorizontalDivider(Modifier.padding(vertical = 4.dp))
            Label("HOW FAR POSTS GO")
            val reach = Reach.of(n)
            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                Reach.entries.forEachIndexed { i, r ->
                    SegmentedButton(
                        selected = reach == r,
                        onClick = { client.dispatch(NativeActions.setNeighborhoodPolicy(n.id, r.publish, r.backlog, r.passOn)) },
                        shape = SegmentedButtonDefaults.itemShape(i, Reach.entries.size),
                    ) { Text(r.label, maxLines = 1) }
                }
            }
            Text(
                reach?.explain ?: "Custom — set under More settings.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )

            Surface(
                shape = RoundedCornerShape(16.dp),
                color = MaterialTheme.colorScheme.surfaceVariant,
                modifier = Modifier.fillMaxWidth().clickable { more = !more },
            ) {
                Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text("More settings", fontWeight = FontWeight.Bold)
                        Text("Name, catching up, files", style = MaterialTheme.typography.bodySmall)
                    }
                    Text(if (more) "⌃" else "⌄", style = MaterialTheme.typography.titleLarge)
                }
            }
            if (more) MoreSettings(n, client)

            if (!n.everyone) {
                Spacer(Modifier.height(12.dp))
                TextButton(onClick = { leaving = true }) {
                    Text("Leave ${n.name}", color = MaterialTheme.colorScheme.error)
                }
                Text(
                    "Asks first. What you received stays on your phone.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Spacer(Modifier.height(24.dp))
        }
    }

    if (inviting) {
        InviteSheet(state = state, client = client, byLink = false, preselect = n.id, onDismiss = { inviting = false })
    }
    nh.peers.firstOrNull { it.key == person }?.let { p ->
        PersonSheet(p, nh, client, onSendFile = { person = null; onSendFile(p.key) }, onDismiss = { person = null })
    }
    if (leaving) {
        AlertDialog(
            onDismissRequest = { leaving = false },
            title = { Text("Leave ${n.name}?") },
            text = { Text("You stop sharing it with everyone you're connected with. What you received stays on your phone.") },
            confirmButton = {
                TextButton(onClick = {
                    client.dispatch(NativeActions.leaveNeighborhood(n.id))
                    leaving = false
                    onBack()
                }) { Text("Leave") }
            },
            dismissButton = { TextButton(onClick = { leaving = false }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun MoreSettings(n: NeighborhoodsState.Neighborhood, client: AppCoreClient) {
    var name by remember(n.id) { mutableStateOf(n.name) }
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        if (!n.everyone) {
            OutlinedTextField(
                value = name,
                onValueChange = { name = it },
                label = { Text("Your name for it") },
                supportingText = { Text("Only on your phone. Others keep their own name for it.") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            if (name.isNotBlank() && name.trim() != n.name) {
                TextButton(onClick = { client.dispatch(NativeActions.renameNeighborhood(n.id, name)) }) { Text("Save name") }
            }
        }
        Choice(
            "Catch up from",
            "When you open an app, ask for what you missed from",
            listOf("nobody", "people you're connected with", "and their connections", "a few steps out"),
            n.backlogHops.coerceIn(0, 3),
        ) { client.dispatch(NativeActions.setNeighborhoodPolicy(n.id, n.publishHops, it, n.passOn)) }
        Choice(
            "Posts travel",
            "How many steps your posts go",
            listOf("nowhere", "to people you're connected with", "one step further", "a few steps out"),
            n.publishHops.coerceIn(0, 3),
        ) { client.dispatch(NativeActions.setNeighborhoodPolicy(n.id, it, n.backlogHops, n.passOn)) }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("Pass on others' posts")
                Text(
                    "Help posts reach people further away.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Switch(checked = n.passOn, onCheckedChange = {
                client.dispatch(NativeActions.setNeighborhoodPolicy(n.id, n.publishHops, n.backlogHops, it))
            })
        }
        Choice(
            "Send files ahead",
            "Files are heavy. Usually they stay here until someone asks; send ahead for your own devices.",
            listOf("no — on request", "to people you're connected with"),
            n.blobHops.coerceIn(0, 1),
        ) { client.dispatch(NativeActions.setNeighborhoodBlobHops(n.id, it)) }
    }
}

/** A labelled choice between a few spelled-out options. */
@Composable
private fun Choice(title: String, hint: String, options: List<String>, selected: Int, onPick: (Int) -> Unit) {
    Column {
        Text(title)
        Text(hint, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        options.forEachIndexed { i, label ->
            Row(
                Modifier.fillMaxWidth().clickable { onPick(i) }.padding(vertical = 2.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                androidx.compose.material3.RadioButton(selected = i == selected, onClick = { onPick(i) })
                Text(label)
            }
        }
    }
}
