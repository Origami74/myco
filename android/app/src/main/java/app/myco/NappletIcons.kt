package app.myco

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.util.LruCache
import app.myco.core.AppCoreClient
import java.security.MessageDigest

/**
 * An installed napplet's icon: the manifest's `["icon", <sha256>, <mime>]`
 * blob, read from this phone's store. Used for the Apps grid tile, the
 * home-screen shortcut and the Recents card.
 *
 * NIP-5D §Icon: the native side hands back only bytes that hash to the signed
 * `icon` and start like the declared type; here they must also *decode* as
 * one of the supported formats. Anything else is null, and the caller shows
 * generic artwork — an icon never stands between a person and the napplet.
 */
object NappletIcons {
    private val SUPPORTED = setOf("image/png", "image/jpeg", "image/webp")

    /** An icon and the sha256 of its bytes — what a caller can tell it apart by. */
    class Keyed(val sha256: String, val bitmap: Bitmap)

    /**
     * Decoded icons by the sha256 of their bytes. Content addressed, so never
     * stale; the grid, the sheet, Recents and the shortcuts share one decode.
     */
    private val decoded = LruCache<String, Bitmap>(32)

    /**
     * Blocking — call off the UI thread. Never throws: a closed core or a
     * failed read is generic artwork, as no icon is.
     */
    fun fetch(client: AppCoreClient, pointer: String): Bitmap? = fetchKeyed(client, pointer)?.bitmap

    /** [fetch], with the bytes' hash. Blocking — call off the UI thread. */
    fun fetchKeyed(client: AppCoreClient, pointer: String): Keyed? =
        runCatching { client.nappletIcon(pointer) }.getOrNull()?.let(::decodeKeyed)

    /** An install sheet's preview, by hash. Blocking — call off the UI thread. */
    fun fetchByHash(client: AppCoreClient, sha256: String, mime: String): Bitmap? =
        runCatching { client.iconBlob(sha256, mime) }.getOrNull()?.let(::decodeKeyed)?.bitmap

    private fun decodeKeyed(bytes: ByteArray): Keyed? {
        val sha256 = MessageDigest.getInstance("SHA-256").digest(bytes)
            .joinToString("") { "%02x".format(it) }
        decoded.get(sha256)?.let { return Keyed(sha256, it) }
        val bitmap = decode(bytes) ?: return null
        decoded.put(sha256, bitmap)
        return Keyed(sha256, bitmap)
    }

    private fun decode(bytes: ByteArray): Bitmap? = runCatching {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
        if (bounds.outMimeType !in SUPPORTED || bounds.outWidth <= 0 || bounds.outHeight <= 0) {
            return null
        }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
    }.getOrNull()
}
