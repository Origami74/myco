package app.myco.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.GridView
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import app.myco.core.FileTransfer
import app.myco.core.SiteStatus

/**
 * The first version of the Myco-app handoff surface.
 *
 * Installed nsites are static web apps today. They do not yet publish a file
 * capability or expose a native file bridge, so this picker deliberately does
 * not claim that selecting an app will deliver the received bytes to it.
 */
@Composable
fun MycoAppPickerDialog(
    transfer: FileTransfer,
    apps: List<SiteStatus>,
    onDismiss: () -> Unit,
) {
    val readyApps = apps
        .filter { it.state == "ready" }
        .sortedBy { it.title.ifBlank { it.host }.lowercase() }

    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(usePlatformDefaultWidth = false),
    ) {
        Surface(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 24.dp),
            shape = RoundedCornerShape(28.dp),
            tonalElevation = 6.dp,
            color = MaterialTheme.colorScheme.surface,
        ) {
            Column(
                modifier = Modifier.padding(22.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Text(
                    "Open With Myco App",
                    style = MaterialTheme.typography.headlineSmall,
                    fontWeight = FontWeight.ExtraBold,
                )
                Text(
                    "Choose an installed app for ${transfer.name}. Myco apps are web apps; " +
                        "the current version does not yet expose a secure file handoff to them.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )

                if (readyApps.isEmpty()) {
                    Text(
                        "No ready Myco apps are installed yet.",
                        modifier = Modifier.padding(vertical = 16.dp),
                        fontWeight = FontWeight.Bold,
                    )
                } else {
                    LazyColumn(
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        items(readyApps, key = { it.host }) { app ->
                            MycoAppCapabilityRow(app)
                        }
                    }
                }

                Text(
                    "The file is still available in Downloads/Myco. File handoff support " +
                        "will be enabled only for apps that explicitly declare what they can open.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
                TextButton(
                    modifier = Modifier.fillMaxWidth(),
                    onClick = onDismiss,
                ) {
                    Text("Done")
                }
            }
        }
    }
}

@Composable
private fun MycoAppCapabilityRow(app: SiteStatus) {
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(18.dp),
        color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.55f),
    ) {
        Row(
            modifier = Modifier.padding(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                modifier = Modifier
                    .size(44.dp)
                    .clip(CircleShape)
                    .background(MaterialTheme.colorScheme.primaryContainer),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    Icons.Filled.GridView,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.primary,
                )
            }
            Column(modifier = Modifier.padding(start = 12.dp).weight(1f)) {
                Text(
                    app.title.ifBlank { app.host },
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    fontWeight = FontWeight.Bold,
                )
                Spacer(Modifier.size(2.dp))
                Text(
                    "File handoff: not declared",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
    }
}
