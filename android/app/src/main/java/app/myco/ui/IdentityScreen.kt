package app.myco.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import app.myco.core.AppState

/**
 * P0 developer screen: shows this device's identity (npub + derived forms) and
 * the embedded FIPS node status. This is the "does the core come up?" view; the
 * real Library/Pair/Discover/Settings UI is later.
 */
@Composable
fun IdentityScreen(state: AppState) {
    Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text("Myco", style = MaterialTheme.typography.headlineMedium)

            if (state.error.isNotEmpty()) {
                Text("Error: ${state.error}", color = MaterialTheme.colorScheme.error)
            }

            Field("Device npub", state.ownNpub)
            Field("Pubkey (hex)", state.ownPubkeyHex)
            Field("node_addr", state.nodeAddrHex)
            Field("FIPS host", state.fipsAddr)
            Field("Node", state.nodeStatus)
            Field("Version / rev", "${state.appVersion} / rev ${state.rev}")
        }
    }
}

@Composable
private fun Field(label: String, value: String) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(label, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary)
        Text(
            value.ifEmpty { "—" },
            style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace),
        )
    }
}
