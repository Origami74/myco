package app.myco.onboarding

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * Every Android prompt has one registry entry ([SystemAsk]), one requester
 * (`SystemAsker`) and one explanation UI (`ExplainCard`). These tests hold
 * the registry complete and the rest of the source tree to it, so a prompt
 * can't again be asked from a second place with its own list and no words.
 */
class SystemAskTest {

    // --- the registry ---

    @Test
    fun everyAskIsExplained() {
        for (ask in SystemAsk.entries) {
            val e = ask.explanation
            assertTrue("$ask has no title", e.title.isNotBlank())
            assertTrue("$ask has no body", e.body.isNotBlank())
            assertTrue("$ask has no note", e.note.isNotBlank())
        }
    }

    @Test
    fun everyAskSaysWhatItAsksFor() {
        for (ask in SystemAsk.entries) {
            when (ask.mechanism) {
                AskMechanism.Permissions -> {
                    assertTrue("$ask asks for nothing on 33+", ask.permissions(sdk = 33).isNotEmpty())
                    // Notifications have no runtime permission below 33: the
                    // ask opens Android's notification settings instead.
                    if (ask != SystemAsk.Notifications) {
                        assertTrue("$ask asks for nothing on 29", ask.permissions(sdk = 29).isNotEmpty())
                    }
                    assertNull(ask.intentAction)
                }
                AskMechanism.Intent -> {
                    assertFalse("$ask has no intent", ask.intentAction.isNullOrBlank())
                    assertTrue(ask.permissions(sdk = 33).isEmpty())
                }
                AskMechanism.VpnConsent -> {
                    assertNull(ask.intentAction)
                    assertTrue(ask.permissions(sdk = 33).isEmpty())
                }
            }
        }
        assertEquals(listOf(SystemAsk.Vpn), SystemAsk.entries.filter { it.mechanism == AskMechanism.VpnConsent })
    }

    @Test
    fun nearbyFollowsTheLaneSwitches() {
        val all = SystemAsk.Nearby.permissions(sdk = 33)
        assertEquals(
            (SystemAsk.ble(33) + SystemAsk.wifiNearby(33)).distinct(),
            all,
        )
        assertEquals(SystemAsk.ble(33), SystemAsk.Nearby.permissions(sdk = 33, awareLane = false))
        assertEquals(SystemAsk.wifiNearby(33), SystemAsk.Nearby.permissions(sdk = 33, bleLane = false))
        // Below 31 both halves are fine location: asked once.
        assertEquals(1, SystemAsk.Nearby.permissions(sdk = 29).size)
    }

    @Test
    fun theHotspotAsksOnlyForWifiWhateverTheLanes() {
        for (sdk in listOf(29, 31, 33)) {
            assertEquals(SystemAsk.wifiNearby(sdk), SystemAsk.Hotspot.permissions(sdk, bleLane = false, awareLane = false))
            assertTrue(SystemAsk.Nearby.permissions(sdk).containsAll(SystemAsk.Hotspot.permissions(sdk)))
        }
    }

    // --- the rest of the tree ---

    /** Where each request API may appear, by path under `app/myco`. */
    private val rules: List<Pair<Regex, Set<String>>> = listOf(
        // The registry is the only place a prompt's permissions, consent or intent are named.
        Regex("""Manifest\.permission\.""") to setOf(REGISTRY),
        Regex("""VpnService\.prepare\(""") to setOf(REGISTRY),
        Regex("""ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS""") to setOf(REGISTRY),
        Regex("""ACTION_REQUEST_ENABLE""") to setOf(REGISTRY),
        // The requester is the only place one is launched or its answer read.
        Regex("""RequestPermission|RequestMultiplePermissions""") to setOf(REQUESTER),
        Regex("""requestPermissions\(""") to setOf(REQUESTER),
        Regex("""shouldShowRequestPermissionRationale""") to setOf(REQUESTER),
        // Launchers that ask Android nothing: the Amber signer and file pickers.
        Regex("""registerForActivityResult""") to setOf(REQUESTER, "signer/SignerActivity.kt"),
        Regex("""rememberLauncherForActivityResult""") to setOf(
            "ui/MycoApp.kt", // file picker: send files to a peer
            "ui/screens/CircleScreen.kt", // file picker: hotspot share
            "ui/screens/AccountSettings.kt", // Amber signer login
        ),
        // Only a tap on an explanation makes a Confirmed.
        Regex("""(class|object)\b[^\n]*:\s*Confirmed\b""") to setOf(EXPLAIN_UI),
        Regex("""\bAskButton\(""") to setOf(EXPLAIN_UI, "ui/onboarding/MeshSetupDialog.kt"),
    )

    @Test
    fun promptsAreAskedOnlyThroughTheRequester() {
        val root = File("src/main/java/app/myco")
        assertTrue("run from android/app: ${root.absolutePath}", root.isDirectory)
        val sources = root.walkTopDown().filter { it.isFile && it.extension == "kt" }.toList()
        assertTrue(sources.size > 20)
        val found = sources.map { it.relativeTo(root).invariantSeparatorsPath }.toSet()
        for (path in listOf(REGISTRY, REQUESTER, EXPLAIN_UI)) assertTrue("$path is gone", path in found)

        val violations = mutableListOf<String>()
        for (file in sources) {
            val path = file.relativeTo(root).invariantSeparatorsPath
            file.readLines().forEachIndexed { i, line ->
                val code = line.trim()
                if (code.startsWith("//") || code.startsWith("*") || code.startsWith("/*")) return@forEachIndexed
                for ((pattern, allowed) in rules) {
                    if (path !in allowed && pattern.containsMatchIn(line)) {
                        violations += "$path:${i + 1}: ${pattern.pattern} — ask through SystemAsk / SystemAsker"
                    }
                }
            }
        }
        assertTrue(violations.joinToString("\n"), violations.isEmpty())
    }

    @Test
    fun theRequesterTakesOnlyAConfirmedTap() {
        val asker = File("src/main/java/app/myco/$REQUESTER").readText()
        assertNotNull(Regex("""fun launch\(confirmed: Confirmed\)""").find(asker))
    }

    private companion object {
        const val REGISTRY = "onboarding/SystemAsk.kt"
        const val REQUESTER = "SystemAsker.kt"
        const val EXPLAIN_UI = "ui/onboarding/ExplainCard.kt"
    }
}
