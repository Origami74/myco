package app.myco

import android.annotation.SuppressLint
import android.content.Context
import android.content.Intent
import android.content.res.Configuration
import android.graphics.Color
import android.net.Uri
import android.os.Bundle
import android.os.SystemClock
import android.util.Log
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.ViewGroup
import android.webkit.RenderProcessGoneDetail
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import android.webkit.WebViewRenderProcess
import android.webkit.WebViewRenderProcessClient
import android.widget.FrameLayout
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.addCallback
import androidx.activity.enableEdgeToEdge
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.ComposeView
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.withContext
import org.json.JSONObject
import app.myco.core.AppCoreClient
import app.myco.core.MycoCore
import app.myco.core.NappletOpen
import app.myco.core.NappletReview
import app.myco.core.NativeActions
import app.myco.ui.screens.NappletReviewSheet
import app.myco.ui.screens.nappletLaunchTarget
import app.myco.ui.theme.MycoTheme
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
 *
 * ## Host commands (NAP-LINK)
 *
 * Two runtime frames are addressed to this window rather than the shell:
 * `open-external` (a web link the napplet asked to open — handed to the
 * browser, after a one-tap confirmation unless the user touched the napplet in
 * the last few seconds) and `review-napplet` (a napplet the napplet pointed at
 * — fetched for review with the same `FetchNapplet` a scanned code uses, and
 * Myco's install review drawn **over** this window, so the user keeps their
 * place). Nothing here installs: the review sheet's "Add" is the user's
 * answer, exactly as on the Apps screen.
 *
 * ## An update that asks for more
 *
 * When the version this window opens declares a capability the user never
 * reviewed, the open withholds it and hands back an update review. The sheet
 * is drawn here, over the app it is about — not queued on the Apps screen,
 * where nobody was looking and where it held up the app's own links. "Allow"
 * records the answer and the window relaunches with the new grants; "Not now"
 * leaves the app running without them, and the next open asks again.
 *
 * ## Updates while open
 *
 * The window runs the version it opened for as long as it lives. When it comes
 * back to the foreground after an update moved the served version on, it
 * offers a restart — once per version, see [RestartPrompt].
 */
class NappletActivity : ComponentActivity() {
    private lateinit var client: AppCoreClient
    private lateinit var webView: WebView
    private lateinit var root: FrameLayout

    /**
     * The per-window session id Rust keyed this napplet's session by.
     *
     * Written by the opener off the main thread and read by [onDestroy] on it,
     * both under [sessionLock] together with [windowGone]: exactly one of the
     * two sides sees the other's mark and closes the session. See [onCreate].
     */
    private var sessionId: String = ""

    /** Set by [onDestroy]; a session opened after this is closed by its opener. */
    private var windowGone = false
    private val sessionLock = Any()

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
    /**
     * Frames from the shell, in arrival order, consumed off the main thread.
     *
     * Bounded. [inFlight] bounds how many calls hold an FFI thread at once;
     * this bounds how much a napplet can queue behind them. A page looping
     * `postMessage` faster than the runtime answers used to grow this without
     * limit until the process died; now the frame past the cap is dropped
     * and logged, and the shim's own per-call timeout answers the call.
     */
    private val inbound = kotlinx.coroutines.channels.Channel<String>(INBOUND_CAPACITY)
    private var frameJob: kotlinx.coroutines.Job? = null

    /**
     * Drive one frame through the runtime and post its replies to the shell.
     * Returns true when a reply was `shell.init` — the handshake has answered
     * and later frames may overlap.
     */
    private suspend fun relay(frame: String): Boolean {
        val replies = runCatching { client.nappletFrame(sessionId, frame) }.getOrDefault(emptyList())
        var init = false
        withContext(Dispatchers.Main) {
            for (reply in replies) {
                if (hostCommand(reply)) continue
                if (relayedType(reply) == "shell.init") init = true
                replyChannel?.postMessage(reply)
            }
        }
        return init
    }

