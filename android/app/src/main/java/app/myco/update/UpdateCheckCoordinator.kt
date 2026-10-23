package app.myco.update

import kotlinx.coroutines.CompletableDeferred

/**
 * Concurrent callers await the same check; only its owner performs network work.
 * A forced caller does not settle for an unforced run, which may skip the query.
 */
internal class UpdateCheckCoordinator {
    private class Active(val force: Boolean) {
        val completion = CompletableDeferred<Unit>()
    }

    private val lock = Any()
    private var active: Active? = null

    suspend fun run(force: Boolean = false, check: suspend () -> Unit) {
        val mine = Active(force)
        while (true) {
            val running = synchronized(lock) { active ?: mine.also { active = it } }
            if (running === mine) break
            if (running.force || !force) {
                running.completion.await()
                return
            }
            // Let the unforced run finish, whatever its outcome, then run ours.
            running.completion.join()
        }
        try {
            check()
            mine.completion.complete(Unit)
        } catch (error: Throwable) {
            mine.completion.completeExceptionally(error)
            throw error
        } finally {
            synchronized(lock) {
                if (active === mine) active = null
            }
        }
    }
}
