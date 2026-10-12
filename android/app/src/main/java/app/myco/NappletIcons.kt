package app.myco

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import app.myco.core.AppCoreClient

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

    /** Blocking — call off the UI thread. */
    fun fetch(client: AppCoreClient, pointer: String): Bitmap? =
        client.nappletIcon(pointer)?.let(::decode)

    /** An install sheet's preview, by hash. Blocking — call off the UI thread. */
    fun fetchByHash(client: AppCoreClient, sha256: String, mime: String): Bitmap? =
        client.iconBlob(sha256, mime)?.let(::decode)

    private fun decode(bytes: ByteArray): Bitmap? = runCatching {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
        if (bounds.outMimeType !in SUPPORTED || bounds.outWidth <= 0 || bounds.outHeight <= 0) {
            return null
        }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
    }.getOrNull()
}
