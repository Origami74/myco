package app.myco.update

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.supervisorScope
import org.junit.Assert.*
import org.junit.Test

class UpdateCheckCoordinatorTest {
    @Test fun manualCallerWaitsForBackgroundResultWithoutAnotherQuery() = runBlocking {
        val checks = UpdateCheckCoordinator()
        val release = CompletableDeferred<Unit>()
        var calls = 0
        var result: String? = null
        val background = launch(start = CoroutineStart.UNDISPATCHED) {
            checks.run { calls++; release.await(); result = "0.12.0" }
        }
        val manual = async(start = CoroutineStart.UNDISPATCHED) {
            checks.run { calls++ }
            result
        }
        assertFalse(manual.isCompleted)
        assertEquals(1, calls)
        release.complete(Unit)
        assertEquals("0.12.0", manual.await())
        background.join()
        checks.run { calls++ }
        assertEquals(2, calls)
    }

    @Test fun forcedCallerRunsAgainAfterUnforcedCheck() = runBlocking {
        val checks = UpdateCheckCoordinator()
        val release = CompletableDeferred<Unit>()
        val runs = mutableListOf<String>()
        val background = launch(start = CoroutineStart.UNDISPATCHED) {
            checks.run { release.await(); runs += "background" }
        }
        val manual = launch(start = CoroutineStart.UNDISPATCHED) {
            checks.run(force = true) { runs += "manual" }
        }
        val other = launch(start = CoroutineStart.UNDISPATCHED) {
            checks.run { runs += "other" }
        }
        release.complete(Unit)
        listOf(background, manual, other).forEach { it.join() }
        assertEquals(listOf("background", "manual"), runs)
    }

    @Test fun forcedCallerJoinsForcedCheck() = runBlocking {
        val checks = UpdateCheckCoordinator()
        val release = CompletableDeferred<Unit>()
        var calls = 0
        val first = launch(start = CoroutineStart.UNDISPATCHED) {
            checks.run(force = true) { calls++; release.await() }
        }
        val second = launch(start = CoroutineStart.UNDISPATCHED) {
            checks.run(force = true) { calls++ }
        }
        release.complete(Unit)
        first.join()
        second.join()
        assertEquals(1, calls)
    }

    @Test fun cancellingWaiterDoesNotCancelBackgroundCheck() = runBlocking {
        val checks = UpdateCheckCoordinator()
        val release = CompletableDeferred<Unit>()
        var finished = false
        val background = launch(start = CoroutineStart.UNDISPATCHED) {
            checks.run { release.await(); finished = true }
        }
        val manual = launch(start = CoroutineStart.UNDISPATCHED) {
            checks.run { fail("must join the running check") }
        }
        manual.cancelAndJoin()
        assertTrue(background.isActive)
        release.complete(Unit)
        background.join()
        assertTrue(finished)
    }

    @Test fun failedOwnerReleasesWaitersAndAllowsRetry() = runBlocking {
        supervisorScope {
            val checks = UpdateCheckCoordinator()
            val release = CompletableDeferred<Unit>()
            val owner = async(start = CoroutineStart.UNDISPATCHED) {
                checks.run { release.await(); error("test failure") }
            }
            val waiter = async(start = CoroutineStart.UNDISPATCHED) {
                checks.run { fail("must join the running check") }
            }
            release.complete(Unit)
            for (job in listOf(owner, waiter)) {
                try { job.await(); fail("failure must reach both callers") }
                catch (error: IllegalStateException) { assertEquals("test failure", error.message) }
            }
            var retried = false
            checks.run { retried = true }
            assertTrue(retried)
        }
    }
}
