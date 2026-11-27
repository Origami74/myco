package app.myco.neighbor

import android.net.Uri
import android.text.format.Formatter
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.ModalBottomSheetProperties
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.SecureFlagPolicy
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.NativeActions
import app.myco.nfc.PairPresent
import app.myco.share.ExternalShare
import app.myco.share.NsiteShare
import app.myco.ui.screens.QrCodeCard
import app.myco.ui.screens.ShareLinkRow
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Files shared into Myco from another app, for the people around you.
 *
 * Mesh off or Nearby off: offers to turn it on. Then Share seals each file
 * and keeps it for the phones in reach, telling nobody. Then each file's
 * code — QR, NFC tap, or link — for whoever should get it.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NearbyShareSheet(
    uris: List<Uri>,
    state: AppState,
    client: AppCoreClient,
    meshEnabled: Boolean,
    onMeshToggle: (Boolean) -> Unit,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val nearby = state.neighborhoods.nearby
    // A content-provider query each: off the main thread.
    val items by androidx.compose.runtime.produceState(emptyList<app.myco.share.SharedItem>(), uris) {
        value = withContext(Dispatchers.IO) { uris.map { ExternalShare.describe(context, it) } }
    }
    var sending by remember { mutableStateOf(false) }
    var sent by remember { mutableStateOf<List<NearbyFiles.Sent>?>(null) }
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        // The codes carry each file's key: keep them out of screenshots and Recents.
        properties = ModalBottomSheetProperties(securePolicy = SecureFlagPolicy.SecureOn),
    ) {
        Column(
            Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(start = 20.dp, end = 20.dp, bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            val done = sent
            if (done != null) {
                SentCodes(done, onDismiss)
                return@Column
            }
            Text("Share with someone nearby", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
            for (item in items.take(5)) {
                Text(
                    item.name + (item.size?.let { " · " + Formatter.formatShortFileSize(context, it) } ?: ""),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            if (items.size > 5) Text("and ${items.size - 5} more", color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.height(4.dp))
            when {
                !meshEnabled -> {
                    Explain("The mesh is off. Turn it on to share with phones around you.")
                    Button(onClick = { onMeshToggle(true) }, modifier = Modifier.fillMaxWidth()) { Text("Turn on the mesh") }
                }
                !nearby.on -> {
                    Explain("Nearby is off. Turn it on to share with phones around you.")
                    Button(onClick = { client.dispatch(NativeActions.setNearby(true)) }, modifier = Modifier.fillMaxWidth()) {
                        Text("Turn on Nearby")
                    }
                }
                else -> {
                    Explain(
                        "It's encrypted and nobody is told. Next you'll get a code to show the person it's for: " +
                            "their phone fetches it from yours while you're in reach.",
                    )
                    Button(
                        enabled = !sending,
                        onClick = {
                            sending = true
                            scope.launch {
                                sent = withContext(Dispatchers.IO) { NearbyFiles.share(context, client, uris) }
                                sending = false
                            }
                        },
                        modifier = Modifier.fillMaxWidth(),
                    ) { Text(if (sending) "Encrypting…" else "Share") }
                }
            }
            TextButton(onClick = onDismiss) { Text("Cancel") }
        }
    }
}

/** Each file's code, one at a time: QR, NFC tap while shown, and the link. */
@Composable
private fun SentCodes(sent: List<NearbyFiles.Sent>, onDone: () -> Unit) {
    val shown = sent.filter { it.link != null }
    val failed = sent.filter { it.link == null }
    var index by remember { mutableIntStateOf(0) }
    val current = shown.getOrNull(index)
    val link = current?.link
    // NFC: a tap from the other phone reads this file's link while it's shown.
    DisposableEffect(link) {
        if (link != null) PairPresent.beginRaw(link)
        onDispose { PairPresent.stop() }
    }
    if (current == null || link == null) {
        Text("Not shared", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
        for (f in failed) Explain("${f.name}: ${f.error}")
        Button(onClick = onDone, modifier = Modifier.fillMaxWidth()) { Text("Close") }
        return
    }
    Text("Show this to who it's for", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
    val qr = remember(link) { NsiteShare.qrBitmap(link) }
    QrCodeCard(qr, contentDescription = "Code for ${current.name}", size = 230.dp)
    Text(current.name, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
    Explain(
        "They scan it with Myco or tap phones. The code holds the file's name and key: " +
            "anyone with it can open the file while you're nearby.",
    )
    ShareLinkRow(uri = link, title = current.name, sensitive = true)
    if (shown.size > 1) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            TextButton(enabled = index > 0, onClick = { index-- }) { Text("Previous") }
            Text("${index + 1} of ${shown.size}", color = MaterialTheme.colorScheme.onSurfaceVariant)
            TextButton(enabled = index < shown.size - 1, onClick = { index++ }) { Text("Next") }
        }
    }
    for (f in failed) Explain("Not shared — ${f.name}: ${f.error}")
    Button(onClick = onDone, modifier = Modifier.fillMaxWidth()) { Text("Done") }
}

/** A file someone showed you the code for: save it from their phone. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NearbyReceiveSheet(
    receive: NearbyFiles.Receive,
    state: AppState,
    client: AppCoreClient,
    meshEnabled: Boolean,
    onMeshToggle: (Boolean) -> Unit,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var saving by remember { mutableStateOf(false) }
    var problem by remember { mutableStateOf<String?>(null) }
    val save = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument(receive.mime.ifEmpty { "*/*" })) { target ->
        if (target == null) return@rememberLauncherForActivityResult
        saving = true
        problem = null
        scope.launch {
            val error = withContext(Dispatchers.IO) { NearbyFiles.save(context, client, receive, target) }
            saving = false
            if (error == null) {
                Toast.makeText(context, "Saved ${receive.name}", Toast.LENGTH_SHORT).show()
                onDismiss()
            } else {
                problem = error.replaceFirstChar { it.uppercase() } + "."
            }
        }
    }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(
            Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text("A file for you", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
            Text(receive.name, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center)
            if (receive.size > 0) {
                Text(Formatter.formatShortFileSize(context, receive.size), color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            when {
                !meshEnabled -> {
                    Explain("The mesh is off. Turn it on to get the file from the phone that shared it.")
                    Button(onClick = { onMeshToggle(true) }, modifier = Modifier.fillMaxWidth()) { Text("Turn on the mesh") }
                }
                !state.neighborhoods.nearby.on -> {
                    Explain("Nearby is off. Turn it on to get the file from the phone that shared it.")
                    Button(onClick = { client.dispatch(NativeActions.setNearby(true)) }, modifier = Modifier.fillMaxWidth()) {
                        Text("Turn on Nearby")
                    }
                }
                NearbyFiles.checking.value == receive.link -> {
                    Explain("Looking for it on the phones around you…")
                    CircularProgressIndicator(Modifier.height(24.dp))
                }
                else -> {
                    Explain("It comes from the phone that shared it, so stay in reach until it's saved.")
                    problem?.let { Text(it, color = MaterialTheme.colorScheme.error, textAlign = TextAlign.Center) }
                    Button(enabled = !saving, onClick = { save.launch(receive.name) }, modifier = Modifier.fillMaxWidth()) {
                        if (saving) {
                            CircularProgressIndicator(Modifier.padding(end = 8.dp).height(18.dp))
                            Text("Getting it…")
                        } else {
                            Text("Save")
                        }
                    }
                }
            }
            TextButton(onClick = onDismiss) { Text("Cancel") }
        }
    }
}

@Composable
private fun Explain(text: String) {
    Text(
        text,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        style = MaterialTheme.typography.bodySmall,
        textAlign = TextAlign.Center,
    )
}
