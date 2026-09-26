package app.myco

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Back in a napplet is an Escape the napplet may consume. These pin how its
 * answer is read, and that a napplet consuming every Escape cannot keep the
 * user in it: after three consumed backs with no touch, back closes.
 */
class BackEscapeTrackerTest {
    private val noTouch = 0L

    /** Sends a back at [at] that the napplet consumes (only the key-up comes back). */
    private fun BackEscapeTracker.consumedBack(at: Long, touch: Long = noTouch) {
        assertFalse(shouldCloseDirectly(at, touch))
        sent(at)
        assertEquals(BackEscapeTracker.Report.CONSUMED, unhandled(at, isDown = false, lastTouchAt = touch))
    }

    @Test
    fun an_unhandled_keydown_of_ours_closes() {
        val t = BackEscapeTracker()
        assertFalse(t.shouldCloseDirectly(1_000, noTouch))
        t.sent(1_000)
        assertEquals(BackEscapeTracker.Report.CLOSE, t.unhandled(1_000, isDown = true, lastTouchAt = noTouch))
        assertEquals(BackEscapeTracker.Report.CONSUMED, t.unhandled(1_000, isDown = false, lastTouchAt = noTouch))
    }

    @Test
    fun an_escape_that_is_not_ours_is_left_alone() {
        val t = BackEscapeTracker()
        t.sent(1_000)
        assertEquals(BackEscapeTracker.Report.NOT_OURS, t.unhandled(999, isDown = true, lastTouchAt = noTouch))
        assertEquals(BackEscapeTracker.Report.NOT_OURS, t.unhandled(2_000, isDown = false, lastTouchAt = noTouch))
    }

    @Test
    fun a_keyup_without_its_keydown_counts_as_consumed() {
        val t = BackEscapeTracker(maxConsumed = 1)
        t.consumedBack(1_000)
        assertTrue(t.shouldCloseDirectly(1_100, noTouch))
    }

    @Test
    fun a_keyup_after_an_unhandled_keydown_is_not_consumed() {
        val t = BackEscapeTracker(maxConsumed = 1)
        t.sent(1_000)
        t.unhandled(1_000, isDown = true, lastTouchAt = noTouch)
        t.unhandled(1_000, isDown = false, lastTouchAt = noTouch)
        assertFalse(t.shouldCloseDirectly(1_100, noTouch))
    }

    @Test
    fun three_consumed_backs_then_the_fourth_closes() {
        val t = BackEscapeTracker()
        t.consumedBack(1_000)
        t.consumedBack(2_000)
        t.consumedBack(3_000)
        assertTrue(t.shouldCloseDirectly(4_000, noTouch))
    }

    @Test
    fun a_touch_starts_the_count_over() {
        val t = BackEscapeTracker()
        t.consumedBack(1_000)
        t.consumedBack(2_000)
        t.consumedBack(3_000)
        val touch = 3_500L
        t.consumedBack(4_000, touch)
        t.consumedBack(5_000, touch)
        t.consumedBack(6_000, touch)
        assertTrue(t.shouldCloseDirectly(7_000, touch))
    }

    @Test
    fun an_unanswered_back_counts_toward_the_limit() {
        // A napplet that consumes the key-up too is never heard from.
        val t = BackEscapeTracker()
        for (at in listOf(1_000L, 1_100L, 1_200L)) {
            assertFalse(t.shouldCloseDirectly(at, noTouch))
            t.sent(at)
        }
        assertTrue(t.shouldCloseDirectly(1_300, noTouch))
    }

    @Test
    fun an_unanswered_back_ages_out_but_stays_counted() {
        val t = BackEscapeTracker(maxConsumed = 2)
        t.sent(1_000)
        assertTrue(t.outstanding(1_000 + BackEscapeTracker.OUTSTANDING_MS))
        assertFalse(t.outstanding(1_001 + BackEscapeTracker.OUTSTANDING_MS))
        val later = 1_000 + BackEscapeTracker.OUTSTANDING_MS + 5_000
        assertFalse(t.shouldCloseDirectly(later, noTouch))
        t.sent(later)
        // The aged-out one was consumed, and the fresh one is unanswered.
        assertTrue(t.shouldCloseDirectly(later + 100, noTouch))
        // Its late report is no longer ours.
        assertEquals(BackEscapeTracker.Report.NOT_OURS, t.unhandled(1_000, isDown = true, lastTouchAt = noTouch))
    }

    @Test
    fun a_keydown_reported_unhandled_is_not_outstanding() {
        val t = BackEscapeTracker()
        t.sent(1_000)
        assertTrue(t.outstanding(1_500))
        t.unhandled(1_000, isDown = true, lastTouchAt = noTouch)
        assertFalse(t.outstanding(1_500))
    }
}
