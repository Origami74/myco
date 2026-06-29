package app.myco.ui.screens

import android.Manifest
import android.content.pm.PackageManager
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
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
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.ChevronRight
import androidx.compose.material.icons.filled.Contactless
import androidx.compose.material.icons.filled.ContentPaste
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.FlashOff
import androidx.compose.material.icons.filled.FlashOn
import androidx.compose.material.icons.filled.MarkEmailUnread
import androidx.compose.material.icons.filled.QrCode2
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.NativeActions
import app.myco.nfc.NfcState
import app.myco.nfc.NfcStatus
import app.myco.nfc.PairPresent
import app.myco.share.DeviceName
import app.myco.share.NsiteShare
import app.myco.ui.PeersPill
import app.myco.ui.theme.Emerald
import app.myco.ui.theme.EmeraldInk
import app.myco.ui.theme.EmeraldSoft
import app.myco.ui.theme.Slate
import app.myco.ui.theme.avatarColorFor
import com.google.zxing.BarcodeFormat
import com.google.zxing.ResultPoint
import com.journeyapps.barcodescanner.BarcodeCallback
import com.journeyapps.barcodescanner.BarcodeResult
import com.journeyapps.barcodescanner.BarcodeView
import com.journeyapps.barcodescanner.DefaultDecoderFactory

private val CardBg = Color(0xFFF4F5F7)

/**
 * **Add to circle** — the pairing home (per reference/mockups/pair-01-home.svg).
 * Tap-to-connect sits on top (zero interaction); below it your own editable name,
 * the live **Nearby** list (one tap to send a pair request), and the manual
 * fallbacks (QR / paste) plus the pending-requests inbox. Pairing is always mutual.
 */
@Composable
fun PairScreen(
    state: AppState,
    client: AppCoreClient,
    onScanned: (String) -> Unit,
    onOpenQr: () -> Unit,
    onOpenRequests: () -> Unit,
    onBack: () -> Unit,
) {
    val context = LocalContext.current
    val clipboard = LocalClipboardManager.current
    var name by remember(state.ownNpub) { mutableStateOf(DeviceName.current(context, state.ownNpub)) }
    var editing by remember { mutableStateOf(false) }
    // NFC availability, re-checked whenever we resume (e.g. back from NFC settings).
    val lifecycleOwner = LocalLifecycleOwner.current
    var nfc by remember { mutableStateOf(NfcStatus.state(context)) }
    DisposableEffect(lifecycleOwner) {
        val obs = LifecycleEventObserver { _, e ->
            if (e == Lifecycle.Event.ON_RESUME) nfc = NfcStatus.state(context)
        }
        lifecycleOwner.lifecycle.addObserver(obs)
        onDispose { lifecycleOwner.lifecycle.removeObserver(obs) }
    }
    // Present over NFC while this screen is open, so two phones both on "Add to
    // circle" pair on a single bump (each reads the other). Fresh secret per open.
    DisposableEffect(state.ownNpub, name) {
        if (state.ownNpub.isNotEmpty()) PairPresent.begin(context, state.ownNpub, name)
        onDispose { PairPresent.stop() }
    }
    // We don't get an outgoing-request list back from the core, so track locally
    // which nearby peers we've already pinged this session for the "Sent" state.
    val sent = remember { mutableStateListOf<String>() }

    val circleNpubs = remember(state.circle) { state.circle.map { it.npub }.toSet() }
    val nearby = state.blePeers.filter {
        it.connected && it.npub.isNotEmpty() && it.npub != state.ownNpub && it.npub !in circleNpubs
    }


    Column(modifier = Modifier.fillMaxSize().padding(20.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
            IconButton(onClick = onBack) {
                Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
            }
            Text("Add to circle", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.ExtraBold)
            Spacer(Modifier.weight(1f))
            PeersPill(state)
        }

        Spacer(Modifier.height(16.dp))
        NfcBanner(
            nfc = nfc,
            onConnect = onOpenQr,
            onEnableNfc = { NfcStatus.openSettings(context) },
        )

        Spacer(Modifier.height(12.dp))
        IdentityCard(name = name, onEdit = { editing = true })

        Spacer(Modifier.height(18.dp))
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
            Text(
                "NEARBY",
                color = Slate,
                fontWeight = FontWeight.Bold,
                style = MaterialTheme.typography.labelMedium,
            )
            Spacer(Modifier.weight(1f))
            if (state.bleScanning) {
                Text("scanning…", color = Color(0xFF94A3B8), style = MaterialTheme.typography.labelSmall)
            }
        }
        Spacer(Modifier.height(8.dp))

        LazyColumn(
            modifier = Modifier.weight(1f).fillMaxWidth(),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (nearby.isEmpty()) {
                item {
                    Text(
                        "No one nearby yet. Make sure both phones have Myco open with Bluetooth on.",
                        color = Slate,
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = Modifier.padding(vertical = 8.dp),
                    )
                }
            } else {
                items(nearby, key = { it.npub }) { peer ->
                    NearbyRow(
                        label = DeviceName.generated(peer.npub),
                        npub = peer.npub,
                        proximity = proximityLabel(peer.rssi),
                        requested = peer.npub in sent,
                        onAdd = {
                            client.dispatch(
                                NativeActions.sendPairRequest(peer.npub, name, NsiteShare.newPairSecret())
                            )
                            sent.add(peer.npub)
                            Toast.makeText(context, "Pair request sent", Toast.LENGTH_SHORT).show()
                        },
                    )
                }
            }
        }

        Spacer(Modifier.height(12.dp))
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            MethodButton(
                icon = Icons.Filled.QrCode2,
                label = "QR",
                filled = true,
                modifier = Modifier.weight(1f),
                onClick = onOpenQr,
            )
            MethodButton(
                icon = Icons.Filled.ContentPaste,
                label = "Paste a code",
                filled = false,
                modifier = Modifier.weight(1f),
                onClick = {
                    val text = clipboard.getText()?.text?.trim().orEmpty()
                    if (text.isNotEmpty()) onScanned(text)
                    else Toast.makeText(context, "Clipboard is empty", Toast.LENGTH_SHORT).show()
                },
            )
        }

        if (state.pendingPairRequests.isNotEmpty()) {
            Spacer(Modifier.height(12.dp))
            RequestsBar(count = state.pendingPairRequests.size, onClick = onOpenRequests)
        }
    }

    if (editing) {
        RenameDialog(
            initial = name,
            onDismiss = { editing = false },
            onSave = {
                DeviceName.set(context, it)
                client.dispatch(NativeActions.setDeviceName(it))
                name = it
                editing = false
            },
        )
    }
}

