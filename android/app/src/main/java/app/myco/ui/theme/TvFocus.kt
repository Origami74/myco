package app.myco.ui.theme

import android.content.pm.PackageManager
import androidx.compose.foundation.IndicationNodeFactory
import androidx.compose.foundation.LocalIndication
import androidx.compose.foundation.interaction.FocusInteraction
import androidx.compose.foundation.interaction.InteractionSource
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LocalRippleConfiguration
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RippleConfiguration
import androidx.compose.material3.ripple
import androidx.compose.material.ripple.RippleAlpha
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.focus.FocusDirection
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.ContentDrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.node.DelegatableNode
import androidx.compose.ui.node.DelegatingNode
import androidx.compose.ui.node.DrawModifierNode
import androidx.compose.ui.node.invalidateDraw
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.android.awaitFrame
import kotlinx.coroutines.launch

/**
 * Android TV support for a UI driven by a remote's D-pad rather than touch.
 *
 * Material's focus state is a faint tint meant for a keyboard user at arm's
 * length; from a sofa it is invisible, so a remote user cannot tell which
 * button OK will press. On a leanback device every clickable gets a solid ring
 * when focused, and Material components a much stronger focus overlay.
 */
val LocalIsTv = staticCompositionLocalOf { false }

/** Whether this device is a TV (leanback) — a remote, not a touchscreen. */
@Composable
fun rememberIsTv(): Boolean {
    val context = LocalContext.current
    return remember(context) {
        context.packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK)
    }
}

/** Provides [LocalIsTv] and, on a TV, the focus ring and focus overlay. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ProvideTvFocus(content: @Composable () -> Unit) {
    val tv = rememberIsTv()
    if (!tv) {
        CompositionLocalProvider(LocalIsTv provides false, content = content)
        return
    }
    val ring = MaterialTheme.colorScheme.primary
    CompositionLocalProvider(
        LocalIsTv provides true,
        // Plain clickables (app tiles, person bubbles, cards) draw through
        // LocalIndication: a ring around them, on top of the usual ripple.
        LocalIndication provides FocusRingIndication(ring),
        // Material components (buttons, rail items, switches) draw their own
        // ripple; raise its focused overlay from ~10% to clearly visible.
        LocalRippleConfiguration provides RippleConfiguration(
            rippleAlpha = RippleAlpha(
                draggedAlpha = 0.16f,
                focusedAlpha = 0.45f,
                hoveredAlpha = 0.08f,
                pressedAlpha = 0.24f,
            ),
        ),
        content = content,
    )
}

/**
 * On a TV, move focus to the first focusable element once [key] settles — a
 * dialog or sheet that opens with nothing focused costs a remote user a
 * wasted key press, and they cannot see where focus went. No-op on touch.
 */
@Composable
fun TvAutoFocus(key: Any?) {
    if (!LocalIsTv.current) return
    val focusManager = LocalFocusManager.current
    LaunchedEffect(key) {
        awaitFrame()
        focusManager.moveFocus(FocusDirection.Next)
    }
}

/**
 * A long press for a remote: the Menu key, or OK held until it repeats, calls
 * [onLongClick] — the touch long-press menus are otherwise out of reach. The
 * OK release that ends a hold is swallowed so it does not also click. Put it
 * before the `combinedClickable` it complements.
 */
fun Modifier.remoteLongClick(onLongClick: (() -> Unit)?): Modifier =
    if (onLongClick == null) this else composed {
        // Not state: nothing draws from it, it only spans one key press.
        val held = remember { BooleanArray(1) }
        onPreviewKeyEvent { event ->
            val ok = event.key == Key.DirectionCenter || event.key == Key.Enter ||
                event.key == Key.NumPadEnter
            when {
                event.key == Key.Menu && event.type == KeyEventType.KeyUp -> {
                    onLongClick(); true
                }
                event.key == Key.Menu -> true
                ok && event.type == KeyEventType.KeyDown &&
                    event.nativeKeyEvent.repeatCount > 0 -> {
                    if (!held[0]) { held[0] = true; onLongClick() }
                    true
                }
                ok && event.type == KeyEventType.KeyUp && held[0] -> {
                    held[0] = false; true
                }
                else -> false
            }
        }
    }

private data class FocusRingIndication(val color: Color) : IndicationNodeFactory {
    override fun create(interactionSource: InteractionSource): DelegatableNode =
        FocusRingNode(interactionSource, color)
}

private class FocusRingNode(
    private val source: InteractionSource,
    private val color: Color,
) : DelegatingNode(), DrawModifierNode {
    private var focused = 0

    init {
        delegate(ripple().create(source))
    }

    override fun onAttach() {
        coroutineScope.launch {
            source.interactions.collect {
                when (it) {
                    is FocusInteraction.Focus -> focused++
                    is FocusInteraction.Unfocus -> focused = (focused - 1).coerceAtLeast(0)
                    else -> return@collect
                }
                invalidateDraw()
            }
        }
    }

    override fun ContentDrawScope.draw() {
        drawContent()
        if (focused == 0) return
        val width = 3.dp.toPx()
        val inset = 2.dp.toPx() + width / 2
        drawRoundRect(
            color = color,
            topLeft = Offset(-inset, -inset),
            size = Size(size.width + 2 * inset, size.height + 2 * inset),
            cornerRadius = CornerRadius(14.dp.toPx()),
            style = Stroke(width),
        )
    }
}
