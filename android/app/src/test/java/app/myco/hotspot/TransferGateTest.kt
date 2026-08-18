package app.myco.hotspot

import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class TransferGateTest {

    private fun awaitPending(): TransferGate.Pending {
        val deadline = System.currentTimeMillis() + 5_000
        while (System.currentTimeMillis() < deadline) {
            TransferGate.pending.value.firstOrNull()?.let { return it }
            Thread.sleep(10)
        }
        throw AssertionError("request never became pending")
    }

    private fun requestInBackground(): ArrayBlockingQueue<Boolean> {
        val result = ArrayBlockingQueue<Boolean>(1)
        thread {
            result.put(
                TransferGate.request(TransferGate.Direction.DOWNLOAD, "file.bin", 42, timeoutSeconds = 10),
            )
        }
        return result
    }

    @Test
    fun allowUnblocksTheTransfer() {
        val result = requestInBackground()
        TransferGate.decide(awaitPending().id, allow = true)
        assertEquals(true, result.poll(5, TimeUnit.SECONDS))
        assertTrue(TransferGate.pending.value.isEmpty())
    }

    @Test
    fun denyUnblocksWithFalse() {
        val result = requestInBackground()
        TransferGate.decide(awaitPending().id, allow = false)
        assertEquals(false, result.poll(5, TimeUnit.SECONDS))
        assertTrue(TransferGate.pending.value.isEmpty())
    }

    @Test
    fun timeoutIsADeny() {
        assertEquals(
            false,
            TransferGate.request(TransferGate.Direction.UPLOAD, "slow.bin", 1, timeoutSeconds = 0),
        )
        assertTrue(TransferGate.pending.value.isEmpty())
    }

    @Test
    fun denyAllFailsEveryWaiter() {
        val a = requestInBackground()
        val b = requestInBackground()
        val deadline = System.currentTimeMillis() + 5_000
        while (TransferGate.pending.value.size < 2 && System.currentTimeMillis() < deadline) {
            Thread.sleep(10)
        }
        assertEquals(2, TransferGate.pending.value.size)
        TransferGate.denyAll()
        assertEquals(false, a.poll(5, TimeUnit.SECONDS))
        assertEquals(false, b.poll(5, TimeUnit.SECONDS))
    }
}
