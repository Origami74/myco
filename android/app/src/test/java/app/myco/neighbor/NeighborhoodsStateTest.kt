package app.myco.neighbor

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NeighborhoodsStateTest {

    @Test
    fun nullAndEmpty() {
        assertEquals(NeighborhoodsState(), NeighborhoodsState.parse(null))
        val empty = NeighborhoodsState.parse(JSONObject())
        assertTrue(empty.list.isEmpty())
        assertTrue(empty.links.isEmpty())
        assertTrue(empty.peers.isEmpty())
        assertNull(empty.invite)
        assertEquals("", empty.status)
        assertFalse(empty.nearby.on)
    }

    @Test
    fun full() {
        val s = NeighborhoodsState.parse(
            JSONObject(
                """
                {"list":[{"id":"n1","name":"Home","autoAccept":true,"everyone":false,"links":2,
                          "publishHops":1,"backlogHops":2,"blobHops":3,"passOn":false,
                          "get":["napplet-updates"],"share":["nsite-updates"]}],
                 "links":[{"id":"l1","name":"Bob","neighborhoods":["n1"],
                           "pending":[{"id":"n2","name":"Club"}],"ephemeral":true}],
                 "invite":"https://napplet.link/neighborhood/join#invite=x",
                 "peers":[{"key":"k","name":"Bob","reachable":true,"neighborhoods":["n1"],"linkId":"l1"}],
                 "nearby":{"on":true,"id":"nb","peers":3,"secret":"deadbeef"},
                 "status":"joined"}
                """,
            ),
        )
        val n = s.list.single()
        assertEquals("Home", n.name)
        assertTrue(n.autoAccept)
        assertEquals(2, n.links)
        assertEquals(3, n.blobHops)
        assertFalse(n.passOn)
        assertEquals(setOf("napplet-updates"), n.get)
        assertEquals(setOf("nsite-updates"), n.share)
        val l = s.links.single()
        assertEquals(listOf(NeighborhoodsState.Pending("n2", "Club")), l.pending)
        assertTrue(l.ephemeral)
        assertEquals("https://napplet.link/neighborhood/join#invite=x", s.invite)
        assertEquals("l1", s.peers.single().linkId)
        assertEquals(NeighborhoodsState.Nearby(true, "nb", 3, "deadbeef"), s.nearby)
        assertEquals("joined", s.status)
        assertEquals("Home", s.nameOf("n1"))
    }

    @Test
    fun missingFieldsTakeDefaults() {
        val s = NeighborhoodsState.parse(
            JSONObject("""{"list":[{}],"links":[{}],"peers":[{}],"nearby":{}}"""),
        )
        val n = s.list.single()
        assertEquals("", n.id)
        assertTrue(n.passOn)
        assertTrue(n.get.isEmpty())
        assertTrue(s.links.single().neighborhoods.isEmpty())
        assertNull(s.peers.single().linkId)
        assertEquals(NeighborhoodsState.Nearby(), s.nearby)
    }

    @Test
    fun wrongTypesDoNotThrow() {
        val s = NeighborhoodsState.parse(
            JSONObject(
                """{"list":"nope","links":[1,"x",null],"peers":{"a":1},"invite":"",
                    "nearby":[],"status":5}""",
            ),
        )
        assertTrue(s.list.isEmpty())
        assertTrue(s.links.isEmpty())
        assertTrue(s.peers.isEmpty())
        assertNull(s.invite)
        assertEquals(NeighborhoodsState.Nearby(), s.nearby)
        assertEquals("5", s.status)

        val odd = NeighborhoodsState.parse(
            JSONObject("""{"list":[{"id":7,"links":"many","autoAccept":"yes","get":"x"}]}"""),
        )
        val n = odd.list.single()
        assertEquals("7", n.id)
        assertEquals(0, n.links)
        assertFalse(n.autoAccept)
        assertTrue(n.get.isEmpty())
    }

    @Test
    fun nameOfFallsBackToShortId() {
        assertEquals("abcdefgh", NeighborhoodsState().nameOf("abcdefghijkl"))
    }
}
