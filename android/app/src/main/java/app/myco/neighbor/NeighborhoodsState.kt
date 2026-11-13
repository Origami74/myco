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
) {
    data class Neighborhood(
        val id: String,
        val name: String,
        val autoAccept: Boolean,
        val everyone: Boolean,
        val links: Int,
        val publishHops: Int,
        val backlogHops: Int,
        val blobHops: Int,
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
 * A scanned link invitation, read just far enough to show what it offers.
 * The core checks everything again; this is only for the picker.
 */
data class InvitePreview(val raw: String, val offered: List<NeighborhoodsState.Pending>) {
    companion object {
        fun read(text: String): InvitePreview? = runCatching {
            val bytes = Base64.decode(text.trim(), Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP)
            val o = JSONObject(String(bytes))
            if (o.optInt("v") != 1) return null
            val n = o.optJSONArray("n") ?: JSONArray()
            InvitePreview(
                raw = text.trim(),
                offered = (0 until n.length()).mapNotNull { n.optJSONObject(it) }.map {
                    NeighborhoodsState.Pending(it.optString("id"), it.optString("name"))
                },
            )
        }.getOrNull()
    }
}
