package app.myco.ui.intro

import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.sin
import kotlin.random.Random

/**
 * Geometry for the Myco mark — the fourfold mycelial mandala from
 * `docs/myco-logo.png`, grown rather than drawn.
 *
 * Only **one quadrant** is generated. [IntroScreen] draws it four times at
 * 90/180/270°, which is where the logo's symmetry comes from and keeps the
 * per-frame path count to a quarter of what it looks like.
 *
 * This file is deliberately free of Compose graphics types: it emits plain
 * coordinates so it can be unit-tested on the JVM without Robolectric. The
 * caller turns [MarkSegment]s into paths once and holds onto them.
 */

/** The coordinate space the mark is authored in; scaled to the viewport. */
const val MARK_VIEWPORT = 400f
const val MARK_CENTER = MARK_VIEWPORT / 2f

/** Where the filaments stop. The ring sits on this radius too, so they end on its line. */
const val MARK_RING_RADIUS = 176f

/** How far from the centre the innermost filaments start — the spark sits inside this. */
private const val ROOT_RADIUS = 22f

private const val DEPTH = 5
private const val ROOT_LENGTH = 44f
private const val LENGTH_DECAY = 0.84f

/** Trunk headings for one quadrant, in degrees. Four arms → sixteen in the round. */
private val ARMS = floatArrayOf(-32f, -6f, 20f, 44f)

/**
 * One filament: a quadratic curve from ([x0], [y0]) to ([x1], [y1]) bent around
 * ([cx], [cy]).
 *
 * The bend is the whole trick — it is what gives the mark its woven look
 * instead of a bicycle wheel of straight spokes. Successive generations bend
 * the opposite way.
 *
 * [generation] is depth from the trunk, and drives when this segment is drawn.
 * [parent] is the segment this one grows out of, so growth can propagate along
 * the structure rather than appearing in disconnected rings.
 */
data class MarkSegment(
    val x0: Float,
    val y0: Float,
    val cx: Float,
    val cy: Float,
    val x1: Float,
    val y1: Float,
    val generation: Int,
    val strokeWidth: Float,
    val alpha: Float,
    /** Trunk segments, drawn in the lighter tint. */
    val bright: Boolean,
    /** True for the handful of spines the nutrient pulses run along. */
    val spine: Boolean,
    val parent: Int,
) {
    /** Distance of this segment's outer end from the centre of the mark. */
    val endRadius: Float get() = hypot(x1 - MARK_CENTER, y1 - MARK_CENTER)
}

/** A swollen node where a trunk first forks. Four of them in the round. */
data class MarkNode(
    val x: Float,
    val y: Float,
    val radius: Float,
    /** Polar position, so the pupil can push the node ahead of itself. */
    val distance: Float,
    val angle: Float,
    val generation: Int,
    val parent: Int,
)

data class MycoMark(
    val segments: List<MarkSegment>,
    val nodes: List<MarkNode>,
) {
    val generations: Int get() = (segments.maxOfOrNull { it.generation } ?: -1) + 1

    /** Indices of the segments the nutrient pulses travel along. */
    val spineIndices: List<Int> get() = segments.indices.filter { segments[it].spine }
}

/**
 * Grow one quadrant of the mark.
 *
 * [seed] fixes the jitter. It must stay fixed across recomposition and
 * configuration changes or the mark would reshuffle itself mid-animation —
 * hold the result in `remember`.
 */
