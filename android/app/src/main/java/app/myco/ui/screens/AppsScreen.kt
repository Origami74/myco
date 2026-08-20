package app.myco.ui.screens

import android.graphics.Bitmap
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
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
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.HomeMax
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
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
import app.myco.core.LibraryItem
import app.myco.core.LibraryKind
import app.myco.core.NappletReview
import app.myco.core.SiteStatus
import app.myco.nfc.NfcReader
import app.myco.nfc.PairPresent
import app.myco.share.NsiteShare
import app.myco.ui.ScreenHeader

import app.myco.ui.theme.tileColorFor
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * The **Apps** surface: an app-drawer of installed nsites (icon grid + search),
 * each opening as its own fullscreen task. Long-press an app for its sheet
 * (share, add-to-home, info); the "+" tile adds one by pasting a link or scanning.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalFoundationApi::class)
@Composable
fun AppsScreen(
    state: AppState,
    client: AppCoreClient,
    onLaunchNsite: (host: String, title: String) -> Unit,
    onLaunchNapplet: (pointer: String, title: String) -> Unit,
    onPinToHome: (host: String, title: String) -> Unit,
    onScanned: (String) -> Unit,
) {
    var query by remember { mutableStateOf("") }
    var sheetFor by remember { mutableStateOf<SiteStatus?>(null) }
    var shareFor by remember { mutableStateOf<ShareTarget?>(null) }
    var confirmRemove by remember { mutableStateOf<SiteStatus?>(null) }
    var showAdd by remember { mutableStateOf(false) }
    var nappletSheetFor by remember { mutableStateOf<LibraryItem?>(null) }
    var confirmForgetNapplet by remember { mutableStateOf<LibraryItem?>(null) }

    // One-shot toast with the result of a "Check for updates" run (fires when the
    // core bumps the check generation), so the user gets explicit feedback.
    val context = androidx.compose.ui.platform.LocalContext.current
    var lastCheckGen by remember { mutableStateOf(state.updateCheck.generation) }
    LaunchedEffect(state.updateCheck.generation) {
        if (state.updateCheck.generation != lastCheckGen) {
            lastCheckGen = state.updateCheck.generation
            if (state.updateCheck.message.isNotBlank()) {
                android.widget.Toast.makeText(context, state.updateCheck.message, android.widget.Toast.LENGTH_SHORT).show()
            }
        }
    }

    // nsites and napplets share one grid — they arrive the same way and open the
    // same way. What differs is the trust model, and the badge says which.
    val napplets = state.library.filter { it.kind == LibraryKind.Napplet }
    val apps: List<AppEntry> = buildList {
        state.sites.forEach { add(AppEntry.Nsite(it)) }
        napplets.forEach { add(AppEntry.Napplet(it)) }
    }.filter {
        query.isBlank() || it.title.contains(query, true) || it.searchKey.contains(query, true)
    }.sortedBy { it.title.ifEmpty { it.searchKey }.lowercase() }

    LazyVerticalGrid(
        columns = GridCells.Fixed(4),
        modifier = Modifier.fillMaxSize(),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(20.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalArrangement = Arrangement.spacedBy(18.dp),
    ) {
        item(span = { GridItemSpan(maxLineSpan) }) {
            Column {
                ScreenHeader("Apps", state)
                Spacer(Modifier.height(16.dp))
                SearchField(query) { query = it }
                Spacer(Modifier.height(4.dp))
            }
        }
        items(apps, key = { it.key }) { entry ->
            when (entry) {
                is AppEntry.Nsite -> NsiteTile(
                    client = client,
                    site = entry.site,
                    modifier = Modifier.animateItem(),
                    // Ready → open the app; still downloading → its live status page.
                    onClick = { onLaunchNsite(entry.site.host, entry.site.title) },
                    onLongClick = { sheetFor = entry.site },
                )
                is AppEntry.Napplet -> NappletTile(
                    item = entry.item,
                    modifier = Modifier.animateItem(),
                    onClick = { onLaunchNapplet(entry.item.nappletPointer, entry.item.title) },
                    onLongClick = { nappletSheetFor = entry.item },
                )
            }
        }
        item {
            AddTile { showAdd = true }
        }
        if (apps.isEmpty() && query.isBlank()) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Text(
                    "No apps yet — scan a friend's share QR or paste a link to add one.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    textAlign = TextAlign.Center,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.fillMaxWidth().padding(top = 24.dp),
                )
            }
        }
    }

    sheetFor?.let { site ->
        ModalBottomSheet(onDismissRequest = { sheetFor = null }) {
            AppSheet(
                client = client,
                site = site,
                onOpen = { sheetFor = null; onLaunchNsite(site.host, site.title) },
                onShare = {
                    val uri = NsiteShare.buildShareUri(
                        nsiteHost = site.host,
                        deviceNpub = state.ownNpub,
                        deviceName = NsiteShare.deviceName(state.ownNpub),
                        pairSecret = NsiteShare.newPairSecret(),
                    )
                    shareFor = ShareTarget(uri = uri, title = site.title.ifEmpty { site.host.take(12) })
                    sheetFor = null
                },
                onPinToHome = { sheetFor = null; onPinToHome(site.host, site.title) },
                onRemove = { sheetFor = null; confirmRemove = site },
            )
        }
    }

    shareFor?.let { target ->
        ShareQrSheet(
            target = target,
            pendingRequestCount = state.pendingPairRequests.size,
            onDismiss = { shareFor = null },
        )
    }

    if (showAdd) {
        AddAppSheet(
            siteCount = state.sites.size,
            onScanned = { showAdd = false; onScanned(it) },
            onDismiss = { showAdd = false },
        )
    }

    // Review is driven by state, not by a local flag: a fetch may finish while
    // the user is elsewhere, and the question should still be waiting.
    state.nappletReview?.let { review ->
        NappletReviewSheet(
            review = review,
            onInstall = { granted ->
                client.dispatch(NativeActions.installNapplet(review.pointer, granted))
            },
            onDismiss = { client.dispatch(NativeActions.dismissNappletReview()) },
        )
    }

    nappletSheetFor?.let { item ->
        ModalBottomSheet(onDismissRequest = { nappletSheetFor = null }) {
            NappletSheet(
                item = item,
                onOpen = {
                    nappletSheetFor = null
                    onLaunchNapplet(item.nappletPointer, item.title)
                },
                onShare = {
                    // Same surface an nsite share uses: a QR, and an NDEF tag
                    // presented while the sheet is up so the phones can just be
                    // tapped together. The payload carries the naddr, so the
                    // author's relay hints travel with it.
                    shareFor = ShareTarget(
                        uri = NsiteShare.buildNappletShareUri(
                            nappletPointer = item.nappletPointer,
                            deviceNpub = state.ownNpub,
                            deviceName = NsiteShare.deviceName(state.ownNpub),
                            pairSecret = NsiteShare.newPairSecret(),
                        ),
                        title = item.title.ifEmpty { item.dTag ?: "napplet" },
                    )
                    nappletSheetFor = null
                },
                onReload = {
                    nappletSheetFor = null
                    // Same path a fresh add takes: fetch, verify, then the
                    // review screen — so a reload can also correct what the app
                    // is allowed to do, and never widens it silently.
                    client.dispatch(NativeActions.fetchNapplet(item.nappletPointer))
                },
                onRemove = {
                    nappletSheetFor = null
                    confirmForgetNapplet = item
                },
            )
        }
    }

    confirmForgetNapplet?.let { item ->
        AlertDialog(
            onDismissRequest = { confirmForgetNapplet = null },
            confirmButton = {
                TextButton(onClick = {
                    client.dispatch(NativeActions.forgetNapplet(item.nappletPointer))
                    confirmForgetNapplet = null
                }) { Text("Remove", color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = {
                TextButton(onClick = { confirmForgetNapplet = null }) { Text("Cancel") }
            },
            title = { Text("Remove napplet?") },
            text = {
                Text(
                    "“${item.title.ifEmpty { item.dTag ?: item.authorNpub.take(12) }}” will be " +
                        "removed, along with everything you granted it. Adding it again will " +
                        "ask you afresh."
                )
            },
        )
    }

    confirmRemove?.let { site ->
        AlertDialog(
            onDismissRequest = { confirmRemove = null },
            confirmButton = {
                TextButton(onClick = {
                    client.dispatch(NativeActions.forgetNsite(site.host))
                    confirmRemove = null
                }) { Text("Remove", color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { confirmRemove = null }) { Text("Cancel") } },
            title = { Text("Remove app?") },
            text = {
                Text("“${site.title.ifEmpty { site.host.take(12) }}” will be removed from your apps. You can add it again later.")
            },
        )
    }
}

@Composable
private fun SearchField(query: String, onChange: (String) -> Unit) {
    OutlinedTextField(
        value = query,
        onValueChange = onChange,
        singleLine = true,
        leadingIcon = { Icon(Icons.Filled.Search, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant) },
        placeholder = { Text("Search apps") },
        shape = RoundedCornerShape(16.dp),
        modifier = Modifier.fillMaxWidth(),
    )
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun NsiteTile(
    client: AppCoreClient,
    site: SiteStatus,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
) {
    var icon by remember(site.host) { mutableStateOf<Bitmap?>(null) }
    // Re-check for the icon as blobs arrive (the sync fetches it first), until found
    // — so the real app icon appears as soon as possible, then the rest downloads.
    LaunchedEffect(site.host, site.filesPulled) {
        if (icon == null) {
            icon = withContext(Dispatchers.IO) {
                runCatching { NsiteIcons.fetch(client, "${site.host}.localhost") }.getOrNull()
            }
        }
    }
    val ready = site.state == "ready"
    val syncing = site.state == "syncing"
    val stalled = site.state == "unreachable" || site.state == "incomplete"
    val total = site.filesTotal.toInt()
    val pulled = site.filesPulled.toInt()
    // Smoothly interpolate the ring between the 1 s status polls; dim the icon
    // while it isn't ready (iOS app-install style).
    val fraction by animateFloatAsState(
        if (total > 0) (pulled.toFloat() / total).coerceIn(0f, 1f) else 0f,
        animationSpec = tween(700),
        label = "dl",
    )
    val iconAlpha by animateFloatAsState(if (ready) 1f else 0.4f, tween(450), label = "alpha")

    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        modifier = modifier.combinedClickable(onClick = onClick, onLongClick = onLongClick),
    ) {
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(1f)
                .clip(RoundedCornerShape(18.dp))
                .background(if (icon == null) tileColorFor(site.host) else MaterialTheme.colorScheme.surfaceVariant),
            contentAlignment = Alignment.Center,
        ) {
            val bmp = icon
            if (bmp != null) {
                Image(bmp.asImageBitmap(), contentDescription = null, modifier = Modifier.fillMaxSize().alpha(iconAlpha))
            } else {
                Text(
                    initialOf(site),
                    color = Color.White,
                    fontWeight = FontWeight.Bold,
                    style = MaterialTheme.typography.titleLarge,
                    modifier = Modifier.alpha(iconAlpha),
                )
            }
            if (syncing) {
                // Scrim for ring contrast on bright favicons, then the progress ring.
                Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.22f)))
                if (total > 0) {
                    CircularProgressIndicator(
                        progress = { fraction },
                        modifier = Modifier.fillMaxSize().padding(12.dp),
                        strokeWidth = 4.dp,
                        color = Color.White,
                        trackColor = Color.White.copy(alpha = 0.30f),
                    )
                } else {
                    CircularProgressIndicator(
                        modifier = Modifier.size(26.dp),
                        strokeWidth = 3.dp,
                        color = Color.White,
                    )
                }
            } else if (stalled) {
                Box(
                    modifier = Modifier
                        .align(Alignment.TopEnd)
                        .padding(6.dp)
                        .size(11.dp)
                        .background(MaterialTheme.colorScheme.error, RoundedCornerShape(50)),
                )
            }
            // An update is downloading in the background (the app keeps working
            // on its current version meanwhile).
            if (site.updateTotal > 0L && site.updatePulled < site.updateTotal) {
                Box(
                    modifier = Modifier
                        .align(Alignment.TopStart)
                        .padding(6.dp)
                        .size(11.dp)
                        .background(MaterialTheme.colorScheme.primary, RoundedCornerShape(50)),
                )
            }
        }
        Spacer(Modifier.height(6.dp))
        Text(
            if (syncing && total > 0) "$pulled/$total" else site.title.ifEmpty { site.host.take(8) },
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            style = MaterialTheme.typography.labelMedium,
            color = if (syncing) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface,
            textAlign = TextAlign.Center,
        )
    }
}

