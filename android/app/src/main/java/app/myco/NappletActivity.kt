package app.myco

import android.annotation.SuppressLint
import android.graphics.Color
import android.net.Uri
import android.os.Bundle
import android.util.Log
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.FrameLayout
import androidx.activity.ComponentActivity
import androidx.activity.addCallback
import androidx.activity.enableEdgeToEdge
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import app.myco.core.AppCoreClient
import app.myco.core.MycoCore
import app.myco.core.NappletOpen
import java.io.ByteArrayInputStream

/**
 * Hosts one napplet: a chrome-less WebView running the **shell**, which in turn
 * runs the napplet in a sandboxed iframe.
 *
 * A separate Activity from [NsiteActivity] on purpose. The two share a look, not
 * a codebase: their intent contracts, request interception, navigation policy,
 * lifecycle and trust boundaries all differ, and merging them would put
 * capability plumbing inside the class that renders untrusted nsite documents.
 * What is genuinely shared is chrome-less task plumbing, and that is
 * [ChromelessChrome] — a helper, not a base class, so nsite semantics cannot
 * arrive here by inheritance.
 *
 * ## What runs where
 *
 * ```text
 *   WebView  ->  the shell page at <label>.napplet.localhost   (ours, trusted)
 *                  └─ iframe sandbox="allow-scripts", srcdoc   (the napplet, untrusted)
 * ```
 *
 * The napplet is never the WebView's top-level document. Loaded that way it
 * would *be* the shell origin — with the shell's storage, inside the origin the
 * capability channel is scoped to. Its only home is the opaque origin the
 * sandboxed iframe gives it.
 *
 * ## The capability channel
 *
 * `addWebMessageListener`, registered against this window's shell origin alone
 * and never a wildcard. The napplet's iframe has an opaque origin, so it matches
 * no rule and never receives the injected object. `addJavascriptInterface` is
 * not used and must not be: its object lands in *every* frame with JavaScript
 * enabled, including the napplet's, which would hand the sandboxed napplet the
 * bridge and make the whole capability seam decorative.
 *
 * Verification, policy and every capability live in Rust. This class is a pipe.
 */
class NappletActivity : ComponentActivity() {
    private lateinit var client: AppCoreClient
    private lateinit var webView: WebView
    private lateinit var root: FrameLayout

    /** The per-window session id Rust keyed this napplet's session by. */
    private var sessionId: String = ""

    /** This window's shell origin — the only origin the channel is scoped to. */
    private var shellHost: String = ""

    /** Whether the shell page asked for the status-bar region. See [ChromelessChrome]. */
    private var pageOptedIntoFullHeight = false

    /**
     * The channel back into the shell, kept between messages.
     *
     * `addWebMessageListener` hands one of these to every inbound message and
     * it stays usable afterwards, which is what lets the runtime speak first —
     * a subscription delivering an event nobody asked for at that moment.
     */
    private var replyChannel: androidx.webkit.JavaScriptReplyProxy? = null

    /** Drains runtime-initiated frames while the window is open. */
    private var drainJob: kotlinx.coroutines.Job? = null

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        installSplashScreen()
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        client = MycoCore.client(this)

        val pointer = intent.getStringExtra(EXTRA_POINTER).orEmpty()
        if (pointer.isEmpty()) {
            finish()
            return
        }

        // Resolve and verify before anything is shown. A napplet that fails any
        // check gets no session and no window — there is no partial render to
        // fall back to, by design.
        val opened = client.nappletOpen(pointer)
        if (!opened.ok) {
            Log.w(TAG, "napplet $pointer did not open: ${opened.error}")
            // TODO(S1): surface this to the user rather than closing silently.
            finish()
            return
        }
        sessionId = opened.sessionId
        shellHost = opened.shellHost

        if (!WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) {
            // WebView 88+ carries this. Older ones need the WebMessagePort
            // fallback, which is not built yet — refuse rather than run a
            // napplet with no way to reach its capabilities.
            Log.w(TAG, "WebView is too old for the capability channel")
            client.nappletClose(sessionId)
            finish()
            return
        }

        webView = WebView(this).apply {
            setBackgroundColor(Color.BLACK)
            settings.javaScriptEnabled = true
            // The shell keeps no state of its own, and the napplet must not have
            // any: its storage is a capability, mediated in Rust, not something
            // the browser hands it. The opaque origin already denies it, and
            // this denies the shell too.
            settings.domStorageEnabled = false
            settings.allowFileAccess = false
            settings.allowContentAccess = false
            settings.mediaPlaybackRequiresUserGesture = false
            webViewClient = NappletWebViewClient(
                client = client,
                shellHost = shellHost,
                onContentVisible = { syncChrome() },
            )
        }

        // The capability channel. `allowedOriginRules` is this one origin,
        // exactly — a wildcard here would inject the object into every frame the
        // rule matched, which is the failure `addJavascriptInterface` has by
        // construction and this API exists to avoid.
        WebViewCompat.addWebMessageListener(
            webView,
            client.nappletRuntimeObject(),
            setOf("http://$shellHost"),
        ) { _, message, _, isMainFrame, replyProxy ->
            // Only the shell's own frame speaks on this channel. The napplet's
            // iframe cannot reach it, but a nested frame in the shell would be a
            // bug worth refusing rather than trusting.
            if (!isMainFrame) return@addWebMessageListener
            replyChannel = replyProxy
            val frame = message.data ?: return@addWebMessageListener
            for (reply in client.nappletFrame(sessionId, frame)) {
                replyProxy.postMessage(reply)
            }
        }

