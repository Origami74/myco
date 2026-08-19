package app.myco.share

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns

/** One document received through Android's system Sharesheet. */
data class SharedItem(
    val uri: Uri,
    val name: String,
    val size: Long,
    val mimeType: String,
)

/** Android Sharesheet input and document-provider helpers. */
object ExternalShare {
    /** Extract the documents attached to a SEND or SEND_MULTIPLE intent. */
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

    /** Keep a provider grant alive when the sender supports persistable access. */
    fun retainReadAccess(context: Context, intent: Intent?, uris: List<Uri>) {
        val flags = intent?.flags?.and(Intent.FLAG_GRANT_READ_URI_PERMISSION) ?: 0
        if (flags == 0) return
        for (uri in uris) {
            runCatching { context.contentResolver.takePersistableUriPermission(uri, flags) }
        }
    }

    /** Resolve a human-readable label without opening or copying the file. */
    fun describe(context: Context, uri: Uri): SharedItem {
        var name: String? = null
        var size = 0L
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
            name = name?.takeIf { it.isNotBlank() } ?: fallback.ifBlank { "Photo" },
            size = size,
            mimeType = context.contentResolver.getType(uri) ?: "image/*",
        )
    }
}
