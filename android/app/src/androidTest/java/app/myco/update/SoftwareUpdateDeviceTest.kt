package app.myco.update

import android.app.NotificationManager
import android.content.Context
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkInfo
import androidx.work.WorkManager
import kotlinx.coroutines.runBlocking
import java.util.concurrent.TimeUnit

/** Live relay + native signature + OS worker/notification proof on an isolated package. */
class SoftwareUpdateDeviceTest {
    @Test
    fun testBackgroundReleaseNotificationAndDeduplication() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        check(context.packageName == "app.myco.updatetest") { "Use the isolated update test build" }
        val prefs = context.getSharedPreferences("signed_software_updates", Context.MODE_PRIVATE)
        val notifications = context.getSystemService(NotificationManager::class.java)
        notifications.cancel(4001)
        prefs.edit().clear().putBoolean("automatic", true)
            .putBoolean("offline_only", false).commit()

        val manager = WorkManager.getInstance(context)
        manager.cancelAllWork().result.get(10, TimeUnit.SECONDS)
        val request = OneTimeWorkRequestBuilder<SoftwareUpdateWorker>().build()
        manager.enqueue(request).result.get(10, TimeUnit.SECONDS)
        val deadline = System.currentTimeMillis() + 45_000
        var state: WorkInfo.State? = null
        while (System.currentTimeMillis() < deadline) {
            state = manager.getWorkInfoById(request.id).get(5, TimeUnit.SECONDS)?.state
            if (state?.isFinished == true) break
            Thread.sleep(200)
        }
        assertEquals(WorkInfo.State.SUCCEEDED, state)
        val version = prefs.getString("available_tag", null)
        assertNotNull("Live signed release must be discovered", version)
        assertTrue(isNewerRelease("0.10.0", version!!))
        assertEquals(version, prefs.getString("notified_tag", null))
        val notification = notifications.activeNotifications.single { it.id == 4001 }
        assertEquals("Myco update available", notification.notification.extras.getString("android.title"))
        val postedAt = notification.postTime

        val updates = SoftwareUpdates(context)
        runBlocking { updates.check(offlineOnly = false, force = true) }
        assertTrue("Repeated live query must succeed", prefs.getBoolean("last_success", false))
        assertEquals("Repeated checks must not repost", postedAt,
            notifications.activeNotifications.single { it.id == 4001 }.postTime)

        val lastAttempt = prefs.getLong("last_attempt", 0)
        updates.configureBackground(offlineOnly = true)
        runBlocking { updates.check(offlineOnly = true, force = true) }
        assertEquals(lastAttempt, prefs.getLong("last_attempt", 0))
        assertEquals("Turn off mesh-only mode to check online", updates.state.value.message)
        updates.configureBackground(offlineOnly = false)
        updates.setAutomatic(false)
        runBlocking { updates.check(offlineOnly = false) }
        assertEquals(lastAttempt, prefs.getLong("last_attempt", 0))
    }

    @Test
    fun testBlockedNotificationsKeepCandidateAvailable() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        check(context.packageName == "app.myco.updatetest")
        val notifications = context.getSystemService(NotificationManager::class.java)
        org.junit.Assume.assumeFalse(notifications.areNotificationsEnabled())
        val prefs = context.getSharedPreferences("signed_software_updates", Context.MODE_PRIVATE)
        notifications.cancel(4001)
        prefs.edit().clear().putBoolean("automatic", false)
            .putBoolean("offline_only", false).commit()
        val updates = SoftwareUpdates(context)
        runBlocking { updates.check(offlineOnly = false, force = true) }
        assertTrue(prefs.getBoolean("last_success", false))
        assertNotNull(updates.state.value.availableTag)
        assertNull(prefs.getString("notified_tag", null))
        assertTrue(notifications.activeNotifications.none { it.id == 4001 })
    }
}
