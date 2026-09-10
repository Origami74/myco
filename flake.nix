# Myco — Nix dev shells for the toolchain described in docs/how-to/build.md §1.
#
#   nix develop            host Rust shell: `just test`, `cargo fmt/clippy/test`
#   nix develop .#android  the above + Android SDK/NDK/JDK 17/Gradle/adb
#
# There is deliberately no `packages` output: the workspace depends on `fips`
# as a path dependency at reference/fips — a local, gitignored checkout — so a
# hermetic Nix build of the crates is not possible. These shells provide the
# toolchain; cargo and Gradle still drive the build.
{
  description = "Myco — offline-first P2P nsite mesh over BLE (Rust core + Android app) toolchain";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      ...
    }:
    let
      inherit (nixpkgs) lib;

      # x86_64-darwin is omitted: nixpkgs 26.11 has dropped it, and the Android
      # SDK archives androidenv resolves have no x86_64-darwin variant either.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      forAllSystems = f: lib.genAttrs systems (system: f system);

      # --- Version pins (keep in sync with android/app/build.gradle.kts) ---
      ndkVersion = "26.1.10909125"; # docs/how-to/build.md §1
      # compileSdk / targetSdk = 36; minSdk 29 kept for lint + docs of the floor.
      platformVersions = [
        "29"
        "36"
      ];
      # 36.0.0 matches compileSdk 36; 35.0.0 is AGP 8.11's default request.
      buildToolsVersions = [
        "35.0.0"
        "36.0.0"
      ];
      aapt2BuildTools = "36.0.0";

      pkgsFor =
        system:
        import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
          config = {
            allowUnfree = true; # the Android SDK is unfree
            android_sdk.accept_license = true;
          };
        };

      # Stable Rust plus the cross target the cargo-ndk build needs. `rust-src`
      # and `rust-analyzer` are here so editors work inside the shell.
      rustToolchainFor =
        pkgs:
        pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rust-analyzer"
          ];
          targets = [ "aarch64-linux-android" ];
        };

      androidCompositionFor =
        pkgs:
        pkgs.androidenv.composeAndroidPackages {
          inherit platformVersions buildToolsVersions;
          cmdLineToolsVersion = "13.0";
          platformToolsVersion = "37.0.1";
          includeNDK = true;
          ndkVersions = [ ndkVersion ];
          abiVersions = [ "arm64-v8a" ]; # arm64-v8a only (LOCKED)
          includeEmulator = false; # no emulator target — BLE/L2CAP need real devices
          includeSystemImages = false;
          includeCmake = false; # the Rust build uses cargo-ndk, not the SDK's CMake
        };
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          rustToolchain = rustToolchainFor pkgs;

          # `rustables` (via fips) runs bindgen; `ring` / `secp256k1-sys` run cc.
          libclang = pkgs.llvmPackages.libclang;

          hostPackages = [
            rustToolchain
            pkgs.just
            pkgs.pkg-config
            pkgs.clang
            libclang.lib
          ];

          # fips' Linux BLE backend (bluer) links libdbus-sys, so a host build
          # needs dbus on the pkg-config path — CI's `libdbus-1-dev`. The Android
          # cross-build does not: there fips uses AndroidBleIo, not bluer.
          hostBuildInputs = lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.dbus ];

          hostEnv = ''
            export LIBCLANG_PATH="${libclang.lib}/lib"

            # The workspace's `fips` path dependency (gitignored local checkout).
            if [ -z "''${MYCO_FIPS_REPO_PATH:-}" ] && [ -f "$PWD/reference/fips/Cargo.toml" ]; then
              export MYCO_FIPS_REPO_PATH="$PWD/reference/fips"
            fi
            if [ ! -f "''${MYCO_FIPS_REPO_PATH:-$PWD/reference/fips}/Cargo.toml" ]; then
              echo "warning: no fips checkout at reference/fips — nothing will build." >&2
              echo "         See docs/how-to/build.md §4 (upstream: github.com/k0sti/fips)." >&2
            fi
          '';
        in
        {
          default = pkgs.mkShell {
            name = "myco";
            packages = hostPackages;
            buildInputs = hostBuildInputs;
            shellHook = hostEnv + ''
              echo "myco host shell — rustc $(rustc --version | cut -d' ' -f2), just $(just --version | cut -d' ' -f2)"
              echo "  just test      host build + Rust unit tests"
              echo "  nix develop .#android   for the APK toolchain"
            '';
          };

          android =
            let
              androidComposition = androidCompositionFor pkgs;
              androidSdk = "${androidComposition.androidsdk}/libexec/android-sdk";
            in
            pkgs.mkShell {
              name = "myco-android";
              packages = hostPackages ++ [
                pkgs.cargo-ndk
                pkgs.jdk17
                pkgs.gradle
                pkgs.android-tools # adb
                androidComposition.androidsdk
              ];
              buildInputs = hostBuildInputs;

              ANDROID_HOME = androidSdk;
              ANDROID_SDK_ROOT = androidSdk;
              ANDROID_NDK_HOME = "${androidSdk}/ndk/${ndkVersion}";
              ANDROID_NDK_ROOT = "${androidSdk}/ndk/${ndkVersion}";
              JAVA_HOME = pkgs.jdk17.home;

              # AGP otherwise downloads an aapt2 from Maven that cannot run on
              # NixOS; point it at the SDK's patched binary instead.
              GRADLE_OPTS = "-Dorg.gradle.project.android.aapt2FromMavenOverride=${androidSdk}/build-tools/${aapt2BuildTools}/aapt2";

              shellHook = hostEnv + ''
                echo "myco android shell — NDK ${ndkVersion}, JDK $(java -version 2>&1 | head -1 | cut -d'"' -f2)"
                echo "  just build     assemble the debug APK (Gradle drives cargo-ndk)"
                echo "  just install   build + adb install -r"
                echo "note: adb device access needs programs.adb.enable = true in your NixOS config."
              '';
            };
        }
      );

      formatter = forAllSystems (system: (pkgsFor system).nixfmt-tree);
    };
}
