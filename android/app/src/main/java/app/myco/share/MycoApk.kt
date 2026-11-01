package app.myco.share

import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.util.Log
import androidx.core.content.FileProvider
import java.io.File
import java.io.FileInputStream
import java.security.MessageDigest
import java.util.zip.ZipFile

/**
 * Myco's own installed APK, as something to hand to a phone that doesn't have
 * Myco yet (roadmap N11, the "to a phone without Myco" half).
 *
 * Two routes read it: the hotspot file page streams it straight from
 * [Info.file] (`/myco.apk`), and the share menu sends a copy through
 * [FileProvider] — the installed APK sits outside every path a provider may
 * expose, so [shareIntent] copies it into the cache first (once per build).
 *
 * A Myco installed in parts (a Play-style split install) has no single file
 * to pass on; [info] is null then and the UI says so.
 */
object MycoApk {

    /**
     * The installed APK. [name] is what the receiver sees, e.g.
     * `myco-0.12.1.apk`. [release] is true only when it is signed with the
     * Myco release key, so its bytes are the ones on the release page and
     * later updates from Zapstore or GitHub install over it. [only64Bit]:
     * every native library it carries is 64-bit, so a 32-bit phone can't run it.
     */
    data class Info(
        val file: File,
        val name: String,
        val size: Long,
        val version: String,
        val release: Boolean,
        val only64Bit: Boolean,
    )

    const val MIME = "application/vnd.android.package-archive"

    /** SHA-256 of the Myco release signing certificate (as pinned in #151). */
    private const val RELEASE_CERT_SHA256 =
        "98432b60e463819071928666c894961dbd646f3e89197cfa654cd52afc9a7e79"

    @Volatile
    private var cached: Pair<String, Info>? = null

    fun info(context: Context): Info? {
        val app = context.applicationInfo
        if (!app.splitSourceDirs.isNullOrEmpty()) return null
        val file = File(app.sourceDir)
        if (!file.canRead()) return null
        val stamp = "${file.path}:${file.lastModified()}:${file.length()}"
        cached?.takeIf { it.first == stamp }?.let { return it.second }
        val version = runCatching {
            context.packageManager.getPackageInfo(context.packageName, 0).versionName
        }.getOrNull().orEmpty().ifBlank { "unknown" }
        val info = Info(
            file = file,
            name = "myco-$version.apk",
            size = file.length(),
            version = version,
            release = signedForRelease(context),
            only64Bit = only64Bit(nativeAbis(file)),
        )
        cached = stamp to info
        return info
    }

    /**
     * A chooser for the system share menu (Quick Share, Bluetooth, a
     * messenger). Copies the APK into the cache, so call it off the main
     * thread; null when there is nothing to share. Synchronized: a second tap
     * (say, after dismissing the sheet mid-copy) waits for the first copy
     * instead of deleting its half-written file.
     */
    @Synchronized
    fun shareIntent(context: Context): Intent? {
        val info = info(context) ?: return null
        val dir = File(context.cacheDir, SHARE_DIR).apply { mkdirs() }
        val copy = File(dir, info.name)
        // One copy, of this build: an update leaves the old one behind otherwise.
        dir.listFiles()?.filter { it.name != copy.name }?.forEach { it.delete() }
        val fresh = copy.exists() &&
            isFreshCopy(copy.length(), copy.lastModified(), info.size, info.file.lastModified())
        if (!fresh) {
            val tmp = File(dir, "${info.name}.part")
            info.file.inputStream().use { src -> tmp.outputStream().use { src.copyTo(it) } }
            copy.delete()
            if (!tmp.renameTo(copy)) {
                tmp.delete()
                Log.w(TAG, "couldn't move the APK copy into place")
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
            digest = key(info) to md.digest().toHex()
        }.onFailure { Log.w(TAG, "hashing the APK failed", it) }
    }

    /**
     * The cached copy is this build's: same size, and written after the
     * installed APK was. Size alone is not enough — local builds all carry
     * the latest tag's version name, so two of them share a file name.
     */
    internal fun isFreshCopy(copyLen: Long, copyModified: Long, apkLen: Long, apkModified: Long): Boolean =
        copyLen == apkLen && copyModified >= apkModified

    /** True when the APK carries native code and all of it is 64-bit. */
    internal fun only64Bit(abis: Set<String>): Boolean =
        abis.isNotEmpty() && abis.all { it in ABIS_64 }

    /** The ABIs the APK has native libraries for (`lib/<abi>/…` entries). */
    private fun nativeAbis(file: File): Set<String> = runCatching {
        ZipFile(file).use { zip ->
            zip.entries().asSequence()
                .map { it.name }
                .filter { it.startsWith("lib/") }
                .mapNotNull { it.split('/').getOrNull(1)?.takeIf(String::isNotEmpty) }
                .toSet()
        }
    }.getOrDefault(emptySet())

    private fun signedForRelease(context: Context): Boolean = runCatching {
        val signing = context.packageManager
            .getPackageInfo(context.packageName, PackageManager.GET_SIGNING_CERTIFICATES)
            .signingInfo ?: return false
        val certs = if (signing.hasMultipleSigners()) signing.apkContentsSigners else signing.signingCertificateHistory
        certs.orEmpty().any { cert ->
            MessageDigest.getInstance("SHA-256").digest(cert.toByteArray()).toHex() == RELEASE_CERT_SHA256
        }
    }.getOrDefault(false)

    private fun ByteArray.toHex() = joinToString("") { "%02x".format(it) }

    private fun key(info: Info) = "${info.file.path}:${info.file.lastModified()}:${info.size}"

    private fun authority(context: Context) = "${context.packageName}.files"

    private val ABIS_64 = setOf("arm64-v8a", "x86_64")
    private const val SHARE_DIR = "shared-apk"
    private const val TAG = "MycoApk"
}
