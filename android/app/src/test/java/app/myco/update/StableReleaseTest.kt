package app.myco.update

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class StableReleaseTest {
    private fun event(version: String = "0.12.0", app: String = "app.myco", channel: String = "main") =
        JSONObject("""{"created_at":100,"tags":[["i","$app"],["d","$app@$version"],["version","$version"],["c","$channel"],["e","${"a".repeat(64)}"]]}""")

    @Test fun acceptsAuthenticatedStableAnnouncement() {
        assertEquals("0.12.0", releaseTag(event(), { true }, 100))
    }
    @Test fun rejectsUnauthenticatedOrWrongAppOrChannel() {
        assertNull(releaseTag(event(), { false }, 100))
        assertNull(releaseTag(event(app = "other.app"), { true }, 100))
        assertNull(releaseTag(event(channel = "beta"), { true }, 100))
    }
    @Test fun rejectsPrereleaseFutureAndMissingAssetReference() {
        assertNull(releaseTag(event(version = "0.12.0-rc.1"), { true }, 100))
        assertNull(releaseTag(event().put("created_at", 1000), { true }, 100))
        val missing = event()
        missing.getJSONArray("tags").remove(4)
        assertNull(releaseTag(missing, { true }, 100))
    }
}
