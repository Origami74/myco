package app.myco

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import app.myco.core.AppCoreClient
import app.myco.core.NativeCore
import app.myco.ui.IdentityScreen

/**
 * P0 entry point: hand the core the app-private data dir, read the identity
 * snapshot, and show the device npub. The fullscreen per-nsite WebView, Library,
 * Pair, etc. arrive in later phases (see docs/roadmap.md).
 */
class MainActivity : ComponentActivity() {
    private lateinit var core: AppCoreClient

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        NativeCore.initializeAndroidContext(applicationContext)
        core = AppCoreClient(filesDir.absolutePath, appVersion())

        val state = core.state()
        setContent { IdentityScreen(state) }
    }

    override fun onDestroy() {
        if (::core.isInitialized) core.close()
        super.onDestroy()
    }

    private fun appVersion(): String =
        runCatching {
            packageManager.getPackageInfo(packageName, 0).versionName ?: ""
        }.getOrDefault("")
}
