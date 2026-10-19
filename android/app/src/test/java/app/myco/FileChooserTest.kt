package app.myco

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** `accept` → MIME types for the napplet file picker. */
class FileChooserTest {
    @Test
    fun csvAcceptsEveryNameAndroidGivesIt() {
        val mimes = acceptedMimeTypes(arrayOf(".csv,text/csv"))
        assertTrue("text/csv" in mimes)
        assertTrue("text/comma-separated-values" in mimes)
        assertEquals(mimes.size, mimes.toSet().size)
    }

    @Test
    fun nothingOrSomethingUnknownMeansAnyFile() {
        assertEquals(emptyList<String>(), acceptedMimeTypes(null))
        assertEquals(emptyList<String>(), acceptedMimeTypes(arrayOf("")))
        assertEquals(emptyList<String>(), acceptedMimeTypes(arrayOf("csv")))
    }

    @Test
    fun aPlainMimeTypePassesThrough() {
        assertEquals(listOf("image/png"), acceptedMimeTypes(arrayOf("image/png")))
    }
}
