package app.myco.ui.intro

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import kotlin.math.hypot
import org.junit.Test

class MycoMarkTest {

    @Test
    fun `same seed grows the same mark`() {
        val a = buildMycoMark(seed = 7)
        val b = buildMycoMark(seed = 7)

        // The mark must not reshuffle across recomposition or a rotation, so
        // the generator has to be a pure function of its seed.
        assertEquals(a.segments, b.segments)
        assertEquals(a.nodes, b.nodes)
    }

    @Test
    fun `different seeds grow different marks`() {
        assertTrue(buildMycoMark(seed = 7).segments != buildMycoMark(seed = 8).segments)
    }

    @Test
    fun `the mark grows out to the ring and is trimmed by it`() {
        val mark = buildMycoMark()
        val outside = mark.segments.count { it.endRadius > MARK_RING_RADIUS }
        val furthest = mark.segments.maxOf { it.endRadius }

        // Branch lengths are jittered, so the outermost filaments land either
        // side of the ring and the draw clips the ones that overshoot. That
        // clip is load-bearing — without it the mark frays outward instead of
        // ending on a line — so this asserts it has work to do…
        assertTrue("nothing reaches the ring; the clip is doing nothing", outside > 0)
        // …and that the overshoot stays a trim rather than a large hidden
        // halo we would be paying to generate and stroke every frame.
        assertTrue(
            "filaments run to $furthest, well past the ring at $MARK_RING_RADIUS",
            furthest < MARK_RING_RADIUS * 1.4f,
        )
        // The mark should fill its disc, not hug the middle.
        assertTrue("mark only reaches $furthest", furthest > MARK_RING_RADIUS * 0.85f)
    }

    @Test
    fun `a segment is always generated after the one it grows from`() {
        // Growth propagates along the chain, so a parent must already exist —
        // and already be drawing — by the time its children are scheduled.
        val mark = buildMycoMark()

        mark.segments.forEachIndexed { index, segment ->
            assertTrue(
                "segment $index has parent ${segment.parent}",
                segment.parent < index,
            )
        }
    }

    @Test
    fun `generations are contiguous and start at the trunk`() {
        val mark = buildMycoMark()
        val byGeneration = mark.segments.groupBy { it.generation }

        assertEquals(4, byGeneration.getValue(0).size)     // one trunk per arm
        for (generation in 0 until mark.generations) {
            assertTrue("generation $generation is empty", byGeneration.containsKey(generation))
        }
        // Each generation branches, so it is never smaller than the last.
        for (generation in 1 until mark.generations) {
            assertTrue(
                "generation $generation is smaller than ${generation - 1}",
                byGeneration.getValue(generation).size >= byGeneration.getValue(generation - 1).size,
            )
        }
    }

    @Test
    fun `child segments start where their parent ended`() {
        // Any drift here would show as filaments floating free of the branch
        // that feeds them, which is exactly what the growth is meant to avoid.
        val mark = buildMycoMark()

        mark.segments.filter { it.parent >= 0 }.forEach { segment ->
            val parent = mark.segments[segment.parent]
            assertEquals(parent.x1, segment.x0, 0.001f)
            assertEquals(parent.y1, segment.y0, 0.001f)
        }
    }

    @Test
    fun `one node per arm, out where the trunk first forks`() {
        val mark = buildMycoMark()

        assertEquals(4, mark.nodes.size)
        mark.nodes.forEach { node ->
            assertTrue("node at ${node.distance} is inside the spark", node.distance > 40f)
            assertTrue("node at ${node.distance} is past the ring", node.distance < MARK_RING_RADIUS)
            // Polar position has to agree with the cartesian one, or the pupil
            // would shunt the node off its own filament as it pushes it out.
            assertEquals(node.distance, hypot(node.x - MARK_CENTER, node.y - MARK_CENTER), 0.01f)
        }
    }

    @Test
    fun `nutrients have spines to run along`() {
        val mark = buildMycoMark()
        val spines = mark.spineIndices

        assertTrue("no spines to pulse along", spines.isNotEmpty())
        spines.forEach { assertTrue(mark.segments[it].generation < 2) }
    }

    @Test
    fun `the tendril hangs below its root`() {
        val tendril = buildTendril()

        assertTrue(tendril.segments.isNotEmpty())
        // It grows downward out of the mark, so nothing may end above the root.
        tendril.segments.forEach { assertTrue(it.y1 > 0f) }
        assertTrue(tendril.segments.all { it.y1 <= TENDRIL_HEIGHT * 1.5f })
    }
}
