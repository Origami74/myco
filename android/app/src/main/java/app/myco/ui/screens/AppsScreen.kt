package app.myco.ui.screens

import android.graphics.Bitmap
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.HomeMax
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.outlined.Language
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SheetValue
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.myco.NsiteIcons
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.LibraryItem
import app.myco.core.LibraryKind
import app.myco.core.NappletAddress
import app.myco.core.NappletReview
import app.myco.core.NativeActions
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
    onPinNappletToHome: (pointer: String, title: String) -> Unit,
    onPinToHome: (host: String, title: String) -> Unit,
    onScanned: (String) -> Unit,
) {
    var query by remember { mutableStateOf("") }
    var sheetFor by remember { mutableStateOf<SiteStatus?>(null) }
    var shareFor by remember { mutableStateOf<ShareTarget?>(null) }
    var confirmRemove by remember { mutableStateOf<SiteStatus?>(null) }
    var showAdd by remember { mutableStateOf(false) }
    var nappletSheetFor by remember { mutableStateOf<LibraryItem?>(null) }
    var permissionsFor by remember { mutableStateOf<LibraryItem?>(null) }
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

    // Launcher-style: icons have a set size per screen class — stock phone
    // size, a step up on tablets — and the column count follows the width:
    // five across a phone, more on a tablet, instead of four tiles stretched
    // to fill whatever screen they are on. The page spans the full width like
    // every other tab.
    BoxWithConstraints(Modifier.fillMaxSize()) {
    val iconSize = appIconSizeFor(maxWidth)
    val columns = ((maxWidth - APPS_SIDE_PADDING * 2) / (iconSize + APP_CELL_GUTTER)).toInt().coerceIn(4, 8)
    CompositionLocalProvider(LocalAppIconSize provides iconSize) {
    LazyVerticalGrid(
        columns = GridCells.Fixed(columns),
        modifier = Modifier.fillMaxSize(),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(
            horizontal = APPS_SIDE_PADDING,
            vertical = 20.dp,
        ),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
        verticalArrangement = Arrangement.spacedBy(20.dp),
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
                    // Unknown counts as ready: the status is computed a moment
                    // after startup, and a tile that dims for that moment reads
                    // as a broken app.
                    status = state.nappletStatus[entry.item.urlHost],
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
            // The same fetch again, sharer first — a tap in a room with no
            // internet fails when the sharer's link is still coming up, and
            // that is the case a retry is for.
            onRetry = { client.dispatch(NativeActions.fetchNapplet(review.pointer, review.holder)) },
            onOpen = {
                val (pointer, title) = nappletLaunchTarget(review, state.library)
                client.dispatch(NativeActions.dismissNappletReview())
                onLaunchNapplet(pointer, title)
            },
            onDismiss = { client.dispatch(NativeActions.dismissNappletReview()) },
        )
    }

    nappletSheetFor?.let { picked ->
        // Read the live entry, not the snapshot the long-press captured: a
        // switch on this sheet changes the grants, and the sheet shows them.
        val item = state.library.firstOrNull { it.nappletPointer == picked.nappletPointer } ?: picked
        ModalBottomSheet(onDismissRequest = { nappletSheetFor = null }) {
            NappletSheet(
                item = item,
                onManagePermissions = {
                    nappletSheetFor = null
                    permissionsFor = item
                },
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
                onPinToHome = {
                    nappletSheetFor = null
                    onPinNappletToHome(item.nappletPointer, item.title)
                },
                onCheckUpdates = {
                    nappletSheetFor = null
                    client.dispatch(NativeActions.checkNsiteUpdates())
                    android.widget.Toast.makeText(context, "Checking for updates…", android.widget.Toast.LENGTH_SHORT).show()
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

    permissionsFor?.let { picked ->
        // Live entry, not the snapshot: the switches change what this shows.
        val item = state.library.firstOrNull { it.nappletPointer == picked.nappletPointer } ?: picked
        ModalBottomSheet(onDismissRequest = { permissionsFor = null }) {
            PermissionsSheet(
                item = item,
                domains = state.nappletDomains,
                onGrant = { domain, allowed ->
                    client.dispatch(NativeActions.setNappletGrant(item.nappletPointer, domain, allowed))
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
                    // All three, or it comes back: an open window reloads its
                    // loading page and a pending deep link is re-asked on the
                    // next resume.
                    app.myco.NsiteActivity.closeTask(context, site.host)
                    app.myco.share.PendingDeepLinks.remove(context, site.host)
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
        val bmp = icon
        LauncherIcon(
            // A favicon sits on a white disc, as a legacy icon does in the
            // stock launcher (not a theme surface: AMOLED's is pure black);
            // without one, the letter on its own colour.
            background = if (bmp == null) tileColorFor(site.host) else Color.White,
            outlined = bmp != null,
            content = {
                if (bmp != null) {
                    Image(
                        bmp.asImageBitmap(),
                        contentDescription = null,
                        contentScale = ContentScale.Fit,
                        modifier = Modifier.fillMaxSize().padding(9.dp).alpha(iconAlpha),
                    )
                } else {
                    Text(
                        initialOf(site),
                        color = Color.White,
                        fontWeight = FontWeight.SemiBold,
                        fontSize = 20.sp,
                        modifier = Modifier.alpha(iconAlpha),
                    )
                }
                if (syncing) {
                    // Scrim for ring contrast on bright favicons.
                    Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.22f)))
                }
            },
            overlay = {
                if (syncing) {
                    if (total > 0) {
                        CircularProgressIndicator(
                            progress = { fraction },
                            modifier = Modifier.fillMaxSize().padding(3.dp),
                            strokeWidth = 3.dp,
                            color = Color.White,
                            trackColor = Color.White.copy(alpha = 0.30f),
                        )
                    } else {
                        CircularProgressIndicator(
                            modifier = Modifier.size(22.dp),
                            strokeWidth = 2.5.dp,
                            color = Color.White,
                        )
                    }
                } else if (stalled) {
                    StatusDot(MaterialTheme.colorScheme.error, Modifier.align(Alignment.TopEnd))
                }
                // An update is downloading in the background (the app keeps
                // working on its current version meanwhile).
                if (site.updateTotal > 0L && site.updatePulled < site.updateTotal) {
                    StatusDot(MaterialTheme.colorScheme.primary, Modifier.align(Alignment.TopStart))
                }
                // Napplets are the default kind of app; an nsite — a website
                // Myco serves, not a program it hosts — carries the mark.
                WebBadge(modifier = Modifier.align(Alignment.BottomEnd).offset(x = 2.dp, y = 2.dp))
            },
        )
        Spacer(Modifier.height(6.dp))
        LauncherLabel(
            if (syncing && total > 0) "$pulled/$total" else site.title.ifEmpty { site.host.take(8) },
            color = if (syncing) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface,
        )
    }
}

/** A small status dot on an icon's rim, ringed in the page colour. */
@Composable
private fun StatusDot(color: Color, modifier: Modifier = Modifier) {
    Box(
        modifier = modifier
            .size(12.dp)
            .clip(CircleShape)
            .background(MaterialTheme.colorScheme.surface)
            .padding(2.dp)
            .clip(CircleShape)
            .background(color),
    )
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
    onManagePermissions: () -> Unit,
    onOpen: () -> Unit,
    onShare: () -> Unit,
    onPinToHome: () -> Unit,
    onCheckUpdates: () -> Unit,
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
        // What this app may do, on its own page: the wording is long and the
        // switches want room.
        SheetAction(Icons.Filled.Lock, "Manage permissions") { onManagePermissions() }
        SheetAction(Icons.Filled.Add, "Add to Home screen") { onPinToHome() }

        // The same check the nsite sheet offers: every installed app, napplets
        // included, asked for a newer version; the toast says what came of it.
        SheetAction(Icons.Filled.Refresh, "Check for updates") { onCheckUpdates() }
        // Fetches the app again and shows the same screen it was added with.
        // The way to revisit what it is allowed to do without removing it and
        // finding its link again — or to force a re-fetch when a check found
        // nothing but the app still misbehaves.
        SheetAction(Icons.Filled.Refresh, "Reload app") { onReload() }
        Spacer(Modifier.height(8.dp))

        SheetAction(Icons.Filled.Delete, "Remove app", tint = MaterialTheme.colorScheme.error) {
            onRemove()
        }
        SheetAction(Icons.Filled.Info, item.dTag ?: item.authorNpub.take(16)) { }
    }
}

/**
 * What a napplet may do, in the same words the install sheet used, with a
 * switch for each. A napplet's own declaration is a statement of intent its
 * toolchain may have dropped; the user is the one who gets to say. A change
 * is live — an open window of the app is restarted so its startup calls are
 * made again under the new grants.
 */
@Composable
private fun PermissionsSheet(
    item: LibraryItem,
    domains: List<String>,
    onGrant: (domain: String, allowed: Boolean) -> Unit,
) {
    Column(modifier = Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 28.dp)) {
        Text(
            item.title.ifEmpty { item.dTag ?: "This app" },
            style = MaterialTheme.typography.titleMedium,
            fontWeight = FontWeight.Bold,
        )
        Text(
            "What it's allowed to do. Changing one restarts the app if it's open.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(Modifier.height(12.dp))
        val listed = (domains + item.granted.filter { it !in domains }).distinct()
        listed.forEach { domain ->
            val allowed = domain in item.granted
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.fillMaxWidth().padding(vertical = 6.dp),
            ) {
                CapabilityRow(domain, dimmed = !allowed, modifier = Modifier.weight(1f))
                Spacer(Modifier.size(12.dp))
                Switch(checked = allowed, onCheckedChange = { onGrant(domain, it) })
            }
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun NappletTile(
    item: LibraryItem,
    status: app.myco.core.NappletStatus?,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
) {
    val ready = status?.ready ?: true
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        modifier = modifier.combinedClickable(onClick = onClick, onLongClick = onLongClick),
    ) {
        LauncherIcon(
            background = tileColorFor(item.nappletPointer),
            // Dimmed like an nsite that is not downloaded: the app is in the
            // Library but not on the phone — after a cache wipe, say.
            alpha = if (ready) 1f else 0.35f,
            content = {
                Text(
                    item.title.take(1).uppercase().ifEmpty { "N" },
                    color = Color.White,
                    fontWeight = FontWeight.SemiBold,
                    fontSize = 20.sp,
                )
            },
        )
        Spacer(Modifier.height(6.dp))
        LauncherLabel(item.title.ifEmpty { item.dTag ?: item.authorNpub.take(8) })
        if (!ready) {
            Text(
                status?.message.orEmpty().ifEmpty { "Not on this phone" },
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
        }
    }
}

/**
 * Launcher icon diameter by screen width: the stock Pixel launcher's 52 dp on
 * a phone, a step up on small and large tablets so icons don't look lost.
 */
private fun appIconSizeFor(width: androidx.compose.ui.unit.Dp): androidx.compose.ui.unit.Dp = when {
    width >= 840.dp -> 68.dp
    width >= 600.dp -> 60.dp
    else -> 52.dp
}

/** A cell is the icon plus this much room — what makes a phone five columns wide. */
private val APP_CELL_GUTTER = 12.dp

private val APPS_SIDE_PADDING = 16.dp

/** The icon size of the grid being drawn (set per screen width). */
private val LocalAppIconSize = androidx.compose.runtime.staticCompositionLocalOf { 52.dp }

/**
 * A round launcher icon of the one fixed size. The content fills the circle;
 * `overlay` draws on top of it (progress, badges).
 */
@Composable
private fun LauncherIcon(
    background: Color,
    modifier: Modifier = Modifier,
    alpha: Float = 1f,
    outlined: Boolean = false,
    content: @Composable androidx.compose.foundation.layout.BoxScope.() -> Unit,
    overlay: @Composable androidx.compose.foundation.layout.BoxScope.() -> Unit = {},
) {
    Box(modifier.size(LocalAppIconSize.current), contentAlignment = Alignment.Center) {
        Box(
            modifier = Modifier
                .fillMaxSize()
                .clip(CircleShape)
                .alpha(alpha)
                .background(background)
                // A white disc on a white page needs an edge to be a disc.
                .then(if (outlined) Modifier.border(1.dp, MaterialTheme.colorScheme.outlineVariant, CircleShape) else Modifier),
            contentAlignment = Alignment.Center,
            content = content,
        )
        overlay()
    }
}

/** A one-line launcher label, stock size. */
@Composable
private fun LauncherLabel(text: String, color: Color = MaterialTheme.colorScheme.onSurface) {
    Text(
        text,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
        fontSize = 12.sp,
        lineHeight = 16.sp,
        color = color,
        textAlign = TextAlign.Center,
        modifier = Modifier.fillMaxWidth().padding(horizontal = 2.dp),
    )
}

/**
 * The website mark: a small globe on a round chip at the icon's lower edge,
 * telling an nsite (a site Myco serves) from a napplet (the default kind of
 * app) at a glance without competing with the icon. The chip takes the page
 * colour so it reads as a cut-out on any tile.
 */
@Composable
internal fun WebBadge(modifier: Modifier = Modifier) {
    Box(
        modifier = modifier
            .size(18.dp)
            .clip(CircleShape)
            .background(MaterialTheme.colorScheme.surface),
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            Icons.Outlined.Language,
            contentDescription = "Website",
            tint = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.size(12.dp),
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
internal fun NappletReviewSheet(
    review: NappletReview,
    onInstall: (List<String>) -> Unit,
    onRetry: () -> Unit,
    onOpen: () -> Unit,
    onDismiss: () -> Unit,
) {
    // Fully expanded, never half: a half-open sheet cut the permission list
    // off with the install button below the fold.
    // Not dismissable while "Adding…": the download is under way, and a
    // failure has to be able to come back to this sheet to be seen.
    val installing by rememberUpdatedState(review.installing)
    val answer = reviewAnswer(review)
    ModalBottomSheet(
        onDismissRequest = { if (!installing) onDismiss() },
        sheetState = rememberModalBottomSheetState(
            skipPartiallyExpanded = true,
            confirmValueChange = { it != SheetValue.Hidden || !installing },
        ),
    ) {
        Column(Modifier.padding(horizontal = 24.dp).padding(bottom = 24.dp)) {
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
                    Button(onClick = onRetry) { Text("Try again") }
                    Spacer(Modifier.height(4.dp))
                    TextButton(onClick = onDismiss) { Text("Close") }
                }
                return@Column
            }

            // The app landed. The sheet stays up to say so and to offer the
            // obvious next step — a sheet that just vanished read as nothing
            // having happened.
            if (review.added) {
                val name = review.title.ifEmpty { "This app" }
                val headline = when {
                    !review.installed -> "$name was added to your apps"
                    review.unreviewed.isEmpty() -> "$name was downloaded again"
                    else -> "$name was updated"
                }
                Column(
                    horizontalAlignment = Alignment.CenterHorizontally,
                    modifier = Modifier.fillMaxWidth().padding(top = 16.dp),
                ) {
                    NappletMark(review, added = true)
                    Spacer(Modifier.height(20.dp))
                    Text(
                        headline,
                        style = MaterialTheme.typography.titleMedium,
                        textAlign = TextAlign.Center,
                    )
                    Spacer(Modifier.height(6.dp))
                    Text(
                        "It's on this phone and ready to use.",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        textAlign = TextAlign.Center,
                    )
                    Spacer(Modifier.height(24.dp))
                    Button(onClick = onOpen, modifier = Modifier.fillMaxWidth()) { Text("Open") }
                    Spacer(Modifier.height(4.dp))
                    TextButton(onClick = onDismiss) { Text("Done") }
                }
                return@Column
            }

            // Everything above the buttons scrolls; the buttons stay put, so a
            // long description or permission list never pushes the answer off
            // screen.
            Column(
                modifier = Modifier
                    .weight(1f, fill = false)
                    .verticalScroll(rememberScrollState()),
            ) {
                // The app's mark, where the spinner was — so finding it resolves
                // into the thing itself rather than swapping one block of text for
                // another.
                Column(
                    horizontalAlignment = Alignment.CenterHorizontally,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    NappletMark(review, added = false)
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

                Spacer(Modifier.height(20.dp))

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
                    Text(
                        if (answer == ReviewAnswer.Allow) {
                            "This update asks for more. With it, this app will be able to:"
                        } else {
                            "This app will be able to:"
                        },
                        style = MaterialTheme.typography.titleSmall,
                    )
                    Spacer(Modifier.height(10.dp))
                    review.grants.forEach { domain ->
                        CapabilityRow(domain)
                        Spacer(Modifier.height(10.dp))
                    }
                    Spacer(Modifier.height(8.dp))
                    Text(
                        "You can change your mind later — press and hold the app.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }

            }

            val alreadyInstalled = answer == ReviewAnswer.AlreadyInstalled ||
                answer == ReviewAnswer.DownloadAgain
            val reinstall = answer == ReviewAnswer.DownloadAgain

            Spacer(Modifier.height(20.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                TextButton(onClick = onDismiss, enabled = !review.installing) {
                    Text(if (alreadyInstalled) "Close" else "Not now")
                }
                Spacer(Modifier.weight(1f))
                if (alreadyInstalled && !reinstall) {
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        Button(enabled = false, onClick = {}) { Text("Add to my apps") }
                        Spacer(Modifier.height(4.dp))
                        Text(
                            "Already installed",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    return@Row
                }
                // Review ran on the manifest alone; the app itself is only
                // downloaded once the user says yes, and the sheet closes when
                // it has landed.
                Button(
                    enabled = !review.installing,
                    onClick = { onInstall(review.grants) },
                ) {
                    if (review.installing) {
                        CircularProgressIndicator(
                            modifier = Modifier.size(16.dp),
                            strokeWidth = 2.dp,
                        )
                        Spacer(Modifier.size(8.dp))
                        Text(if (reinstall) "Downloading…" else "Adding…")
                    } else {
                        Text(
                            when (answer) {
                                ReviewAnswer.DownloadAgain -> "Download again"
                                ReviewAnswer.Allow -> "Allow"
                                else -> "Add to my apps"
                            },
                        )
                    }
                }
            }
        }
    }
}

/** What a review sheet's main button does. See [reviewAnswer]. */
internal enum class ReviewAnswer {
    /** Not installed: add it, with what the sheet lists. */
    Add,
    /** Installed and here, with nothing new to agree to: nothing to do. */
    AlreadyInstalled,
    /** Installed, nothing new to agree to, but its files are not on this phone. */
    DownloadAgain,
    /** Installed, and this version asks for more than was reviewed: agree to it. */
    Allow,
}

/**
 * Already in the Library with nothing new to agree to: adding it again would
 * change nothing, so the sheet says so rather than offer it — or, when it is
 * not on this phone ("hold to reload"), offers its download; the grants it
 * has are kept either way. An installed app whose update declares more is
 * asked to be allowed: that is how the new permissions are agreed to, on the
 * Apps screen and over the app's own window alike.
 */
internal fun reviewAnswer(review: NappletReview): ReviewAnswer = when {
    !review.installed -> ReviewAnswer.Add
    review.unreviewed.isNotEmpty() -> ReviewAnswer.Allow
    review.ready -> ReviewAnswer.AlreadyInstalled
    else -> ReviewAnswer.DownloadAgain
}

/** The app's mark on the review sheet, with a check once it has been added. */
@Composable
private fun NappletMark(review: NappletReview, added: Boolean) {
    Box {
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
        if (added) {
            Icon(
                Icons.Filled.CheckCircle,
                contentDescription = "Added",
                tint = MaterialTheme.colorScheme.primary,
                modifier = Modifier
                    .align(Alignment.BottomEnd)
                    .offset(x = 8.dp, y = 8.dp)
                    .size(32.dp)
                    .background(MaterialTheme.colorScheme.surface, CircleShape),
            )
        }
    }
}

/**
 * Where "Open" on an added review goes: the Library entry's own pointer and
 * title when there is one, so the window lands in the same task the Apps grid
 * opens — its document is keyed on the pointer — and the review's otherwise.
 * Matched on author and `d` tag, not the pointer string: a review opened by an
 * `naddr` names the same napplet as an entry stored as `<npub>:<dtag>`.
 */
internal fun nappletLaunchTarget(review: NappletReview, library: List<LibraryItem>): Pair<String, String> {
    val address = NappletAddress.parse(review.pointer)
    val item = library.firstOrNull {
        it.kind == LibraryKind.Napplet &&
            (it.nappletPointer == review.pointer ||
                (address != null && NappletAddress.of(it.authorNpub, it.dTag) == address))
    }
    return (item?.nappletPointer ?: review.pointer) to (item?.title ?: review.title)
}

/**
 * A NAP domain in words a person can act on.
 *
 * Unknown domains are shown verbatim rather than hidden: a napplet asking for
 * something this build has never heard of is exactly what the user should see,
 * and dropping it from the list would understate what is being agreed to.
 */
/** A capability as a person reads it: a name, and what allowing it means. */
private data class Capability(val title: String, val detail: String)

private fun capabilityWording(domain: String): Capability = when (domain) {
    "relay" -> Capability("Relays", "Read and post as you on your relays, without asking each time")
    "outbox" -> Capability("Outbox", "Post as you to your relays and to other people's, and read from theirs")
    "mesh" -> Capability("Mesh", "Post as you to phones nearby, and read what they share, without asking each time")
    "local" -> Capability("Keep on this phone", "Keep notes, and things it showed you, on this phone")
    "identity" -> Capability("Identity", "See your name and profile")
    "resource" -> Capability("Pictures & files", "Load pictures and files by their content hash")
    "storage" -> Capability("Storage", "Save things on this phone")
    "intent" -> Capability("Other apps", "Open your other apps")
    "inc" -> Capability("App to app", "Talk to your other open apps")
    "notify" -> Capability("Notifications", "Send you notifications")
    "theme" -> Capability("Theme", "Match your colours")
    "link" -> Capability("Links", "Ask to open web links in your browser, and suggest other apps for you to add")
    "config" -> Capability("Settings", "Have settings you can change")
    "shell" -> Capability("Start up", "Every app does this")
    else -> Capability(domain, "Something this version of Myco doesn't know about")
}

/** One capability, as a title with its meaning underneath. */
@Composable
private fun CapabilityRow(domain: String, dimmed: Boolean = false, modifier: Modifier = Modifier) {
    val c = capabilityWording(domain)
    Column(modifier = modifier) {
        Text(
            c.title,
            style = MaterialTheme.typography.bodyLarge,
            fontWeight = FontWeight.SemiBold,
            color = if (dimmed) MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.onSurface,
        )
        Text(
            c.detail,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun AddTile(onClick: () -> Unit) {
    Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.combinedClickableSafe(onClick)) {
        LauncherIcon(
            // A faint disc from the text colour, so it shows in every theme.
            background = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.10f),
            content = {
                Icon(Icons.Filled.Add, contentDescription = "Add app", tint = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.size(24.dp))
            },
        )
        Spacer(Modifier.height(6.dp))
        LauncherLabel("Add", color = MaterialTheme.colorScheme.onSurfaceVariant)
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
