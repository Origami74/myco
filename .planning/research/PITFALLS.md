# Domain Pitfalls

**Domain:** Android peer-to-peer mesh networking (BLE L2CAP + Wi-Fi Aware) with a Nostr-based app runtime
**Researched:** 2026-08-04
**Overall confidence:** MEDIUM — Android BLE/NAN platform behavior is well documented (HIGH); Myco-specific root causes for the zero/one-peer symptom are inferred from code (MEDIUM), not yet confirmed by device logs.

## Critical Pitfalls

Mistakes that cause rewrites, missed release dates, or a mesh that silently fails in the field.

### Pitfall 1: Simultaneous BLE connect race with no deterministic tiebreaker

**What goes wrong:** Two phones in range both discover each other and both attempt to initiate a GATT/L2CAP connection at roughly the same time. Without a deterministic rule for who is central and who is peripheral, both sides can end up in the same role (both central, both peripheral), one side's connect call fails or times out, or a partial handshake stalls with no completed L2CAP CoC channel. This is the most likely mechanical explanation for "connects to zero peers, or one or two out of many."

**Why it happens:** Android's BLE stack requires an explicit connector/acceptor split — nothing in the platform arbitrates this automatically for two peer apps running the same code. If the tiebreaker (e.g., "peer with lexicographically lower pubkey/MAC becomes central") is decided independently by each side from slightly different discovery timing or a stale peer list, both sides can compute different winners, or the same side can retry a losing role and never flip.

**Consequences:** In a room of N phones, only the pairs that happen to pick complementary roles connect. This explains asymmetric results (some phones see everyone, most see nobody) far better than a chipset connection-count limit — chipsets in this class hold 4-8 concurrent connections, well above what a room-scale mesh needs.

