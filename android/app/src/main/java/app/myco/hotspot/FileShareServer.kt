package app.myco.hotspot

import android.util.Log
import app.myco.share.MycoApk
import fi.iki.elonen.NanoHTTPD
import java.io.ByteArrayInputStream
import java.io.FileInputStream
import java.io.FilterInputStream
import java.io.InputStream
import java.net.URLDecoder
import java.net.URLEncoder
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import org.json.JSONArray
import org.json.JSONObject

/**
 * The web page a hotspot guest lands on: a plain-HTML file list with download
 * links and an upload form. Served on every interface (the guest reaches it
 * via the hotspot's gateway address); state lives in [SharedFiles].
 *
 * Framework-free HTML styled after getmyco.app, with one inline script. Uploads go as one raw `PUT /upload/<urlencoded-name>` per picked
 * file, because NanoHTTPD 2.3.1's multipart parser surfaces only the first of
 * several same-named file parts and mangles non-ASCII filenames (it decodes
 * part headers as ASCII) — a multipart POST form stays as the no-JS fallback.
 * The script also polls `/offers` for files the phone is pushing ([Outbox])
 * and pops an accept/decline dialog per offer, AirDrop-style.
 *
 * On top of the page sits **Get Myco**: Myco's own APK ([apk], null for a
 * split install), so a guest without Myco installs it from here with no
 * internet. It is served without asking the owner — it is the app itself,
 * nothing of theirs. That makes it the one large download here with no
 * approval in front of it, so it answers `Range` (a dropped 100 MB transfer
 * resumes) and `HEAD`, and serves at most [MAX_APK_STREAMS] guests at once.
 * The mesh file transfer refuses APKs on purpose (`myco-core/src/
 * file_transfer.rs`); this route is not a way around that: it serves only
 * Myco's own installed APK, never a file someone picked or sent.
 */
