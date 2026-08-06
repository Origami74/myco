package app.myco.ui.intro

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathMeasure
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.material3.Text
import app.myco.ui.theme.MarkBright
import app.myco.ui.theme.MarkFilament
import app.myco.ui.theme.MarkGround
import app.myco.ui.theme.MarkNodeColor
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.sin

/**
 * The first-run intro: a spark, roots growing out of it into the Myco mark,
 * the ring closing, and — on a tap — a pupil opening in the middle of the mark
 * that the camera then falls into.
 *
 * The pupil is a hole in the intro, not a black disc: the ground is drawn
 * inside the same off-screen layer the hole is punched through, so whatever is
 * composed underneath — the app itself — shows through it from the moment it
 * opens. Diving is then literally that hole growing until it is the whole
 * screen, at which point [onFinished] fires and the overlay comes off.
 *
 * Until the dive starts the hole is frosted rather than clear: a wash of the
 * ground is laid back over it, and [onFrost] reports how much, so the caller
 * can blur what it has composed underneath by the same amount. You can see
 * that there is something behind the mark long before you can read it.
 *
 * The growing and the waiting are a first-launch event. On every launch after
 * that [IntroMode.Returning] keeps the pupil and drops everything in front of
 * it, so what is otherwise a piece of theatre stays out of the way of someone
 * who just wants to open the app.
 *
 * ### How it is driven
 *
 * One clock, in seconds, advanced from `withFrameNanos`, and every beat is a
 * window on it (see [phase]). That mirrors the timeline this was designed
 * against and keeps the whole thing seekable — hand [elapsed] a different
 * starting value and you land mid-animation, which is how it was tuned.
 */
enum class IntroMode {
    /** The whole thing: the mark grows, then waits to be tapped. */
    FirstRun,

    /** Straight to the pupil, and quicker. Roughly two seconds, start to app. */
    Returning,
}

@Composable
fun IntroScreen(
    modifier: Modifier = Modifier,
    mode: IntroMode = IntroMode.FirstRun,
    /** 1 while the pupil is frosted, easing to 0 as the dive clears it. */
    onFrost: (Float) -> Unit = {},
    onFinished: () -> Unit,
) {
    val returning = mode == IntroMode.Returning
    val mark = remember { buildMycoMark() }
    val tendril = remember { buildTendril() }
    val markPaths = remember(mark) { mark.segments.map { it.toPath() } }
    val markLengths = remember(markPaths) { markPaths.map { it.measuredLength() } }
    val tendrilPaths = remember(tendril) { tendril.segments.map { it.toPath() } }
    val tendrilLengths = remember(tendrilPaths) { tendrilPaths.map { it.measuredLength() } }

    // Growth schedule: generation g starts GEN_STEP after g-1. Within a
    // generation each filament is nudged a little further along so the front
    // goes ragged, but the nudge is capped — generations roughly double in
    // size, so an uncapped per-element stagger leaves the outermost few
    // hundred fibres trickling in seconds after the rest of the mark is done.
    val markStarts = remember(mark) { mark.segments.growthOffsets() }

    // Returning: the mark is already fully grown on the first frame, and it is
    // already going in.
    var elapsed by remember { mutableFloatStateOf(if (returning) MARK_SETTLED else 0f) }
    var diveAt by remember { mutableStateOf(if (returning) MARK_SETTLED else null) }
    var finished by remember { mutableStateOf(false) }

    val finish by rememberUpdatedState(onFinished)
    val frosted by rememberUpdatedState(onFrost)

    LaunchedEffect(Unit) {
        var last = withFrameNanos { it }
        while (!finished) {
            val now = withFrameNanos { it }
            elapsed += (now - last) / 1_000_000_000f
            last = now

            // No auto-start: on first launch it waits as long as it takes.
            frosted(frost(diveClock(diveAt, elapsed, returning)))

            if (diveClock(diveAt, elapsed, returning).let { it != null && it >= DIVE_END }) {
                finished = true
                finish()
            }
        }
    }

    val dive = diveClock(diveAt, elapsed, returning)
    val pupil = pupilRadius(dive)

    Box(
        modifier = modifier
            .fillMaxSize()
            .pointerInput(Unit) {
                detectTapGestures { if (diveAt == null) diveAt = elapsed }
            },
    ) {
        Box(
            Modifier
                .fillMaxSize()
                // Ground and mark are drawn together in one off-screen layer
                // so a single Clear punches through both, leaving a genuine
                // window rather than a black disc. Working the retraction out
                // per filament instead is cheaper and wrong twice over: it
                // cannot make the ground transparent, and a filament that bows
                // inward mid-curve reads as outside the pupil on its endpoints,
                // leaving a bead of itself floating in the hole.
                .graphicsLayer { compositingStrategy = CompositingStrategy.Offscreen },
        ) {
            Canvas(Modifier.fillMaxSize()) {
                drawRect(MarkGround)

                val fit = min(size.width, size.height) / MARK_VIEWPORT
                val zoom = fit * MARK_FILL * diveZoom(dive)
                val breath = 1f + 0.05f * breathPhase(elapsed, dive)
                val dim = 1f - 0.28f * breathPhase(elapsed, dive)

                translate(size.width / 2f, size.height / 2f) {
                    scale(zoom * breath, zoom * breath, Offset.Zero) {
                        translate(-MARK_CENTER, -MARK_CENTER) {
                            drawMark(
                                mark = mark,
                                paths = markPaths,
                                lengths = markLengths,
                                starts = markStarts,
                                elapsed = elapsed,
                                dive = dive,
                                pupil = pupil,
                                dim = dim,
                            )
                        }
                    }
                }

                // …and the hole, punched through ground and mark alike.
                if (pupil > 0f) {
                    drawCircle(
                        color = Color.Transparent,
                        radius = pupil * zoom,
                        center = center,
                        blendMode = BlendMode.Clear,
                    )
                    // Frost: a wash of the ground laid back over the hole, so
                    // early on it reads as something seen through the mark
                    // rather than a window straight onto the app. It clears as
                    // the dive starts.
                    val haze = frost(dive)
                    if (haze > 0f) {
                        drawCircle(
                            color = MarkGround.copy(alpha = 0.62f * haze),
                            radius = pupil * zoom,
                            center = center,
                        )
                    }
                }
            }
        }

        IdlePrompt(
            elapsed = elapsed,
            diving = dive != null,
            tendril = tendril,
            paths = tendrilPaths,
            lengths = tendrilLengths,
            modifier = Modifier.align(Alignment.BottomCenter),
        )
    }
}

