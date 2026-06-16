package app.myco

import android.annotation.SuppressLint
import android.net.Uri
import android.os.Bundle
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

        webView = WebView(this).apply {
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            // No file/content access — an nsite is pure web content served by us.
            settings.allowFileAccess = false
            settings.allowContentAccess = false
            settings.mediaPlaybackRequiresUserGesture = false
            webViewClient = NsiteWebViewClient(client)
        }
        setContentView(webView)

        // Android Back navigates the WebView history, then leaves the nsite task.
        onBackPressedDispatcher.addCallback(this) {
            if (webView.canGoBack()) webView.goBack() else finish()
        }

        webView.loadUrl("http://$hostLabel.nsite/")
    }

    override fun onDestroy() {
        // Detach the WebView so the process-shared core isn't retained by it.
        if (this::webView.isInitialized) {
            webView.destroy()
        }
        super.onDestroy()
    }

    companion object {
        const val EXTRA_HOST = "app.myco.extra.HOST"

        /** A per-host document URI so re-opening the same nsite re-surfaces its task. */
        fun documentUri(hostLabel: String): Uri = Uri.parse("myco://app/$hostLabel")
    }
}

/**
 * Serves every `*.nsite` request from the in-process gateway. Runs on a WebView
 * worker thread, so the blocking `gatewayGet` call is fine here.
 */
private class NsiteWebViewClient(private val client: AppCoreClient) : WebViewClient() {
    override fun shouldInterceptRequest(
        view: WebView,
        request: WebResourceRequest,
    ): WebResourceResponse? {
        val uri = request.url
        val host = uri.host ?: return null
        // Only our nsite hosts are served locally; anything else falls through
        // (and, offline, simply fails — v1 nsites are self-contained).
        if (!host.endsWith(".nsite", ignoreCase = true)) return null

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
}
