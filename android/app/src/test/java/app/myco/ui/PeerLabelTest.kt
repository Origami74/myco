package app.myco.ui

import app.myco.core.AppState
import app.myco.core.CacheStatus
import app.myco.core.PeerDiagnostic
import app.myco.share.DeviceName
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * Peer names are the one thing on these screens a user can check against
 * another phone in their hand, so the precedence has to hold: a name they told
 * us in a neighborhood invitation first, an unauthenticated BLE advert only to fill
 * a gap, and the npub-derived placeholder as a floor that is never stored.
 */
class PeerLabelTest {

    private val them = "npub1" + "%059x".format(1)

    private fun state(
        peers: List<PeerDiagnostic> = emptyList(),
    ) = AppState(
        rev = 1,
        error = "",
        appVersion = "test",
        ownNpub = "npub1" + "%059x".format(99),
        ownPubkeyHex = "",
        nodeAddrHex = "",
        fipsAddr = "",
        fipsIpv6 = "",
        fipsMtu = 0,
        nodeRunning = false,
        nodeStatus = "",
        bleEnabled = false,
        bleRole = "",
        bleScanning = false,
        bleAdapterName = "",
        blePeers = emptyList(),
        bleAdverts = emptyList(),
        wifiAwareEnabled = false,
        wifiAwarePort = 0,
        sites = emptyList(),
        library = emptyList(),
        cache = CacheStatus(relayEvents = 0, blobCount = 0, usedBytes = 0),
        offlineOnly = true,
        peers = peers,
    )

    private fun advert(npub: String, advertisedName: String, linkName: String = "") = PeerDiagnostic(
        key = npub,
        npub = npub,
        nodeAddrHex = "",
        bleAddr = "",
        name = "",
        state = "connected",
        transport = "ble",
        lastSeenMs = 0,
        advertisedName = advertisedName,
        linkName = linkName,
        rssi = null,
        psm = 0,
    )

    @Test
    fun `a name they told us wins over what they broadcast`() {
        val s = state(
            peers = listOf(advert(them, "forged name", linkName = "Redmi Note 8 Pro")),
        )
        assertEquals("Redmi Note 8 Pro", peerLabel(s, them))
    }

    @Test
    fun `an advertised name fills a gap when nothing signed has arrived`() {
        assertEquals("their pixel", peerLabel(state(peers = listOf(advert(them, "their pixel"))), them))
    }

    @Test
    fun `an unknown peer falls back to its npub-derived name, not to nothing`() {
        assertNull(peerNameOrNull(state(), them))
        assertEquals(DeviceName.generated(them), peerLabel(state(), them))
    }

    @Test
    fun `a blank name is treated as no name at all`() {
        val s = state(
            peers = listOf(advert(them, "their pixel", linkName = "   ")),
        )
        assertEquals("their pixel", peerLabel(s, them))
    }
}
