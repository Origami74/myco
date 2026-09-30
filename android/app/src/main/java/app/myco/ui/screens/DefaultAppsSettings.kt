package app.myco.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.myco.AppMonogram
import app.myco.archetypeLabel
import app.myco.core.AppCoreClient
import app.myco.core.AppState
import app.myco.core.IntentArchetype
import app.myco.core.NativeActions
import app.myco.ui.GroupLabel
import app.myco.ui.SectionCard

/**
 * Settings › Default apps: which app opens each role (NAP-INTENT archetype)
 * when another app asks — like an OS's default apps. One row per role
 * something installed can handle; "Ask every time" clears the default, and
 * the next request with more than one candidate shows the chooser.
 *
 * Only the user sets these. A napplet can ask for a role to be opened, never
 * for which app is its default.
 */
@Composable
internal fun DefaultAppsSettings(state: AppState, client: AppCoreClient, onBack: () -> Unit) {
    var editing by remember { mutableStateOf<IntentArchetype?>(null) }
    SettingsColumn {
        SubHeader("Default apps", onBack)
        Text(
            "When one app asks to open something — a profile, a note, a site — " +
                "this is the app that opens it.",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(Modifier.size(4.dp))
        if (state.intentHandlers.isEmpty()) {
            Text(
                "None of your apps can be opened this way yet.",
                style = MaterialTheme.typography.bodyMedium,
            )
        } else {
            GroupLabel("OPENS")
            SectionCard {
                state.intentHandlers.forEachIndexed { i, row ->
                    if (i > 0) RowDivider()
                    val current = row.candidates.firstOrNull { it.key == row.defaultKey }
                    SettingRow(
                        icon = null,
                        title = archetypeLabel(row.archetype).replaceFirstChar { it.uppercase() },
                        subtitle = current?.title ?: if (row.candidates.size == 1) {
                            "${row.candidates[0].title} (the only one)"
                        } else {
                            "Ask every time"
                        },
                        onClick = { editing = row },
                    )
                }
            }
        }
    }

    editing?.let { row ->
        AlertDialog(
            onDismissRequest = { editing = null },
            title = { Text("Open ${archetypeLabel(row.archetype)} with") },
            text = {
                Column {
                    val pick = { key: String? ->
                        editing = null
                        client.dispatch(NativeActions.setIntentDefault(row.archetype, key))
                    }
                    ChoiceRow(selected = row.defaultKey.isEmpty(), onClick = { pick(null) }) {
                        Text("Ask every time", style = MaterialTheme.typography.bodyLarge)
                    }
                    for (c in row.candidates) {
                        ChoiceRow(selected = c.key == row.defaultKey, onClick = { pick(c.key) }) {
                            AppMonogram(c.title, c.pointer, size = 28.dp)
                            Spacer(Modifier.width(12.dp))
                            Text(c.title, style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.Normal)
                        }
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = { editing = null }) { Text("Close") }
            },
        )
    }
}

@Composable
private fun ChoiceRow(selected: Boolean, onClick: () -> Unit, content: @Composable () -> Unit) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier.fillMaxWidth().clickable(onClick = onClick).padding(vertical = 4.dp),
    ) {
        RadioButton(selected = selected, onClick = onClick)
        content()
    }
}
