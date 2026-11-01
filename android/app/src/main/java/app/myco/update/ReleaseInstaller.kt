package app.myco.update

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import android.widget.Toast
import app.myco.core.NativeCore
import app.myco.share.MycoApk
import java.io.File
import org.json.JSONObject

/** A release APK this phone keeps for its Circle (`release_apk` in the core). */
data class CarriedRelease(
    val version: String,
    val versionCode: Long?,
    val sha256: String,
    val size: Long,
) {
    companion object {
        fun fromJson(json: String): CarriedRelease? = runCatching {
            val o = JSONObject(json)
            CarriedRelease(
                version = o.getString("version"),
                versionCode = if (o.isNull("version_code")) null else o.getLong("version_code"),
                sha256 = o.getString("sha256"),
                size = o.getLong("size"),
            )
        }.getOrNull()
    }
}

/**
 * Installs a Myco update the Circle carried, through Android's
 * [PackageInstaller]. The core checked the bytes against the publisher-signed
 * hash; this checks the APK once more as Android reads it — this app's
 * package, a newer version, signed with the release key — and Android shows
 * its own "Update?" prompt before anything is installed. Android refuses an
 * update signed with another key regardless.
 */
object ReleaseInstaller {
    private const val TAG = "MycoUpdate"

    /**
     * Write [release] out of the core's store, check it and hand it to
     * Android. Blocking — call off the main thread. Returns null once the
     * install is under way (Android's prompt follows), else why it isn't.
     */
    fun install(context: Context, handle: Long, release: CarriedRelease): String? {
        if (handle == 0L) return "Start the mesh to install the update"
        val dir = File(context.cacheDir, "update").apply { mkdirs() }
        dir.listFiles()?.forEach { it.delete() }
        val apk = File(dir, "myco-${release.version}.apk")
        try {
            NativeCore.exportSoftwareRelease(handle, release.sha256, apk.path)
            checkArchive(context, apk)?.let { return it }
            commit(context, apk)
            return null
        } catch (e: Exception) {
            Log.w(TAG, "installing ${release.version} failed", e)
            return e.message ?: "Couldn't install the update"
        } finally {
            // The session holds its own copy once written.
            apk.delete()
        }
    }

    /** Why [apk] must not be installed over this app, or null when it may. */
    private fun checkArchive(context: Context, apk: File): String? {
        val pm = context.packageManager
        val flags = PackageManager.GET_SIGNING_CERTIFICATES
        val archive = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            pm.getPackageArchiveInfo(apk.path, PackageManager.PackageInfoFlags.of(flags.toLong()))
        } else {
            @Suppress("DEPRECATION")
            pm.getPackageArchiveInfo(apk.path, flags)
        } ?: return "The update isn't a readable app"
        val installed = pm.getPackageInfo(context.packageName, 0)
        return when {
            archive.packageName != context.packageName -> "The update is a different app"
            archive.longVersionCode <= installed.longVersionCode -> "This Myco is already as new"
            !MycoApk.releaseSigned(archive.signingInfo) -> "The update isn't signed by Myco's publisher"
            else -> null
        }
    }

    private fun commit(context: Context, apk: File) {
        val installer = context.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(context.packageName)
            setSize(apk.length())
        }
        val id = installer.createSession(params)
        installer.openSession(id).use { session ->
            try {
                apk.inputStream().use { src ->
                    session.openWrite("base.apk", 0, apk.length()).use { out ->
                        src.copyTo(out)
                        session.fsync(out)
                    }
                }
            } catch (e: Exception) {
                // A half-written session would sit in Android's installer.
                session.abandon()
                throw e
            }
            // Mutable: the installer fills in the status and, for the user's
            // confirmation, the activity to start. Explicit, as Android 14
            // requires of a mutable PendingIntent.
            val result = PendingIntent.getBroadcast(
                context,
                id,
                Intent(context, InstallResultReceiver::class.java),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE,
            )
            session.commit(result.intentSender)
        }
    }
}

/** Android's answers about an update install ([ReleaseInstaller]). */
class InstallResultReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)
        when (status) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                val confirm = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
                } else {
                    @Suppress("DEPRECATION")
                    intent.getParcelableExtra(Intent.EXTRA_INTENT)
                }
                confirm?.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)?.let { context.startActivity(it) }
            }
            PackageInstaller.STATUS_SUCCESS -> Log.i("MycoUpdate", "update installed")
            PackageInstaller.STATUS_FAILURE_ABORTED -> Log.i("MycoUpdate", "update cancelled")
            else -> {
                val message = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)
                Log.w("MycoUpdate", "update failed: $status $message")
                Toast.makeText(context, "Couldn't install the update", Toast.LENGTH_LONG).show()
            }
        }
    }
}
