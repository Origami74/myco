package app.myco.neighbor

import android.util.Base64
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
) {
    data class PeerNode(
        val key: String,
        val name: String,
        val reachable: Boolean,
        val neighborhoods: List<String>,
        /** The link this peer is on; null when the core reports none. */
        val linkId: String?,
    )

    data class Neighborhood(
        val id: String,
        val name: String,
        val autoAccept: Boolean,
        val everyone: Boolean,
        val links: Int,
        val publishHops: Int,
        val backlogHops: Int,
        val blobHops: Int,
        val passOn: Boolean = true,
        /** Fetch and serve apps, updates and files with the people here. */
        val apps: Boolean = true,
    )

    data class Pending(val id: String, val name: String)

    data class LinkRow(
        val id: String,
        val name: String,
        val neighborhoods: List<String>,
        val pending: List<Pending>,
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
                        links = it.optInt("links"),
                        publishHops = it.optInt("publishHops"),
                        backlogHops = it.optInt("backlogHops"),
                        blobHops = it.optInt("blobHops"),
                        passOn = it.optBoolean("passOn", true),
                        apps = it.optBoolean("apps", true),
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
 * A neighborhood invitation, `myco://link/<base64url(json)>` like the share
 * codes, read just far enough to show what it offers and who
 * from. The core checks everything again.
 */
data class InvitePreview(
    val raw: String,
    val from: String,
    val offered: List<NeighborhoodsState.Pending>,
    /** Unix seconds after which it no longer works. */
    val exp: Long = 0,
) {
    companion object {
        const val PREFIX = "myco://link/"

        fun read(text: String): InvitePreview? = runCatching {
            val t = text.trim()
            if (!t.startsWith(PREFIX)) return null
            val bytes = Base64.decode(t.removePrefix(PREFIX), Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP)
            val o = JSONObject(String(bytes))
            if (o.optInt("v") != 1) return null
            val n = o.optJSONArray("n") ?: JSONArray()
            InvitePreview(
                raw = t,
                from = o.optString("from"),
                exp = o.optLong("exp"),
                offered = (0 until n.length()).mapNotNull { n.optJSONObject(it) }.map {
                    NeighborhoodsState.Pending(it.optString("id"), it.optString("name"))
                },
            )
        }.getOrNull()
    }
}

/**
 * An invitation that arrived by scan, tap or link, waiting for the user to
 * confirm. Set from `MainActivity.handleScannedText`; the confirm sheet is
 * drawn at the app root, so it shows over whichever tab is open.
 */
object IncomingInvite {
    val pending = androidx.compose.runtime.mutableStateOf<InvitePreview?>(null)
}
