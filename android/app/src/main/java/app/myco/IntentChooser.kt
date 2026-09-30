package app.myco

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Checkbox
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.myco.ui.theme.tileColorFor
import org.json.JSONObject

/** One app the "open with…" sheet offers. [pointer] is empty for Myco itself. */
data class IntentChoice(val key: String, val title: String, val pointer: String)

/**
 * An "open with…" question from a NAP-INTENT invoke (`choose-intent-handler`).
 * The keys go back to Rust with the answer; the napplet that asked never
 * sees them.
 */
data class IntentChooser(
    val token: String,
    val archetype: String,
    val action: String,
    val choices: List<IntentChoice>,
) {
    companion object {
        fun parse(obj: JSONObject): IntentChooser? {
            val token = obj.optString("token").ifEmpty { return null }
            val arr = obj.optJSONArray("candidates") ?: return null
            val choices = buildList {
                for (i in 0 until arr.length()) {
                    val c = arr.optJSONObject(i) ?: continue
                    add(IntentChoice(c.optString("key"), c.optString("title"), c.optString("pointer")))
                }
            }
            if (choices.isEmpty()) return null
            return IntentChooser(token, obj.optString("archetype"), obj.optString("action"), choices)
        }
    }
}

/**
 * Another app a napplet's intent resolved to, waiting on the user's say-so:
 * a napplet handler ([pointer] + delivery [token]) or Myco's nsite opener
 * ([nsiteHost]).
 */
data class IntentOpen(val title: String, val pointer: String, val token: String, val nsiteHost: String)

/** A role slug in words: `emoji-list` → "emoji list". */
fun archetypeLabel(archetype: String): String = archetype.replace('-', ' ')

/**
 * An app's round monogram — the Apps grid's letter tile, in its colour (keyed
 * on the same pointer), or Myco's for the built-in handler.
 */
@Composable
fun AppMonogram(title: String, pointer: String, size: Dp = 40.dp) {
    val background = if (pointer.isEmpty()) MaterialTheme.colorScheme.primary else tileColorFor(pointer)
    Box(
        contentAlignment = Alignment.Center,
        modifier = Modifier.size(size).clip(CircleShape).background(background),
    ) {
        Text(
            title.take(1).uppercase().ifEmpty { "?" },
            color = Color.White,
            fontWeight = FontWeight.SemiBold,
            fontSize = 16.sp,
        )
    }
}

/**
 * The "open with…" sheet, over the napplet that asked. Picking an app answers
 * it; "Always use this" also makes that app the default for the role
 * (changeable in Settings › Default apps). Dismissing it is a cancel.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun IntentChooserSheet(
    chooser: IntentChooser,
    onPick: (key: String, always: Boolean) -> Unit,
    onCancel: () -> Unit,
) {
    var always by remember(chooser.token) { mutableStateOf(false) }
    ModalBottomSheet(onDismissRequest = onCancel) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 24.dp).padding(bottom = 24.dp)) {
            Text("Open with", style = MaterialTheme.typography.titleLarge)
            Spacer(Modifier.height(4.dp))
            Text(
                "This app wants to open a ${archetypeLabel(chooser.archetype)}. Pick which app does it.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(12.dp))
            for (choice in chooser.choices) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .clip(MaterialTheme.shapes.medium)
                        .clickable { onPick(choice.key, always) }
                        .padding(vertical = 10.dp),
                ) {
                    AppMonogram(choice.title, choice.pointer)
                    Spacer(Modifier.width(16.dp))
                    Text(choice.title, style = MaterialTheme.typography.bodyLarge)
                }
            }
            Spacer(Modifier.height(8.dp))
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.fillMaxWidth().clickable { always = !always },
            ) {
                Checkbox(checked = always, onCheckedChange = { always = it })
                Text("Always use this", style = MaterialTheme.typography.bodyMedium)
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                TextButton(onClick = onCancel) { Text("Cancel") }
            }
        }
    }
}
