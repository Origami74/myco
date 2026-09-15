# Build Myco

How to build the Rust core and the Android APK. Myco is **arm64-only** and
targets **minSdk 29** (Android 10): the BLE transport uses L2CAP
Connection-Oriented Channels, and `createL2capChannel` /
`listenUsingInsecureL2capChannel` exist only on API 29+. There is no emulator
target — BLE, Wi-Fi Aware and NFC need phones.

For what this build produces, see [concepts.md](../design/core/concepts.md)
and [architecture.md](../design/core/architecture.md).

---

## 1. Prerequisites

### Option A — Nix (recommended)

`flake.nix` at the repo root ships two dev shells:

```sh
nix develop            # host shell: Rust (+ aarch64-linux-android target), clippy,
                       # rustfmt, rust-analyzer, just, clang/libclang, dbus —
                       # enough for `just test` and `cargo fmt --check`
nix develop .#android  # the above + Android SDK (platforms 29 + 36, build-tools
                       # 35/36), NDK 26.1.10909125, cargo-ndk, JDK 17, Gradle, adb
```

The Android shell exports `ANDROID_HOME`, `ANDROID_SDK_ROOT`, `ANDROID_NDK_HOME`,
`ANDROID_NDK_ROOT` and `JAVA_HOME`, so no `local.properties` is needed. It also
sets `GRADLE_OPTS=-Dorg.gradle.project.android.aapt2FromMavenOverride=…`: without
it AGP downloads an `aapt2` that cannot run on NixOS.

