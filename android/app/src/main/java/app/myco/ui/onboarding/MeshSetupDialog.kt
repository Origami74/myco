package app.myco.ui.onboarding

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Badge
import androidx.compose.material.icons.outlined.GppMaybe
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.window.DialogWindowProvider
import app.myco.onboarding.MeshOutcome
import app.myco.onboarding.MeshSetup
import app.myco.onboarding.Segment
import app.myco.onboarding.SegmentState
import app.myco.onboarding.SetupPlan
import app.myco.share.DeviceName
import app.myco.onboarding.SetupStep

/** A button in the setup popup. The Activity turns each into its side effect. */
enum class SetupAction {
    Yes,
    NoThanks,
    RetryNearby,
    OpenAppSettings,
    ContinueAfterNearby,
    RetryVpn,
    NotNow,
    OpenVpnSettings,
    ContinueWithout,
    KeepName,
}

/**
 * The setup popup: a card over the dimmed app with a segmented progress bar
 * (Install Myco · Nearby phones · Connection · Name, as [plan] has them).
 *
 * Stateless — the Activity owns [step], because Android's permission and VPN
 * results land there, and it has to survive the Activity being recreated
 * behind a system prompt. Not dismissable by back or an outside tap: every
 * card has its own way out, and a stray tap must not decide for the user.
 *
 * @param outcome how the mesh steps behind the current one went.
 * @param nearbyBlocked Android refused the nearby permissions without asking,
 *   so "Try again" becomes "Open app settings".
 * @param name the name the Name step offers to keep.
 * @param onSaveName a name typed into "Change it".
 */
@Composable
fun MeshSetupDialog(
    step: SetupStep,
    plan: SetupPlan,
    outcome: MeshOutcome,
    nearbyBlocked: Boolean,
    name: String,
    onAction: (SetupAction) -> Unit,
    onSaveName: (String) -> Unit,
) {
    // "Change it" is a modal over the card: the card's own UI state.
    var editingName by rememberSaveable { mutableStateOf(false) }
    Dialog(
        onDismissRequest = {},
        properties = DialogProperties(
            dismissOnBackPress = false,
            dismissOnClickOutside = false,
            usePlatformDefaultWidth = false,
        ),
    ) {
        // The platform's dialog dim is lighter than the design's; the card
        // should read as the only thing on screen.
        val window = (LocalView.current.parent as? DialogWindowProvider)?.window
        SideEffect { window?.setDimAmount(0.72f) }

        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            modifier = Modifier
                .padding(horizontal = 20.dp)
                .widthIn(max = 440.dp)
                .fillMaxWidth(),
        ) {
            SetupCard(step, plan, MeshSetup.segments(step, plan, outcome)) {
                StepContent(step, nearbyBlocked, name, onAction, onChangeName = { editingName = true })
            }
            footnote(step, nearbyBlocked)?.let {
                Spacer(Modifier.height(18.dp))
                // On the scrim, not the card: dark in both themes.
                Text(
                    it,
                    color = Color.White.copy(alpha = 0.85f),
                    fontSize = 11.5.sp,
                    textAlign = TextAlign.Center,
                )
            }
        }
        if (editingName && step == SetupStep.Name) {
            ChangeNameDialog(
                initial = name,
                onSave = {
                    editingName = false
                    onSaveName(it)
                },
                onCancel = { editingName = false },
            )
        }
    }
}

private fun footnote(step: SetupStep, nearbyBlocked: Boolean): String? = when (step) {
    SetupStep.VpnRefused -> "“Try again” shows Android’s VPN prompt again"
    SetupStep.NearbyRefused ->
        if (nearbyBlocked) "Android won’t ask again — allow Nearby devices in Myco’s app info" else null
    else -> null
}

