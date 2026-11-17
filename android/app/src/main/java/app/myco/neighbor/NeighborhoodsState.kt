package app.myco.neighbor

import org.json.JSONArray
import org.json.JSONObject

/** The core's `neighborhoods` state: what the link screens show. */
data class NeighborhoodsState(
    val list: List<Neighborhood> = emptyList(),
    val links: List<LinkRow> = emptyList(),
    /** The open invitation, encoded for a QR code. */
    val invite: String? = null,
    /** The last link action's outcome. */
    val status: String = "",
    /** Everyone the user is connected to: the map's nodes. */
    val peers: List<PeerNode> = emptyList(),
    /** The Nearby switch, on or off. */
    val nearby: Nearby = Nearby(),
) {
    /**
     * Nearby: the `nearby` neighborhood, carried on short-lived links to
     * phones in reach. While [on] it is also in [list] (flagged
     * [Neighborhood.nearby]); off, only this is left, so it can be turned
     * back on.
     */
    data class Nearby(
        val on: Boolean = false,
        val id: String = "",
        /** People in reach on a nearby link now. */
        val peers: Int = 0,
        /** The nearby secret to advertise over BLE (8 hex), empty while off. */
        val secret: String = "",
    )

    data class PeerNode(
        val key: String,
        val name: String,
        val reachable: Boolean,
        val neighborhoods: List<String>,
        /** The link this peer is on; null when the core reports none. */
        val linkId: String?,
        /** On a nearby link: in reach for now, not someone you invited. */
        val nearby: Boolean = false,
    )

    data class Neighborhood(
        val id: String,
        val name: String,
        val autoAccept: Boolean,
        val everyone: Boolean,
        /** The runtime's `nearby` neighborhood: switched on and off, not left, renamed or invited to. */
        val nearby: Boolean = false,
        val links: Int,
        val publishHops: Int,
        val backlogHops: Int,
        val blobHops: Int,
        val passOn: Boolean = true,
        /** Extensions whose updates this phone takes from here (`napplet-updates`, `nsite-updates`, `myco-release`). */
        val get: Set<String> = emptySet(),
        /** Extensions whose updates this phone passes on here. */
        val share: Set<String> = emptySet(),
    )

    data class Pending(val id: String, val name: String)

    data class LinkRow(
        val id: String,
        val name: String,
        val neighborhoods: List<String>,
        val pending: List<Pending>,
        /** A nearby link. */
        val ephemeral: Boolean = false,
    )

    fun nameOf(id: String): String = list.firstOrNull { it.id == id }?.name ?: id.take(8)

    companion object {
        fun parse(o: JSONObject?): NeighborhoodsState {
            if (o == null) return NeighborhoodsState()
            return NeighborhoodsState(
                list = o.optJSONArray("list").objects().map {
                    Neighborhood(
                        id = it.optString("id"),
                        name = it.optString("name"),
                        autoAccept = it.optBoolean("autoAccept"),
                        everyone = it.optBoolean("everyone"),
                        nearby = it.optBoolean("nearby"),
                        links = it.optInt("links"),
                        publishHops = it.optInt("publishHops"),
                        backlogHops = it.optInt("backlogHops"),
                        blobHops = it.optInt("blobHops"),
                        passOn = it.optBoolean("passOn", true),
                        get = it.optJSONArray("get").strings().toSet(),
                        share = it.optJSONArray("share").strings().toSet(),
                    )
                },
                links = o.optJSONArray("links").objects().map { l ->
                    LinkRow(
                        id = l.optString("id"),
                        name = l.optString("name"),
                        neighborhoods = l.optJSONArray("neighborhoods").strings(),
                        pending = l.optJSONArray("pending").objects().map {
                            Pending(it.optString("id"), it.optString("name"))
                        },
                        ephemeral = l.optBoolean("ephemeral"),
                    )
                },
                invite = o.optString("invite").takeIf { it.isNotEmpty() },
                peers = o.optJSONArray("peers").objects().map {
                    PeerNode(
                        key = it.optString("key"),
                        name = it.optString("name"),
                        reachable = it.optBoolean("reachable"),
                        neighborhoods = it.optJSONArray("neighborhoods").strings(),
                        linkId = it.optString("linkId").takeIf { s -> s.isNotEmpty() },
                        nearby = it.optBoolean("nearby"),
                    )
                },
                nearby = o.optJSONObject("nearby").let { n ->
                    Nearby(
                        on = n?.optBoolean("on") ?: false,
                        id = n?.optString("id").orEmpty(),
                        peers = n?.optInt("peers") ?: 0,
                        secret = n?.optString("secret").orEmpty(),
                    )
                },
                status = o.optString("status"),
            )
        }

        private fun JSONArray?.objects(): List<JSONObject> =
            if (this == null) emptyList() else (0 until length()).mapNotNull { optJSONObject(it) }

        private fun JSONArray?.strings(): List<String> =
            if (this == null) emptyList() else (0 until length()).map { optString(it) }
    }
}

