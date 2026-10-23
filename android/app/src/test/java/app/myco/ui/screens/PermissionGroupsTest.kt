package app.myco.ui.screens

import org.junit.Assert.assertEquals
import org.junit.Test

class PermissionGroupsTest {

    private fun grouped(vararg domains: String) =
        groupPermissions(domains.toList()).map { (group, parts) -> group.title to parts }

    /** Each domain lands in its group, in the groups' order; Basics are left out. */
    @Test
    fun domainsGoToTheirGroups() {
        assertEquals(
            listOf(
                "Post as you" to listOf("relay"),
                "Nearby devices" to listOf("mesh"),
                "Files & media" to listOf("resource", "local"),
                "Profile" to listOf("identity"),
                "Apps & links" to listOf("intent", "inc", "link"),
            ),
            grouped("identity", "inc", "intent", "link", "local", "mesh", "relay", "resource", "theme", "shell"),
        )
    }

    /** A domain this build has never heard of is shown under Other, never dropped. */
    @Test
    fun unknownDomainsGoToOther() {
        assertEquals(
            listOf("Profile" to listOf("identity"), "Other" to listOf("wallet")),
            grouped("identity", "wallet"),
        )
    }

    /** A group is as sensitive as its most sensitive part the app gets. */
    @Test
    fun aGroupTakesItsMostSensitivePart() {
        assertEquals(Tier.Standard, tierOf(listOf("resource", "local")))
        assertEquals(Tier.Sensitive, tierOf(listOf("resource", "fs")))
        assertEquals(Tier.ActsAsYou, tierOf(listOf("resource", "fs", "upload")))
        assertEquals(Tier.ActsAsYou, tierOf(listOf("relay")))
        assertEquals(Tier.Sensitive, tierOf(listOf("mesh")))
        assertEquals(Tier.Standard, tierOf(listOf("identity")))
        // Unknown is treated as sensitive, not waved through as standard.
        assertEquals(Tier.Sensitive, tierOf(listOf("wallet")))
    }
}