@Composable
private fun StepContent(
    step: SetupStep,
    nearbyBlocked: Boolean,
    name: String,
    onAction: (SetupAction) -> Unit,
    onChangeName: () -> Unit,
) {
    when (step) {
        // Deliberately short: the title and one line. What the VPN is and
        // isn't is said on the cards that need it, after a refusal.
        SetupStep.EnableMesh -> {
            IconBadge(warn = false) { PhonesIcon(MaterialTheme.colorScheme.primary) }
            Title("Enable mesh?")
            Body("Mesh needs a few permissions in order to work.")
            Spacer(Modifier.height(28.dp))
            PrimaryButton("Yes, enable") { onAction(SetupAction.Yes) }
            Spacer(Modifier.height(10.dp))
            NeutralButton("No thanks") { onAction(SetupAction.NoThanks) }
        }
        SetupStep.AskingNearby -> Waiting(
            icon = { PhonesIcon(MaterialTheme.colorScheme.primary) },
            title = "Allow nearby devices",
            body = "Android is asking now.",
        )
        SetupStep.NearbyRefused -> {
            IconBadge(warn = true) { PhonesIcon(MaterialTheme.colorScheme.tertiary) }
            Title("Nearby phones are off")
            Body(
                "Without nearby devices, Myco can’t look for phones around you over " +
                    "Bluetooth and Wi-Fi.",
            )
            Note("Phones on the same Wi-Fi network can still connect. Apps on this phone still open.")
            Spacer(Modifier.height(24.dp))
            if (nearbyBlocked) {
                PrimaryButton("Open app settings") { onAction(SetupAction.OpenAppSettings) }
            } else {
                PrimaryButton("Try again") { onAction(SetupAction.RetryNearby) }
            }
            Spacer(Modifier.height(6.dp))
            QuietButton("Continue") { onAction(SetupAction.ContinueAfterNearby) }
        }
        SetupStep.AskingVpn -> Waiting(
            icon = { MeshIcon(MaterialTheme.colorScheme.primary) },
            title = "Allow the VPN",
            body = "Android is asking now.",
        )
        SetupStep.Connecting -> Waiting(
            icon = { MeshIcon(MaterialTheme.colorScheme.primary) },
            title = "Starting the mesh…",
            body = "This takes a few seconds.",
        )
        SetupStep.VpnRefused -> {
            IconBadge(warn = true) { MeshIcon(MaterialTheme.colorScheme.tertiary) }
            Title("Mesh needs the VPN")
            Body("Myco’s VPN only links Myco phones; your internet traffic doesn’t go through it.")
            Note("Without it, apps on nearby phones can’t be reached. Apps on this phone still open.")
            Spacer(Modifier.height(20.dp))
            PrimaryButton("Try again") { onAction(SetupAction.RetryVpn) }
            Spacer(Modifier.height(6.dp))
            QuietButton("Not now") { onAction(SetupAction.NotNow) }
        }
        SetupStep.AlwaysOnVpn -> {
            IconBadge(warn = true, size = 68) {
                Icon(
                    Icons.Outlined.GppMaybe,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.tertiary,
                    modifier = Modifier.size(34.dp),
                )
            }
            Title("Another VPN is always on", size = 21)
            Body(
                "Android runs only one VPN at a time, and another app’s VPN is set to " +
                    "always on, so Myco’s mesh connection can’t start.",
            )
            // True to the code: with the VPN step failed the mesh is switched
            // off, and the radios only start the node while it is on — but the
            // gateway, relay and Blossom store are local and never needed it.
            Note(
                "Apps already on this phone still open. Finding nearby phones and " +
                    "sharing apps with them won’t work until it’s fixed.",
            )
            Spacer(Modifier.height(16.dp))
            FixBox(
                "Settings › Network & internet › VPN › the other app: turn off " +
                    "“Always-on VPN”, then come back here.",
            )
            Spacer(Modifier.height(24.dp))
            PrimaryButton("Open VPN settings") { onAction(SetupAction.OpenVpnSettings) }
            Spacer(Modifier.height(6.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
                QuietButton("Try again", Modifier.weight(1f)) { onAction(SetupAction.RetryVpn) }
                QuietButton("Continue without", Modifier.weight(1f)) {
                    onAction(SetupAction.ContinueWithout)
                }
            }
            Text(
                "Continuing marks this step as skipped.",
                color = muted(),
                fontSize = 11.5.sp,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
            )
        }
        SetupStep.Name -> {
            IconBadge(warn = false) {
                Icon(
                    Icons.Outlined.Badge,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.size(34.dp),
                )
            }
            Title("What should people call you?")
            Body("Phones you pair with see this name.")
            Spacer(Modifier.height(16.dp))
            Box(
                contentAlignment = Alignment.Center,
                modifier = Modifier
                    .fillMaxWidth()
                    .background(MaterialTheme.colorScheme.onSurface.copy(alpha = 0.06f), RoundedCornerShape(12.dp))
                    .border(1.dp, MaterialTheme.colorScheme.outline, RoundedCornerShape(12.dp))
                    .padding(horizontal = 14.dp, vertical = 14.dp),
            ) {
                Text(name, color = MaterialTheme.colorScheme.onSurface, fontWeight = FontWeight.SemiBold, fontSize = 17.sp)
            }
            Spacer(Modifier.height(24.dp))
            PrimaryButton("Keep using this") { onAction(SetupAction.KeepName) }
            Spacer(Modifier.height(10.dp))
            NeutralButton("Change it", onChangeName)
        }
    }
}

/** "Change it": one field, Save or Cancel. Save finishes the popup. */
@Composable
private fun ChangeNameDialog(initial: String, onSave: (String) -> Unit, onCancel: () -> Unit) {
    var text by rememberSaveable { mutableStateOf(initial) }
    AlertDialog(
        onDismissRequest = onCancel,
        title = { Text("Your name") },
        text = {
            OutlinedTextField(
                value = text,
                onValueChange = { text = it.take(DeviceName.MAX_LENGTH) },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
        },
        confirmButton = {
            TextButton(enabled = text.isNotBlank(), onClick = { onSave(text.trim()) }) { Text("Save") }
        },
        dismissButton = { TextButton(onClick = onCancel) { Text("Cancel") } },
    )
}

// ----------------------------------------------------------------------------
// Card and progress bar
// ----------------------------------------------------------------------------

private fun label(segment: Segment): String = when (segment) {
    Segment.Install -> "Install Myco"
    Segment.Nearby -> "Nearby phones"
    Segment.Connection -> "Connection"
    Segment.Name -> "Name"
}

@Composable
private fun SetupCard(
    step: SetupStep,
    plan: SetupPlan,
    segments: List<SegmentState>,
    content: @Composable () -> Unit,
) {
    Surface(
        shape = RoundedCornerShape(28.dp),
        color = MaterialTheme.colorScheme.surface,
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            modifier = Modifier
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 24.dp, vertical = 24.dp),
        ) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                Text(
                    "Step ${MeshSetup.stepNumber(step, plan)} of ${plan.segments.size}",
                    color = muted(),
                    fontWeight = FontWeight.SemiBold,
                    fontSize = 12.sp,
                )
                Text("Set up Myco", color = muted(), fontSize = 12.sp)
            }
            Spacer(Modifier.height(10.dp))
            SegmentBar(plan.segments, segments)
            Spacer(Modifier.height(28.dp))
            content()
        }
    }
}

