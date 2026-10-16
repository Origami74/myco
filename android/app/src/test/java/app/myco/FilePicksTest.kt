package app.myco

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class FilePicksTest {
    @Test
    fun thePickerFiltersOnWhatRustSentOrOnEverything() {
        assertArrayEquals(arrayOf("*/*"), FilePicks.pickerMimeTypes(emptyList()))
        assertArrayEquals(arrayOf("*/*"), FilePicks.pickerMimeTypes(listOf("", "nonsense")))
        assertArrayEquals(
            arrayOf("text/plain", "text/markdown"),
            FilePicks.pickerMimeTypes(listOf("text/plain", "text/markdown")),
        )
    }

    @Test
    fun anExistingDocumentIsNeverOpenedInAModeThatMayTruncateIt() {
        for (mode in listOf(FilePick.Mode.OPEN, FilePick.Mode.OPEN_MULTIPLE)) {
            assertEquals(listOf("r"), FilePicks.openModes(mode, write = false))
            assertEquals(listOf("rw", "r"), FilePicks.openModes(mode, write = true))
            assertFalse(FilePicks.openModes(mode, write = true).contains("w"))
        }
        // A save target was just created empty: "w" is only a fallback there.
        assertEquals(listOf("rw", "w"), FilePicks.openModes(FilePick.Mode.SAVE, write = true))
    }

    @Test
    fun whatAModeOpenedIsWhatRustIsTold() {
        assertTrue(FilePicks.readable("r") && !FilePicks.writable("r"))
        assertTrue(FilePicks.readable("rw") && FilePicks.writable("rw"))
        assertTrue(!FilePicks.readable("w") && FilePicks.writable("w"))
    }
}
