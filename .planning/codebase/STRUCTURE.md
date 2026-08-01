# Codebase Structure

**Analysis Date:** 2026-08-01

## Directory Layout

```
fips-pop/
├── android/                    # Android app (Kotlin, Jetpack Compose)
│   ├── app/src/main/
│   │   ├── kotlin/             # Kotlin UI, JNI call site, FFI contract
│   │   └── res/                # Strings, drawables, layouts
│   └── build.gradle.kts
│
├── myco-core/                  # App crate: FIPS endpoint, JNI bridge, content wiring
│   ├── src/
│   │   ├── lib.rs              # Library root, module declarations
│   │   ├── runtime.rs          # AppRuntime, FFI dispatch, Tokio runtime
│   │   ├── state.rs            # AppState, IdentityView, NodeStatus (serde)
│   │   ├── action.rs           # NativeAppAction enum (Redux actions)
│   │   ├── content.rs          # Content layer orchestrator (102KB)
│   │   │                        # Library, Circle, sites, nsite sync
│   │   ├── peer_relay.rs       # PeerRelayPool: persistent peer WebSocket connections
│   │   ├── gossip.rs           # MeshGossiper: subscribe + fan-out propagation
│   │   ├── dns_intercept.rs    # DNS `.fips` interception via TUN
│   │   ├── tun_bridge.rs       # Read/write IPv6 packets from VpnService TUN
│   │   ├── ip_source.rs        # IpPeerSource: HTTPS fallback (public relay/Blossom)
│   │   ├── identity_store.rs   # Load/generate Nostr keypair (nsec)
│   │   ├── udp_fd_bridge.rs    # Expose UDP transport fd to Android (WiFi Aware pin)
│   │   ├── jni_abi.rs          # JNI entry point, string marshalling
│   │   ├── ble_bridge_jni.rs   # Bridge to Android BLE radio (L2CAP I/O)
│   │   ├── aware_bridge_jni.rs # Bridge to Android WiFi Aware NDP interface
│   │   ├── tun_bridge_jni.rs   # Bridge to Android VpnService TUN device
│   │   └── examples/
│   │       └── identity.rs     # Dev-only identity generation example
│   └── Cargo.toml
│
├── nsite-deck/                 # Reusable nsite host (transport-agnostic)
│   ├── src/
│   │   ├── lib.rs              # Public API: serve, sync_site, import_site
│   │   ├── gateway.rs          # HTTP gateway: manifest → path → blob → serve
│   │   ├── host.rs             # Host parsing: npub + dTag → SiteAddr
│   │   ├── sync.rs             # Sync/import engine: fetch + verify + store
│   │   ├── seams.rs            # Trait seams: RelayBackend, BlobStore, PeerSource, FanoutSink
│   │   ├── model.rs            # Manifest types, site keys, path normalization
│   │   ├── content_type.rs     # MIME type detection
│   │   ├── base36.rs           # Base36 encoding (pubkey compression for named sites)
│   │   ├── testing.rs          # Test fixtures (MemRelay, MemBlobs, build_test_site)
│   │   └── tests/              # Unit tests (import_then_serve, incomplete_site, etc.)
│   └── Cargo.toml
│
├── myco-relay/                 # Embedded Nostr relay (NIP-01)
│   ├── src/
│   │   ├── lib.rs              # Public API, module exports
│   │   ├── store.rs            # RelayStore: in-memory event store (replaceable semantics)
│   │   └── server/
│   │       ├── mod.rs          # Server plumbing, bind, serve_on_hub
│   │       ├── hub.rs          # RelayHub: routes messages to subscribers
│   │       ├── connection.rs   # Per-client WebSocket connection handler
│   │       └── gossip.rs       # Gossiper trait (P3 propagation interface)
│   └── Cargo.toml
│
├── myco-blossom/               # Embedded Blossom blob store (SHA256-addressed)
│   ├── src/
│   │   ├── lib.rs              # Public API
│   │   ├── store.rs            # FsBlobStore: filesystem blob store
│   │   └── server/
│   │       ├── mod.rs          # HTTP server, bind, serve_on_guarded
│   │       └── handler.rs      # GET /hash, PUT /hash, HEAD /hash
│   └── Cargo.toml
│
├── myco-bitchat/               # nsite app: chat over Nostr events
│   ├── src/                    # React + TypeScript source
│   ├── dist/                   # Built nsite (index.html + assets)
│   └── package.json
│
├── myco-ics/                   # nsite app: calendar/schedule
│   ├── src/                    # React + TypeScript
│   ├── dist/                   # Built nsite
│   └── package.json
│
├── fips-exitnode/              # Reference: FIPS exit node (server-side mesh gateway)
│   ├── src/
│   │   └── main.rs
│   └── Cargo.toml
│
├── reference/                  # Upstream references (not modified by this repo)
│   ├── fips/                   # Upstream FIPS crate (used via path dependency)
│   ├── fips-android/           # Reference Android integration
│   ├── nostr-vpn/              # Upstream nostr-vpn (original project)
│   ├── bitchat-android/        # Reference Android app
│   └── Numo/                   # Reference no-VPN app
│
├── docs/                       # Design docs, roadmap, architecture
│   ├── design/
│   │   ├── architecture.md     # System layering, crate responsibilities
│   │   ├── concepts.md         # Glossary (npub, node_addr, fd00::, etc.)
│   │   ├── nsite-layer.md      # Nsite host, gateway, sync, propagation
│   │   ├── identity-pairing.md # Device identity, pairing handshake
│   │   ├── event-gossip.md     # Gossip protocol, mesh event propagation
│   │   ├── security.md         # Threat model, mitigations
│   │   ├── wifi-aware-interop.md # Android WiFi Aware bulk lane
│   │   ├── ble-interop.md      # BLE L2CAP interop
│   │   ├── nsite-updates.md    # Update check, staged install
│   │   └── diagrams/           # System diagrams (SVG)
│   ├── reference/
│   │   ├── ffi-surface.md      # JNI contract: action/state shapes
│   │   └── ports.md            # Port assignments (:4870, :24243, :80, etc.)
│   ├── how-to/
│   │   └── build.md            # Build instructions
│   └── roadmap.md              # Phase breakdown (P0–P5)
│
├── .claude/                    # Claude Code config
│   └── settings.json
│
├── .planning/                  # Planning outputs (this mapper writes here)
│   └── codebase/
│       ├── ARCHITECTURE.md     # This file
│       └── STRUCTURE.md        # Directory layout, naming, placement guide
│
├── .github/                    # GitHub CI/CD
│   └── workflows/              # GitHub Actions
│
├── android/build.gradle.kts    # Android app build config
├── Cargo.toml                  # Rust workspace root
├── Cargo.lock                  # Dependency lock
├── justfile                    # Just recipes (test, build, fmt)
├── README.md                   # Project overview
├── CHANGELOG.md                # Release notes
├── CONTRIBUTING.md             # Contribution guidelines
├── LICENSE                     # MIT
└── zapstore.yaml               # Zap store config (P2P distribution)
```

