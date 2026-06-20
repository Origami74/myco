package app.myco.core

import android.content.Context
import app.myco.BuildConfig

/**
 * Process-wide holder for the single [AppCoreClient] (one fips node per process,
 * matching the native bridge's process-global). Both [app.myco.MainActivity]
 * (the developer UI) and the BLE foreground service drive the same node through
 * this, so the UI sees the peers the radio finds.
 */
object MycoCore {
    @Volatile
    private var client: AppCoreClient? = null

    fun client(context: Context): AppCoreClient {
        client?.let { return it }
        return synchronized(this) {
            client ?: run {
                val app = context.applicationContext
                NativeCore.initializeAndroidContext(app)
                AppCoreClient(app.filesDir.absolutePath, appVersion(app)).also { client = it }
            }
        }
    }

    private fun appVersion(context: Context): String {
        val name = runCatching {
            context.packageManager.getPackageInfo(context.packageName, 0).versionName ?: ""
        }.getOrDefault("")
        val rev = BuildConfig.GIT_REV
        return if (rev.isNotBlank() && rev != "unknown") "$name ($rev)" else name
    }
}
