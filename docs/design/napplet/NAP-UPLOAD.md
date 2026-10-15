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
> `nap/upload/strip.rs`, `assets/shell.html`, `assets/myco-prelude.js`;
> `myco-core/src/blossom_upload.rs`.

## What it does

A napplet hands Myco a file. Myco strips its metadata, keeps it in this
device's own Blossom store, then puts it on the user's Blossom servers,
signed as the user, and answers with the URL and NIP-94 tags. Doodle Duo uses
it to post its score picture in a note.

An upload always works at least on this device. When no server takes it, the
answer still says `complete`, with a local URL (see Results).

API, schemas, wire and error strings: see the spec.

## Rails

- **Blossom only.** `info` lists one rail, `blossom`, always enabled: this
  device's own Blossom takes every upload.
- `returns` is `["https", "blossom"]`, or just `["blossom"]` while
  offline-only is on.
- A request naming any other rail (`nip96` included) gets `unsupported rail`.
  No rail means Blossom.
- `info` is advisory; nothing requires calling it first.

## This device first

Every upload is kept in this device's Blossom before any server is tried
(`NapContext::kept_blobs`, the store `resource.keep` writes to). Kept means
not cache: nothing evicts it.

- It is the stripped file (see Metadata), so the hash is the same here and on
  the servers.
- Circle peers can fetch it by hash over the mesh, from `:24243`, the
  paired-only Blossom server. `mesh.blobs` on their side counts this phone
  among those that hold it. That server answers from the kept store and the
  cache together.
- With a custom blob store set in Settings, "this device's Blossom" is that
  store, and the phone runs no mesh Blossom server. Peers then cannot fetch
  it from this phone.
- If keeping fails (a full disk), the servers are still tried. Only when
  both fail is the upload `failed`.

## Metadata

Stripped by the shell, in Rust, before anything is kept or sent
(`nap/upload/strip.rs`). Segments and chunks are dropped; nothing is decoded
or re-encoded. The spec allows it ("The shell MAY strip or rewrite metadata").

- **JPEG:** every `APPn` but `APP0` JFIF, `APP2` ICC profile and `APP14`
  Adobe (needed to decode CMYK), and every `COM`. Also anything after the
  end-of-image marker (a "motion photo" hides a video there).
- **PNG:** every ancillary chunk but `iCCP`, `gAMA`, `cHRM`, `sRGB`, `pHYs`,
  `tRNS`, `sBIT`, `bKGD`, `cICP`, `mDCV`, `cLLI` and the APNG chunks. So
  `tEXt`, `zTXt`, `iTXt`, `tIME` and unknown chunks go. Anything after
  `IEND` goes too.
- **WebP:** every chunk but `VP8X`, `VP8 `, `VP8L`, `ALPH`, `ANIM`, `ANMF`,
  `ICCP`. So `XMP ` goes; the `VP8X` flags are made to match and the RIFF
  size rewritten.
- **GLB** (`model/gltf-binary`): `asset.generator`, every `extras`, and the
  `KHR_xmp_json_ld` extension, anywhere in the JSON. `asset.version` and
  `asset.copyright` stay. Pictures embedded in the binary chunk are stripped
  as above, and the chunk is laid out again with each buffer view's offset
  and length rewritten (alignment kept).
  - Not for meshopt-compressed files, or ones whose buffer views overlap:
    their embedded pictures go as they are. A napplet that cares strips its
    textures itself (lightertrek re-encodes them anyway).
  - Pictures in `data:` URIs or separate files are not touched.

### Orientation is kept

A phone stores a portrait photo's pixels landscape and says "rotate 90°" in
the EXIF Orientation tag. Dropping it would show the photo on its side.

- The first EXIF block (JPEG `APP1` Exif, PNG `eXIf`, WebP `EXIF`) is read,
  both byte orders, every read bounds-checked.
- If IFD0 has an Orientation (tag `0x0112`, SHORT) of 2–8, a new EXIF block
  replaces it: a TIFF header in the original byte order, IFD0 with that one
  entry, no next IFD. 26 bytes.
- Nothing else of the original survives: no GPS, camera, timestamps,
  thumbnail or other IFDs.
