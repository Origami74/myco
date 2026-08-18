package app.myco.share

import android.bluetooth.BluetoothManager
import android.content.Context
import android.provider.Settings
import java.security.MessageDigest

/**
 * This device's **memorable name** — what nearby devices see when you pair.
 *
 * Three sources, in falling order of "would a person recognise this":
 *
 *  1. the user's own override, if they typed one;
 *  2. the phone's own name (Settings ▸ About phone ▸ Device name) — the name
 *     they already gave this handset, and the one it announces over Bluetooth
 *     and Cast, so it is the name they expect to see;
 *  3. a colour+name pair derived from the device npub, deterministic so the
 *     same npub always reads the same on every screen that shows it.
 *
 * The name only ever rides inside `myco://pair` payloads and outgoing pair
 * requests (the receiver shows it before accepting), so a client-side override
 * is the whole story; the core doesn't store our own name.
 *
 * The phone name defaulting ahead of the generated one is a deliberate trade:
 * it is far more recognisable across a table, but it often carries a real name
 * ("Arjen's S21"), so [suggestions] always offers the pseudonymous generated
 * name as a one-tap alternative rather than burying it behind typing.
 */
object DeviceName {
    private const val PREFS = "myco_prefs"
    private const val KEY = "device_name"

    /** Longest name we'll accept or emit — matches the editor's input cap. */
    const val MAX_LENGTH = 40

    // Kept short and speakable — the name doubles as a say-it-out-loud check.
    //
    // 32 × 64 = 2048 combinations. The old 12 × 12 = 144 was the whole reason
    // duplicates turned up: by the birthday bound a room of 14 devices was
    // already even money for a collision, and a Circle of 20 was near-certain.
    // At 2048 the same bet needs ~53 devices.
    private val COLORS = listOf(
        "green", "blue", "amber", "violet", "teal", "coral",
        "indigo", "rose", "olive", "cyan", "ruby", "slate",
        "jade", "plum", "sand", "mint", "rust", "navy",
        "lilac", "ochre", "moss", "peach", "cobalt", "umber",
        "saffron", "crimson", "azure", "sage", "copper", "orchid",
        "pewter", "hazel",
    )
    private val NAMES = listOf(
        "sammy", "james", "rosa", "otto", "lena", "milo",
        "ada", "kai", "nova", "finn", "juno", "remy",
        "iris", "theo", "mika", "zola", "arlo", "nina",
        "pablo", "suki", "dara", "elio", "mona", "yuri",
        "bex", "cleo", "dario", "esme", "fenn", "gia",
        "hugo", "ines", "jonas", "kira", "luca", "maya",
        "nils", "orla", "pia", "quinn", "rune", "sana",
        "tariq", "uma", "vera", "wren", "zane", "bruno",
        "cora", "dev", "eero", "faye", "gus", "hana",
        "ivo", "jules", "koa", "lars", "mira", "noor",
        "oona", "piet", "rafa", "tess",
    )

    /**
     * A deterministic colour+name derived from the npub (same npub → same name).
     *
     * Keyed on SHA-256 rather than `String.hashCode()`: the old hash fed one
     * 32-bit value to both list lookups, so the colour and the name were drawn
     * from correlated bits of the same number and the pair space was smaller
     * than 32 × 64 in practice. Separate digest bytes make the two independent.
     */
    fun generated(ownNpub: String): String {
        if (ownNpub.isEmpty()) return "new device"
        val digest = MessageDigest.getInstance("SHA-256").digest(ownNpub.toByteArray())
        fun byteAt(i: Int) = digest[i].toInt() and 0xff
        // Two bytes per index so the modulo bias stays far below one part in a
        // list length, and disjoint bytes so the two picks are independent.
        val color = ((byteAt(0) shl 8) or byteAt(1)) % COLORS.size
        val name = ((byteAt(2) shl 8) or byteAt(3)) % NAMES.size
        return "${COLORS[color]} ${NAMES[name]}"
    }

    /**
     * The phone's own name, as the user set it in Settings ▸ About phone.
     *
     * `Settings.Global.DEVICE_NAME` is the canonical one and needs no
     * permission. Some OEM builds leave it unset, so fall back to the Bluetooth
     * adapter name (the same string on most devices) and then to the legacy
     * secure setting. Null when none of them has anything usable.
     */
    fun phoneName(context: Context): String? {
        val resolver = context.contentResolver
        val candidates = sequence {
            yield(runCatching { Settings.Global.getString(resolver, Settings.Global.DEVICE_NAME) }.getOrNull())
            // Throws SecurityException on API 31+ until BLUETOOTH_CONNECT is
            // granted, which on a first run it is not — hence runCatching.
            yield(
                runCatching {
                    context.getSystemService(BluetoothManager::class.java)?.adapter?.name
                }.getOrNull(),
            )
            yield(runCatching { Settings.Secure.getString(resolver, "bluetooth_name") }.getOrNull())
        }
        return candidates.firstNotNullOfOrNull { it?.trim()?.take(MAX_LENGTH)?.ifBlank { null } }
    }

    /** The user's override if set, otherwise the phone's own name, otherwise generated. */
    fun current(context: Context, ownNpub: String): String {
        val override = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY, "") ?: ""
        return override.ifBlank { phoneName(context) ?: generated(ownNpub) }
    }

    /**
     * The names worth offering as a single tap, best-first and de-duplicated.
     *
     * Always at least the generated name, so the pseudonymous option is never
     * more than one tap away even on a phone whose own name is unreadable.
     */
    fun suggestions(context: Context, ownNpub: String): List<String> =
        listOfNotNull(phoneName(context), generated(ownNpub)).distinct()

    fun set(context: Context, name: String) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit().putString(KEY, name.trim().take(MAX_LENGTH)).apply()
    }
}
