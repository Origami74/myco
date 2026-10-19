package app.myco

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.util.Log
import android.widget.Toast
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
    asker: String = "This app",
) {
    var always by remember(chooser.token) { mutableStateOf(false) }
    ModalBottomSheet(onDismissRequest = onCancel) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 24.dp).padding(bottom = 24.dp)) {
            Text("Open with", style = MaterialTheme.typography.titleLarge)
            Spacer(Modifier.height(4.dp))
            Text(
                "$asker wants to open a ${archetypeLabel(chooser.archetype)}. Pick which app does it.",
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

/**
 * A `napplet.link` nothing installed here opens (`link-refused`): the role it
 * asked for, and the page to offer instead. [webUrl] is empty when there is
 * none — a link that did not parse.
 */
data class NappletLinkRefusal(val archetype: String, val webUrl: String) {
    companion object {
        fun parse(obj: JSONObject): NappletLinkRefusal =
            NappletLinkRefusal(obj.optString("archetype"), obj.optString("webUrl"))
    }
}

/**
 * Says that no app here opens the link, and offers its web page in a
 * browser. Never anything that could install: getting an app that handles
 * the role is the user's own business, from the Apps screen.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NappletLinkRefusedSheet(
    refusal: NappletLinkRefusal,
    onOpenInBrowser: () -> Unit,
    onDismiss: () -> Unit,
) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 24.dp).padding(bottom = 24.dp)) {
            val role = refusal.archetype.ifEmpty { null }?.let { archetypeLabel(it) }
            Text(
                if (role != null) "No app here opens $role" else "Myco can't open this link",
                style = MaterialTheme.typography.titleLarge,
            )
            Spacer(Modifier.height(4.dp))
            Text(
                if (refusal.webUrl.isNotEmpty()) {
                    "None of your apps can open this link. You can open it in your browser instead."
                } else {
                    "This link isn't one Myco understands."
                },
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(12.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                TextButton(onClick = onDismiss) { Text("Dismiss") }
                if (refusal.webUrl.isNotEmpty()) {
                    TextButton(onClick = onOpenInBrowser) { Text("Open in browser") }
                }
            }
        }
    }
}

/** `napplet.link` links: which intents carry one, and the browser fallback. */
object NappletLinks {
    const val HOST = "napplet.link"

    /**
     * Whether [uri] is for [MainActivity]'s napplet-link path rather than its
     * `myco:` one: `https://napplet.link/…`, or a `napplet:` URI that is not a
     * napplet pointer (`napplet:naddr1…` is install review, as before). Rust
     * has the last word on the shape.
     */
    fun isNappletLink(uri: Uri): Boolean = when (uri.scheme?.lowercase()) {
        "https" -> uri.host.equals(HOST, ignoreCase = true)
        "napplet" -> {
            val rest = uri.schemeSpecificPart.orEmpty().trimStart('/')
            !rest.startsWith("naddr1", ignoreCase = true)
        }
        else -> false
    }

    /**
     * Open [url] in a browser, never in Myco. Myco claims `napplet.link`, so a
     * plain VIEW could come straight back here; the selector asks for an app
     * in the browser category, which Myco is not.
     */
    fun openInBrowser(context: Context, url: String) {
        val view = Intent(Intent.ACTION_VIEW, Uri.parse(url))
            .addCategory(Intent.CATEGORY_BROWSABLE)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        view.selector = Intent.makeMainSelectorActivity(Intent.ACTION_MAIN, Intent.CATEGORY_APP_BROWSER)
        try {
            context.startActivity(view)
        } catch (e: ActivityNotFoundException) {
            Log.w("MycoNappletLink", "no browser for $url", e)
            Toast.makeText(context, "No browser to open this link", Toast.LENGTH_SHORT).show()
        }
    }
}
