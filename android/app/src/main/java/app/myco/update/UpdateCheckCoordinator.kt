package app.myco.update

import kotlinx.coroutines.CompletableDeferred

/** Concurrent callers await the same check; only its owner performs network work. */
internal class UpdateCheckCoordinator {
    private val lock = Any()
    private var active: CompletableDeferred<Unit>? = null

    suspend fun run(check: suspend () -> Unit) {
        val (completion, owner) = synchronized(lock) {
            active?.let { it to false } ?: CompletableDeferred<Unit>().let {
                active = it
                it to true
            }
        }
        if (!owner) {
            completion.await()
            return
        }
        try {
            check()
            completion.complete(Unit)
        } catch (error: Throwable) {
            completion.completeExceptionally(error)
            throw error
        } finally {
            synchronized(lock) {
                if (active === completion) active = null
            }
        }
    }
}
