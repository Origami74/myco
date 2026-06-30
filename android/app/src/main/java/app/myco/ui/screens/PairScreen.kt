package app.myco.ui.screens

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
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
import androidx.compose.material.icons.filled.Contactless
import androidx.compose.material.icons.filled.FlashOff
import androidx.compose.material.icons.filled.FlashOn
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import app.myco.core.AppState
import app.myco.nfc.PairPresent
import app.myco.share.DeviceName
import app.myco.share.NsiteShare
import app.myco.ui.theme.Emerald
import app.myco.ui.theme.EmeraldSoft
import app.myco.ui.theme.Slate
import com.google.zxing.BarcodeFormat
import com.google.zxing.ResultPoint
import com.journeyapps.barcodescanner.BarcodeCallback
import com.journeyapps.barcodescanner.BarcodeResult
import com.journeyapps.barcodescanner.BarcodeView
import com.journeyapps.barcodescanner.DefaultDecoderFactory


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
        PasteCodeButton(label = "Paste a code", onPaste = onScanned)
    }
}

@Composable
internal fun ScanPanel(onScanned: (String) -> Unit) {
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
            QrCodeCard(qr, contentDescription = "Your pairing code")
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