    /**
     * At most this many capability calls in flight per window. Each one holds a
     * background thread in the FFI for as long as its relays take to answer, and
     * a napplet that fires a hundred queries at dead relays would otherwise pin
     * the whole IO pool and stall the rest of the app behind it.
     */
    private val inFlight = Semaphore(MAX_IN_FLIGHT)

    /** The install review drawn over this window, mirrored from app state. */
    private var review by mutableStateOf<NappletReview?>(null)

    /**
     * The review of what this version asks for beyond what was reviewed, from
     * this window's own open. Held here, not in app state: it is about this
     * window's app and is answered over it.
     */
    private var updateReview by mutableStateOf<NappletReview?>(null)

    /** The pointer this window opened, for answering [updateReview]. */
    private var openedPointer: String = ""

    /** A web link waiting on the user's tap, when no recent touch vouched for it. */
    private var pendingExternal by mutableStateOf<Uri?>(null)

    /** Mirrors `state.nappletReview` into [review] while one is up. */
    private var reviewWatch: kotlinx.coroutines.Job? = null

    /** When the user last lifted a finger off this window. See [openLink]. */
    private var lastTouchAt = 0L

    /**
     * The newer version this window is offering to restart onto, while the
     * offer is on screen. See [offerRestartIfUpdated].
     */
    private var updatedTo by mutableStateOf<String?>(null)

    /** The name the restart offer calls this napplet by. */
    private var appTitle: String = ""

    /** Whether the window has left the foreground since it was last shown. */
    private var wasStopped = false

    private val restartPrompt by lazy { RestartPrompt.persisted(applicationContext) }

    override fun onStop() {
        super.onStop()
        wasStopped = true
    }

    override fun onStart() {
        super.onStart()
        if (wasStopped) {
            wasStopped = false
            offerRestartIfUpdated()
        }
    }

    /**
     * Back in the foreground — re-opened from Apps, from Recents, or by a
     * link — after an update moved this napplet past the version the window
     * opened: offer a restart, once per version. The window kept its session
     * on purpose (napplet-runtime.md §7.2); this is how the user reaches the
     * new one without hunting for the task in Recents.
     *
     * Never on a fresh open — that one is current by construction — and never
     * over the review sheet or a link confirmation: the next return asks
     * again, because nothing was shown.
     *
     * "I'll restart later" holds only until the next configuration change the
     * manifest does not absorb (a rotation): that already recreates the window,
     * which opens the version now served.
     */
    private fun offerRestartIfUpdated() {
        val id = synchronized(sessionLock) { sessionId }
        val napplet = shellHost
        // Both are set by the opener; until then (the splash) there is no
        // session to ask about, and no key to record an answer under.
        if (id.isEmpty() || napplet.isEmpty() || updatedTo != null) return
        lifecycleScope.launch {
            val newer = withContext(Dispatchers.IO) {
                val newer = runCatching { client.nappletNewerVersion(id) }.getOrNull()
                newer.takeIf { restartPrompt.shouldAsk(napplet, it) }
            } ?: return@launch
            if (sheetUp()) return@launch
            restartPrompt.markAsked(napplet, newer)
            updatedTo = newer
        }
    }

    override fun dispatchTouchEvent(ev: MotionEvent): Boolean {
        if (ev.actionMasked == MotionEvent.ACTION_UP) lastTouchAt = SystemClock.uptimeMillis()
        return super.dispatchTouchEvent(ev)
    }

    /**
     * A runtime frame addressed to this window rather than the shell. Returns
     * true when [frame] was one and has been handled; main thread.
     */
    private fun hostCommand(frame: String): Boolean {
        val obj = runCatching { JSONObject(frame) }.getOrNull() ?: return false
        return when (obj.optString("channel")) {
            "open-external" -> { openLink(obj.optString("url")); true }
            "review-napplet" -> { reviewNapplet(obj.optString("pointer")); true }
            else -> false
        }
    }