/**
 * One tile in the Apps grid. nsites and napplets sit side by side: they arrive
 * the same way and open the same way, and what differs — the trust model — is
 * what the badge says.
 */
private sealed interface AppEntry {
    val title: String

    /** What the search box matches besides the title. */
    val searchKey: String

    /** Stable across recomposition, and distinct between the two kinds. */
    val key: String

    data class Nsite(val site: SiteStatus) : AppEntry {
        override val title get() = site.title
        override val searchKey get() = site.host
        override val key get() = "nsite:${site.host}"
    }

    data class Napplet(val item: LibraryItem) : AppEntry {
        override val title get() = item.title
        override val searchKey get() = item.nappletPointer
        override val key get() = "napplet:${item.nappletPointer}"
    }
}

/**
 * A napplet's tile.
 *
 * No progress ring: a napplet is a single file that was fetched and verified
 * before it ever reached the Library, so there is no partial state to show.
 */
/**
 * The long-press sheet for a napplet — the same pull-up an nsite gets, because
 * from the grid they are both just apps.
 *
 * What differs is what is on it. A napplet has no files to sync and no update
 * check, and it does have something an nsite never has: capabilities someone
 * agreed to, which they should be able to see and take back. That is what makes
 * the grant reachable rather than a decision made once and buried.
 */