**Prevention:** Make the tiebreaker a pure function of stable, mutually-known identifiers (both sides' Nostr pubkeys, not ephemeral BLE MAC — since MAC can rotate), computed identically on both ends from the same inputs at the same discovery event. Log the computed role decision on both sides during the demo so a mismatch is visible immediately. Add a role-flip retry: if a connect attempt fails or times out, don't just retry the same role — reverse it after N failures.

**Detection:** In-app peer diagnostics (already a Milestone A requirement) should show, per peer: last discovered timestamp, computed role, and connect attempt outcome. If two phones show themselves both as central (or both peripheral) for the same peer, that's the smoking gun.

**Milestone:** A — this is very likely the top-priority fix for the reported symptom. Should be the first thing instrumented, before broader rebase work.

### Pitfall 2: Advertise/scan duty-cycle asymmetry hides peers from each other

**What goes wrong:** BLE advertising and scanning are separate radio duty cycles; a phone that is scanning aggressively but advertising infrequently (or vice versa) may take much longer to be discovered by, or to discover, a given peer. In a room with many phones this compounds: pairwise discovery times are not symmetric, so the mesh converges unevenly — some nodes get discovered by everyone quickly, others sit unseen for tens of seconds or don't get seen at all before a scan window elapses.

**Why it happens:** Android throttles background BLE scanning and advertising (more aggressively behind Android 12+ nearby-devices permission and battery-optimization rules), and app code that doesn't request BLUETOOTH_SCAN/BLUETOOTH_ADVERTISE with the right flags, or doesn't request an unrestricted/aggressive scan mode in the foreground, gets deprioritized scan results.

**Consequences:** "Some phones see everyone, others see nobody" — the classic asymmetric-reachability report from this milestone's demo symptom.

**Prevention:** Verify (in the diagnostics view) that advertising and scanning are both active and foregrounded with the correct Android 12+ permissions (`BLUETOOTH_SCAN` with `neverForLocation` where applicable, `BLUETOOTH_ADVERTISE`, `BLUETOOTH_CONNECT`) on all test devices. Use `SCAN_MODE_LOW_LATENCY`/high-duty-cycle scan while the app is foregrounded and mesh is actively converging, and confirm advertise intervals are short during the same window. Test with the app foregrounded first to isolate this from broader background-restriction pitfalls (Pitfall 3).

**Detection:** Diagnostics should log discovery latency per peer, not just yes/no state. Wide variance in discovery time across a set of phones, or one-way discovery (A sees B, B never sees A), points here.

**Milestone:** A.

### Pitfall 3: Android 12+ background restrictions silently deny BLE/Wi-Fi Aware operations

**What goes wrong:** Android 12 replaced the coarse-location requirement for BLE scanning with runtime `BLUETOOTH_SCAN`/`BLUETOOTH_ADVERTISE`/`BLUETOOTH_CONNECT` permissions, plus battery/background execution limits that can silently pause scanning, advertising, or Wi-Fi Aware sessions when the app is backgrounded — with no exception thrown, just no results.

**Why it happens:** Apps written against older BLE permission models, or that assume "if I called startScan() it's scanning," don't check for background-restricted states. OEM battery managers (Samsung, Xiaomi especially) add another layer of background kill/throttle on top of stock AOSP behavior.

**Consequences:** Peering that works fine while the app is open and foregrounded stops working, or degrades severely, the moment the screen locks or the app backgrounds — this is exactly the "app backgrounding" churn scenario called out in Milestone A's success criteria.

**Prevention:** Use a foreground service with an active notification while the mesh is meant to be running, and audit all three BLE runtime permissions plus Wi-Fi Aware's `NEARBY_WIFI_DEVICES` permission (Android 13+) are granted and not silently revoked. Test explicitly on Samsung and Xiaomi devices — these OEMs are known for aggressive background app killers beyond AOSP defaults; Pixel is the closest to stock behavior and should not be treated as representative.

**Detection:** Reproduce by backgrounding the app for 60+ seconds mid-mesh and checking whether peer connections drop or peer diagnostics stop updating. Compare behavior across at least one Samsung, one Xiaomi, and one Pixel device before the release — vendor divergence here is well known and not merely theoretical.

**Milestone:** A.

### Pitfall 4: Wi-Fi Aware / NAN MAC rotation breaks peer identity continuity across reconnect

**What goes wrong:** Android's Wi-Fi Aware (NAN) layer randomizes MAC addresses as part of normal operation (this is standard privacy behavior, and got more aggressive with Android 12's per-connection rotation for regular Wi-Fi too). If Myco's peer/session state is keyed by MAC or IP derived from MAC rather than by the peer's stable Nostr identity, a MAC rotation after a Wi-Fi reconnect makes the mesh treat a returning peer as a brand-new, unknown node — or worse, leaves a stale session entry pointing at an address that no longer resolves, and the peering silently stalls rather than re-establishing. This matches upstream FIPS#130 exactly: "Wi-Fi AP peering stalling after reconnect on MAC-rotating phones."

**Why it happens:** NAN discovery sessions themselves are ephemeral and re-negotiated per session, but any layer above NAN (peer relay pool, Circle IP mapping, DNS-to-mesh-address cache) that caches a peer's network address rather than re-resolving it on every reconnect will desync the moment the address changes underneath it. CONCERNS.md independently flags this exact shape of bug: DNS/IP resolution for `<npub>.fips` is cached and address-dependent, with no fallback re-resolution path.

**Consequences:** The only documented workaround today is "toggle mesh off/on to force node refresh" — unacceptable for the Milestone A bar of "survives churn... without toggling mesh off/on."

**Prevention:** Key all peer session and Circle-gate state by stable identity (npub / pubkey), never by MAC or the IP derived from it. On any transport-level disconnect/reconnect signal (not just a timeout), force re-resolution of the peer's current mesh address before assuming the peer is unreachable. Treat address change as an expected, not exceptional, event.

**Detection:** Reproduce FIPS#130 directly: pair two phones over Wi-Fi AP transport, force a Wi-Fi reconnect (toggle airplane mode or move out of AP range and back) on the MAC-rotating side, and confirm peering resumes within a bounded time without a manual mesh toggle.

**Milestone:** A — named explicitly as a target symptom.