- Orientation 1, no Orientation, or EXIF that does not parse: no EXIF at all.

Where it goes:

- **JPEG:** a new `APP1` right after start-of-image, or after `APP0` JFIF
  when that comes first.
- **PNG:** in place of the old `eXIf`, with a new CRC.
- **WebP:** in place of the old `EXIF`, with the `VP8X` EXIF flag set. A
  file without `VP8X` cannot carry EXIF, so it gets none.

The format is read from the bytes' magic number, and from the type only when
the bytes have none. A file of one of these types that does not parse is
refused with a top-level `malformed file` (a Myco string, not the spec's),
and nothing is kept or sent. Every other type goes as it is.

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
- `sha256` is always the stripped file's. When stripping changed the file,
  `originalSha256` is the hash of what the napplet sent, as the spec asks
  ("SHOULD reflect this in the returned hashes"). It is not put in `nip94`
  as `ox`: that tag would publish the hash of the unstripped file.
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

- **Offline-only on, no internet** (the breaker is tripped), **no server
  configured, or every server refused:** the upload is still `ok: true`,
  `complete`, from this device (see Results). Nothing is signed while
  offline-only is on.
- `policy denied` as a top-level error is only for a napplet without the
  `upload` grant.
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

### When no server took it

Myco additions, marked as such since the spec has no local result:

```json
{
  "ok": true,
  "uploadId": "…",
  "status": "complete",
  "rail": "blossom",
  "url": "blossom:sha256:<hex>",
  "sha256": "<hex>",
  "size": 1234,
  "mimeType": "image/png",
  "nip94": [["url", "blossom:sha256:<hex>"], ["m", "image/png"], ["x", "<hex>"], ["size", "1234"]],
  "localOnly": true,
  "remoteError": "policy denied"
}
```

- `status` stays `complete`: the spec's states are a closed list, and the
  file is stored. `localOnly: true` is the one field to check.
- `url` is `blossom:sha256:<hex>`, which NAP-RESOURCE resolves (this device
  first, then Circle peers). It is not a link anyone on the web can open.
- `remoteError` is why no server has it, in the spec's strings:
  `policy denied` (offline-only on, or not signed in), `upload failed` (no
  internet, out of time, another upload running), `no server configured`,
  `server rejected`, `quota exceeded`.
- No `fallbackUrls`. `originalSha256` and `dimensions` as on success.
- A signer's "no" is still `cancelled`, `ok: false`, not a local success. The
  copy kept on this device stays.
- Nothing retries the servers later. A napplet that wants the file public can
  upload again when `info` lists `https`.

## Not built

- NIP-96, and any rail but Blossom.
- Progress (`uploading`, `upload.status.changed`).
- Blurhash. Dimensions beyond PNG and GIF headers.
- Metadata stripping beyond JPEG, PNG, WebP and GLB (GIF comments, video,
  PDF, audio tags), and GLB pictures in meshopt files (see Metadata).
- Showing the user that stripping happened, or a setting to turn it off.
- Retrying the servers for an upload kept only on this device.
- A consent prompt showing type, size, target server and napplet, or a
  preview (spec SHOULD). Consent is the install grant.
- Rate limiting per napplet (spec SHOULD). The size cap bounds one upload
  and only one runs at a time; nothing bounds how many in a row.
- `noTransform` and `metadata` are ignored. BUD-02 `PUT /upload` does not
  transform anyway.
- Cancelling an upload in flight.

## Implementations

- Runtime: `nap/upload.rs`, `nap/upload/strip.rs`, the `UploadSink` seam (`UploadError` carries the
  spec's error strings), `NapContext::uploads`, `Session::record_upload`.
- Shell: `assets/shell.html` (bytes to base64), `assets/myco-prelude.js`
  (`upload.upload` with the signing wait).
- Core: `BlossomUploader` in `myco-core/src/blossom_upload.rs`, wired in
  `AppRuntime::napplet_context`; `GRANTED_ONLY_WHEN_SHOWN` in `napplet.rs`.
- Android: the "Upload files" wording in `AppsScreen.kt`.
