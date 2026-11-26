package app.myco.share

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns

/** One document received through Android's system Sharesheet. */
data class SharedItem(
    val uri: Uri,
    val name: String,
    /** Bytes, or null when the provider doesn't say. */
    val size: Long?,
    val mimeType: String,
)

/** Android Sharesheet input: what another app shared with Myco. */
object ExternalShare {
    /** Whether [intent] is something shared with Myco. */
    fun isShare(intent: Intent?): Boolean =
        intent?.action == Intent.ACTION_SEND || intent?.action == Intent.ACTION_SEND_MULTIPLE

    /** The documents attached to a SEND or SEND_MULTIPLE intent. */
    @Suppress("DEPRECATION")
    fun uris(intent: Intent?): List<Uri> {
        if (intent == null) return emptyList()
        val found = buildList {
            when (intent.action) {
                Intent.ACTION_SEND -> intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)?.let(::add)
                Intent.ACTION_SEND_MULTIPLE ->
                    intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)?.let(::addAll)
            }
            // Some providers put multiple documents only in ClipData, even when
            // EXTRA_STREAM is also present. Keep both paths and de-duplicate them.
            intent.clipData?.let { clip ->
                for (i in 0 until clip.itemCount) clip.getItemAt(i).uri?.let(::add)
            }
        }
        return found.distinct()
    }

    /** Name, size and type, without opening or copying the file. */
    fun describe(context: Context, uri: Uri): SharedItem {
        var name: String? = null
        var size: Long? = null
        runCatching {
            context.contentResolver.query(
                uri,
                arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE),
                null,
                null,
                null,
            )?.use { cursor ->
                if (cursor.moveToFirst()) {
                    val nameColumn = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                    val sizeColumn = cursor.getColumnIndex(OpenableColumns.SIZE)
                    name = nameColumn.takeIf { it >= 0 }?.let(cursor::getString)
                    if (sizeColumn >= 0 && !cursor.isNull(sizeColumn)) size = cursor.getLong(sizeColumn)
                }
            }
        }
        val fallback = uri.lastPathSegment?.substringAfterLast('/').orEmpty()
        return SharedItem(
            uri = uri,
            name = name?.takeIf { it.isNotBlank() } ?: fallback.ifBlank { "file" },
            size = size,
            mimeType = context.contentResolver.getType(uri) ?: "application/octet-stream",
        )
    }
}
