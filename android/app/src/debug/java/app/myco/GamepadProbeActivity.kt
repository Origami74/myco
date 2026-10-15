package app.myco

import android.annotation.SuppressLint
import android.app.Activity
import android.os.Bundle
import android.util.Log
import android.webkit.ConsoleMessage
import android.webkit.WebChromeClient
import android.webkit.WebView

/** Debug-only hardware probe; no core session, identity or network bridge. */
class GamepadProbeActivity : Activity() {
    private lateinit var webView: WebView

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        webView = WebView(this).apply {
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = false
            settings.allowFileAccess = false
            settings.allowContentAccess = false
            webChromeClient = object : WebChromeClient() {
                override fun onConsoleMessage(message: ConsoleMessage): Boolean {
                    Log.i("MycoGamepadProbe", message.message())
                    return true
                }
            }
        }
        setContentView(webView)
        val bootstrap = assets.open("gamepad-probe.js").bufferedReader().use { it.readText() }
        val shell = assets.open("shell.html").bufferedReader().use { it.readText() }
        // Same secure-context origin shape as NappletActivity; no network load.
        webView.loadDataWithBaseURL(
            "http://gamepad-probe.napplet.localhost/",
            "<script>$bootstrap</script>$shell",
            "text/html", "UTF-8", null,
        )
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (::webView.isInitialized) {
            webView.evaluateJavascript(
                "window.dispatchEvent(new CustomEvent('probe-window-focus', {detail: $hasFocus}))",
                null,
            )
        }
    }

    override fun onDestroy() {
        webView.destroy()
        super.onDestroy()
    }
}