/**
 * The segmented progress bar. Done is a full green bar; the current step is
 * half filled inside a green ring; a refused step is amber inside an amber
 * ring; a step not reached, or skipped, is a grey track.
 */
@Composable
private fun SegmentBar(labels: List<Segment>, segments: List<SegmentState>) {
    val primary = MaterialTheme.colorScheme.primary
    val warn = MaterialTheme.colorScheme.tertiary
    val track = MaterialTheme.colorScheme.outline
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        segments.forEachIndexed { i, seg ->
            Column(Modifier.weight(1f), horizontalAlignment = Alignment.CenterHorizontally) {
                Canvas(Modifier.fillMaxWidth().height(10.dp)) {
                    val inset = 2.dp.toPx()
                    val barH = size.height - 2 * inset
                    val r = CornerRadius(barH / 2, barH / 2)
                    val barSize = Size(size.width - 2 * inset, barH)
                    val origin = Offset(inset, inset)
                    val (fill, fraction) = when (seg) {
                        SegmentState.Done -> primary to 1f
                        SegmentState.Active -> primary to 0.5f
                        SegmentState.Problem -> warn to 1f
                        SegmentState.Pending, SegmentState.Skipped -> track to 0f
                    }
                    drawRoundRect(track, origin, barSize, r)
                    if (fraction > 0f) {
                        drawRoundRect(fill, origin, Size(barSize.width * fraction, barH), r)
                    }
                    if (seg == SegmentState.Active || seg == SegmentState.Problem) {
                        val stroke = 1.5.dp.toPx()
                        drawRoundRect(
                            color = fill,
                            topLeft = Offset(stroke / 2, stroke / 2),
                            size = Size(size.width - stroke, size.height - stroke),
                            cornerRadius = CornerRadius(size.height / 2, size.height / 2),
                            style = Stroke(stroke),
                        )
                    }
                }
                Spacer(Modifier.height(8.dp))
                val label = label(labels[i])
                Text(
                    when (seg) {
                        SegmentState.Done -> "✓ $label"
                        SegmentState.Problem -> "! $label"
                        SegmentState.Skipped -> "– $label"
                        else -> label
                    },
                    color = when (seg) {
                        SegmentState.Active -> MaterialTheme.colorScheme.onSurface
                        SegmentState.Problem -> warn
                        else -> muted()
                    },
                    fontWeight = if (seg == SegmentState.Active || seg == SegmentState.Problem) {
                        FontWeight.Bold
                    } else {
                        FontWeight.Normal
                    },
                    fontSize = 11.sp,
                    maxLines = 1,
                    textAlign = TextAlign.Center,
                )
            }
        }
    }
}

