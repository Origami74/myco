# Coding Conventions

**Analysis Date:** 2026-08-01

## Naming Patterns

**Files:**
- Rust: snake_case (e.g., `runtime.rs`, `dns_intercept.rs`, `identity_store.rs`)
- TypeScript/React: camelCase for utilities (e.g., `nostr.ts`, `debug.ts`), PascalCase for components (e.g., `App.tsx`)
- Modules: Plural for collections (e.g., `ble_peers`), singular descriptive names for subsystems

**Functions:**
- Rust: snake_case universally (e.g., `load_or_generate()`, `temp_dir()`, `make_query()`)
- TypeScript: camelCase for regular functions, PascalCase for React components (e.g., `<Chat>`, `<NickModal>`)
- Prefix pattern: Test helpers often use descriptive verbs (e.g., `temp_dir()`, `make_query()`, `host_for()`)

**Variables:**
- Rust: snake_case for locals, constants (e.g., `TTL_SECONDS`, `PRESENCE_KIND`, `data_dir`)
- TypeScript: camelCase for all locals and state (e.g., `nick`, `debugOpen`, `renaming`)
- Constants: UPPER_SNAKE_CASE (e.g., `RELAY`, `CHAT_KIND`, `HEARTBEAT_MS`, `SK_KEY`)

**Types:**
- Rust: PascalCase for structs, enums, traits (e.g., `AppRuntime`, `NativeAppAction`, `NodeStatus`)
- TypeScript: PascalCase for interfaces/types (e.g., `NostrEvent`, `Filter`)
- Lifetimes: Implicit unless necessary for clarity in Rust

## Code Style

**Formatting:**
- Tool: `cargo fmt` (Rust) — enforced in CI with `cargo fmt --all --check`
- Tool: TypeScript via Vite + tsc (TS) — no formatter configured
- Line width: Default Rustfmt (~100), no explicit limit documented
- Trailing commas: Allowed in Rust (idiomatic)

**Linting:**
- Rust: `cargo clippy --all-targets -- -D warnings` (enforced in CI as non-gating)
  - Pre-existing clippy warnings in the workspace are not blocking
  - New code must not introduce clippy warnings
  - Clippy issues are fixed locally before pushing
- TypeScript: Strict mode enabled via `tsconfig.json`:
  - `"strict": true` — all strict checks active
  - `"noUnusedLocals": true` — unused variables error
  - `"noUnusedParameters": true` — unused parameters error
  - `"noFallthroughCasesInSwitch": true` — switch cases must terminate
- Type checking: `tsc -b --noEmit` via npm run typecheck

## Import Organization

**Order (Rust):**
1. Standard library (`use std::...`)
2. External crates (alphabetical)
3. Internal modules/crates (relative paths)
4. Conditional imports via `#[cfg(...)]`

Example from `myco-core/src/runtime.rs`:
```rust
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fips::control::read_handle::ControlReadHandle;
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

use crate::action::NativeAppAction;
use crate::content::{CacheView, Content};
```

**Order (TypeScript):**
1. Standard library imports
2. External packages (npm)
3. Local relative imports
4. Type imports separated with `import type`

Example from `myco-bitchat/src/main.tsx`:
```typescript
import "./debug"; // Side effects first
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { EventStoreProvider } from "applesauce-react/providers";
import { eventStore } from "./nostr";
import { App } from "./App";
import "./styles.css";
```

**Path Aliases:**
- Rust: None documented (uses `use crate::` pattern)
- TypeScript: None configured (uses relative paths)

## Error Handling

**Rust Patterns:**
- Primary: `anyhow::Result<T>` for fallible operations
  - Propagates errors via `?` operator
  - Used for I/O, deserialization, async operations
- Custom errors: `thiserror` crate for domain-specific error types
- Fallback: `anyhow::anyhow!("message")` for one-off error construction

