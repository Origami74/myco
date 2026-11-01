package app.myco.hotspot

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** `Range` handling for the hotspot page's `/myco.apk` (a dropped download resumes). */
class ApkRangeTest {

    private fun range(header: String?, size: Long = 1000) = FileShareServer.parseRange(header, size)

    @Test
    fun noHeaderSendsEverything() {
        assertNull(range(null))
    }

    @Test
    fun openEndedResumesFromAByte() {
        assertEquals(400L..999L, range("bytes=400-"))
    }

    @Test
    fun closedRangeIsClampedToTheFile() {
        assertEquals(0L..99L, range("bytes=0-99"))
        assertEquals(900L..999L, range("bytes=900-5000"))
    }

    @Test
    fun suffixRangeTakesTheTail() {
        assertEquals(900L..999L, range("bytes=-100"))
        assertEquals(0L..999L, range("bytes=-5000"))
    }

    @Test
    fun anythingElseFallsBackToTheWholeFile() {
        assertNull(range("bytes=1000-"))       // starts past the end
        assertNull(range("bytes=500-100"))     // backwards
        assertNull(range("bytes=0-1,5-9"))     // several ranges
        assertNull(range("items=0-9"))         // not bytes
        assertNull(range("bytes=abc-"))
        assertNull(range("bytes=-0"))
        assertNull(range("bytes=0-", size = 0))
    }
}
