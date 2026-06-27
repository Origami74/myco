package app.myco

import android.annotation.SuppressLint
import android.app.ActivityManager
import android.content.ActivityNotFoundException
import android.content.Intent
import android.graphics.Color
import android.net.Uri
import android.os.Bundle
import android.util.Log
import android.widget.FrameLayout
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.ComponentActivity
import androidx.activity.addCallback
import androidx.activity.enableEdgeToEdge
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import app.myco.core.AppCoreClient
import app.myco.core.MycoCore
import java.io.ByteArrayInputStream

/**
 * A fullscreen, chrome-less nsite browser: a single [WebView] filling the screen
 * with no Myco toolbar/URL bar, running in its **own task** (documentLaunchMode +
 * FLAG_ACTIVITY_NEW_DOCUMENT) so each nsite is its own card in Recents.
 *
 * The WebView loads `http://<host>.nsite/` and every request — the page and all
 * its subresources — is served by [WebViewClient.shouldInterceptRequest] straight
 * from the in-process gateway (`client.gatewayGet`), direct from the local relay +
 * Blossom. This serve path is **TUN-independent**: it needs no VpnService, no DNS
 * interception, and no bound socket. The app-owned TUN (P3) only adds system-wide
 * `.nsite` for browsers *outside* the app.
 */
class NsiteActivity : ComponentActivity() {
    private lateinit var client: AppCoreClient
    private lateinit var webView: WebView

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        // Black splash (Myco mark) while the nsite loads — same as MainActivity;
        // must be installed before super.onCreate.
        installSplashScreen()
        super.onCreate(savedInstanceState)
        // Fill the whole device height with transparent system bars (pre-15 devices
        // otherwise keep opaque bars that frame the nsite with borders). The page
        // gets the bar regions via `env(safe-area-inset-*)` when it sets
        // `viewport-fit=cover`; chrome-less nsites without it simply draw full-bleed.
        // Bar-icon contrast can't be fixed at a constant here (unlike the always-white
        // Myco shell) — an nsite's background is arbitrary, so we sniff the page's
        // theme-color/background after load and flip the icons to match (see [syncBarContrast]).
        enableEdgeToEdge()
        client = MycoCore.client(this)

        val hostLabel = intent.getStringExtra(EXTRA_HOST).orEmpty()
        if (hostLabel.isEmpty()) {
            finish()
            return
        }
        val title = intent.getStringExtra(EXTRA_TITLE).orEmpty()
        // Give the Recents card the nsite's own title + favicon, for a native feel.
        applyTaskIcon("$hostLabel.localhost", title)

