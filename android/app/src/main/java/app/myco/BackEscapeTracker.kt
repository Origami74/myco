package app.myco

/**
 * The bookkeeping behind a napplet's back gesture, kept free of Android so it
 * can be unit-tested. See `NappletActivity.onBack` for the mechanism.
 *
 * Back is sent to the napplet as an Escape key-down and key-up, identified by
 * their shared `downTime`. WebView reports only the keys the page did **not**
 * consume, so each back resolves as one of:
 *
 * - key-down reported unhandled: the napplet left back to Myco — close;
 * - key-up reported, key-down never: the napplet consumed the back;
 * - nothing reported within [outstandingMs]: also counted as consumed (a
 *   napplet swallowing both keys), or a renderer too busy to answer.
 *
 * **Anti-trap rule.** A napplet that consumes every Escape would make back
 * never leave it. So at most [maxConsumed] backs in a row may be consumed
 * without the user touching the napplet in between; the next one closes the
 * window without asking the napplet. Enough for walking back up a few nested
 * pages, not enough to hold the user hostage. A touch starts the count over.
 *
 * Times are `SystemClock.uptimeMillis()` values. Not thread-safe: main thread.
 */
internal class BackEscapeTracker(
    private val maxConsumed: Int = MAX_CONSUMED,
    private val outstandingMs: Long = OUTSTANDING_MS,
) {
    private class Pending(val downTime: Long, var downUnhandled: Boolean = false)

    private val pending = ArrayDeque<Pending>()

    /** Backs consumed since [countedTouch]. */
    private var consumed = 0

    /** The touch the [consumed] count belongs to. */
    private var countedTouch = Long.MIN_VALUE

    /** What a reported unhandled Escape means for the window. */
    enum class Report {
        /** Not an Escape a back sent: leave it to WebView's default. */
        NOT_OURS,

        /** The napplet left back to Myco: leave the window (to the background). */
        LEAVE,

        /** One of ours, nothing to do. */
        CONSUMED,
    }

    /**
     * A back arrived. True when it must leave the window directly — the
     * napplet has already consumed [maxConsumed] backs since the last touch —
     * rather than be offered to the napplet.
     */
    fun shouldLeaveDirectly(now: Long, lastTouchAt: Long): Boolean {
        syncTouch(lastTouchAt)
        expire(now, lastTouchAt)
        val unanswered = pending.count { !it.downUnhandled && it.downTime >= lastTouchAt }
        return consumed + unanswered >= maxConsumed
    }

    /** An Escape pair with this `downTime` was sent for a back. */
    fun sent(downTime: Long) {
        pending.addLast(Pending(downTime))
    }

    /** WebView reported an Escape with [downTime] unhandled, [isDown] or up. */
    fun unhandled(downTime: Long, isDown: Boolean, lastTouchAt: Long): Report {
        val entry = pending.firstOrNull { it.downTime == downTime } ?: return Report.NOT_OURS
        if (isDown) {
            entry.downUnhandled = true
            return Report.LEAVE
        }
        pending.remove(entry)
        if (!entry.downUnhandled) countConsumed(entry, lastTouchAt)
        return Report.CONSUMED
    }

    /**
     * The window was sent to the background: forget what is in flight and
     * start the count over, so coming back to it is a fresh start — not a
     * back that leaves again at once.
     */
    fun reset() {
        pending.clear()
        consumed = 0
        countedTouch = Long.MIN_VALUE
    }

    /** A back was sent recently and the page has not answered it yet. */
    fun outstanding(now: Long): Boolean =
        pending.any { !it.downUnhandled && now - it.downTime <= outstandingMs }

    private fun syncTouch(lastTouchAt: Long) {
        if (lastTouchAt != countedTouch) {
            countedTouch = lastTouchAt
            consumed = 0
        }
    }

    private fun expire(now: Long, lastTouchAt: Long) {
        while (pending.isNotEmpty() && now - pending.first().downTime > outstandingMs) {
            val entry = pending.removeFirst()
            if (!entry.downUnhandled) countConsumed(entry, lastTouchAt)
        }
    }

    private fun countConsumed(entry: Pending, lastTouchAt: Long) {
        syncTouch(lastTouchAt)
        // A back from before the latest touch belongs to a count already reset.
        if (entry.downTime >= lastTouchAt) consumed++
    }

    companion object {
        /** Consumed backs allowed in a row without a touch. */
        const val MAX_CONSUMED = 3

        /**
         * How long a back's Escape counts as unanswered. Longer than WebView's
         * unresponsive-renderer timeout, so a napplet hung in a script is
         * closed by the back that found it hung.
         */
        const val OUTSTANDING_MS = 10_000L
    }
}
