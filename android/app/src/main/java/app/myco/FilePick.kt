package app.myco

import android.content.ContentResolver
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Log
import org.json.JSONArray
import org.json.JSONObject

/**
 * A NAP-FS pick the runtime asked this window to show (`pick-file`).
 *
 * Every field but [token] is a hint the napplet supplied and Rust cleaned:
 * which picker, which types to filter on, what to call a new file. None of
 * it decides anything — the user picks, and the document provider decides
 * what can be opened.
 */
data class FilePick(
    val token: String,
    val mode: Mode,
    val mimeTypes: List<String>,
    val suggestedName: String,
    val saveMime: String,
    val read: Boolean,
    val write: Boolean,
) {
    enum class Mode { OPEN, OPEN_MULTIPLE, SAVE }

    companion object {
        fun parse(obj: JSONObject): FilePick? {
            val token = obj.optString("token").ifEmpty { return null }
            val mode = when (obj.optString("mode")) {
                "open" -> Mode.OPEN
                "open-multiple" -> Mode.OPEN_MULTIPLE
                "save" -> Mode.SAVE
                else -> return null
            }
            val types = obj.optJSONArray("mimeTypes")
            val mimeTypes = buildList {
                if (types != null) for (i in 0 until types.length()) types.optString(i).takeIf { it.isNotEmpty() }?.let(::add)
            }
            return FilePick(
                token = token,
                mode = mode,
                mimeTypes = mimeTypes,
                suggestedName = obj.optString("suggestedName"),
                saveMime = obj.optString("saveMime").ifEmpty { FilePicks.ANY_BINARY },
                read = obj.optBoolean("read"),
                write = obj.optBoolean("write"),
            )
        }
    }
}

/**
 * The window host's half of a NAP-FS pick: the system picker's intent, and
 * opening what the user chose as file descriptors handed to Rust.
 *
 * Rust reads and writes those descriptors by offset and closes them when the
 * window's session goes. Nothing here reads a byte of the file.
 */
object FilePicks {
    private const val TAG = "FilePicks"

    const val ANY_BINARY = "application/octet-stream"

    /** The most files one pick hands over; Rust keeps no more per window. */
    const val MAX_FILES = 64

    /** The picker's type filter: what Rust sent, or everything. */
    fun pickerMimeTypes(mimeTypes: List<String>): Array<String> =
        mimeTypes.filter { it.contains('/') }.ifEmpty { listOf("*/*") }.toTypedArray()

    /**
     * The `openFileDescriptor` modes to try, best first. Never `"w"` on a
     * document that already existed: providers may truncate on it. A save
     * target was just created empty, so there it is only a fallback.
     */
    fun openModes(mode: FilePick.Mode, write: Boolean): List<String> = when {
        mode == FilePick.Mode.SAVE -> listOf("rw", "w")
        write -> listOf("rw", "r")
        else -> listOf("r")
    }

    fun readable(openMode: String): Boolean = openMode.contains('r')

    fun writable(openMode: String): Boolean = openMode.contains('w')

    /** The system picker for [pick]. */
    fun intentFor(pick: FilePick): Intent = when (pick.mode) {
        FilePick.Mode.SAVE -> Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = pick.saveMime
            if (pick.suggestedName.isNotEmpty()) putExtra(Intent.EXTRA_TITLE, pick.suggestedName)
        }
        else -> Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            val types = pickerMimeTypes(pick.mimeTypes)
            type = if (types.size == 1) types[0] else "*/*"
            if (types.size > 1) putExtra(Intent.EXTRA_MIME_TYPES, types)
            if (pick.mode == FilePick.Mode.OPEN_MULTIPLE) putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
        }
    }

    /** The URIs a picker result carries, in order: one, or a multiple pick's clip. */
    fun urisOf(data: Intent?): List<Uri> {
        if (data == null) return emptyList()
        val clip = data.clipData
        if (clip != null && clip.itemCount > 0) {
            return (0 until clip.itemCount).mapNotNull { clip.getItemAt(it).uri }.take(MAX_FILES)
        }
        return listOfNotNull(data.data)
    }

    /** What to tell Rust when the user backed out. */
    fun cancelled(): String = JSONObject().put("status", "cancelled").toString()

    /** What to tell Rust when the picker could not be shown. */
    fun failed(error: String): String = JSONObject().put("status", "failed").put("error", error).toString()

    /**
     * Open each of [uris] for [pick] and return the detached descriptors with
     * the metadata Rust reads beside them. A document the provider will not
     * open, or opens as something other than a seekable file (a pipe from a
     * streaming provider), is left out and logged. Off the main thread.
     */
    fun open(resolver: ContentResolver, uris: List<Uri>, pick: FilePick): Pair<IntArray, String> {
        if (uris.isEmpty()) return IntArray(0) to cancelled()
        val fds = ArrayList<Int>()
        val files = JSONArray()
        for (uri in uris.take(MAX_FILES)) {
            var opened: Pair<android.os.ParcelFileDescriptor, String>? = null
            for (m in openModes(pick.mode, pick.write)) {
                val pfd = runCatching { resolver.openFileDescriptor(uri, m) }.getOrNull() ?: continue
                opened = pfd to m
                break
            }
            if (opened == null) {
                Log.i(TAG, "picked document could not be opened; left out")
                continue
            }
            val (pfd, openMode) = opened
            if (pfd.statSize < 0) {
                Log.i(TAG, "picked document is not a seekable file; left out")
                runCatching { pfd.close() }
                continue
            }
            val name = displayName(resolver, uri)
            val mime = runCatching { resolver.getType(uri) }.getOrNull().orEmpty()
            fds.add(pfd.detachFd())
            files.put(
                JSONObject()
                    .put("name", name)
                    .put("mime", mime)
                    .put("readable", readable(openMode))
                    .put("writable", writable(openMode)),
            )
        }
        if (fds.isEmpty()) return IntArray(0) to failed("io-error")
        return fds.toIntArray() to JSONObject().put("status", "picked").put("files", files).toString()
    }

    /** The provider's display name for [uri], or "file". */
    private fun displayName(resolver: ContentResolver, uri: Uri): String =
        runCatching {
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                if (c.moveToFirst()) c.getString(0) else null
            }
        }.getOrNull()?.ifEmpty { null } ?: "file"
}
