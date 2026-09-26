package app.myco

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class RestartPromptTest {

    /** A store that outlives the prompt reading it, as SharedPreferences outlives a process. */
    private class MapStore : RestartPrompt.Store {
        val map = HashMap<String, String>()
        override fun get(napplet: String): String? = map[napplet]
        override fun put(napplet: String, version: String) {
            map[napplet] = version
        }
    }

    @Test
    fun aCurrentWindowIsNeverAsked() {
        assertFalse(RestartPrompt(MapStore()).shouldAsk("chat.napplet.localhost", null))
    }

    @Test
    fun aNewVersionIsOfferedOnce() {
        val prompt = RestartPrompt(MapStore())
        assertTrue(prompt.shouldAsk("chat.napplet.localhost", "v2"))
        prompt.markAsked("chat.napplet.localhost", "v2")
        assertFalse(prompt.shouldAsk("chat.napplet.localhost", "v2"))
    }

    @Test
    fun theAnswerSurvivesARestartOfTheProcess() {
        val store = MapStore()
        RestartPrompt(store).markAsked("chat.napplet.localhost", "v2")
        assertFalse(RestartPrompt(store).shouldAsk("chat.napplet.localhost", "v2"))
    }

    @Test
    fun aLaterUpdateAsksAgain() {
        val prompt = RestartPrompt(MapStore())
        prompt.markAsked("chat.napplet.localhost", "v2")
        assertTrue(prompt.shouldAsk("chat.napplet.localhost", "v3"))
    }

    @Test
    fun eachNappletIsAskedForItself() {
        val prompt = RestartPrompt(MapStore())
        prompt.markAsked("chat.napplet.localhost", "v2")
        assertTrue(prompt.shouldAsk("notes.napplet.localhost", "v2"))
    }
}
