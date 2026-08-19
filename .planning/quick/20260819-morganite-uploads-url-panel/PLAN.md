---
status: complete
date: 2026-08-19
repo: reference/Morganite (separate git repo, gitignored by fips-pop)
branch: feat/blossom-uploads-and-url-panel
commit: 8a7bd784660400076cac310e538ab9f9fbdbb8f4
---

# Morganite: Blossom uploads + reachable-URL panel

Work landed in `reference/Morganite`, not in fips-pop. `reference/` is gitignored
here, so GSD worktree isolation could not see the target files — this record exists
for traceability; the code and its history live in Morganite's own repo.

## Why

Myco's `RemoteBlobStore` (`myco-core/src/remote_blobs.rs`) does `PUT /upload`, and
Morganite had no write path at all — it was a pull-through cache in front of public
Blossom servers, so pointing Myco at `:24242` could only ever 404.

## Decisions

- **Upload auth is by origin, not by Nostr event.** Loopback peers may write; anything
  else needs the new "Allow uploads from the network" setting. Chosen over BUD-02
  signature verification because the requirement was "same device by default, toggle
  to open it up". Myco's signed `Authorization: Nostr` header is accepted but not
  required, so its client works unchanged either way.
- **Endpoint scope:** `PUT`/`HEAD /upload` and `PUT /mirror`. No `GET /list` or
  `DELETE` — the store is hash-keyed with no owner index, so those need new metadata.
- **`BlobDescriptor` serialises itself.** The project applies no kotlinx-serialization
  compiler plugin, so its `@Serializable` was inert and would not have produced a
  serializer.
- **No icon library.** `AddressRow` copies via a text button; `Icons.Default.ContentCopy`
  needs material-icons-extended, which the app does not depend on.

## Also fixed

`CustomHttpServer.start()` reused a stopped Ktor `EmbeddedServer`, which never rebinds.
Verified dead on a Pixel 7 Pro after Stop→Start: no listen socket, `curl` got
"Empty reply from server", while the UI said running. Now a fresh server per start,
`isRunning` from resolved connectors, and bind failures surfaced in the UI.

## Verification

- `:app:compileDebugKotlin` — pass
- `:app:ktlintCheck` — pass
- `:app:assembleDebug` — pass
- On-device endpoint testing — NOT DONE (needs the debug APK installed on the Pixel)
