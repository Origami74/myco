# Testing Patterns

**Analysis Date:** 2026-08-01

## Test Framework

**Rust:**
- Runner: `cargo test` (built-in test harness)
- Async testing: `#[tokio::test]` macro for tests requiring Tokio runtime
- Config: No separate test config file; tests live in-module via `#[cfg(test)]` blocks

**TypeScript:**
- Framework: None configured (only TypeScript compiler for type checking)
- Test runner: Not in use; type checking via `tsc -b --noEmit` is primary validation
- Build tool: Vite for development/building

**Run Commands:**

Rust:
```bash
cargo test              # Run all workspace tests
cargo test --lib       # Library tests only
cargo test --test *    # Integration tests only
cargo build --tests    # Compile tests without running
```

TypeScript:
```bash
npm run typecheck      # Type check via tsc (tsconfig.json strict mode)
npm run build          # Build production bundle
npm run dev            # Development mode (no automated tests run)
```

**CI/CD:**
- `.github/workflows/ci.yml` — Rust tests run on every push/PR:
  ```bash
  cargo fmt --all --check   # Format gate (fails build if drift)
  cargo clippy --workspace --all-targets  # Linting (non-gating)
  cargo test --workspace    # Unit + integration tests (gating)
  ```

## Test File Organization

**Rust:**

**Location:** Inline in source files via `#[cfg(test)]` modules
- Tests live in the same file as the code they test, not in separate `tests/` directory
- Example: `myco-core/src/runtime.rs` contains `#[cfg(test)] mod tests { }`

**Naming:**
- Test modules: `mod tests { }` (standard Rust pattern)
- Test functions: Descriptive snake_case (e.g., `identity_generates_persists_and_is_stable`, `reducer_rev_and_bad_action`)
- Fixtures: `temp_dir(tag)`, `make_query(qname)`, `host_for(author)` as helper functions within the test module

**Structure:**
```
myco-core/src/
├── runtime.rs
│   └── #[cfg(test)] mod tests
│       ├── fn temp_dir(tag) -> PathBuf
│       ├── #[test] fn identity_generates_persists_and_is_stable()
│       └── #[test] fn reducer_rev_and_bad_action()
├── dns_intercept.rs
│   └── #[cfg(test)] mod tests
│       ├── fn make_query(qname, dst) -> Vec<u8>
│       └── #[test] fn intercept_matches_query()
└── [other modules with inline tests]
```

**TypeScript:**

**Location:** No formal test files (no Jest/Vitest config)
- Validation is compile-time via TypeScript strict mode
- Runtime testing done manually or via browser dev tools

## Test Structure

