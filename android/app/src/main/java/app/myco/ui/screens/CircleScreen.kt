package app.myco.ui.screens

import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Contactless
import androidx.compose.material.icons.filled.ChevronRight
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.MarkEmailUnread
import androidx.compose.material.icons.filled.QrCode2
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.CircleContact
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
import app.myco.ui.theme.StatusConnected
import app.myco.ui.theme.avatarColorFor

private val Ink = Color(0xFF0F172A)
private val Hairline = Color(0xFFCBD5E1)
private val CardBg = Color(0xFFF4F5F7)

private enum class Ring { ONLINE, DASHED, NONE }
private enum class Badge { NONE, PLUS, SENT }

/**
 * The **Circle** home — also the only place you add people (the separate
 * "Add to circle" screen is merged in here). Top to bottom: who you appear as,
 * a tap-to-connect (NFC) hint, **Nearby** people (tap a bubble to add), and your
 * **Circle** as bubbles (green ring = online). A QR bubble (bottom-right) opens
 * scan / show / paste. While this screen is open the device presents over NFC, so
 * two phones both here pair on a single bump.
 */
@OptIn(ExperimentalFoundationApi::class, ExperimentalLayoutApi::class)
@Composable
fun CircleScreen(
    state: AppState,
    client: AppCoreClient,
    onOpenQr: () -> Unit,
    onOpenRequests: () -> Unit,
) {
    val context = LocalContext.current
    var name by remember(state.ownNpub) { mutableStateOf(DeviceName.current(context, state.ownNpub)) }
    var editing by remember { mutableStateOf(false) }
    var forget by remember { mutableStateOf<CircleContact?>(null) }
    val sent = remember { mutableStateListOf<String>() }

    // NFC availability, re-checked on resume (e.g. back from NFC settings).
    val lifecycleOwner = LocalLifecycleOwner.current
    var nfc by remember { mutableStateOf(NfcStatus.state(context)) }
    DisposableEffect(lifecycleOwner) {
        val obs = LifecycleEventObserver { _, e ->
            if (e == Lifecycle.Event.ON_RESUME) nfc = NfcStatus.state(context)
        }
        lifecycleOwner.lifecycle.addObserver(obs)
        onDispose { lifecycleOwner.lifecycle.removeObserver(obs) }
    }
    // Present (emulate an NFC card) only while the Circle tab is open. Leaving the
    // tab stops it, so we never advertise from Apps/Settings/etc.
    DisposableEffect(state.ownNpub, name) {
        if (state.ownNpub.isNotEmpty()) PairPresent.begin(context, state.ownNpub, name)
        onDispose { PairPresent.stop() }
    }

    val connected = state.blePeers.filter { it.connected }.map { it.npub }.toSet()
    val circleNpubs = remember(state.circle) { state.circle.map { it.npub }.toSet() }
    val nearby = state.blePeers.filter {
        it.connected && it.npub.isNotEmpty() && it.npub != state.ownNpub && it.npub !in circleNpubs
    }

    Box(modifier = Modifier.fillMaxSize()) {
        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = androidx.compose.foundation.layout.PaddingValues(20.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            item {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text("Circle", style = MaterialTheme.typography.displaySmall)
                    PeersPill(state)
                }
            }
            item { IdentityChip(name = name, onEdit = { editing = true }) }

            if (state.pendingPairRequests.isNotEmpty()) {
                item { RequestsBar(count = state.pendingPairRequests.size, onClick = onOpenRequests) }
            }

            if (nfc != NfcState.UNAVAILABLE) {
                item { TapToConnect(nfc = nfc, onEnableNfc = { NfcStatus.openSettings(context) }) }
            }

            if (nearby.isNotEmpty()) {
                item { SectionLabel("NEARBY", trailing = "· tap to add", scanning = state.bleScanning) }
                item {
                    FlowRow(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalArrangement = Arrangement.spacedBy(12.dp),
                        maxItemsInEachRow = 4,
                    ) {
                        nearby.forEach { peer ->
                            val isSent = peer.npub in sent
                            PersonBubble(
                                label = DeviceName.generated(peer.npub),
                                npub = peer.npub,
                                ring = Ring.DASHED,
                                badge = if (isSent) Badge.SENT else Badge.PLUS,
                                dim = false,
                                onClick = if (isSent) null else {
                                    {
                                        client.dispatch(
                                            NativeActions.sendPairRequest(peer.npub, name, NsiteShare.newPairSecret())
                                        )
                                        sent.add(peer.npub)
                                    }
                                },
                            )
                        }
                    }
                }
            }

            item { SectionLabel("IN YOUR CIRCLE", trailing = null, scanning = false) }
            if (state.circle.isEmpty()) {
                item {
                    Text(
                        "No one yet. Bump phones, or tap someone in Nearby.",
                        color = Slate,
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            } else {
                item {
                    FlowRow(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalArrangement = Arrangement.spacedBy(12.dp),
                        maxItemsInEachRow = 4,
                    ) {
                        state.circle.forEach { c ->
                            val online = c.npub in connected
                            PersonBubble(
                                label = c.name.ifEmpty { "unknown" },
                                npub = c.npub,
                                ring = if (online) Ring.ONLINE else Ring.NONE,
                                badge = Badge.NONE,
                                dim = !online,
                                onLongClick = { forget = c },
                            )
                        }
                    }
                }
            }

            item { Spacer(Modifier.height(72.dp)) } // room for the FAB
        }

        // QR bubble — scan / show / paste.
        Surface(
            shape = CircleShape,
            color = Emerald,
            shadowElevation = 6.dp,
            modifier = Modifier
                .align(Alignment.BottomEnd)
                .padding(24.dp)
                .size(56.dp)
                .clickable(onClick = onOpenQr),
        ) {
            Box(contentAlignment = Alignment.Center) {
                Icon(Icons.Filled.QrCode2, contentDescription = "Scan or show a code", tint = Color.White, modifier = Modifier.size(26.dp))
            }
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
                // Refresh the NFC payload so we present the new name immediately.
                if (state.ownNpub.isNotEmpty()) PairPresent.begin(context, state.ownNpub, it)
                editing = false
            },
        )
    }

    forget?.let { c ->
        AlertDialog(
            onDismissRequest = { forget = null },
            confirmButton = {
                TextButton(onClick = { client.dispatch(NativeActions.removeFromCircle(c.npub)); forget = null }) { Text("Forget") }
            },
            dismissButton = { TextButton(onClick = { forget = null }) { Text("Cancel") } },
            title = { Text("Forget ${c.name.ifEmpty { "this peer" }}?") },
            text = { Text("They'll be removed from your Circle. You can re-pair anytime.") },
        )
    }
}

