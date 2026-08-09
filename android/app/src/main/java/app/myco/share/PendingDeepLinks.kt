package app.myco.share

import android.content.Context
import org.json.JSONObject

/**
 * Deep links waiting for their app to finish arriving.
 *
 * Tapping `myco://app/<host>/dumpling/…` for an app you don't have yet starts a mesh
 * retrieval that may take seconds — or days, if nobody carrying that app has been near
 * you since. The link has to outlive all of it: the sync, the app being swiped away,
 * the process being killed, the phone being rebooted. So the pending path lives in
 * SharedPreferences, not in memory, and is consumed on the app's **first** open —
 * whether Myco opens it automatically the moment it turns ready, or the user taps it in
 * the Apps grid themselves a week later.
 *
 * Stored as one JSON object, `{ "<host>": { "path": …, "ts": … } }`, under a single key:
 * a handful of entries at most, and one write per change beats a key per host.
 */
object PendingDeepLinks {
    private const val KEY = "pending_deep_links"

    /** How long a pending link is worth honouring. Past this, the moment has passed:
     *  landing someone on a month-old link is more confusing than opening the app. */
    private const val TTL_MS = 30L * 24 * 60 * 60 * 1000

    private const val FIELD_PATH = "path"
    private const val FIELD_TS = "ts"

    private fun prefs(ctx: Context) = ctx.getSharedPreferences("myco_prefs", Context.MODE_PRIVATE)

    private fun read(ctx: Context): JSONObject =
        runCatching { JSONObject(prefs(ctx).getString(KEY, "{}").orEmpty()) }.getOrElse { JSONObject() }

    private fun write(ctx: Context, obj: JSONObject) {
        prefs(ctx).edit().putString(KEY, obj.toString()).apply()
    }

    /** Remember that [host] should open at [path] once it is installed. Last link wins:
     *  someone who sends a second link means the second one. */
    fun put(ctx: Context, host: String, path: String, now: Long = System.currentTimeMillis()) {
        if (host.isEmpty()) return
        val obj = read(ctx)
        obj.put(host, JSONObject().put(FIELD_PATH, path).put(FIELD_TS, now))
        write(ctx, obj)
    }

    /** The path pending for [host], without consuming it. Null if none or expired. */
    fun peek(ctx: Context, host: String, now: Long = System.currentTimeMillis()): String? {
        val entry = read(ctx).optJSONObject(host) ?: return null
        if (now - entry.optLong(FIELD_TS) > TTL_MS) return null
        return entry.optString(FIELD_PATH).takeIf { it.isNotEmpty() }
    }

    /** The path pending for [host], clearing it — it is spent on this one open. */
    fun take(ctx: Context, host: String, now: Long = System.currentTimeMillis()): String? {
        val path = peek(ctx, host, now)
        remove(ctx, host)
        return path
    }

    fun remove(ctx: Context, host: String) {
        val obj = read(ctx)
        if (!obj.has(host)) return
        obj.remove(host)
        write(ctx, obj)
    }

    /** Every host with a live pending link, expired entries swept as a side effect. */
    fun hosts(ctx: Context, now: Long = System.currentTimeMillis()): Set<String> {
        val obj = read(ctx)
        val live = mutableSetOf<String>()
        var expired = false
        for (host in obj.keys()) {
            val entry = obj.optJSONObject(host)
            if (entry != null && now - entry.optLong(FIELD_TS) <= TTL_MS) live += host else expired = true
        }
        if (expired) {
            val kept = JSONObject()
            for (host in live) kept.put(host, obj.get(host))
            write(ctx, kept)
        }
        return live
    }
}
