# Codebase Concerns

**Analysis Date:** 2026-08-01

## Tech Debt

**High unwrap() density in error paths:**
- Files: `myco-core/src/content.rs` (114), `myco-relay/src/server.rs` (36), `myco-core/src/ip_source.rs` (34), `myco-relay/src/lib.rs` (31), `myco-core/src/runtime.rs` (31)
- Impact: 273 total unwrap/panic/expect calls that can panic at runtime. While most are on mutex locks (rarely poisoned) or hardcoded values, they remain panic vectors that should be error-handled gracefully.
- Fix approach: Replace critical path unwraps with proper error handling; add a clippy lint to discourage new ones. Test mutex poisoning scenarios.

**Hardcoded socket address parsing panics:**
- Files: `myco-core/src/runtime.rs:153`, `myco-core/src/runtime.rs:170`, `myco-core/src/runtime.rs:194`
- Issue: Socket addresses `[::]:4870`, `127.0.0.1:4870`, `[::]:24243` are hardcoded and parsed with `.unwrap()` — they cannot fail, but the pattern invites error. If these were later made dynamic, the unwrap becomes a real bug vector.
- Fix approach: Use `const` socket addresses or use `.expect()` with explanatory messages. Better: pass addresses as configuration.

**Large monolithic file:**
- File: `myco-core/src/content.rs` (2,508 lines)
- Issue: The content layer (relay + Blossom + gateway + Library + Circle + pairing + discovery) is all in one file, making it hard to navigate and test. Complex abstractions (PendingUpdate, ActiveBackend, multiple state machines) are interleaved.
- Fix approach: Split into sub-modules (`content/library.rs`, `content/circle.rs`, `content/sync.rs`, `content/pairing.rs`) organized by concern. Extract CircleGate and related traits into separate files. Create a facade module.

**Multiple mutex fields create deadlock potential:**
- Files: `myco-core/src/content.rs` (14 distinct Mutex fields on the Content struct)
- Issue: Lock ordering is documented informally ("snapshot X before taking Y") in comments, but there is no automatic enforcement. A future refactor could accidentally reverse lock order.
- Fix approach: Document lock hierarchy explicitly in struct comments with ASCII diagram. Add integration tests that exercise all multi-mutex paths. Consider using a single outer Mutex or a structured lock guard with guaranteed ordering.

**Fire-and-forget mesh fan-out may lose messages:**
- Files: `myco-core/src/peer_relay.rs` (backoff strategy), `myco-core/src/gossip.rs`
- Issue: The mesh push plane (outgoing events) is fire-and-forget via `try_send()` with exponential backoff. If a peer relay's queue fills, the message is silently dropped with no retry or notification to the user. Chat or app event delivery can fail silently.
- Fix approach: Implement message buffering with retry on backoff reset (when peer reconnects). Log dropped events at warn level. Consider a fallback relay for critical messages (pairing, unpairing). Track delivery metrics.

## Known Bugs

**DNS resolver selection non-deterministic on mesh names:**
- Files: `myco-core/src/dns_intercept.rs` (fixed in v0.4.1)
- Status: **FIXED** in v0.4.1 per CHANGELOG. The tunnel had listed real resolvers alongside Myco's own; now Myco's resolver answers all `.fips` lookups directly.

**Wi-Fi AP peering flapping:**
- Files: Reference nostr-vpn code (upstream FIPS issue)
- Status: **PARTIALLY FIXED** in v0.4.0/v0.4.1 per CHANGELOG. Address rotation and mDNS handling improved, but upstream FIPS#130 reports peering can still stall after Wi-Fi reconnect on MAC-rotating phones.
- Workaround: Toggle mesh off/off to force node refresh.

**Mutex lock contention on state polls:**
- Files: `myco-core/src/runtime.rs` (state() method), `myco-core/src/content.rs` (many lock points)
- Issue: The UI polls `state()` up to ~60x/second; each poll locks 10+ mutexes on the Content and Runtime structs to build the state snapshot. On a slow device or under heavy sync load, this can cause noticeable lag.
- Fix approach: Implement a read-write lock hierarchy or atomic snapshot swap so readers don't block writers. Move status polling to a lower frequency with a broadcast channel update on change.

