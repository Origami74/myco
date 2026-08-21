---
quick_id: 260820-mwx
slug: pr34-file-share-fixes
date: 2026-08-20
branch: feat/paired-myco-file-sharing
status: in-progress
---

# Fix PR #34 security + flow issues, rework file-share UX

Follow-up to the review of PR #34 (paired Myco file sharing). Fixes the four
blocking findings, adds transfer cancellation, and reshapes the UI to match the
hotspot share (pull-up sheets, top drop-in offer, Circle-tab transfer rows).

Out of scope (deferred by request): Blossom blob cleanup.

## Task 1 — Rust: security fixes (myco-core)

- `file_transfer.rs`: drop the direct `getrandom` dep; generate key + nonce via
  `chacha20poly1305::aead::OsRng`. Remove `getrandom` from `Cargo.toml` (both
  workspace and crate).
- `content.rs` `Ready` handler: stop overwriting the offer's `filename`/`mime`/
  `size`. A `Ready` that disagrees with what the user approved fails the transfer.
- `content.rs` `finish_incoming_transfer`: bound the download. Reject a
  `ciphertext_size` over the cap, check `Content-Length` up front, and stream
  chunks with a hard running-byte cap instead of `response.bytes()`.
- `content.rs`: delete the dead `let _ = keys;` and its unused `device_keys` clone.

## Task 2 — Rust: flow fixes (myco-core)

- Stop calling `forget_file_transfer` on failure paths so `failed` rows survive
  and carry their error to the UI.
- Add `sweep_file_transfers()`: non-terminal transfers past `expires_at` become
  `failed` with a timeout message. Drive from the existing keepwarm tick.
  Outgoing offers therefore expire after `OFFER_TTL_SECS` (10 min).
- Add `CancelFileTransfer` action (`action.rs`, `runtime.rs`, `content.rs`):
  marks the transfer cancelled and tells the peer.
- `forget_file_transfer` accepts `cancelled` as terminal.

## Task 3 — Kotlin: UX rework

- New `FileOfferBanner`: incoming offer drops in from the top, notification-like,
  Accept/Deny. Replaces the modal `AlertDialog`.
- `PeerShareSheet`: `skipPartiallyExpanded = true` + `verticalScroll` so it stops
  falling off the bottom; offline peers collapse behind an "N offline" expander
  (mirrors `CircleSection` in `MeshStatusSheet.kt`); send progress renders inline
  in the sheet instead of a separate modal.
- `ReceivedFileDialog` becomes a pull-up bottom sheet.
- Circle tab gains a transfers section modelled on "WAITING TO JOIN", with a
  per-transfer cancel.
- Delete `FileTransferProgressOverlay` (top banner + modal).
- Dedupe the three byte formatters onto the existing `formatSize`.

## Verification

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
- `cd android && ./gradlew assembleDebug`
- Device testing is required for the transfer path and cannot be done here; note
  it in the handoff.