private fun proximityLabel(rssi: Int?): String = when {
    rssi == null -> "nearby"
    rssi >= -55 -> "right next to you"
    rssi >= -72 -> "very close"
    else -> "nearby"
}

/** The tap-to-connect banner. Turns into a warning (and a shortcut to NFC
 *  settings) when NFC is off, so a dead tap isn't silent. */
@Composable
private fun NfcBanner(nfc: NfcState, onConnect: () -> Unit, onEnableNfc: () -> Unit) {
    when (nfc) {
        NfcState.ENABLED -> Surface(
            shape = RoundedCornerShape(18.dp),
            color = EmeraldSoft,
            modifier = Modifier.fillMaxWidth().clickable(onClick = onConnect),
        ) {
            Row(
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 14.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                Icon(Icons.Filled.Contactless, contentDescription = null, tint = Emerald, modifier = Modifier.size(30.dp))
                Column {
                    Text("Tap phones to connect", fontWeight = FontWeight.ExtraBold, color = EmeraldInk, style = MaterialTheme.typography.titleMedium)
                    Text("Hold the backs together — pairs instantly", color = Color(0xFF047857), style = MaterialTheme.typography.bodySmall)
                }
            }
        }
        NfcState.DISABLED -> Surface(
            shape = RoundedCornerShape(18.dp),
            color = Color(0xFFFFF7ED),
            border = androidx.compose.foundation.BorderStroke(1.dp, Color(0xFFFED7AA)),
            modifier = Modifier.fillMaxWidth().clickable(onClick = onEnableNfc),
        ) {
            Row(
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 14.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                Icon(Icons.Filled.Contactless, contentDescription = null, tint = Color(0xFFB45309), modifier = Modifier.size(30.dp))
                Column(modifier = Modifier.weight(1f)) {
                    Text("NFC is off", fontWeight = FontWeight.ExtraBold, color = Color(0xFF9A3412), style = MaterialTheme.typography.titleMedium)
                    Text("Tap to turn it on so you can pair by touching phones", color = Color(0xFFB45309), style = MaterialTheme.typography.bodySmall)
                }
                Text("Turn on", fontWeight = FontWeight.Bold, color = Color(0xFFB45309), style = MaterialTheme.typography.labelLarge)
            }
        }
        NfcState.UNAVAILABLE -> Surface(
            shape = RoundedCornerShape(18.dp),
            color = Color(0xFFF1F5F9),
            modifier = Modifier.fillMaxWidth(),
        ) {
            Row(
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 14.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                Icon(Icons.Filled.Contactless, contentDescription = null, tint = Slate, modifier = Modifier.size(30.dp))
                Column {
                    Text("No NFC on this phone", fontWeight = FontWeight.Bold, color = Color(0xFF334155), style = MaterialTheme.typography.titleMedium)
                    Text("Use a nearby person, QR, or paste instead", color = Slate, style = MaterialTheme.typography.bodySmall)
                }
            }
        }
    }
}

