# Build Myco

How to cross-compile the Rust backend and assemble the Android APK for
**Myco**. This is a forward-looking runbook for an app still under
construction: where a step depends on an unbuilt phase, the *intended*
procedure is described and marked **(once Phase N lands)**.

Myco is **arm64-only** and targets **minSdk 29** (Android 10). The `minSdk`
floor is a hard requirement: the FIPS BLE transport uses L2CAP Connection-Oriented
Channels, and `BluetoothDevice.createL2capChannel(psm)` /
`listenUsingInsecureL2capChannel()` only exist on **API 29+**.

For the system this build produces, see
[../design/concepts.md](../design/concepts.md) and
[diagrams/01-system-layering.svg](../design/diagrams/01-system-layering.svg).

> These are design docs for a not-yet-built app. Commands below are modelled on
> our base, [reference/nostr-vpn](../../reference/nostr-vpn/) (with the
> [fips](../../reference/fips/) core for protocol detail), and adapted to
> Myco's decisions. Anything not yet verifiable against Myco's own tree
> is marked **TBD / open**.

---

## 1. Prerequisites

The toolchain matches the two reference projects. The fastest path is Nix; a
manual install is also fine.

### Option A — Nix (optional, recommended)

```sh
nix develop          # provides Rust, cargo-ndk, NDK, JDK 17, Gradle, just, adb
```

A Nix dev shell would provide Rust, cargo-ndk, the Android NDK, JDK 17, Gradle,
just and adb. A `flake.nix` for Myco is **TBD / open** — until it lands, use
Option B.

### Option B — manual install