Example from `identity_store.rs`:
```rust
pub fn load_or_generate(data_dir: &Path) -> anyhow::Result<String> {
    let path = data_dir.join(KEY_FILE);
    if path.exists() {
        let nsec = std::fs::read_to_string(&path)?.trim().to_string();
        if !nsec.is_empty() {
            return Ok(nsec);
        }
    }
    let id = fips::Identity::generate();
    let nsec = fips::encode_nsec(&id.keypair().secret_key());
    std::fs::write(&path, &nsec)?;
    Ok(nsec)
}
```

**Error Recovery (AppRuntime):**
- Errors captured in constructor (via `try_new()`) and stored in state
- State field `error: String` serialized to UI as JSON
- Constructor never panics — failures are converted to error state
- Pattern: `pub fn new() -> Self { match Self::try_new() { Ok(rt) => rt, Err(e) => Self::from_error(msg) } }`

**TypeScript Error Handling:**
- Errors propagated in try/catch for async operations
- No formal Result type in use (standard TS/JS patterns)
- Observable errors in RxJS chains via `.pipe(retry(...), catchError(...))`

## Comments

**When to Comment:**
- Design rationale: Explain *why* in non-obvious areas (threading, state machines, protocol details)
- Crate-level documentation: `//!` module docs required for public crates
- Struct/enum docs: Every public type documented with `///`
- Enum variant docs: Every variant explained (see `NativeAppAction` — 20+ variants fully documented)
- Inline logic: Comments added only when the code alone doesn't explain the intent

**JSDoc/Doc-comment Style:**
- Rust: Triple-slash `///` for documentation, `//!` for module-level
- Markdown in comments: Supported (backticks for code, links via `[text](path)`)
- No @param/@return tags — docs are prose + examples

Example from `runtime.rs`:
```rust
/// UDP port for the Wi-Fi Aware bulk lane. Both peers bind it on the NDP
/// interface and exchange over it — symmetric, no listener/dialer roles. A
/// fixed app constant (we bind our own port), so there is no PSM-style
/// discovery problem. UDP is fips's native transport and the LAN-discovery
/// path (which this reuses) is already UDP + scoped link-local IPv6.
/// See docs/design/wifi-aware-interop.md.
const WIFI_AWARE_PORT: u16 = 4871;
```

## Function Design

**Size:**
- Rust: Varies; large functions documented when they handle multiple concerns
  - Test functions: 5–30 lines each
  - Utility/transform functions: 10–50 lines
  - Main reducer: `dispatch()` method delegates to smaller helpers
- TypeScript: React components kept small; hook composition preferred
  - Component function: 50–100 lines including JSX
  - Sub-components extracted for reusability (e.g., `NickModal`, `DebugPanel`)

**Parameters:**
- Rust: Owned types or borrows as needed (no artificial lifetime constraints)
- Type hints: Always explicit; Rust inference used but not relied upon for clarity
- TypeScript: Typed function parameters with interface for multi-param callbacks
  ```typescript
  function NickModal({
    nick,
    onSave,
    onClose,
  }: {
    nick: string;
    onSave: (name: string) => void;
    onClose: () => void;
  }) { }
  ```

**Return Values:**
- Rust: `Result<T>` for fallible, `Option<T>` for nullable
- TypeScript: Explicit types (not inferred from usage) for exported functions
- Async: Rust uses `.await` on futures; TypeScript uses async/await syntax

## Module Design

**Exports (Rust):**
- Pattern: `pub use` re-exports at crate root for public API
- Internal modules marked `mod` without `pub`; only intended-public items re-exported
- Examples from `nsite-deck/src/lib.rs`:
  ```rust
  pub use gateway::{serve, GatewayResponse, Readiness};
  pub use host::{parse_link, resolve_host, SiteAddr};
  pub use model::{kind_for, site_key, Manifest, KIND_NAMED, KIND_ROOT};
  ```

**Feature Flags (Rust):**
- `#[cfg(feature = "testing")]` for test-only code (e.g., `nsite-deck/src/testing.rs`)
- Modules conditionally compiled: `#[cfg_attr(not(target_os = "android"), allow(dead_code))]` for Android-only code visible to host build
- Dev dependencies used for test fixtures

