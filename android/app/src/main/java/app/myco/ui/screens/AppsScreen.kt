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
import app.myco.core.SiteStatus
import app.myco.nfc.PairPresent
import app.myco.share.NsiteShare
import app.myco.ui.ScreenHeader
import app.myco.ui.theme.EmeraldInk
import app.myco.ui.theme.EmeraldSoft
import app.myco.ui.theme.Slate
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
    onPinToHome: (host: String, title: String) -> Unit,
    onAddApp: () -> Unit,
) {
    var query by remember { mutableStateOf("") }
    var sheetFor by remember { mutableStateOf<SiteStatus?>(null) }
    var shareFor by remember { mutableStateOf<ShareTarget?>(null) }
    var confirmRemove by remember { mutableStateOf<SiteStatus?>(null) }

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

    val apps = state.sites.filter {
        query.isBlank() || it.title.contains(query, true) || it.host.contains(query, true)
    }.sortedBy { it.title.ifEmpty { it.host }.lowercase() }

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
        items(apps, key = { it.host }) { site ->
            NsiteTile(
                client = client,
                site = site,
                modifier = Modifier.animateItem(),
                // Ready → open the app; still downloading → its live status page.
                onClick = { onLaunchNsite(site.host, site.title) },
                onLongClick = { sheetFor = site },
            )
        }
        item {
            AddTile { onAddApp() }
        }
        if (apps.isEmpty() && query.isBlank()) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Text(
                    "No apps yet — scan a friend's share QR or paste a link to add one.",
                    color = Slate,
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

    shareFor?.let { target -> ShareQrSheet(target = target, onDismiss = { shareFor = null }) }

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
        leadingIcon = { Icon(Icons.Filled.Search, contentDescription = null, tint = Slate) },
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
            Icon(Icons.Filled.Add, contentDescription = "Add app", tint = Slate, modifier = Modifier.size(28.dp))
        }
        Spacer(Modifier.height(6.dp))
        Text("Add", color = Slate, style = MaterialTheme.typography.labelMedium)
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
                    color = if (updating) MaterialTheme.colorScheme.primary else Slate,
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
private fun ShareQrSheet(target: ShareTarget, onDismiss: () -> Unit) {
    // Emulate an NDEF tag carrying the share URI for as long as the sheet is up.
    // The reader phone's OS reads it → MainActivity.openSharedNsite (pair + pull).
    DisposableEffect(target.uri) {
        PairPresent.beginRaw(target.uri)
        onDispose { PairPresent.stop() }
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
            Surface(shape = RoundedCornerShape(20.dp), color = MaterialTheme.colorScheme.surfaceVariant, modifier = Modifier.fillMaxWidth()) {
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
                        color = Slate,
                        style = MaterialTheme.typography.bodySmall,
                        textAlign = TextAlign.Center,
                    )
                }
            }
            Spacer(Modifier.height(12.dp))
            // The headline action: just bump phones (no scanning needed). The
            // breathing NFC bubble mirrors the Circle tab's tap-to-connect hint.
            Surface(shape = RoundedCornerShape(16.dp), color = EmeraldSoft, modifier = Modifier.fillMaxWidth()) {
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
                            color = EmeraldInk,
                            style = MaterialTheme.typography.titleSmall,
                        )
                        Text(
                            "Hold the backs together — no scan needed",
                            color = Color(0xFF047857),
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            }
        }
    }
}

private fun initialOf(site: SiteStatus): String =
    site.title.ifEmpty { site.host }.firstOrNull { it.isLetterOrDigit() }?.uppercase() ?: "?"
