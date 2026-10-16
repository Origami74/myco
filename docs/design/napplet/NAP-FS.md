NAP-FS in Myco
==============

Files the User Picks
--------------------

`draft` · **implementation notes**

**NAP ID:** NAP-FS
**Domain:** `fs`
**Spec:** [napplet/naps PR #88](https://github.com/napplet/naps/pull/88)
(`naps/NAP-FS.md`)
**Web binding (NIP-5D):** `window.napplet.fs` · `shell.supports("fs")`

> How Myco implements the upstream NAP-FS. The spec is authoritative; this
> page records Myco's choices where the spec leaves room, and what is not
> built. The spec is a **draft in an open PR, not merged**: re-audit this page
> and the code whenever it changes.
> Implementation: `myco-napplet-runtime/src/nap/fs.rs`,
> `assets/myco-prelude.js`; `myco-core/src/fs_pick.rs`;
> `android/.../FilePick.kt`, `NappletActivity.kt`.

## What it does

A napplet asks the user to pick a file, or a place to save one. Android's
own file picker comes up. The napplet then reads and writes **that file**,
in ranges, for as long as its window is open. It sees nothing else.

API, schemas, wire, error strings and base64 rules: see the spec.

## What Myco implements

The picker subset:

- `fs.info`
- `fs.pickFile`, `fs.pickFiles`, `fs.pickSaveFile`
- `fs.stat`, `fs.read`, `fs.write` — on picked entries only

Everything else answers `unsupported`, never silence:

- `fs.pickDirectory`, `fs.list`, `fs.mkdir`, `fs.remove`, `fs.move`
- `fs.watch`, `fs.unwatch`

`fs.info` says so: one root, `/picked` ("Files you choose"), with `read`,
`write` and `create`, and `maxWatchCount: 0`.

## Paths

- Every path is minted by Myco at pick time: `/picked/<name>`.
- `<name>` is the provider's display name, cleaned: no `/`, `\`, control or
  bidi formatting characters, at most 120 characters. Two picks with one
  name get `name (2).ext`, `name (3).ext`…
- A path is checked against the spec's path model, then looked up exactly
  (code point for code point, case-sensitive). Anything not picked in this
  window is `not-found`, however it is spelled. There is no host filesystem
  behind it to escape into.
- No host path, URI, provider or volume ever reaches the napplet.

## Picking

- The window shows Android's picker: `ACTION_OPEN_DOCUMENT` (one or
  several) or `ACTION_CREATE_DOCUMENT` (save).
- A napplet's message carries no user gesture. A touch on the window in the
  last 5 s vouches for it, as for links; otherwise a one-tap "Choose a
  file?" comes first. Cancel there is `cancelled`.
- One pick at a time per window. A second while one is up is
  `policy-denied`.
- `accept` → the picker's MIME filter. A `mime` is used as given; an
  `extension` through a small table (`nap::fs::mime_for_extension`). Nothing
  usable means every type. Advisory, as the spec says.
- `suggestedName` → the save picker's starting name. The save file's type is
  the first concrete `accept` type, else the name's extension, else
  `application/octet-stream`.
- `description` is ignored: Android's picker has nowhere to show it.
- Backing out of the picker is `cancelled`. An empty pick is never a success.
- The prelude gives the three pickers a 5 min wait (the vendored shim's is
  30 s); a person browsing folders takes longer.

## Permissions

What a picked entry carries is the intent, narrowed to what the provider
opened:

- `pickFile` / `pickFiles`: `read` by default. `write` if asked for and the
  provider opens the document `"rw"`; otherwise it is opened `"r"` and has
  no `write`. Never `"w"` on an existing document: some providers truncate.
- `pickSaveFile`: `write` and `create` by default; `read` too if asked for.
  The new file is opened `"rw"`, else `"w"`.
- `list`, `delete`, `watch` are never granted.
- A save destination is "absent" until first written, so `ifAbsent: true`
  works on it once.

## Bytes and limits

