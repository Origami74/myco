package app.myco.update

import app.myco.core.NativeCore
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONArray
import org.json.JSONObject
import java.io.IOException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

internal object ZapstoreRelease {
    // Same publisher as zapstore.yaml; relay filtering alone is never authentication.
    private const val PUBLISHER = "bbb5dda0e15567979f0543407bdc2033d6f0bbb30f72512a981cfdb2f09e2747"
    private const val SUB = "myco-release"
    private val client = OkHttpClient.Builder().connectTimeout(10, TimeUnit.SECONDS)
        .readTimeout(15, TimeUnit.SECONDS).build()

    fun fetch(): String {
        val done = CountDownLatch(1)
        val tags = mutableListOf<String>()
        var failure: Throwable? = null
        var messages = 0
        val socket = client.newWebSocket(Request.Builder()
            .url("wss://relay.zapstore.dev").build(), object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                val filter = JSONObject().put("kinds", JSONArray().put(30063))
                    .put("authors", JSONArray().put(PUBLISHER))
                    .put("#i", JSONArray().put("app.myco")).put("limit", 100)
                webSocket.send(JSONArray().put("REQ").put(SUB).put(filter).toString())
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                try {
                    require(text.length <= 256 * 1024 && ++messages <= 150) { "Relay response too large" }
                    val frame = JSONArray(text)
                    if (frame.optString(1) != SUB) return
                    when (frame.optString(0)) {
                        "EVENT" -> releaseTag(frame.getJSONObject(2))?.let { tags.add(it) }
                        "EOSE" -> done.countDown()
                        "CLOSED" -> throw IOException("Relay refused release query")
                    }
                } catch (error: Exception) {
                    failure = error
                    done.countDown()
                }
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                failure = t
                done.countDown()
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                failure = IOException("Relay closed before completing release query")
                done.countDown()
            }
        })
        try {
            check(done.await(20, TimeUnit.SECONDS)) { "Release query timed out" }
            failure?.let { throw IOException("Release query failed", it) }
            return tags.maxWithOrNull { a, b ->
                ReleaseVersion.parse(a)!!.compareTo(ReleaseVersion.parse(b)!!)
            } ?: throw IOException("No verified Myco release found")
        } finally {
            socket.cancel()
        }
    }
}

internal fun releaseTag(event: JSONObject,
    verify: (String) -> Boolean = NativeCore::verifySoftwareRelease,
    nowSeconds: Long = System.currentTimeMillis() / 1000): String? {
    if (!verify(event.toString())) return null
    if (event.optLong("created_at", Long.MAX_VALUE) > nowSeconds + 300) return null
    val tags = event.optJSONArray("tags") ?: return null
    fun values(name: String) = (0 until tags.length()).mapNotNull {
        tags.optJSONArray(it)?.takeIf { tag -> tag.optString(0) == name }?.optString(1)
    }
    if (values("i") != listOf("app.myco")) return null
    val version = values("version").singleOrNull() ?: return null
    if (ReleaseVersion.parse(version) == null) return null
    if (values("d") != listOf("app.myco@$version")) return null
    if (values("e").none { it.matches(Regex("[0-9a-f]{64}")) }) return null
    val channel = values("c")
    if (channel.isNotEmpty() && channel != listOf("main")) return null
    return version
}