@Composable
private fun IdentityChip(name: String, onEdit: () -> Unit) {
    Surface(shape = RoundedCornerShape(50), color = CardBg, modifier = Modifier.clickable(onClick = onEdit)) {
        Row(
            modifier = Modifier.padding(start = 6.dp, end = 12.dp, top = 5.dp, bottom = 5.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Box(
                modifier = Modifier.size(22.dp).background(Color(0xFF4F46E5), CircleShape),
                contentAlignment = Alignment.Center,
            ) {
                Text(name.firstOrNull()?.uppercase() ?: "?", color = Color.White, fontWeight = FontWeight.Bold, fontSize = 11.sp)
            }
            Text(name, fontWeight = FontWeight.Bold, style = MaterialTheme.typography.titleSmall)
            Icon(Icons.Filled.Edit, contentDescription = "Rename", tint = Slate, modifier = Modifier.size(15.dp))
        }
    }
}

@Composable
private fun TapToConnect(nfc: NfcState, onEnableNfc: () -> Unit) {
    Column {
        SectionLabel("TAP TO CONNECT", trailing = null, scanning = false)
        Spacer(Modifier.height(10.dp))
        val warn = nfc == NfcState.DISABLED
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .then(if (warn) Modifier.clickable(onClick = onEnableNfc) else Modifier),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            NfcBubble(warn = warn)
            Column(modifier = Modifier.weight(1f)) {
                if (warn) {
                    Text("NFC is off", fontWeight = FontWeight.ExtraBold, color = Color(0xFF9A3412), style = MaterialTheme.typography.titleSmall)
                    Text("Tap to turn it on, then bump phones", color = Color(0xFFB45309), style = MaterialTheme.typography.bodySmall)
                } else {
                    Text("Bump phones to connect", fontWeight = FontWeight.ExtraBold, color = Ink, style = MaterialTheme.typography.titleSmall)
                    Text("Hold the backs together — pairs instantly", color = Slate, style = MaterialTheme.typography.bodySmall)
                }
            }
        }
    }
}

/** The tap-to-connect bubble with a subtle, looping "signal" animation — the two
 *  outer contactless arcs breathe to draw a little attention without distracting. */
