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
import app.myco.core.LibraryKind
import app.myco.core.NativeActions
import app.myco.ui.ScreenHeader

import app.myco.ui.theme.tileColorFor
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * **Discover** — an app-drawer of apps to add: a curated **Suggested** row
 * (the bundled bitchat + community nsites, and a few napplets) and **Around
 * you** — nsites your connected Circle peers are hosting, queried over the
 * mesh. Tiles mirror the Apps grid (favicon or lettered fallback; a duck chip
 * marks a napplet). Tapping an nsite opens it exactly like opening a shared
 * app — it starts syncing and shows its live page, pulling from a Circle holder
 * (Around you) or public relays/Blossom (Suggested). Tapping a napplet fetches
 * it and hands over to the install review on the Apps tab; nothing is granted
 * until the user says so there.
 */
@Composable
fun DiscoverScreen(
    state: AppState,
    client: AppCoreClient,
    onLaunchNsite: (host: String, title: String) -> Unit,
    onShowNappletReview: () -> Unit,
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

        // A napplet already in the Library is on the Apps tab; it is not news
        // here. Nsite suggestions stay listed when installed (unchanged).
        val installedNapplets = state.library.filter { it.kind == LibraryKind.Napplet && it.pinned }
        val suggestions = SUGGESTED_APPS.filterNot { s ->
            s is Suggestion.Napplet && installedNapplets.any { it.authorNpub == s.authorNpub && it.dTag == s.dTag }
        }

        item(span = { GridItemSpan(maxLineSpan) }) { SectionLabel("Suggested") }
        items(suggestions, key = { it.key }) { s ->
            when (s) {
                is Suggestion.Nsite -> DiscoverTile(
                    client = client,
                    iconHost = s.host,
                    colorKey = s.host,
                    title = s.title,
                ) {
                    // Same as opening a shared app (minus pairing): kick off the sync
                    // and open its live page. No holder — a public nsite pulls from the
                    // Circle if a peer has it, else public relays/Blossom.
                    client.dispatch(NativeActions.openNsite(s.host))
                    onLaunchNsite(s.host, s.title)
                }
                is Suggestion.Napplet -> DiscoverTile(
                    client = client,
                    iconHost = null,
                    colorKey = s.pointer,
                    title = s.title,
                    napplet = true,
                ) {
                    // The tap fetches and asks; the install action belongs to the
                    // review sheet alone, so a suggestion can never grant a
                    // capability by itself (mirrors MainActivity.handleScannedText).
                    client.dispatch(NativeActions.fetchNapplet(s.pointer))
                    onShowNappletReview()
                }
            }
        }

        // "Around you" is for apps you could add. Two things are therefore not
        // news here: one already offered under Suggested (the tile above opens
        // it), and one you have already pinned — that lives on your Apps tab, and
        // finding it again on a peer's relay does not make it a discovery.
        // `state.discovered` is an nsite search, so only nsite hosts belong here.
        val alreadyOffered = SUGGESTED_APPS.filterIsInstance<Suggestion.Nsite>().map { it.host }.toSet() +
            state.library.filter { it.pinned }.map { it.urlHost }
        val around = state.discovered.filter { it.host !in alreadyOffered }

        item(span = { GridItemSpan(maxLineSpan) }) { SectionLabel("Around you") }
        if (around.isEmpty()) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Text(
                    "Nothing found yet. Make sure a Circle peer is connected, then tap Discover.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(top = 4.dp),
                )
            }
        } else {
            items(around, key = { it.host }) { d ->
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
 *
 * A napplet ([iconHost] null, [napplet] true) has no nsite favicon to fetch; it
 * gets the lettered tile tinted by its pointer (the same key `NappletTile` tints
 * by, so the colour survives install) and the duck chip.
 */
@Composable
private fun DiscoverTile(
    client: AppCoreClient,
    iconHost: String?,
    colorKey: String,
    title: String,
    napplet: Boolean = false,
    onClick: () -> Unit,
) {
    var icon by remember(iconHost) { mutableStateOf<Bitmap?>(null) }
    LaunchedEffect(iconHost) {
        if (iconHost != null && icon == null) {
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
            if (napplet) {
                NappletBadge(modifier = Modifier.align(Alignment.TopEnd).padding(4.dp))
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

/** A curated app suggestion; [key] is the stable grid key. */
private sealed interface Suggestion {
    val title: String
    val key: String

    /**
     * An nsite. [host] is the nsite gateway label (the `nsite.lol` subdomain,
     * i.e. a `<base36-pubkey><d-tag>` named site) — the same string a discovered
     * nsite carries in `host`, so opening one runs the identical path.
     */
    data class Nsite(override val title: String, val host: String) : Suggestion {
        override val key: String get() = "nsite:$host"
    }

    /**
     * A napplet. [pointer] is the `naddr` handed to `fetchNapplet` — an naddr
     * rather than `<npub>:<d>` because its relay hints ride inside it and are
     * where the fetch looks first. [authorNpub] + [dTag] are what the Library
     * keys a napplet by, whatever pointer spelling it was added under; they are
     * how an installed suggestion is recognised.
     */
    data class Napplet(
        override val title: String,
        val pointer: String,
        val authorNpub: String,
        val dTag: String,
    ) : Suggestion {
        override val key: String get() = "napplet:$authorNpub:$dTag"
    }
}

/**
 * Curated starter apps shown in Discover: nsites first, then napplets. `bitchat`
 * and `DingDong` are also the bundled first-run defaults (`DEFAULT_SITES` /
 * `DEFAULT_NAPPLETS` in `myco-core`); listing them here lets a user who removed
 * one get it back.
 */
private val SUGGESTED_APPS: List<Suggestion> = listOf(
    Suggestion.Nsite("bitchat", "4ofb5evx6765n3syphyhlocydo8q7fyipswzgpkx59u7p1yiivbitchat"),
    Suggestion.Nsite("ICS", "4ofb5evx6765n3syphyhlocydo8q7fyipswzgpkx59u7p1yiivics"),
    Suggestion.Nsite("Dumplings", "4ofb5evx6765n3syphyhlocydo8q7fyipswzgpkx59u7p1yiivdumplings"),
    Suggestion.Napplet(
        "Mappy",
        "naddr1qqyx6ctswpkx2arnqgsqhtasevhkqty908ymemjgwuphelgrv33gf62p64ywy5ldum0as5srqsqqpzfe5a247a",
        "npub1pwhmpje0vqkg27wfhnhysacr0n7sxerzsn55r42guff7meklmpfqka6r38",
        "mapplets",
    ),
    Suggestion.Napplet(
        "Minesweeper",
        "naddr1qq9k66twv4ehwet9wpjhyqg4waehxw309aex2mrp0yhxg6t5w3hjuur4vgpzqfngzhsvjggdlgeycm96x4emzjlwf8dyyzdfg4hefp89zpkdgz99qvzqqqyf8yzehfvw",
        "npub1ye5ptcxfyyxl5vjvdjar2ua3f0hynkjzpx552mu5snj3qmx5pzjscpknpr",
        "minesweeper",
    ),
    Suggestion.Napplet(
        "DingDong",
        "naddr1qvzqqqyf8ypzpwa4mkswz4t8j70s2s6q00wzqv7k7zamxrmj2y4fs88aktcfuf68qyt8wumn8ghj7un9d3shjtnswf5k6ctv9ehx2aqpp4mhxue69uhkummn9ekx7mqpz4mhxue69uhhyetvv9ujuerfw36x7tnsw43qqzryd9hxwer0denstp6v0k",
        "npub1hw6amg8p24ne08c9gdq8hhpqx0t0pwanpae9z25crn7m9uy7yarse465gr",
        "dingdong",
    ),
)