// ── the mark ────────────────────────────────────────────────────────────────

private fun DrawScope.drawMark(
    mark: MycoMark,
    paths: List<Path>,
    lengths: List<Float>,
    starts: List<Float>,
    elapsed: Float,
    dive: Float?,
    pupil: Float,
    dim: Float,
) {
    // Filaments stop at the ring rather than straggling past it.
    val bound = Path().apply {
        addOval(
            Rect(
                MARK_CENTER - MARK_RING_RADIUS,
                MARK_CENTER - MARK_RING_RADIUS,
                MARK_CENTER + MARK_RING_RADIUS,
                MARK_CENTER + MARK_RING_RADIUS,
            ),
        )
    }

    val fade = dive?.let { 1f - phase(it, FILAMENT_FADE_AT, FILAMENT_FADE_FOR) } ?: 1f
    if (fade <= 0f) return

    clipPath(bound) {
        // One quadrant, drawn four times. This is the <use rotate(90/180/270)>
        // of the original: a quarter of the geometry, all of the symmetry.
        for (turn in 0 until 4) {
            rotate(turn * 90f, Offset(MARK_CENTER, MARK_CENTER)) {
                mark.segments.forEachIndexed { i, segment ->
                    val drawn = phase(elapsed, starts[i], GEN_DRAW)
                    if (drawn <= 0f) return@forEachIndexed

                    val head = paths[i].head(drawn.easeOutQuad(), lengths[i]) ?: return@forEachIndexed
                    val colour = if (segment.bright) MarkBright else MarkFilament
                    val alpha = segment.alpha * dim * fade

                    // Bloom without a blur filter: a wide translucent pass
                    // under a narrow bright one. Cheaper than a real
                    // BlurMaskFilter and survives hardware layers.
                    if (segment.bright) {
                        drawPath(
                            path = head,
                            color = colour,
                            alpha = alpha * 0.22f,
                            style = Stroke(width = segment.strokeWidth * 3.2f, cap = ROUND),
                        )
                    }
                    drawPath(
                        path = head,
                        color = colour,
                        alpha = alpha,
                        style = Stroke(width = segment.strokeWidth, cap = ROUND),
                    )
                }

                // Nutrients running the spines, once the mark is mostly there.
                val pulse = pulsePhase(elapsed, dive)
                if (pulse != null) {
                    mark.spineIndices.forEach { i ->
                        val seg = paths[i].segment(pulse, min(pulse + 0.22f, 1f), lengths[i])
                            ?: return@forEach
                        drawPath(
                            path = seg,
                            color = Color.White,
                            alpha = 0.85f * fade,
                            style = Stroke(width = 2.6f, cap = ROUND),
                        )
                    }
                }

                mark.nodes.forEach { node ->
                    val appeared = phase(elapsed, NODE_AT, 0.26f)
                    if (appeared <= 0f) return@forEach
                    // The pupil pushes the nodes out ahead of itself rather
                    // than swallowing them; +7 keeps them clear of the cut.
                    val distance = max(node.distance, pupil + 7f)
                    drawCircle(
                        color = MarkNodeColor,
                        radius = node.radius * appeared.easeOutBack() * fade,
                        center = Offset(
                            MARK_CENTER + kotlin.math.cos(node.angle) * distance,
                            MARK_CENTER + kotlin.math.sin(node.angle) * distance,
                        ),
                    )
                }
            }
        }
    }

    // The ring, closing as the growth reaches the rim.
    val ring = phase(elapsed, RING_AT, RING_FOR)
    if (ring > 0f) {
        val ringFade = dive?.let { 1f - phase(it, RING_FADE_AT, 0.9f) } ?: 1f
        drawArc(
            color = MarkFilament,
            startAngle = -90f,
            sweepAngle = 360f * ring.easeInOutQuad(),
            useCenter = false,
            topLeft = Offset(MARK_CENTER - MARK_RING_RADIUS, MARK_CENTER - MARK_RING_RADIUS),
            size = Size(MARK_RING_RADIUS * 2, MARK_RING_RADIUS * 2),
            alpha = 0.85f * dim * ringFade * fade,
            style = Stroke(width = 2.5f),
        )
    }

    // The spark, before the pupil takes its place.
    val spark = phase(elapsed, 0f, 0.35f)
    val sparkOut = dive?.let { 1f - phase(it, 0f, 0.45f) } ?: 1f
    if (spark > 0f && sparkOut > 0f) {
        drawCircle(
            color = MarkNodeColor,
            radius = 10f * spark.easeOutBack(),
            center = Offset(MARK_CENTER, MARK_CENTER),
            alpha = sparkOut,
        )
    }

    // The pupil's edge. Radius and fade both come off the pupil radius, so it
    // can never drift off the cut.
    if (pupil > 0f) {
        drawCircle(
            color = MarkNodeColor,
            radius = pupil,
            center = Offset(MARK_CENTER, MARK_CENTER),
            alpha = min(pupil / 35f, 1f) * 0.75f * fade,
            style = Stroke(width = 1.2f),
        )
    }
}