class FileShareServer(
    private val files: SharedFiles,
    private val outbox: Outbox,
    private val apk: MycoApk.Info?,
    private val icon: ByteArray?,
    port: Int,
) : NanoHTTPD(port) {

    /** Approved-download tokens: token → (file id, expiry). Single-use. */
    private val downloadTokens = ConcurrentHashMap<String, Pair<String, Long>>()

    /** APK downloads streaming right now (see [MAX_APK_STREAMS]). */
    private val apkStreams = AtomicInteger(0)

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
            (session.method == Method.GET || session.method == Method.HEAD) && session.uri == APK_PATH ->
                sendApk(session)
            session.method == Method.GET && session.uri == ICON_PATH && icon != null ->
                newFixedLengthResponse(Response.Status.OK, "image/png", ByteArrayInputStream(icon), icon.size.toLong())
                    .apply { addHeader("Cache-Control", "max-age=86400") }
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

    /** Myco's own APK, streamed from where it is installed; resumable. */
    private fun sendApk(session: IHTTPSession): Response {
        val apk = apk ?: return newFixedLengthResponse(Response.Status.NOT_FOUND, MIME_PLAINTEXT, "gone\n")
        val range = parseRange(session.headers["range"], apk.size)
        val (start, length) = range?.let { it.first to it.last - it.first + 1 } ?: (0L to apk.size)
        val status = if (range != null) Response.Status.PARTIAL_CONTENT else Response.Status.OK
        val resp = if (session.method == Method.HEAD) {
            // NanoHTTPD sends no body for HEAD; the length still goes out.
            newFixedLengthResponse(status, MycoApk.MIME, ByteArrayInputStream(ByteArray(0)), length)
        } else {
            if (apkStreams.incrementAndGet() > MAX_APK_STREAMS) {
                apkStreams.decrementAndGet()
                return newFixedLengthResponse(
                    Response.Status.SERVICE_UNAVAILABLE, MIME_PLAINTEXT,
                    "The phone is busy sending Myco to others. Try again in a minute.\n",
                ).apply { addHeader("Retry-After", "30") }
            }
            Log.i(TAG, "guest is downloading ${apk.name} from byte $start")
            val stream = CountedStream(FileInputStream(apk.file).apply { skipFully(start) }) {
                apkStreams.decrementAndGet()
            }
            newFixedLengthResponse(status, MycoApk.MIME, stream, length)
        }
        resp.addHeader("Accept-Ranges", "bytes")
        if (range != null) resp.addHeader("Content-Range", "bytes ${range.first}-${range.last}/${apk.size}")
        attachAs(resp, apk.name)
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
        // No `download` attribute: saving is driven by the Content-Disposition
        // header, so a no-JS denial (an HTML page) renders instead of being
        // saved as a "rejected" file.
        val rows = files.list().joinToString("\n") { e ->
            """<li><a href="$FILE_PREFIX${e.id}"><span class="fname">${esc(e.name)}</span><span class="fsize">${human(e.size)}</span></a></li>"""
        }
        val listing = if (rows.isEmpty()) {
            "<p class=\"empty\">Nothing shared from the phone yet.</p>"
        } else {
            "<ul class=\"files\">\n$rows\n</ul>"
        }
        val note = banner?.let { "<p class=\"banner\">${esc(it)}</p>" } ?: ""
        val iconTag = if (icon != null) "<img src=\"$ICON_PATH\" width=\"28\" height=\"28\" alt=\"\">" else ""
        // Get Myco leads: someone on this page without Myco most likely came
        // for it. Plain link, no approval round-trip: see the class doc.
        val hero = apk?.let { a ->
            val needs = if (a.only64Bit) "Android 10 or newer, 64-bit" else "Android 10 or newer"
            // Only a release-signed APK has these bytes on the release page,
            // and only that one takes later updates from Zapstore or GitHub.
            val trust = if (a.release) {
                MycoApk.cachedSha256(a)?.let {
                    """
                    <details class="howto">
                      <summary>Check it's genuine</summary>
                      <p>This is the official release, signed by Myco. Its SHA-256 matches the
                        checksum on the release page (github.com/Origami74/myco/releases):</p>
                      <p class="sha">${esc(it)}</p>
                    </details>
                    """.trimIndent()
                } ?: ""
            } else {
                """<p class="devnote"><strong>Development build.</strong> Fine for trying Myco, but it won't
                  update from Zapstore or GitHub. Uninstall it before installing an official release.</p>"""
            }
            """
            <section class="hero">
              <p class="eyebrow"><span class="eyebrow-dot"></span>Shared from a phone nearby</p>
              <h1 class="wordmark">Myco</h1>
              <p class="tagline">Your portal to the mesh.</p>
              <p class="blurb">Get apps from the people around you. Tap your phones together and
                the app is yours too — and it keeps working when there's no internet.</p>
              <a class="btn-direct" id="getMyco" href="$APK_PATH">
                <svg class="btn-icon" viewBox="0 0 24 24" aria-hidden="true"><path fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" d="M12 3v12m0 0-5-5m5 5 5-5M5 21h14"/></svg>
                <span class="btn-text"><small>No internet needed</small>Download Myco</span>
              </a>
              <p class="get-meta">Myco ${esc(a.version)} · ${human(a.size)} · $needs</p>
              <p class="started" id="started" hidden>Downloading… When it finishes, tap it in your
                notifications (or find it in <em>Files › Downloads</em>).</p>
              <details class="howto" open>
                <summary>After it downloads</summary>
                <ol>
                  <li>Tap the download when it finishes (or find it in <em>Files › Downloads</em>).</li>
                  <li>If your phone asks, allow your browser to install apps. It's a one-time switch.</li>
                  <li>Tap <em>Install</em>, then <em>Open</em>.</li>
                </ol>
              </details>
              $trust
            </section>
            """.trimIndent()
        } ?: """
            <section class="hero">
              <p class="eyebrow"><span class="eyebrow-dot"></span>Shared from a phone nearby</p>
              <h1 class="wordmark small">Files</h1>
            </section>
        """.trimIndent()
        // Styled after getmyco.app (myco/getmyco-site, src/style.css): the
        // slate "soil" ground with its two glows, light wordmark, the
        // cyan→emerald tagline, the teal direct-download button, the
        // "+ / –" details and the uppercase thread-coloured kickers.
        val html = """
            <!doctype html>
            <html lang="en">
            <meta charset="utf-8">
            <meta name="viewport" content="width=device-width, initial-scale=1">
            <meta name="theme-color" content="#060a12">
            <title>Myco — shared from a phone nearby</title>
            <style>
              :root { --soil:#060a12; --thread:#7dd3c8; --fg:#f1f5f9; --dim:#8fa3b8; --teal:#0d9488;
                      --emerald:#34d399; --bright:#a5f3fc; --line:rgba(226,232,240,.12);
                      --card:rgba(148,163,184,.06); --error:#ff6b6b;
                      --mono: ui-monospace, "SF Mono", Menlo, Consolas, monospace; }
              *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
              body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
                     color: var(--fg); line-height: 1.5; -webkit-font-smoothing: antialiased; min-height: 100vh;
                     background: radial-gradient(120% 90% at 76% 20%, #10202a 0%, rgba(16,32,42,0) 60%),
                                 radial-gradient(90% 70% at 10% 90%, #0b1a1c 0%, rgba(11,26,28,0) 62%), var(--soil);
                     background-attachment: fixed; }
              main { max-width: 32rem; margin: 0 auto; padding: 1rem 1.25rem 4rem; }
              .brand { display: flex; align-items: center; gap: .55rem; font-weight: 600;
                       letter-spacing: -.01em; padding: .4rem 0 0; }
              .brand img { border-radius: 8px; }
              .hero { padding: 2.6rem 0 1rem; }
              .eyebrow { display: flex; align-items: center; gap: .6rem; font-size: .72rem; letter-spacing: .2em;
                         text-transform: uppercase; color: var(--dim); margin-bottom: 1.1rem; }
              .eyebrow-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--thread);
                             box-shadow: 0 0 10px var(--thread); }
              .wordmark { font-size: clamp(4rem, 22vw, 6rem); line-height: .88; font-weight: 300;
                          letter-spacing: -.055em; margin-bottom: .9rem; }
              .wordmark.small { font-size: clamp(3rem, 16vw, 4.5rem); }
              .tagline { font-size: 1.4rem; letter-spacing: -.02em; margin-bottom: .8rem;
                         background: linear-gradient(90deg, var(--bright), var(--thread) 60%, var(--emerald));
                         -webkit-background-clip: text; background-clip: text; color: transparent; }
              .blurb { color: var(--dim); font-size: 1rem; line-height: 1.65; margin-bottom: 1.6rem; max-width: 38ch; }
              .btn-direct { display: inline-flex; align-items: center; gap: .75rem; min-height: 3.6rem;
                            padding: .55rem 1.35rem .55rem 1.1rem; border-radius: 14px; text-decoration: none;
                            background: var(--teal); color: #fff; transition: transform .18s ease, background .2s; }
              .btn-direct:active { transform: translateY(1px); background: #0f766e; }
              .btn-icon { width: 1.6rem; height: 1.6rem; }
              .btn-text { display: flex; flex-direction: column; font-size: 1.2rem; font-weight: 600;
                          line-height: 1.1; letter-spacing: -.01em; }
              .btn-text small { font-size: .68rem; font-weight: 500; letter-spacing: .04em; opacity: .8;
                                margin-bottom: .15rem; }
              .get-meta { margin-top: .85rem; font-size: .8rem; color: var(--dim); font-family: var(--mono); }
              .started { margin-top: .8rem; color: var(--emerald); font-size: .9rem; }
              .howto { margin-top: 1rem; font-size: .9rem; color: var(--dim); }
              .howto summary { cursor: pointer; width: fit-content; color: var(--thread); list-style: none; }
              .howto summary::-webkit-details-marker { display: none; }
              .howto summary::before { content: "+ "; }
              .howto[open] summary::before { content: "– "; }
              .howto[open] summary { margin-bottom: .6rem; }
              .howto p, .howto ol { margin-bottom: .55rem; line-height: 1.55; }
              .howto ol { padding-left: 1.2rem; }
              .howto li { margin-bottom: .25rem; }
              em { color: var(--fg); font-style: normal; font-weight: 600; }
              .sha { font-family: var(--mono); font-size: .72rem; word-break: break-all; color: var(--fg); }
              .devnote { margin-top: 1rem; padding: .7rem .95rem; border: 1px dashed var(--line);
                         border-radius: 12px; font-size: .85rem; color: var(--dim); }
              .devnote strong { color: var(--fg); }
              .chapter { margin-top: 2.6rem; padding-top: 2.2rem; border-top: 1px solid var(--line); }
              .kicker { font-size: .72rem; letter-spacing: .2em; text-transform: uppercase; color: var(--thread);
                        margin-bottom: .7rem; }
              h2 { font-size: 1.7rem; line-height: 1.1; font-weight: 500; letter-spacing: -.035em; margin-bottom: .5rem; }
              .aside { color: var(--dim); font-size: .92rem; margin-bottom: 1.2rem; }
              .card { padding: 1.1rem 1.2rem; border-radius: 20px; background: var(--card);
                      border: 1px solid var(--line); margin-top: .9rem; }
              .card h3 { font-size: 1rem; font-weight: 600; margin-bottom: .4rem; }
              .empty { color: var(--dim); font-size: .9rem; }
              ul.files { list-style: none; }
              ul.files li + li { border-top: 1px solid var(--line); }
              ul.files a { display: flex; justify-content: space-between; gap: 1rem; align-items: baseline;
                           padding: .7rem 0; text-decoration: none; color: var(--fg); }
              .fname { word-break: break-all; font-weight: 600; }
              .fsize { color: var(--dim); white-space: nowrap; font-size: .8rem; font-family: var(--mono); }
              .banner { margin-top: 1rem; background: rgba(52,211,153,.08); border: 1px solid rgba(52,211,153,.3);
                        color: var(--emerald); padding: .6rem .9rem; border-radius: 12px; font-size: .9rem; }
              .pick-list { list-style: none; margin: .2rem 0 0; }
              .pick-list li { display: flex; justify-content: space-between; gap: 1rem; color: var(--dim);
                              font-size: .85rem; padding: .25rem 0; }
              .actions { display: flex; gap: .7rem; align-items: center; margin-top: .9rem; flex-wrap: wrap; }
              .btn { border-radius: 999px; padding: .6rem 1.3rem; font-size: .92rem; font-weight: 600;
                     border: 1px solid transparent; font-family: inherit; cursor: pointer; }
              .btn-primary { background: var(--fg); color: var(--soil); }
              .btn-primary:disabled { background: rgba(148,163,184,.18); color: var(--dim); }
              .btn-outline { background: transparent; color: var(--fg); border-color: rgba(226,232,240,.4); }
              .btn-text-danger { background: none; border: none; color: var(--error); font-weight: 600;
                                 font-family: inherit; font-size: .92rem; cursor: pointer; }
              #fileInput { display: none; }
              .overlay { position: fixed; inset: 0; background: rgba(0,0,0,.65); display: none;
                         align-items: center; justify-content: center; }
              .overlay.show { display: flex; }
              .dialog { background: #0b141c; color: var(--fg); border: 1px solid var(--line);
                        border-radius: 20px; padding: 1.2rem 1.2rem 1rem; max-width: 20rem; margin: 1rem;
                        box-shadow: 0 30px 80px rgba(0,0,0,.6); }
              .dialog h2 { font-size: 1.1rem; letter-spacing: -.01em; }
              .dialog .actions { justify-content: flex-end; margin-top: 1rem; }
              .footer { margin-top: 3rem; color: var(--dim); font-size: .78rem; text-align: center; }
            </style>
            <main>
            <div class="brand">$iconTag<span>Myco</span></div>
            $hero
            <section class="chapter">
              <p class="kicker">Files</p>
              <h2>Swap files with this phone</h2>
              <p class="aside">Files wait for an OK on the phone — give it a moment.</p>
              $note
              <div class="card">
                <h3>From the phone</h3>
                $listing
              </div>
              <div class="card">
                <h3>Send to the phone</h3>
                <input type="file" id="fileInput" multiple>
                <ul class="pick-list" id="pickList"></ul>
                <div class="actions">
                  <label for="fileInput" class="btn btn-outline">+ Choose files</label>
                  <button class="btn btn-primary" id="sendBtn" disabled>Send</button>
                </div>
                <noscript>
                  <p class="empty">JavaScript is off — transfers with approval need it. Basic upload:</p>
                  <form method="post" action="/upload" enctype="multipart/form-data">
                    <input type="file" name="file" multiple>
                    <button type="submit">Upload</button>
                  </form>
                </noscript>
              </div>
            </section>
            <p class="footer">Served by Myco from the phone you're connected to. Nothing here goes over the internet.</p>
            </main>
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

            // --- Get Myco: the browser saves the APK; say where it went ---
            const getMyco = document.getElementById('getMyco');
            if (getMyco) getMyco.addEventListener('click', () => {
              document.getElementById('started').hidden = false;
            });

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
        private const val APK_PATH = "/myco.apk"
        private const val ICON_PATH = "/myco-icon.png"

        /** Parallel APK downloads; past this a guest is told to retry. Each
         *  holds a server thread for the whole ~100 MB transfer. */
        private const val MAX_APK_STREAMS = 4

        /**
         * The byte range one `Range: bytes=…` header asks for, clamped to
         * [size]; null means "send it all" (no header, several ranges, or one
         * that doesn't parse or fit — a full 200 is always a valid answer).
         */
        internal fun parseRange(header: String?, size: Long): LongRange? {
            val spec = header?.trim()?.removePrefix("bytes=") ?: return null
            if (spec == header.trim() || ',' in spec || size <= 0) return null
            val dash = spec.indexOf('-').takeIf { it >= 0 } ?: return null
            val from = spec.substring(0, dash).trim()
            val to = spec.substring(dash + 1).trim()
            val range = when {
                from.isEmpty() -> {
                    val tail = to.toLongOrNull()?.takeIf { it > 0 } ?: return null
                    maxOf(0L, size - tail)..(size - 1)
                }
                else -> {
                    val start = from.toLongOrNull() ?: return null
                    val end = if (to.isEmpty()) size - 1 else (to.toLongOrNull() ?: return null).coerceAtMost(size - 1)
                    start..end
                }
            }
            return range.takeIf { it.first in 0 until size && it.first <= it.last }
        }

        /** How long an approved-download token stays spendable. Generous — it
         *  only bridges the page's redirect after the owner already said yes. */
        private const val TOKEN_TTL_MS = 60_000L
    }
}

/** Skip exactly [n] bytes (a [FileInputStream] may skip fewer per call). */
private fun InputStream.skipFully(n: Long) {
    var left = n
    while (left > 0) {
        val skipped = skip(left)
        if (skipped <= 0) break
        left -= skipped
    }
}

/** Runs [onClose] once, when the server closes the stream after sending it. */
private class CountedStream(src: InputStream, private val onClose: () -> Unit) : FilterInputStream(src) {
    private val closed = AtomicBoolean(false)

    override fun close() {
        try {
            super.close()
        } finally {
            if (closed.compareAndSet(false, true)) onClose()
        }
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