## Security Considerations

**JNI unsafe blocks on Android:**
- Files: `myco-core/src/jni_abi.rs` (4 unsafe blocks), `myco-core/src/ble_bridge_jni.rs` (8 unsafe blocks)
- Current mitigation: Pointers are boxed and stored as `jlong`, with SAFETY comments documenting the invariant (pointer must be from `appNew`, not freed). Kotlin side is responsible for not reusing or double-freeing handles.
- Recommendations: Add debug assertions to detect double-frees (store a marker or gen-counter in the Box). Document the contract in a shared FFI specification file. Test on GrapheneOS with address space layout randomization.

**Pairing secrets are one-time but short-lived:**
- Files: `myco-core/src/content.rs` (KIND_PAIR_REQUEST/ACCEPT handling)
- Issue: The pair request carries a `secret` tag that prevents replay, but the secret is included in the JSON event **in plaintext over the mesh**. A compromised mesh peer or eavesdropper can capture an in-flight pair request and see the secret, though the NIP-40 expiration (120s) limits the window.
- Recommendations: Consider encrypting the secret using the target peer's public key (asymmetric) before including it in the event. Document the threat model for pairing on a potentially-compromised mesh. The current expiration is reasonable for local mesh use cases.

**Circle gate implementation assumes Circle is consistent:**
- Files: `myco-core/src/content.rs:CircleGate` (reads Circle on every request)
- Issue: The relay's PeerGate is consulted **per event** to check if the sender's IP is a paired Circle member. The Circle is mutable (add/remove peers), so a peer can be gates out mid-stream. Also, if the Circle file is corrupted, `load_circle` silently falls back to an empty Circle, locking everyone out.
- Fix approach: Add CRC or versioning to persisted Circle/library files. Log when the Circle changes and how many active subscriptions are affected. Consider a read-write lock so subscribers aren't interrupted mid-EOSE.

**Blob store has no quota enforcement:**
- Files: `myco-blossom/src/lib.rs` (embedded Blossom), `myco-core/src/content.rs`
- Issue: The embedded Blossom server accepts uploads up to 64 MiB per blob with no global quota. A malicious peer could spam the device until storage is full, causing the app to fail.
- Recommendations: Enforce a per-app or per-peer upload quota. Add disk space checks before accepting large blobs. Implement an LRU eviction for blobs older than a threshold (P5 feature per CHANGELOG). Log uploads by peer.

## Performance Bottlenecks

**Content sync spawns new tasks without backpressure:**
- Files: `myco-core/src/runtime.rs` (line ~774 spawns `open_site` per retriable Library item)
- Issue: On startup or after a mesh reconnect, `retriable_library_addrs()` can return dozens of nsites. The loop spawns one `open_site` task per address without batching or rate limiting. Each task opens mesh connections, fetches manifests, and pulls blobs concurrently, overwhelming a BLE link.
- Fix approach: Implement a bounded queue or semaphore limiting concurrent syncs to 2-4. Prioritize by recency (pinned > recently added). Add exponential backoff between retries per site.

**Mutex clones on hot paths:**
- Files: `myco-core/src/content.rs` (lines 563, 571 clone `peer_relays` Arc; lines 570, 919, 969, 980 clone Vec/String from mutex)
- Issue: The hot path for opening a site clones the peer relay pool Arc and circle contacts Vec multiple times. While cheap (Arc is ~8 bytes), unnecessary clones accumulate.
- Fix approach: Pass references where possible instead of cloning. Use Arc references directly rather than cloning the pool. Profile to measure impact on BLE latency.

**DNS intercept handles every outbound IPv6 query synchronously:**
- Files: `myco-core/src/dns_intercept.rs` (handle_query called from TUN send_packet)
- Issue: Every outbound DNS query is parsed and checked for `.fips` names in the TUN send path. Non-mesh queries are forwarded upstream, but the parsing is per-packet. On heavy traffic, this adds latency.
- Fix approach: Cache the last N queried domain names (time-windowed) to skip parsing. Use a faster DNS parsing library (currently simple_dns which is general-purpose). Move filtering to the tunnel level if possible.

