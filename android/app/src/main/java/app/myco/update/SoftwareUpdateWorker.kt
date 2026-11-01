package app.myco.update

import android.content.Context
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import java.util.concurrent.TimeUnit

/** OS-scheduled checks also run after the UI closes; delivery is not immediate. */
class SoftwareUpdateWorker(context: Context, parameters: WorkerParameters) :
    CoroutineWorker(context, parameters) {
    override suspend fun doWork(): Result {
        val updates = SoftwareUpdates.getInstance(applicationContext)
        updates.check(softwareUpdatesOfflineOnly(applicationContext))
        // The carry runs apart from the check; the work isn't done until it is.
        updates.awaitCarry()
        return Result.success()
    }

    companion object {
        private const val NAME = "myco-software-update"

        fun schedule(context: Context, enabled: Boolean) {
            val manager = WorkManager.getInstance(context)
            if (!enabled) {
                manager.cancelUniqueWork(NAME)
                return
            }
            // No network constraint: in mesh-only mode the Circle answers.
            val request = PeriodicWorkRequestBuilder<SoftwareUpdateWorker>(1, TimeUnit.HOURS).build()
            manager.enqueueUniquePeriodicWork(NAME, ExistingPeriodicWorkPolicy.UPDATE, request)
        }
    }
}