    /**
     * NAP-LINK to the web. Rust already classified it; the scheme is checked
     * again because this is where the intent is fired. A napplet's
     * `postMessage` carries no user gesture, so a touch on this window in the
     * last few seconds stands in for one — otherwise the user is asked once.
     * The napplet's `label` never reaches here: the dialog shows the real host.
     */
    private fun openLink(url: String) {
        val uri = Uri.parse(url)
        val scheme = uri.scheme?.lowercase()
        if (scheme != "https" && scheme != "http") return
        if (SystemClock.uptimeMillis() - lastTouchAt <= GESTURE_WINDOW_MS) {
            ExternalNavigation.openExternally(this, uri, TAG)
        } else {
            pendingExternal = uri
        }
    }

    /**
     * NAP-LINK to another napplet: the same fetch-for-review a scanned code
     * starts, with the sheet drawn over this window. Never an install.
     */
    private fun reviewNapplet(pointer: String) {
        if (pointer.isEmpty()) return
        act(NativeActions.fetchNapplet(pointer))
        watchReview()
    }

    /** Dispatch off the main thread: a reducer call takes the runtime lock. */
    private fun act(action: JSONObject) {
        lifecycleScope.launch(Dispatchers.IO) { runCatching { client.dispatch(action) } }
    }

    /**
     * "Open" on a review this window added: close the review and start the
     * new napplet in its own window, exactly as the Apps grid would — its own
     * task, keyed on its pointer — leaving this one running behind it.
     */
    private fun openAdded(added: NappletReview) {
        lifecycleScope.launch {
            val library = withContext(Dispatchers.IO) {
                runCatching { client.dispatch(NativeActions.dismissNappletReview()) }
                runCatching { client.state().library }.getOrDefault(emptyList())
            }
            val (pointer, title) = nappletLaunchTarget(added, library)
            startActivity(intent(this@NappletActivity, pointer, title))
        }
    }

    /**
     * Mirror the review slot into [review] until it empties — dismissed, or
     * never filled (the fetch refused the pointer outright is still a review,
     * with an error; an empty slot for this long means another surface already
     * answered it). A landed install does not empty it: the sheet turns
     * "added" and offers Open until the user closes it.
     */
    private fun watchReview() {
        reviewWatch?.cancel()
        reviewWatch = lifecycleScope.launch {
            val started = SystemClock.uptimeMillis()
            var seen = false
            while (isActive) {
                val current = withContext(Dispatchers.IO) {
                    runCatching { client.state().nappletReview }.getOrNull()
                }
                review = current
                if (current != null) {
                    seen = true
                } else if (seen || SystemClock.uptimeMillis() - started > REVIEW_APPEAR_MS) {
                    break
                }
                delay(REVIEW_POLL_MS)
            }
        }
    }

    /** Whether a sheet or dialog of this window is on screen over the napplet. */
    private fun sheetUp(): Boolean =
        review != null || updateReview != null || pendingExternal != null || updatedTo != null

