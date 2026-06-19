package app.myco

import android.annotation.SuppressLint
import android.app.ActivityManager
import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.util.Log
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.ComponentActivity
import androidx.activity.addCallback
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
        super.onCreate(savedInstanceState)
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
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            // No file/content access — an nsite is pure web content served by us.
            settings.allowFileAccess = false
            settings.allowContentAccess = false
            settings.mediaPlaybackRequiresUserGesture = false
            webViewClient = NsiteWebViewClient(client, "$hostLabel.localhost")
        }
        setContentView(webView)

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
) : WebViewClient() {
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
