# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Myco is an offline-first, peer-to-peer Android app for sharing **nsites** (static web apps published on Nostr) over a Bluetooth LE mesh — no internet, no app store. Kotlin/Compose shell on top of a single Rust native library (`libmyco_core.so`).

## Commands

```bash
just test        # host build + all Rust unit tests (default recipe; no Android toolchain needed)
cargo test -p myco-core <name>       # run a single test / filter by name
cargo fmt --check
cargo clippy --all-targets -- -D warnings
just identity    # host smoke check: prints device identity via myco-core example

just build       # debug APK (Gradle cross-compiles the Rust via the buildRustArm64 task)
just ndk-build   # standalone cargo-ndk cross-compile into jniLibs (don't combine with just build — double-compiles)
just install     # build + adb install -r
cd android && ./gradlew testDebugUnitTest   # Kotlin unit tests (app/src/test)
```

Required before opening a PR (per CONTRIBUTING.md): `cargo fmt --check`, `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and `./gradlew assembleDebug` if you touched `android/`. Run your branch through the 13-criteria checklist in `PR-REVIEW.md`.

### The fips dependency

The workspace depends on `fips` as a **path dependency at `reference/fips`** — a local, gitignored checkout (upstream: github.com/k0sti/fips) carrying local patches (app-owned TUN, injectable `BleIo`, per-peer PSM discovery, macOS `BleIo`). Nothing builds without it. The Android Gradle build additionally reads `MYCO_FIPS_REPO_PATH` to emit a `patch.crates-io` override; `patch.crates-io` builds perturb `Cargo.lock`, so watch for a dirty lockfile after Android builds. Details: `docs/how-to/build.md` §4.

### Android constraints (LOCKED)

- **arm64-v8a only**, no emulator target — BLE/L2CAP need physical devices.
- **minSdk 29** — hard floor: L2CAP Connection-Oriented Channel APIs exist only on API 29+.
- Android builds need cargo-ndk, Android SDK + NDK, JDK 17, and the `aarch64-linux-android` Rust target.

## Architecture

Four Rust crates build into the one `cdylib`, `libmyco_core.so`:

- **`myco-core`** — the app crate and only cdylib. Owns device identity (one Nostr keypair, persisted on first launch), embeds the FIPS mesh node, and wires everything together: Tokio multi-thread runtime (`runtime.rs`), TUN packet bridge, `.fips` DNS interception, BLE/Wi-Fi Aware/AP lane bridges, peer diagnostics, and mesh gossip.
- **`nsite-deck`** — reusable, transport-agnostic nsite host: gateway (manifest → path → sha256 → serve), sync/import engine, propagator. Reaches the outside world only through four trait seams in `seams.rs`: `RelayBackend`, `BlobStore`, `PeerSource`, `FanoutSink`. It names no concrete relay, store, or radio — keep it that way.
- **`myco-relay`** — embedded NIP-01 relay implementing `RelayBackend` (ws on :4870). Hand-rolled store over rust-nostr `Event` types, deliberately no relay framework: manifests are replaceable/addressable (newest-per-slot, persisted to JSON); regular events (chat) are by-id, ephemeral, memory-only.
- **`myco-blossom`** — embedded Blossom blob store implementing `BlobStore` (http on :24243). Content-addressed files named by sha256; verifies hash on write (atomic temp+rename), trusts the name on read.

### The FFI boundary

Kotlin ↔ Rust is a **JNI + JSON-over-strings Redux-style reducer**: `dispatch(actionJson) -> stateJson` over an opaque `jlong` handle, with a monotonic `rev` (`myco-core/src/jni_abi.rs` ↔ `android/.../core/NativeCore.kt`). There is no UniFFI/bindgen step. Actions live in `action.rs`, the state snapshot in `state.rs`. Kotlin owns UI, WebViews, the radios (BLE/NFC/Aware/AP), and the `VpnService`/TUN fd; Rust owns identity, the mesh node, and the content services.

`jni_abi.rs` compiles **only for Android** — host `cargo test` never sees it, so JNI glue is verified only by the cargo-ndk build. Conversely, many `myco-core` modules are Android-only consumers marked `#[cfg_attr(not(target_os = "android"), allow(dead_code))]`; host tests drive `AppRuntime` directly. The Rust talks to the running fips node via its Unix-domain **control socket** (`control_client.rs`) — the only way to read peer state or push a platform-discovered peer once the node's rx loop owns it.

### Android app (`android/app/src/main/java/app/myco/`)

Compose bottom-nav shell (`ui/MycoApp.kt`: Apps · Circle · Discover · Settings · Dev). Each installed nsite renders in its own chrome-less WebView activity (`NsiteActivity`). Radios are their own packages: `ble/`, `nfc/`, `aware/`, `ap/`; pairing/share links in `share/`; the reducer client in `core/`.

## Testing philosophy

Host `cargo test` uses in-memory mocks (`nsite-deck/src/testing.rs`: `MemRelay`, `MemBlobs`) and is necessary but **not sufficient** for pairing/BLE/NFC/mesh changes — those regress only on physical devices, usually two paired ones. When core logic can't be host-tested, say so in the PR and describe the manual on-device test. See `docs/how-to/run-two-device-demo.md`.

## Conventions

- Single-trunk: branch off `main`, PR back into `main`, squash WIP commits.
- One logical change per PR; no drive-by reformatting or out-of-footprint cleanups.
- Design-affecting changes update the matching `docs/design/` page; user-visible changes update `README.md`; release notes go in `CHANGELOG.md` under `[Unreleased]`.
- Many docs in `docs/design/` and `docs/how-to/` were written in forward-looking "proposal voice" before the code existed and mark items **TBD / open** — where a doc and the tree disagree, the tree (justfile, Cargo.toml, build.gradle.kts) is current. `docs/design/concepts.md` is the glossary; start there for terminology (npub/node_addr, `.fips` vs `.nsite`, Pillars of Propagation).