    /** The review sheets and the link confirmation, over the WebView. */
    private fun overlay(): ComposeView = ComposeView(this).apply {
        setContent {
            MycoTheme {
                // The app's own update review first: it opened with the app,
                // and answering it relaunches the window.
                val update = updateReview
                if (update != null && review == null) {
                    NappletReviewSheet(
                        review = update,
                        onInstall = { granted ->
                            updateReview = null
                            // The core relaunches this window once the
                            // grants are recorded (a `relaunch` frame).
                            act(NativeActions.acceptNappletUpdate(openedPointer, granted))
                        },
                        onRetry = { updateReview = null },
                        onOpen = { updateReview = null },
                        onDismiss = { updateReview = null },
                    )
                }
                review?.let { r ->
                    NappletReviewSheet(
                        review = r,
                        onInstall = { granted ->
                            act(NativeActions.installNapplet(r.pointer, granted))
                            watchReview()
                        },
                        onRetry = {
                            act(NativeActions.fetchNapplet(r.pointer, r.holder))
                            watchReview()
                        },
                        onOpen = {
                            reviewWatch?.cancel()
                            review = null
                            openAdded(r)
                        },
                        onDismiss = {
                            reviewWatch?.cancel()
                            review = null
                            act(NativeActions.dismissNappletReview())
                        },
                    )
                }
                if (review == null && updateReview == null && pendingExternal == null && updatedTo != null) {
                    AlertDialog(
                        onDismissRequest = { updatedTo = null },
                        title = { Text("$appTitle was updated") },
                        text = { Text("Restart it to use the new version.") },
                        confirmButton = {
                            TextButton(onClick = {
                                updatedTo = null
                                // The grant-change relaunch: a new session,
                                // which opens the version now served.
                                recreate()
                            }) { Text("Restart") }
                        },
                        dismissButton = {
                            TextButton(onClick = { updatedTo = null }) { Text("I'll restart later") }
                        },
                    )
                }
                pendingExternal?.let { uri ->
                    AlertDialog(
                        onDismissRequest = { pendingExternal = null },
                        title = { Text("Open this link?") },
                        text = { Text("This app wants to open ${uri.host.orEmpty()} in your browser.") },
                        confirmButton = {
                            TextButton(onClick = {
                                pendingExternal = null
                                ExternalNavigation.openExternally(this@NappletActivity, uri, TAG)
                            }) { Text("Open") }
                        },
                        dismissButton = {
                            TextButton(onClick = { pendingExternal = null }) { Text("Cancel") }
                        },
                    )
                }
            }
        }
    }

    private fun isDark(): Boolean =
        (resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) ==
            Configuration.UI_MODE_NIGHT_YES

    /**
     * Dark mode flipped while the napplet is open. The manifest keeps this
     * window alive through a `uiMode` change, so the napplet is not restarted
     * for it: the session hears the new appearance and pushes NAP-THEME's
     * `theme.changed`; the sheet above re-themes on its own.
     */
    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        val id = synchronized(sessionLock) { sessionId }
        if (id.isNotEmpty()) {
            val dark = isDark()
            lifecycleScope.launch(Dispatchers.IO) {
                runCatching { client.nappletSetAppearance(id, dark) }
            }
        }
        if (this::webView.isInitialized) syncChrome()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        installSplashScreen()
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        client = MycoCore.client(this)
        // Until the shell is up there is no napplet to offer back to; it still
        // only sends the window behind, never closes it. Replaced by [onBack]
        // once mounted (a later callback takes precedence).
        onBackPressedDispatcher.addCallback(this) { moveTaskToBack(true) }

        val pointer = intent.getStringExtra(EXTRA_POINTER).orEmpty()
        if (pointer.isEmpty()) {
            finish()
            return
        }

