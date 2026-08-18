package app.myco.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import app.myco.share.DeviceName

/**
 * The one-tap name choices: this phone's own name, and the pseudonymous
 * generated one.
 *
 * Both are chips rather than a "reset" button, because neither is a fallback —
 * they are two genuinely different answers to "what should people see". A phone
 * name is recognisable across a table and usually carries a real name; the
 * generated one is speakable and gives nothing away. Making the second option
 * cost a tap instead of typing is the whole point of this row.
 *
 * The chip matching the current value shows as selected, so the row also
 * answers "which one am I on".
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun NameSuggestions(ownNpub: String, current: String, onPick: (String) -> Unit) {
    val context = LocalContext.current
    val suggestions = DeviceName.suggestions(context, ownNpub)
    if (suggestions.isEmpty()) return
    FlowRow(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        suggestions.forEach { suggestion ->
            FilterChip(
                selected = suggestion.equals(current.trim(), ignoreCase = true),
                onClick = { onPick(suggestion) },
                label = { Text(suggestion, style = MaterialTheme.typography.labelLarge) },
            )
        }
    }
}
