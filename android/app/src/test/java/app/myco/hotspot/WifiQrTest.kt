package app.myco.hotspot

import org.junit.Assert.assertEquals
import org.junit.Test

class WifiQrTest {

    @Test
    fun plainCredentialsPassThrough() {
        assertEquals(
            "WIFI:T:WPA;S:AndroidShare_1234;P:hunter22;;",
            WifiQr.payload("AndroidShare_1234", "hunter22"),
        )
    }

    @Test
    fun reservedCharactersAreEscaped() {
        // Every reserved char of the WIFI: format: \ ; , " :
        assertEquals("""a\\b\;c\,d\"e\:f""", WifiQr.escape("""a\b;c,d"e:f"""))
        assertEquals(
            """WIFI:T:WPA;S:my\;net;P:p\:a\,s\"s\\w;;""",
            WifiQr.payload("""my;net""", """p:a,s"s\w"""),
        )
    }

    @Test
    fun unicodeSurvivesUnchanged() {
        assertEquals("WIFI:T:WPA;S:Ünïcode ✓;P:pässwörd;;", WifiQr.payload("Ünïcode ✓", "pässwörd"))
    }
}