        // Resolve and verify before anything is shown. A napplet that fails any
        // check gets no session and no window — there is no partial render to
        // fall back to, by design. Off the main thread: the resolve reads the
        // relay and the blob store, and with a custom relay configured that is
        // a network round trip. The splash screen covers the wait.
        //
        // The open is not cancellable, and it settles its own session. Back
        // during the splash used to cancel this coroutine mid-call: `onDestroy`
        // ran with `sessionId` still empty and skipped `nappletClose`, and when
        // the JNI call returned the assignment below never ran — the session
        // (the assembled HTML, its outbox channel) sat in `NappletHost.sessions`
        // forever. `NonCancellable` lets the call finish; the hand-off under
        // [sessionLock] is what closes the leak. It happens *inside* the IO
        // block on purpose: a result crossing back to the main thread is
        // discarded when the scope was cancelled meanwhile (prompt
        // cancellation), so anything after `withContext` may never run.
        lifecycleScope.launch {
            val opened = withContext(Dispatchers.IO + NonCancellable) {
                val opened = client.nappletOpen(pointer)
                if (opened.ok) {
                    val orphaned = synchronized(sessionLock) {
                        if (windowGone) true else { sessionId = opened.sessionId; false }
                    }
                    if (orphaned) {
                        Log.i(TAG, "napplet $pointer opened after its window closed; session closed")
                        client.nappletClose(opened.sessionId)
                    } else {
                        // Before the shell mounts, so the napplet's first
                        // `theme.get` already answers in the app's mode.
                        runCatching { client.nappletSetAppearance(opened.sessionId, isDark()) }
                    }
                }
                opened
            }
            if (!opened.ok) {
                Log.w(TAG, "napplet $pointer did not open: ${opened.error}")
                // Said out loud: a window that closes on its own reads as a tap
                // that did not register, and hides that the app is gone.
                Toast.makeText(
                    this@NappletActivity,
                    "Couldn't open this app: ${opened.error.orEmpty().ifEmpty { "it isn't on this phone" }}",
                    Toast.LENGTH_LONG,
                ).show()
                finish()
                return@launch
            }
            shellHost = opened.shellHost
            openedPointer = pointer
            updateReview = opened.updateReview
            appTitle = intent.getStringExtra(EXTRA_TITLE).orEmpty()
                .ifEmpty { opened.title.orEmpty() }
                .ifEmpty { "This app" }
            mountShell()
        }
    }

    @SuppressLint("SetJavaScriptEnabled")
    private fun mountShell() {
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
                onRendererGone = { finish() },
                onUnhandledEscape = ::backEscapeUnhandled,
            )
            // A renderer stuck in a script never answers the Escape a back
            // sent, and would otherwise leave back doing nothing at all.
            setWebViewRenderProcessClient(
                object : WebViewRenderProcessClient() {
                    override fun onRenderProcessUnresponsive(
                        view: WebView,
                        renderer: WebViewRenderProcess?,
                    ) {
                        if (backs.outstanding(SystemClock.uptimeMillis())) {
                            Log.w(TAG, "napplet unresponsive after back; closing the window")
                            finish()
                        }
                    }

                    override fun onRenderProcessResponsive(
                        view: WebView,
                        renderer: WebViewRenderProcess?,
                    ) = Unit
                },
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
            // Never on this thread: a capability call can wait on the network
            // for seconds, and this is the main thread. Queued in arrival
            // order; the consumer decides what may overlap. A full queue drops
            // the frame rather than the memory: see [inbound].
            val queued = inbound.trySend(frame)
            if (queued.isFailure && !queued.isClosed) {
                Log.w(TAG, "napplet frame queue full; frame dropped")
            }
        }

        // The inbound frames, driven off the main thread. Until the handshake
        // has answered, frames run one at a time in order — `shell.ready` must
        // land before the first capability call, or that call is refused as
        // "not established". After it, each frame gets its own coroutine, so
        // a relay query waiting on a slow relay does not hold up the publish
        // behind it. Rust holds the session only for calls that change it.
        frameJob = lifecycleScope.launch(Dispatchers.IO) {
            var established = false
            for (frame in inbound) {
                if (established) {
                    launch { inFlight.withPermit { relay(frame) } }
                } else {
                    if (relay(frame)) established = true
                }
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
                for (frame in frames) {
                    // A grant changed on the app's sheet: this window's napplet
                    // made its startup calls under the old grants, so start it
                    // over — new session, fresh handshake. Never in place: the
                    // shell runs one napplet for one lifetime.
                    if (channelOf(frame) == "relaunch") {
                        recreate()
                        break
                    }
                    if (hostCommand(frame)) continue
                    replyChannel?.postMessage(frame)
                }
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
        // Sized to its content, which is nothing until a sheet or dialog is up
        // — and those draw in windows of their own — so it never takes a
        // touch meant for the napplet.
        root.addView(
            overlay(),
            FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.WRAP_CONTENT,
                FrameLayout.LayoutParams.WRAP_CONTENT,
            ),
        )
        setContentView(root)
        ChromelessChrome.applyInsets(root) { pageOptedIntoFullHeight }

        ChromelessChrome.applyTaskDescription(
            this,
            intent.getStringExtra(EXTRA_TITLE).orEmpty().ifEmpty { "napplet" },
            null,
        )

        // Back is offered to the napplet first, as Escape. See [onBack]. Added
        // after the splash-time callback in onCreate, so this one wins.
        onBackPressedDispatcher.addCallback(this) { onBack() }

        webView.loadUrl("http://$shellHost/")
    }

    /** Which back Escapes are still unanswered, and how many were consumed. */
    private val backs = BackEscapeTracker()

    /**
     * The back gesture, delivered to the napplet as an Escape key it may
     * consume.
     *
     * A napplet has no history to walk: it runs in an opaque-origin `srcdoc`
     * frame where `history.pushState` throws, and the shell never navigates.
     * So back is sent as what it means on the web — Escape, "dismiss" — as a
     * real key-down and key-up into the WebView. The shell keeps focus in the
     * napplet's frame, so the napplet's document receives it.
     *
     * - The napplet called `preventDefault()` on the key-down: it closed a
     *   page or dialog of its own, and the window stays.
     * - It did not: Chromium hands the unhandled key back to the embedder,
     *   and WebView calls [WebViewClient.onUnhandledKeyEvent] with the **same**
     *   `KeyEvent` object dispatched here (Chromium keeps it as the event's
     *   `os_event`, and `WebContentsDelegateAndroid::HandleKeyboardEvent`
     *   passes it back through `AwWebContentsDelegateAdapter`). That lands in
     *   [backEscapeUnhandled], which [leave]s: the window goes to the
     *   background, still running, as Android does for an app at its root.
     *   Back never closes a napplet.
     *
     * A handled key-down is never reported; the key-up's report (or its
     * absence) marks the back consumed. So that no napplet can trap the user
     * by consuming every Escape, only [BackEscapeTracker.MAX_CONSUMED] backs in
     * a row are offered to it without a touch in between; the next leaves the
     * window directly. Every back sends its own Escape, so a rapid second back
     * is simply the napplet's next chance to go back.
     */
    private fun onBack() {
        if (isFinishing) return
        // The review sheet, the link dialog and the restart offer are windows
        // of their own and take back before it reaches this callback; should
        // one ever not, the napplet underneath must not hear an Escape meant
        // for them.
        if (sheetUp()) return
        val view = webView
        // Detached by a renderer crash: nobody is there to answer.
        if (view.parent == null) {
            finish()
            return
        }
        if (!view.hasFocus()) view.requestFocus()
        if (!view.hasFocus()) {
            leave()
            return
        }
        val now = SystemClock.uptimeMillis()
        if (backs.shouldLeaveDirectly(now, lastTouchAt)) {
            Log.i(TAG, "napplet consumed ${BackEscapeTracker.MAX_CONSUMED} backs without a touch; leaving")
            leave()
            return
        }
        backs.sent(now)
        view.dispatchKeyEvent(KeyEvent(now, now, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_ESCAPE, 0))
        view.dispatchKeyEvent(KeyEvent(now, now, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_ESCAPE, 0))
    }

    /**
     * The page left an Escape unhandled. Returns true when it was one [onBack]
     * sent — then it is consumed here, and an unhandled key-down [leave]s the
     * window. Any other Escape (a hardware keyboard) takes the default path.
     */
    private fun backEscapeUnhandled(event: KeyEvent): Boolean {
        val isDown = when (event.action) {
            KeyEvent.ACTION_DOWN -> true
            KeyEvent.ACTION_UP -> false
            else -> return false
        }
        return when (backs.unhandled(event.downTime, isDown, lastTouchAt)) {
            BackEscapeTracker.Report.NOT_OURS -> false
            BackEscapeTracker.Report.LEAVE -> { leave(); true }
            BackEscapeTracker.Report.CONSUMED -> true
        }
    }

    /**
     * Where back takes you when the napplet doesn't go back itself: out of the
     * window, not out of the napplet. The task moves behind — to Myco's Apps
     * screen or the launcher, wherever you came from — and keeps running with
     * its state, in Recents. Only a broken window (a dead or hung renderer) is
     * ever closed.
     */
    private fun leave() {
        backs.reset()
        moveTaskToBack(true)
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
        reviewWatch?.cancel()
        frameJob?.cancel()
        inbound.close()
        replyChannel = null
        // Drop the session with the window. Rust ignores every later frame for
        // it, so a leaked WebView cannot keep a capability session alive. An
        // open still in flight sees [windowGone] and closes its own session.
        val toClose = synchronized(sessionLock) {
            windowGone = true
            sessionId
        }
        if (toClose.isNotEmpty()) client.nappletClose(toClose)
        // Fine on a view a renderer crash already detached: `destroy` wants the
        // view out of the hierarchy, not in it.
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

        /** See [inFlight]. */
        private const val MAX_IN_FLIGHT = 8

        /** See [inbound]. */
        private const val INBOUND_CAPACITY = 64

        /** A touch this recent stands in for a gesture behind a web link. See [openLink]. */
        private const val GESTURE_WINDOW_MS = 5_000L

        /** How often the review slot is re-read while the sheet is up. */
        private const val REVIEW_POLL_MS = 400L

        /** How long a requested review may take to appear in state before giving up. */
        private const val REVIEW_APPEAR_MS = 3_000L

        /** The top-level `channel` of a runtime frame, or null if it is not one. */
        private fun channelOf(frame: String): String? =
            runCatching { JSONObject(frame).optString("channel") }.getOrNull()?.ifEmpty { null }

        /**
         * The `type` of a napplet-bound message inside a runtime frame, or null.
         * Parsed rather than searched for: the message body carries text the
         * napplet — or a stranger whose event it subscribed to — chose.
         */
        private fun relayedType(frame: String): String? =
            runCatching {
                val obj = JSONObject(frame)
                if (obj.optString("channel") != "napplet") return null
                obj.optJSONObject("message")?.optString("type")
            }.getOrNull()?.ifEmpty { null }

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

        /**
         * The intent that opens a napplet as its own fullscreen task — from the
         * Apps grid, a home-screen shortcut, or an added review's Open — so all
         * of them land in the same task rather than a second card for one app.
         */
        fun intent(context: Context, pointer: String, title: String): Intent =
            Intent(context, NappletActivity::class.java).apply {
                action = Intent.ACTION_VIEW
                // Keyed on the addressable pointer, not the napplet's identity:
                // its identity is its aggregate hash and changes every build, so
                // keying the task on it would strand the Recents card on update.
                data = documentUri(pointer)
                putExtra(EXTRA_POINTER, pointer)
                putExtra(EXTRA_TITLE, title)
                addFlags(Intent.FLAG_ACTIVITY_NEW_DOCUMENT)
            }
    }
}

