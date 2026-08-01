# Technology Stack

**Analysis Date:** 2026-08-01

## Languages

**Primary:**
- **Rust** 2021 edition - Core P2P mesh node, embedded relay, blob store, JNI FFI to Android; all workspace crates (`myco-core`, `nsite-deck`, `myco-relay`, `myco-blossom`)
- **TypeScript** 5.6+ - Web-based nsite apps (React SPAs) deployed on Nostr, using Vite for bundling
- **JavaScript (ES Modules)** - Runtime environment for TypeScript builds

**Secondary:**
- **Java/Kotlin** - Android wrapper/JNI bridge (`android/` directory; calls into Rust via libmyco_core.so)
- **TOML** - Build and workspace configuration
- **JSON** - TypeScript config, package manifests, nsite metadata

## Runtime

**Environment:**
- **Rust**: tokio 1.x (multi-threaded async runtime) — required for background BLE/FSP/Noise handshakes to progress between FFI polls
- **Node.js**: v18+ (inferred from ES Module `"type": "module"` in package.json files)
- **Android Runtime** (Dalvik/ART) via JNI for mobile deployment

**Package Manager:**
- **Cargo** (Rust) - workspace resolver v2, pinned workspace dependencies with local path dependencies
- **npm** (Node.js) - per-app package management in `myco-ics/` and `myco-bitchat/`
- **Lockfile**: `Cargo.lock` (checked in); `package-lock.json` files (inferred, not committed)

## Frameworks

**Core:**
- **FIPS** (local path dependency) - Mesh networking, BLE (L2CAP), Noise IK handshakes, IPv6 TUN device
  - Built on `nostr-vpn`, source at `reference/fips` (gitignored, checked out separately)
- **nsite-deck** (internal crate) - Reusable content layer: gateway + sync + propagator (P2/P3)
  - Nostr event/NIP-19 handling via `nostr` crate v0.44
  - Event store and manifest queries (author-signed nsite metadata)
- **axum** 0.8 - Embedded HTTP/WebSocket servers for relay (NIP-01) and Blossom blob store
- **React** 18.3.1 - UI framework for nsite apps (`myco-ics`, `myco-bitchat`)
- **Vite** 5.4.11 - Build tool and dev server for TypeScript/React apps

**Testing:**
- Not detected in core codebase (dev-dependencies present in Cargo.toml but no explicit test framework listed)
- Unit tests referenced in Cargo examples and test features (e.g., `myco_core::examples`, `nsite-deck::testing` feature)

**Build/Dev:**
- **rustls** (TLS via OpenSSL alternative) - Cross-compilation friendly for Android
- **@vitejs/plugin-react** 4.3.4 - Vite React Fast Refresh plugin
- **TypeScript** 5.6.3 - Language and type checking (`tsc -b`)

## Key Dependencies

**Critical (Crypto/Protocol):**
- **nostr** 0.44 - Nostr event primitives, NIP-01 id computation, Schnorr signature verification, NIP-19 (npub) encoding/decoding
- **fips** (local path) - FIPS mesh protocol stack (BLE, Noise IK, IPv6 TUN) — **no public relay used**
- **async-trait** 0.1 - Trait async/await support (trait-based architecture)
- **sha2** 0.10 - Cryptographic hashing (manifest/blob integrity)
- **hex** 0.4 - Hex encoding/decoding for cryptographic outputs

**HTTP/Networking:**
- **reqwest** 0.12 (with `rustls-tls`, cross-compile friendly) - HTTP client for public relay/Blossom fallback
- **tokio-tungstenite** 0.24 (with `rustls-tls-webpki-roots`) - WebSocket client for relay connectivity
- **tokio** 1.x (features: `rt-multi-thread`, `sync`, `time`, `net`, `io-util`, `macros`) - Async runtime
- **socket2** 0.6 - Low-level socket control (IPv6_V6ONLY for mesh servers)
- **futures-util** 0.3 - Async utilities

**Serialization & Error Handling:**
- **serde** 1.x (with `derive`) - Serialization framework
- **serde_json** 1.x - JSON parsing/generation
- **thiserror** 2 - Ergonomic error types
- **anyhow** 1 - Flexible error handling

**Observability:**
- **tracing** 0.1 - Distributed tracing and logging framework (bridge to Android logcat via `paranoid-android`)
- **tracing-subscriber** 0.3 (Android-only, with `env-filter`) - Tracing sink

**Frontend/Web:**
- **applesauce-core** 6.x - Nostr SDK core reactive event store
- **applesauce-react** 6.x - React bindings for applesauce
- **applesauce-relay** 6.x - Relay client on top of applesauce
- **applesauce-signers** 6.x - Signer abstraction (NIP-07, hardware wallets)
- **nostr-tools** 2.10.0 - Nostr utilities (alternative to nostr crate for JS/TS)
- **rxjs** 7.8.1 - Reactive Extensions for JavaScript (observable streams)
- **react-dom** 18.3.1 - React DOM rendering

**Android-Specific:**
- **jni** 0.21 - Rust JNI bindings
- **paranoid-android** 0.2 - Bridge tracing logs to Android logcat

## Configuration

**Environment:**
- `.env` file present (configuration variables loaded at runtime)
- Referenced environment setup: `.envrc` (direnv for shell environment)
- No secrets committed; `.env*` patterns gitignored

**Build:**
- `Cargo.toml` (workspace root) - Rust workspace, member crates, workspace dependencies
- `tsconfig.json` per app (`myco-ics/`, `myco-bitchat/`) - TypeScript compiler configuration
- `vite.config.ts` (inferred from Vite scripts) - Vite build configuration (not read but referenced in build scripts)
- `justfile` - Task runner for common build/run commands

**Deployment:**
- Android app bundled via `android/app/` (Gradle-based)
- Web apps deployed via `nsite-cli upload dist` to Nostr (see `myco-ics` deploy script)
- No CI/CD config files detected (no `.github/workflows`, `gitlab-ci.yml`, etc.)

## Platform Requirements

**Development:**
- **Rust**: 1.70+ (edition 2021)
- **Node.js**: 18+ (for npm and Vite)
- **Android NDK**: Required for cross-compilation to `aarch64-linux-android`
- **macOS/Linux**: Host development environment
- **Java**: 11+ (for Android Gradle build)

**Production:**
- **Android 6.0+** - Target platform for mobile deployment (minSdkVersion inferred from toolchain)
- **Modern browsers** (ES2020+) for nsite web apps
- **No server backend required** - All services embedded or provided by Nostr network

## Architecture Layers

- **Layer 0 (Mesh)**: FIPS + BLE + Noise IK (via `fips` crate)
- **Layer 1 (Relay + Blob Store)**: Embedded HTTP/WebSocket servers (`myco-relay`, `myco-blossom` crates)
- **Layer 2 (Content)**: nsite manifest sync + propagation (`nsite-deck` crate)
- **Layer 3 (App)**: Device identity, app state, JNI FFI (`myco-core` crate + Android wrapper)
- **Layer 4 (UI)**: React SPAs (nsites) rendered in WebView

---

*Stack analysis: 2026-08-01*