@Composable
private fun NappletSheet(
    item: LibraryItem,
    onOpen: () -> Unit,
    onShare: () -> Unit,
    onReload: () -> Unit,
    onRemove: () -> Unit,
) {
    Column(modifier = Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 28.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(
                modifier = Modifier
                    .size(44.dp)
                    .clip(RoundedCornerShape(12.dp))
                    .background(tileColorFor(item.nappletPointer)),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    item.title.take(1).uppercase().ifEmpty { "N" },
                    color = Color.White,
                    fontWeight = FontWeight.Bold,
                )
            }
            Spacer(Modifier.size(12.dp))
            Column {
                Text(
                    item.title.ifEmpty { item.dTag ?: item.authorNpub.take(12) },
                    style = MaterialTheme.typography.titleMedium,
                    fontWeight = FontWeight.Bold,
                )
                Text(
                    "napplet",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
        Spacer(Modifier.height(16.dp))
        SheetAction(Icons.Filled.HomeMax, "Open") { onOpen() }
        SheetAction(Icons.Filled.Share, "Share") { onShare() }

        // Fetches the app again and shows the same screen it was added with.
        // The way to pick up a newer version, and the way to revisit what it is
        // allowed to do without removing it and finding its link again.
        SheetAction(Icons.Filled.Refresh, "Reload app") { onReload() }

        // What this app was allowed to do, in the same words it was asked in.
        Spacer(Modifier.height(12.dp))
        Text(
            if (item.granted.isEmpty()) "This app can't do anything on its own." else "This app can:",
            style = MaterialTheme.typography.titleSmall,
        )
        Spacer(Modifier.height(8.dp))
        item.granted.forEach { domain ->
            Text(
                "•  " + capabilityWording(domain),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(4.dp))
        }
        Spacer(Modifier.height(12.dp))

        SheetAction(Icons.Filled.Delete, "Remove app", tint = MaterialTheme.colorScheme.error) {
            onRemove()
        }
        SheetAction(Icons.Filled.Info, item.dTag ?: item.authorNpub.take(16)) { }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun NappletTile(
    item: LibraryItem,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
) {
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        modifier = modifier.combinedClickable(onClick = onClick, onLongClick = onLongClick),
    ) {
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(1f)
                .clip(RoundedCornerShape(18.dp))
                .background(tileColorFor(item.nappletPointer)),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                item.title.take(1).uppercase().ifEmpty { "N" },
                color = Color.White,
                fontWeight = FontWeight.Bold,
                style = MaterialTheme.typography.titleLarge,
            )
            // The duck marks a napplet: a program Myco hosts, as against an
            // nsite, which is a document Myco serves.
            Text(
                "\uD83E\uDD86",
                style = MaterialTheme.typography.labelMedium,
                modifier = Modifier.align(Alignment.TopEnd).padding(5.dp),
            )
        }
        Spacer(Modifier.height(6.dp))
        Text(
            item.title.ifEmpty { item.dTag ?: item.authorNpub.take(8) },
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurface,
            textAlign = TextAlign.Center,
        )
    }
}

