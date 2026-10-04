NAP-UPLOAD in Myco
==================

Upload a File to the User's Blossom Servers
-------------------------------------------

`draft` · **implementation notes**

**NAP ID:** NAP-UPLOAD
**Domain:** `upload`
**Spec:** [napplet/naps PR #33](https://github.com/napplet/naps/pull/33)
(`naps/NAP-UPLOAD.md` on the `nap-upload` branch)
**Web binding (NIP-5D):** `window.napplet.upload` · `shell.supports("upload")`

> How Myco implements the upstream NAP-UPLOAD. The spec is authoritative; this
> page records Myco's choices where the spec leaves room, and what is not
> built. The spec is a **draft in an open PR, not merged**: re-audit this page
> and the code whenever it changes.
> Implementation: `myco-napplet-runtime/src/nap/upload.rs`,
> `assets/shell.html`, `assets/myco-prelude.js`;
> `myco-core/src/blossom_upload.rs`.

## What it does

A napplet hands Myco a file. Myco puts it on the user's Blossom servers,
signed as the user, and answers with the URL and NIP-94 tags. Doodle Duo uses
it to post its score picture in a note.

API, schemas, wire and error strings: see the spec.

## Rails

- **Blossom only.** `info` lists one rail, `blossom`, returning `https` URLs.
- A request naming any other rail (`nip96` included) gets `unsupported rail`.
  No rail means Blossom.
- `info` is advisory. It says Blossom is disabled while offline-only is on;
  nothing requires calling it first.

## Servers

The napplet never names a server. In order:

1. The user's BUD-03 list (kind 10063, `server` tags), signed by the napplet
   user's key. This device's relay first; if it isn't there and the internet
   is up, the user's own relays (their NIP-65 write relays), for up to 8 s.
2. With no list: `https://blossom.primal.net`, then `https://cdn.hzrd149.com`.
   Both take uploads from any key. The other read defaults
   (`ip_source::default_blossom_servers`) are replicas that don't.

The defaults are used only when the lookup finished and there is no list.
If the user's relays don't answer in time, it's `upload failed`. If the list
names no server Myco can use, it's `no server configured`. Either way nothing
goes to servers the user never picked.

At most five servers, tried one at a time. Only `https://` servers whose
names resolve to public addresses. Redirects are never followed: a 3xx
counts as that server refusing, and the next one is tried. Otherwise a
redirect could send the signed upload to an address that was never checked,
such as this phone's own relay or Blossom on loopback.

The first server that takes it gives `url`. One more is then tried as a
mirror for up to 15 s; if it stored the same bytes, it is `fallbackUrls`.

## Authorization

A BUD-02 kind 24242 event: `t` `upload`, `x` the sha256 Myco computed,
`expiration` five minutes out, content `Upload a file`. The napplet's
filename is not put in it: a signer app shows that text on its approval
screen, and it isn't the napplet's to write. Signed through the napplet `Signer`: the user's
key, guest or a NIP-55 signer app. Sent as `Authorization: Nostr <base64>` on
`PUT <server>/upload`. One event covers every server tried.

## What is reported

- Success is only ever a 2xx with a descriptor naming the hash that was
  sent. BUD-02's `PUT /upload` stores bytes as given, so a descriptor naming
  another hash is that server failing, and the next one is tried. The runtime
  can still report a transform (`originalSha256`, `ox`) from an `UploadSink`
  that makes one; the Blossom uploader never does.
- `url` is the descriptor's only if it is `https` (or `http` on a `.fips`
  mesh name) and names the hash. Anything else gets `<server>/<sha256>`, so a
  server can't point the user's post at another file or host.
- `nip94`: `url`, `m`, `x`, `size`, `dim` (PNG and GIF
  headers only), `fallback`, and `alt` from the caption.
- `mimeType` is the server's `type` if it gave one, else what was sent.

## The byte hop

The napplet sends a `Blob` or `ArrayBuffer`, as the spec says. It never
base64s anything.

Inside the shell, the shell page hands napplet messages to Rust as JSON, and
a `Blob` would arrive as `{}`. So for `upload.upload` only, the shell page
(`assets/shell.html`, trusted) reads `request.data` and forwards it as
`request.dataBase64`, with the blob's type as `request.dataType`. It drops
any such field the napplet set first. Over the cap it reads nothing and sends
`request.dataSize`, and Rust refuses. This hop is internal and not visible to
the napplet.

