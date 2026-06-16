package app.myco.ui.screens

import android.Manifest
import android.content.pm.PackageManager
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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.FlashOff
import androidx.compose.material.icons.filled.FlashOn
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import app.myco.core.AppState
import app.myco.share.NsiteShare
import app.myco.ui.PeersPill
import app.myco.ui.theme.Emerald
import app.myco.ui.theme.Slate
import com.google.zxing.BarcodeFormat
import com.google.zxing.ResultPoint
import com.journeyapps.barcodescanner.BarcodeCallback
import com.journeyapps.barcodescanner.BarcodeResult
import com.journeyapps.barcodescanner.BarcodeView
import com.journeyapps.barcodescanner.DefaultDecoderFactory

/**
 * **Pair a device** (per docs/design/mockups/ui-06-qr-scan.svg): scan a friend's
 * code, or flip to "My code" to show your own. Pairing is mutual.
 */
@Composable
fun PairScreen(state: AppState, onScanned: (String) -> Unit, onBack: () -> Unit) {
    ScanScreen(
        title = "Pair a device",
        state = state,
        altLabel = "My code",
        captionScan = "Point at your friend's Myco code.",
        captionAlt = "Let your friend scan this with their Myco.",
        subCaption = "Pairing is mutual — they confirm on their phone too.",
        onBack = onBack,
        onScanned = onScanned,
    ) { MyCodePanel(state) }
}

/**
 * **Add an app**: the same scanner, but the alternate tab is "Enter URL" (paste an
 * nsite link) instead of "My code". Default tab is Scan.
 */
@Composable
fun AddScreen(state: AppState, onScanned: (String) -> Unit, onBack: () -> Unit) {
    ScanScreen(
        title = "Add an app",
        state = state,
        altLabel = "Enter URL",
        captionScan = "Scan a friend's share code to add their app.",
        captionAlt = "Paste a public nsite link to fetch it.",
        onBack = onBack,
        onScanned = onScanned,
    ) { UrlPanel(onSubmit = onScanned) }
}

/** Shared scaffold: a Scan / <alt> segmented toggle over the camera viewfinder, or
 *  an alternate panel. Default tab is Scan. */
@Composable
private fun ScanScreen(
    title: String,
    state: AppState,
    altLabel: String,
    captionScan: String,
    captionAlt: String,
    subCaption: String? = null,
    onBack: () -> Unit,
    onScanned: (String) -> Unit,
    altContent: @Composable () -> Unit,
) {
    var showAlt by remember { mutableStateOf(false) }

    Column(modifier = Modifier.fillMaxSize().padding(20.dp)) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = onBack) {
                    Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                }
                Text(title, style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.ExtraBold)
            }
            PeersPill(state)
        }

        Spacer(Modifier.height(16.dp))
        SegmentedToggle(showAlt = showAlt, altLabel = altLabel, onSelect = { showAlt = it })
        Spacer(Modifier.height(20.dp))

        Box(modifier = Modifier.fillMaxWidth().weight(1f)) {
            if (showAlt) altContent() else ScanPanel(onScanned = onScanned)
        }

        Spacer(Modifier.height(16.dp))
        Text(
            if (showAlt) captionAlt else captionScan,
            color = MaterialTheme.colorScheme.onSurface,
            style = MaterialTheme.typography.bodyLarge,
            textAlign = TextAlign.Center,
            modifier = Modifier.fillMaxWidth(),
        )
        if (subCaption != null) {
            Text(
                subCaption,
                color = Slate,
                style = MaterialTheme.typography.bodySmall,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth().padding(top = 4.dp),
            )
        }
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
        modifier = Modifier.fillMaxSize().clip(RoundedCornerShape(22.dp)).background(Color(0xFF111827)),
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
private fun MyCodePanel(state: AppState) {
    val pairUri = remember(state.ownNpub) {
        NsiteShare.buildPairUri(state.ownNpub, NsiteShare.deviceName(state.ownNpub), NsiteShare.newPairSecret())
    }
    val qr = remember(pairUri) { NsiteShare.qrBitmap(pairUri) }
    Box(
        modifier = Modifier.fillMaxSize().clip(RoundedCornerShape(22.dp)).background(MaterialTheme.colorScheme.surfaceVariant),
        contentAlignment = Alignment.Center,
    ) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Box(modifier = Modifier.clip(RoundedCornerShape(20.dp)).background(Color.White).padding(16.dp)) {
                Image(qr.asImageBitmap(), contentDescription = "Your pairing code", modifier = Modifier.size(240.dp))
            }
            Spacer(Modifier.height(16.dp))
            Text(NsiteShare.deviceName(state.ownNpub), fontWeight = FontWeight.Bold, style = MaterialTheme.typography.titleMedium)
        }
    }
}

@Composable
private fun UrlPanel(onSubmit: (String) -> Unit) {
    var link by remember { mutableStateOf("") }
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
