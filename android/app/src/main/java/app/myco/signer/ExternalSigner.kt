package app.myco.signer

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.util.Log
import app.myco.core.AppCoreClient
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit

/**
 * The Android half of signing with a signer app (NIP-55 — Amber).
 *
 * The Rust core queues each request and waits; this pumps the queue on one
 * background thread, the same long-poll shape as the napplet frame pump. For
 * each request it asks the signer's content resolver first — that answers in
 * the background once the user chose "remember" in the signer — and falls back
 * to the signer's approval screen through [SignerActivity]. Requests are
 * handled one at a time: a signer shows one approval at once.
 *
 * Nothing here is trusted by the core: the signed event it hands back is
 * checked against the request before anyone sees it.
 */
object ExternalSigner {
    private const val TAG = "myco-signer"
    private const val POLL_MS = 30_000L

    /** How long one approval screen may stay open; the core gives up at 120 s. */
    private const val APPROVAL_TIMEOUT_S = 115L

    /** Requests waiting on [SignerActivity], by request id. */
    private val approvals = ConcurrentHashMap<String, CompletableFuture<Result<String>>>()

    @Volatile
    private var started = false

    /** Start the pump, once per process. */
    fun start(context: Context, client: AppCoreClient) {
        synchronized(this) {
            if (started) return
            started = true
        }
        val app = context.applicationContext
        Thread({
            while (true) {
                // Nothing on this thread may throw out of the loop: an uncaught
                // exception on it would take the whole process down.
                val handled = runCatching {
                    val json = client.signerNextRequest(POLL_MS)
                    if (json.isEmpty()) return@runCatching
                    val req = Request.parse(json)
                    val answer = runCatching { handle(app, req) }.getOrElse { Result.failure(it) }
                    answer.fold(
                        onSuccess = { client.signerRespond(req.id, it, "") },
                        onFailure = { client.signerRespond(req.id, "", it.message ?: "signer failed") },
                    )
                }
                if (handled.isFailure) {
                    Log.w(TAG, "signer pump", handled.exceptionOrNull())
                    Thread.sleep(1_000)
                }
            }
        }, "myco-signer").apply { isDaemon = true }.start()
    }

    /** Whether a NIP-55 signer app is installed. */
    fun isInstalled(context: Context): Boolean =
        context.packageManager
            .queryIntentActivities(Intent(Intent.ACTION_VIEW, Uri.parse("nostrsigner:")), 0)
            .isNotEmpty()

    /**
     * The intent for logging in: `get_public_key`, asking up front for the
     * permissions Myco will use, so the user can let the signer answer them
     * in the background.
     */
    fun loginIntent(): Intent =
        Intent(Intent.ACTION_VIEW, Uri.parse("nostrsigner:")).apply {
            putExtra("type", "get_public_key")
            putExtra(
                "permissions",
                JSONArray()
                    .put(JSONObject().put("type", "sign_event"))
                    .toString(),
            )
        }

    /** The signer app's name for display, or its package when it has none. */
    fun label(context: Context, pkg: String): String = runCatching {
        val pm = context.packageManager
        pm.getApplicationLabel(pm.getApplicationInfo(pkg, 0)).toString()
    }.getOrDefault(pkg)

    private fun handle(context: Context, req: Request): Result<String> {
        if (req.type != "sign_event") {
            return Result.failure(IllegalArgumentException("unsupported signer request ${req.type}"))
        }
        // Background first: no screen when the user already said "remember".
        when (val quiet = ContentResolverSigner.signEvent(context, req)) {
            is ContentResolverSigner.Answer.Signed -> return Result.success(quiet.value)
            is ContentResolverSigner.Answer.Rejected ->
                // NIP-55: an "always reject" is final — no approval screen.
                return Result.failure(IllegalStateException("rejected"))
            ContentResolverSigner.Answer.NotRemembered -> Unit
        }
        // Started from the application context. Android 10+ blocks activity
        // starts from the background, so this only shows while a Myco window
        // is in front — which is where signing requests come from (an open
        // napplet). Otherwise the request fails when the wait below runs out.
        val future = CompletableFuture<Result<String>>()
        approvals[req.id] = future
        try {
            context.startActivity(SignerActivity.intent(context, req))
            return future.get(APPROVAL_TIMEOUT_S, TimeUnit.SECONDS)
        } catch (e: Exception) {
            Log.w(TAG, "signer approval failed", e)
            return Result.failure(IllegalStateException("the signer app did not answer"))
        } finally {
            approvals.remove(req.id)
        }
    }

    /** [SignerActivity]'s answer to request [id]. */
    internal fun complete(id: String, result: Result<String>) {
        approvals[id]?.complete(result)
    }

    data class Request(
        val id: String,
        val type: String,
        val payload: String,
        val currentUser: String,
        val pkg: String,
    ) {
        companion object {
            fun parse(json: String): Request {
                val o = JSONObject(json)
                return Request(
                    id = o.getString("id"),
                    type = o.getString("type"),
                    payload = o.getString("payload"),
                    currentUser = o.getString("currentUser"),
                    pkg = o.getString("package"),
                )
            }
        }
    }
}

/** NIP-55's background path: `content://<package>.SIGN_EVENT`. */
internal object ContentResolverSigner {
    sealed interface Answer {
        data class Signed(val value: String) : Answer
        data object Rejected : Answer
        /** The user has not let the signer answer this without asking. */
        data object NotRemembered : Answer
    }

    fun signEvent(context: Context, req: ExternalSigner.Request): Answer {
        val cursor = runCatching {
            context.contentResolver.query(
                Uri.parse("content://${req.pkg}.SIGN_EVENT"),
                arrayOf(req.payload, "", req.currentUser),
                null,
                null,
                null,
            )
        }.getOrNull() ?: return Answer.NotRemembered
        cursor.use { c ->
            if (c.getColumnIndex("rejected") > -1) return Answer.Rejected
            if (!c.moveToFirst()) return Answer.NotRemembered
            // The signed event when the signer sends one, else the signature;
            // the core accepts either and checks both.
            val event = c.getColumnIndex("event").takeIf { it > -1 }?.let { c.getString(it) }
            val result = c.getColumnIndex("result").takeIf { it > -1 }?.let { c.getString(it) }
            val value = event?.takeIf { it.isNotBlank() } ?: result?.takeIf { it.isNotBlank() }
            return value?.let { Answer.Signed(it) } ?: Answer.NotRemembered
        }
    }
}
