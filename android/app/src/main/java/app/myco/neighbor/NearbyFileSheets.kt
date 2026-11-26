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
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.NativeActions
import app.myco.share.ExternalShare
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Files shared into Myco from another app, about to go to the people in
 * Nearby. Says plainly who sees them: anyone in reach, unencrypted.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NearbyShareSheet(uris: List<Uri>, state: AppState, client: AppCoreClient, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val nearby = state.neighborhoods.nearby
    val items = remember(uris) { uris.map { ExternalShare.describe(context, it) } }
    var sending by remember { mutableStateOf(false) }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(
            Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(start = 20.dp, end = 20.dp, bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text("Share with people nearby", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
            for (item in items.take(5)) {
                Text(
                    item.name + (item.size?.let { " · " + Formatter.formatShortFileSize(context, it) } ?: ""),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            if (items.size > 5) Text("and ${items.size - 5} more", color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.height(4.dp))
            if (!nearby.on) {
                Text(
                    "Nearby is off. Turn it on to share with phones around you.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Button(onClick = { client.dispatch(NativeActions.setNearby(true)) }, modifier = Modifier.fillMaxWidth()) {
                    Text("Turn on Nearby")
                }
            } else {
                Text(
                    "Anyone in reach can see and save it — " +
                        (if (nearby.peers > 0) "${nearby.peers} phone${if (nearby.peers == 1) "" else "s"} now" else "nobody yet") +
                        ". It isn't encrypted, and it's forgotten when nobody is in reach.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
                Button(
                    enabled = !sending,
                    onClick = {
                        sending = true
                        scope.launch {
                            val (sent, error) = withContext(Dispatchers.IO) { NearbyFiles.share(context, client, uris) }
                            val message = when {
                                error == null -> if (sent == 1) "Shared with Nearby" else "$sent files shared with Nearby"
                                sent > 0 -> "$sent shared; one wasn't: $error"
                                else -> "Not shared: $error"
                            }
                            Toast.makeText(context, message, Toast.LENGTH_LONG).show()
                            onDismiss()
                        }
                    },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text(if (sending) "Sharing…" else "Share") }
            }
            TextButton(onClick = onDismiss, modifier = Modifier.align(Alignment.CenterHorizontally)) { Text("Cancel") }
        }
    }
}

/** The files in Nearby, newest first, each with Save. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NearbyFilesSheet(client: AppCoreClient, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val files by NearbyFiles.files
    var saving by remember { mutableStateOf<NearbyFile?>(null) }
    // Fresh while the sheet is open.
    LaunchedEffect(Unit) {
        while (true) {
            withContext(Dispatchers.IO) { NearbyFiles.refresh(client) }
            delay(3_000)
        }
    }
    val save = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("*/*")) { target ->
        val file = saving
        saving = null
        if (target == null || file == null) return@rememberLauncherForActivityResult
        scope.launch {
            val ok = withContext(Dispatchers.IO) { NearbyFiles.save(context, client, file, target) }
            Toast.makeText(
                context,
                if (ok) "Saved ${file.name}" else "Couldn't get ${file.name} — is its phone still in reach?",
                Toast.LENGTH_LONG,
            ).show()
        }
    }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(
            Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(start = 20.dp, end = 20.dp, bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text("Files nearby", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
            if (files.isEmpty()) {
                Text(
                    "Nothing yet. Share a file from any app and pick Myco to send it to the people around you.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            for (file in files) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(file.name, maxLines = 1, overflow = TextOverflow.Ellipsis, fontWeight = FontWeight.SemiBold)
                        Text(
                            Formatter.formatShortFileSize(context, file.size) + if (file.mine) " · from this phone" else "",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    TextButton(enabled = saving == null, onClick = {
                        saving = file
                        save.launch(file.name)
                    }) { Text("Save") }
                }
            }
        }
    }
}