// ----------------------------------------------------------------------------
// Pieces
// ----------------------------------------------------------------------------

/** Secondary text: the design's zinc-400 on black, a muted ink on white. */
@Composable
private fun muted(): Color = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.64f)

@Composable
private fun isDark(): Boolean = MaterialTheme.colorScheme.background.luminance() < 0.5f

@Composable
private fun IconBadge(warn: Boolean, size: Int = 76, icon: @Composable () -> Unit) {
    val tint = if (warn) MaterialTheme.colorScheme.tertiary else MaterialTheme.colorScheme.primary
    Box(
        contentAlignment = Alignment.Center,
        modifier = Modifier.size(size.dp).background(tint.copy(alpha = 0.16f), CircleShape),
    ) { icon() }
    Spacer(Modifier.height(20.dp))
}

@Composable
private fun Title(text: String, size: Int = 22) {
    Text(
        text,
        color = MaterialTheme.colorScheme.onSurface,
        fontWeight = FontWeight.Bold,
        fontSize = size.sp,
        textAlign = TextAlign.Center,
    )
    Spacer(Modifier.height(10.dp))
}

@Composable
private fun Body(text: String) {
    Text(
        text,
        color = MaterialTheme.colorScheme.onSurface,
        fontSize = 14.sp,
        lineHeight = 20.sp,
        textAlign = TextAlign.Center,
    )
}

@Composable
private fun Note(text: String) {
    Spacer(Modifier.height(10.dp))
    Text(text, color = muted(), fontSize = 12.5.sp, lineHeight = 18.sp, textAlign = TextAlign.Center)
}

@Composable
private fun FixBox(text: String) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.onSurface.copy(alpha = 0.06f), RoundedCornerShape(12.dp))
            .border(1.dp, MaterialTheme.colorScheme.outline, RoundedCornerShape(12.dp))
            .padding(horizontal = 14.dp, vertical = 10.dp),
    ) {
        Text("To fix", color = MaterialTheme.colorScheme.tertiary, fontWeight = FontWeight.Bold, fontSize = 12.sp)
        Spacer(Modifier.height(4.dp))
        Text(text, color = MaterialTheme.colorScheme.onSurface, fontSize = 12.sp, lineHeight = 16.sp)
    }
}

