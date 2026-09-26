package app.myco.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Test

class NappletAddressTest {
    // Author bytes 0x01..0x20; the naddr is kind 35129, d = "chat".
    private val npub = "npub1qypqxpq9qcrsszg2pvxq6rs0zqg3yyc5z5tpwxqergd3c8g7rusqdknev3"
    private val naddr =
        "naddr1qqzxx6rpwspzqqgzqvzq2ps8pqys5zcvp58q7yq3zgf3g9gkzuvpjxsmrsw3u8eqqvzqqqyf8y3clxd0"
    private val hex = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20"

    @Test
    fun bothSpellingsNameTheSameNapplet() {
        val expected = NappletAddress(hex, "chat")
        assertEquals(expected, NappletAddress.parse(naddr))
        assertEquals(expected, NappletAddress.parse("nostr:$naddr"))
        assertEquals(expected, NappletAddress.parse("$npub:chat"))
        assertEquals(expected, NappletAddress.of(npub, "chat"))
    }

    @Test
    fun anotherDTagIsAnotherNapplet() {
        assertNotEquals(NappletAddress.parse(naddr), NappletAddress.parse("$npub:other"))
        assertEquals(NappletAddress(hex, null), NappletAddress.parse(npub))
    }

    @Test
    fun garbageIsNotAnAddress() {
        assertNull(NappletAddress.parse("naddr1added"))
        assertNull(NappletAddress.parse("https://example.com"))
        assertNull(NappletAddress.parse("npub1zz:chat"))
    }
}