/**
 * Serves the shell page, and nothing else.
 *
 * The napplet's own bytes never pass through here — they are pushed over the
 * capability channel and assigned to `srcdoc`. Serving them at this origin would
 * make them reachable by URL, and anything navigating to that URL would run the
 * napplet as the shell origin.
 *
 * Shell bytes land in the **main frame only**, and that is enforced here in
 * [shouldInterceptRequest], not only in [shouldOverrideUrlLoading]: Chromium
 * does not offer subframe http(s) navigations to the latter, so a napplet
 * setting `location.href` to the shell URL arrives here as a subframe request
 * and is refused. Inert while the sandbox keeps the frame's origin opaque, but
 * the invariant is checked rather than assumed.
 *
 * @param onRendererGone the renderer process died; the window closes itself.
 * @param onUnhandledEscape an Escape the page left unhandled; true when the
 *   window took it (see `NappletActivity.onBack`).
 */
private class NappletWebViewClient(
    private val client: AppCoreClient,
    private val shellHost: String,
    private val onContentVisible: () -> Unit,
    private val onRendererGone: () -> Unit,
    private val onUnhandledEscape: (KeyEvent) -> Boolean,
) : WebViewClient() {

    /**
     * Only an Escape is looked at: the back gesture arrives as one, and when
     * the page does not consume it, this is where it comes back. Everything
     * else keeps WebView's default, which hands it to the view hierarchy.
     */
    override fun onUnhandledKeyEvent(view: WebView, event: KeyEvent) {
        if (event.keyCode == KeyEvent.KEYCODE_ESCAPE && onUnhandledEscape(event)) return
        super.onUnhandledKeyEvent(view, event)
    }

    override fun onPageCommitVisible(view: WebView, url: String) = onContentVisible()

    override fun onPageFinished(view: WebView, url: String) = onContentVisible()

    /**
     * The renderer died — a napplet that allocated until OOM, or any crash in
     * the page. Returning `true` is what keeps WebView from killing the app
     * process (its default on API 26+): the mesh node, the relay, the blob
     * store and every other window stay up, and only this window closes.
     * The dead view leaves the hierarchy first so nothing paints or scripts
     * against it; the Activity's `onDestroy` still calls `destroy()` on it.
     */
    override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
        Log.w(
            "NappletActivity",
            "napplet renderer gone: crashed=${detail.didCrash()} " +
                "priority=${detail.rendererPriorityAtExit()}; closing the window",
        )
        (view.parent as? ViewGroup)?.removeView(view)
        onRendererGone()
        return true
    }

    /**
     * The shell never navigates, and the napplet's frame never leaves.
     *
     * This is the boundary that keeps the shell origin the shell's: a navigation
     * away and back, or into an nsite host, would put other content inside the
     * origin the capability channel is scoped to.
     *
     * The napplet's iframe is `sandbox="allow-scripts"`, which still lets it
     * navigate *itself* — `location.href = …` — and this callback fires for
     * that too. Refused outright: a subframe navigation is either the napplet
     * trying to reach the network around its CSP, or trying to fire a system
     * intent (`intent:`, `tel:`, a browser) with no one having tapped anything.
     * A link the shell itself would open externally needs a gesture behind it.
     */
    override fun shouldOverrideUrlLoading(
        view: WebView,
        request: WebResourceRequest,
    ): Boolean {
        val uri = request.url
        // The shell's own initial load is not a navigation request.
        if (request.isForMainFrame &&
            uri.host?.equals(shellHost, ignoreCase = true) == true &&
            uri.path == "/"
        ) {
            return false
        }
        if (!request.isForMainFrame) {
            Log.w("NappletActivity", "napplet frame tried to navigate to $uri; refused")
            return true
        }
        if (ExternalNavigation.staysInPage(uri)) return true
        if (!request.hasGesture()) {
            Log.w("NappletActivity", "shell navigation to $uri without a gesture; refused")
            return true
        }
        return ExternalNavigation.openExternally(view.context, uri, "NappletActivity")
    }

    override fun shouldInterceptRequest(
        view: WebView,
        request: WebResourceRequest,
    ): WebResourceResponse? {
        val uri = request.url
        val host = uri.host.orEmpty()

        // This window's shell origin, and only it. Everything else is answered
        // with a refusal rather than handed to the network: the shell loads
        // nothing external, and the napplet's CSP already forbids it — this is
        // the belt to that braces, so a request that slips past the CSP (a
        // frame navigation, a WebView quirk) still reaches nothing. Another
        // napplet's shell origin is refused here just as firmly as an nsite
        // host: each window serves itself.
        if (!host.equals(shellHost, ignoreCase = true)) {
            return WebResourceResponse(
                "text/plain",
                "utf-8",
                403,
                "Forbidden",
                emptyMap(),
                ByteArrayInputStream(ByteArray(0)),
            )
        }

        // The shell page is a main-frame document, never a subframe's. The
        // napplet's iframe navigating itself here would otherwise be handed
        // the trusted shell page, and `shouldOverrideUrlLoading` never sees
        // that navigation (see the class doc).
        if (!request.isForMainFrame) {
            Log.w("NappletActivity", "subframe request for shell path ${uri.path}; refused")
            return WebResourceResponse(
                "text/plain",
                "utf-8",
                403,
                "Forbidden",
                emptyMap(),
                ByteArrayInputStream(ByteArray(0)),
            )
        }

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
