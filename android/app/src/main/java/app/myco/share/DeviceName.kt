package app.myco.share

import android.content.Context

/**
 * This device's **memorable name** (colour + name, e.g. "green sammy"). It is
 * auto-generated deterministically from the device npub so the user has something
 * to recognise — and editable, persisted locally. The name only ever rides inside
 * `myco://pair` payloads and outgoing pair requests (the receiver shows it before
 * accepting), so a client-side override is the whole story; the core doesn't store
 * our own name.
 */
object DeviceName {
    private const val PREFS = "myco_prefs"
    private const val KEY = "device_name"

    // Kept short and speakable — the name doubles as a say-it-out-loud check.
    private val COLORS = listOf(
        "green", "blue", "amber", "violet", "teal", "coral",
        "indigo", "rose", "olive", "cyan", "ruby", "slate",
    )
    private val NAMES = listOf(
        "sammy", "james", "rosa", "otto", "lena", "milo",
        "ada", "kai", "nova", "finn", "juno", "remy",
    )

    /** A deterministic colour+name derived from the npub (same npub → same name). */
    fun generated(ownNpub: String): String {
        if (ownNpub.isEmpty()) return "new device"
        val h = ownNpub.hashCode().toLong() and 0xffffffffL
        val color = COLORS[(h % COLORS.size).toInt()]
        val name = NAMES[((h / COLORS.size) % NAMES.size).toInt()]
        return "$color $name"
    }

    /** The user's override if set, otherwise the generated name. */
    fun current(context: Context, ownNpub: String): String {
        val override = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY, "") ?: ""
        return override.ifBlank { generated(ownNpub) }
    }

    fun set(context: Context, name: String) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit().putString(KEY, name.trim()).apply()
    }
}
