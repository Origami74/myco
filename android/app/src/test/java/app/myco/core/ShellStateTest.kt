package app.myco.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The shell recomposes when [forShell] changes. It must stay equal across polls
 * that only moved diagnostics, and must still change when anything a screen
 * draws from the shell state does.
 */
class ShellStateTest {

    private fun path(lane: String, active: Boolean, rtt: Long?, etx: Double) = PeerPath(
        lane = lane,
        state = "live",
        active = active,
        role = "normal",
        minRttMs = rtt,
        rttSamples = if (rtt == null) 0 else 5,
        etx = etx,
        score = rtt?.let { etx * (1 + it / 100.0) },
    )

    private fun peer(
        key: String,
        lastSeen: Long,
        srtt: Double?,
        rssi: Int?,
        drops: Long,
        rtt: Long?,
        state: String = "connected",
        name: String = "",
    ) = PeerDiagnostic(
        key = key,
        npub = key,
        nodeAddrHex = "",
        bleAddr = "",
        name = name,
        state = state,
        transport = "ble",
        paths = listOf(path("ble", true, rtt, 1.0 + drops / 10.0), path("aware", false, rtt, 1.2)),
        lastSeenMs = lastSeen,
        authenticatedAtMs = lastSeen - 60_000,
        srttMs = srtt,
        rssi = rssi,
        psm = 0x80,
        pairState = "paired",
        inCircle = true,
        role = "central",
        discoveryMs = drops * 3,
        sendDrops = drops,
        attempts = listOf(PeerAttempt(atMs = lastSeen, role = "central", discoveryMs = 40, outcome = "connected")),
    )

    /** One poll. [t] stands for the clock: everything diagnostic moves with it. */
    private fun snapshot(
        t: Long,
        peersOrder: Boolean = true,
        circleName: String = "alice",
        aliceState: String = "connected",
    ): AppState {
        val alice = peer("npub1alice", 1_000 + t, 10.0 + t, -60 - t.toInt(), t, 5 + t, state = aliceState)
        val bob = peer("npub1bob", 2_000 - t, 30.0 + t, null, 2 * t, 20 + t)
        return AppState(
            rev = 7,
            error = "",
            appVersion = "test",
            ownNpub = "npub1me",
            ownPubkeyHex = "",
            nodeAddrHex = "",
            fipsAddr = "",
            fipsIpv6 = "",
            fipsMtu = 1280,
            nodeRunning = true,
            nodeStatus = "running",
            bleEnabled = true,
            bleRole = "peripheral+central",
            bleScanning = true,
            bleAdapterName = "phone",
            blePeers = listOf(BlePeer("aa", "npub1alice", connected = true, psm = 0x80, rssi = -50 - t.toInt())),
            bleAdverts = listOf(BleAdvert("11:22", 0x80, -70 - t.toInt())),
            wifiAwareEnabled = false,
            wifiAwarePort = 0,
            sites = emptyList(),
            library = emptyList(),
            cache = CacheStatus(
                relayEvents = 100 + t,
                blobCount = 10 + t,
                usedBytes = 4096 * t,
                eventCache = CacheTier(count = t, bytes = 50 * t, limit = 1_000_000),
                blobCache = CacheTier(count = 2 * t, bytes = 70 * t, limit = 2_000_000),
            ),
            circle = listOf(CircleContact("npub1alice", circleName, addedAt = 1)),
            reachableNpubs = setOf("npub1alice"),
            pendingPairRequests = emptyList(),
            offlineOnly = false,
            publicNodes = PublicNodes(
                enabled = true,
                nodes = listOf(
                    PublicNode(
                        npub = "npub1pub", name = "test-us01", recommended = true, selected = true,
                        advertised = true, endpoint = "1.2.3.4:2121", advertisedAtMs = 5,
                        state = "connected", srttMs = 80.0 + t, connectedSinceMs = 9, lastError = "",
                    ),
                ),
            ),
            // The core orders peers by last-heard, so they swap as peers speak.
            peers = if (peersOrder) listOf(alice, bob) else listOf(bob, alice),
        )
    }

    @Test
    fun pollsThatOnlyMoveDiagnosticsProjectEqual() {
        val first = snapshot(t = 1)
        val second = snapshot(t = 2, peersOrder = false)
        assertNotEquals("the raw snapshots differ", first, second)
        assertEquals(first.forShell(), second.forShell())
    }

    @Test
    fun aChangeTheShellDrawsStillShows() {
        assertNotEquals(snapshot(t = 1).forShell(), snapshot(t = 1, circleName = "alicia").forShell())
        assertNotEquals(
            snapshot(t = 1).forShell(),
            snapshot(t = 2, aliceState = "paired-offline").forShell(),
        )
    }

    @Test
    fun keepsWhatShellScreensRead() {
        val shell = snapshot(t = 3).forShell()
        // Lane icons and peer lookups still work off the projection.
        val alice = shell.peers.single { it.npub == "npub1alice" }
        assertEquals(listOf("ble" to true, "aware" to false), alice.paths.map { it.lane to it.active })
        assertEquals("connected", alice.state)
        assertTrue(shell.blePeers.single().connected)
        // Budgets and the custom-store flags are settings, not counters.
        assertEquals(1_000_000L, shell.cache.eventCache.limit)
        assertEquals(2_000_000L, shell.cache.blobCache.limit)
        assertEquals("connected", shell.publicNodes.nodes.single().state)
    }

    @Test
    fun projectionIsIdempotent() {
        val shell = snapshot(t = 4).forShell()
        assertEquals(shell, shell.forShell())
    }
}