**Barrel Files:**
- TypeScript: Not used; imports are direct from source files
- Rust: `pub use` patterns in `lib.rs` create a logical barrel

**Conditional Compilation (Rust):**
- Platform gates: `#[cfg(target_os = "android")]` for JNI glue, BLE bridge
- Host build: Cross-platform debugging possible (JNI modules compile but are not used)
- Testing feature: Optional crate-private test utilities (e.g., `MemRelay`, `MemBlobs`)

## Traits and Abstractions

**Trait Definition (Rust):**
- Location: `nsite-deck/src/seams.rs` defines `RelayBackend`, `BlobStore`, `PeerSource`, `FanoutSink`
- Async traits: `async-trait` macro used for methods returning futures
- Implementation: In-memory mocks in `testing.rs` for unit tests; real impls in consumer crates

## Dependencies

**Workspace Dependencies:**
- All crates in the workspace pin versions at workspace level (`Cargo.toml` [workspace.dependencies])
- Path dependencies: `fips` (local checkout at `reference/fips`), internal crates
- Async runtime: Tokio multi-threaded, configured at workspace level

## Android UI (Compose)

<!-- Added 2026-08-04 after the v0.4.2 dark-mode merge (#23). The rest of this
     document predates it and covers Rust only. -->

The app follows the Android system theme. `MycoTheme` picks between
`MycoLightColors` and `MycoAmoledColors` via `isSystemInDarkTheme()`, both defined
in `android/app/src/main/java/app/myco/ui/theme/Theme.kt`.

**New UI must not hardcode colours.** Read them from `MaterialTheme.colorScheme`
semantic roles (`surface`, `onSurface`, `surfaceVariant`, `outline`, `primary`,
`tertiary` for warnings) so both themes render correctly. The palette is fixed —
no Material You / dynamic colour, which would need API 31+ while Myco targets
API 29+.

**Peer-state colours are already defined and are theme-independent** — reuse them
rather than inventing new ones:

| Constant | Meaning |
|---|---|
| `StatusConnected` | Live mesh connection |
| `StatusReachable` | Reachable, not directly connected |
| `StatusThin` | Exactly one peer — works, but no redundancy |
| `StatusAlone` | No peers; a real fault state |

Three deliberate exceptions to theme-following: the QR card stays white in both
themes (scanners read dark-on-light far more reliably), pending/warning states
keep a distinct amber rather than collapsing into the theme's accent, and the
first-run intro (`ui/intro/`) draws its mark in fixed cyan on near-black. The
intro runs before any app chrome and owns the whole screen, so it has nothing
to sit against and no light-mode variant to render — those are the logo's own
colours rather than a colour choice, and they live as the `Mark*` constants in
`Theme.kt`, next to the peer-state ones.

`ThemeTest.kt` asserts palette invariants and runs in CI, so a new screen that
reaches past the colour scheme will be caught there.

## Build environment

- **`reference/fips` must be checked out on `integration/platform`.** It is a
  gitignored path dependency. That branch sits on current fips master and adds
  the mobile seams myco-core needs: Android BLE behind the existing `BleIo`
  backend trait, per-instance transport addressing (the LAN and Wi-Fi Aware
  lanes take one UDP socket each), and the app-owned TUN/UDP/radio seams.
  `ci.yml` and `release.yml` clone the same ref from `jmcorgan/fips`.

  The old `feat/platform-peer-queue` branch is gone from the build. What
  myco-core used to reach for there — `fips::discovery::platform`, a public
  `ControlReadHandle`, `ble::attempts`, `ble::android_io` — no longer exists in
  any form. Peer state and peer commands now go over the node's own control
  socket, and each commit on `integration/platform` is shaped to be
  cherry-picked upstream on its own rather than living as a fork.

---

*Convention analysis: 2026-08-01; Android UI and build-environment sections added 2026-08-04*
