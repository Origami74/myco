---
quick_id: 260820-mwx
slug: pr34-file-share-fixes
date: 2026-08-20
branch: feat/paired-myco-file-sharing
status: complete
---

# Summary

Fixed the four blocking findings from the PR #34 review, added transfer
cancellation and a stall timeout, and reshaped the file-share UI to match the
hotspot share.

## Security

- `file_transfer.rs` now takes its key and nonce from `chacha20poly1305`'s
  `aead::OsRng`. The direct `getrandom` dependency is gone from both manifests.
- A `myco.file_ready.v1` message can no longer change the filename, MIME type or
  size the user approved in the offer — `file_offer_mismatch` fails the transfer
  instead. Compared post-sanitisation so a re-spelled path is still the same name.
- The incoming download is bounded: `ciphertext_size` is checked against
  `MAX_PACKAGE_BYTES` on arrival, `Content-Length` is checked before reading, and
  the body streams with a running cap instead of `response.bytes()`.
- Removed a dead `device_keys` clone (`let _ = keys;`).

## Flow

- Failure paths keep their row. `error` now reaches the UI instead of being
  written and deleted in the same breath. Only a *completed send* self-clears.
- `sweep_file_transfers()` runs off the keepwarm tick and fails anything stalled
  past `expires_at`. The deadline is reset on every status change, so it measures
  a stall rather than total duration — a large transfer that is progressing is
  not killed for outliving its offer window.
- New `CancelFileTransfer` action. Cancel travels as a decline, and the decline
  handler now resolves the *recipient's* row too, so a sender-side cancel doesn't
  leave the other phone waiting.
- `clear_transfer_secrets` drops the key and staging file of a dead transfer
  while keeping the row that explains it.

## UX

- Incoming offers drop in from the top as a notification-style card
  (`FileOfferBanner`), replacing an undismissable modal.
- `PeerShareSheet` uses `skipPartiallyExpanded` + `verticalScroll` so the send
  button stops falling off the bottom; offline peers collapse behind an
  "N offline" expander; send progress renders in the sheet instead of a modal.
- `ReceivedFileDialog` is a pull-up sheet.
- Circle tab gained a TRANSFERS section next to WAITING TO JOIN, with cancel on
  live rows and dismiss on failed ones.
- Deleted `FileTransferProgress.kt` (top banner + blocking modal).
- One byte formatter (`app.myco.ui.formatSize`); the two duplicates are gone.
- Dropped the optimistic "offer sent" toast, which claimed success before any
  send had been attempted.

## Verification

- `cargo fmt --check`, `cargo build`, `cargo test` — 142 myco-core tests pass
  (4 new), full workspace 1830 pass.
- `./gradlew assembleDebug` succeeds.
- New tests: ready-message mismatch, offer expiry, stall-window reset, terminal
  states that may be dismissed.

## Not done

- Blossom blob cleanup — deferred by request, to be handled elsewhere.
- `docs/design/` page for the wire protocol (four message kinds, TTL-0
  addressing, `MYCO-FILE-V1` container, trust model). CHANGELOG entry was added.
- The PR's `Box` re-wrap in `MycoApp.kt` left ~100 lines at the old indentation.
  Left alone to avoid a reformatting diff on top of these changes.
- On-device testing. None of this was run on two phones; the transfer path
  cannot regress in host tests.