/**
 * A neighborhood invitation, read just far enough to show what it offers and
 * who from. The core checks everything again. Written as the napplet link
 * `https://napplet.link/neighborhood/join#invite=<base64url(json)>`, the
 * `neighborhood/join` convention the shell handles itself. The payload rides
 * in the fragment so the one-time secret never reaches napplet.link's web
 * server. The older `?invite=` query, the `napplet:neighborhood/join…`
 * spelling and `myco://link/<base64url(json)>` are read too.
 */
data class InvitePreview(
    val raw: String,
    /** What the inviter calls themselves: their claim, not checked. */
    val from: String,
    val offered: List<NeighborhoodsState.Pending>,
    /** Unix seconds after which it no longer works; 0 when it names none. */
    val exp: Long = 0,
    /** The inviter's link public key, hex. */
    val pk: String = "",
    /** How it reached this phone. */
    val source: Source = Source.Link,
) {
    /** How an invitation arrived, shown on the join sheet. */
    enum class Source { Link, Qr, Tap }

    /**
     * A short code for the inviter's link key, `ab12 · cd34`. The invite
     * sheet shows the same one, so the two people can compare. Empty when
     * the invitation carries no key.
     */
    val fingerprint: String
        get() = fingerprintOf(pk)

    /** Whether it has run out at [nowSecs]. */
    fun expired(nowSecs: Long): Boolean = exp in 1..nowSecs

    companion object {
        const val PREFIX = "myco://link/"
        private val LINK_PREFIXES = listOf("https://napplet.link/neighborhood/join", "napplet:neighborhood/join")

        /** `ab12 · cd34` from a hex key, or empty. */
        fun fingerprintOf(pk: String): String {
            val k = pk.trim().lowercase()
            if (k.length < 8 || k.any { it !in "0123456789abcdef" }) return ""
            return "${k.take(4)} · ${k.takeLast(4)}"
        }

        /**
         * The base64url payload of any invitation spelling, or null: the
         * `invite` parameter of the fragment or the query (key in any case,
         * value percent-decoded), or the rest of a `myco://link/…`.
         */
        fun payload(text: String): String? {
            val t = text.trim()
            if (t.startsWith(PREFIX)) return t.removePrefix(PREFIX).ifEmpty { null }
            val prefix = LINK_PREFIXES.firstOrNull { t.startsWith(it, ignoreCase = true) } ?: return null
            val rest = t.substring(prefix.length).removePrefix("/")
            if (rest.isNotEmpty() && rest[0] != '?' && rest[0] != '#') return null
            val hash = rest.indexOf('#')
            val query = if (rest.startsWith("?")) rest.substring(1, if (hash < 0) rest.length else hash) else ""
            val fragment = if (hash < 0) "" else rest.substring(hash + 1)
            return inviteParam(fragment) ?: inviteParam(query)
        }

        private fun inviteParam(params: String): String? = params.split('&').firstNotNullOfOrNull { pair ->
            val key = pair.substringBefore('=')
            if (!pair.contains('=') || !key.equals("invite", ignoreCase = true)) return@firstNotNullOfOrNull null
            runCatching { java.net.URLDecoder.decode(pair.substringAfter('='), "UTF-8") }
                .getOrNull()?.trim()?.takeIf { it.isNotEmpty() }
        }

        fun read(text: String, source: Source = Source.Link): InvitePreview? = runCatching {
            val t = text.trim()
            val payload = payload(t) ?: return null
            // java.util's decoder takes unpadded input, and runs in JVM tests.
            val bytes = java.util.Base64.getUrlDecoder().decode(payload.trimEnd('='))
            val o = JSONObject(String(bytes, Charsets.UTF_8))
            if (o.optInt("v") != 1) return null
            val n = o.optJSONArray("n") ?: JSONArray()
            InvitePreview(
                raw = t,
                from = o.optString("from"),
                exp = o.optLong("exp"),
                pk = o.optString("pk"),
                offered = (0 until n.length()).mapNotNull { n.optJSONObject(it) }.map {
                    NeighborhoodsState.Pending(it.optString("id"), it.optString("name"))
                },
                source = source,
            )
        }.getOrNull()
    }
}

/**
 * An invitation that arrived by scan, tap or link, waiting for the user to
 * confirm, with [InvitePreview.source] saying which. Set from
 * `MainActivity.handleScannedText` / `handleDeepLink` and the scan sheet's
 * paste; the confirm sheet is drawn at the app root, so it shows over
 * whichever tab is open.
 */
object IncomingInvite {
    val pending = androidx.compose.runtime.mutableStateOf<InvitePreview?>(null)
}
