package app.myco.update

import org.junit.Assert.*
import org.junit.Test

class ReleaseVersionTest {
    @Test fun comparesNumbersRatherThanStrings() {
        assertTrue(isNewerRelease("0.9.1", "v0.10.0"))
        assertTrue(isNewerRelease("0.10.9", "v0.10.10"))
        assertTrue(isNewerRelease("0.99.99", "v1.0.0"))
        assertFalse(isNewerRelease("0.10.0", "v0.9.9"))
        assertFalse(isNewerRelease("0.10.0", "v0.10.0"))
    }
    @Test fun rejectsMalformedAndPrereleaseVersions() {
        listOf("", "latest", "0.10", "v0.10.0-rc.1", "01.2.3", "1.2.-3", "1.2.3/evil",
            "999999999999999999999.0.0").forEach {
            assertNull(it, ReleaseVersion.parse(it))
            assertFalse(isNewerRelease("0.1.0", it))
        }
        assertFalse(isNewerRelease("dev", "v1.0.0"))
    }
    @Test fun ignoresBuildMetadata() {
        assertFalse(isNewerRelease("1.2.3+build.1", "v1.2.3+build.2"))
        assertTrue(isNewerRelease("1.2.3+build.1", "v1.2.4"))
    }
    @Test fun rateLimitAllowsFirstCheckAndClockRollback() {
        assertTrue(updateCheckDue(1000, 0, 100))
        assertFalse(updateCheckDue(1000, 950, 100))
        assertTrue(updateCheckDue(1050, 950, 100))
        assertTrue(updateCheckDue(900, 950, 100))
    }
}