/**
 * The install-review screen: what a napplet is asking for, before it has it.
 *
 * This is the only place a grant is written. Fetching a napplet stores its
 * bytes and grants nothing, so a napplet that is never reviewed can do nothing
 * but complete the handshake.
 *
 * The wording matters more than usual. A granted `relay` covers publishing with
 * no per-event prompt, which means the napplet can publish as you at will — so
 * the screen says that in words, rather than showing a domain name and hoping.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun NappletReviewSheet(
    review: NappletReview,
    onInstall: (List<String>) -> Unit,
    onDismiss: () -> Unit,
) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.padding(horizontal = 24.dp).padding(bottom = 32.dp)) {
            if (review.loading) {
                Column(
                    horizontalAlignment = Alignment.CenterHorizontally,
                    modifier = Modifier.fillMaxWidth().padding(vertical = 40.dp),
                ) {
                    CircularProgressIndicator(
                        modifier = Modifier.size(84.dp),
                        strokeWidth = 6.dp,
                    )
                    Spacer(Modifier.height(28.dp))
                    Text("Looking for this app", style = MaterialTheme.typography.titleMedium)
                    Spacer(Modifier.height(6.dp))
                    Text(
                        "This can take a few seconds.",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                return@Column
            }

            if (review.error.isNotEmpty()) {
                Column(
                    horizontalAlignment = Alignment.CenterHorizontally,
                    modifier = Modifier.fillMaxWidth().padding(top = 24.dp),
                ) {
                    Text("Couldn't find this app", style = MaterialTheme.typography.titleMedium)
                    Spacer(Modifier.height(8.dp))
                    Text(
                        // Plain words. The reason underneath is for a log, not
                        // for someone holding a phone.
                        "It might not be shared any more, or the link might be wrong.",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        textAlign = TextAlign.Center,
                    )
                    Spacer(Modifier.height(24.dp))
                    TextButton(onClick = onDismiss) { Text("Close") }
                }
                return@Column
            }

            // The app's mark, where the spinner was — so finding it resolves
            // into the thing itself rather than swapping one block of text for
            // another.
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                modifier = Modifier.fillMaxWidth(),
            ) {
                Box(
                    modifier = Modifier
                        .size(84.dp)
                        .clip(RoundedCornerShape(22.dp))
                        .background(tileColorFor(review.pointer)),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        review.title.take(1).uppercase().ifEmpty { "N" },
                        color = Color.White,
                        fontWeight = FontWeight.Bold,
                        style = MaterialTheme.typography.headlineMedium,
                    )
                }
                Spacer(Modifier.height(16.dp))
                Text(
                    review.title.ifEmpty { "Untitled app" },
                    style = MaterialTheme.typography.titleMedium,
                    textAlign = TextAlign.Center,
                )
                if (review.description.isNotEmpty()) {
                    Spacer(Modifier.height(6.dp))
                    Text(
                        review.description,
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        textAlign = TextAlign.Center,
                    )
                }
            }

            Spacer(Modifier.height(28.dp))

            // What is listed is what is granted — including the defaults every
            // app gets. A default that was not shown would be a grant nobody
            // made, and one of them lets an app post as you.
            if (review.grants.isEmpty()) {
                Text(
                    "This app runs on its own. It can't reach the internet, " +
                        "save anything, or use your account.",
                    style = MaterialTheme.typography.bodyMedium,
                )
            } else {
                Text("This app will be able to:", style = MaterialTheme.typography.titleSmall)
                Spacer(Modifier.height(10.dp))
                review.grants.forEach { domain ->
                    Text(
                        "•  " + capabilityWording(domain),
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    Spacer(Modifier.height(6.dp))
                }
                Spacer(Modifier.height(8.dp))
                Text(
                    "You can change your mind later — press and hold the app.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }

            Spacer(Modifier.height(28.dp))
            Row {
                TextButton(onClick = onDismiss) { Text("Not now") }
                Spacer(Modifier.weight(1f))
                Button(onClick = { onInstall(review.grants) }) { Text("Add to my apps") }
            }
        }
    }
}

/**
 * A NAP domain in words a person can act on.
 *
 * Unknown domains are shown verbatim rather than hidden: a napplet asking for
 * something this build has never heard of is exactly what the user should see,
 * and dropping it from the list would understate what is being agreed to.
 */