### Pitfall 5: Fire-and-forget delivery masquerading as "connected"

**What goes wrong:** Both the mesh event fan-out (`gossip.rs`/`peer_relay.rs`) and pair request delivery (`dial_pair_event`) use fire-and-forget sends with backoff and no delivery confirmation surfaced to the user. A peer can appear "connected" at the transport layer while every actual message to it is being silently dropped because its outbound queue is full — the UI has no way to distinguish "connected and working" from "connected and mute."

**Why it happens:** `try_send()` with backoff is a reasonable transport-layer choice for a lossy mesh, but treating it as the whole story (no retry-on-reconnect, no dropped-message counter, no user-visible signal) conflates transport connectivity with application-level delivery.

**Consequences:** Demo-day failure mode: phones show as "connected" in the peer list but pairing/sync never completes, and there is no diagnostic signal pointing at the real cause — which directly undermines the Milestone A requirement for "in-app peer diagnostics: for any peer, why it is or isn't connected."

**Prevention:** Surface a distinct connection state beyond binary connected/disconnected — at minimum, track and expose per-peer send failures/drops in the diagnostics view CONCERNS.md already calls for. For pairing specifically (already an Active requirement), persist undelivered pair requests to disk and retry on reconnect rather than a fixed 60s fire-and-forget window.

**Detection:** Instrument a dropped-message counter per peer relay queue; any nonzero count while the peer shows "connected" in the UI is the signature of this pitfall.

**Milestone:** A (diagnostics + durable pairing are both already scoped here — this pitfall is the rationale for doing them together rather than diagnostics-only).

### Pitfall 6: Rebasing a long-lived feature branch onto a heavily refactored master without re-deriving the diff per concern

**What goes wrong:** Attempting a single mechanical `git rebase` of all 19 commits against master head-on, then fighting merge conflicts commit-by-commit, tends to produce a branch that compiles but has silently absorbed or duplicated logic that master's refactor already provides — and loses the "each commit is a focused, extractable upstream PR" property that the theme-separated commit history currently has.

**Why it happens:** A 232-commit gap with heavy refactoring means the file layout, seams, and abstractions the original 19 commits were written against likely no longer exist verbatim. Naive rebase conflict resolution optimizes for "make it compile," not "preserve which commit is the minimal, generic diff for feature X."

**Consequences:** Directly threatens two Milestone A requirements at once: the rebase itself, and "every fips commit extractable as a focused upstream pull request." A botched rebase either misses the hard cutoff (falling back to targeted fixes on the old branch, per the timebox decision) or lands but produces PRs upstream maintainers can't easily review.