@Composable
private fun Waiting(icon: @Composable () -> Unit, title: String, body: String) {
    IconBadge(warn = false, icon = icon)
    Title(title)
    Text(body, color = muted(), fontSize = 13.sp, textAlign = TextAlign.Center)
    Spacer(Modifier.height(20.dp))
    CircularProgressIndicator(
        color = MaterialTheme.colorScheme.primary,
        strokeWidth = 2.5.dp,
        modifier = Modifier.size(24.dp),
    )
    Spacer(Modifier.height(8.dp))
}

@Composable
private fun PrimaryButton(text: String, onClick: () -> Unit) {
    Button(
        onClick = onClick,
        shape = RoundedCornerShape(24.dp),
        colors = ButtonDefaults.buttonColors(
            containerColor = MaterialTheme.colorScheme.primary,
            contentColor = MaterialTheme.colorScheme.onPrimary,
        ),
        modifier = Modifier.fillMaxWidth().height(48.dp),
    ) { Text(text, fontWeight = FontWeight.SemiBold, fontSize = 15.sp) }
}

/** "No thanks": filled, but grey — a real choice, not a hidden one. */
@Composable
private fun NeutralButton(text: String, onClick: () -> Unit) {
    val dark = isDark()
    Button(
        onClick = onClick,
        shape = RoundedCornerShape(24.dp),
        colors = ButtonDefaults.buttonColors(
            containerColor = if (dark) Color(0xFF27272A) else Color(0xFFE4E4E7),
            contentColor = if (dark) Color(0xFFE4E4E7) else Color(0xFF27272A),
        ),
        modifier = Modifier.fillMaxWidth().height(48.dp),
    ) { Text(text, fontWeight = FontWeight.SemiBold, fontSize = 15.sp) }
}

@Composable
private fun QuietButton(text: String, modifier: Modifier = Modifier.fillMaxWidth(), onClick: () -> Unit) {
    TextButton(onClick = onClick, modifier = modifier.height(48.dp)) {
        Text(text, color = MaterialTheme.colorScheme.primary, fontWeight = FontWeight.SemiBold, fontSize = 15.sp)
    }
}

// ----------------------------------------------------------------------------
// Icons — drawn, to match the design; Material has no two-phones or mesh glyph.
// ----------------------------------------------------------------------------

/** Two phones with a signal between them: "Nearby phones". */
@Composable
internal fun PhonesIcon(color: Color, modifier: Modifier = Modifier.size(40.dp)) {
    Canvas(modifier) {
        val u = size.minDimension / 40f
        val stroke = Stroke(width = 2.4f * u, cap = StrokeCap.Round)
        val phone = Size(11f * u, 20f * u)
        val r = CornerRadius(2.5f * u, 2.5f * u)
        drawRoundRect(color, Offset(1.5f * u, 10f * u), phone, r, style = stroke)
        drawRoundRect(color, Offset(27.5f * u, 10f * u), phone, r, style = stroke)
        val arc = Size(8f * u, 10f * u)
        drawArc(color, -55f, 110f, false, Offset(13.5f * u, 15f * u), arc, style = stroke)
        drawArc(color, 125f, 110f, false, Offset(18.5f * u, 15f * u), arc, style = stroke)
    }
}

/** Five linked nodes: the mesh connection. */
@Composable
internal fun MeshIcon(color: Color, modifier: Modifier = Modifier.size(40.dp)) {
    Canvas(modifier) {
        val u = size.minDimension / 40f
        val c = Offset(size.width / 2, size.height / 2)
        val nodes = listOf(
            Offset(0f, -14f), Offset(-13f, 0f), Offset(13f, 0f), Offset(-7f, 13f), Offset(7f, 13f),
        ).map { c + it * u }
        val links = listOf(0 to 1, 0 to 2, 0 to 3, 0 to 4, 1 to 3, 2 to 4, 3 to 4)
        for ((a, b) in links) drawLine(color, nodes[a], nodes[b], strokeWidth = 2f * u)
        for (n in nodes) drawCircle(color, radius = 4f * u, center = n)
    }
}
