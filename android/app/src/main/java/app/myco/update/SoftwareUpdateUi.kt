package app.myco.update

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

@Composable
fun rememberSoftwareUpdates(offlineOnly: Boolean): SoftwareUpdates {
    val context = LocalContext.current.applicationContext
    val updates = remember(context) { SoftwareUpdates.getInstance(context) }
    val state by updates.state.collectAsState()
    val owner = LocalLifecycleOwner.current
    // One scheduling entry point; toggle handlers only persist their setting.
    LaunchedEffect(updates, owner, offlineOnly, state.automatic) {
        updates.configureBackground()
        owner.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (true) {
                updates.check(offlineOnly)
                delay(60 * 60 * 1000L)
            }
        }
    }
    return updates
}

@Composable
fun SoftwareUpdateBanner(updates: SoftwareUpdates) {
    val state by updates.state.collectAsState()
    val tag = state.availableTag ?: return
    if (state.dismissedTag == tag) return
    Surface(modifier = Modifier.statusBarsPadding(), color = MaterialTheme.colorScheme.primaryContainer) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp)) {
            Text("Myco $tag is available", style = MaterialTheme.typography.titleSmall)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                TextButton(onClick = updates::openRelease) { Text("View update") }
                TextButton(onClick = updates::dismissBanner) { Text("Later") }
            }
        }
    }
}

@Composable
fun SoftwareUpdateSettings(updates: SoftwareUpdates, offlineOnly: Boolean) {
    val state by updates.state.collectAsState()
    val scope = rememberCoroutineScope()
    Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("Software updates", style = MaterialTheme.typography.titleMedium)
        Row(Modifier.fillMaxWidth()) {
            Column(Modifier.weight(1f)) {
                Text("Automatic update checks")
                Text("Check signed Zapstore releases daily, including in the background. Requires internet.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Switch(checked = state.automatic, onCheckedChange = updates::setAutomatic)
        }
        val message = when {
            offlineOnly -> "Update checks are paused in mesh-only mode"
            state.checking -> "Checking for updates…"
            state.availableTag != null -> "Myco ${state.availableTag} is available"
            else -> state.message
        }
        message?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            TextButton(enabled = !state.checking && !offlineOnly,
                onClick = { scope.launch { updates.check(offlineOnly, force = true) } }) { Text("Check now") }
            if (state.availableTag != null) {
                TextButton(onClick = updates::openRelease) { Text("View update") }
            }
        }
    }
}
