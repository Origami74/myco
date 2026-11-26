package app.myco.neighbor

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import androidx.compose.runtime.mutableStateOf
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import app.myco.MainActivity
import app.myco.R
import app.myco.core.AppCoreClient
import app.myco.share.ExternalShare
import org.json.JSONArray
import org.json.JSONObject

/** A file someone in Nearby shared (or this phone did). See `neighbor/files.rs`. */
data class NearbyFile(
    val id: String,
    val sha256: String,
    val name: String,
    val mime: String,
    val size: Long,
    val at: Long,
    val mine: Boolean,
) {
    companion object {
        fun parseList(json: String): List<NearbyFile> = runCatching {
            val arr = JSONArray(json)
            (0 until arr.length()).mapNotNull { arr.optJSONObject(it) }.map {
                NearbyFile(
                    id = it.optString("id"),
                    sha256 = it.optString("sha256"),
                    name = it.optString("name").ifEmpty { "file" },
                    mime = it.optString("mime").ifEmpty { "application/octet-stream" },
                    size = it.optLong("size"),
                    at = it.optLong("at"),
                    mine = it.optBoolean("mine"),
                )
            }
        }.getOrDefault(emptyList())
    }
}

/**
 * Myco's interim file share with Nearby (roadmap N13 moves it into a
 * napplet): what the sheets show, and the watch that notifies when someone
 * in reach shares a file.
 */
object NearbyFiles {
    private const val TAG = "myco-nearby-files"
    private const val CHANNEL = "nearby-files"
    private const val EVERY_MS = 10_000L
    const val EXTRA_OPEN = "app.myco.extra.OPEN_NEARBY_FILES"

    /** Files shared from another app, waiting for the user to send them to Nearby. */
    val sharing = mutableStateOf<List<Uri>?>(null)

    /** Whether the files sheet is open. */
    val showing = mutableStateOf(false)

    /** The files in Nearby, as last read. */
    val files = mutableStateOf<List<NearbyFile>>(emptyList())

    @Volatile
    private var started = false
    private val announced = mutableSetOf<String>()

    /** Read the files now, off the main thread. */
    fun refresh(client: AppCoreClient) {
        val now = runCatching { NearbyFile.parseList(client.nearbyFiles()) }.getOrDefault(emptyList())
        files.value = now
    }

    /** Watch for files others share, and tell the user. Once per process. */
    fun start(context: Context, client: AppCoreClient) {
        synchronized(this) {
            if (started) return
            started = true
        }
        val app = context.applicationContext
        val thread = HandlerThread("myco-nearby-files").apply { start() }
        val handler = Handler(thread.looper)
        val poll = object : Runnable {
            override fun run() {
                runCatching {
                    refresh(client)
                    val fresh = files.value.filter { !it.mine && announced.add(it.id) }
                    fresh.firstOrNull()?.let { notify(app, it, fresh.size) }
                }.onFailure { Log.w(TAG, "could not read Nearby's files", it) }
                handler.postDelayed(this, EVERY_MS)
            }
        }
        handler.post(poll)
    }

    private fun notify(context: Context, file: NearbyFile, count: Int) {
        if (!NotificationManagerCompat.from(context).areNotificationsEnabled()) return
        val nm = context.getSystemService(NotificationManager::class.java)
        if (nm.getNotificationChannel(CHANNEL) == null) {
            nm.createNotificationChannel(
                NotificationChannel(CHANNEL, "Files nearby", NotificationManager.IMPORTANCE_DEFAULT).apply {
                    description = "When someone in reach shares a file with Nearby"
                },
            )
        }
        val open = PendingIntent.getActivity(
            context,
            0,
            Intent(context, MainActivity::class.java)
                .putExtra(EXTRA_OPEN, true)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val title = if (count > 1) "$count files nearby" else "A file nearby"
        val notification = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle(title)
            .setContentText("${file.name} · someone in reach shared it")
            .setContentIntent(open)
            .setAutoCancel(true)
            .build()
        runCatching { NotificationManagerCompat.from(context).notify(CHANNEL, 1, notification) }
    }

    /**
     * Send [uris] to Nearby, one by one. Opens each while the sender's grant
     * holds and hands the core the detached descriptor. Off the main thread.
     * Returns how many went, and the first error.
     */
    fun share(context: Context, client: AppCoreClient, uris: List<Uri>): Pair<Int, String?> {
        var sent = 0
        var error: String? = null
        for (uri in uris) {
            val item = ExternalShare.describe(context, uri)
            val answer = runCatching {
                val fd = context.contentResolver.openFileDescriptor(uri, "r")?.detachFd() ?: return@runCatching null
                val meta = JSONObject().put("name", item.name).put("mime", item.mimeType).toString()
                client.nearbyShare(fd, meta)
            }.getOrNull()
            if (answer?.optBoolean("ok") == true) {
                sent++
            } else if (error == null) {
                error = answer?.optString("error")?.ifEmpty { null } ?: "couldn't open ${item.name}"
            }
        }
        refresh(client)
        return sent to error
    }

    /** Write [file] to [target], a document the user chose. Off the main thread. */
    fun save(context: Context, client: AppCoreClient, file: NearbyFile, target: Uri): Boolean = runCatching {
        val fd = context.contentResolver.openFileDescriptor(target, "w")?.detachFd() ?: return false
        client.nearbySave(file.sha256, fd)
    }.getOrDefault(false)
}
