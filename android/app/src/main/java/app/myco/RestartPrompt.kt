package app.myco

import android.content.Context

/**
 * Whether a napplet window brought back to the foreground should offer to
 * restart onto a newer version — at most once per napplet and version.
 *
 * A window keeps the version it opened (napplet-runtime.md §7.2), so an update
 * that lands while it sits in the background would otherwise go unused until
 * the task is closed. The offer is made once for a given new version: shown,
 * it is recorded, and neither "I'll restart later" nor a dismiss brings it back
 * for that version. A later update is a different version and may ask again.
 * Restarting needs no record of its own — the new session is current.
 *
 * Kept apart from the Activity so the decision is testable without a device.
 *
 * @param asked the version last offered, per napplet.
 */
class RestartPrompt(private val asked: Store) {

    /** The version last offered for each napplet. */
    interface Store {
        fun get(napplet: String): String?
        fun put(napplet: String, version: String)
    }

    /**
     * Offer [napplet]'s restart now? [newer] is the served version when it
     * moved past the window's, or null while the window is current.
     */
    fun shouldAsk(napplet: String, newer: String?): Boolean =
        newer != null && asked.get(napplet) != newer

    /** The offer for [version] is on screen: never make it again. */
    fun markAsked(napplet: String, version: String) = asked.put(napplet, version)

    companion object {
        /**
         * Its own preferences file. What was offered is a fact about this
         * window's UI, not the napplet's state: the core never needs it, and
         * app data cleared with the app is exactly the lifetime it wants.
         */
        private const val PREFS = "napplet_restart_prompts"

        /** Backed by SharedPreferences, so an answer survives process death. */
        fun persisted(context: Context): RestartPrompt {
            val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            return RestartPrompt(object : Store {
                override fun get(napplet: String): String? = prefs.getString(napplet, null)
                override fun put(napplet: String, version: String) {
                    prefs.edit().putString(napplet, version).apply()
                }
            })
        }
    }
}
