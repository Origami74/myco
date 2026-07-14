package app.myco.ui.screens

import android.graphics.Bitmap
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.myco.NsiteIcons
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.NativeActions
import app.myco.ui.ScreenHeader
import app.myco.ui.theme.Slate
import app.myco.ui.theme.tileColorFor
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * **Discover** — an app-drawer of nsites to add: a curated **Suggested** row
 * (the bundled bitchat + community apps) and **Around you** — nsites your
 * connected Circle peers are hosting, queried over the mesh. Tiles mirror the
 * Apps grid (favicon or lettered fallback); tapping an nsite opens it exactly
 * like opening a shared app — it starts syncing and shows its live page, pulling
 * from a Circle holder (Around you) or public relays/Blossom (Suggested).
 */
@Composable
fun DiscoverScreen(
    state: AppState,
    client: AppCoreClient,
    onLaunchNsite: (host: String, title: String) -> Unit,
) {
    // Auto-run discovery when the screen first appears, so results show without a
    // manual tap (the button stays available as Refresh).
    LaunchedEffect(Unit) {
        client.dispatch(NativeActions.searchNsites())
    }
    LazyVerticalGrid(
        columns = GridCells.Fixed(4),
        modifier = Modifier.fillMaxSize(),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(20.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalArrangement = Arrangement.spacedBy(18.dp),
    ) {
        item(span = { GridItemSpan(maxLineSpan) }) {
            Column {
                ScreenHeader("Discover", state, subtitle = "apps to add — suggested, and nsites your Circle is hosting.")
                Spacer(Modifier.height(8.dp))
                OutlinedButton(onClick = { client.dispatch(NativeActions.searchNsites()) }) {
                    Icon(Icons.Filled.Refresh, contentDescription = null)
                    Spacer(Modifier.size(8.dp))
                    Text(if (state.discovered.isEmpty()) "Discover" else "Refresh")
                }
            }
        }

        item(span = { GridItemSpan(maxLineSpan) }) { SectionLabel("Suggested") }
        items(SUGGESTED_APPS, key = { it.host }) { app ->
            DiscoverTile(
                client = client,
                iconHost = app.host,
                colorKey = app.host,
                title = app.title,
            ) {
                // Same as opening a shared app (minus pairing): kick off the sync
                // and open its live page. No holder — a public nsite pulls from the
                // Circle if a peer has it, else public relays/Blossom.
                client.dispatch(NativeActions.openNsite(app.host))
                onLaunchNsite(app.host, app.title)
            }
        }

        item(span = { GridItemSpan(maxLineSpan) }) { SectionLabel("Around you") }
        if (state.discovered.isEmpty()) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Text(
                    "Nothing found yet. Make sure a Circle peer is connected, then tap Discover.",
                    color = Slate,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(top = 4.dp),
                )
            }
        } else {
            items(state.discovered, key = { it.host + it.holderNpub }) { d ->
                DiscoverTile(
                    client = client,
                    iconHost = d.host,
                    colorKey = d.host,
                    title = d.title.ifEmpty { d.host.take(8) },
                ) {
                    client.dispatch(NativeActions.openNsite(d.host, d.holderNpub))
                    onLaunchNsite(d.host, d.title)
                }
            }
        }
    }
}

@Composable
private fun SectionLabel(text: String) {
    Text(
        text,
        fontWeight = FontWeight.Bold,
        style = MaterialTheme.typography.titleMedium,
        modifier = Modifier.padding(top = 4.dp),
    )
}

/**
 * A Discover grid tile, styled like an Apps-drawer icon: the nsite's favicon when
 * one can be fetched locally (installed / already-pulled sites), otherwise a
 * lettered tile tinted by [colorKey].
 */
@Composable
private fun DiscoverTile(
    client: AppCoreClient,
    iconHost: String,
    colorKey: String,
    title: String,
    onClick: () -> Unit,
) {
    var icon by remember(iconHost) { mutableStateOf<Bitmap?>(null) }
    LaunchedEffect(iconHost) {
        if (icon == null) {
            icon = withContext(Dispatchers.IO) {
                runCatching { NsiteIcons.fetch(client, "$iconHost.localhost") }.getOrNull()
            }
        }
    }
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        modifier = Modifier.clickable(onClick = onClick),
    ) {
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(1f)
                .clip(RoundedCornerShape(18.dp))
                .background(if (icon == null) tileColorFor(colorKey) else MaterialTheme.colorScheme.surfaceVariant),
            contentAlignment = Alignment.Center,
        ) {
            val bmp = icon
            if (bmp != null) {
                Image(bmp.asImageBitmap(), contentDescription = null, modifier = Modifier.fillMaxSize())
            } else {
                Text(
                    initialOf(title),
                    color = Color.White,
                    fontWeight = FontWeight.Bold,
                    style = MaterialTheme.typography.titleLarge,
                )
            }
        }
        Spacer(Modifier.height(6.dp))
        Text(
            title,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            style = MaterialTheme.typography.labelMedium,
            textAlign = TextAlign.Center,
        )
    }
}

private fun initialOf(label: String): String =
    label.firstOrNull { it.isLetterOrDigit() }?.uppercase() ?: "?"

/**
 * A curated app suggestion. [host] is the nsite gateway label (the `nsite.lol`
 * subdomain, i.e. a `<base36-pubkey><d-tag>` named site) — the same string a
 * discovered nsite carries in `host`, so opening one runs the identical path.
 */
private data class SuggestedApp(val title: String, val host: String)

/**
 * Curated starter apps shown in Discover. `bitchat` is also the bundled first-run
 * default (`DEFAULT_SITES` in `myco-core`); listing it here lets a user who wiped
 * it get it back.
 */
private val SUGGESTED_APPS = listOf(
    SuggestedApp("bitchat", "4ofb5evx6765n3syphyhlocydo8q7fyipswzgpkx59u7p1yiivbitchat"),
    SuggestedApp("ICS", "4ofb5evx6765n3syphyhlocydo8q7fyipswzgpkx59u7p1yiivics"),
)
