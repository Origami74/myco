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
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL

private const val RELEASE_PAGE = "https://github.com/Origami74/myco/releases/latest"
private const val RELEASE_API = "https://api.github.com/repos/Origami74/myco/releases/latest"
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

/** Foreground-only checks. No APK downloads, credentials, or installation authority. */
class SoftwareUpdates(context: Context) {
    private val context = context.applicationContext
    private val prefs = this.context.getSharedPreferences("software_updates", Context.MODE_PRIVATE)
    private val mutex = Mutex()
    private val mutableState = MutableStateFlow(
        SoftwareUpdateState(
            // Development builds use a placeholder version; avoid unsolicited notices.
            automatic = prefs.getBoolean("automatic", !BuildConfig.DEBUG),
            availableTag = prefs.getString("available_tag", null)?.takeIf {
                isNewerRelease(BuildConfig.VERSION_NAME, it)
            },
            dismissedTag = prefs.getString("dismissed_tag", null),
        ),
    )
    val state = mutableState.asStateFlow()

    fun setAutomatic(enabled: Boolean) {
        prefs.edit().putBoolean("automatic", enabled).apply()
        mutableState.value = mutableState.value.copy(automatic = enabled)
    }

    fun dismissBanner() {
        val tag = mutableState.value.availableTag ?: return
        prefs.edit().putString("dismissed_tag", tag).apply()
        mutableState.value = mutableState.value.copy(dismissedTag = tag)
    }

    fun openRelease() {
        try {
            context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(RELEASE_PAGE))
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        } catch (error: android.content.ActivityNotFoundException) {
            Toast.makeText(context, "No browser available to open the release page", Toast.LENGTH_LONG).show()
        }
    }

    suspend fun check(offlineOnly: Boolean, force: Boolean = false) {
        if (!mutex.tryLock()) return
        try {
            if (offlineOnly) {
                if (force) mutableState.value = mutableState.value.copy(message = "Turn off mesh-only mode to check online")
                return
            }
            if (!force && !mutableState.value.automatic) return
            val now = System.currentTimeMillis()
            val interval = if (prefs.getBoolean("last_success", false)) DAY_MS else RETRY_MS
            if (!force && !updateCheckDue(now, prefs.getLong("last_attempt", 0), interval)) return
            val connectivity = context.getSystemService(ConnectivityManager::class.java)
            // The mesh VPN may be the default network without itself validating.
            // A validated underlying internet connection is sufficient for a check.
            val online = connectivity.allNetworks.any { network ->
                val capabilities = connectivity.getNetworkCapabilities(network)
                capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true &&
                    capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)
            }
            if (!online) {
                if (force) mutableState.value = mutableState.value.copy(message = "Connect to the internet to check for updates")
                return
            }
            mutableState.value = mutableState.value.copy(checking = true, message = null)
            prefs.edit().putLong("last_attempt", now).putBoolean("last_success", false).apply()
            val tag = withContext(Dispatchers.IO) { fetchReleaseTag() }
            val available = tag.takeIf { isNewerRelease(BuildConfig.VERSION_NAME, it) }
            prefs.edit().putString("available_tag", available).putBoolean("last_success", true).apply()
            mutableState.value = mutableState.value.copy(
                availableTag = available,
                message = if (available != null) "Myco $available is available" else "Myco is up to date",
            )
            if (available != null) notifyOnce(available)
            else NotificationManagerCompat.from(context).cancel(NOTIFICATION_ID)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            Log.w("SoftwareUpdates", "Release check failed", error)
            mutableState.value = mutableState.value.copy(message = "Couldn't check for updates. Try again later.")
        } finally {
            mutableState.value = mutableState.value.copy(checking = false)
            mutex.unlock()
        }
    }

    private fun fetchReleaseTag(): String {
        val connection = URL(RELEASE_API).openConnection() as HttpURLConnection
        try {
            connection.connectTimeout = 10_000
            connection.readTimeout = 10_000
            connection.instanceFollowRedirects = false
            connection.setRequestProperty("Accept", "application/vnd.github+json")
            connection.setRequestProperty("User-Agent", "Myco/${BuildConfig.VERSION_NAME}")
            check(connection.responseCode == 200) { "Release API returned ${connection.responseCode}" }
            val bytes = connection.inputStream.use { input ->
                val output = java.io.ByteArrayOutputStream()
                val buffer = ByteArray(8192)
                while (true) {
                    val count = input.read(buffer)
                    if (count < 0) break
                    check(output.size() + count <= 256 * 1024) { "Release response too large" }
                    output.write(buffer, 0, count)
                }
                output.toByteArray()
            }
            check(bytes.size <= 256 * 1024) { "Release response too large" }
            return stableReleaseTag(JSONObject(String(bytes, Charsets.UTF_8)))
        } finally {
            connection.disconnect()
        }
    }

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

internal fun stableReleaseTag(release: JSONObject): String {
    require(!release.getBoolean("draft") && !release.getBoolean("prerelease")) { "Not a stable release" }
    val tag = release.getString("tag_name")
    require(ReleaseVersion.parse(tag) != null) { "Invalid release version" }
    val assets = release.getJSONArray("assets")
    require((0 until assets.length()).any {
        val asset = assets.getJSONObject(it)
        asset.optString("name").endsWith(".apk") && asset.optString("state") == "uploaded"
    }) { "Release has no APK" }
    return tag
}
