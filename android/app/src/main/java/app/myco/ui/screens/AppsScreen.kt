package app.myco.ui.screens

import android.graphics.Bitmap
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
import androidx.compose.material.icons.filled.HomeMax
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
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
import app.myco.share.NsiteShare
import app.myco.ui.ScreenHeader
import app.myco.ui.theme.Slate
import app.myco.ui.theme.tileColorFor
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * The **Apps** surface: an app-drawer of installed nsites (icon grid + search),
 * each opening as its own fullscreen task. Long-press an app for its sheet
 * (share, add-to-home, info); the "+" tile adds one by pasting a link or scanning.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AppsScreen(
    state: AppState,
    client: AppCoreClient,
    onLaunchNsite: (host: String, title: String) -> Unit,
    onPinToHome: (host: String, title: String) -> Unit,
    onScan: () -> Unit,
) {
    var query by remember { mutableStateOf("") }
    var sheetFor by remember { mutableStateOf<SiteStatus?>(null) }
    var shareUri by remember { mutableStateOf<String?>(null) }
    var showAdd by remember { mutableStateOf(false) }

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
                onClick = { onLaunchNsite(site.host, site.title) },
                onLongClick = { sheetFor = site },
            )
        }
        item {
            AddTile { showAdd = true }
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
                    shareUri = NsiteShare.buildShareUri(
                        nsiteHost = site.host,
                        deviceNpub = state.ownNpub,
                        deviceName = NsiteShare.deviceName(state.ownNpub),
                        pairSecret = NsiteShare.newPairSecret(),
                    )
                    sheetFor = null
                },
                onPinToHome = { sheetFor = null; onPinToHome(site.host, site.title) },
            )
        }
    }

    shareUri?.let { uri -> ShareQrDialog(uri) { shareUri = null } }

    if (showAdd) {
        AddAppDialog(
            onDismiss = { showAdd = false },
            onPaste = { link -> client.dispatch(NativeActions.openNsite(link)); showAdd = false },
            onScan = { showAdd = false; onScan() },
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
    onClick: () -> Unit,
    onLongClick: () -> Unit,
) {
    var icon by remember(site.host) { mutableStateOf<Bitmap?>(null) }
    LaunchedEffect(site.host) {
        icon = withContext(Dispatchers.IO) {
            runCatching { NsiteIcons.fetch(client, "${site.host}.nsite") }.getOrNull()
        }
    }
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        modifier = Modifier.combinedClickable(onClick = onClick, onLongClick = onLongClick),
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
                Image(bmp.asImageBitmap(), contentDescription = null, modifier = Modifier.fillMaxSize())
            } else {
                Text(
                    initialOf(site),
                    color = androidx.compose.ui.graphics.Color.White,
                    fontWeight = FontWeight.Bold,
                    style = MaterialTheme.typography.titleLarge,
                )
            }
            if (site.state != "ready") {
                Box(
                    modifier = Modifier
                        .size(10.dp)
                        .align(Alignment.TopEnd)
                        .padding(0.dp)
                        .background(MaterialTheme.colorScheme.error, RoundedCornerShape(50)),
                )
            }
        }
        Spacer(Modifier.height(6.dp))
        Text(
            site.title.ifEmpty { site.host.take(8) },
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            style = MaterialTheme.typography.labelMedium,
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

@OptIn(ExperimentalFoundationApi::class)
private fun Modifier.combinedClickableSafe(onClick: () -> Unit): Modifier =
    this.combinedClickable(onClick = onClick)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AppSheet(
    client: AppCoreClient,
    site: SiteStatus,
    onOpen: () -> Unit,
    onShare: () -> Unit,
    onPinToHome: () -> Unit,
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
                Text(
                    "nsite · ${site.filesTotal} files · ${site.state}",
                    color = Slate,
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
        SheetAction(Icons.Filled.Info, site.host) { }
    }
}

@Composable
private fun SheetAction(icon: androidx.compose.ui.graphics.vector.ImageVector, label: String, onClick: () -> Unit) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .combinedClickableSafe(onClick)
            .padding(vertical = 12.dp),
    ) {
        Icon(icon, contentDescription = null, tint = MaterialTheme.colorScheme.primary)
        Spacer(Modifier.size(14.dp))
        Text(label, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

@Composable
private fun ShareQrDialog(uri: String, onDismiss: () -> Unit) {
    val qr = remember(uri) { NsiteShare.qrBitmap(uri) }
    AlertDialog(
        onDismissRequest = onDismiss,
        confirmButton = { TextButton(onClick = onDismiss) { Text("Close") } },
        title = { Text("Share this app") },
        text = {
            Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Image(qr.asImageBitmap(), contentDescription = "share QR", modifier = Modifier.size(260.dp))
                Text(
                    "Scan with another Myco to open this app — and pair with this device.",
                    color = Slate,
                    style = MaterialTheme.typography.bodySmall,
                    textAlign = TextAlign.Center,
                )
            }
        },
    )
}

@Composable
private fun AddAppDialog(onDismiss: () -> Unit, onPaste: (String) -> Unit, onScan: () -> Unit) {
    var link by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        confirmButton = {
            Button(onClick = { if (link.isNotBlank()) onPaste(link.trim()) }, enabled = link.isNotBlank()) {
                Text("Fetch")
            }
        },
        dismissButton = { OutlinedButton(onClick = onScan) { Text("Scan QR") } },
        title = { Text("Add an app") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Paste an nsite link, or scan a friend's share QR.", color = Slate, style = MaterialTheme.typography.bodySmall)
                OutlinedTextField(
                    value = link,
                    onValueChange = { link = it },
                    singleLine = true,
                    placeholder = { Text("npub… / <npub>.nsite.lol") },
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        },
    )
}

private fun initialOf(site: SiteStatus): String =
    site.title.ifEmpty { site.host }.firstOrNull { it.isLetterOrDigit() }?.uppercase() ?: "?"
