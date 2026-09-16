package app.myco

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.util.Log

/**
 * What both WebView hosts do with a navigation that is not theirs: hand it to
 * the system, or let the WebView keep it in-page.
 *
 * Shared so the two cannot drift. [NsiteActivity] and [NappletActivity] decide
 * *whether* a request leaves — each has its own origin rule — and this decides
 * *how*, the same way for both.
 */
object ExternalNavigation {
    /** Schemes with no external app handler — the WebView renders them itself. */
    val IN_PAGE_SCHEMES = setOf("data", "blob", "about", "javascript", "file", "content")

    /** Whether the WebView should be left to handle [uri] itself. */
    fun staysInPage(uri: Uri): Boolean {
        val scheme = uri.scheme?.lowercase()
        return scheme == null || scheme in IN_PAGE_SCHEMES
    }

    /**
     * Hand [uri] to the system's default handler; swallow a missing handler.
     * Always returns `true`: the WebView must not navigate to it either way.
     */
    fun openExternally(context: Context, uri: Uri, tag: String): Boolean {
        val intent = Intent(Intent.ACTION_VIEW, uri).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        return try {
            context.startActivity(intent)
            true
        } catch (e: ActivityNotFoundException) {
            // Nothing on the device can open it (e.g. a bare .nsite with no TUN);
            // stay put rather than navigating the WebView to a dead page.
            Log.w(tag, "No handler for $uri", e)
            true
        }
    }
}
