package app.myco.ui

import app.myco.core.AppState
import app.myco.core.CacheStatus
import app.myco.core.CircleContact
import app.myco.core.OutboundPair
import app.myco.core.PairRequest
import app.myco.core.PeerDiagnostic
import app.myco.share.DeviceName
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * Peer names are the one thing on these screens a user can check against
 * another phone in their hand, so the precedence has to hold: a name they told
 * us over signed pair traffic first, an unauthenticated BLE advert only to fill
 * a gap, and the npub-derived placeholder as a floor that is never stored.
 */
class PeerLabelTest {

    private val them = "npub1" + "%059x".format(1)

    private fun state(
        circle: List<CircleContact> = emptyList(),
        pending: List<PairRequest> = emptyList(),
        outbound: List<OutboundPair> = emptyList(),
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
        circle = circle,
        discovered = emptyList(),
        pendingPairRequests = pending,
        outboundPairs = outbound,
        offlineOnly = true,
        peers = peers,
    )

    private fun advert(npub: String, advertisedName: String) = PeerDiagnostic(
        key = npub,
        npub = npub,
        nodeAddrHex = "",
        bleAddr = "",
        name = "",
        state = "connected",
        transport = "ble",
        lastSeenMs = 0,
        advertisedName = advertisedName,
        rssi = null,
        psm = 0,
        pairState = "",
        inCircle = false,
    )

    @Test
    fun `a name they told us wins over what they broadcast`() {
        val s = state(
            circle = listOf(CircleContact(npub = them, name = "Redmi Note 8 Pro", addedAt = 0)),
            peers = listOf(advert(them, "forged name")),
        )
        assertEquals("Redmi Note 8 Pro", peerLabel(s, them))
    }

    @Test
    fun `an advertised name fills a gap when nothing signed has arrived`() {
        assertEquals("their pixel", peerLabel(state(peers = listOf(advert(them, "their pixel"))), them))
    }

    @Test
    fun `an invite recorded without a name does not become that peer's name`() {
        // The nearby-tap invite used to store *our own* device name here, which
        // peerLabel then read back as the name they had told us.
        val s = state(
            outbound = listOf(OutboundPair(npub = them, name = "", since = 0)),
            peers = listOf(advert(them, "their pixel")),
        )
        assertNull(peerNameOrNull(state(outbound = listOf(OutboundPair(them, "", 0))), them))
        assertEquals("their pixel", peerLabel(s, them))
    }

    @Test
    fun `an unknown peer falls back to its npub-derived name, not to nothing`() {
        assertNull(peerNameOrNull(state(), them))
        assertEquals(DeviceName.generated(them), peerLabel(state(), them))
    }

    @Test
    fun `a blank name is treated as no name at all`() {
        val s = state(
            circle = listOf(CircleContact(npub = them, name = "   ", addedAt = 0)),
            peers = listOf(advert(them, "their pixel")),
        )
        assertEquals("their pixel", peerLabel(s, them))
    }
}