- `maxReadBytes` 1 MiB, `maxWriteBytes` 1 MiB, `maxInFlightBytes` 4 MiB per
  window. Decoded bytes, as the spec says. Over the in-flight cap:
  `policy-denied`.
- A read is one positional read of that range (`pread`). The file is never
  loaded whole.
- Write modes: `replace` (write, then cut to length), `append`, `patch`.
  Writes to one entry are serialized, so appends never interleave.
- `patch` past the end of the file is `unsupported`: not every provider can
  make a hole.
- `ifRevision` is `unsupported`: no revisions are exposed. `stat` and pick
  results carry no `revision` and no timestamps.
- Storage full is `quota-exceeded`; any other provider failure `io-error`.
- Malformed options (unknown mode, `offset` with `replace`/`append`, `patch`
  without one, a negative number) are `invalid-data`. The spec has no
  argument error yet.

## Lifetime

- Picks live in the window's session (`Session::picked`). Session-only, as
  the spec allows. Nothing is persisted; no persistable URI permission is
  taken.
- Closing the window drops the session and closes the descriptors.
- A window holds at most 64 picked entries. Past that the oldest is dropped
  (revoked, which the spec allows at any time).

## How the bytes get to Rust

The window host opens each chosen URI with
`ContentResolver.openFileDescriptor` and hands Rust the **detached
descriptor** (`nappletFsPicked`), never the URI. Rust owns it from then on
and reads and writes it by offset (`PickedBytes` for `std::fs::File`). A
document that is not a seekable file (`statSize < 0`, a pipe from a
streaming provider) is left out; if nothing is left, `io-error`.

No upcall from Rust into Kotlin, and no shell-page byte hop: the spec's
`data` is base64 text already, so it crosses as the napplet sent it.

## Consent

- `fs` is **not a default grant**. A napplet gets it only if its manifest
  declares it and the user allows it on the install sheet: "Files — Open
  files you choose, and save files where you choose."
- Like `upload`, a reviewed list recorded before this build does not count
  for it (`GRANTED_ONLY_WHEN_SHOWN`); the update review asks.
- Not granted, or not yet handshaken: `policy-denied`.
- Each pick is its own consent: the user chose that file.

## Picked files for other features (follow-up)

The store is built so other features can take a picked file by its path
later. Not wired in this change:

- `Session::picked().get(path)` → `PickedHandle { name, mime, permissions,
  bytes }`. Holding one keeps the descriptor open; drop it when done.
- `PickedBytes::read_all(max)` reads a whole file under a cap, for
  something that sends it on.
- **NAP-UPLOAD** could accept a picked path instead of a `Blob`, so a large
  file never crosses the JSON channel as base64.
- **Mesh file sharing** (`myco-core/src/file_transfer.rs`) could offer a
  picked file to a peer the same way.

## Not built

- `pickDirectory`, `list`, `mkdir`, `remove`, `move`, `watch`, `unwatch`,
  and any shared or persistent root.
- Revisions and `ifRevision`.
- Durable grants across reloads.
- An atomic `replace`. Android's document providers have no
  write-then-rename, so a write that fails part way (storage full, provider
  gone) can leave the file changed. The spec asks for all-or-nothing; this
  is a known gap.
- Timestamps (`modifiedAt`, `createdAt`): omitted, as the spec suggests.
- Picker rate limiting beyond one-at-a-time and the gesture gate.

## Implementations

- Runtime: `nap/fs.rs` (`PickRequest`, `PickedStore`, path model, read and
  write), the `PickedBytes` / `PickedFile` seams, `Outcome::FsPick`,
  `Session::picked`.
- Shell: `assets/myco-prelude.js` (`installFsPick`, the long picker wait).
- Core: `myco-core/src/fs_pick.rs` (pending pick per window, `pick-file`
  command, `pick_answer`), `nappletFsPicked` in `jni_abi.rs`.
- Android: `FilePick.kt` (picker intents, opening descriptors),
  `NappletActivity.kt` (gesture gate, launcher), the "Files" wording in
  `AppsScreen.kt`.