**Rust Unit Test Pattern:**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Helper functions defined first
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("myco-test-{}-{}", std::process::id(), tag))
    }

    // Actual tests
    #[test]
    fn identity_generates_persists_and_is_stable() {
        let dir = temp_dir("identity");
        let _ = std::fs::remove_dir_all(&dir);

        let first = AppRuntime::new(dir.to_str().unwrap(), "0.0.1");
        let s1 = first.state();
        
        // Assertions
        assert!(s1.error.is_empty(), "startup error: {}", s1.error);
        assert!(s1.identity.own_npub.starts_with("npub1"), "npub: {}", s1.identity.own_npub);
        
        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

**Patterns:**

1. **Setup:** Helper functions generate test data (temp dirs, query packets, test sites)
2. **Execution:** Direct function/method calls; no mocking framework
3. **Assertions:** Rust's built-in `assert!`, `assert_eq!` macros with custom messages
4. **Teardown:** Manual cleanup (file deletion) at test end
5. **Async:** `#[tokio::test]` for tests using `.await`:
   ```rust
   #[tokio::test]
   async fn import_then_serve_root_site() {
       let relay = MemRelay::new();
       let blobs = MemBlobs::new();
       let outcome = import_site(&relay, &blobs, site.manifest, &site.blobs).await.unwrap();
       assert_eq!(outcome, SyncOutcome::Ready);
   }
   ```

## Mocking

**Framework:** No formal mocking library (e.g., no `mockito`, `mock_trait`, etc.)

**Patterns:**

1. **In-Memory Implementations:** Trait implementors that store state in-memory
   - `MemRelay` — in-memory Nostr relay backed by a HashMap
   - `MemBlobs` — in-memory Blossom blob store
   - Located in `nsite-deck/src/testing.rs` (behind `#[cfg(feature = "testing")]`)

2. **Example from `nsite-deck/src/lib.rs` tests:**
   ```rust
   #[cfg(test)]
   mod tests {
       use super::testing::{build_test_site, MemBlobs, MemRelay};

       #[tokio::test]
       async fn import_then_serve_root_site() {
           let relay = MemRelay::new();
           let blobs = MemBlobs::new();
           
           let site = build_test_site(
               &[("/index.html", b"<h1>hello</h1>")],
               None,
               Some("Test Site"),
           );
           
           let outcome = import_site(&relay, &blobs, site.manifest, &site.blobs).await.unwrap();
           assert_eq!(outcome, SyncOutcome::Ready);
       }
   }
   ```

3. **Trait Seams:** Tests use trait objects (`RelayBackend`, `BlobStore`) to inject test implementations

**What to Mock:**
- Persistence (file I/O) — use temp dirs and actual file operations (not mocked)
- Relay queries — `MemRelay` in-memory store
- Blob storage — `MemBlobs` in-memory store
- Async operations — `#[tokio::test]` handles concurrency

**What NOT to Mock:**
- Core cryptographic operations — tested directly
- Serialization (`serde`) — actual serialization/deserialization tested
- Protocol parsing (DNS packets, Nostr events) — actual parser tested with crafted payloads

## Fixtures and Factories

**Test Data Builders:**

1. **`temp_dir(tag)`** — Creates isolated temp directory for each test
   - Used by AppRuntime tests to avoid interference
   - Cleaned up at test end: `let _ = std::fs::remove_dir_all(&dir)`

2. **`make_query(qname, dst)`** — Builds DNS query packets
   - Uses `simple-dns` crate (dev dependency)
   - Crafts raw IPv6/UDP/DNS packets for interception tests

3. **`build_test_site(paths, blobs, name)`** — Generates a signed Nostr manifest + blobs
   - Behind `#[cfg(feature = "testing")]` in `nsite-deck/src/testing.rs`
   - Returns `TestSite` with `.manifest`, `.blobs`, and `.author` fields

4. **`host_for(author)`** — Derives nsite host from public key
   - Helper function showing how host resolution works

**Location:** `nsite-deck/src/testing.rs` (public module for in-tree tests)

**Accessibility:**
- Private dev dependencies in `Cargo.toml` (`simple-dns`) for package-internal tests
- Shared fixtures exported via `#[cfg(feature = "testing")]` feature flag (opt-in)
- Tests that need fixtures must activate the feature: `nsite-deck = { path = ".", features = ["testing"] }`

## Coverage

**Requirements:** Not enforced
- No coverage targets, thresholds, or tools configured (e.g., tarpaulin, llvm-cov)
- Guidance from `CONTRIBUTING.md`: "Add `cargo test` coverage for new core logic" (best-effort)

**Scope:**
- "Behavior that can only be exercised on-device" (pairing, BLE/NFC, WebView) tested manually on physical devices
- "Host `cargo test` run is necessary but not sufficient" for mesh/pairing changes

**View Coverage:** Not available (no coverage tool integration)

## Test Types

**Unit Tests:**

**Scope:** Individual modules, isolated from I/O and network
- `runtime.rs`: Tests for reducer (state machine), identity persistence
- `dns_intercept.rs`: Tests for DNS query parsing and rewriting
- `gossip.rs`: Tests for message signing, relay operations
- `peer_relay.rs`: Tests for relay subscription logic

**Approach:**
- Test one function/method per test
- Use helpers (fixtures) to reduce boilerplate
- Use `unwrap()` on expected successes (test panics indicate failure)

**Integration Tests:**

**Scope:** Multi-module flows, especially sync and content serving
- `nsite-deck/src/lib.rs`: Tests for manifest import → site serving → content verification
- Path: Import a signed manifest + blobs → resolve hostname → serve over gateway → verify responses
- Used test implementations: `MemRelay`, `MemBlobs`

**Approach:**
```rust
#[tokio::test]
async fn import_then_serve_root_site() {
    // Setup: in-memory impls
    let relay = MemRelay::new();
    let blobs = MemBlobs::new();
    
    // Execute: full flow
    let outcome = import_site(&relay, &blobs, site.manifest, &site.blobs).await.unwrap();
    
    // Verify: each step
    assert_eq!(outcome, SyncOutcome::Ready);
    let resp = serve(&relay, &blobs, &host, "/", None).await;
    assert_eq!(resp.status, 200);
}
```

**E2E Tests:**

**Not automated in CI/CD.**
- On-device testing: Pairing, BLE/NFC, WebView integration (Android app)
- Manual testing documented in PR descriptions
- Device requirements: "Two paired phones" for pairing-related changes

## Common Patterns

**Async Testing:**

```rust
#[tokio::test]
async fn test_name() {
    // Setup
    let relay = MemRelay::new();
    
    // Execute with .await
    let result = some_async_operation(&relay).await;
    
    // Assert
    assert!(result.is_ok());
}
```

**Error Testing:**

```rust
#[test]
fn reducer_bad_action() {
    let mut rt = AppRuntime::new(dir, "0.0.1");
    let json = rt.dispatch_json("not json");
    assert!(json.contains("invalid action JSON"));
}
```

**Cleanup Pattern:**

```rust
#[test]
fn test_with_temp_state() {
    let dir = temp_dir("test-tag");
    let _ = std::fs::remove_dir_all(&dir);  // Pre-cleanup in case of crash
    
    // Test code here
    
    let _ = std::fs::remove_dir_all(&dir);  // Post-cleanup
}
```

**Setup Pattern:**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        // Shared across all tests in this module
        std::env::temp_dir().join(format!("myco-test-{}-{}", std::process::id(), tag))
    }

    #[test]
    fn test1() { /* uses temp_dir */ }
    
    #[test]
    fn test2() { /* uses temp_dir */ }
}
```

## Test Dependencies

**Rust:**
- `tokio` with `"rt", "macros"` features (for `#[tokio::test]`)
- `simple-dns "0.11.2"` — for DNS packet construction in `myco-core` tests
- In-memory trait implementations in `nsite-deck/src/testing.rs`

**TypeScript:**
- No test framework dependencies (no Jest, Vitest, Mocha configured)
- Type validation only via TypeScript compiler

## Pre-Submit Checklist

From `CONTRIBUTING.md`, before opening a PR:

**Rust:**
```bash
cargo fmt --check           # No formatting drift
cargo build                 # Compiles
cargo clippy --all-targets  # No clippy warnings on new code
cargo test                  # All tests pass
```

**TypeScript:**
```bash
npm run typecheck          # No type errors
npm run build              # Build succeeds
```

---

*Testing analysis: 2026-08-01*
