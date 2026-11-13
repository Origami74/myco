package app.myco.neighbor

import androidx.compose.foundation.BorderStroke
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
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.NativeActions

/**
 * The **Neighbors** tab. Top to bottom: who's here now, shares waiting for
 * you, your neighborhoods (as a list, or a map), and one action — Add
 * people. Every number on it is something this phone knows: the people you
 * are connected with, never a neighborhood's size.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NeighborsScreen(
    state: AppState,
    client: AppCoreClient,
    onOpenNeighborhood: (String) -> Unit,
    /** MainActivity's router for scanned, tapped and pasted codes. */
    onScanned: (String) -> Unit,
    /** Pick files and offer them to a paired device (by npub). */
    onSendFile: (String) -> Unit,
) {
    val nh = state.neighborhoods
    val context = LocalContext.current
    val prefs = remember { context.getSharedPreferences("neighbors", android.content.Context.MODE_PRIVATE) }
    var noticeSeen by remember { mutableStateOf(prefs.getBoolean("circle_notice_seen", false)) }
    var showMap by remember { mutableStateOf(false) }
    var person by remember { mutableStateOf<String?>(null) }
    var adding by remember { mutableStateOf(false) }
    var flow by remember { mutableStateOf<AddFlow?>(null) }

    val here = nh.hereNow()
    val groups = nh.list.filter { !it.everyone }
    val paired = nh.list.firstOrNull { it.everyone }

    Box(Modifier.fillMaxSize()) {
        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(start = 20.dp, end = 20.dp, top = 20.dp, bottom = 96.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            item {
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Text("Neighbors", style = MaterialTheme.typography.displaySmall, modifier = Modifier.weight(1f))
                    HereChip(here.size)
                }
            }

            if (!noticeSeen) {
                item {
                    Banner(
                        title = "The Circle is now Neighbors",
                        body = "Your paired devices are under Paired devices. New people join by invitation: tap Add people.",
                        action = "Got it",
                        onAction = {
                            prefs.edit().putBoolean("circle_notice_seen", true).apply()
                            noticeSeen = true
                        },
                    )
                }
            }

            // Shares waiting for an answer, one tap each.
            val waiting = nh.links.flatMap { l -> l.pending.map { l to it } }
            items(waiting, key = { (l, p) -> "w" + l.id + p.id }) { (l, p) ->
                Surface(shape = RoundedCornerShape(18.dp), color = MaterialTheme.colorScheme.secondaryContainer) {
                    Row(Modifier.padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
                        Avatar(l.name, l.id, here = false, size = 36.dp)
                        Spacer(Modifier.width(12.dp))
                        Column(Modifier.weight(1f)) {
                            Text("${l.name} added you to", style = MaterialTheme.typography.bodySmall)
                            Text(p.name, fontWeight = FontWeight.Bold)
                        }
                        TextButton(onClick = { client.dispatch(NativeActions.answerLinkNeighborhood(l.id, p.id, false)) }) {
                            Text("No thanks")
                        }
                        Button(onClick = { client.dispatch(NativeActions.answerLinkNeighborhood(l.id, p.id, true)) }) {
                            Text("Join")
                        }
                    }
                }
            }

            if (here.isNotEmpty()) {
                item { Label("HERE NOW") }
                item {
                    LazyRow(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                        items(here, key = { it.key }) { p ->
                            Column(
                                horizontalAlignment = Alignment.CenterHorizontally,
                                modifier = Modifier.width(60.dp).clickable { person = p.key },
                            ) {
                                Avatar(p.name, p.key, here = true, size = 48.dp)
                                Spacer(Modifier.height(4.dp))
                                Text(p.name, style = MaterialTheme.typography.labelSmall, maxLines = 1)
                            }
                        }
                    }
                }
            }

            item {
                Row(Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Label("YOUR NEIGHBORHOODS", Modifier.weight(1f))
                    if (groups.isNotEmpty()) {
                        TextButton(onClick = { showMap = !showMap }) { Text(if (showMap) "List" else "Map") }
                    }
                }
            }

            if (groups.isEmpty()) {
                item {
                    Banner(
                        title = "Start your first neighborhood",
                        body = "A household, a club, a trip. Then invite people to it — in person or with a link.",
                        action = "Start one",
                        onAction = { flow = AddFlow.New },
                    )
                }
            } else if (showMap) {
                item {
                    Surface(shape = RoundedCornerShape(20.dp), color = MaterialTheme.colorScheme.surfaceVariant) {
                        Column(Modifier.padding(8.dp)) {
                            NeighborMap(nh, onNeighborhood = { onOpenNeighborhood(it.id) }, onPeer = { person = it.key })
                            Text(
                                "Solid: here now · faded: not now",
                                style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                modifier = Modifier.padding(8.dp),
                            )
                        }
                    }
                }
            } else {
                items(groups, key = { it.id }) { n -> NeighborhoodCard(n, nh) { onOpenNeighborhood(n.id) } }
            }

            if (paired != null && nh.peopleIn(paired.id).isNotEmpty()) {
                item {
                    Surface(
                        shape = RoundedCornerShape(18.dp),
                        color = MaterialTheme.colorScheme.surface,
                        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
                        modifier = Modifier.fillMaxWidth().clickable { onOpenNeighborhood(paired.id) },
                    ) {
                        Column(Modifier.padding(16.dp)) {
                            Text("Paired devices", fontWeight = FontWeight.Bold)
                            Text(
                                withNames(nh.peopleIn(paired.id)) ?: "",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                }
            }
            if (nh.status.isNotEmpty()) {
                item { Text(nh.status, style = MaterialTheme.typography.bodySmall) }
            }
        }

        ExtendedFloatingActionButton(
            onClick = { adding = true },
            icon = { Icon(Icons.Filled.Add, contentDescription = null) },
            text = { Text("Add people") },
            containerColor = MaterialTheme.colorScheme.primary,
            contentColor = MaterialTheme.colorScheme.onPrimary,
            modifier = Modifier.align(Alignment.BottomEnd).padding(20.dp),
        )
    }

    if (adding) {
        AddPeopleSheet(
            hasNeighborhoods = groups.isNotEmpty(),
            onPick = { adding = false; flow = it },
            onDismiss = { adding = false },
        )
    }
    when (val f = flow) {
        AddFlow.InPerson, AddFlow.ByLink -> InviteSheet(
            state = state,
            client = client,
            byLink = f == AddFlow.ByLink,
            preselect = null,
            onDismiss = { flow = null },
        )
        AddFlow.Join -> JoinSheet(onScanned = onScanned, onDismiss = { flow = null })
        AddFlow.New -> NewNeighborhoodDialog(
            client = client,
            onCreated = { id -> flow = null; if (id != null) onOpenNeighborhood(id) },
            onDismiss = { flow = null },
        )
        null -> {}
    }
    nh.peers.firstOrNull { it.key == person }?.let { p ->
        PersonSheet(p, nh, client, onSendFile = { person = null; onSendFile(p.key) }, onDismiss = { person = null })
    }
}

/** The ways into Add people. */
internal enum class AddFlow { InPerson, ByLink, Join, New }

@Composable
private fun HereChip(count: Int) {
    val on = count > 0
    Surface(shape = RoundedCornerShape(50), color = if (on) Color(0xFFD1FAE5) else MaterialTheme.colorScheme.surfaceVariant) {
        Row(Modifier.padding(horizontal = 12.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(8.dp).background(if (on) Color(0xFF22C55E) else Color(0xFF94A3B8), CircleShape))
            Spacer(Modifier.width(6.dp))
            Text(
                if (on) "$count here now" else "Nobody here now",
                style = MaterialTheme.typography.labelMedium,
                fontWeight = FontWeight.Bold,
                color = if (on) Color(0xFF064E3B) else MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
internal fun Label(text: String, modifier: Modifier = Modifier) {
    Text(
        text,
        style = MaterialTheme.typography.labelMedium,
        fontWeight = FontWeight.Bold,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = modifier,
    )
}

@Composable
private fun Banner(title: String, body: String, action: String, onAction: () -> Unit) {
    Surface(shape = RoundedCornerShape(18.dp), color = MaterialTheme.colorScheme.primaryContainer) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(title, fontWeight = FontWeight.Bold, color = MaterialTheme.colorScheme.onPrimaryContainer)
            Text(body, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onPrimaryContainer)
            TextButton(onClick = onAction) { Text(action) }
        }
    }
}

@Composable
private fun NeighborhoodCard(n: NeighborhoodsState.Neighborhood, nh: NeighborhoodsState, onOpen: () -> Unit) {
    val people = nh.peopleIn(n.id)
    val hereCount = people.count { it.reachable }
    Surface(
        shape = RoundedCornerShape(18.dp),
        color = MaterialTheme.colorScheme.surfaceVariant,
        modifier = Modifier.fillMaxWidth().clickable(onClick = onOpen),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.width(6.dp).height(76.dp).background(bubbleColor(n)))
            Column(Modifier.weight(1f).padding(horizontal = 16.dp, vertical = 12.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(n.name, fontWeight = FontWeight.Bold, style = MaterialTheme.typography.titleMedium)
                    if (n.autoAccept) {
                        Spacer(Modifier.width(8.dp))
                        Surface(shape = RoundedCornerShape(50), color = MaterialTheme.colorScheme.tertiaryContainer) {
                            Text("Open", style = MaterialTheme.typography.labelSmall, modifier = Modifier.padding(horizontal = 8.dp, vertical = 2.dp))
                        }
                    }
                }
                Text(
                    (withNames(people) ?: "Just you so far — invite someone") +
                        if (hereCount > 0) " · $hereCount here now" else "",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Box(Modifier.padding(end = 16.dp)) {
                people.take(3).forEachIndexed { i, p ->
                    Avatar(p.name, p.key, here = p.reachable, size = 30.dp, modifier = Modifier.offset(x = (i * 20).dp))
                }
                if (people.isNotEmpty()) Spacer(Modifier.width((30 + 20 * (minOf(people.size, 3) - 1)).dp))
            }
        }
    }
}

/** **Add people**: ask about the situation, not the mechanism. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AddPeopleSheet(hasNeighborhoods: Boolean, onPick: (AddFlow) -> Unit, onDismiss: () -> Unit) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState) {
        Column(
            Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text("Add people", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
            Text("How are you meeting them?", color = MaterialTheme.colorScheme.onSurfaceVariant)
            // Inviting needs a neighborhood to invite to; without one, start there.
            val inviteFlow: (AddFlow) -> Unit = { if (hasNeighborhoods) onPick(it) else onPick(AddFlow.New) }
            Option("They're with me", "Show a code, or tap phones together · works offline", primary = true) {
                inviteFlow(AddFlow.InPerson)
            }
            Option("Send an invite link", "By message · works once, for 15 minutes") { inviteFlow(AddFlow.ByLink) }
            Option("I got an invite", "Scan it, tap phones, or paste it") { onPick(AddFlow.Join) }
            HorizontalDivider(Modifier.padding(vertical = 4.dp))
            Row(
                Modifier.fillMaxWidth().clickable { onPick(AddFlow.New) }.padding(vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(Modifier.weight(1f)) {
                    Text("Start a new neighborhood", fontWeight = FontWeight.Bold)
                    Text("A household, a club, a trip…", style = MaterialTheme.typography.bodySmall)
                }
                Text("›", style = MaterialTheme.typography.titleLarge, color = MaterialTheme.colorScheme.primary)
            }
        }
    }
}

@Composable
private fun Option(title: String, subtitle: String, primary: Boolean = false, onClick: () -> Unit) {
    Surface(
        shape = RoundedCornerShape(18.dp),
        color = if (primary) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceVariant,
        modifier = Modifier.fillMaxWidth().clickable(onClick = onClick),
    ) {
        Column(Modifier.padding(18.dp)) {
            Text(title, fontWeight = FontWeight.Bold, style = MaterialTheme.typography.titleMedium)
            Text(subtitle, style = MaterialTheme.typography.bodySmall)
        }
    }
}

/** Name a new neighborhood; on create, hands back its id to open its page. */
@Composable
internal fun NewNeighborhoodDialog(client: AppCoreClient, onCreated: (String?) -> Unit, onDismiss: () -> Unit) {
    var name by remember { mutableStateOf("") }
    var open by remember { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("New neighborhood") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(value = name, onValueChange = { name = it }, label = { Text("Name") }, singleLine = true)
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text("Open to anyone nearby")
                        Text(
                            "Anyone using this exact name joins it — for a festival or a city. Leave off for people you invite.",
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
                val before = client.state().neighborhoods.list.map { it.id }.toSet()
                val after = client.dispatch(NativeActions.createNeighborhood(name, open)).neighborhoods.list
                onCreated(after.firstOrNull { it.id !in before }?.id ?: after.firstOrNull { it.name == name.trim() }?.id)
            }) { Text("Create") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** One person: here or not, the neighborhoods you share, and what you can do. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun PersonSheet(
    p: NeighborhoodsState.PeerNode,
    nh: NeighborhoodsState,
    client: AppCoreClient,
    onSendFile: () -> Unit,
    onDismiss: () -> Unit,
) {
    var confirm by remember { mutableStateOf(false) }
    val linkId = p.linkId
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            Modifier.padding(start = 20.dp, end = 20.dp, bottom = 24.dp).verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Avatar(p.name, p.key, here = p.reachable, size = 56.dp)
                Spacer(Modifier.width(14.dp))
                Column {
                    Text(p.name, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
                    Text(
                        if (p.reachable) "Here now" else "Not here right now",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            if (linkId == null) {
                Text("A device you paired with before neighborhoods.", style = MaterialTheme.typography.bodySmall)
                Button(onClick = onSendFile, modifier = Modifier.fillMaxWidth()) { Text("Send a file") }
            } else {
                Label("YOU SHARE")
                if (p.neighborhoods.isEmpty()) Text("Nothing yet.", style = MaterialTheme.typography.bodySmall)
                for (id in p.neighborhoods) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(nh.nameOf(id), Modifier.weight(1f), fontWeight = FontWeight.SemiBold)
                        TextButton(onClick = { client.dispatch(NativeActions.dropFromLink(linkId, id)) }) { Text("Remove") }
                    }
                }
                val others = nh.list.filter { !it.everyone && it.id !in p.neighborhoods }
                if (others.isNotEmpty()) {
                    Label("ADD TO")
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        for (n in others.take(4)) {
                            OutlinedButton(onClick = { client.dispatch(NativeActions.shareOnLink(linkId, n.id)) }) { Text(n.name) }
                        }
                    }
                    Text("They'll be asked first.", style = MaterialTheme.typography.bodySmall)
                }
            }
            HorizontalDivider()
            TextButton(onClick = { confirm = true }) {
                Text(if (linkId == null) "Remove ${p.name}" else "Disconnect from ${p.name}", color = MaterialTheme.colorScheme.error)
            }
        }
    }
    if (confirm) {
        AlertDialog(
            onDismissRequest = { confirm = false },
            title = { Text(if (linkId == null) "Remove ${p.name}?" else "Disconnect from ${p.name}?") },
            text = {
                Text(
                    if (linkId == null) "They leave your paired devices. To connect again, invite them to a neighborhood."
                    else "You stop sharing every neighborhood with them. What you received stays on your phone.",
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    if (linkId == null) client.dispatch(NativeActions.removeFromCircle(p.key))
                    else client.dispatch(NativeActions.endLink(linkId))
                    confirm = false
                    onDismiss()
                }) { Text(if (linkId == null) "Remove" else "Disconnect") }
            },
            dismissButton = { TextButton(onClick = { confirm = false }) { Text("Cancel") } },
        )
    }
}
