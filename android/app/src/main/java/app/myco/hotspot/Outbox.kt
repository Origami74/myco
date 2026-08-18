package app.myco.hotspot

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Log
import java.io.InputStream
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

/**
 * Files this phone is actively *sending* to the hotspot guest — the push half
 * of the AirDrop-style flow, as opposed to [SharedFiles]' browse-and-pull list.
 *
 * The owner picks files in the hotspot sheet; each becomes a WAITING offer.
 * The guest's page polls `/offers`, pops an accept/decline dialog, and either
 * fetches `/offer/<id>` (streams the document, marks it SENT) or declines it.
 * Offers stream straight from their content URIs — the picker grant lives as
 * long as the process, which outlives any hotspot session.
 *
 * Same singleton shape as [SharedFiles]: the service's server threads and the
 * Compose sheet observe one instance.
 */
class Outbox private constructor(private val context: Context) {

    enum class Status { WAITING, SENT, DECLINED }

    data class Offer(
        val id: Long,
        val name: String,
        val size: Long,
        val uri: Uri,
        val status: Status,
    )

    private val nextId = AtomicLong(1)
    private val _offers = MutableStateFlow<List<Offer>>(emptyList())

    /** Everything offered this session, for the sheet's status rows. */
    val offers: StateFlow<List<Offer>> = _offers.asStateFlow()

    /** Offer the picked documents to the guest. */
    fun add(uris: List<Uri>) {
        val fresh = uris.mapNotNull { uri ->
            val (name, size) = nameAndSize(uri) ?: return@mapNotNull null
            Offer(nextId.getAndIncrement(), name, size, uri, Status.WAITING)
        }
        if (fresh.isNotEmpty()) _offers.update { it + fresh }
    }

    /** What the guest's poll sees: only offers still waiting on them. */
    fun waiting(): List<Offer> = _offers.value.filter { it.status == Status.WAITING }

    /** The guest accepted: mark SENT and open the document for streaming. */
    fun accept(id: Long): Pair<Offer, InputStream>? {
        val offer = _offers.value.firstOrNull { it.id == id && it.status == Status.WAITING }
            ?: return null
        val stream = runCatching { context.contentResolver.openInputStream(offer.uri) }
            .onFailure { Log.w(TAG, "offer '${offer.name}' unreadable", it) }
            .getOrNull() ?: return null
        setStatus(id, Status.SENT)
        return offer.copy(status = Status.SENT) to stream
    }

    /** The guest declined the offer. */
    fun decline(id: Long) {
        setStatus(id, Status.DECLINED)
    }

    /** Hotspot stopped: the session's offers are void. */
    fun clear() {
        _offers.value = emptyList()
    }

    private fun setStatus(id: Long, status: Status) {
        _offers.update { list -> list.map { if (it.id == id) it.copy(status = status) else it } }
    }

    private fun nameAndSize(uri: Uri): Pair<String, Long>? = runCatching {
        context.contentResolver.query(
            uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null,
        )?.use { c ->
            if (!c.moveToFirst()) return null
            val name = c.getString(0)?.takeIf { it.isNotBlank() } ?: "file"
            name to (if (c.isNull(1)) 0L else c.getLong(1))
        }
    }.getOrNull()

    companion object {
        private const val TAG = "MycoOutbox"

        @Volatile
        private var instance: Outbox? = null

        fun get(context: Context): Outbox =
            instance ?: synchronized(this) {
                instance ?: Outbox(context.applicationContext).also { instance = it }
            }
    }
}
