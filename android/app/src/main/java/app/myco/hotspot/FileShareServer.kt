package app.myco.hotspot

import android.util.Log
import fi.iki.elonen.NanoHTTPD
import java.io.FileInputStream
import java.io.InputStream
import java.net.URLDecoder
import java.net.URLEncoder

/**
 * The web page a hotspot guest lands on: a plain-HTML file list with download
 * links and an upload form. Served on every interface (the guest reaches it
 * via the hotspot's gateway address); state lives in [SharedFiles].
 *
 * Framework-free HTML with one inline script: the form submits each picked
 * file as a raw `PUT /upload/<urlencoded-name>`, because NanoHTTPD 2.3.1's
 * multipart parser surfaces only the first of several same-named file parts
 * and mangles non-ASCII filenames (it decodes part headers as ASCII). The
 * multipart POST stays as the no-JS fallback.
 */
class FileShareServer(
    private val files: SharedFiles,
    port: Int,
) : NanoHTTPD(port) {

    override fun serve(session: IHTTPSession): Response = try {
        when {
            session.method == Method.GET && session.uri == "/" -> {
                val sent = session.parameters["sent"]?.firstOrNull()?.toIntOrNull()
                page(banner = sent?.let { "Received $it file${if (it == 1) "" else "s"}." })
            }
            session.method == Method.GET && session.uri.startsWith(FILE_PREFIX) ->
                download(session.uri.removePrefix(FILE_PREFIX))
            session.method == Method.PUT && session.uri.startsWith(UPLOAD_PREFIX) ->
                uploadPut(session)
            session.method == Method.POST && session.uri == "/upload" -> upload(session)
            else -> newFixedLengthResponse(Response.Status.NOT_FOUND, MIME_PLAINTEXT, "not found\n")
        }
    } catch (e: Exception) {
        Log.w(TAG, "request ${session.method} ${session.uri} failed", e)
        newFixedLengthResponse(Response.Status.INTERNAL_ERROR, MIME_PLAINTEXT, "error\n")
    }

    private fun download(id: String): Response {
        val entry = files.list().firstOrNull { it.id == id }
            ?: return newFixedLengthResponse(Response.Status.NOT_FOUND, MIME_PLAINTEXT, "gone\n")
        // Blocks this request thread until the owner taps Allow — the guest's
        // browser sits on a spinning tab, then the download starts (or 403s).
        val allowed = TransferGate.request(TransferGate.Direction.DOWNLOAD, entry.name, entry.size)
        Log.i(TAG, "download '${entry.name}' ${if (allowed) "allowed" else "denied"}")
        if (!allowed) return denied()
        val (stream, size) = files.open(id)
            ?: return newFixedLengthResponse(Response.Status.NOT_FOUND, MIME_PLAINTEXT, "gone\n")
        val resp = if (size > 0) {
            newFixedLengthResponse(Response.Status.OK, SharedFiles.mimeFor(entry.name), stream, size)
        } else {
            newChunkedResponse(Response.Status.OK, SharedFiles.mimeFor(entry.name), stream)
        }
        // RFC 5987 filename* so non-ASCII names survive; quoted fallback for
        // browsers that ignore it.
        val encoded = URLEncoder.encode(entry.name, "UTF-8").replace("+", "%20")
        resp.addHeader(
            "Content-Disposition",
            "attachment; filename=\"${entry.name.replace("\"", "_")}\"; filename*=UTF-8''$encoded",
        )
        return resp
    }

    /** The primary upload path: one raw body per file, its name in the URL. */
    private fun uploadPut(session: IHTTPSession): Response {
        val name = URLDecoder.decode(session.uri.removePrefix(UPLOAD_PREFIX), "UTF-8")
            .substringAfterLast('/').substringAfterLast('\\').ifBlank { "upload.bin" }
        // The socket does not EOF between keep-alive requests, so the copy must
        // stop at Content-Length rather than reading to end-of-stream.
        val len = session.headers["content-length"]?.toLongOrNull()
            ?: return newFixedLengthResponse(Response.Status.BAD_REQUEST, MIME_PLAINTEXT, "length required\n")
        // Ask the owner *before* reading the body — TCP backpressure holds the
        // guest's send while the request waits. A denial answers without ever
        // consuming the body, so the connection must close rather than let the
        // unread bytes garble the next keep-alive request.
        val allowed = TransferGate.request(TransferGate.Direction.UPLOAD, name, len)
        Log.i(TAG, "upload '$name' ($len B) ${if (allowed) "allowed" else "denied"}")
        if (!allowed) {
            return denied().apply { addHeader("Connection", "close") }
        }
        val ok = files.saveUpload(name, BoundedInputStream(session.inputStream, len))
        return if (ok) {
            newFixedLengthResponse(Response.Status.OK, MIME_PLAINTEXT, "ok\n")
        } else {
            newFixedLengthResponse(Response.Status.INTERNAL_ERROR, MIME_PLAINTEXT, "failed\n")
        }
    }

    /** No-JS fallback (multipart form POST) — see the class doc's caveats. */
    private fun upload(session: IHTTPSession): Response {
        // parseBody spools each part to a temp file and fills `body` with
        // field-name -> temp path ("file", "file2", …); parameters["file"]
        // carries whatever client filenames the parser managed to decode.
        val body = HashMap<String, String>()
        session.parseBody(body)
        val names = session.parameters["file"].orEmpty()
        var saved = 0
        var i = 0
        while (true) {
            val tmp = body[if (i == 0) "file" else "file${i + 1}"] ?: break
            val clientName = names.getOrNull(i)?.substringAfterLast('/')?.substringAfterLast('\\')
            i++
            // An empty <input type=file> still submits one nameless, empty part.
            if (clientName.isNullOrBlank() && java.io.File(tmp).length() == 0L) continue
            val name = clientName?.takeIf { it.isNotBlank() } ?: "upload-$i.bin"
            // The bytes are already in the temp file, but nothing is kept
            // without the owner's OK.
            if (!TransferGate.request(TransferGate.Direction.UPLOAD, name, java.io.File(tmp).length())) continue
            FileInputStream(tmp).use { if (files.saveUpload(name, it)) saved++ }
        }
        // Re-render instead of redirecting, so the confirmation and the fresh
        // list arrive in the same response.
        return page(banner = if (saved > 0) "Received $saved file${if (saved == 1) "" else "s"}." else "Nothing was uploaded.")
    }

    private fun page(banner: String?): Response {
        val rows = files.list().joinToString("\n") { e ->
            """<li><a href="$FILE_PREFIX${e.id}" download>${esc(e.name)}</a> <span>${human(e.size)}</span></li>"""
        }
        val listing = if (rows.isEmpty()) "<p class=\"empty\">No files shared yet.</p>" else "<ul>\n$rows\n</ul>"
        val note = banner?.let { "<p class=\"banner\">${esc(it)}</p>" } ?: ""
        val html = """
            <!doctype html>
            <html lang="en">
            <meta charset="utf-8">
            <meta name="viewport" content="width=device-width, initial-scale=1">
            <title>Myco file share</title>
            <style>
              body { font-family: system-ui, sans-serif; margin: 0 auto; max-width: 32rem; padding: 1.2rem; }
              h1 { font-size: 1.3rem; } h2 { font-size: 1rem; margin-top: 1.6rem; }
              ul { list-style: none; padding: 0; }
              li { display: flex; justify-content: space-between; gap: 1rem; padding: .45rem 0; border-bottom: 1px solid #ddd; }
              li span { color: #777; white-space: nowrap; }
              a { word-break: break-all; }
              .banner { background: #e6f4ea; border: 1px solid #b7dfc2; padding: .5rem .8rem; border-radius: .5rem; }
              .empty { color: #777; }
              form { margin-top: .6rem; }
              button { margin-top: .6rem; padding: .45rem 1rem; }
            </style>
            <h1>Myco file share</h1>
            <p class="empty">Every transfer waits for an OK on this phone — give it a moment.</p>
            $note
            <h2>Download</h2>
            $listing
            <h2>Send files to this phone</h2>
            <form method="post" action="/upload" enctype="multipart/form-data">
              <input type="file" name="file" multiple>
              <button type="submit">Upload</button>
            </form>
            <script>
            // One raw PUT per file (see the server's uploadPut); the form's
            // multipart POST remains as the no-JS fallback.
            const form = document.querySelector('form');
            form.addEventListener('submit', async (e) => {
              e.preventDefault();
              const files = form.querySelector('input[type=file]').files;
              if (!files.length) return;
              const btn = form.querySelector('button');
              btn.disabled = true; btn.textContent = 'Waiting for the OK…';
              let sent = 0;
              for (const f of files) {
                try {
                  const r = await fetch('$UPLOAD_PREFIX' + encodeURIComponent(f.name), {method: 'PUT', body: f});
                  if (r.ok) sent++;
                } catch (_) {}
              }
              location.href = '/?sent=' + sent;
            });
            </script>
            </html>
        """.trimIndent()
        val resp = newFixedLengthResponse(Response.Status.OK, MIME_HTML, html)
        resp.addHeader("Cache-Control", "no-store")
        return resp
    }

    private fun denied(): Response = newFixedLengthResponse(
        Response.Status.FORBIDDEN, MIME_PLAINTEXT,
        "The phone's owner did not approve this transfer.\n",
    )

    private fun esc(s: String): String = s
        .replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace("\"", "&quot;")

    private fun human(bytes: Long): String = when {
        bytes >= 1L shl 30 -> "%.1f GB".format(bytes.toDouble() / (1L shl 30))
        bytes >= 1L shl 20 -> "%.1f MB".format(bytes.toDouble() / (1L shl 20))
        bytes >= 1L shl 10 -> "%.0f kB".format(bytes.toDouble() / (1L shl 10))
        bytes > 0 -> "$bytes B"
        else -> ""
    }

    companion object {
        private const val TAG = "MycoFileShare"
        private const val FILE_PREFIX = "/files/"
        private const val UPLOAD_PREFIX = "/upload/"
    }
}

/** Reads at most `remaining` bytes from `src`, then reports end-of-stream. */
private class BoundedInputStream(
    private val src: InputStream,
    private var remaining: Long,
) : InputStream() {
    override fun read(): Int {
        if (remaining <= 0) return -1
        val b = src.read()
        if (b >= 0) remaining--
        return b
    }

    override fun read(b: ByteArray, off: Int, len: Int): Int {
        if (remaining <= 0) return -1
        val n = src.read(b, off, minOf(len.toLong(), remaining).toInt())
        if (n > 0) remaining -= n
        return n
    }
}
