package app.myco.aware

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * The pool decides which UDP socket carries which peer, and a socket can be
 * marked for exactly one data path — so handing two peers the same slot is not
 * a bookkeeping slip, it is one of them silently going dark.
 */
class AwareSlotPoolTest {

    private fun npub(i: Int) = "npub1" + "%058x".format(i)

    @Test
    fun `each peer gets a slot of its own`() {
        val pool = AwareSlotPool(4)
        val slots = (0 until 4).map { pool.acquire(npub(it)) }
        assertEquals(listOf(0, 1, 2, 3), slots)
        assertEquals(0, pool.free())
    }

    @Test
    fun `asking again returns the same slot without spending the pool`() {
        val pool = AwareSlotPool(4)
        val first = pool.acquire(npub(1))
        assertEquals(first, pool.acquire(npub(1)))
        assertEquals(first, pool.slotOf(npub(1)))
        // The identity exchange asks on every message; only one slot is held.
        assertEquals(3, pool.free())
    }

    /**
     * A full pool must say so. Falling back to any slot would advertise a port
     * whose socket is marked for another peer's data path — the packets arrive
     * and the replies leave down the wrong network.
     */
    @Test
    fun `a full pool refuses rather than reusing a slot`() {
        val pool = AwareSlotPool(2)
        pool.acquire(npub(1))
        pool.acquire(npub(2))
        assertNull(pool.acquire(npub(3)))
        assertNull(pool.slotOf(npub(3)))
    }

    @Test
    fun `a released slot is handed to the next peer`() {
        val pool = AwareSlotPool(2)
        pool.acquire(npub(1))
        val second = pool.acquire(npub(2))
        pool.release(npub(1))
        assertEquals(0, pool.acquire(npub(3)))
        // The peer that kept its data path keeps its socket.
        assertEquals(second, pool.slotOf(npub(2)))
    }

    @Test
    fun `releasing a peer that holds nothing is harmless`() {
        val pool = AwareSlotPool(2)
        pool.acquire(npub(1))
        pool.release(npub(9))
        pool.release(npub(9))
        assertEquals(0, pool.slotOf(npub(1)))
        assertEquals(1, pool.free())
    }

    @Test
    fun `the lane going down frees everything`() {
        val pool = AwareSlotPool(3)
        pool.acquire(npub(1))
        pool.acquire(npub(2))
        pool.clear()
        assertEquals(3, pool.free())
        assertEquals(0, pool.inUse())
        assertEquals(0, pool.acquire(npub(2)))
    }

    /**
     * Slot 0 is the base port — what a peer discovered before its identity is
     * known is told — so the first peer in the room needs no correction at all.
     */
    @Test
    fun `the first peer lands on slot zero`() {
        assertEquals(0, AwareSlotPool(4).acquire(npub(7)))
    }

    @Test
    fun `two peers never share a socket`() {
        val pool = AwareSlotPool(4)
        assertNotEquals(pool.acquire(npub(1)), pool.acquire(npub(2)))
    }
}
