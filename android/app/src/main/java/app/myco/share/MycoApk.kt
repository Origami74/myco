package app.myco.share

import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.util.Log
import androidx.core.content.FileProvider
import java.io.File
import java.io.FileInputStream
import java.security.MessageDigest

/**
 * Myco's own installed APK, as something to hand to a phone that doesn't have
 * Myco yet (roadmap N11, the "to a phone without Myco" half).
 *
 * Two routes read it: the hotspot file page streams it straight from
 * [Info.file] (`/myco.apk`), and the share menu sends a copy through
 * [FileProvider] — the installed APK sits outside every path a provider may
 * expose, so [shareIntent] copies it into the cache first (once per version).
 *
 * A Myco installed in parts (a Play-style split install) has no single file
 * to pass on; [info] is null then and the UI says so.
 */
object MycoApk {

    /** The installed APK. [name] is what the receiver sees, e.g. `myco-0.12.1.apk`. */
    data class Info(val file: File, val name: String, val size: Long, val version: String)

    const val MIME = "application/vnd.android.package-archive"

    fun info(context: Context): Info? {
        val app = context.applicationInfo
        if (!app.splitSourceDirs.isNullOrEmpty()) return null
        val file = File(app.sourceDir)
        if (!file.canRead()) return null
        val version = runCatching {
            context.packageManager.getPackageInfo(context.packageName, 0).versionName
        }.getOrNull().orEmpty().ifBlank { "unknown" }
        return Info(file, "myco-$version.apk", file.length(), version)
    }

    /**
     * A chooser for the system share menu (Quick Share, Bluetooth, a
     * messenger). Copies the APK into the cache, so call it off the main
     * thread; null when there is nothing to share.
     */
    fun shareIntent(context: Context): Intent? {
        val info = info(context) ?: return null
        val dir = File(context.cacheDir, SHARE_DIR).apply { mkdirs() }
        val copy = File(dir, info.name)
        // One copy, of this version: an update leaves the old one behind otherwise.
        dir.listFiles()?.filter { it.name != copy.name }?.forEach { it.delete() }
        if (copy.length() != info.size) {
            val tmp = File(dir, "${info.name}.part")
            info.file.inputStream().use { src -> tmp.outputStream().use { src.copyTo(it) } }
            if (!tmp.renameTo(copy)) {
                tmp.delete()
                return null
            }
        }
        val uri = FileProvider.getUriForFile(context, authority(context), copy)
        val send = Intent(Intent.ACTION_SEND).apply {
            type = MIME
            putExtra(Intent.EXTRA_STREAM, uri)
            // The ClipData carries the read grant through the chooser to the target.
            clipData = ClipData.newRawUri(info.name, uri)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        return Intent.createChooser(send, "Share Myco")
    }

    @Volatile
    private var digest: Pair<String, String>? = null

    /** The APK's sha256 once [warmSha256] has computed it, else null. */
    fun cachedSha256(info: Info): String? =
        digest?.takeIf { it.first == key(info) }?.second

    /** Hash the APK (~a second for 100 MB) so the hotspot page can show it.
     *  Blocking; run it off the main thread. */
    fun warmSha256(context: Context) {
        val info = info(context) ?: return
        if (cachedSha256(info) != null) return
        runCatching {
            val md = MessageDigest.getInstance("SHA-256")
            FileInputStream(info.file).use { src ->
                val buf = ByteArray(1 shl 16)
                while (true) {
                    val n = src.read(buf)
                    if (n < 0) break
                    md.update(buf, 0, n)
                }
            }
            digest = key(info) to md.digest().joinToString("") { "%02x".format(it) }
        }.onFailure { Log.w(TAG, "hashing the APK failed", it) }
    }

    private fun key(info: Info) = "${info.file.path}:${info.file.lastModified()}:${info.size}"

    private fun authority(context: Context) = "${context.packageName}.files"

    private const val SHARE_DIR = "shared-apk"
    private const val TAG = "MycoApk"
}