**Prevention:** Before rebasing, re-diff each of the 19 commits against current master independently to identify which are now no-ops (master's Android feature gate / custom TUN already cover them — PROJECT.md already flags this for some commits) versus which carry real new logic. Rebase in theme order (BLE backend+PSM discovery, then peer queue, then TUN/DNS seams, then transport-preference roaming, then the UDP fix) so each theme's conflicts are resolved with only that theme's intent in mind, and re-verify compilability and test pass after each theme, not just at the end. Keep the timebox: if a theme is fighting the refactor hard, that's the one to drop to the fallback branch, not the whole rebase.

**Detection:** After rebase, diff the rebased branch against master file-by-file for each theme; if a theme's diff is empty or trivial, that commit was superseded — drop it rather than force it in. If a theme's diff touches files well outside its stated seam (BLE, DNS/mDNS, TUN), that's scope creep the PR-extractability goal explicitly forbids.

**Milestone:** A — hard cutoff end of day one per PROJECT.md constraints; this pitfall is about how to spend that day, not whether to attempt it.

## Moderate Pitfalls

### Pitfall 7: Discovery/gossip storms masking real convergence failures

**What goes wrong:** In an all-to-all room-scale mesh, uncoordinated retry/backoff across many peers (each independently deciding a peer is unreachable and re-announcing) can produce a discovery storm that looks like activity but doesn't converge — nodes keep re-discovering the same peers without ever stabilizing a connected state, especially once retry timers correlate (all phones started the app roughly simultaneously at demo start).

**Why it happens:** Fixed or lightly-jittered backoff intervals synchronize across devices that all started under similar conditions; without randomized jitter, retries from many peers cluster in time and compete for the same discovery windows (see Pitfall 2 — this compounds with duty-cycle contention).

**Prevention:** Add randomized jitter to retry/backoff/rescan intervals so device retries desynchronize over time rather than staying correlated. Cap the number of concurrent in-flight discovery/connect attempts per device.

**Detection:** Diagnostics should show connect-attempt rate per peer over time — a sawtooth pattern with no convergence trend by minute 2-3 indicates storming rather than progress.

**Milestone:** A.

### Pitfall 8: Conflating napplet.run (NIP-5D) with pablof7z's "nampplets" lineage

**What goes wrong:** Both projects independently use "napplet"-adjacent naming, and `jodobear/uzel` forks pablof7z's nampplets work, not napplet.run. Treating uzel's API surface, manifest format, or runtime model as authoritative for NIP-5D — or citing uzel issues/docs as if they describe napplet.run behavior — will produce a Milestone B implementation that doesn't match the protocol Myco actually needs to support.

**Why it happens:** Superficial name similarity plus both being Nostr-adjacent "run untrusted web apps" concepts invites conflation, especially when searching for prior art or implementation references.

**Prevention:** Treat napplet.run/docs as the sole normative source for the NIP-5D protocol surface (event kinds, manifest shape, capability model). Use `jodobear/uzel` only as a secondary implementation-pattern reference (per PROJECT.md's own framing: "a loose implementation reference"), and flag any place uzel's behavior is used to fill a gap in napplet.run's docs explicitly, so it can be revisited if the two diverge.

**Detection:** Before implementing any napplet.run-derived feature, confirm the spec detail traces to napplet.run/docs, not to uzel source code or pablof7z's nampplets writeups.

**Milestone:** B.

### Pitfall 9: Clean-break protocol migration leaves already-paired v0.4 devices in a confusing half-broken state

**What goes wrong:** Moving mesh TTL out of event bodies into the relay-to-relay wire protocol (`MESH_EVENT`) is a wire-format change. Devices still on v0.4 in the field will either fail to parse v0.5 mesh events, or worse, partially parse them (missing TTL) and forward events with no hop limit — turning a clean break into a live footgun (unbounded relay loops) rather than a clean failure.

**Why it happens:** "Clean break, no interop shim" (an explicit Out of Scope decision) is the right call for a small user base, but clean break still needs the old and new sides to fail loudly and immediately rather than silently misbehaving when they meet on the wire.

**Prevention:** Version the `MESH_EVENT` wire format explicitly (a protocol version byte/field) so a v0.4 relay receiving a v0.5 frame — or vice versa — rejects it outright rather than misinterpreting missing TTL as "forward forever." Ship the release notes/changelog with a clear statement that mixed v0.4/v0.5 mesh is unsupported, and confirm the app surfaces something to the user rather than silently degrading (e.g., "peer is running an incompatible mesh version").

**Detection:** Test a v0.4 build against a v0.5 build directly before release; confirm the failure mode is an explicit rejection, not silent forwarding or a panic (the existing 273-unwrap density makes an unhandled version mismatch a plausible new panic vector).

**Milestone:** B.

## Minor Pitfalls

### Pitfall 10: Sandboxing napplets without network access is necessary but not sufficient

**What goes wrong:** "No network access" is the easy 80% of sandboxing an untrusted runtime with system capability exposure (`napplet.neighbours`, mesh pub/sub). The harder 20% — capability scoping per-napplet (can napplet A see napplet B's mesh traffic? can it enumerate all Circle peers or just ones it's been introduced to?), resource limits (CPU/memory/storage per napplet), and preventing a napplet from using the mesh API itself as a covert network channel once it has a `neighbours` capability — is where real escapes happen.

**Why it happens:** "No network access" reads as a complete sandboxing story but the mesh pub/sub API is itself a network primitive by another name; granting it access without per-napplet scoping reintroduces exactly the network surface the sandbox was meant to remove.

**Prevention:** Treat `napplet.neighbours` grants as a capability to be scoped per-napplet (which peers, what event kinds, what rate) rather than an all-or-nothing mesh-access toggle. Apply the same Circle-gating logic already used for relay/Blossom access to napplet mesh capability grants.

**Detection:** Design review question for Milestone B: "if a napplet is malicious, what's the worst it can broadcast to the mesh, and to how many peers, before anything notices?" If the answer is "everything, to everyone, immediately," the sandbox isn't scoped yet.

**Milestone:** B.

## Phase-Specific Warnings

| Phase Topic | Likely Pitfall | Mitigation |
|-------------|-----------------|------------|
| BLE handshake / role assignment fix | Pitfall 1 (tiebreaker race), Pitfall 2 (duty-cycle asymmetry) | Instrument role decisions and discovery latency in diagnostics first; fix tiebreaker before touching anything else |
| Peering churn resilience (Wi-Fi reconnect, backgrounding) | Pitfall 3 (Android 12+ background restrictions), Pitfall 4 (NAN MAC rotation) | Key sessions by pubkey not MAC/IP; test on Samsung + Xiaomi + Pixel, not just one vendor; reproduce FIPS#130 directly |
| In-app peer diagnostics | Pitfall 5 (fire-and-forget masks real state) | Diagnostics must expose send-failure counts and role decisions, not just connected/disconnected |
| fips rebase | Pitfall 6 (mechanical rebase loses PR-extractability) | Rebase theme-by-theme with a hard per-theme timebox; drop themes that fight the refactor to the fallback branch |
| Discover / gossip fan-out fixes | Pitfall 7 (discovery storms) | Add jitter to retry intervals; cap concurrent connect attempts |
| Mesh TTL → wire protocol (Milestone B) | Pitfall 9 (silent misparse on mixed versions) | Explicit protocol version field; reject rather than degrade |
| napplet.run runtime implementation | Pitfall 8 (wrong reference lineage), Pitfall 10 (incomplete sandboxing) | Cite napplet.run/docs only as normative; scope `neighbours` capability per-napplet |

## Sources

- [L2CAP (COC) sends one packet per connection interval — ST Community](https://community.st.com/stm32-mcus-wireless-35/l2cap-coc-sends-one-packet-per-connection-interval-129926) — MEDIUM confidence, vendor forum, cross-checked against general BLE connection-count knowledge (4-8 concurrent connections typical for BLE chipsets)
- [Maximum connection limit reached on connecting via Android BLE — Android Community](https://support.google.com/android/thread/43071437/maximum-connection-limit-reached-on-connecting-via-android-ble?hl=en) — MEDIUM confidence, official Google support forum
- [Wi-Fi Aware overview — Android Developers](https://developer.android.com/develop/connectivity/wifi/wifi-aware) — HIGH confidence, official Android docs
- [Wi-Fi Aware — Android Open Source Project](https://source.android.com/docs/core/connect/wifi-aware) — HIGH confidence, official AOSP source docs
- [Implement MAC randomization — Android Open Source Project](https://source.android.com/docs/core/connect/wifi-mac-randomization) — HIGH confidence, official AOSP source docs
- [MAC randomization behavior — Android Open Source Project](https://source.android.com/docs/core/connect/wifi-mac-randomization-behavior) — HIGH confidence, official AOSP source docs, confirms Android 12+ per-reconnect MAC rotation behavior that underlies FIPS#130
- Project-internal: `/Users/gump/Documents/development/fips/fips-pop/.planning/PROJECT.md` and `/Users/gump/Documents/development/fips/fips-pop/.planning/codebase/CONCERNS.md` — HIGH confidence, primary source for Myco-specific fragile areas (fire-and-forget delivery, DNS/IP caching, content.rs lock ordering, rebase scope)
