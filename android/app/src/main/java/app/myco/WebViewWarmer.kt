package app.myco

import android.content.ComponentCallbacks2
import android.content.Context
import android.content.res.Configuration
import android.os.Looper
import android.util.Log
import android.webkit.RenderProcessGoneDetail
import android.webkit.WebView
import android.webkit.WebViewClient

/**
 * Keeps one idle, never-attached WebView alive while Myco is on screen, so the
 * WebView renderer process stays up between windows.
 *
 * Android runs every WebView of an app in one sandboxed renderer and kills it
 * the moment the last WebView is destroyed ("isolated not needed"). Closing a
 * napplet or nsite window therefore threw the renderer away, and the next
 * fresh open forked a new one and loaded Chromium into it again — measured at
 * 90–150 ms of black screen on a mid-range tablet. Holding this one blank
 * WebView keeps the renderer alive; it loads `about:blank` and nothing else,
 * has no JavaScript, and is never shown.
 *
 * Released when the app's UI is hidden ([ComponentCallbacks2.TRIM_MEMORY_UI_HIDDEN]):
 * Myco keeps running in the background for the mesh, and a renderer held for
 * a user who is not looking is memory taken from them for nothing. It is
 * warmed again the next time Myco's main screen resumes.
 *
 * The renderer is shared, so when a napplet or nsite crashes it this view
 * hears about it too. It answers like every other WebView here — it drops
 * itself and returns `true` — or the default would kill the app process,
 * mesh node and all, over one page's crash.
 *
 * Main thread only.
 */
object WebViewWarmer {
    private const val TAG = "WebViewWarmer"

    private var warm: WebView? = null
    private var callbacksRegistered = false

    /**
     * Cleared when the UI is hidden, so a warm-up still waiting for the idle
     * main thread does not start a renderer for an app nobody is looking at.
     */
    private var wanted = false

    /**
     * Warm the renderer once the main thread is idle — after the caller's
     * first frame, never in front of it. Idempotent.
     */
    fun warmWhenIdle(context: Context) {
        val app = context.applicationContext
        registerCallbacks(app)
        wanted = true
        Looper.myQueue().addIdleHandler {
            if (wanted) warm(app)
            false
        }
    }

    private fun warm(app: Context) {
        if (warm != null) return
        warm = runCatching {
            WebView(app).apply {
                settings.javaScriptEnabled = false
                webViewClient = object : WebViewClient() {
                    override fun onRenderProcessGone(
                        view: WebView,
                        detail: RenderProcessGoneDetail,
                    ): Boolean {
                        if (warm === view) warm = null
                        view.destroy()
                        return true
                    }
                }
                // Creating the view loads Chromium into this process; loading a
                // page is what binds (and so starts) the renderer.
                loadUrl("about:blank")
            }
        }.onFailure { Log.w(TAG, "could not warm a WebView", it) }.getOrNull()
    }

    private fun release() {
        wanted = false
        warm?.destroy()
        warm = null
    }

    private fun registerCallbacks(app: Context) {
        if (callbacksRegistered) return
        callbacksRegistered = true
        app.registerComponentCallbacks(
            object : ComponentCallbacks2 {
                override fun onTrimMemory(level: Int) {
                    if (level >= ComponentCallbacks2.TRIM_MEMORY_UI_HIDDEN) release()
                }

                override fun onConfigurationChanged(newConfig: Configuration) = Unit

                @Deprecated("Deprecated in Java")
                override fun onLowMemory() = release()
            },
        )
    }
}