// ── the idle prompt ─────────────────────────────────────────────────────────

@Composable
private fun IdlePrompt(
    elapsed: Float,
    diving: Boolean,
    tendril: MycoMark,
    paths: List<Path>,
    lengths: List<Float>,
    modifier: Modifier = Modifier,
) {
    // Follows the mark rather than waiting on a timer of its own.
    val shown = phase(elapsed, PROMPT_AT, 0.45f)
    if (shown <= 0f || diving) return

    val words = phase(elapsed, PROMPT_AT + 0.28f, 0.45f)
    // Then it just breathes, waiting.
    val breathing = if (words >= 1f) 0.72f + 0.28f * (0.5f + 0.5f * sin(elapsed * 2.2f)) else words

    Column(
        modifier = modifier.fillMaxWidth(),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Canvas(
            Modifier
                .fillMaxWidth(0.5f)
                // Sized off the tendril's own proportions. Fixing the height
                // independently of the width let the sprig scale past the
                // bottom of its canvas and grow down over the words.
                .aspectRatio(TENDRIL_WIDTH / TENDRIL_HEIGHT),
        ) {
            // Fit both ways, so a wide phone cannot scale it taller than the
            // box it was given either.
            val fit = min(size.width / TENDRIL_WIDTH, size.height / TENDRIL_HEIGHT)
            scale(fit, fit, Offset.Zero) {
                tendril.segments.forEachIndexed { i, segment ->
                    val drawn = phase(elapsed, PROMPT_AT + segment.generation * 0.1f, 0.3f)
                    if (drawn <= 0f) return@forEachIndexed
                    val head = paths[i].head(drawn.easeOutQuad(), lengths[i]) ?: return@forEachIndexed
                    drawPath(
                        path = head,
                        color = MarkFilament,
                        alpha = segment.alpha,
                        style = Stroke(width = segment.strokeWidth, cap = ROUND),
                    )
                }
            }
        }

        Spacer(Modifier.height(14.dp))

        Text(
            text = "tap to start",
            style = TextStyle(
                color = MarkBright,
                fontSize = 13.sp,
                fontWeight = FontWeight.Normal,
                letterSpacing = 4.4.sp,
                textAlign = TextAlign.Center,
            ),
            modifier = Modifier
                .fillMaxWidth()
                .padding(bottom = 56.dp)
                .graphicsLayer { alpha = breathing },
        )
    }
}

