package app.myco.neighbor

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.unit.dp
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.min
import kotlin.math.sin
import kotlin.math.sqrt

/** One bubble's colour: stable per neighborhood id, Everyone always green. */
internal fun bubbleColor(n: NeighborhoodsState.Neighborhood): Color {
    val palette = listOf(
        Color(0xFFF59E0B), Color(0xFF3B82F6), Color(0xFFEF4444), Color(0xFF8B5CF6),
        Color(0xFFEC4899), Color(0xFF14B8A6), Color(0xFFF97316), Color(0xFF6366F1),
    )
    return palette[(n.id.hashCode() and 0x7fffffff) % palette.size]
}

private data class Placed(val center: Offset, val radius: Float)

/**
 * The user's neighborhoods as overlapping bubbles around them. Each bubble is
 * sized by how many peers carry it; peers orbit the bubbles they carry, and a
 * peer in several sits between them with a line to each. Reachable peers are
 * solid, the rest faded. Tap a bubble or a peer.
 */
@Composable
fun NeighborMap(
    state: NeighborhoodsState,
    onNeighborhood: (NeighborhoodsState.Neighborhood) -> Unit,
    onPeer: (NeighborhoodsState.PeerNode) -> Unit,
) {
    val pulse by rememberInfiniteTransition(label = "pulse").animateFloat(
        initialValue = 0f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(2400), RepeatMode.Restart),
        label = "pulse",
    )
    val onSurface = MaterialTheme.colorScheme.onSurface
    val surface = MaterialTheme.colorScheme.surface
    val hoods = state.list
    val peers = state.peers

    // Positions are recomputed on draw and on tap from the same function, so
    // what you touch is what you see.
    fun layout(w: Float, h: Float): Pair<Map<String, Placed>, Map<String, Offset>> {
        val c = Offset(w / 2, h / 2)
        val ring = min(w, h) * 0.30f
        val maxPeers = (hoods.maxOfOrNull { carriers(peers, it.id) } ?: 1).coerceAtLeast(1)
        val bubbles = hoods.mapIndexed { i, n ->
            val a = if (hoods.size == 1) -PI / 2 else -PI / 2 + 2 * PI * i / hoods.size
            val count = carriers(peers, n.id)
            val r = min(w, h) * (0.09f + 0.07f * sqrt(count.toFloat() / maxPeers))
            n.id to Placed(
                if (hoods.size == 1) c else Offset(c.x + ring * cos(a).toFloat(), c.y + ring * sin(a).toFloat()),
                r,
            )
        }.toMap()
        val nodes = mutableMapOf<String, Offset>()
        val perHood = mutableMapOf<String, Int>()
        for (p in peers) {
            val homes = p.neighborhoods.mapNotNull { bubbles[it] }
            if (homes.isEmpty()) continue
            nodes[p.key] = if (homes.size == 1) {
                // Orbit the one bubble.
                val home = p.neighborhoods.first { bubbles[it] != null }
                val k = perHood.getOrDefault(home, 0).also { perHood[home] = it + 1 }
                val placed = homes.first()
                val a = 2 * PI * (k * 0.618034)
                val r = placed.radius + 18f
                Offset(placed.center.x + r * cos(a).toFloat(), placed.center.y + r * sin(a).toFloat())
            } else {
                // Between its bubbles.
                Offset(homes.map { it.center.x }.average().toFloat(), homes.map { it.center.y }.average().toFloat())
            }
        }
        return bubbles to nodes
    }

    Canvas(
        modifier = Modifier
            .fillMaxWidth()
            .height(320.dp)
            .pointerInput(state) {
                detectTapGestures { tap ->
                    val (bubbles, nodes) = layout(size.width.toFloat(), size.height.toFloat())
                    peers.firstOrNull { p ->
                        nodes[p.key]?.let { hypot(it.x - tap.x, it.y - tap.y) < 28f } == true
                    }?.let { onPeer(it); return@detectTapGestures }
                    hoods.firstOrNull { n ->
                        bubbles[n.id]?.let { hypot(it.center.x - tap.x, it.center.y - tap.y) < it.radius } == true
                    }?.let(onNeighborhood)
                }
            },
    ) {
        val (bubbles, nodes) = layout(size.width, size.height)
        val me = Offset(size.width / 2, size.height / 2)

        // Bubbles, then lines, then peers on top.
        for (n in hoods) {
            val b = bubbles[n.id] ?: continue
            val color = bubbleColor(n)
            if (n.nearby) {
                drawCircle(color.copy(alpha = 0.25f * (1 - pulse)), b.radius * (1 + 0.35f * pulse), b.center)
            }
            drawCircle(color.copy(alpha = 0.22f), b.radius, b.center)
            drawCircle(color, b.radius, b.center, style = Stroke(width = 3f))
            label(n.name, b.center, onSurface)
        }
        for (p in peers) {
            val at = nodes[p.key] ?: continue
            for (id in p.neighborhoods) {
                val b = bubbles[id] ?: continue
                drawLine(onSurface.copy(alpha = if (p.reachable) 0.5f else 0.2f), at, b.center, strokeWidth = 2f)
            }
        }
        for (p in peers) {
            val at = nodes[p.key] ?: continue
            val alpha = if (p.reachable) 1f else 0.4f
            drawCircle(surface, 14f, at)
            drawCircle(onSurface.copy(alpha = alpha), 14f, at, style = Stroke(width = 3f))
            if (p.reachable) drawCircle(Color(0xFF10B981), 5f, Offset(at.x + 10f, at.y - 10f))
            label(p.name.take(10), Offset(at.x, at.y + 30f), onSurface.copy(alpha = alpha), small = true)
        }
        drawCircle(onSurface, 20f, me)
        label("you", Offset(me.x, me.y + 40f), onSurface)
    }
}

private fun carriers(peers: List<NeighborhoodsState.PeerNode>, id: String): Int =
    peers.count { id in it.neighborhoods }

private fun DrawScope.label(text: String, at: Offset, color: Color, small: Boolean = false) {
    val paint = android.graphics.Paint().apply {
        isAntiAlias = true
        textAlign = android.graphics.Paint.Align.CENTER
        textSize = if (small) 24f else 32f
        this.color = android.graphics.Color.argb(
            (color.alpha * 255).toInt(), (color.red * 255).toInt(), (color.green * 255).toInt(), (color.blue * 255).toInt(),
        )
    }
    drawContext.canvas.nativeCanvas.drawText(text, at.x, at.y + paint.textSize / 3, paint)
}
