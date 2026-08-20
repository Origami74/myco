package app.myco.share

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import app.myco.MainActivity
import app.myco.R
import app.myco.core.AppCoreClient
import app.myco.core.FileTransfer
import app.myco.core.MycoCore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/**
 * Tells you a paired phone is offering you a file while Myco is not on screen.
 *
 * An offer expires after ten minutes, so one that arrives while the app is in
 * the background is simply missed — the in-app banner only exists while
 * something is composed. This watches for offers exactly while no activity is
 * visible, and stops the moment one is: in the foreground the banner is already
 * doing the job, and two prompts for the same offer is worse than one.
 *
 * It says who, and nothing else. The filename is not on it: a notification is
 * shown on the lock screen, so putting the name of an incoming file there leaks
 * it to whoever can see the phone — and the file has not been accepted yet.
 *
 * Tapping only opens the app. Accepting is a consent decision that belongs
 * where the sender, name, type and size are all on screen together, not behind
 * a notification line — the same information gap the payload checks in the core
 * exist to close.
 */
object FileOfferNotifier {
    private const val CHANNEL_ID = "myco_file_offers"
    private const val NOTIF_ID = 4870
    private const val POLL_MS = 3_000L

    private var installed = false
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private var watch: Job? = null

    /** Idempotent: safe to call from every `MainActivity.onCreate`. */
    fun install(context: Context) {
        if (installed) return
        installed = true
        val app = context.applicationContext
        ProcessLifecycleOwner.get().lifecycle.addObserver(
            object : DefaultLifecycleObserver {
                override fun onStart(owner: LifecycleOwner) {
                    watch?.cancel()
                    watch = null
                    // Back on screen: the in-app banner takes over, and anything
                    // still pending is visible there.
                    clear(app)
                }

                override fun onStop(owner: LifecycleOwner) {
                    watch?.cancel()
                    watch = scope.launch {
                        val core = MycoCore.client(app)
                        var notified: String? = null
                        while (isActive) {
                            notified = tick(app, core, notified)
                            delay(POLL_MS)
                        }
                    }
                }
            },
        )
    }

    /** One poll. Returns the offer id currently notified, if any. */
    private fun tick(context: Context, core: AppCoreClient, notified: String?): String? {
        val offer = runCatching {
            core.state().fileTransfers
                .firstOrNull { it.direction == "incoming" && it.status == "waiting_user" }
        }.getOrNull()
        return when {
            // Answered elsewhere, cancelled by the sender, or timed out.
            offer == null -> {
                if (notified != null) clear(context)
                null
            }
            offer.id != notified -> {
                notify(context, offer)
                offer.id
            }
            else -> notified
        }
    }

    private fun notify(context: Context, offer: FileTransfer) {
        val nm = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "Incoming files",
                NotificationManager.IMPORTANCE_HIGH,
            )
            channel.description = "A paired phone wants to send you a file"
            nm.createNotificationChannel(channel)
        }
        val open = Intent(context, MainActivity::class.java)
            .setFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        val pending = PendingIntent.getActivity(
            context,
            0,
            open,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val peer = offer.peerName.ifBlank { "A paired phone" }
        val notification = NotificationCompat.Builder(context, CHANNEL_ID)
            .setContentTitle("$peer wants to send you a file")
            .setContentText("Tap to view")
            .setSmallIcon(R.mipmap.ic_launcher)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setAutoCancel(true)
            .setContentIntent(pending)
            .build()
        // Silently dropped when the runtime notification permission is refused,
        // which is fine — the in-app banner is still there when they come back.
        runCatching { nm.notify(NOTIF_ID, notification) }
    }

    private fun clear(context: Context) {
        val nm = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        runCatching { nm.cancel(NOTIF_ID) }
    }
}
