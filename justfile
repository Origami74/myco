# Myco build / test recipes. See docs/how-to/build.md.
#
# Host (macOS/Linux): `just test` needs only Rust — it compiles the workspace
# and runs myco-core's identity/reducer tests, no Android toolchain required.
#
# Android: `just build` needs cargo-ndk, the Android SDK + NDK (API 29+), and
# JDK 17. Set MYCO_FIPS_REPO_PATH to the local fips checkout (see §4).

android_dir := justfile_directory() / "android"
jni_dir     := android_dir / "app/src/main/jniLibs/arm64-v8a"
apk         := android_dir / "app/build/outputs/apk/debug/app-debug.apk"
so_name     := "libmyco_core.so"

# Default: host build + tests.
default: test

# Host build + unit tests (no Android toolchain).
test:
    cargo test

# Host build of the whole workspace.
check:
    cargo build

# Print the device identity from the real core path (host smoke check).
identity:
    cargo run -q -p myco-core --example identity

# Cross-compile myco-core for Android arm64 into jniLibs (standalone; the Gradle
# build below also does this via the buildRustArm64 task — pick one driver).
ndk-build:
    cargo ndk -t arm64-v8a --platform 29 -o {{android_dir}}/app/src/main/jniLibs build -p myco-core --release

# Build the debug APK (the Gradle task cross-compiles the Rust first).
build:
    cd {{android_dir}} && ./gradlew assembleDebug

# Build + install on the connected device (no launch).
install: build
    adb -d install -r {{apk}}

# Clean Gradle + Cargo + jniLibs.
clean:
    cd {{android_dir}} && ./gradlew clean
    cargo clean
    rm -rf {{jni_dir}}
