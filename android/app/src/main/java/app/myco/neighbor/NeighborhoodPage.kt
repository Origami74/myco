package app.myco.neighbor

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.ExitToApp
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Upload
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.Groups
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.People
import androidx.compose.material.icons.filled.PersonAdd
import androidx.compose.material.icons.filled.Public
import androidx.compose.material.icons.filled.Tune
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
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
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.NativeActions
import app.myco.ui.screens.SheetAction
import app.myco.ui.screens.SheetIconAction

/** A reach preset's icon. */
private fun Reach.icon(): ImageVector = when (this) {
    Reach.Close -> Icons.Filled.Home
    Reach.Friends -> Icons.Filled.People
    Reach.Wide -> Icons.Filled.Public
}

/** A reach preset's short name, for under its icon. */
private fun Reach.short(): String = when (this) {
    Reach.Close -> "Close"
    Reach.Friends -> "Friends+"
    Reach.Wide -> "Wide"
}

/**
 * One neighborhood, in the app sheet's style: tile, name and a status line;
 * an icon row (rename, reach, files); the people you're connected with as
 * faces; how far posts go as three icons; updates in and out as two
 * switches; Leave as a red row; Invite pinned
 * at the bottom. Finer settings open in their own small sheets.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NeighborhoodPage(id: String, state: AppState, client: AppCoreClient, onBack: () -> Unit) {
    val nh = state.neighborhoods
    val n = nh.list.firstOrNull { it.id == id }
    if (n == null) {
        androidx.compose.runtime.LaunchedEffect(Unit) { onBack() }
        return
    }
    var person by remember { mutableStateOf<String?>(null) }
    var inviting by remember { mutableStateOf(false) }
    var sheet by remember { mutableStateOf<String?>(null) }
    var leaving by remember { mutableStateOf(false) }
    val people = nh.peopleIn(n.id).distinctBy { it.name.lowercase() }
    val hereCount = people.count { it.reachable }
    val color = bubbleColor(n)
    val reach = Reach.of(n)

    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 96.dp)) {
            IconButton(onClick = onBack, modifier = Modifier.padding(8.dp)) {
                Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
            }
            Column(Modifier.padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(20.dp)) {
                // Tile, name, one line.
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(
                        Modifier.size(64.dp).background(color.copy(alpha = 0.2f), RoundedCornerShape(18.dp)),
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(Icons.Filled.Groups, contentDescription = null, tint = color, modifier = Modifier.size(34.dp))
                    }
                    Spacer(Modifier.width(16.dp))
                    Column {
                        Text(n.name, style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold)
                        Text(
                            listOfNotNull(
                                if (n.autoAccept) "Open" else "Invite only",
                                if (hereCount > 0) "$hereCount here" else null,
                            ).joinToString(" · "),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }

                // Actions as icons.
                Row(Modifier.fillMaxWidth()) {
                    SheetIconAction(Icons.Filled.Edit, "Rename", Modifier.weight(1f)) { sheet = "rename" }
                    SheetIconAction(Icons.Filled.Tune, "Reach", Modifier.weight(1f)) { sheet = "reach" }
                    SheetIconAction(Icons.Filled.Folder, "Files", Modifier.weight(1f)) { sheet = "files" }
                }

                // People as faces.
                Surface(shape = RoundedCornerShape(20.dp), color = MaterialTheme.colorScheme.surfaceVariant) {
                    Column(Modifier.fillMaxWidth().padding(16.dp)) {
                        Label("PEOPLE")
                        Spacer(Modifier.height(12.dp))
                        if (people.isEmpty()) {
                            Text("Just you", color = MaterialTheme.colorScheme.onSurfaceVariant)
                        } else {
                            LazyRow(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                                items(people, key = { it.key }) { p ->
                                    Column(
                                        horizontalAlignment = Alignment.CenterHorizontally,
                                        modifier = Modifier.width(56.dp).clickable { person = p.key },
                                    ) {
                                        Avatar(p.name, p.key, here = p.reachable, size = 48.dp)
                                        Spacer(Modifier.height(4.dp))
                                        Text(p.name, style = MaterialTheme.typography.labelSmall, maxLines = 1)
                                    }
                                }
                            }
                        }
                    }
                }

                // How far posts go, as three icons.
                Surface(shape = RoundedCornerShape(20.dp), color = MaterialTheme.colorScheme.surfaceVariant) {
                    Column(Modifier.fillMaxWidth().padding(16.dp)) {
                        Label("POSTS REACH")
                        Spacer(Modifier.height(8.dp))
                        Row(Modifier.fillMaxWidth()) {
                            for (r in Reach.entries) {
                                ReachChoice(r, selected = reach == r, color = color, modifier = Modifier.weight(1f)) {
                                    client.dispatch(NativeActions.setNeighborhoodPolicy(n.id, r.publish, r.backlog, r.passOn))
                                }
                            }
                        }
                    }
                }

                // Updates, both ways, for apps, sites and Myco at once. The core
                // keeps a setting per kind; these set all three together.
                Surface(shape = RoundedCornerShape(20.dp), color = MaterialTheme.colorScheme.surfaceVariant) {
                    Column(Modifier.fillMaxWidth().padding(vertical = 6.dp)) {
                        UpdatesSwitch(
                            icon = Icons.Filled.Download,
                            title = "Install updates from people here",
                            on = EXTENSIONS.all { it in n.get },
                        ) { on -> EXTENSIONS.forEach { client.dispatch(NativeActions.setNeighborhoodExtension(n.id, it, share = false, enabled = on)) } }
                        UpdatesSwitch(
                            icon = Icons.Filled.Upload,
                            title = "Send my updates to people here",
                            on = EXTENSIONS.all { it in n.share },
                        ) { on -> EXTENSIONS.forEach { client.dispatch(NativeActions.setNeighborhoodExtension(n.id, it, share = true, enabled = on)) } }
                        Text(
                            "Apps, sites and Myco",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(start = 52.dp, bottom = 8.dp),
                        )
                    }
                }

                SheetAction(
                    Icons.AutoMirrored.Filled.ExitToApp,
                    "Leave neighborhood",
                    tint = MaterialTheme.colorScheme.error,
                ) { leaving = true }
            }
        }

        // Invite, pinned where Home has it.
        Button(
            onClick = { inviting = true },
            modifier = Modifier.align(Alignment.BottomCenter).fillMaxWidth().padding(20.dp).height(56.dp),
        ) {
            Icon(Icons.Filled.PersonAdd, contentDescription = null)
            Spacer(Modifier.width(8.dp))
            Text("Invite", maxLines = 1)
        }
    }

    if (inviting) {
        InviteSheet(state = state, client = client, preselect = listOf(n.id), onDismiss = { inviting = false })
    }
    nh.peers.firstOrNull { it.key == person }?.let { p ->
        PersonSheet(p, nh, client, onDismiss = { person = null })
    }
    when (sheet) {
        "rename" -> RenameDialog(n, client) { sheet = null }
        "reach" -> ModalBottomSheet(onDismissRequest = { sheet = null }) { ReachSheet(n, client) }
        "files" -> ModalBottomSheet(onDismissRequest = { sheet = null }) { FilesSheet(n, client) }
    }
    if (leaving) {
        AlertDialog(
            onDismissRequest = { leaving = false },
            icon = { Icon(Icons.AutoMirrored.Filled.ExitToApp, contentDescription = null) },
            title = { Text("Leave ${n.name}?") },
            text = { Text("What you received stays on your phone.") },
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
private fun ReachChoice(r: Reach, selected: Boolean, color: Color, modifier: Modifier, onClick: () -> Unit) {
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        modifier = modifier.clickable(onClick = onClick).padding(vertical = 6.dp),
    ) {
        Box(
            Modifier.size(48.dp).background(if (selected) color else MaterialTheme.colorScheme.surface, RoundedCornerShape(16.dp)),
            contentAlignment = Alignment.Center,
        ) {
            Icon(r.icon(), contentDescription = null, tint = if (selected) Color.White else MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Spacer(Modifier.height(6.dp))
        Text(
            r.short(),
            style = MaterialTheme.typography.labelMedium,
            fontWeight = if (selected) FontWeight.Bold else FontWeight.Normal,
            textAlign = TextAlign.Center,
        )
    }
}

@Composable
private fun RenameDialog(n: NeighborhoodsState.Neighborhood, client: AppCoreClient, onDone: () -> Unit) {
    var name by remember(n.id) { mutableStateOf(n.name) }
    AlertDialog(
        onDismissRequest = onDone,
        icon = { Icon(Icons.Filled.Edit, contentDescription = null) },
        title = { Text("Rename") },
        text = {
            OutlinedTextField(
                value = name,
                onValueChange = { name = it },
                singleLine = true,
                supportingText = { Text("Only on this phone") },
            )
        },
        confirmButton = {
            TextButton(enabled = name.isNotBlank(), onClick = {
                client.dispatch(NativeActions.renameNeighborhood(n.id, name))
                onDone()
            }) { Text("Save") }
        },
        dismissButton = { TextButton(onClick = onDone) { Text("Cancel") } },
    )
}

/** Reach in detail: the knobs behind the presets. */
@Composable
private fun ReachSheet(n: NeighborhoodsState.Neighborhood, client: AppCoreClient) {
    val set = { publish: Int, backlog: Int, passOn: Boolean ->
        client.dispatch(NativeActions.setNeighborhoodPolicy(n.id, publish, backlog, passOn))
    }
    Column(Modifier.padding(start = 20.dp, end = 20.dp, bottom = 28.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text("Reach", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
        Spacer(Modifier.height(8.dp))
        Steps("Posts travel", n.publishHops) { set(it, n.backlogHops, n.passOn) }
        Steps("Catch up from", n.backlogHops) { set(n.publishHops, it, n.passOn) }
        HorizontalDivider(Modifier.padding(vertical = 8.dp))
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("Pass on others' posts", Modifier.weight(1f))
            Switch(checked = n.passOn, onCheckedChange = { set(n.publishHops, n.backlogHops, it) })
        }
    }
}

/** Steps out, 0–3, as round buttons. */
@Composable
private fun Steps(label: String, value: Int, onPick: (Int) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth().padding(vertical = 6.dp)) {
        Text(label, Modifier.weight(1f))
        for (i in 0..3) {
            val chosen = i == value
            Surface(
                shape = RoundedCornerShape(50),
                color = if (chosen) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.surfaceVariant,
                modifier = Modifier.padding(start = 6.dp).size(36.dp).clickable { onPick(i) },
            ) {
                Box(contentAlignment = Alignment.Center) {
                    Text(
                        if (i == 0) "off" else "$i",
                        style = MaterialTheme.typography.labelMedium,
                        color = if (chosen) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.onSurface,
                    )
                }
            }
        }
    }
}

