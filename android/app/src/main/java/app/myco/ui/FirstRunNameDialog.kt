package app.myco.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import app.myco.share.DeviceName

/**
 * The one thing first run asks: what nearby devices should call you.
 *
 * It exists because the default is now this handset's own name, which is
 * usually the most recognisable option and quite often carries a real one
 * ("Arjen's S21"). Taking that silently would publish it to everyone you pair
 * with before you knew it was the name being sent. Showing it once, with the
 * pseudonymous generated name one tap away, is the whole point.
 *
 * Not dismissable by tapping outside: it is one decision on one screen, and a
 * stray tap that skipped it would leave the real name in place by accident —
 * exactly the outcome this is here to prevent.
 */
@Composable
fun FirstRunNameDialog(ownNpub: String, onDone: (String) -> Unit) {
    val context = LocalContext.current
    val suggestions = remember(ownNpub) { DeviceName.suggestions(context, ownNpub) }
    var name by remember(ownNpub) { mutableStateOf(suggestions.firstOrNull().orEmpty()) }

    AlertDialog(
        onDismissRequest = {},
        title = { Text("What should people call you?") },
        text = {
            Column {
                Text(
                    "Nearby devices see this name when you pair, so they can tell it's " +
                        "you. Your phone's own name is filled in — tap the other one if " +
                        "you'd rather not share it, or type anything you like.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodyMedium,
                )
                Spacer(Modifier.height(14.dp))
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it.take(DeviceName.MAX_LENGTH) },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(10.dp))
                NameSuggestions(ownNpub, name) { name = it }
            }
        },
        confirmButton = {
            TextButton(
                enabled = name.isNotBlank(),
                onClick = { onDone(name.trim()) },
            ) { Text("Continue") }
        },
    )
}