// ── the clock ───────────────────────────────────────────────────────────────
//
// Every beat below is app-relative seconds. They were tuned as a timeline, so
// they read as one: change a number here and only that beat moves.

private const val GEN_STEP = 0.21f          // gap between generations
private const val GEN_DRAW = 0.45f          // how long one filament takes to draw
private const val GEN_SPREAD_CAP = 1.2f     // ceiling on within-generation stagger
private const val GROWTH_AT = 0.2f
private const val NODE_AT = 0.75f
private const val RING_AT = 1.15f
private const val RING_FOR = 0.75f
private const val PULSE_AT = 1.95f

/** When the mark is done and the prompt follows it. */
private const val PROMPT_AT = 2.55f
private const val MARK_SETTLED = PROMPT_AT

private const val BREATH_PERIOD = 4.6f      // a full in-and-out

// Dive-relative, from the moment of the tap.
private const val PUPIL_OPEN_AT = 0.25f
private const val PUPIL_OPEN_FOR = 1.5f
private const val PUPIL_OPEN_TO = 112f
private const val CONSTRICT_AT = 1.8f
private const val CONSTRICT_FOR = 0.28f
private const val CONSTRICT_TO = 72f
private const val DILATE_AT = 2.12f
private const val DILATE_FOR = 1.05f
private const val DILATE_TO = 100f
/** Where a returning launch joins the dive, and how much faster it runs. */
private const val RETURNING_FROM = 2.55f
private const val RETURNING_SPEED = 1.6f

private const val FROST_CLEARS_AT = 2.9f
private const val FROST_CLEARS_FOR = 0.8f
private const val DIVE_AT = 3.3f
private const val DIVE_FOR = 2.2f
private const val DIVE_TO = 175f
private const val DIVE_ZOOM = 15f
private const val RING_FADE_AT = 3.8f
private const val FILAMENT_FADE_AT = 4.0f
private const val FILAMENT_FADE_FOR = 1.6f
private const val DIVE_END = 5.6f

/** How much of the viewport the mark fills. */
private const val MARK_FILL = 1.22f

/**
 * Progress through a window: 0 before [start], 1 after `start + duration`.
 * The timeline is nothing but a stack of these.
 */
private fun phase(now: Float, start: Float, duration: Float): Float =
    ((now - start) / duration).coerceIn(0f, 1f)

private fun List<MarkSegment>.growthOffsets(): List<Float> {
    val perGeneration = groupingBy { it.generation }.eachCount()
    val seen = mutableMapOf<Int, Int>()
    return map { segment ->
        val index = seen.merge(segment.generation, 1, Int::plus)!! - 1
        val count = perGeneration.getValue(segment.generation)
        val spread = min(count * GEN_DRAW * 0.14f, GEN_SPREAD_CAP)
        GROWTH_AT + segment.generation * GEN_STEP + spread * (index.toFloat() / count)
    }
}

