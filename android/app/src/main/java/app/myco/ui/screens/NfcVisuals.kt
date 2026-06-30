package app.myco.ui.screens

import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.myco.ui.theme.Emerald

/**
 * The animated "tap to connect" NFC bubble — a filled circle whose two outer
 * contactless arcs breathe to draw a little attention without distracting. Shared
 * by the Circle tab's tap-to-connect hint and the share-an-app sheet, so both NFC
 * surfaces read the same. Radii scale with [size], so callers can size it freely.
 */
@Composable
internal fun NfcPulseBubble(
    modifier: Modifier = Modifier,
    size: Dp = 48.dp,
    background: Color = Emerald,
) {
    androidx.compose.foundation.layout.Box(
        modifier = modifier.size(size).background(background, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        val transition = rememberInfiniteTransition(label = "nfc")
        val wave by transition.animateFloat(
            initialValue = 0.25f,
            targetValue = 1f,
            animationSpec = infiniteRepeatable(tween(950, easing = FastOutSlowInEasing), RepeatMode.Reverse),
            label = "wave",
        )
        Canvas(modifier = Modifier.size(size * 0.58f)) {
            val w = this.size.width
            val cx = w * 0.30f
            val cy = w / 2f
            val white = Color.White
            drawCircle(white, radius = w * 0.064f, center = Offset(cx, cy))
            fun arc(rFraction: Float, alpha: Float) {
                val r = w * rFraction
                drawArc(
                    color = white.copy(alpha = alpha),
                    startAngle = -52f,
                    sweepAngle = 104f,
                    useCenter = false,
                    topLeft = Offset(cx - r, cy - r),
                    size = Size(2 * r, 2 * r),
                    style = Stroke(width = w * 0.078f, cap = StrokeCap.Round),
                )
            }
            arc(0.16f, 1f)
            arc(0.286f, wave)
            arc(0.41f, wave * 0.7f)
        }
    }
}
