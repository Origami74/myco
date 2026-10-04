package app.myco.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Public
import androidx.compose.material.icons.filled.Star
import androidx.compose.material3.Checkbox
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.NativeActions
import app.myco.core.PublicNode
import app.myco.ui.GroupLabel
import app.myco.ui.SectionCard
import app.myco.ui.theme.StatusConnected
import java.text.DateFormat
import java.util.Date

/**
 * Settings › Internet: peer with public FIPS nodes over the internet, so the
 * Circle reaches members who are not in the same room (roadmap N10).
 *
 * Off by default, and the page says what turning it on costs before the
 * switch: a public node learns this phone's IP address and which mesh
 * addresses it talks to, never what is said. The list is every dialable node
 * heard of on Nostr, the ones join.fips.network recommends first and starred;
 * those are selected unless the user unticks them.
 */
@Composable
internal fun PublicNodesSettings(state: AppState, client: AppCoreClient, onBack: () -> Unit) {
    val pub = state.publicNodes
    // Read the adverts on open, so the list is there to look at before the
    // user opts in. The core still refuses while mesh-only is on.
    LaunchedEffect(Unit) { client.dispatch(NativeActions.refreshPublicNodes()) }

    SettingsColumn {
        SubHeader("Internet", onBack)
        Text(
            "Reach your Circle when you're not in the same room. When this phone " +
                "is online, Myco also links to public mesh nodes on the internet, " +
                "and your messages and apps travel through them.",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )

        SectionCard {
            Row(
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                LeadingIcon(Icons.Filled.Public)
                Spacer(Modifier.size(14.dp))
                Column(modifier = Modifier.weight(1f)) {
                    Text(
                        "Use public nodes",
                        fontWeight = FontWeight.SemiBold,
                        style = MaterialTheme.typography.titleMedium,
                    )
                    Text(
                        statusLine(state),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                Switch(
                    checked = pub.enabled,
                    onCheckedChange = { client.dispatch(NativeActions.setPublicNodesEnabled(it)) },
                )
            }
        }

        // The trade-off, plainly, next to the switch rather than behind a link.
        Text(
            "A public node can see this phone's IP address and which mesh " +
                "addresses it talks to. It can't see what you send — that stays " +
                "encrypted end to end. Nodes you don't know are run by people you " +
                "don't know.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )

        val recommended = pub.nodes.filter { it.recommended }
        val others = pub.nodes.filter { !it.recommended }

        GroupLabel("RECOMMENDED")
        SectionCard {
            recommended.forEachIndexed { i, node ->
                if (i > 0) RowDivider()
                NodeRow(node) { selected ->
                    client.dispatch(NativeActions.setPublicNodeSelected(node.npub, selected))
                }
            }
        }
        Text(
            "★ Recommended by join.fips.network · " + if (pub.recommendedUpdatedMs > 0) {
                "list updated ${DateFormat.getDateInstance().format(Date(pub.recommendedUpdatedMs))}"
            } else {
                "list shipped with Myco"
            },
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )

        if (others.isNotEmpty()) {
            GroupLabel("OTHER NODES ON NOSTR")
            SectionCard {
                others.forEachIndexed { i, node ->
                    if (i > 0) RowDivider()
                    NodeRow(node) { selected ->
                        client.dispatch(NativeActions.setPublicNodeSelected(node.npub, selected))
                    }
                }
            }
        }

        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                when {
                    pub.fetching -> "Looking for nodes…"
                    pub.lastFetchMs == 0L -> "Not looked for nodes yet."
                    pub.fetchError.isNotEmpty() -> pub.fetchError.replaceFirstChar { it.uppercase() } + "."
                    else -> "Found ${pub.nodes.count { it.advertised }} nodes on " +
                        "${pub.relaysAnswered} of ${pub.relaysAsked} relays."
                },
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.weight(1f),
            )
            TextButton(
                onClick = { client.dispatch(NativeActions.refreshPublicNodes()) },
                enabled = !pub.fetching && !state.offlineOnly,
            ) { Text("Refresh") }
        }
    }
}

private fun statusLine(state: AppState): String {
    val pub = state.publicNodes
    val up = pub.nodes.count { it.state == "connected" }
    return when {
        state.offlineOnly -> "Off while mesh-only is on"
        !pub.enabled -> "Off"
        pub.blocked == "no-internet" -> "Waiting for the internet"
        up > 0 -> "Linked to $up public node${if (up == 1) "" else "s"}"
        else -> "Connecting…"
    }
}

@Composable
private fun NodeRow(node: PublicNode, onSelect: (Boolean) -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable { onSelect(!node.selected) }
            .padding(start = 16.dp, end = 8.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(modifier = Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (node.recommended) {
                    Icon(
                        Icons.Filled.Star,
                        contentDescription = "Recommended",
                        tint = MaterialTheme.colorScheme.primary,
                        modifier = Modifier.size(16.dp),
                    )
                    Spacer(Modifier.size(6.dp))
                }
                Text(
                    node.name,
                    fontWeight = if (node.recommended) FontWeight.SemiBold else FontWeight.Normal,
                    style = MaterialTheme.typography.bodyLarge,
                )
            }
            Text(
                nodeLine(node),
                color = if (node.state == "connected") StatusConnected else MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
            )
        }
        Checkbox(checked = node.selected, onCheckedChange = onSelect)
    }
}

private fun nodeLine(node: PublicNode): String {
    val where = node.endpoint.ifEmpty { "not advertising now" }
    val what = when (node.state) {
        "connected" -> "linked" + (node.srttMs?.let { " · ${"%.0f".format(it)} ms" } ?: "")
        "connecting" -> "connecting"
        "waiting" -> "will retry"
        else -> ""
    }
    return if (what.isEmpty()) where else "$where · $what"
}