fun buildMycoMark(seed: Long = 20_260_806L): MycoMark {
    val random = Random(seed)
    val segments = mutableListOf<MarkSegment>()
    val nodes = mutableListOf<MarkNode>()

    fun between(min: Float, max: Float) = min + random.nextFloat() * (max - min)

    fun grow(
        x: Float,
        y: Float,
        angle: Float,
        length: Float,
        depth: Int,
        generation: Int,
        curl: Float,
        trunk: Boolean,
        parent: Int,
    ) {
        if (depth <= 0) return

        val x1 = x + cos(angle) * length
        val y1 = y + sin(angle) * length

        // Control point pushed off the perpendicular. Alternating `curl` down
        // the chain is what weaves the filaments together.
        val perpendicular = angle + (Math.PI / 2).toFloat()
        val bow = length * between(0.26f, 0.42f) * curl
        val cx = (x + x1) / 2f + cos(perpendicular) * bow
        val cy = (y + y1) / 2f + sin(perpendicular) * bow

        val index = segments.size
        segments += MarkSegment(
            x0 = x, y0 = y, cx = cx, cy = cy, x1 = x1, y1 = y1,
            generation = generation,
            strokeWidth = depth * 0.62f + 0.5f,
            alpha = 0.55f + depth * 0.09f,
            bright = depth >= DEPTH - 1,
            // The first two trunk segments of each arm carry the nutrients.
            spine = trunk && generation < 2,
            parent = parent,
        )

        if (trunk && depth == DEPTH - 1) {
            nodes += MarkNode(
                x = x1, y = y1, radius = 5.5f,
                distance = hypot(x1 - MARK_CENTER, y1 - MARK_CENTER),
                angle = atan2(y1 - MARK_CENTER, x1 - MARK_CENTER),
                generation = generation + 1,
                parent = index,
            )
        }

        val branches = if (depth > 2) 2 else if (random.nextFloat() < 0.7f) 2 else 1
        for (i in 0 until branches) {
            val fan = (i - (branches - 1) / 2f) * between(0.34f, 0.6f)
            grow(
                x = x1,
                y = y1,
                angle = angle + fan + between(-0.06f, 0.06f),
                length = length * between(LENGTH_DECAY - 0.06f, LENGTH_DECAY + 0.06f),
                depth = depth - 1,
                generation = generation + 1,
                curl = -curl,
                trunk = trunk && i == branches / 2,
                parent = index,
            )
        }
    }

    ARMS.forEachIndexed { i, degrees ->
        val angle = (degrees * Math.PI / 180.0).toFloat()
        grow(
            x = MARK_CENTER + cos(angle) * ROOT_RADIUS,
            y = MARK_CENTER + sin(angle) * ROOT_RADIUS,
            angle = angle,
            length = ROOT_LENGTH,
            depth = DEPTH,
            generation = 0,
            curl = if (i % 2 == 0) -1f else 1f,
            trunk = true,
            parent = -1,
        )
    }

    return MycoMark(segments, nodes)
}

/**
 * The tendril the idle prompt sits on: a small branching sprig that grows
 * downward out of the mark and carries the words up on it.
 *
 * Authored in its own little coordinate space ([TENDRIL_WIDTH] × [TENDRIL_HEIGHT]),
 * rooted at the top centre.
 */
const val TENDRIL_WIDTH = 200f
const val TENDRIL_HEIGHT = 54f

fun buildTendril(seed: Long = 20_260_807L): MycoMark {
    val random = Random(seed)
    val segments = mutableListOf<MarkSegment>()

    fun between(min: Float, max: Float) = min + random.nextFloat() * (max - min)

    fun grow(x: Float, y: Float, angle: Float, length: Float, depth: Int, generation: Int, parent: Int) {
        if (depth <= 0) return

        val x1 = x + cos(angle) * length
        val y1 = y + sin(angle) * length
        val perpendicular = angle + (Math.PI / 2).toFloat()
        val bow = length * between(0.15f, 0.3f) * if (generation % 2 == 0) -1f else 1f
        val cx = (x + x1) / 2f + cos(perpendicular) * bow
        val cy = (y + y1) / 2f + sin(perpendicular) * bow

        val index = segments.size
        segments += MarkSegment(
            x0 = x, y0 = y, cx = cx, cy = cy, x1 = x1, y1 = y1,
            generation = generation,
            strokeWidth = depth * 0.45f + 0.35f,
            alpha = 0.35f + depth * 0.14f,
            bright = false,
            spine = false,
            parent = parent,
        )

        // It runs sideways along the words as it thins out.
        for (i in 0 until 2) {
            val fan = (if (i == 0) -1f else 1f) * between(0.5f, 1.15f)
            grow(x1, y1, angle + fan, length * between(0.62f, 0.82f), depth - 1, generation + 1, index)
        }
    }

    grow(
        x = TENDRIL_WIDTH / 2f,
        y = 0f,
        angle = (Math.PI / 2).toFloat(),   // straight down, out of the mark
        length = 17f,
        depth = 4,
        generation = 0,
        parent = -1,
    )

    return MycoMark(segments, emptyList())
}