Rust treats the decoded bytes as untrusted: cap checked again, hash computed
itself, type sniffed when none was given.

## Policy

Per napplet, the spec's three policies are:

- **Rails:** Blossom, for every napplet granted `upload`.
- **Size:** 16 MiB (`nap::upload::MAX_BYTES`, pinned to the shell page by a
  test). Over it: `file too large`. The bytes cross to Rust as base64 and are
  held several times over on the way (JS string, Java string, Rust string,
  JSON value, decoded bytes), so one upload can cost around 100 MB for a
  moment. Hence **one upload at a time** per phone: a second, while one
  runs, is `upload failed` at once rather than queued.
- **MIME types:** any. The napplet's `mimeType` if it is a valid
  `type/subtype`, else the blob's type, else sniffed from the bytes
  (`application/octet-stream` when nothing matches). No `mimeTypes` in `info`.

## Consent

- `upload` is implemented but **not a default grant**. A napplet gets it only
  if its manifest declares it in `requires` and the user allows it on the
  install sheet: "Upload files — Put files on your Blossom servers, signed as
  you, where anyone with the link can see them."
- That install-time grant is the consent the spec asks for before the first
  upload, and the "per-napplet allowance" after it. There is no prompt per
  upload. Reason: the grant is shown, in plain words, before the napplet can
  do anything, and can be switched off per app; a prompt per upload would
  ask about every picture a game posts.
- A signer app may still ask the user to approve the 24242 event. Saying no
  there ends the upload as `cancelled`, `user cancelled`. That relies on the
  signer answering NIP-55's `rejected`. One that reports the "no" as a failed
  result instead reads as a failure, and the upload ends as `failed`.
  Myco's signer code and the uploader share one string for it
  (`external_signer::REJECTED` / `ExternalSigner.REJECTED`).
- A napplet installed before this build that declared `upload` was never
  shown it: the sheet lists only what the build implements. Its stored
  reviewed list does not count for `upload` (`GRANTED_ONLY_WHEN_SHOWN` in
  `myco-core/src/napplet.rs`); the update review asks.
- Not granted: `policy denied`.

## Offline

- **Offline-only on:** `policy denied`, before anything is signed.
- **No internet** (the breaker is tripped): `upload failed`.
- **Bounds:** server list 8 s, uploads 120 s together, mirror 15 s, per server
  5 s to connect and 20 s without progress, plus the signer's own wait (120 s
  for a signer app). The sum stays under the prelude's 5 min wait, checked by
  a test, so the napplet hears why rather than "timed out".

## Results, status and progress

- `upload.upload` is answered once the upload is finished: `complete`,
  `failed` or `cancelled`. The spec allows a synchronous `complete`.
- No progress is streamed: `upload.status.changed` is never sent, and there
  are no `bytesSent` / `bytesTotal`.
- The vendored shim waits 30 s for `upload.upload`. Myco's prelude
  supplement replaces it with one that waits 5 min (`SIGNING_TIMEOUT`, as for
  `outbox.publish`): signing may be a person approving in a signer app.
- `upload.status` answers from the session's last 32 uploads; older or
  unknown ids get a top-level `error`.
- After a successful upload the file is also kept in this device's own
  Blossom, so paired phones can fetch it by hash. Best effort.

## Not built

- NIP-96, and any rail but Blossom.
- Progress (`uploading`, `upload.status.changed`).
- Blurhash. Dimensions beyond PNG and GIF headers.
- EXIF or other metadata stripping. Files go up as the napplet sent them.
- A consent prompt showing type, size, target server and napplet, or a
  preview (spec SHOULD). Consent is the install grant.
- Rate limiting per napplet (spec SHOULD). The size cap bounds one upload
  and only one runs at a time; nothing bounds how many in a row.
- `noTransform` and `metadata` are ignored. BUD-02 `PUT /upload` does not
  transform anyway.
- Cancelling an upload in flight.

## Implementations

- Runtime: `nap/upload.rs`, the `UploadSink` seam (`UploadError` carries the
  spec's error strings), `NapContext::uploads`, `Session::record_upload`.
- Shell: `assets/shell.html` (bytes to base64), `assets/myco-prelude.js`
  (`upload.upload` with the signing wait).
- Core: `BlossomUploader` in `myco-core/src/blossom_upload.rs`, wired in
  `AppRuntime::napplet_context`; `GRANTED_ONLY_WHEN_SHOWN` in `napplet.rs`.
- Android: the "Upload files" wording in `AppsScreen.kt`.