private fun pupilRadius(dive: Float?): Float {
    if (dive == null) return 0f
    var r = PUPIL_OPEN_TO * phase(dive, PUPIL_OPEN_AT, PUPIL_OPEN_FOR).easeInOutQuad()
    // Constriction is fast and dilation is slow. That asymmetry is most of
    // what makes it read as an eye rather than a circle.
    val constrict = phase(dive, CONSTRICT_AT, CONSTRICT_FOR).easeOutQuart()
    if (constrict > 0f) r = lerp(r, CONSTRICT_TO, constrict)
    val dilate = phase(dive, DILATE_AT, DILATE_FOR)
    if (dilate > 0f) r = lerp(r, DILATE_TO, dilate)
    val swallow = phase(dive, DIVE_AT, DIVE_FOR).easeInQuad()
    if (swallow > 0f) r = lerp(r, DIVE_TO, swallow)
    return r
}

/**
 * Where the dive is up to, or null before it starts.
 *
 * Returning runs the same schedule from [RETURNING_FROM] — past the pupil
 * opening and its light response, which are only worth watching once — and
 * runs it faster. One timeline, two ways through it, rather than a second
 * animation to keep in step with the first.
 */
private fun diveClock(diveAt: Float?, elapsed: Float, returning: Boolean): Float? {
    val start = diveAt ?: return null
    val t = elapsed - start
    return if (returning) RETURNING_FROM + t * RETURNING_SPEED else t
}

/**
 * How frosted the pupil is: solid until the dive, then gone by the time the
 * hole starts running away with itself.
 */
private fun frost(dive: Float?): Float {
    if (dive == null) return 1f
    return 1f - phase(dive, FROST_CLEARS_AT, FROST_CLEARS_FOR)
}

private fun diveZoom(dive: Float?): Float {
    if (dive == null) return 1f
    return lerp(1f, DIVE_ZOOM, phase(dive, DIVE_AT, DIVE_FOR).easeInQuad())
}

/** −1…1, the mark's slow in-and-out once it has settled. Still during the dive. */
private fun breathPhase(elapsed: Float, dive: Float?): Float {
    if (dive != null) return 0f
    val settled = phase(elapsed, MARK_SETTLED, BREATH_PERIOD * 0.4f)
    if (settled <= 0f) return 0f
    return settled * (0.5f - 0.5f * kotlin.math.cos((elapsed - MARK_SETTLED) * 2f * Math.PI.toFloat() / BREATH_PERIOD))
}

/** Where the nutrient dash currently sits along a spine, or null when they are off. */
private fun pulsePhase(elapsed: Float, dive: Float?): Float? {
    if (dive != null && dive > 0.2f) return null      // snuffed the moment we start in
    if (elapsed < PULSE_AT) return null
    val cycle = ((elapsed - PULSE_AT) / 3.4f) % 1f
    return cycle * 0.78f
}

// ── small helpers ───────────────────────────────────────────────────────────

private val ROUND = StrokeCap.Round

private fun lerp(from: Float, to: Float, t: Float) = from + (to - from) * t

private fun Float.easeOutQuad() = 1f - (1f - this).pow(2)
private fun Float.easeInQuad() = this * this
private fun Float.easeOutQuart() = 1f - (1f - this).pow(4)
private fun Float.easeInOutQuad() =
    if (this < 0.5f) 2f * this * this else 1f - (-2f * this + 2f).pow(2) / 2f

private fun Float.easeOutBack(): Float {
    val c = 1.70158f
    val t = this - 1f
    return 1f + (c + 1f) * t.pow(3) + c * t.pow(2)
}

private fun MarkSegment.toPath() = Path().apply {
    moveTo(x0, y0)
    quadraticTo(cx, cy, x1, y1)
}

private fun Path.measuredLength(): Float =
    PathMeasure().let { it.setPath(this, false); it.length }

/** The first [fraction] of this path, or null if there is nothing to draw yet. */
private fun Path.head(fraction: Float, length: Float): Path? =
    segment(0f, fraction, length)

private fun Path.segment(from: Float, to: Float, length: Float): Path? {
    if (to <= from || length <= 0f) return null
    val out = Path()
    val measure = PathMeasure()
    measure.setPath(this, false)
    measure.getSegment(from * length, to * length, out, true)
    return out
}