        // Runtime-initiated frames. A long poll on a background thread rather
        // than a callback into Kotlin: the FFI only runs when called, so the
        // waiting happens on this side — the same shape the BLE and TUN bridges
        // use. Bound to the window's lifecycle, so it stops when the napplet
        // closes rather than outliving it.
        drainJob = lifecycleScope.launch {
            while (isActive) {
                val frames = withContext(Dispatchers.IO) {
                    runCatching { client.nappletNextFrames(sessionId, DRAIN_WAIT_MS) }
                        .getOrDefault(emptyList())
                }
                // postMessage is main-thread work; the wait above was not.
                for (frame in frames) replyChannel?.postMessage(frame)
            }
        }

        root = FrameLayout(this).apply { setBackgroundColor(Color.BLACK) }
        root.addView(
            webView,
            FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT,
            ),
        )
        setContentView(root)
        ChromelessChrome.applyInsets(root) { pageOptedIntoFullHeight }

        ChromelessChrome.applyTaskDescription(
            this,
            intent.getStringExtra(EXTRA_TITLE).orEmpty().ifEmpty { "napplet" },
            null,
        )

        // Back leaves the napplet. There is no history to walk: the shell never
        // navigates, and the napplet cannot navigate the top-level document.
        onBackPressedDispatcher.addCallback(this) { finish() }

        webView.loadUrl("http://$shellHost/")
    }

    private fun syncChrome() {
        ChromelessChrome.syncBarContrast(this, webView)
        ChromelessChrome.probeFullHeight(webView) { wants ->
            if (wants != pageOptedIntoFullHeight) {
                pageOptedIntoFullHeight = wants
                ChromelessChrome.requestInsets(root)
            }
        }
    }

    override fun onDestroy() {
        drainJob?.cancel()
        replyChannel = null
        // Drop the session with the window. Rust ignores every later frame for
        // it, so a leaked WebView cannot keep a capability session alive.
        if (sessionId.isNotEmpty()) client.nappletClose(sessionId)
        if (this::webView.isInitialized) webView.destroy()
        super.onDestroy()
    }

    companion object {
        private const val TAG = "NappletActivity"

        /**
         * How long one drain call waits before coming back empty.
         *
         * Long enough that an idle napplet is not spinning across the FFI,
         * short enough that closing the window is not held up by a call already
         * in flight.
         */
        private const val DRAIN_WAIT_MS = 20_000L

        /** `naddr1…`, or the `<npub>:<dtag>` shorthand. */
        const val EXTRA_POINTER = "app.myco.extra.NAPPLET_POINTER"
        const val EXTRA_TITLE = "app.myco.extra.NAPPLET_TITLE"

        /**
         * A per-napplet document URI, so re-opening one re-surfaces its task.
         *
         * Keyed on the addressable pointer rather than the napplet's identity: a
         * napplet's identity is its aggregate hash and changes on every build, so
         * keying the *task* on it would strand the old Recents card on every
         * update. The session still pins the hash — see the design doc §7.8.
         */
        fun documentUri(pointer: String): Uri = Uri.parse("myco://napplet/$pointer")
    }
}

/**
 * Serves the shell page, and nothing else.
 *
 * The napplet's own bytes never pass through here — they are pushed over the
 * capability channel and assigned to `srcdoc`. Serving them at this origin would
 * make them reachable by URL, and anything navigating to that URL would run the
 * napplet as the shell origin.
 */
private class NappletWebViewClient(
    private val client: AppCoreClient,
    private val shellHost: String,
    private val onContentVisible: () -> Unit,
) : WebViewClient() {

    override fun onPageCommitVisible(view: WebView, url: String) = onContentVisible()

    override fun onPageFinished(view: WebView, url: String) = onContentVisible()

    /**
     * The shell never navigates. Every navigation attempt is refused, and any
     * link that reaches here goes to the system instead.
     *
     * This is the boundary that keeps the shell origin the shell's: a navigation
     * away and back, or into an nsite host, would put other content inside the
     * origin the capability channel is scoped to.
     */
    override fun shouldOverrideUrlLoading(
        view: WebView,
        request: WebResourceRequest,
    ): Boolean {
        val uri = request.url
        // The shell's own initial load is not a navigation request.
        if (uri.host?.equals(shellHost, ignoreCase = true) == true && uri.path == "/") {
            return false
        }
        val intent = android.content.Intent(android.content.Intent.ACTION_VIEW, uri)
            .addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
        return try {
            view.context.startActivity(intent)
            true
        } catch (e: android.content.ActivityNotFoundException) {
            Log.w("NappletActivity", "No handler for $uri", e)
            true
        }
    }

    override fun shouldInterceptRequest(
        view: WebView,
        request: WebResourceRequest,
    ): WebResourceResponse? {
        val uri = request.url
        val host = uri.host ?: return null

        // This window's shell origin, and only it. Another napplet's shell
        // origin is refused here just as firmly as an nsite host: each window
        // serves itself.
        if (!host.equals(shellHost, ignoreCase = true)) return null

        val path = uri.path?.ifEmpty { "/" } ?: "/"
        if (path != "/" && path != "/index.html") {
            return WebResourceResponse(
                "text/plain",
                "utf-8",
                404,
                "Not Found",
                emptyMap(),
                ByteArrayInputStream(ByteArray(0)),
            )
        }

        return WebResourceResponse(
            "text/html",
            "utf-8",
            200,
            "OK",
            // No caching: the shell is compiled into the binary and changes with
            // the app, and a stale one would be a stale capability channel.
            mapOf("Cache-Control" to "no-store"),
            ByteArrayInputStream(client.nappletShellPage().toByteArray()),
        )
    }
}