/** "YOU APPEAR AS · <name>" with an edit button — the generated name is shown so
 *  the user actually knows it, and can rename it. */
@Composable
private fun IdentityCard(name: String, onEdit: () -> Unit) {
    Surface(shape = RoundedCornerShape(16.dp), color = CardBg, modifier = Modifier.fillMaxWidth()) {
        Row(
            modifier = Modifier.padding(horizontal = 14.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                modifier = Modifier.size(40.dp).background(Color(0xFF4F46E5), CircleShape),
                contentAlignment = Alignment.Center,
            ) {
                Text(name.firstOrNull()?.uppercase() ?: "?", color = Color.White, fontWeight = FontWeight.Bold)
            }
            Spacer(Modifier.size(12.dp))
            Column(modifier = Modifier.weight(1f)) {
                Text("YOU APPEAR AS", color = Color(0xFF94A3B8), fontWeight = FontWeight.Bold, style = MaterialTheme.typography.labelSmall)
                Text(name, fontWeight = FontWeight.ExtraBold, style = MaterialTheme.typography.titleMedium)
            }
            Surface(shape = CircleShape, color = Color.White, modifier = Modifier.size(34.dp).clickable(onClick = onEdit)) {
                Box(contentAlignment = Alignment.Center) {
                    Icon(Icons.Filled.Edit, contentDescription = "Rename", tint = Slate, modifier = Modifier.size(18.dp))
                }
            }
        }
    }
}

@Composable
private fun NearbyRow(
    label: String,
    npub: String,
    proximity: String,
    requested: Boolean,
    onAdd: () -> Unit,
) {
    Surface(shape = RoundedCornerShape(16.dp), color = CardBg, modifier = Modifier.fillMaxWidth()) {
        Row(
            modifier = Modifier.padding(horizontal = 12.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                modifier = Modifier.size(40.dp).background(avatarColorFor(npub), CircleShape),
                contentAlignment = Alignment.Center,
            ) {
                Text(label.firstOrNull()?.uppercase() ?: "?", color = Color.White, fontWeight = FontWeight.Bold)
            }
            Spacer(Modifier.size(12.dp))
            Column(modifier = Modifier.weight(1f)) {
                Text(label, fontWeight = FontWeight.Bold, style = MaterialTheme.typography.titleSmall)
                Text(
                    proximity,
                    color = if (proximity == "right next to you") Color(0xFF14B8A6) else Slate,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            if (requested) {
                Surface(shape = RoundedCornerShape(50), color = Color.White, border = androidx.compose.foundation.BorderStroke(1.dp, Color(0xFFCBD5E1))) {
                    Text("Sent", color = Slate, fontWeight = FontWeight.Bold, style = MaterialTheme.typography.labelLarge, modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp))
                }
            } else {
                Surface(shape = RoundedCornerShape(50), color = Emerald, modifier = Modifier.clickable(onClick = onAdd)) {
                    Text("Add", color = Color.White, fontWeight = FontWeight.Bold, style = MaterialTheme.typography.labelLarge, modifier = Modifier.padding(horizontal = 18.dp, vertical = 6.dp))
                }
            }
        }
    }
}

@Composable
private fun MethodButton(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    label: String,
    filled: Boolean,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    Surface(
        shape = RoundedCornerShape(14.dp),
        color = if (filled) Color(0xFFF1F5F9) else Color.White,
        border = if (filled) null else androidx.compose.foundation.BorderStroke(1.dp, Color(0xFFCBD5E1)),
        modifier = modifier.height(48.dp).clickable(onClick = onClick),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp),
            horizontalArrangement = Arrangement.Center,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(icon, contentDescription = null, tint = Color(0xFF334155), modifier = Modifier.size(18.dp))
            Spacer(Modifier.size(8.dp))
            Text(label, color = Color(0xFF334155), fontWeight = FontWeight.Bold, style = MaterialTheme.typography.labelLarge)
        }
    }
}

