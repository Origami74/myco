package app.myco.neighbor

import android.content.Context
import android.net.Uri
import androidx.compose.runtime.mutableStateOf
import app.myco.core.AppCoreClient
import app.myco.share.ExternalShare
import org.json.JSONObject

/**
 * Myco's interim file share with Nearby (roadmap N13 moves it into the
 * filedrop napplet). See `myco-core/src/neighbor/files.rs`.
 *
 * A shared file is sealed under a key of its own and kept for the phones in
 * reach; nobody is told. The sender shows its link (QR, NFC, copy) to whoever
 * should get it; their phone fetches the sealed bytes from the sender's over
 * Nearby and opens them with the key in the link.
 */
object NearbyFiles {
    /** Files shared from another app, waiting for the user to send them. */
    val sharing = mutableStateOf<List<Uri>?>(null)

    /** A file's link that was scanned, tapped or opened, waiting to be saved. */
    val receiving = mutableStateOf<Receive?>(null)

    /** One file sent: its name and link, or why it wasn't. */
    data class Sent(val name: String, val link: String?, val error: String?)

    /** A file a link names. */
    data class Receive(val link: String, val name: String, val mime: String, val size: Long)

    /** [text] as a file's link this phone can open, or null. */
    fun read(client: AppCoreClient, text: String): Receive? {
        val o = client.nearbyLink(text) ?: return null
        return Receive(text.trim(), o.optString("name").ifEmpty { "file" }, o.optString("mime"), o.optLong("size"))
    }

    /**
     * Seal and keep [uris] for Nearby, one by one: each opened while the
     * sender's grant holds and handed to the core as a detached descriptor.
     * Off the main thread.
     */
    fun share(context: Context, client: AppCoreClient, uris: List<Uri>): List<Sent> = uris.map { uri ->
        val item = ExternalShare.describe(context, uri)
        val answer = runCatching {
            val fd = context.contentResolver.openFileDescriptor(uri, "r")?.detachFd() ?: return@runCatching null
            val meta = JSONObject().put("name", item.name).put("mime", item.mimeType).toString()
            client.nearbyShare(fd, meta)
        }.getOrNull()
        when {
            answer == null -> Sent(item.name, null, "couldn't open it")
            answer.optBoolean("ok") -> Sent(answer.optString("name").ifEmpty { item.name }, answer.optString("link"), null)
            else -> Sent(item.name, null, answer.optString("error").ifEmpty { "it couldn't be shared" })
        }
    }

    /** Fetch, open and write the file [receive] names into [target]. Null when saved, else why not. */
    fun save(context: Context, client: AppCoreClient, receive: Receive, target: Uri): String? = runCatching {
        val fd = context.contentResolver.openFileDescriptor(target, "w")?.detachFd() ?: return "couldn't write there"
        val answer = client.nearbySave(receive.link, fd)
        if (answer.optBoolean("ok")) null else answer.optString("error").ifEmpty { "it couldn't be saved" }
    }.getOrElse { "it couldn't be saved" }
}