private fun capabilityWording(domain: String): String = when (domain) {
    "relay" -> "Read and post as you, whenever it likes, without asking again"
    "identity" -> "See your name and profile"
    "storage" -> "Save things on this phone"
    "intent" -> "Open your other apps"
    "inc" -> "Talk to your other open apps"
    "outbox" -> "Choose where to send things on your behalf"
    "notify" -> "Send you notifications"
    "theme" -> "Match your colours"
    "link" -> "Open links outside Myco"
    "resource" -> "Load pictures and files"
    "config" -> "Have settings you can change"
    "shell" -> "Start up (every app does this)"
    else -> "Do something this version of Myco doesn't know about (\"$domain\")"
}

@Composable
private fun AddTile(onClick: () -> Unit) {
    Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.combinedClickableSafe(onClick)) {
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(1f)
                .clip(RoundedCornerShape(18.dp))
                .background(MaterialTheme.colorScheme.surfaceVariant),
            contentAlignment = Alignment.Center,
        ) {
            Icon(Icons.Filled.Add, contentDescription = "Add app", tint = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.size(28.dp))
        }
        Spacer(Modifier.height(6.dp))
        Text("Add", color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.labelMedium)
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AppSheet(
    client: AppCoreClient,
    site: SiteStatus,
    onOpen: () -> Unit,
    onShare: () -> Unit,
    onPinToHome: () -> Unit,
    onRemove: () -> Unit,
) {
    Column(modifier = Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 28.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(
                modifier = Modifier
                    .size(44.dp)
                    .clip(RoundedCornerShape(12.dp))
                    .background(tileColorFor(site.host)),
                contentAlignment = Alignment.Center,
            ) {
                Text(initialOf(site), color = androidx.compose.ui.graphics.Color.White, fontWeight = FontWeight.Bold)
            }
            Spacer(Modifier.size(12.dp))
            Column {
                Text(site.title.ifEmpty { site.host.take(12) }, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
                val updating = site.updateTotal > 0L && !site.updateAvailable
                Text(
                    if (updating) "Updating… ${site.updatePulled}/${site.updateTotal}"
                    else "nsite · ${site.filesTotal} files · ${site.state}",
                    color = if (updating) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
        Spacer(Modifier.height(16.dp))
        SheetAction(Icons.Filled.HomeMax, "Open") { onOpen() }
        SheetAction(Icons.Filled.Share, "Share") { onShare() }
        if (site.state == "ready") {
            SheetAction(Icons.Filled.Add, "Add to Home screen") { onPinToHome() }
        }
        val ctx = androidx.compose.ui.platform.LocalContext.current
        SheetAction(Icons.Filled.Refresh, "Check for updates") {
            client.dispatch(NativeActions.checkNsiteUpdates())
            android.widget.Toast.makeText(ctx, "Checking for updates…", android.widget.Toast.LENGTH_SHORT).show()
        }
        SheetAction(Icons.Filled.Delete, "Remove app", tint = MaterialTheme.colorScheme.error) { onRemove() }
        SheetAction(Icons.Filled.Info, site.host) { }
    }
}

/** What a [ShareQrSheet] presents: the prebuilt `myco://share/…` URI (stable, so
 *  its QR and the NFC payload carry the same one-time secret) plus a label. */
private class ShareTarget(val uri: String, val title: String)

/**
 * The **Share an app** surface — a bottom sheet styled like the pair "My code"
 * panel: the share QR plus a prominent tap-phones-together prompt. While it's
 * visible the device presents the share URI over NFC ([PairPresent.beginRaw]),
 * so a phone tap shares the app exactly like a scan would.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ShareQrSheet(target: ShareTarget, pendingRequestCount: Int, onDismiss: () -> Unit) {
    // Emulate an NDEF tag carrying the share URI for as long as the sheet is up.
    // The reader phone's OS reads it → MainActivity.openSharedNsite (pair + pull).
    DisposableEffect(target.uri) {
        PairPresent.beginRaw(target.uri)
        onDispose { PairPresent.stop() }
    }
    // The recipient tapping/scanning sends a pair request back to us. Once one
    // lands, the share has gone through — drop the sheet and let the normal
    // accept prompt take over (it's suppressed while we're presenting).
    val baselineRequests = remember { pendingRequestCount }
    LaunchedEffect(pendingRequestCount) {
        if (pendingRequestCount > baselineRequests) onDismiss()
    }
    val qr = remember(target.uri) { NsiteShare.qrBitmap(target.uri) }
    // Open fully expanded (not the half-height detent) so the whole thing — QR plus
    // the tap-to-share prompt — is on screen without a drag.
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 20.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text("Share this app", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
            Spacer(Modifier.height(12.dp))
            Surface(
                shape = RoundedCornerShape(20.dp),
                color = MaterialTheme.colorScheme.surfaceVariant,
                border = androidx.compose.foundation.BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
                modifier = Modifier.fillMaxWidth(),
            ) {
                Column(
                    modifier = Modifier.fillMaxWidth().padding(16.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    QrCodeCard(qr, contentDescription = "Share code for ${target.title}", size = 210.dp)
                    Spacer(Modifier.height(10.dp))
                    Text(target.title, fontWeight = FontWeight.ExtraBold, style = MaterialTheme.typography.titleMedium)
                    Spacer(Modifier.height(2.dp))
                    Text(
                        "Scan to open this app — and pair with this device",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.bodySmall,
                        textAlign = TextAlign.Center,
                    )
                }
            }
            Spacer(Modifier.height(12.dp))
            // The headline action: just bump phones (no scanning needed). The
            // breathing NFC bubble mirrors the Circle tab's tap-to-connect hint.
            Surface(shape = RoundedCornerShape(16.dp), color = MaterialTheme.colorScheme.primaryContainer, modifier = Modifier.fillMaxWidth()) {
                Row(
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(14.dp),
                ) {
                    NfcPulseBubble(size = 44.dp)
                    Column {
                        Text(
                            "Tap phones together to share",
                            fontWeight = FontWeight.ExtraBold,
                            color = MaterialTheme.colorScheme.onPrimaryContainer,
                            style = MaterialTheme.typography.titleSmall,
                        )
                        Text(
                            "Hold the backs together — no scan needed",
                            color = MaterialTheme.colorScheme.onPrimaryContainer,
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            }
        }
    }
}

/**
 * **Add an app** — a bottom sheet mirroring the share sheet, but with a live
 * camera scanner in place of the QR (you're the one reading a friend's code).
 * A scan, a pasted link, or an NFC tap (handled passively by the OS) all route
 * through [onScanned] → MainActivity.handleScannedText.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AddAppSheet(siteCount: Int, onScanned: (String) -> Unit, onDismiss: () -> Unit) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    // Claim foreground NFC reader mode while we're up — a modal sheet's own window
    // misses the OS's passive tap dispatch, so the Activity reads the tag directly.
    DisposableEffect(Unit) {
        NfcReader.begin()
        onDispose { NfcReader.stop() }
    }
    // A camera scan routes through onScanned (which dismisses us); an NFC tap is read
    // into MainActivity — either way close the sheet when a new app shows up, to
    // reveal it downloading in the grid behind us.
    val baselineSites = remember { siteCount }
    LaunchedEffect(siteCount) {
        if (siteCount > baselineSites) onDismiss()
    }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 20.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text("Add an app", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
            Spacer(Modifier.height(12.dp))
            Box(modifier = Modifier.fillMaxWidth().height(300.dp)) {
                ScanPanel(onScanned = onScanned)
            }
            Spacer(Modifier.height(12.dp))
            // Same breathing NFC bubble as the share sheet — here you're the reader.
            Surface(shape = RoundedCornerShape(16.dp), color = MaterialTheme.colorScheme.primaryContainer, modifier = Modifier.fillMaxWidth()) {
                Row(
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(14.dp),
                ) {
                    NfcPulseBubble(size = 44.dp)
                    Column {
                        Text(
                            "Or tap a friend's phone",
                            fontWeight = FontWeight.ExtraBold,
                            color = MaterialTheme.colorScheme.onPrimaryContainer,
                            style = MaterialTheme.typography.titleSmall,
                        )
                        Text(
                            "Hold the backs together to add their app",
                            color = MaterialTheme.colorScheme.onPrimaryContainer,
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            }
            Spacer(Modifier.height(12.dp))
            PasteCodeButton(label = "Paste a link", onPaste = onScanned)
        }
    }
}

private fun initialOf(site: SiteStatus): String =
    site.title.ifEmpty { site.host }.firstOrNull { it.isLetterOrDigit() }?.uppercase() ?: "?"