@Composable
private fun RequestsBar(count: Int, onClick: () -> Unit) {
    Surface(shape = RoundedCornerShape(16.dp), color = EmeraldSoft, modifier = Modifier.fillMaxWidth().clickable(onClick = onClick)) {
        Row(
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Filled.MarkEmailUnread, contentDescription = null, tint = Emerald, modifier = Modifier.size(22.dp))
            Spacer(Modifier.size(12.dp))
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    "$count ${if (count == 1) "request" else "requests"} waiting",
                    fontWeight = FontWeight.ExtraBold,
                    color = EmeraldInk,
                    style = MaterialTheme.typography.titleSmall,
                )
                Text("someone wants to join your circle", color = Color(0xFF047857), style = MaterialTheme.typography.bodySmall)
            }
            Icon(Icons.Filled.ChevronRight, contentDescription = null, tint = Emerald)
        }
    }
}

@Composable
private fun RenameDialog(initial: String, onDismiss: () -> Unit, onSave: (String) -> Unit) {
    var value by remember { mutableStateOf(initial) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Your name") },
        text = {
            Column {
                Text("How you appear to people you pair with. Pick something you can say out loud.", color = Slate, style = MaterialTheme.typography.bodyMedium)
                Spacer(Modifier.height(12.dp))
                OutlinedTextField(value = value, onValueChange = { value = it }, singleLine = true, modifier = Modifier.fillMaxWidth())
            }
        },
        confirmButton = { TextButton(onClick = { if (value.isNotBlank()) onSave(value.trim()) }) { Text("Save") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/**
 * The **QR** screen (per reference/mockups/pair-05-qr.svg): scan a friend's code on
 * top, show your own below. The "show yours" half is also the NFC "present" spot.
 */
@Composable
fun QrScreen(
    state: AppState,
    onScanned: (String) -> Unit,
    onBack: () -> Unit,
) {
    val context = LocalContext.current
    val name = remember(state.ownNpub) { DeviceName.current(context, state.ownNpub) }

    Column(modifier = Modifier.fillMaxSize().padding(20.dp).verticalScroll(rememberScrollState())) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") }
            Text("QR", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.ExtraBold)
        }

        Spacer(Modifier.height(12.dp))
        Text("SCAN A FRIEND'S CODE", color = Slate, fontWeight = FontWeight.Bold, style = MaterialTheme.typography.labelMedium)
        Spacer(Modifier.height(8.dp))
        Box(modifier = Modifier.fillMaxWidth().height(240.dp)) { ScanPanel(onScanned = onScanned) }

        Spacer(Modifier.height(20.dp))
        Text("OR SHOW YOURS", color = Slate, fontWeight = FontWeight.Bold, style = MaterialTheme.typography.labelMedium)
        Spacer(Modifier.height(8.dp))
        MyCodePanel(state = state, deviceName = name)

        Spacer(Modifier.height(16.dp))
        val clipboard = LocalClipboardManager.current
        Surface(
            shape = RoundedCornerShape(14.dp),
            color = Color.White,
            border = androidx.compose.foundation.BorderStroke(1.dp, Color(0xFFCBD5E1)),
            modifier = Modifier.fillMaxWidth().height(48.dp).clickable {
                val text = clipboard.getText()?.text?.trim().orEmpty()
                if (text.isNotEmpty()) onScanned(text)
                else Toast.makeText(context, "Clipboard is empty", Toast.LENGTH_SHORT).show()
            },
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.Center,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(Icons.Filled.ContentPaste, contentDescription = null, tint = Color(0xFF334155), modifier = Modifier.size(18.dp))
                Spacer(Modifier.size(8.dp))
                Text("Paste a code", color = Color(0xFF334155), fontWeight = FontWeight.Bold, style = MaterialTheme.typography.labelLarge)
            }
        }
    }
}

/**
 * **Add an app**: the same scanner, but the alternate tab is "Enter URL" (paste an
 * nsite link). Default tab is Scan.
 */
@Composable
fun AddScreen(state: AppState, onScanned: (String) -> Unit, onBack: () -> Unit) {
    var showAlt by remember { mutableStateOf(false) }

    Column(modifier = Modifier.fillMaxSize().padding(20.dp)) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") }
                Text("Add an app", style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.ExtraBold)
            }
            PeersPill(state)
        }

        Spacer(Modifier.height(16.dp))
        SegmentedToggle(showAlt = showAlt, altLabel = "Enter URL", onSelect = { showAlt = it })
        Spacer(Modifier.height(20.dp))

        Box(modifier = Modifier.fillMaxWidth().weight(1f)) {
            if (showAlt) UrlPanel(onSubmit = onScanned) else ScanPanel(onScanned = onScanned)
        }

        Spacer(Modifier.height(16.dp))
        Text(
            if (showAlt) "Paste a public nsite link to fetch it." else "Scan a friend's share code to add their app.",
            color = MaterialTheme.colorScheme.onSurface,
            style = MaterialTheme.typography.bodyLarge,
            textAlign = TextAlign.Center,
            modifier = Modifier.fillMaxWidth(),
        )
    }
}