@Composable
private fun NfcBubble(warn: Boolean) {
    Box(
        modifier = Modifier.size(48.dp).background(if (warn) Color(0xFFB45309) else Emerald, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        if (warn) {
            Icon(
                Icons.Filled.Contactless,
                contentDescription = null,
                tint = Color.White,
                modifier = Modifier.size(26.dp),
            )
        } else {
            val transition = rememberInfiniteTransition(label = "nfc")
            val wave by transition.animateFloat(
                initialValue = 0.25f,
                targetValue = 1f,
                animationSpec = infiniteRepeatable(tween(950, easing = FastOutSlowInEasing), RepeatMode.Reverse),
                label = "wave",
            )
            Canvas(modifier = Modifier.size(28.dp)) {
                val cx = size.width * 0.30f
                val cy = size.height / 2f
                val white = Color.White
                drawCircle(white, radius = 1.8.dp.toPx(), center = Offset(cx, cy))
                fun arc(rDp: Float, alpha: Float) {
                    val r = rDp.dp.toPx()
                    drawArc(
                        color = white.copy(alpha = alpha),
                        startAngle = -52f,
                        sweepAngle = 104f,
                        useCenter = false,
                        topLeft = Offset(cx - r, cy - r),
                        size = Size(2 * r, 2 * r),
                        style = Stroke(width = 2.2.dp.toPx(), cap = StrokeCap.Round),
                    )
                }
                arc(4.5f, 1f)
                arc(8f, wave)
                arc(11.5f, wave * 0.7f)
            }
        }
    }
}

@Composable
private fun SectionLabel(text: String, trailing: String?, scanning: Boolean) {
    Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
        Text(text, color = Slate, fontWeight = FontWeight.Bold, style = MaterialTheme.typography.labelMedium)
        if (trailing != null) {
            Spacer(Modifier.width(6.dp))
            Text(trailing, color = Color(0xFF94A3B8), style = MaterialTheme.typography.labelMedium)
        }
        if (scanning) {
            Spacer(Modifier.weight(1f))
            Text("looking…", color = Color(0xFF94A3B8), style = MaterialTheme.typography.labelSmall)
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun PersonBubble(
    label: String,
    npub: String,
    ring: Ring,
    badge: Badge,
    dim: Boolean,
    onClick: (() -> Unit)? = null,
    onLongClick: (() -> Unit)? = null,
) {
    Column(
        modifier = Modifier
            .width(58.dp)
            .then(
                if (onClick != null || onLongClick != null) {
                    Modifier.combinedClickable(onClick = { onClick?.invoke() }, onLongClick = onLongClick)
                } else {
                    Modifier
                }
            )
            .alpha(if (dim) 0.5f else 1f),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Box(modifier = Modifier.size(52.dp), contentAlignment = Alignment.Center) {
            Canvas(modifier = Modifier.matchParentSize()) {
                val r = size.minDimension / 2f - 1.5.dp.toPx()
                when (ring) {
                    Ring.ONLINE -> drawCircle(StatusConnected, r, style = Stroke(2.5.dp.toPx()))
                    Ring.DASHED -> drawCircle(
                        Hairline, r,
                        style = Stroke(1.5.dp.toPx(), pathEffect = PathEffect.dashPathEffect(floatArrayOf(8f, 8f))),
                    )
                    Ring.NONE -> {}
                }
            }
            Box(
                modifier = Modifier.size(44.dp).clip(CircleShape).background(avatarColorFor(npub)),
                contentAlignment = Alignment.Center,
            ) {
                Text(label.firstOrNull()?.uppercase() ?: "?", color = Color.White, fontWeight = FontWeight.Bold, style = MaterialTheme.typography.titleMedium)
            }
            if (badge != Badge.NONE) {
                Box(
                    modifier = Modifier
                        .align(Alignment.BottomEnd)
                        .size(18.dp)
                        .clip(CircleShape)
                        .background(if (badge == Badge.SENT) Slate else Emerald)
                        .border(2.dp, Color.White, CircleShape),
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(
                        if (badge == Badge.SENT) Icons.Filled.Check else Icons.Filled.Add,
                        contentDescription = null,
                        tint = Color.White,
                        modifier = Modifier.size(11.dp),
                    )
                }
            }
        }
        Spacer(Modifier.height(6.dp))
        Text(
            label,
            fontSize = 10.sp,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            color = if (dim) Slate else Ink,
            textAlign = TextAlign.Center,
            modifier = Modifier.fillMaxWidth(),
        )
    }
}

@Composable
private fun RequestsBar(count: Int, onClick: () -> Unit) {
    Surface(shape = RoundedCornerShape(16.dp), color = EmeraldSoft, modifier = Modifier.fillMaxWidth().clickable(onClick = onClick)) {
        Row(modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Filled.MarkEmailUnread, contentDescription = null, tint = Emerald, modifier = Modifier.size(22.dp))
            Spacer(Modifier.size(12.dp))
            Text(
                "$count ${if (count == 1) "request" else "requests"} waiting",
                fontWeight = FontWeight.ExtraBold,
                color = EmeraldInk,
                style = MaterialTheme.typography.titleSmall,
                modifier = Modifier.weight(1f),
            )
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