## Fragile Areas

**Pair request/accept delivery is fire-and-forget:**
- Files: `myco-core/src/content.rs:dial_pair_event()` (retries up to 15x with 4s delays)
- Why fragile: A pair request succeeds locally (added to outbound_pairs, event built) but might never reach the peer if the mesh route flaps during the ~60s retry window. The peer never gets a notification, the user never gets feedback, and the outbound entry sits indefinitely unless the user manually cancels it.
- Safe modification: Wrap retry logic in a struct with timeout and max-attempts. Add a timeout after which the pair request is auto-cleared or surfaced as "delivery failed". Test by unplugging one phone mid-pairing.

**Manifest update staging without atomicity:**
- Files: `myco-core/src/content.rs` (PendingUpdate, `set_active()`)
- Why fragile: A newer manifest can sit staged while its blobs download (pending_updates). The active version is swapped atomically, but if the app crashes mid-download, the pending update is lost. If two updates arrive for the same slot, the newer one can be discarded if an older one is already syncing.
- Safe modification: Persist pending updates to disk (like active.json). Implement a lock per slot so only one sync can be in flight. Test app crash scenarios during staging.

**Circle resolution by npub → IP address mapping is DNS-dependent:**
- Files: `myco-core/src/ip_source.rs:mesh_relay_url()`, `myco-core/src/dns_intercept.rs`
- Why fragile: Opening a site from a Circle peer requires resolving `<npub>.fips` to the peer's mesh address. If DNS is temporarily broken (node is offline, resolver query times out), the peer is unreachable even though the mesh connection is up. Fallback to using the raw `fd00::` address doesn't work for anyone who isn't a direct neighbour.
- Safe modification: Cache recent DNS resolutions (5 min TTL). Implement a fallback that queries the peer's own relay for a signed identity record containing its mesh address. Test resolver outages.

