package app.myco.update

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.Uri
import android.util.Log
import android.widget.Toast
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import app.myco.BuildConfig
import app.myco.onboarding.SystemAsk
import app.myco.R
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.withContext

private const val RELEASE_PAGE = "https://zapstore.dev/apps/app.myco"
private const val DAY_MS = 24 * 60 * 60 * 1000L
private const val RETRY_MS = 60 * 60 * 1000L
private const val CHANNEL = "software_updates"
private const val NOTIFICATION_ID = 4001

data class SoftwareUpdateState(
    val automatic: Boolean,
    val availableTag: String? = null,
    val checking: Boolean = false,
    val message: String? = null,
    val dismissedTag: String? = null,
)

/** Release checks shared by the UI and persistent background work. */
class SoftwareUpdates private constructor(context: Context) {
    companion object {
        @Volatile private var instance: SoftwareUpdates? = null

        fun getInstance(context: Context): SoftwareUpdates =
            instance ?: synchronized(this) {
                instance ?: SoftwareUpdates(context.applicationContext).also { instance = it }
            }
    }
    private val context = context.applicationContext
    private val prefs = this.context.getSharedPreferences("signed_software_updates", Context.MODE_PRIVATE)
    private val checks = UpdateCheckCoordinator()
    private val mutableState = MutableStateFlow(
        SoftwareUpdateState(
            // Network checks are opt-in in every build type.
            automatic = prefs.getBoolean("automatic", false),
            availableTag = prefs.getString("available_tag", null)?.takeIf {
                isNewerRelease(BuildConfig.VERSION_NAME, it)
            },
            dismissedTag = prefs.getString("dismissed_tag", null),
        ),
    )
    val state = mutableState.asStateFlow()

    fun configureBackground() {
        SoftwareUpdateWorker.schedule(context,
            prefs.getBoolean("automatic", false) && !softwareUpdatesOfflineOnly(context))
    }

    fun setAutomatic(enabled: Boolean) {
        prefs.edit().putBoolean("automatic", enabled).apply()
        mutableState.update { it.copy(automatic = enabled) }
    }

    fun dismissBanner() {
        val tag = mutableState.value.availableTag ?: return
        prefs.edit().putString("dismissed_tag", tag).apply()
        mutableState.update { it.copy(dismissedTag = tag) }
    }

    fun openRelease() {
        try {
            context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(RELEASE_PAGE))
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        } catch (error: android.content.ActivityNotFoundException) {
            Toast.makeText(context, "No browser available to open the release page", Toast.LENGTH_LONG).show()
        }
    }

    suspend fun check(offlineOnly: Boolean, force: Boolean = false) = checks.run {
        try {
            if (offlineOnly || softwareUpdatesOfflineOnly(context)) {
                if (force) mutableState.update { it.copy(message = "Turn off mesh-only mode to check online") }
                return@run
            }
            if (!force && (!prefs.getBoolean("automatic", false) ||
                    softwareUpdatesOfflineOnly(context))) return@run
            val cached = prefs.getString("available_tag", null)?.takeIf {
                isNewerRelease(BuildConfig.VERSION_NAME, it)
            }
            mutableState.update { it.copy(availableTag = cached) }
            if (cached != null) notifyOnce(cached)
            val now = System.currentTimeMillis()
            val interval = if (prefs.getBoolean("last_success", false)) DAY_MS else RETRY_MS
            if (!force && !updateCheckDue(now, prefs.getLong("last_attempt", 0), interval)) return@run
            val connectivity = context.getSystemService(ConnectivityManager::class.java)
            // The mesh VPN may be the default network without itself validating.
            // A validated underlying internet connection is sufficient for a check.
            val online = connectivity.allNetworks.any { network ->
                val capabilities = connectivity.getNetworkCapabilities(network)
                capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true &&
                    capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)
            }
            if (!online) {
                if (force) mutableState.update { it.copy(message = "Connect to the internet to check for updates") }
                return@run
            }
            mutableState.update { it.copy(checking = true, message = null) }
            prefs.edit().putLong("last_attempt", now).putBoolean("last_success", false).apply()
            val tag = withContext(Dispatchers.IO) { fetchReleaseTag() }
            // A setting may have changed while the relay query was in flight.
            if (softwareUpdatesOfflineOnly(context) ||
                (!force && !prefs.getBoolean("automatic", false))) return@run
            val available = tag.takeIf { isNewerRelease(BuildConfig.VERSION_NAME, it) }
            prefs.edit().putString("available_tag", available).putBoolean("last_success", true).apply()
            mutableState.update { it.copy(
                availableTag = available,
                message = if (available != null) "Myco $available is available" else "Myco is up to date",
            ) }
            if (available != null) notifyOnce(available)
            else NotificationManagerCompat.from(context).cancel(NOTIFICATION_ID)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            Log.w("SoftwareUpdates", "Release check failed", error)
            mutableState.update { it.copy(message = "Couldn't check for updates. Try again later.") }
        } finally {
            mutableState.update { it.copy(checking = false) }
        }
    }

    private fun fetchReleaseTag(): String = ZapstoreRelease.fetch()

    private fun notifyOnce(tag: String) {
        if (prefs.getString("notified_tag", null) == tag) return
        if (!SystemAsk.Notifications.granted(context)) return
        val manager = NotificationManagerCompat.from(context)
        val channel = NotificationChannel(CHANNEL, "Software updates", NotificationManager.IMPORTANCE_DEFAULT)
        channel.description = "A newer stable Myco release is available"
        manager.createNotificationChannel(channel)
        if (!manager.areNotificationsEnabled() ||
            manager.getNotificationChannel(CHANNEL)?.importance == NotificationManager.IMPORTANCE_NONE) return
        val pending = PendingIntent.getActivity(context, NOTIFICATION_ID,
            Intent(Intent.ACTION_VIEW, Uri.parse(RELEASE_PAGE)),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        try {
            manager.notify(NOTIFICATION_ID, NotificationCompat.Builder(context, CHANNEL)
                .setSmallIcon(R.drawable.ic_software_update)
                .setContentTitle("Myco update available")
                .setContentText("$tag is ready. Tap to view the release.")
                .setContentIntent(pending)
                .setAutoCancel(true)
                .setOnlyAlertOnce(true)
                .build())
            prefs.edit().putString("notified_tag", tag).apply()
        } catch (error: SecurityException) {
            Log.w("SoftwareUpdates", "Notification permission changed", error)
        }
    }
}

/** Use the same preference and default as MainActivity's native-core configuration. */
internal fun softwareUpdatesOfflineOnly(context: Context): Boolean =
    context.getSharedPreferences("myco_prefs", Context.MODE_PRIVATE)
        .getBoolean("offline_only", false)
