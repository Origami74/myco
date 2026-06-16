package app.myco.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.DiscoveredNsite
import app.myco.core.NativeActions
import app.myco.ui.ScreenHeader
import app.myco.ui.theme.Slate

/**
 * **Discover** — "nsites around me": tap Discover to query connected Circle peers'
 * relays for the nsites they host. Opening a result pulls it from that peer.
 */
@Composable
fun DiscoverScreen(
    state: AppState,
    client: AppCoreClient,
    onLaunchNsite: (host: String, title: String) -> Unit,
) {
    LazyColumn(
        modifier = Modifier.fillMaxSize(),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(20.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item {
            ScreenHeader("Discover", state, subtitle = "nsites your Circle is hosting, over the mesh.")
            Spacer(Modifier.height(8.dp))
            OutlinedButton(onClick = { client.dispatch(NativeActions.searchNsites()) }) {
                Icon(Icons.Filled.Refresh, contentDescription = null)
                Spacer(Modifier.size(8.dp))
                Text(if (state.discovered.isEmpty()) "Discover" else "Refresh")
            }
        }
        if (state.discovered.isEmpty()) {
            item {
                Text(
                    "Nothing found yet. Make sure a Circle peer is connected, then tap Discover.",
                    color = Slate,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(top = 12.dp),
                )
            }
        } else {
            items(state.discovered, key = { it.host + it.holderNpub }) { d ->
                DiscoveredCard(d) {
                    client.dispatch(NativeActions.openNsite(d.host, d.holderNpub))
                    onLaunchNsite(d.host, d.title)
                }
            }
        }
    }
}

@Composable
private fun DiscoveredCard(d: DiscoveredNsite, onOpen: () -> Unit) {
    Surface(
        shape = RoundedCornerShape(18.dp),
        color = MaterialTheme.colorScheme.surfaceVariant,
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    d.title.ifEmpty { d.host.take(12) },
                    fontWeight = FontWeight.Bold,
                    style = MaterialTheme.typography.titleMedium,
                )
                Text(
                    "from ${d.holderName.ifEmpty { "a peer" }}",
                    color = Slate,
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
            Button(onClick = onOpen) { Text("Open") }
        }
    }
}
