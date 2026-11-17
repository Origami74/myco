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
import androidx.compose.material.icons.filled.PersonAdd
import androidx.compose.material.icons.filled.QrCodeScanner
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
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
import app.myco.ui.ScreenHeader

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
) {
    val nh = state.neighborhoods
    val context = LocalContext.current
    val prefs = remember { context.getSharedPreferences("neighbors", android.content.Context.MODE_PRIVATE) }
    var noticeSeen by remember { mutableStateOf(prefs.getBoolean("circle_removed_seen", false)) }
    var showMap by remember { mutableStateOf(false) }
    var person by remember { mutableStateOf<String?>(null) }
    var inviting by remember { mutableStateOf(false) }
    var joining by remember { mutableStateOf(false) }

    val here = nh.hereNow()
    // Nearby has its own row, with its switch, whether on or off.
    val groups = nh.list.filter { !it.nearby }
    val nearby = nh.list.firstOrNull { it.nearby }

    Box(Modifier.fillMaxSize()) {
        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(start = 20.dp, end = 20.dp, top = 20.dp, bottom = 160.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            // The mesh pill on top, as on Settings.
            item { ScreenHeader("Neighbors", state) }

            if (!noticeSeen) {
                item {
                    Banner(
                        title = "The Circle is gone",
                        body = "People now connect through neighborhoods. Tap Invite to show a code, or Join to scan one.",
                        action = "Got it",
                        onAction = {
                            prefs.edit().putBoolean("circle_removed_seen", true).apply()
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
                    if (nh.list.isNotEmpty()) {
                        TextButton(onClick = { showMap = !showMap }) { Text(if (showMap) "List" else "Map") }
                    }
                }
            }

            if (!showMap) {
                item(key = "nearby") {
                    NearbyCard(
                        nh = nh,
                        neighborhood = nearby,
                        onToggle = { on -> client.dispatch(NativeActions.setNearby(on)) },
                        onOpen = { nearby?.let { onOpenNeighborhood(it.id) } },
                    )
                }
            }

            if (groups.isEmpty() && !showMap) {
                item {
                    Banner(
                        title = "Start your first neighborhood",
                        body = "A household, a club, a trip. Name it and show the code — or tap Join if someone invited you.",
                        action = "Start one",
                        onAction = { inviting = true },
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

            if (nh.status.isNotEmpty()) {
                item { Text(nh.status, style = MaterialTheme.typography.bodySmall) }
            }
        }

        // Join (scan) bottom-right, Invite above it.
        app.myco.ui.ActionBubbles(
            main = app.myco.ui.BubbleAction(Icons.Filled.QrCodeScanner, "Join") { joining = true },
            above = app.myco.ui.BubbleAction(Icons.Filled.PersonAdd, "Invite") { inviting = true },
            modifier = Modifier.align(Alignment.BottomEnd),
        )
    }

    if (inviting) {
        InviteSheet(state = state, client = client, preselect = emptyList(), onDismiss = { inviting = false })
    }
    if (joining) {
        JoinSheet(onScanned = onScanned, onDismiss = { joining = false })
    }
    nh.peers.firstOrNull { it.key == person }?.let { p ->
        PersonSheet(p, nh, client, onDismiss = { person = null })
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

/**
 * Nearby's row: people in reach right now, with the switch for it on the
 * card itself. Off, the row stays so it can be turned back on.
 */
@Composable
private fun NearbyCard(
    nh: NeighborhoodsState,
    neighborhood: NeighborhoodsState.Neighborhood?,
    onToggle: (Boolean) -> Unit,
    onOpen: () -> Unit,
) {
    val on = nh.nearby.on
    val people = if (on) nh.peopleIn(nh.nearby.id) else emptyList()
    val hereCount = people.count { it.reachable }
    Surface(
        shape = RoundedCornerShape(18.dp),
        color = MaterialTheme.colorScheme.surfaceVariant,
        modifier = Modifier.fillMaxWidth().clickable(enabled = on && neighborhood != null, onClick = onOpen),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            val stripe = neighborhood?.let { bubbleColor(it) } ?: MaterialTheme.colorScheme.outline
            Box(Modifier.width(6.dp).height(76.dp).background(if (on) stripe else stripe.copy(alpha = 0.3f)))
            Column(Modifier.weight(1f).padding(horizontal = 16.dp, vertical = 12.dp)) {
                Text("Nearby", fontWeight = FontWeight.Bold, style = MaterialTheme.typography.titleMedium)
                Text(
                    when {
                        !on -> "Off — phones around you can't see you"
                        hereCount > 0 -> (withNames(people) ?: "") + " · $hereCount here now"
                        else -> "People around you appear here"
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Switch(checked = on, onCheckedChange = onToggle, modifier = Modifier.padding(end = 16.dp))
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

/** One person: here or not, the neighborhoods you share, and what you can do. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun PersonSheet(
    p: NeighborhoodsState.PeerNode,
    nh: NeighborhoodsState,
    client: AppCoreClient,
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
            if (linkId != null) {
                Label("YOU SHARE")
                if (p.neighborhoods.isEmpty()) Text("Nothing yet.", style = MaterialTheme.typography.bodySmall)
                for (id in p.neighborhoods) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(nh.nameOf(id), Modifier.weight(1f), fontWeight = FontWeight.SemiBold)
                        if (id != nh.nearby.id) {
                            TextButton(onClick = { client.dispatch(NativeActions.dropFromLink(linkId, id)) }) { Text("Remove") }
                        }
                    }
                }
                // Nearby comes and goes with being in reach; it isn't added by hand.
                val others = nh.list.filter { it.id !in p.neighborhoods && !it.nearby }
                // A nearby link carries open neighborhoods only, and adds them by itself.
                if (others.isNotEmpty() && !p.nearby) {
                    Label("ADD TO")
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        for (n in others.take(4)) {
                            OutlinedButton(onClick = { client.dispatch(NativeActions.shareOnLink(linkId, n.id)) }) { Text(n.name) }
                        }
                    }
                    Text("They'll be asked first.", style = MaterialTheme.typography.bodySmall)
                }
            }
            if (linkId != null) {
                HorizontalDivider()
                TextButton(onClick = { confirm = true }) {
                    Text("Disconnect from ${p.name}", color = MaterialTheme.colorScheme.error)
                }
            }
        }
    }
    if (confirm && linkId != null) {
        AlertDialog(
            onDismissRequest = { confirm = false },
            title = { Text("Disconnect from ${p.name}?") },
            text = { Text("You stop sharing every neighborhood with them. What you received stays on your phone.") },
            confirmButton = {
                TextButton(onClick = {
                    client.dispatch(NativeActions.endLink(linkId))
                    confirm = false
                    onDismiss()
                }) { Text("Disconnect") }
            },
            dismissButton = { TextButton(onClick = { confirm = false }) { Text("Cancel") } },
        )
    }
}
