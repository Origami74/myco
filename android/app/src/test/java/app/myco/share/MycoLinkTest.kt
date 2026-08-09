package app.myco.share

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class MycoLinkTest {

    private val host = "5rnyv42gd5hia53curyrl58o9vnouebwyt4gxdice14ialcs4vcahmls"

    @Test
    fun `parses an app link with a deep path`() {
        val link = MycoLink.parseAppLink("myco://app/$host/dumpling/dmpl1qqqsyqcyq5rqwzqf")
        assertEquals(host, link?.host)
        assertEquals("/dumpling/dmpl1qqqsyqcyq5rqwzqf", link?.path)
    }

    @Test
    fun `a bare app link opens the app at its root`() {
        assertEquals("/", MycoLink.parseAppLink("myco://app/$host")?.path)
        assertEquals("/", MycoLink.parseAppLink("myco://app/$host/")?.path)
    }

    @Test
    fun `keeps the query and fragment for the app to read`() {
        val link = MycoLink.parseAppLink("myco://app/$host/dumpling/abc?ref=qr#top")
        assertEquals("/dumpling/abc?ref=qr#top", link?.path)
    }

    @Test
    fun `a query with no path still lands on the root`() {
        assertEquals("/?ref=qr", MycoLink.parseAppLink("myco://app/$host?ref=qr")?.path)
    }

    @Test
    fun `lowercases the host so an uppercased QR still resolves`() {
        // QR alphanumeric mode can only carry uppercase.
        val link = MycoLink.parseAppLink("MYCO://APP/${host.uppercase()}/Invite/Abc")
        assertEquals(host, link?.host)
        // The path's case is the app's business, so it is left verbatim.
        assertEquals("/Invite/Abc", link?.path)
    }

    @Test
    fun `rejects the other myco schemes and junk`() {
        assertNull(MycoLink.parseAppLink("myco://pair/eyJ2IjoxfQ"))
        assertNull(MycoLink.parseAppLink("myco://share/eyJ2IjoxfQ"))
        assertNull(MycoLink.parseAppLink("https://example.com/app/$host"))
        assertNull(MycoLink.parseAppLink("myco://app/"))
        assertNull(MycoLink.parseAppLink(""))
    }

    @Test
    fun `rejects a host that is not a usable label`() {
        // It becomes `<host>.localhost` in the WebView, so it must be a DNS label.
        assertNull(MycoLink.parseAppLink("myco://app/has_underscore/x"))
        assertNull(MycoLink.parseAppLink("myco://app/-leading/x"))
        assertNull(MycoLink.parseAppLink("myco://app/trailing-/x"))
        assertNull(MycoLink.parseAppLink("myco://app/${"a".repeat(64)}/x"))
    }

    @Test
    fun `rejects a path carrying control characters`() {
        assertNull(MycoLink.parseAppLink("myco://app/$host/in\nvite"))
        assertNull(MycoLink.parseAppLink("myco://app/$host/in vite"))
        assertNull(MycoLink.parseAppLink("myco://app/$host/in\\vite"))
    }

    @Test
    fun `builds links that parse back`() {
        val built = MycoLink.buildAppLink(host, "/dumpling/dmpl1abc")
        assertEquals("myco://app/$host/dumpling/dmpl1abc", built)
        assertEquals("/dumpling/dmpl1abc", MycoLink.parseAppLink(built)?.path)
        assertEquals("myco://app/$host", MycoLink.buildAppLink(host, "/"))
    }
}
