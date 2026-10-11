package app.myco.update

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class StableReleaseTest {
    private fun release(tag: String = "v0.10.0", draft: Boolean = false,
        prerelease: Boolean = false, assets: String = """[{"name":"myco-v0.10.0-arm64.apk","state":"uploaded"}]""") =
        JSONObject("""{"tag_name":"$tag","draft":$draft,"prerelease":$prerelease,"assets":$assets}""")

    @Test fun acceptsStableReleaseWithUploadedApk() {
        assertEquals("v0.10.0", stableReleaseTag(release()))
    }
    @Test fun rejectsDraftsPrereleasesAndMissingApks() {
        listOf(release(draft = true), release(prerelease = true), release(tag = "v0.11.0-rc.1"),
            release(assets = "[]"), release(assets = """[{"name":"source.zip","state":"uploaded"}]"""),
            release(assets = """[{"name":"myco.apk","state":"new"}]""")).forEach {
            assertThrows(IllegalArgumentException::class.java) { stableReleaseTag(it) }
        }
    }
}
