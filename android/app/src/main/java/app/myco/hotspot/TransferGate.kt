package app.myco.hotspot

import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

/**
 * Per-file consent for the hotspot file page: every transfer a guest starts —
 * downloading a shared file, sending one over — parks here until the phone's
 * owner taps Allow or Deny in the hotspot sheet. The guest's own consent is the
 * click that started the transfer; this is the owner's half.
 *
 * [request] is called from the web server's request threads and **blocks**
 * them; the HTTP response only starts once the owner decides. Undecided
 * requests are denied after [APPROVAL_TIMEOUT_S] so an unattended phone never
 * leaks a file, and a stopping hotspot denies everything still waiting
 * ([denyAll]). A process-wide singleton like the rest of the hotspot state.
 */
object TransferGate {

    enum class Direction { DOWNLOAD, UPLOAD }

    data class Pending(val id: Long, val direction: Direction, val name: String, val size: Long)

    private val nextId = AtomicLong(1)
    private val waiters = ConcurrentHashMap<Long, CompletableFuture<Boolean>>()
    private val _pending = MutableStateFlow<List<Pending>>(emptyList())

    /** What the sheet renders, oldest first. */
    val pending: StateFlow<List<Pending>> = _pending.asStateFlow()

    /** Block until the owner decides (or the timeout denies). Server threads only. */
    fun request(
        direction: Direction,
        name: String,
        size: Long,
        timeoutSeconds: Long = APPROVAL_TIMEOUT_S,
    ): Boolean {
        val id = nextId.getAndIncrement()
        val decision = CompletableFuture<Boolean>()
        waiters[id] = decision
        // update {} (CAS) — concurrent request threads must not lose each
        // other's list edits.
        _pending.update { it + Pending(id, direction, name, size) }
        return try {
            decision.get(timeoutSeconds, TimeUnit.SECONDS)
        } catch (e: Exception) {
            false // timeout (or interrupt) is a deny
        } finally {
            waiters.remove(id)
            _pending.update { list -> list.filterNot { it.id == id } }
        }
    }

    /** The owner tapped Allow/Deny on one row. Unknown ids are ignored (the
     *  request may have just timed out). */
    fun decide(id: Long, allow: Boolean) {
        waiters[id]?.complete(allow)
    }

    /** Hotspot stopping: fail every transfer still waiting. */
    fun denyAll() {
        waiters.values.forEach { it.complete(false) }
    }

    /** How long a guest's transfer waits for the owner before it is denied. */
    private const val APPROVAL_TIMEOUT_S = 90L
}
