package app.myco.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.border
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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.SwapHoriz
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.NativeActions
import app.myco.core.PairRequest
import app.myco.ui.theme.Emerald
import app.myco.ui.theme.EmeraldInk
import app.myco.ui.theme.EmeraldSoft
import app.myco.ui.theme.Slate
import app.myco.ui.theme.avatarColorFor

private val CardBg = Color(0xFFF4F5F7)

/**
 * The **Requests** inbox (per reference/mockups/pair-02-requests.svg): incoming
 * pair requests wait here — accept whenever, no timing pressure. Accepting is
 * always mutual (it adds you to their circle too). NFC taps connect on their own
 * and never land here.
 */
@Composable
fun RequestsScreen(
    state: AppState,
    client: AppCoreClient,
    onBack: () -> Unit,
) {
    val pending = state.pendingPairRequests

    Column(modifier = Modifier.fillMaxSize().padding(20.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") }
            Text("Requests", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.ExtraBold)
            if (pending.isNotEmpty()) {
                Spacer(Modifier.size(10.dp))
                Surface(shape = CircleShape, color = Emerald) {
                    Text(
                        "${pending.size}",
                        color = Color.White,
                        fontWeight = FontWeight.ExtraBold,
                        style = MaterialTheme.typography.labelMedium,
                        modifier = Modifier.padding(horizontal = 8.dp, vertical = 2.dp),
                    )
                }
            }
        }

        Spacer(Modifier.height(8.dp))
        Text(
            "People who scanned your code or opened your link. They wait here — accept whenever you like, no rush.",
            color = Slate,
            style = MaterialTheme.typography.bodyMedium,
        )
        Spacer(Modifier.height(16.dp))

        if (pending.isEmpty()) {
            Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.TopCenter) {
                Text(
                    "No requests right now.\nWhen someone scans your code, they'll show up here.",
                    color = Slate,
                    textAlign = TextAlign.Center,
                    modifier = Modifier.padding(top = 40.dp),
                )
            }
            return@Column
        }

        LazyColumn(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            items(pending, key = { it.npub }) { req ->
                RequestCard(
                    req = req,
                    onAccept = {
                        // Adds them to the Circle; the "connected" celebration fires
                        // from MycoApp when the Circle grows (covers both sides).
                        client.dispatch(NativeActions.acceptPairRequest(req.npub, req.name))
                    },
                    onIgnore = { client.dispatch(NativeActions.declinePairRequest(req.npub)) },
                )
            }
            item { VerifyHint() }
            item {
                Spacer(Modifier.height(4.dp))
                Text(
                    "Accepting is mutual — it adds you to their circle too. NFC taps skip this list; the tap is the confirmation.",
                    color = Color(0xFF94A3B8),
                    style = MaterialTheme.typography.bodySmall,
                    textAlign = TextAlign.Center,
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        }
    }

}

@Composable
private fun RequestCard(req: PairRequest, onAccept: () -> Unit, onIgnore: () -> Unit) {
    val name = req.name.ifEmpty { "Unknown device" }
    Surface(shape = RoundedCornerShape(18.dp), color = CardBg, modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(14.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(
                    modifier = Modifier.size(40.dp).background(avatarColorFor(req.npub), CircleShape),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(name.firstOrNull()?.uppercase() ?: "?", color = Color.White, fontWeight = FontWeight.Bold)
                }
                Spacer(Modifier.size(12.dp))
                Column {
                    Text(name, fontWeight = FontWeight.ExtraBold, style = MaterialTheme.typography.titleSmall)
                    Text(
                        shortNpub(req.npub),
                        color = Slate,
                        style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                    )
                }
            }
            Spacer(Modifier.height(12.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Surface(
                    shape = RoundedCornerShape(50),
                    color = Color.White,
                    border = androidx.compose.foundation.BorderStroke(1.dp, Color(0xFFCBD5E1)),
                    modifier = Modifier.weight(1f).clickable(onClick = onIgnore),
                ) {
                    Box(modifier = Modifier.padding(vertical = 8.dp), contentAlignment = Alignment.Center) {
                        Text("Ignore", color = Slate, fontWeight = FontWeight.Bold)
                    }
                }
                Surface(
                    shape = RoundedCornerShape(50),
                    color = Emerald,
                    modifier = Modifier.weight(1f).clickable(onClick = onAccept),
                ) {
                    Row(
                        modifier = Modifier.padding(vertical = 8.dp).fillMaxWidth(),
                        horizontalArrangement = Arrangement.Center,
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Icon(Icons.Filled.Check, contentDescription = null, tint = Color.White, modifier = Modifier.size(18.dp))
                        Spacer(Modifier.size(6.dp))
                        Text("Accept", color = Color.White, fontWeight = FontWeight.Bold)
                    }
                }
            }
        }
    }
}

@Composable
private fun VerifyHint() {
    Surface(
        shape = RoundedCornerShape(14.dp),
        color = Color(0xFFFFF7ED),
        border = androidx.compose.foundation.BorderStroke(1.dp, Color(0xFFFED7AA)),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(modifier = Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(
                modifier = Modifier.size(22.dp).background(Color(0xFFFDE68A), CircleShape),
                contentAlignment = Alignment.Center,
            ) { Text("!", color = Color(0xFFB45309), fontWeight = FontWeight.ExtraBold) }
            Spacer(Modifier.size(10.dp))
            Column {
                Text("Names are self-chosen", color = Color(0xFF9A3412), fontWeight = FontWeight.Bold, style = MaterialTheme.typography.labelLarge)
                Text("check it matches what they tell you out loud", color = Color(0xFFB45309), style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}

/** The mutual "You're connected ✓" celebration (per pair-03-connected.svg). */
@Composable
fun PairConnectedDialog(theirName: String, onDone: () -> Unit) {
    Dialog(onDismissRequest = onDone) {
        Surface(shape = RoundedCornerShape(24.dp), color = Color.White) {
            Column(
                modifier = Modifier.padding(24.dp).fillMaxWidth(),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                // two avatars + center check
                Box(contentAlignment = Alignment.Center) {
                    Row(horizontalArrangement = Arrangement.spacedBy(36.dp)) {
                        Avatar("Y", Color(0xFF4F46E5))
                        Avatar(theirName.firstOrNull()?.uppercase() ?: "?", Color(0xFFF59E0B))
                    }
                    Surface(shape = CircleShape, color = Color(0xFF16A34A), border = androidx.compose.foundation.BorderStroke(3.dp, Color.White)) {
                        Box(modifier = Modifier.size(34.dp), contentAlignment = Alignment.Center) {
                            Icon(Icons.Filled.Check, contentDescription = null, tint = Color.White, modifier = Modifier.size(20.dp))
                        }
                    }
                }
                Spacer(Modifier.height(18.dp))
                Text("You're connected", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.ExtraBold)
                Spacer(Modifier.height(10.dp))
                Surface(shape = RoundedCornerShape(50), color = EmeraldSoft) {
                    Row(
                        modifier = Modifier.padding(horizontal = 14.dp, vertical = 5.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        Icon(Icons.Filled.SwapHoriz, contentDescription = null, tint = Emerald, modifier = Modifier.size(16.dp))
                        Text("mutual", color = EmeraldInk, fontWeight = FontWeight.ExtraBold, style = MaterialTheme.typography.labelMedium)
                    }
                }
                Spacer(Modifier.height(14.dp))
                Text(
                    "You and $theirName are now in each other's circle — one tap did both. Their apps will show up in Discover.",
                    color = Slate,
                    style = MaterialTheme.typography.bodyMedium,
                    textAlign = TextAlign.Center,
                )
                Spacer(Modifier.height(20.dp))
                Surface(
                    shape = RoundedCornerShape(24.dp),
                    color = Emerald,
                    modifier = Modifier.fillMaxWidth().height(48.dp).clickable(onClick = onDone),
                ) {
                    Box(contentAlignment = Alignment.Center) {
                        Text("Done", color = Color.White, fontWeight = FontWeight.ExtraBold)
                    }
                }
            }
        }
    }
}

@Composable
private fun Avatar(initial: String, color: Color) {
    Box(
        modifier = Modifier
            .size(60.dp)
            .background(Color.White, CircleShape)
            .border(3.dp, Color(0xFFBBF7D0), CircleShape)
            .padding(4.dp)
            .background(color, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        Text(initial, color = Color.White, fontWeight = FontWeight.ExtraBold, style = MaterialTheme.typography.titleLarge)
    }
}

private fun shortNpub(npub: String): String =
    if (npub.length > 16) "${npub.take(10)}…${npub.takeLast(3)}" else npub
