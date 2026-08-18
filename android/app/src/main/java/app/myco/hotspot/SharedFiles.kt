package app.myco.hotspot

import android.content.ContentUris
import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.util.Log
import android.webkit.MimeTypeMap
import java.io.InputStream
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * The files the hotspot file page serves and receives.
 *
 * Two sources, one list — both scoped to the **current hotspot session**
 * ([beginSession] wipes the slate when a hotspot starts, so a guest is never
 * offered anything the owner didn't put up *this* time):
 *
 *  - **Received uploads** live in public `Download/Myco/` via [MediaStore], so
 *    the phone's owner finds them in the Files app like any other download —
 *    and keeps them across sessions. Only the rows created this session are
 *    re-served, though; earlier sessions' files stay private to the owner.
 *  - **Picked shares** are documents the owner adds through the system picker
 *    (SAF `OpenMultipleDocuments`). They are streamed from their content URIs,
 *    never copied, and forgotten at the next session start.
 *
 * Entry ids are `m<mediastore-id>` / `u<index>` — opaque to the web page.
 * A process-wide singleton (like the radios) so the service's server threads
 * and the Compose sheet observe the same list.
 */
class SharedFiles private constructor(private val context: Context) {

    data class Entry(val id: String, val name: String, val size: Long)

    private data class Picked(val uri: Uri, val name: String, val size: Long)

    private val picked = ArrayList<Picked>()

    /** Entry ids (`m<rowid>`) of uploads received during the current hotspot
     *  session — the only MediaStore rows [list] serves. */
    private val sessionUploads = HashSet<String>()
    private val lock = Any()

    private val _entries = MutableStateFlow<List<Entry>>(emptyList())

    /** Live list for the sheet; the server re-queries per request instead. */
    val entries: StateFlow<List<Entry>> = _entries.asStateFlow()

    /** A hotspot session is starting: nothing from before is on offer. */
    fun beginSession() {
        synchronized(lock) {
            picked.clear()
            sessionUploads.clear()
        }
        refresh()
    }

    /** Add documents picked with SAF to the served list (session-scoped). */
    fun addUris(uris: List<Uri>) {
        synchronized(lock) {
            for (uri in uris) {
                if (picked.any { it.uri == uri }) continue
                val (name, size) = displayNameAndSize(uri) ?: continue
                picked.add(Picked(uri, name, size))
            }
        }
        refresh()
    }

    /** Everything currently served: this session's uploads, then picked shares. */
    fun list(): List<Entry> {
        val session = synchronized(lock) { sessionUploads.toSet() }
        val fromStore = queryUploads().filter { it.id in session }
        val fromPicker = synchronized(lock) {
            picked.mapIndexed { i, p -> Entry("u$i", p.name, p.size) }
        }
        return fromStore + fromPicker
    }

    /** Open an entry for serving. Returns the stream and its length (0 = unknown). */
    fun open(id: String): Pair<InputStream, Long>? = runCatching {
        when {
            id.startsWith("m") -> {
                val rowId = id.drop(1).toLongOrNull() ?: return null
                val uri = ContentUris.withAppendedId(collection, rowId)
                val size = queryUploads().firstOrNull { it.id == id }?.size ?: 0L
                context.contentResolver.openInputStream(uri)?.let { it to size }
            }
            id.startsWith("u") -> {
                val p = synchronized(lock) { picked.getOrNull(id.drop(1).toIntOrNull() ?: -1) }
                    ?: return null
                context.contentResolver.openInputStream(p.uri)?.let { it to p.size }
            }
            else -> null
        }
    }.getOrNull()

    /**
     * Persist an uploaded file into `Download/Myco/` and add it to the list.
     * MediaStore uniquifies a colliding name itself ("x (1).png"). `IS_PENDING`
     * keeps a half-written row invisible to other apps until the copy finishes.
     */
    fun saveUpload(name: String, src: InputStream): Boolean {
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, name)
            put(MediaStore.MediaColumns.MIME_TYPE, mimeFor(name))
            put(MediaStore.MediaColumns.RELATIVE_PATH, RELATIVE_PATH)
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val resolver = context.contentResolver
        val uri = resolver.insert(collection, values) ?: return false
        return runCatching {
            resolver.openOutputStream(uri)?.use { out -> src.copyTo(out) }
                ?: error("no output stream for $uri")
            values.clear()
            values.put(MediaStore.MediaColumns.IS_PENDING, 0)
            resolver.update(uri, values, null, null)
            synchronized(lock) { sessionUploads.add("m${ContentUris.parseId(uri)}") }
            refresh()
            true
        }.getOrElse {
            Log.w(TAG, "saving upload '$name' failed", it)
            resolver.delete(uri, null, null)
            false
        }
    }

    fun refresh() {
        _entries.value = runCatching { list() }.getOrDefault(emptyList())
    }

    private val collection: Uri = MediaStore.Downloads.EXTERNAL_CONTENT_URI

    private fun queryUploads(): List<Entry> {
        val projection = arrayOf(
            MediaStore.MediaColumns._ID,
            MediaStore.MediaColumns.DISPLAY_NAME,
            MediaStore.MediaColumns.SIZE,
        )
        val rows = ArrayList<Entry>()
        runCatching {
            context.contentResolver.query(
                collection, projection,
                "${MediaStore.MediaColumns.RELATIVE_PATH} LIKE ?", arrayOf("$RELATIVE_PATH%"),
                "${MediaStore.MediaColumns.DISPLAY_NAME} ASC",
            )?.use { c ->
                while (c.moveToNext()) {
                    rows.add(Entry("m${c.getLong(0)}", c.getString(1) ?: continue, c.getLong(2)))
                }
            }
        }.onFailure { Log.w(TAG, "querying uploads failed", it) }
        return rows
    }

    private fun displayNameAndSize(uri: Uri): Pair<String, Long>? = runCatching {
        context.contentResolver.query(
            uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null,
        )?.use { c ->
            if (!c.moveToFirst()) return null
            val name = c.getString(0)?.takeIf { it.isNotBlank() } ?: "file"
            name to (if (c.isNull(1)) 0L else c.getLong(1))
        }
    }.getOrNull()

    companion object {
        private const val TAG = "MycoSharedFiles"
        private const val RELATIVE_PATH = "Download/Myco"

        fun mimeFor(name: String): String =
            MimeTypeMap.getSingleton()
                .getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase())
                ?: "application/octet-stream"

        @Volatile
        private var instance: SharedFiles? = null

        fun get(context: Context): SharedFiles =
            instance ?: synchronized(this) {
                instance ?: SharedFiles(context.applicationContext).also {
                    instance = it
                    it.refresh()
                }
            }
    }
}
