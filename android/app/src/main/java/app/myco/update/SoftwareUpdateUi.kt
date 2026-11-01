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
import app.myco.LocalSystemAsker
import app.myco.onboarding.SystemAsk
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

/**
 * Install the carried update: Android's "install unknown apps" switch first
 * (explained, then its settings page), then the install, which Android
 * confirms with the user.
 */
@Composable
private fun rememberInstall(updates: SoftwareUpdates): () -> Unit {
    val context = LocalContext.current
    val asker = LocalSystemAsker.current
    val scope = rememberCoroutineScope()
    return {
        if (SystemAsk.InstallUpdates.granted(context)) scope.launch { updates.install() }
        else asker?.explain(SystemAsk.InstallUpdates)
    }
}

@Composable
fun SoftwareUpdateBanner(updates: SoftwareUpdates) {
    val state by updates.state.collectAsState()
    val tag = state.availableTag ?: return
    if (state.dismissedTag == tag) return
    val install = rememberInstall(updates)
    Surface(modifier = Modifier.statusBarsPadding(), color = MaterialTheme.colorScheme.primaryContainer) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp)) {
            Text(
                if (state.installable) "Myco $tag is ready to install" else "Myco $tag is available",
                style = MaterialTheme.typography.titleSmall,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (state.installable) {
                    TextButton(enabled = !state.installing, onClick = install) {
                        Text(if (state.installing) "Installing…" else "Install")
                    }
                } else {
                    TextButton(onClick = updates::openRelease) { Text("View update") }
                }
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
                Text(if (offlineOnly) "Mesh-only: asks your Circle, never the internet."
                    else "Checks signed releases on Zapstore and your Circle daily, including in the background.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Switch(checked = state.automatic, onCheckedChange = updates::setAutomatic)
        }
        val install = rememberInstall(updates)
        val message = when {
            state.checking -> "Checking for updates…"
            state.installing -> "Installing…"
            state.message != null && state.installable -> state.message
            state.installable -> "Myco ${state.availableTag} is ready to install"
            state.availableTag != null -> "Myco ${state.availableTag} is available"
            else -> state.message
        }
        message?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
        state.carried?.let { carried ->
            // What this phone hands its Circle, update or not.
            Text(
                "Carrying Myco ${carried.version} for your Circle (${carried.size / 1_048_576} MB)",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            TextButton(enabled = !state.checking,
                onClick = { scope.launch { updates.check(offlineOnly, force = true) } }) { Text("Check now") }
            if (state.installable) {
                TextButton(enabled = !state.installing, onClick = install) { Text("Install") }
            } else if (state.availableTag != null) {
                TextButton(onClick = updates::openRelease) { Text("View update") }
            }
        }
    }
}