/** Files: sent when asked, or ahead to the people here. */
@Composable
private fun FilesSheet(n: NeighborhoodsState.Neighborhood, client: AppCoreClient) {
    Column(Modifier.padding(start = 20.dp, end = 20.dp, bottom = 28.dp)) {
        Text("Files", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
        Spacer(Modifier.height(8.dp))
        for ((i, label) in listOf("Send when asked", "Send ahead to people here").withIndex()) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.fillMaxWidth().clickable { client.dispatch(NativeActions.setNeighborhoodBlobHops(n.id, i)) },
            ) {
                RadioButton(selected = n.blobHops.coerceAtMost(1) == i, onClick = {
                    client.dispatch(NativeActions.setNeighborhoodBlobHops(n.id, i))
                })
                Text(label)
            }
        }
    }
}

/** The update kinds the switches cover: napplets, nsites, Myco itself. */
private val EXTENSIONS = listOf("napplet-updates", "nsite-updates", "myco-release")

/** One updates switch: icon, a plain sentence, and the switch. */
@Composable
private fun UpdatesSwitch(icon: ImageVector, title: String, on: Boolean, onChange: (Boolean) -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(icon, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Spacer(Modifier.width(12.dp))
        Text(title, Modifier.weight(1f))
        Switch(checked = on, onCheckedChange = onChange)
    }
}