@Composable
private fun SegmentedToggle(showAlt: Boolean, altLabel: String, onSelect: (Boolean) -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(50))
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .padding(4.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Segment("Scan", selected = !showAlt, modifier = Modifier.weight(1f)) { onSelect(false) }
        Segment(altLabel, selected = showAlt, modifier = Modifier.weight(1f)) { onSelect(true) }
    }
}

@Composable
private fun Segment(label: String, selected: Boolean, modifier: Modifier, onClick: () -> Unit) {
    Box(
        modifier = modifier
            .clip(RoundedCornerShape(50))
            .background(if (selected) MaterialTheme.colorScheme.primary else Color.Transparent)
            .clickable(onClick = onClick)
            .padding(vertical = 12.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            label,
            color = if (selected) MaterialTheme.colorScheme.onPrimary else Slate,
            fontWeight = FontWeight.Bold,
            style = MaterialTheme.typography.titleMedium,
        )
    }
}

@Composable
private fun ScanPanel(onScanned: (String) -> Unit) {
    val context = LocalContext.current
    var granted by remember {
        mutableStateOf(
            ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
        )
    }
    val permLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted = it }
    LaunchedEffect(Unit) { if (!granted) permLauncher.launch(Manifest.permission.CAMERA) }

    Box(
        modifier = Modifier.fillMaxSize().clip(RoundedCornerShape(22.dp)).background(Color(0xFF0F172A)),
        contentAlignment = Alignment.Center,
    ) {
        if (granted) {
            CameraScanner(onScanned = onScanned)
            ReticleOverlay()
        } else {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                Text("Camera access needed to scan", color = Color.White, textAlign = TextAlign.Center)
                Spacer(Modifier.height(12.dp))
                Button(onClick = { permLauncher.launch(Manifest.permission.CAMERA) }) { Text("Allow camera") }
            }
        }
    }
}

@Composable
private fun CameraScanner(onScanned: (String) -> Unit) {
    val lifecycleOwner = LocalLifecycleOwner.current
    var view by remember { mutableStateOf<BarcodeView?>(null) }
    var torchOn by remember { mutableStateOf(false) }
    var handled by remember { mutableStateOf(false) }

    DisposableEffect(lifecycleOwner, view) {
        val bv = view
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_RESUME -> bv?.resume()
                Lifecycle.Event.ON_PAUSE -> bv?.pause()
                else -> {}
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose {
            lifecycleOwner.lifecycle.removeObserver(observer)
            bv?.pause()
        }
    }

    androidx.compose.ui.viewinterop.AndroidView(
        modifier = Modifier.fillMaxSize(),
        factory = { ctx ->
            BarcodeView(ctx).apply {
                decoderFactory = DefaultDecoderFactory(listOf(BarcodeFormat.QR_CODE))
                decodeContinuous(object : BarcodeCallback {
                    override fun barcodeResult(result: BarcodeResult) {
                        if (!handled) {
                            handled = true
                            pause()
                            onScanned(result.text)
                        }
                    }
                    override fun possibleResultPoints(points: MutableList<ResultPoint>) {}
                })
                resume()
                view = this
            }
        },
    )

    Box(modifier = Modifier.fillMaxSize().padding(12.dp), contentAlignment = Alignment.TopEnd) {
        Box(
            modifier = Modifier
                .size(44.dp)
                .clip(CircleShape)
                .background(Color.White.copy(alpha = 0.18f))
                .clickable { torchOn = !torchOn; view?.setTorch(torchOn) },
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                if (torchOn) Icons.Filled.FlashOn else Icons.Filled.FlashOff,
                contentDescription = "Torch",
                tint = Color.White,
            )
        }
    }
}