Both shells export `LIBCLANG_PATH` (bindgen, via fips's `rustables` dependency)
and carry dbus, which fips's Linux BLE backend (bluer) needs to link
`libdbus-sys`. Both default `MYCO_FIPS_REPO_PATH` to `reference/fips` when that
checkout is present, warning when it is not (§4).

The flake deliberately has **no `packages` output** — `fips` is a gitignored
path dependency, so a hermetic build is impossible; the flake provides the
toolchain and cargo/Gradle drive the build — and **no udev rules** (`adb` needs
`programs.adb.enable = true` in the host NixOS config). It sets `allowUnfree`
and `android_sdk.accept_license` in its own nixpkgs import. Pins live at the top
of `flake.nix`; keep them in sync with `android/app/build.gradle.kts`.

### Option B — manual install

| Tool | Version | Notes |
| --- | --- | --- |
| Rust | stable | [rustup](https://rustup.rs) |
| Rust target | `aarch64-linux-android` | `rustup target add aarch64-linux-android` |
| `cargo-ndk` | latest | `cargo install cargo-ndk` |
| Android NDK | 26.1.10909125 | SDK Manager or Nix |
| Android SDK | `compileSdk` / `targetSdk` 36, platform 29 | SDK Manager |
| JDK | 17 | Gradle + AGP |
| Gradle | the `gradlew` wrapper | no global install |
| `just` | latest | optional; recipes below are one-liners anyway |
| `adb` | platform-tools | install + logcat |

```sh
export ANDROID_HOME="$HOME/Library/Android/sdk"          # macOS default
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/26.1.10909125"
export MYCO_FIPS_REPO_PATH="$PWD/reference/fips"         # §4
```

---

## 2. Repository layout

```
fips-pop/
  android/                    Kotlin / Compose app, WebViews, radios, VpnService
    app/build.gradle.kts      arm64 abiFilter, minSdk 29, the buildRustArm64 task
    app/src/main/jniLibs/arm64-v8a/libmyco_core.so     (build output, gitignored)
  myco-core/                  the app crate and only cdylib: wiring, identity, node, JNI
  myco-napplet-runtime/       the napplet host (Android-free)
  nsite-deck/                 the nsite host (Android-free)
  myco-relay/                 embedded Nostr relay
  myco-blossom/               embedded Blossom store
  reference/fips/             LOCAL fips checkout — gitignored, required (§4)
  Cargo.toml                  the workspace; excludes reference/fips
  justfile                    test / build / install recipes
  flake.nix                   the toolchain
```

All five crates build into one `libmyco_core.so` behind one JNI surface —
a JSON reducer plus a few byte entry points
([ffi-surface.md](../reference/ffi-surface.md)). There is no UniFFI or
bindgen step.

---

## 3. Building

### Host — tests and checks (no Android toolchain)

```sh
just test                                     # cargo test, the default recipe
cargo test -p myco-core <name>                # one test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
just identity                                 # prints this host's device identity
```

Host `cargo test` runs every crate against in-memory seams. It never sees
`jni_abi.rs` or the radio bridges — those compile only for Android, so they are
verified by the cross-compile below. Pairing, BLE, NFC and mesh behaviour
regress only on phones.

### Android — the APK

```sh
just build        # cd android && ./gradlew assembleDebug
just install      # build + adb -d install -r
```

`assembleDebug`'s `mergeDebugNativeLibs` depends on the Gradle task
**`buildRustArm64`**, which runs

```sh
cargo ndk --target arm64-v8a --platform 29 --output-dir <jniLibs> \
  build --package myco-core --release [--features fips-multipath]
```

so one command cross-compiles the Rust and packages it. `./gradlew
:app:compileDebugKotlin` compiles the Kotlin alone, without the Rust build —
useful for a quick check of UI changes.

The standalone alternative, `just ndk-build`, runs the same `cargo ndk` line
directly into `jniLibs`. Don't combine it with `just build` in one go — that
compiles the Rust twice.

Required before a PR (per `CONTRIBUTING.md`): `cargo fmt --check`,
`cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and
`./gradlew assembleDebug` if you touched `android/`.

---

## 4. The `fips` dependency

Myco depends on the upstream [`fips`](https://github.com/jmcorgan/fips) crate
as a **path dependency at `reference/fips`** — a local, gitignored checkout.
Nothing builds without it.

```toml
# Cargo.toml (workspace)
fips = { path = "reference/fips", default-features = false }
exclude = ["reference/fips"]      # its own warnings, not ours; clippy skips it
```

### Which branch

Build against **`master`** — that is what CI clones (`FIPS_REF: master` in
`.github/workflows/ci.yml`). The checkout may carry local patches on top:
the app-owned TUN, injectable `BleIo`, per-peer PSM discovery, the macOS
`BleIo` — see [ble-interop.md](../design/fips/ble-interop.md) for what each is
and which are upstream candidates.

### `MYCO_FIPS_REPO_PATH` and the Gradle build

The Android build additionally reads `MYCO_FIPS_REPO_PATH` and emits a
`--config patch.crates-io.fips.path="…"` override for the `cargo ndk` call
(`localFipsCargoConfigArgs()` in `build.gradle.kts`). It refuses to build if
the variable is set and does not point at a fips checkout. A `patch.crates-io`
build perturbs `Cargo.lock`; if it comes back dirty after an Android build,
`git checkout Cargo.lock`.

### Features that follow the checkout

What a newer fips branch adds is read when present and absent otherwise — the
`paths` array of `show_peers` needs no flag. The one thing that does not compile
against `master` is the BLE transport's `role: backup` (path roles exist only on
`feat/multi-path-switchover`), so it sits behind the **`fips-multipath`** Cargo
feature. Gradle turns it on by itself when the checkout has `TransportRole`
(`mycoCoreFeatureArgs()`); a manual `cargo ndk … build -p myco-core` against
that branch wants `--features fips-multipath` added by hand. `state.multipathCore`
tells the radios which core they got.

### macOS dev build (host, not cross-compiled)

`cargo build -p myco-core --features ble-macos` builds a host core with the
CoreBluetooth `BleIo`, for an Android↔Mac test pair. Not needed for the phone.

---

## 5. arm64-only and minSdk 29

Both are locked and enforced in two places:

- `android/app/build.gradle.kts`: `minSdk = 29`, `ndk { abiFilters += "arm64-v8a" }`.
- `buildRustArm64`: `--target arm64-v8a --platform 29`.

An APK built here carries exactly one ABI:

```sh
unzip -l android/app/build/outputs/apk/debug/app-debug.apk | grep '\.so$'
# expect only lib/arm64-v8a/libmyco_core.so
```

---

## 6. Install and see it run

```sh
adb devices -l
adb -s <serial> install -r android/app/build/outputs/apk/debug/app-debug.apk
adb -s <serial> logcat | grep -E "myco|Napplet|Ble"
```

The Rust core logs through `paranoid-android` under the `myco` tag; the Dev tab
in the app shows the same peer diagnostics the logs do.

---

## 7. Release

`./gradlew assembleRelease` signs with `android/keystore.properties` (gitignored)
or the `MYCO_KEYSTORE_*` environment variables; with neither present the APK is
left unsigned. Publishing to GitHub Releases and Zapstore: [publish.md](./publish.md).
