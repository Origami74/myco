package app.myco.share

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The generated name is what people read off each other's screens, so it has to
 * be stable for a given npub and spread thinly enough that two phones in a room
 * don't land on the same one.
 */
class DeviceNameTest {

    private fun npub(i: Int) = "npub1" + "%059x".format(i)

    @Test
    fun `the same npub always generates the same name`() {
        val one = DeviceName.generated(npub(7))
        assertEquals(one, DeviceName.generated(npub(7)))
        assertNotEquals(one, DeviceName.generated(npub(8)))
    }

    @Test
    fun `an empty npub is never given a fabricated name`() {
        assertEquals("new device", DeviceName.generated(""))
    }

    @Test
    fun `it is always two speakable lowercase words`() {
        repeat(200) { i ->
            val name = DeviceName.generated(npub(i))
            val words = name.split(" ")
            assertEquals("'$name' should be two words", 2, words.size)
            assertTrue("'$name' should be lowercase a-z", name.all { it in 'a'..'z' || it == ' ' })
        }
    }

    /**
     * The bound that matters. Drawing 1000 names from a 2048-wide space fills
     * about 2048·(1−e^−1000/2048) ≈ 790 buckets, so anything near that means
     * the generator really is using the whole space. The old 12 × 12 = 144
     * space could not have cleared 144 no matter how good its hash was, and a
     * correlated hash fails this while the word lists stay the same size.
     */
    @Test
    fun `a thousand npubs spread across most of the name space`() {
        val distinct = (0 until 1000).map { DeviceName.generated(npub(it)) }.toSet()
        assertTrue("only ${distinct.size} distinct names from 1000 npubs", distinct.size > 700)
    }

    /** Twenty devices — a big Circle — should almost never collide. */
    @Test
    fun `a circle sized run is collision free`() {
        val names = (0 until 20).map { DeviceName.generated(npub(it)) }
        assertEquals(names.toString(), names.size, names.toSet().size)
    }
}