## Directory Purposes

**android/**
- Purpose: Kotlin/Jetpack Compose Android app (UI, VpnService, WebView)
- Contains: App UI (Library, Pair, Discover, Settings), per-nsite WebView activity, JNI call site, VpnService/TUN bridge
- Key files: `MainActivity.kt`, `NsiteActivity.kt`, `TunBridge.kt`, `BleIO.kt`
- Generated: `build/` (APK output)
- Committed: Source only (build/ gitignored)

**myco-core/**
- Purpose: App crate gluing nsite-deck + relay + Blossom to FIPS; FFI entry point
- Contains: `AppRuntime` (identity + Tokio runtime + FIPS node), Redux reducer, JNI bridges, content orchestration
- Key files: `runtime.rs`, `content.rs`, `peer_relay.rs`, `jni_abi.rs`
- Tests: Unit tests in each file (`#[cfg(test)] mod tests`); host-only (no JNI)

**nsite-deck/**
- Purpose: Reusable nsite host (gateway + sync) independent of relay/blob/radio implementation
- Contains: HTTP gateway, manifest resolver, sync engine, trait seams for abstraction
- Key files: `gateway.rs`, `sync.rs`, `seams.rs`, `model.rs`
- Tests: Integration tests in `lib.rs` (import_then_serve, incomplete_site, hash_mismatch)

**myco-relay/**
- Purpose: Embedded NIP-01 Nostr relay (event store + WebSocket server)
- Contains: Event store (in-memory, replaceable semantics), WebSocket server, subscription routing
- Key files: `store.rs`, `server/hub.rs`, `server/connection.rs`
- Tests: Unit tests for store (dedup, kind filtering)

**myco-blossom/**
- Purpose: Embedded Blossom blob store (SHA256-addressed HTTP server)
- Contains: Filesystem blob store, HTTP GET/PUT/HEAD handlers, access control (gate)
- Key files: `store.rs`, `server/handler.rs`
- Tests: Unit tests for put/get/has

**myco-bitchat/, myco-ics/**
- Purpose: Example nsites built with React + TypeScript over Nostr events
- Contains: Web app source (React components), build output (dist/)
- Committed: Source only; dist/ regenerated on build
- Notes: Not part of core library; shipped separately as bundled nsites

**fips-exitnode/**
- Purpose: Reference server-side mesh gateway (exit node for .fips proxy)
- Contains: Standalone server; can route traffic for non-peer exit mode
- Status: Reference implementation; not deployed in P1 (single-device focus)

**reference/**
- Purpose: Upstream dependencies and reference implementations
- Contains: `fips/` (FIPS mesh crate), `nostr-vpn/` (original project), example Android apps
- Committed: Git submodules (not squashed into this repo)
- Used by: Cargo workspace resolves `fips` via path dependency

**docs/**
- Purpose: System design, architecture, roadmap
- Key files: `architecture.md`, `concepts.md`, `nsite-layer.md`, `roadmap.md`
- Diagrams: `diagrams/` (SVG system layering, pairing, propagation flows)

## Key File Locations

**Entry Points:**
- `android/app/src/main/kotlin/.../MainActivity.kt` — Kotlin app entry, JNI call site (not in this repo)
- `myco-core/src/jni_abi.rs` — JNI interface: `dispatch(actionJson) → stateJson`
- `myco-core/src/runtime.rs:AppRuntime::dispatch` — Redux action processor

**Configuration:**
- `Cargo.toml` — Workspace members (myco-core, nsite-deck, myco-relay, myco-blossom), shared dependencies
- `android/build.gradle.kts` — Android app config, native library linking
- `docs/reference/ffi-surface.md` — FFI contract (action/state JSON shapes)

**Core Logic:**
- `myco-core/src/runtime.rs` — `AppRuntime`, Tokio runtime, FIPS node lifecycle
- `myco-core/src/content.rs` — Content orchestrator (Library, Circle, nsite sync, pairing)
- `myco-core/src/peer_relay.rs` — Per-peer relay subscription pool
- `nsite-deck/src/gateway.rs` — HTTP gateway (manifest → serve)
- `nsite-deck/src/sync.rs` — Sync engine (fetch + verify + store)
- `myco-relay/src/store.rs` — Event store
- `myco-blossom/src/store.rs` — Blob store

**Testing:**
- `myco-core/src/*.rs` — Unit tests (host build; no JNI)
- `nsite-deck/src/lib.rs` — Integration tests (full import + serve)
- `myco-relay/src/store.rs` — Store tests (dedup, filtering)

## Naming Conventions

**Files:**
- Rust: `snake_case.rs` (e.g., `peer_relay.rs`, `jni_abi.rs`)
- Kotlin: `PascalCase.kt` (e.g., `MainActivity.kt`, `NsiteActivity.kt`)
- TypeScript: `camelCase.ts` / `PascalCase.tsx` (React components)
- Cargo: `Cargo.toml`, `Cargo.lock`

**Directories:**
- Rust crates: `kebab-case` (e.g., `myco-core`, `myco-relay`, `nsite-deck`)
- Rust modules: `snake_case/` (e.g., `src/server/`, `src/model/`)
- Android packages: `com.example.` prefix (e.g., `com.myco.app`)
- Assets: `camelCase` (e.g., `drawables/`, `values/`)

**Types:**
- Rust structs: `PascalCase` (e.g., `AppRuntime`, `AppState`, `RelayStore`)
- Rust traits: `PascalCase` (e.g., `RelayBackend`, `BlobStore`)
- Rust enums: `PascalCase` (e.g., `NativeAppAction`, `Readiness`)
- Kotlin classes: `PascalCase` (e.g., `MainActivity`, `NsiteActivity`)

**Functions:**
- Rust: `snake_case` (e.g., `sync_site`, `store_event`, `open_site`)
- Kotlin: `camelCase` (e.g., `dispatchAction`, `updateState`)

## Where to Add New Code

**New nsite Sync Feature (e.g., "download progress")**
- Primary code: `nsite-deck/src/sync.rs` (sync state machine)
- State exposure: `myco-core/src/content.rs::SiteStatusView` (add field)
- UI: `android/app/src/main/kotlin/.../SiteStatusCard.kt` (render new field)
- Tests: `nsite-deck/src/lib.rs` (new test case in tests module)

**New Relay Query (e.g., "filter by author")**
- Backend: `myco-relay/src/store.rs::RelayStore` (add query method)
- Consumer: `myco-core/src/content.rs` or `gossip.rs` (call query)
- Tests: `myco-relay/src/store.rs::tests` (unit test query)

**New JNI Action (e.g., "SetDeviceColor")**
- Action definition: `myco-core/src/action.rs::NativeAppAction` (add variant)
- Handler: `myco-core/src/runtime.rs::dispatch` (match arm)
- State field: `myco-core/src/state.rs::AppState` (add field if needed)
- Kotlin dispatch: `android/app/src/main/kotlin/.../ActionDispatcher.kt` (marshal to JSON)

**New Transport (e.g., "add WiFi Direct support")**
- Bridge: `myco-core/src/aware_bridge_jni.rs` (model after existing bridges)
- FIPS config: `myco-core/src/runtime.rs::build_node` (enable transport)
- State: `myco-core/src/state.rs` (add transport status)
- Lifecycle: `runtime.rs::StartNode / StopNode` (manage transport lifecycle)

**New nsite App (e.g., "photo gallery")**
- Language: TypeScript + React (use myco-bitchat/myco-ics as templates)
- Build: Create `myco-photos/` with `package.json`, `src/`, `vite.config.ts`
- Output: Bundle to `dist/index.html` + assets
- Distribution: Add manifest event to relay (kind 15128), push to public Blossom
- Installation: User pastes link or discovers via Circle peer relay

**Utilities / Helpers**
- General helpers: `nsite-deck/src/model.rs` (shared logic)
- Nostr-specific: `myco-core/src/content.rs` (event creation, signing)
- FIPS-specific: `myco-core/src/ip_source.rs` (mesh address parsing)
- JNI glue: `myco-core/src/jni_abi.rs` (string marshalling)

## Special Directories

**target/**
- Purpose: Cargo build artifacts (binaries, dependencies, intermediate objects)
- Generated: Yes (via `cargo build`)
- Committed: No (.gitignore)
- Clean: `cargo clean` or `rm -rf target/`

**.git/**
- Purpose: Git repository metadata
- Generated: On `git init` / clone
- Committed: N/A (is the VCS itself)

**.planning/codebase/**
- Purpose: Codebase analysis documents (written by gsd-map-codebase)
- Generated: Yes (by this mapper)
- Committed: Yes (checked in so it's available in future sessions)
- Contents: ARCHITECTURE.md, STRUCTURE.md, CONVENTIONS.md, TESTING.md, CONCERNS.md (as applicable)

**docs/design/diagrams/**
- Purpose: SVG system diagrams (system layering, pairing flow, propagation)
- Format: Manually maintained SVGs (not generated)
- Committed: Yes (diagrams are reference material)

**reference/fips/**
- Purpose: Upstream FIPS mesh crate (submodule or path dependency)
- Status: External; not modified in this repo
- Build: Pulled via Cargo workspace resolution

**android/.gradle/, .kotlin/**
- Purpose: Android Studio/Gradle caches
- Generated: Yes (by Android toolchain)
- Committed: No (.gitignore)
- Safe to delete: Yes

---

*Structure analysis: 2026-08-01*
