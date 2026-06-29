package app.myco.share

import android.content.Context
import org.json.JSONObject

/**
 * A **single-use** ledger of pairing secrets this device has issued (in its QR /
 * NFC "present" code). A secret is recorded on issue and removed on first
 * [consume]; a second consume of the same secret fails, so a captured/replayed
 * code can't pair twice. Secrets also expire after [TTL_MS]. Persisted so it
 * survives process death between showing a code and the peer echoing it back.
 */
object PairSecrets {
    private const val PREFS = "myco_prefs"
    private const val KEY = "issued_secrets"
    private const val TTL_MS = 30L * 60 * 1000 // 30 minutes

    /** Mint a fresh secret, record it as live, and return it. */
    fun issue(context: Context): String {
        val secret = NsiteShare.newPairSecret()
        val now = System.currentTimeMillis()
        val map = load(context).also { prune(it, now) }
        map[secret] = now
        save(context, map)
        return secret
    }

    /** Returns true exactly once per issued secret: the first time it's seen and
     *  not expired. Removes it, so subsequent calls return false (single-use). */
    fun consume(context: Context, secret: String): Boolean {
        if (secret.isBlank()) return false
        val now = System.currentTimeMillis()
        val map = load(context).also { prune(it, now) }
        val had = map.remove(secret) != null
        save(context, map)
        return had
    }

    private fun prune(map: MutableMap<String, Long>, now: Long) {
        val it = map.entries.iterator()
        while (it.hasNext()) if (now - it.next().value > TTL_MS) it.remove()
    }

    private fun load(context: Context): MutableMap<String, Long> {
        val raw = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY, "") ?: ""
        val map = HashMap<String, Long>()
        if (raw.isNotBlank()) runCatching {
            val o = JSONObject(raw)
            for (k in o.keys()) map[k] = o.optLong(k)
        }
        return map
    }

    private fun save(context: Context, map: Map<String, Long>) {
        val o = JSONObject()
        for ((k, v) in map) o.put(k, v)
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit().putString(KEY, o.toString()).apply()
    }
}
