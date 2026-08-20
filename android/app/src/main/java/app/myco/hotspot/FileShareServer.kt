package app.myco.hotspot

import android.util.Log
import fi.iki.elonen.NanoHTTPD
import java.io.FileInputStream
import java.io.InputStream
import java.net.URLDecoder
import java.net.URLEncoder
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import org.json.JSONArray
import org.json.JSONObject

/**
 * The web page a hotspot guest lands on: a plain-HTML file list with download
 * links and an upload form. Served on every interface (the guest reaches it
 * via the hotspot's gateway address); state lives in [SharedFiles].
 *
 * Framework-free HTML styled to the app's AMOLED theme, with one inline
 * script. Uploads go as one raw `PUT /upload/<urlencoded-name>` per picked
 * file, because NanoHTTPD 2.3.1's multipart parser surfaces only the first of
 * several same-named file parts and mangles non-ASCII filenames (it decodes
 * part headers as ASCII) — a multipart POST form stays as the no-JS fallback.
 * The script also polls `/offers` for files the phone is pushing ([Outbox])
 * and pops an accept/decline dialog per offer, AirDrop-style.
 */
class FileShareServer(
    private val files: SharedFiles,
    private val outbox: Outbox,
    port: Int,
) : NanoHTTPD(port) {

    /** Approved-download tokens: token → (file id, expiry). Single-use. */
    private val downloadTokens = ConcurrentHashMap<String, Pair<String, Long>>()

    override fun serve(session: IHTTPSession): Response = try {
        when {
            session.method == Method.GET && session.uri == "/" -> {
                val sent = session.parameters["sent"]?.firstOrNull()?.toIntOrNull()
                page(banner = sent?.let { "Received $it file${if (it == 1) "" else "s"}." })
            }
            // Approval is asked for FIRST, via fetch — so a denial can be a
            // dialog on the page instead of a navigation the browser saves as
            // a "rejected" file. Approval mints a one-time token; the real GET
            // spends it and streams without asking again.
            session.method == Method.POST && session.uri.startsWith(FILE_PREFIX) &&
                session.uri.endsWith(REQUEST_SUFFIX) ->
                requestDownload(session.uri.removePrefix(FILE_PREFIX).removeSuffix(REQUEST_SUFFIX))
            session.method == Method.GET && session.uri.startsWith(FILE_PREFIX) ->
                download(session.uri.removePrefix(FILE_PREFIX), session)
            session.method == Method.PUT && session.uri.startsWith(UPLOAD_PREFIX) ->
                uploadPut(session)
            session.method == Method.POST && session.uri == "/upload" -> upload(session)
            // The push half: the guest's page polls /offers, then accepts
            // (GET, streams the file) or declines (POST) one by id.
            session.method == Method.GET && session.uri == "/offers" -> offersJson()
            session.method == Method.POST && session.uri.startsWith(OFFER_PREFIX) &&
                session.uri.endsWith("/decline") ->
                declineOffer(session.uri.removePrefix(OFFER_PREFIX).removeSuffix("/decline"))
            session.method == Method.GET && session.uri.startsWith(OFFER_PREFIX) ->
                sendOffer(session.uri.removePrefix(OFFER_PREFIX))
            else -> newFixedLengthResponse(Response.Status.NOT_FOUND, MIME_PLAINTEXT, "not found\n")
        }
    } catch (e: Exception) {
        Log.w(TAG, "request ${session.method} ${session.uri} failed", e)
        newFixedLengthResponse(Response.Status.INTERNAL_ERROR, MIME_PLAINTEXT, "error\n")
    }

    /** Ask the owner about one download; blocks this request thread until the
     *  dialog is answered. Allowed → a short-lived single-use token the page
     *  navigates with; denied → 403 the page turns into a dialog. */
    private fun requestDownload(id: String): Response {
        val entry = files.list().firstOrNull { it.id == id }
            ?: return newFixedLengthResponse(Response.Status.NOT_FOUND, MIME_PLAINTEXT, "gone\n")
        val allowed = TransferGate.request(TransferGate.Direction.DOWNLOAD, entry.name, entry.size)
        Log.i(TAG, "download '${entry.name}' ${if (allowed) "allowed" else "denied"}")
        if (!allowed) {
            return newFixedLengthResponse(Response.Status.FORBIDDEN, "application/json", "{\"ok\":false}")
        }
        val token = UUID.randomUUID().toString()
        downloadTokens[token] = id to System.currentTimeMillis() + TOKEN_TTL_MS
        return newFixedLengthResponse(
            Response.Status.OK, "application/json",
            JSONObject().put("ok", true).put("token", token).toString(),
        )
    }

    /** Spend a token minted by [requestDownload]. */
    private fun consumeToken(token: String?, id: String): Boolean {
        if (token == null) return false
        val now = System.currentTimeMillis()
        downloadTokens.entries.removeIf { it.value.second < now }
        val v = downloadTokens.remove(token) ?: return false
        return v.first == id
    }

    private fun download(id: String, session: IHTTPSession): Response {
        val entry = files.list().firstOrNull { it.id == id }
            ?: return newFixedLengthResponse(Response.Status.NOT_FOUND, MIME_PLAINTEXT, "gone\n")
        if (!consumeToken(session.parameters["t"]?.firstOrNull(), id)) {
            // No-JS fallback: gate here, like before. The denial is an HTML
            // page (no attachment header), so even a plain navigation renders
            // it instead of saving a "rejected" file.
            val allowed = TransferGate.request(TransferGate.Direction.DOWNLOAD, entry.name, entry.size)
            Log.i(TAG, "download '${entry.name}' ${if (allowed) "allowed" else "denied"} (no-token path)")
            if (!allowed) return deniedPage()
        }
        val (stream, size) = files.open(id)
            ?: return newFixedLengthResponse(Response.Status.NOT_FOUND, MIME_PLAINTEXT, "gone\n")
        val resp = if (size > 0) {
            newFixedLengthResponse(Response.Status.OK, SharedFiles.mimeFor(entry.name), stream, size)
        } else {
            newChunkedResponse(Response.Status.OK, SharedFiles.mimeFor(entry.name), stream)
        }
        attachAs(resp, entry.name)
        return resp
    }

    /** RFC 5987 filename* so non-ASCII names survive; quoted fallback for
     *  browsers that ignore it. */
    private fun attachAs(resp: Response, name: String) {
        val encoded = URLEncoder.encode(name, "UTF-8").replace("+", "%20")
        resp.addHeader(
            "Content-Disposition",
            "attachment; filename=\"${name.replace("\"", "_")}\"; filename*=UTF-8''$encoded",
        )
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
        // No `download` attribute: saving is driven by the Content-Disposition
        // header, so a no-JS denial (an HTML page) renders instead of being
        // saved as a "rejected" file.
        val rows = files.list().joinToString("\n") { e ->
            """<li><a href="$FILE_PREFIX${e.id}"><span class="fname">${esc(e.name)}</span><span class="fsize">${human(e.size)}</span></a></li>"""
        }
        val listing = if (rows.isEmpty()) {
            "<p class=\"muted\">Nothing shared from the phone yet.</p>"
        } else {
            "<ul class=\"files\">\n$rows\n</ul>"
        }
        val note = banner?.let { "<p class=\"banner\">${esc(it)}</p>" } ?: ""
        // Styled to match the app's AMOLED theme (ui/theme/Theme.kt): black
        // ground, white text, emerald 34D399 accent with black on-accent,
        // 3F3F46 outlines, FF6B6B error — and the hotspot sheet's shapes
        // (uppercase section labels, pill buttons, a green tethering mark).
        val html = """
            <!doctype html>
            <html lang="en">
            <meta charset="utf-8">
            <meta name="viewport" content="width=device-width, initial-scale=1">
            <meta name="theme-color" content="#000000">
            <title>Myco file share</title>
            <style>
              :root { --bg:#000; --fg:#fff; --muted:#9ca3af; --accent:#34d399; --on-accent:#000;
                      --outline:#3f3f46; --error:#ff6b6b; --surface:#101012; }
              * { box-sizing: border-box; }
              body { font-family: system-ui, sans-serif; background: var(--bg); color: var(--fg);
                     margin: 0 auto; max-width: 30rem; padding: 1.4rem 1.2rem 3rem; }
              header { display: flex; align-items: center; gap: .6rem; margin-bottom: .3rem; }
              header svg { flex: none; }
              h1 { font-size: 1.35rem; font-weight: 800; margin: 0; }
              .hint { color: var(--muted); font-size: .85rem; margin: .3rem 0 1.2rem; }
              .label { color: var(--muted); font-weight: 700; font-size: .72rem; letter-spacing: .08em;
                       text-transform: uppercase; margin: 1.7rem 0 .5rem; }
              .muted { color: var(--muted); font-size: .9rem; }
              ul.files { list-style: none; margin: 0; padding: 0; }
              ul.files li + li { border-top: 1px solid var(--outline); }
              ul.files a { display: flex; justify-content: space-between; gap: 1rem; align-items: baseline;
                           padding: .8rem .2rem; text-decoration: none; color: var(--fg); }
              .fname { word-break: break-all; font-weight: 600; }
              .fsize { color: var(--muted); white-space: nowrap; font-size: .85rem; }
              .banner { background: #04241a; border: 1px solid #14532d; color: var(--accent);
                        padding: .6rem .9rem; border-radius: .7rem; font-size: .9rem; }
              .pick-list { list-style: none; margin: .4rem 0 0; padding: 0; }
              .pick-list li { display: flex; justify-content: space-between; gap: 1rem;
                              color: var(--muted); font-size: .85rem; padding: .25rem .2rem; }
              .actions { display: flex; gap: .8rem; align-items: center; margin-top: .9rem; }
              .btn { border-radius: 999px; padding: .65rem 1.4rem; font-size: .95rem; font-weight: 700;
                     border: 1px solid transparent; font-family: inherit; cursor: pointer; }
              .btn-primary { background: var(--accent); color: var(--on-accent); }
              .btn-primary:disabled { background: #1f2937; color: #6b7280; }
              .btn-outline { background: transparent; color: var(--accent); border-color: var(--outline); }
              .btn-text-danger { background: none; border: none; color: var(--error); font-weight: 700;
                                 font-family: inherit; font-size: .95rem; cursor: pointer; }
              #fileInput { display: none; }
              .overlay { position: fixed; inset: 0; background: rgba(0,0,0,.6); display: none;
                         align-items: center; justify-content: center; }
              .overlay.show { display: flex; }
              .dialog { background: var(--surface); color: var(--fg); border: 1px solid var(--outline);
                        border-radius: 1rem; padding: 1.2rem 1.2rem 1rem; max-width: 20rem; margin: 1rem;
                        box-shadow: 0 8px 30px rgba(0,0,0,.5); }
              .dialog h2 { margin: 0 0 .5rem; font-size: 1.05rem; }
              .dialog p { margin: 0; }
              .dialog .actions { justify-content: flex-end; margin-top: 1rem; }
            </style>
            <header>
              <svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="#34d399" stroke-width="2" stroke-linecap="round" aria-hidden="true">
                <circle cx="12" cy="14" r="2" fill="#34d399" stroke="none"/>
                <path d="M8.5 10.5a5 5 0 0 1 7 0"/>
                <path d="M5.7 7.7a9 9 0 0 1 12.6 0"/>
              </svg>
              <h1>Share files over hotspot</h1>
            </header>
            <p class="hint">Every transfer waits for an OK on the phone — give it a moment.</p>
            $note
            <div class="label">Download from the phone</div>
            $listing
            <div class="label">Send files to the phone</div>
            <input type="file" id="fileInput" multiple>
            <ul class="pick-list" id="pickList"></ul>
            <div class="actions">
              <label for="fileInput" class="btn btn-outline">+ Choose files</label>
              <button class="btn btn-primary" id="sendBtn" disabled>Send</button>
            </div>
            <noscript>
              <p class="muted">JavaScript is off — transfers with approval need it. Basic upload:</p>
              <form method="post" action="/upload" enctype="multipart/form-data">
                <input type="file" name="file" multiple>
                <button type="submit">Upload</button>
              </form>
            </noscript>
            <div class="overlay" id="statusOverlay">
              <div class="dialog">
                <p id="statusText"></p>
                <div class="actions" id="statusActions" style="display:none">
                  <button class="btn btn-outline" id="statusOk">OK</button>
                </div>
              </div>
            </div>
            <div class="overlay" id="offerOverlay">
              <div class="dialog">
                <h2>Incoming file</h2>
                <p id="offerText"></p>
                <div class="actions">
                  <button class="btn-text-danger" id="offerDecline">Decline</button>
                  <button class="btn btn-primary" id="offerAccept">Accept</button>
                </div>
              </div>
            </div>
            <script>
            function humanSize(b) {
              if (b >= 1048576) return (b / 1048576).toFixed(1) + ' MB';
              if (b >= 1024) return Math.round(b / 1024) + ' kB';
              return b > 0 ? b + ' B' : '';
            }

            // --- picking + sending (one raw PUT per file; see uploadPut) ---
            const fileInput = document.getElementById('fileInput');
            const pickList = document.getElementById('pickList');
            const sendBtn = document.getElementById('sendBtn');
            fileInput.addEventListener('change', () => {
              pickList.textContent = '';
              for (const f of fileInput.files) {
                const li = document.createElement('li');
                const name = document.createElement('span');
                name.textContent = f.name;
                const size = document.createElement('span');
                size.textContent = humanSize(f.size);
                li.append(name, size);
                pickList.append(li);
              }
              sendBtn.disabled = fileInput.files.length === 0;
            });
            sendBtn.addEventListener('click', async () => {
              const files = fileInput.files;
              if (!files.length) return;
              sendBtn.disabled = true;
              sendBtn.textContent = 'Waiting for the OK…';
              let sent = 0;
              for (const f of files) {
                try {
                  const r = await fetch('$UPLOAD_PREFIX' + encodeURIComponent(f.name), {method: 'PUT', body: f});
                  if (r.ok) sent++;
                } catch (_) {}
              }
              location.href = '/?sent=' + sent;
            });

            // --- status dialog (download approval / denial) ---
            const sOverlay = document.getElementById('statusOverlay');
            const sText = document.getElementById('statusText');
            const sActions = document.getElementById('statusActions');
            document.getElementById('statusOk').addEventListener('click', () => sOverlay.classList.remove('show'));
            function showStatus(text, dismissible) {
              sText.textContent = text;
              sActions.style.display = dismissible ? 'flex' : 'none';
              sOverlay.classList.add('show');
            }
            // Downloads ask the phone's owner first (POST …/request blocks on
            // their dialog). Denial becomes THIS dialog — never a saved file;
            // approval navigates with a one-time token and streams instantly.
            document.querySelectorAll('a[href^="$FILE_PREFIX"]').forEach(a => {
              a.addEventListener('click', async (e) => {
                e.preventDefault();
                const href = a.getAttribute('href');
                showStatus("Waiting for the OK on the other phone…", false);
                try {
                  const r = await fetch(href + '$REQUEST_SUFFIX', {method: 'POST'});
                  if (r.ok) {
                    const data = await r.json();
                    sOverlay.classList.remove('show');
                    window.location.href = href + '?t=' + data.token;
                  } else {
                    showStatus("The phone's owner declined this download.", true);
                  }
                } catch (err) {
                  showStatus("Connection lost — try again.", true);
                }
              });
            });

            // --- AirDrop-style receive: poll for files the phone is offering ---
            const overlay = document.getElementById('offerOverlay');
            const offerText = document.getElementById('offerText');
            let current = null;
            const handled = new Set();
            function settle(accepted) {
              if (!current) return;
              handled.add(current.id);
              if (accepted) {
                window.location.href = '/offer/' + current.id;
              } else {
                fetch('/offer/' + current.id + '/decline', {method: 'POST'});
              }
              overlay.classList.remove('show');
              current = null;
            }
            document.getElementById('offerAccept').addEventListener('click', () => settle(true));
            document.getElementById('offerDecline').addEventListener('click', () => settle(false));
            async function pollOffers() {
              try {
                const r = await fetch('/offers');
                const data = await r.json();
                const next = data.offers.find(o => !handled.has(o.id));
                if (next && !current) {
                  current = next;
                  const size = humanSize(next.size);
                  offerText.textContent = 'The phone wants to send you "' + next.name + '"' +
                    (size ? ' (' + size + ')' : '') + '.';
                  overlay.classList.add('show');
                }
              } catch (e) {}
              setTimeout(pollOffers, 2000);
            }
            pollOffers();
            </script>
            </html>
        """.trimIndent()
        val resp = newFixedLengthResponse(Response.Status.OK, MIME_HTML, html)
        resp.addHeader("Cache-Control", "no-store")
        return resp
    }

    // --- host -> guest offers ---

    private fun offersJson(): Response {
        val arr = JSONArray()
        outbox.waiting().forEach { o ->
            arr.put(JSONObject().put("id", o.id).put("name", o.name).put("size", o.size))
        }
        val resp = newFixedLengthResponse(
            Response.Status.OK, "application/json", JSONObject().put("offers", arr).toString(),
        )
        resp.addHeader("Cache-Control", "no-store")
        return resp
    }

    /** The guest tapped Accept: stream the offered document. No [TransferGate]
     *  here — the owner's consent *was* offering the file. */
    private fun sendOffer(idStr: String): Response {
        val id = idStr.toLongOrNull()
            ?: return newFixedLengthResponse(Response.Status.NOT_FOUND, MIME_PLAINTEXT, "gone\n")
        val (offer, stream) = outbox.accept(id)
            ?: return newFixedLengthResponse(Response.Status.NOT_FOUND, MIME_PLAINTEXT, "gone\n")
        Log.i(TAG, "offer '${offer.name}' accepted by the guest")
        val resp = if (offer.size > 0) {
            newFixedLengthResponse(Response.Status.OK, SharedFiles.mimeFor(offer.name), stream, offer.size)
        } else {
            newChunkedResponse(Response.Status.OK, SharedFiles.mimeFor(offer.name), stream)
        }
        attachAs(resp, offer.name)
        return resp
    }

    private fun declineOffer(idStr: String): Response {
        idStr.toLongOrNull()?.let {
            Log.i(TAG, "offer #$it declined by the guest")
            outbox.decline(it)
        }
        return newFixedLengthResponse(Response.Status.OK, MIME_PLAINTEXT, "ok\n")
    }

    private fun denied(): Response = newFixedLengthResponse(
        Response.Status.FORBIDDEN, MIME_PLAINTEXT,
        "The phone's owner did not approve this transfer.\n",
    )

    /** Denial for a plain navigation (no-JS): a page, never a saved file. */
    private fun deniedPage(): Response = newFixedLengthResponse(
        Response.Status.FORBIDDEN, MIME_HTML,
        """
        <!doctype html>
        <meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
        <title>Not approved</title>
        <p style="font-family: system-ui, sans-serif; margin: 2rem;">
          The phone's owner did not approve this transfer. <a href="/">Back</a>
        </p>
        """.trimIndent(),
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
        private const val OFFER_PREFIX = "/offer/"
        private const val REQUEST_SUFFIX = "/request"

        /** How long an approved-download token stays spendable. Generous — it
         *  only bridges the page's redirect after the owner already said yes. */
        private const val TOKEN_TTL_MS = 60_000L
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