        webView = WebView(this).apply {
            // Paint black until the page renders, so there's no white flash
            // between the black splash and the nsite's first frame.
            setBackgroundColor(Color.BLACK)
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            // No file/content access — an nsite is pure web content served by us.
            settings.allowFileAccess = false
            settings.allowContentAccess = false
            settings.mediaPlaybackRequiresUserGesture = false
            webViewClient = NsiteWebViewClient(
                client,
                "$hostLabel.localhost",
                onContentVisible = { syncBarContrast() },
            )
        }
        // Host the WebView in a container we can inset. We draw edge-to-edge and
        // expect pages to pad via `env(safe-area-inset-bottom)`, but older Android
        // WebViews map only display cutouts into that env() — not the nav bar — so a
        // chrome-less nsite's bottom content (e.g. a chat composer) hides behind the
        // 3-button bar. Padding the WebView *view* doesn't reliably shrink its CSS
        // viewport (the page still lays out at full `100dvh` and the content is just
        // clipped), so pad the *parent*: that shrinks the WebView's layout height,
        // and thus the page's viewport, lifting the composer above the bar. Works on
        // every WebView version. Top stays full-bleed; the IME is handled by the
        // page's own visual-viewport logic.
        val root = FrameLayout(this).apply { setBackgroundColor(Color.BLACK) }
        root.addView(
            webView,
            FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT,
            ),
        )
        setContentView(root)
        ViewCompat.setOnApplyWindowInsetsListener(root) { v, insets ->
            val nav = insets.getInsets(WindowInsetsCompat.Type.navigationBars()).bottom
            val ime = insets.getInsets(WindowInsetsCompat.Type.ime()).bottom
            // Reserve whichever is taller: the nav bar (idle) or the soft keyboard
            // (open). Shrinking the WebView above the IME keeps the composer visible
            // on WebViews too old for `interactive-widget`/visualViewport keyboard
            // handling (e.g. the DC-1). Newer WebViews then see no occlusion, so
            // their own keyboard logic becomes a no-op — no double lift.
            v.setPadding(0, 0, 0, maxOf(nav, ime))
            insets
        }

        // Android Back navigates the WebView history, then leaves the nsite task.
        onBackPressedDispatcher.addCallback(this) {
            if (webView.canGoBack()) webView.goBack() else finish()
        }

        // Serve the WebView under `.localhost` (not `.nsite`): Chromium treats
        // `*.localhost` as loopback + a secure context, so the nsite's
        // `ws://localhost:4869` to the embedded relay isn't blocked by Private/
        // Local Network Access (a `.nsite` page is classed "public" → blocked).
        webView.loadUrl("http://$hostLabel.localhost/")
    }

    override fun onDestroy() {
        // Detach the WebView so the process-shared core isn't retained by it.
        if (this::webView.isInitialized) {
            webView.destroy()
        }
        super.onDestroy()
    }

    /**
     * Match the system-bar icon contrast to the nsite's own background. We can't
     * predict an nsite's palette, so we read its `<meta name="theme-color">` (or, if
     * absent, the computed page background) and pick light icons over a dark page,
     * dark icons over a light one. Runs on the WebView's JS callback (UI thread).
     * A missing/transparent/unparseable value leaves the current appearance as-is.
     */
    private fun syncBarContrast() {
        webView.evaluateJavascript(BG_PROBE_JS) { raw ->
            val color = parseCssColor(unquoteJs(raw)) ?: return@evaluateJavascript
            // "Light appearance" = dark icons (for a light bar background). So a light
            // page → light appearance (dark icons); a dark page → dark icons off (white).
            val lightBg = isLightColor(color)
            WindowCompat.getInsetsController(window, webView).apply {
                isAppearanceLightStatusBars = lightBg
                isAppearanceLightNavigationBars = lightBg
            }
        }
    }

    /**
     * Set the Recents task label + icon from the nsite itself — the title from the
     * manifest and the favicon (the blob the manifest maps at `/favicon.ico`,
     * falling back to common icon paths), fetched from the local gateway off the
     * UI thread. A real `.ico` may not decode; then the card just gets the title.
     */
    private fun applyTaskIcon(host: String, title: String) {
        Thread {
            val label = title.ifEmpty { "nsite" }
            val icon = NsiteIcons.fetch(client, host)
            runOnUiThread {
                @Suppress("DEPRECATION")
                val desc = if (icon != null) {
                    ActivityManager.TaskDescription(label, icon)
                } else {
                    ActivityManager.TaskDescription(label)
                }
                setTaskDescription(desc)
            }
        }.start()
    }

    companion object {
        const val EXTRA_HOST = "app.myco.extra.HOST"
        const val EXTRA_TITLE = "app.myco.extra.TITLE"

        /** A per-host document URI so re-opening the same nsite re-surfaces its task. */
        fun documentUri(hostLabel: String): Uri = Uri.parse("myco://app/$hostLabel")

        /** Read the page's declared theme-color, else the computed body/html background. */
        private const val BG_PROBE_JS = """
            (function () {
              try {
                var m = document.querySelector('meta[name="theme-color"]');
                if (m && m.content) return m.content;
                var b = document.body ? getComputedStyle(document.body).backgroundColor : '';
                if (b && b !== 'rgba(0, 0, 0, 0)' && b !== 'transparent') return b;
                return getComputedStyle(document.documentElement).backgroundColor || '';
              } catch (e) { return ''; }
            })()
        """

        /** Strip the JSON quoting `evaluateJavascript` wraps a returned string in. */
        private fun unquoteJs(raw: String?): String {
            val s = raw?.trim().orEmpty()
            if (s.length < 2 || s == "null" || !s.startsWith("\"")) return s
            return s.substring(1, s.length - 1).replace("\\\"", "\"").replace("\\\\", "\\")
        }

        /** Parse a CSS color — `#rgb`/`#rrggbb`, a name, or `rgb()/rgba()`. Null if
         *  unparseable or fully transparent (no usable background to contrast against). */
        private fun parseCssColor(value: String): Int? {
            val v = value.trim()
            if (v.isEmpty()) return null
            val rgb = Regex("rgba?\\(([^)]+)\\)").find(v)
            if (rgb != null) {
                val p = rgb.groupValues[1].split(",").map { it.trim() }
                if (p.size < 3) return null
                val r = p[0].toFloatOrNull()?.toInt() ?: return null
                val g = p[1].toFloatOrNull()?.toInt() ?: return null
                val b = p[2].toFloatOrNull()?.toInt() ?: return null
                if ((p.getOrNull(3)?.toFloatOrNull() ?: 1f) == 0f) return null
                return Color.rgb(r, g, b)
            }
            return runCatching { Color.parseColor(v) }.getOrNull()
        }

        /** Perceptual luminance test: true if [color] reads as a light background. */
        private fun isLightColor(color: Int): Boolean {
            val luminance =
                0.299 * Color.red(color) + 0.587 * Color.green(color) + 0.114 * Color.blue(color)
            return luminance > 140  // 0..255; midline biased slightly toward "dark icons".
        }
    }
}

