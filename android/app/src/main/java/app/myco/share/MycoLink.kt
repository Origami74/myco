package app.myco.share

/**
 * The **app deep link**: `myco://app/<host>/<deep-path…>`.
 *
 * One link that names an app and a place inside it — `myco://app/<host>/dumpling/dmpl1…`
 * is "open this app, at this route". Myco resolves the `<host>` half; everything after
 * it is opaque and handed to the nsite verbatim.
 *
 * **It carries no secrets.** No holder npub, no pairing secret, no token that
 * Myco interprets. A deep link travels through channels nobody controls — a chat
 * message, a printed QR, a URL bar, someone's screenshot — so anything in it is public
 * and replayable, which is the opposite of what a one-time pairing secret needs to be.
 * Pairing has its own carrier: the scanned/tapped `myco://pair/…` and `myco://share/…`
 * payloads in [NsiteShare], exchanged face to face.
 *
 * Losing the holder hint costs nothing: `Content::open_site` with `holder = None`
 * already tries every Circle peer in turn and then the public fallback, so a link with
 * no sharer attached still retrieves the app from whoever nearby happens to have it.
 *
 * Deliberately plain string parsing (no `android.net.Uri`), so it is a pure function
 * the JVM unit tests can exercise without an emulator.
 */
object MycoLink {
    const val APP_PREFIX = "myco://app/"

    /** An app deep link: which nsite, and where inside it to land. */
    data class AppLink(
        /** The `<host>` label — an `npub1…` root, or `<pubkeyB36><dTag>`. */
        val host: String,
        /** The in-app path, leading slash included, query/fragment intact. `/` if none. */
        val path: String,
    )

    /**
     * Parse `myco://app/<host>[/<path>][?query][#fragment]`, or null if [uri] is not an
     * app deep link (a `myco://pair/…`, a `myco://share/…`, a pasted nsite link, junk).
     *
     * A bare `myco://app/<host>` is valid and means "just open it" — `path = "/"`.
     */
    fun parseAppLink(uri: String): AppLink? {
        val trimmed = uri.trim()
        if (!trimmed.startsWith(APP_PREFIX, ignoreCase = true)) return null
        val rest = trimmed.substring(APP_PREFIX.length)
        if (rest.isEmpty()) return null

        // The host label ends at the path, query, or fragment — whichever comes first.
        val end = rest.indexOfFirst { it == '/' || it == '?' || it == '#' }
        // Lowercased: a QR in alphanumeric mode carries an uppercased URI, and the
        // label is case-insensitive (it is a DNS label). The path is left verbatim —
        // its payload is the app's to interpret, and bech32 must not be case-mixed.
        val host = (if (end < 0) rest else rest.substring(0, end)).lowercase()
        if (!isHostLabel(host)) return null

        val tail = if (end < 0) "" else rest.substring(end)
        if (tail.any { it.isWhitespace() || it.code < 0x20 || it == '\\' }) return null
        val path = when {
            tail.isEmpty() -> "/"
            // A link that jumps straight to a query/fragment still needs a path root.
            tail.startsWith("/") -> tail
            else -> "/$tail"
        }
        return AppLink(host, path)
    }

    /** Build the deep link for [host] at [path] (the share side of [parseAppLink]). */
    fun buildAppLink(host: String, path: String): String {
        val p = if (path.startsWith("/")) path else "/$path"
        return "$APP_PREFIX$host${if (p == "/") "" else p}"
    }

    /**
     * A plausible `<host>` label. Not a full resolve — the core does that, and this
     * side has no secp256k1 — just the shape the label must have to be usable at all:
     * it becomes a DNS label (`<host>.localhost`) for the WebView, so it is limited to
     * what a DNS label may hold.
     */
    private fun isHostLabel(host: String): Boolean =
        host.isNotEmpty() &&
            host.length <= 63 &&
            host.all { it in 'a'..'z' || it in '0'..'9' || it == '-' } &&
            !host.startsWith("-") &&
            !host.endsWith("-")
}