**Test coverage for content layer is minimal:**
- Files: `myco-core/src/content.rs` (only 3 #[test] and 5 #[tokio::test] functions for 2,508 lines)
- Gaps: No tests for circle gate edge cases (member added/removed mid-stream), pairing expiration, manifest version upgrades, lock ordering, or concurrent access patterns. Discovery deduplication is tested but not the discovery resync on peer reconnect.
- Impact: Refactors to the Content struct risk silent breakage in production. Hard to validate changes.
- Priority: Add integration tests for: (1) concurrent library/circle updates, (2) pair request timeout, (3) manifest staging + crash recovery, (4) discovery dedup stability.

## Scaling Limits

**No per-app size limit:**
- Issue: An nsite with 10,000 resources can be pinned. The gateway serves it in-process. No pagination or streaming; the manifest is loaded entirely into memory.
- Limit: Effective limit is device storage (Android typically 5-50 GB total, shared with all apps). A pathological manifest with millions of tiny files could exhaust inode limits on the underlying filesystem.
- Scaling path: Implement lazy manifest loading (load TOC, stream resources on demand). Add a per-app size warning in the Library. Enforce a 500 MB per-app soft limit with user override.

**Blob store directory listing on every poll:**
- Issue: CHANGELOG v0.3.0 notes that state() polls now skip walking the blob cache directory, but the CacheView (relay_events, blob_count, used_bytes) still requires traversing the blob store or caching the counts.
- Limit: On a device with 100k+ small blobs, the traversal can stall briefly.
- Scaling path: Cache blob counts in a `.metadata` file, updated on add/delete. Fall back to a full scan on startup or if out of sync.

**No rate limiting on relay publishing:**
- Issue: Any Circle member can publish events to the embedded relay without limits. A peer could spam events or push huge multi-MB payloads per event.
- Limit: Relay fills up, old events are evicted, then newer spam events. The relay store may not have an eviction policy.
- Scaling path: Implement per-peer publish rate limits (events/sec, bytes/sec). Add max event size enforcement. Use the relay store's eviction policy if available; if not, implement LRU.

## Dependencies at Risk

**Nostr crate pinned to v0.44:**
- File: `Cargo.toml` (workspace dependencies)
- Risk: v0.44 is several versions behind the latest. Newer versions may have security fixes, performance improvements, or API breaking changes that require refactoring event handling.
- Migration plan: Review breaking changes in the v0.45+ changelog. Benchmark cryptographic operations (signature verification dominates in the relay). Update incrementally and test against a real Circle.

**Tokio multi-thread runtime without explicit thread count:**
- File: `myco-core/src/runtime.rs:83` (Runtime::new() uses defaults)
- Risk: Default Tokio thread pool is `num_cpus`, which on Android can vary (1-8). On a dual-core device, spawning dozens of sync tasks can lead to thread pool starvation and UI lag.
- Fix approach: Set a bounded thread pool size (4-8 worker threads) and use a separate channel or work queue for bursty tasks. Profile on low-end devices.

## Missing Critical Features

**No UI/user indication of sync failures:**
- Issue: If all mesh sources and the IP fallback fail to deliver a site, the user sees "Unreachable" but not why (peer offline? no internet? blob server down?). Fire-and-forget retry logic means the user has no way to force a retry.
- Blocks: Users can't debug why a site won't sync. Frustrates first-time users trying to add an app.

**No offboarding for corrupted Circle/Library files:**
- Issue: If circle.json or library.json is corrupted, `serde_json::from_slice` fails silently and the file is loaded as empty. The user's Circle and pinned apps disappear with no warning.
- Blocks: Data loss without recovery. Users lose trust in the app.
- Fix: Add a migration function that detects version/checksum mismatches, backs up the old file, and surfaces a warning. Implement file versioning.

**No durable pair request delivery:**
- Issue: Pair requests are sent fire-and-forget. If the peer is offline, the request is lost after the retry window (60s). The user has to manually re-initiate the pairing.
- Blocks: Pairing two phones that are offline initially requires multiple attempts. User experience is poor.
- Fix: Queue undelivered pair requests to disk. Replay them on mesh reconnect or after a timeout. Add a "Resend invite" button on the outbound_pairs list.

## Test Coverage Gaps

**Content layer synchronization:**
- What's not tested: Concurrent updates to library, circle, and sites from multiple threads. Lock ordering under contention. Mutex poisoning recovery.
- Files: `myco-core/src/content.rs`
- Risk: Deadlocks or data corruption under high concurrency (e.g., app receiving a new pairing while syncing multiple sites).
- Priority: HIGH — Add integration tests for multi-site sync + peer add/remove + manifest updates.

**JNI/FFI boundary conditions:**
- What's not tested: Null pointer handling (e.g., appNew fails, returning null; appDispose called with null). Double-free detection. Reentrant calls (e.g., reducer calling back into app).
- Files: `myco-core/src/jni_abi.rs`, `myco-core/src/ble_bridge_jni.rs`
- Risk: Crashes on malformed input or unexpected calling patterns.
- Priority: MEDIUM — Add host-side unit tests that simulate error conditions.

**Mesh transport flapping and recovery:**
- What's not tested: Rapid BLE connect/disconnect cycles. Peer address changing (e.g., after a Wi-Fi reconnect on MAC-rotating phones). DNS resolution failures.
- Files: `myco-core/src/dns_intercept.rs`, `myco-core/src/peer_relay.rs`
- Risk: Chat and app sync stall under transient network conditions.
- Priority: MEDIUM — Add integration tests with a simulated flaky transport. Reproduce FIPS#130 scenario.

**Pairing handshake expiration and resend:**
- What's not tested: Pair requests that expire mid-send (PAIR_TTL_SECS = 120s). Retries that outlive the original secret. Concurrent accept/decline from both sides.
- Files: `myco-core/src/content.rs` (dial_pair_event, handle_pair_event)
- Risk: Asymmetric Circle state (one side paired, the other not). Silent failures.
- Priority: MEDIUM — Add tests for timing and edge cases. Mock clock for deterministic testing.

---

*Concerns audit: 2026-08-01*