| Tool | Version | Notes |
| --- | --- | --- |
| Rust | stable toolchain | install via [rustup](https://rustup.rs) |
| `cargo-ndk` | latest | `cargo install cargo-ndk` |
| Rust target | `aarch64-linux-android` | `rustup target add aarch64-linux-android` |
| Android NDK | 26.1 (matches reference) | installed via Android SDK Manager or Nix |
| JDK | 17 | Gradle + Android Gradle Plugin require it |
| Android SDK | platform + build-tools for API 29+ | `compileSdk` 36, `targetSdk` 36 (proposed; matches reference) |
| Gradle | via the project `gradlew` wrapper | no global install needed |
| `just` | latest | command runner; recipes are adapted below |
| `adb` | from platform-tools | install + logcat |

Set the standard SDK/NDK environment variables (the reference
`tools/run-android` autodetects these, but for a clean shell set them
explicitly):

```sh
export ANDROID_HOME="$HOME/Library/Android/sdk"        # macOS default
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/26.1.10909125"
```

(See [reference/nostr-vpn/tools/run-android](../../reference/nostr-vpn/tools/run-android)
for the autodetection logic this mirrors.)

---

## 2. Repository layout

The expected workspace layout (proposed; **TBD / open** until the Myco tree
is scaffolded):

```
Myco/
  android/                Kotlin / Jetpack Compose app + WebView, BLE radio
    app/build.gradle.kts  arm64 abiFilter, minSdk 29, Rust build task
    app/src/main/jniLibs/arm64-v8a/libmyco_core.so   (build output)
  myco-core/            Rust crate: one .so, one FFI surface
                          (embedded Nostr relay + Blossom + FIPS endpoint + DNS intercept)
  Cargo.toml              workspace root
  justfile                build / install / demo recipes
  reference/fips/         LOCAL FIPS checkout, wired in via patch.crates-io (see §4)
```

`myco-core` is the **backend crate** (it depends on the reusable `nsite-deck`
crate): together they build to a single `cdylib` (`libmyco_core.so`) exposing one
JNI surface. Rather than a UniFFI / `uniffi-bindgen` codegen step, Myco follows
nostr-vpn's **JNI + JSON-over-strings** FFI
(`System.loadLibrary`, a Redux-style `dispatch(actionJson) -> stateJson`
reducer over an opaque `jlong` handle; see
[reference/nostr-vpn/crates/nostr-vpn-app-core/src/c_abi.rs](../../reference/nostr-vpn/crates/nostr-vpn-app-core/src/c_abi.rs)).
**There is therefore no bindings-generation step** in the Myco pipeline.

---

## 3. The build pipeline

Two stages, same shape as both reference projects:

1. **cargo-ndk** cross-compiles `myco-core` for `aarch64-linux-android` and
   drops `libmyco_core.so` into `android/app/src/main/jniLibs/arm64-v8a/`.
2. **Gradle** assembles the APK, packaging that `.so`.

There are two equivalent ways to drive this, depending on whether you want
Gradle to invoke Cargo for you (the nostr-vpn model) or to run the cross-compile
as an explicit `just` step. Myco will support both;
the Gradle-driven path is the default because it keeps `./gradlew assembleDebug`
self-contained.

### 3a. Gradle-driven (default) — modelled on nostr-vpn

`reference/nostr-vpn/android/app/build.gradle.kts` registers an `Exec` task that
runs cargo-ndk and wires it into the native-libs merge step. The Myco
equivalent (proposed) looks like:

```kotlin
// android/app/build.gradle.kts
val repoRoot = layout.projectDirectory.dir("../..")
val rustOutputDir = layout.projectDirectory.dir("src/main/jniLibs")

tasks.register<Exec>("buildRustArm64") {
    workingDir = repoRoot.asFile
    commandLine(
        *(listOf(
            "cargo", "ndk",
            "--target", "arm64-v8a",
            "--platform", "29",                          // minSdk 29 (L2CAP)
            "--output-dir", rustOutputDir.asFile.absolutePath,
            "build",
        ) + localFipsCargoConfigArgs()                   // §4 — local FIPS wiring
          + listOf("--package", "myco-core", "--release")
        ).toTypedArray()
    )
}

tasks.matching { it.name in listOf("mergeDebugNativeLibs", "mergeReleaseNativeLibs") }
    .configureEach { dependsOn("buildRustArm64") }
```

Then a plain Gradle build also compiles the Rust:

```sh
cd android && ./gradlew assembleDebug
```

Note the differences from the nostr-vpn original:
`--package myco-core` (not `nostr-vpn-app-core`) and `--platform 29` (not 26).

### 3b. `just`-driven — modelled on nostr-vpn

For a CLI-first flow, Myco's `justfile` (proposed) adapts
[nostr-vpn's `Justfile`](../../reference/nostr-vpn/Justfile) recipes (no UniFFI
bindings step — Myco uses JNI/JSON):

```just
android_dir := justfile_directory() / "android"
jni_dir     := android_dir / "app/src/main/jniLibs/arm64-v8a"
apk         := android_dir / "app/build/outputs/apk/debug/app-debug.apk"
package     := "app.myco"          # placeholder applicationId
ndk_target  := "aarch64-linux-android"
so_name     := "libmyco_core.so"

# Cross-compile myco-core for Android arm64
ndk-build:
    cargo ndk -t arm64-v8a --platform 29 build -p myco-core --release

# Copy native library into Android jniLibs
libs: ndk-build
    mkdir -p {{jni_dir}}
    cp target/{{ndk_target}}/release/{{so_name}} {{jni_dir}}/

# Build debug APK
build: libs
    cd {{android_dir}} && ./gradlew assembleDebug

# Build + install on the connected device (no launch)
install: build
    adb -d install -r {{apk}}

# Clean Gradle + Cargo + jniLibs
clean:
    cd {{android_dir}} && ./gradlew clean
    cargo clean
    rm -rf {{jni_dir}}
```

A one-shot debug build then is simply:

```sh
just build        # cross-compile myco-core -> jniLibs, assemble debug APK
just install      # build + adb install -r
```

> If you use the §3a Gradle-driven task, `just build` should *not* also run
> `cargo ndk` directly, or you double-compile. Pick one driver per repo and make
> the `justfile` `build` recipe just call `./gradlew assembleDebug`. Which driver
> ships as Myco's canonical path is **TBD / open**; this doc documents both
> so the build wiring is unambiguous either way.

---

## 4. Wiring in the LOCAL `reference/fips` checkout

Myco builds against a **local** FIPS source tree (a LOCKED decision), so we
can patch PSM / dial-direction for Android L2CAP without waiting on upstream.
The mechanism is borrowed directly from nostr-vpn:
[reference/nostr-vpn/android/app/build.gradle.kts](../../reference/nostr-vpn/android/app/build.gradle.kts)
reads an env var and emits `--config patch.crates-io.<crate>.path="…"` flags into
the `cargo ndk` invocation.

### 4a. Point an env var at the checkout

nostr-vpn uses `NVPN_FIPS_REPO_PATH`. Myco's analogue (proposed):

```sh
export FIPSPOP_FIPS_REPO_PATH="$PWD/reference/fips"
```

### 4b. Emit `patch.crates-io` overrides

The Gradle helper (adapted from `localFipsCargoConfigArgs()` in the reference)
validates the path and emits one `--config` pair per FIPS crate:

```kotlin
fun localFipsCargoConfigArgs(): List<String> {
    val fipsPath = System.getenv("FIPSPOP_FIPS_REPO_PATH")?.takeIf { it.isNotBlank() }
        ?: return emptyList()
    val fipsRoot = file(fipsPath)
    // crate set is TBD — see note below
    val crateNames = listOf("fips-core", "fips-endpoint", "fips-identity")
    return crateNames.flatMap { crate ->
        val dir = fipsRoot.resolve("crates/$crate").absolutePath
        require(file(dir).isDirectory) {
            "FIPSPOP_FIPS_REPO_PATH must point at a fips checkout with $crate"
        }
        listOf("--config", "patch.crates-io.$crate.path=\"$dir\"")
    }
}
```

Equivalently, by hand on the CLI:

```sh
cargo ndk -t arm64-v8a --platform 29 build -p myco-core --release \
  --config 'patch.crates-io.fips-core.path="reference/fips/crates/fips-core"' \
  --config 'patch.crates-io.fips-endpoint.path="reference/fips/crates/fips-endpoint"' \
  --config 'patch.crates-io.fips-identity.path="reference/fips/crates/fips-identity"'
```

### 4c. **Open issue — crate layout mismatch**

> **TBD / open.** The nostr-vpn helper assumes a *multi-crate* FIPS checkout
> (`crates/fips-core`, `crates/fips-endpoint`, `crates/fips-identity`). The
> `reference/fips` checkout vendored here is a **single crate** named `fips`
> ([reference/fips/Cargo.toml](../../reference/fips/Cargo.toml) declares
> `name = "fips"`, with `src/transport/ble/` inline, no `crates/` directory).
>
> So the override list above is **not** correct against this exact checkout —
> the crate names and paths depend on how `myco-core` actually declares its
> FIPS dependency. Two outcomes, to be resolved when `myco-core` is written:
>
> - **If** `myco-core` depends on the single `fips` crate, the wiring is one
>   override: `--config 'patch.crates-io.fips.path="reference/fips"'`.
> - **If** we re-split FIPS into `fips-core` / `fips-endpoint` / `fips-identity`
>   to match the nostr-vpn embedding (`FipsEndpoint::builder().without_system_tun()`,
>   see [reference/nostr-vpn/crates/nostr-vpn-cli/src/fips_private_mesh/runtime_send.rs](../../reference/nostr-vpn/crates/nostr-vpn-cli/src/fips_private_mesh/runtime_send.rs)),
>   the three-crate list above applies.
>
> Resolving this is a Phase 0 task **(once the myco-core crate lands)**. Treat
> the crate names in §4b as a placeholder, not a verified fact.

### 4d. `Cargo.lock` hygiene

The reference `tools/run-android` snapshots `Cargo.lock` before a local-FIPS
build and restores it afterward, because `patch.crates-io` overrides perturb the
lockfile. Myco should do the same — see the `restore_lock` / `prepare_lock_restore`
trap in [reference/nostr-vpn/tools/run-android](../../reference/nostr-vpn/tools/run-android).
Without it, a local-path build leaves `Cargo.lock` dirty in your working tree.

---

## 5. arm64-only and minSdk 29

Both constraints are LOCKED and enforced in two places:

1. **Gradle** — restrict the ABI filter and SDK floor in
   `android/app/build.gradle.kts` (proposed):

   ```kotlin
   android {
       defaultConfig {
           applicationId = "app.myco"       // placeholder
           minSdk = 29                          // L2CAP CoC requirement
           ndk { abiFilters += "arm64-v8a" }    // arm64 only
       }
   }
   ```

   (The reference nostr-vpn build sets `minSdk = 26` with the same single
   `arm64-v8a` abiFilter; Myco raises the floor to 29.)

2. **cargo-ndk** — only ever pass `-t arm64-v8a` / `--target arm64-v8a`. No
   other Rust target is cross-compiled, so the APK carries a single `.so`.

There is **no x86 / emulator target**. The v1 demo runs on physical arm64
handsets (BLE + L2CAP are not available on the standard emulator). See
[run-two-device-demo.md](./run-two-device-demo.md).

---

## 6. Verify the build

```sh
# the native lib is present in the APK staging dir
ls -l android/app/src/main/jniLibs/arm64-v8a/libmyco_core.so

# the APK was produced
ls -l android/app/build/outputs/apk/debug/app-debug.apk

# confirm it carries exactly one ABI
unzip -l android/app/build/outputs/apk/debug/app-debug.apk | grep 'lib/'
# expect only lib/arm64-v8a/libmyco_core.so
```

A green build does **not** mean the mesh works end to end — for that, run the
two-device demo: [run-two-device-demo.md](./run-two-device-demo.md).

---

## Open questions

- **Crate split (§4c):** does `myco-core` depend on the single `fips` crate or
  a re-split `fips-core`/`fips-endpoint`/`fips-identity`? Determines the
  `patch.crates-io` override set. **TBD / open** until `myco-core` lands.
- **`flake.nix`:** Myco has no Nix dev shell yet. **TBD / open.**
- **`compileSdk`/`targetSdk`:** proposed at 36 to match reference; not yet pinned
  for Myco. **TBD / open.**
- **Canonical build driver (§3):** Gradle-driven `Exec` task vs. `just ndk-build`.
  Both documented; the shipped default is **TBD / open**.
