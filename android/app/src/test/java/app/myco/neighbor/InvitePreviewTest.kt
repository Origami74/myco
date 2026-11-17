package app.myco.neighbor

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Base64

class InvitePreviewTest {

    private val pk = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    private val json =
        """{"v":1,"link":["http://x"],"pk":"$pk","s":"aa","n":[{"id":"n1","name":"Home"}],"exp":2000000000,"from":"Ann"}"""

    private fun b64(s: String) = Base64.getUrlEncoder().withoutPadding().encodeToString(s.toByteArray())

    private val payload = b64(json)

    @Test
    fun fragmentForm() {
        val text = "https://napplet.link/neighborhood/join#invite=$payload"
        assertEquals(payload, InvitePreview.payload(text))
        val p = InvitePreview.read(text, InvitePreview.Source.Tap)!!
        assertEquals("Ann", p.from)
        assertEquals(2000000000L, p.exp)
        assertEquals(listOf(NeighborhoodsState.Pending("n1", "Home")), p.offered)
        assertEquals(InvitePreview.Source.Tap, p.source)
        assertEquals(text, p.raw)
        assertEquals("0123 · cdef", p.fingerprint)
    }

    @Test
    fun queryForm() {
        assertEquals(payload, InvitePreview.payload("https://napplet.link/neighborhood/join?invite=$payload"))
        assertEquals(payload, InvitePreview.payload("https://napplet.link/neighborhood/join?x=1&invite=$payload#y"))
        assertEquals(payload, InvitePreview.payload("HTTPS://NAPPLET.LINK/neighborhood/join/?invite=$payload"))
        assertNotNull(InvitePreview.read("https://napplet.link/neighborhood/join?invite=$payload"))
    }

    @Test
    fun fragmentWinsOverQuery() {
        assertEquals(payload, InvitePreview.payload("https://napplet.link/neighborhood/join?invite=old#invite=$payload"))
    }

    @Test
    fun nappletScheme() {
        assertEquals(payload, InvitePreview.payload("napplet:neighborhood/join?invite=$payload"))
        assertEquals(payload, InvitePreview.payload("napplet:neighborhood/join#invite=$payload"))
        assertNotNull(InvitePreview.read("napplet:neighborhood/join#invite=$payload"))
    }

    @Test
    fun olderMycoLink() {
        assertEquals("Ann", InvitePreview.read("myco://link/$payload")!!.from)
    }

    @Test
    fun uppercaseKeyAndPercentEncodedValue() {
        val encoded = "%" + "%02X".format(payload[0].code) + payload.drop(1)
        assertEquals(payload, InvitePreview.payload("https://napplet.link/neighborhood/join#INVITE=$encoded"))
        assertNotNull(InvitePreview.read("https://napplet.link/neighborhood/join?Invite=$encoded"))
    }

    @Test
    fun paddedPayloadReads() {
        val padded = Base64.getUrlEncoder().encodeToString(json.toByteArray())
        assertNotNull(InvitePreview.read("https://napplet.link/neighborhood/join#invite=$padded"))
    }

    @Test
    fun otherLinksAreNotInvitations() {
        assertNull(InvitePreview.payload("https://napplet.link/chat/open?invite=$payload"))
        assertNull(InvitePreview.payload("https://napplet.link/neighborhood/joinx?invite=$payload"))
        assertNull(InvitePreview.payload("https://example.com/neighborhood/join#invite=$payload"))
        assertNull(InvitePreview.payload("https://napplet.link/neighborhood/join"))
        assertNull(InvitePreview.payload("https://napplet.link/neighborhood/join#invite="))
        assertNull(InvitePreview.payload("https://napplet.link/neighborhood/join#invites=$payload"))
        assertNull(InvitePreview.payload("myco://share/abc"))
        assertNull(InvitePreview.read("hello"))
    }

    @Test
    fun badBase64() {
        assertNull(InvitePreview.read("https://napplet.link/neighborhood/join#invite=***not base64***"))
        assertNull(InvitePreview.read("https://napplet.link/neighborhood/join#invite=a"))
    }

    @Test
    fun notJson() {
        assertNull(InvitePreview.read("https://napplet.link/neighborhood/join#invite=${b64("hello there")}"))
        assertNull(InvitePreview.read("https://napplet.link/neighborhood/join#invite=${b64("[1,2]")}"))
    }

    @Test
    fun wrongVersion() {
        assertNull(InvitePreview.read("myco://link/${b64("""{"v":2}""")}"))
        assertNull(InvitePreview.read("myco://link/${b64("{}")}"))
    }

    @Test
    fun missingFields() {
        val p = InvitePreview.read("myco://link/${b64("""{"v":1}""")}")!!
        assertEquals("", p.from)
        assertEquals(0L, p.exp)
        assertTrue(p.offered.isEmpty())
        assertEquals("", p.fingerprint)
        assertFalse(p.expired(Long.MAX_VALUE))
    }

    @Test
    fun expiry() {
        val p = InvitePreview.read("myco://link/$payload")!!
        assertFalse(p.expired(1_999_999_999L))
        assertTrue(p.expired(2_000_000_000L))
    }

    @Test
    fun fingerprintNeedsAHexKey() {
        assertEquals("", InvitePreview.fingerprintOf(""))
        assertEquals("", InvitePreview.fingerprintOf("abc"))
        assertEquals("", InvitePreview.fingerprintOf("zzzzzzzzzz"))
        assertEquals("abcd · 6789", InvitePreview.fingerprintOf("ABCD00006789"))
    }
}