/**
 * Serves every `*.nsite` request from the in-process gateway. Runs on a WebView
 * worker thread, so the blocking `gatewayGet` call is fine here.
 *
 * @param nsiteHost the host this nsite *is* (`<host>.nsite`); navigations to it
 *   stay inside the WebView, everything else is handed off to the system.
 */
private class NsiteWebViewClient(
    private val client: AppCoreClient,
    private val nsiteHost: String,
    private val onContentVisible: () -> Unit,
) : WebViewClient() {
    // First paint (early) and full load (catches late theme-color/background) both
    // re-sync the system-bar icon contrast to whatever the page is showing.
    override fun onPageCommitVisible(view: WebView, url: String) {
        onContentVisible()
    }

    override fun onPageFinished(view: WebView, url: String) {
        onContentVisible()
    }

    /**
     * Keep same-nsite navigation inside the WebView; send any link that leaves
     * this nsite — the open web, another nsite, `mailto:`/`tel:`/… — to the
     * system so it opens in the user's normal browser/app, for a native feel.
     */
    override fun shouldOverrideUrlLoading(
        view: WebView,
        request: WebResourceRequest,
    ): Boolean {
        val uri = request.url
        val scheme = uri.scheme?.lowercase()
        if (scheme == "http" || scheme == "https") {
            // In-origin link → let the WebView (and our gateway) handle it.
            if (uri.host?.equals(nsiteHost, ignoreCase = true) == true) return false
            return openExternally(view, uri)
        }
        // No external handler for these — let the WebView deal with them in-page.
        if (scheme == null || scheme in IN_PAGE_SCHEMES) return false
        // mailto:, tel:, sms:, geo:, intent:, … always belong to a native app.
        return openExternally(view, uri)
    }

    /** Hand [uri] to the system's default handler; swallow a missing handler. */
    private fun openExternally(view: WebView, uri: Uri): Boolean {
        val intent = Intent(Intent.ACTION_VIEW, uri).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        return try {
            view.context.startActivity(intent)
            true
        } catch (e: ActivityNotFoundException) {
            // Nothing on the device can open it (e.g. a bare .nsite with no TUN);
            // stay put rather than navigating the WebView to a dead page.
            Log.w("NsiteActivity", "No handler for $uri", e)
            true
        }
    }

    override fun shouldInterceptRequest(
        view: WebView,
        request: WebResourceRequest,
    ): WebResourceResponse? {
        val uri = request.url
        val host = uri.host ?: return null
        // Only our nsite hosts are served locally; anything else falls through
        // (and, offline, simply fails — v1 nsites are self-contained). The in-app
        // WebView serves nsites under `.localhost` (see loadUrl) so loopback WS to
        // the relay isn't blocked; `.nsite` is still accepted for compatibility.
        if (!host.endsWith(".localhost", ignoreCase = true) &&
            !host.endsWith(".nsite", ignoreCase = true)
        ) {
            return null
        }

        return try {
            val path = uri.path?.ifEmpty { "/" } ?: "/"
            val range = request.requestHeaders["Range"] ?: request.requestHeaders["range"] ?: ""
            val result = client.gatewayGet(host, path, range)

            val responseHeaders = LinkedHashMap<String, String>()
            for ((k, v) in result.headers) responseHeaders[k] = v
            // Same-origin content, but be permissive so in-page fetch() works.
            responseHeaders.putIfAbsent("Access-Control-Allow-Origin", "*")

            WebResourceResponse(
                result.mimeType,
                result.encoding,
                result.status,
                reasonPhrase(result.status),
                responseHeaders,
                ByteArrayInputStream(result.body),
            )
        } catch (t: Throwable) {
            val body = "<h1>500</h1><pre>${t.message}</pre>".toByteArray()
            WebResourceResponse(
                "text/html",
                "utf-8",
                500,
                "Internal Error",
                emptyMap(),
                ByteArrayInputStream(body),
            )
        }
    }

    private fun reasonPhrase(status: Int): String = when (status) {
        200 -> "OK"
        206 -> "Partial Content"
        404 -> "Not Found"
        416 -> "Range Not Satisfiable"
        500 -> "Internal Error"
        503 -> "Service Unavailable"
        else -> "OK"
    }

    private companion object {
        /** Schemes with no external app handler — the WebView renders them itself. */
        val IN_PAGE_SCHEMES = setOf("data", "blob", "about", "javascript", "file", "content")
    }
}
