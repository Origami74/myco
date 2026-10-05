package app.myco.ui.onboarding

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.BatteryFull
import androidx.compose.material.icons.outlined.Bluetooth
import androidx.compose.material.icons.outlined.Notifications
import androidx.compose.material.icons.outlined.PhotoCamera
import androidx.compose.material.icons.outlined.WifiTethering
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import app.myco.onboarding.SystemAsk

/**
 * A tap on an explanation's button that asks Android — the only thing
 * `SystemAsker.launch` takes. Made only here, by [AskButton], so every
 * prompt Myco shows follows a tap on a card that said what it is for.
 */
sealed interface Confirmed {
    val ask: SystemAsk
}

private class Tapped(override val ask: SystemAsk) : Confirmed

enum class AskButtonStyle { Primary, Quiet }

/**
 * The button that asks Android for [ask]. Used by [ExplainCard], and by the
 * setup popup's refusal cards for "Try again" (those say what was refused
 * and what it is for).
 */
@Composable
fun AskButton(
    ask: SystemAsk,
    text: String,
    onConfirm: (Confirmed) -> Unit,
    style: AskButtonStyle = AskButtonStyle.Primary,
    modifier: Modifier = Modifier.fillMaxWidth(),
) {
    when (style) {
        AskButtonStyle.Primary -> PrimaryButton(text) { onConfirm(Tapped(ask)) }
        AskButtonStyle.Quiet -> QuietButton(text, modifier) { onConfirm(Tapped(ask)) }
    }
}

/**
 * The explanation shown before every Android prompt, from the registry
 * ([SystemAsk.explanation]): what Android is about to ask, what Myco does
 * with it, "Continue" (the tap that asks) and "Not now". The setup popup's
 * "Nearby devices" and "Mesh connection" cards are this; so is the explain
 * dialog every other screen asks through ([ExplainAskDialog]).
 */
@Composable
fun ExplainCard(ask: SystemAsk, onConfirm: (Confirmed) -> Unit, onNotNow: () -> Unit) {
    val e = ask.explanation
    IconBadge(warn = false) { AskIcon(ask, MaterialTheme.colorScheme.primary) }
    Title(e.title)
    Body(e.body)
    Note(e.note)
    Spacer(Modifier.height(28.dp))
    AskButton(ask, "Continue", onConfirm)
    Spacer(Modifier.height(10.dp))
    NeutralButton("Not now", onNotNow)
}

/**
 * [ExplainCard] on its own card, for the prompts asked outside the setup
 * popup: Settings › Permissions, the hotspot, the camera, Bluetooth.
 * Back or an outside tap is "Not now" — it never asks.
 */
@Composable
fun ExplainAskDialog(ask: SystemAsk, onConfirm: (Confirmed) -> Unit, onDismiss: () -> Unit) {
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(
            shape = RoundedCornerShape(28.dp),
            color = MaterialTheme.colorScheme.surface,
            border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
            modifier = Modifier.padding(horizontal = 20.dp).widthIn(max = 440.dp).fillMaxWidth(),
        ) {
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                modifier = Modifier.verticalScroll(rememberScrollState()).padding(24.dp),
            ) {
                ExplainCard(ask, onConfirm, onDismiss)
            }
        }
    }
}

/** [ask]'s icon on its explanation. */
@Composable
private fun AskIcon(ask: SystemAsk, color: Color) {
    val glyph = Modifier.size(34.dp)
    when (ask) {
        SystemAsk.Nearby -> PhonesIcon(color)
        SystemAsk.Vpn -> MeshIcon(color)
        SystemAsk.Notifications -> Icon(Icons.Outlined.Notifications, null, tint = color, modifier = glyph)
        SystemAsk.Battery -> Icon(Icons.Outlined.BatteryFull, null, tint = color, modifier = glyph)
        SystemAsk.Camera -> Icon(Icons.Outlined.PhotoCamera, null, tint = color, modifier = glyph)
        SystemAsk.Hotspot -> Icon(Icons.Outlined.WifiTethering, null, tint = color, modifier = glyph)
        SystemAsk.BluetoothOn -> Icon(Icons.Outlined.Bluetooth, null, tint = color, modifier = glyph)
    }
}