/** Emerald corner brackets framing the viewfinder. */
@Composable
private fun ReticleOverlay() {
    Canvas(modifier = Modifier.fillMaxSize().padding(28.dp)) {
        val arm = 34.dp.toPx()
        val sw = 5.dp.toPx()
        val l = 0f
        val t = 0f
        val r = size.width
        val b = size.height
        fun line(a: Offset, c: Offset) = drawLine(Emerald, a, c, strokeWidth = sw, cap = StrokeCap.Round)
        line(Offset(l, t + arm), Offset(l, t)); line(Offset(l, t), Offset(l + arm, t))
        line(Offset(r - arm, t), Offset(r, t)); line(Offset(r, t), Offset(r, t + arm))
        line(Offset(l, b - arm), Offset(l, b)); line(Offset(l, b), Offset(l + arm, b))
        line(Offset(r - arm, b), Offset(r, b)); line(Offset(r, b), Offset(r, b - arm))
    }
}

@Composable
private fun MyCodePanel(state: AppState, deviceName: String) {
    // Use the same payload NFC is presenting (rotates after each tap is consumed),
    // so the shown QR and the tapped code always carry the same live secret.
    val fallback = remember(state.ownNpub, deviceName) {
        NsiteShare.buildPairUri(state.ownNpub, deviceName, NsiteShare.newPairSecret())
    }
    val pairUri = PairPresent.payload.value ?: fallback
    val qr = remember(pairUri) { NsiteShare.qrBitmap(pairUri) }
    Surface(shape = RoundedCornerShape(20.dp), color = MaterialTheme.colorScheme.surfaceVariant, modifier = Modifier.fillMaxWidth()) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(20.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Box(modifier = Modifier.clip(RoundedCornerShape(16.dp)).background(Color.White).padding(14.dp)) {
                Image(qr.asImageBitmap(), contentDescription = "Your pairing code", modifier = Modifier.size(180.dp))
            }
            Spacer(Modifier.height(12.dp))
            Text(deviceName, fontWeight = FontWeight.ExtraBold, style = MaterialTheme.typography.titleMedium)
            Spacer(Modifier.height(4.dp))
            Text("Friend scans this — or taps phones — to add you", color = Slate, style = MaterialTheme.typography.bodySmall, textAlign = TextAlign.Center)
            Spacer(Modifier.height(10.dp))
            Surface(shape = RoundedCornerShape(50), color = EmeraldSoft) {
                Row(
                    modifier = Modifier.padding(horizontal = 12.dp, vertical = 5.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Icon(Icons.Filled.Contactless, contentDescription = null, tint = Emerald, modifier = Modifier.size(14.dp))
                    Text("also tap-to-pair ready", color = Color(0xFF047857), fontWeight = FontWeight.Bold, style = MaterialTheme.typography.labelMedium)
                }
            }
        }
    }
}

@Composable
private fun UrlPanel(onSubmit: (String) -> Unit) {
    var link by remember { mutableStateOf("") }
    val clipboard = LocalClipboardManager.current
    Box(
        modifier = Modifier.fillMaxSize().clip(RoundedCornerShape(22.dp)).background(MaterialTheme.colorScheme.surfaceVariant),
        contentAlignment = Alignment.Center,
    ) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(20.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            OutlinedTextField(
                value = link,
                onValueChange = { link = it },
                singleLine = true,
                placeholder = { Text("npub… / <npub>.nsite.lol") },
                trailingIcon = {
                    IconButton(onClick = {
                        clipboard.getText()?.text?.let { if (it.isNotBlank()) link = it.trim() }
                    }) {
                        Icon(Icons.Filled.ContentPaste, contentDescription = "Paste")
                    }
                },
                modifier = Modifier.fillMaxWidth(),
            )
            Spacer(Modifier.height(14.dp))
            Button(
                onClick = { if (link.isNotBlank()) onSubmit(link.trim()) },
                enabled = link.isNotBlank(),
                modifier = Modifier.fillMaxWidth(),
            ) { Text("Fetch") }
        }
    }
}
