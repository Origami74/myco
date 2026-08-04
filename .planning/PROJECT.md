# Myco

## What This Is

Myco is a peer-to-peer app-sharing network for Android. Devices form a FIPS mesh
(BLE L2CAP, Wi-Fi Aware/UDP, TCP/Tor) and sync nsites — static web apps published on
Nostr — directly between phones with no internet. A Jetpack Compose UI drives a native
Rust core (`libmyco_core.so`) over a JNI/JSON reducer; the core embeds a Nostr relay, a
Blossom blob store, a local gateway, and a FIPS node.

The next two milestones make the mesh actually reliable, then turn Myco from an nsite
renderer into a mesh app runtime that also runs napplets.

## Core Value

**Phones in the same room connect to each other, reliably, without the user doing
anything.** Every other feature is worthless if peering doesn't hold.

## Requirements

### Validated

<!-- Shipped and confirmed valuable — inferred from the codebase map. -->

- ✓ Embedded FIPS mesh node with BLE L2CAP, UDP/Wi-Fi Aware, TCP transports — existing
- ✓ QR-based mutual pairing (pair-request/pair-accept signed events, one-time secret) — existing
- ✓ Circle membership gating relay and Blossom access to paired peers — existing
- ✓ Embedded Nostr relay (`:4870`) and Blossom blob store (`:24243`) — existing
- ✓ Local gateway serving `http://<host>.nsite` to a per-nsite WebView — existing
- ✓ DNS interception for `.fips` and `.nsite` names over the TUN device — existing
- ✓ Mesh names (`<npub>.fips`) resolving peers system-wide — existing
- ✓ Peer relay pool with persistent per-peer WebSockets, keepalive, backoff — existing
- ✓ Gossip fan-out of local events to connected Circle peers — existing
- ✓ Library (pinned nsites), Discover (nsites on Circle peers), staged nsite updates — existing
- ✓ Exit-node mode for routing mesh traffic to the internet — existing

### Active

**Milestone A — Rock-solid peering (v0.5, release 2026-08-05)**

- [ ] Phones in a room converge: every reachable peer connects, all-to-all, within a bounded time
- [ ] Peering survives churn — Wi-Fi reconnect, MAC rotation, BLE flap, app backgrounding — without toggling mesh off/on
- [ ] In-app peer diagnostics: for any peer, why it is or isn't connected
- [ ] `feat/platform-peer-queue` rebased onto current fips master, changes scoped to BLE + DNS/mDNS/TUN seams
- [ ] Every fips commit extractable as a focused upstream pull request
- [ ] `content.rs` split into concern-shaped modules with a documented lock hierarchy
- [ ] Panic-prone error paths hardened; corrupted Circle/library files surfaced instead of silently emptied
- [ ] Pair requests delivered durably instead of fire-and-forget
- [ ] Discover no longer pins every app when one is opened
- [ ] Wi-Fi Aware defaults on; pending peering requests visible in the UI

**Milestone B — Mesh protocol and napplet runtime**

- [ ] Mesh TTL lives in the relay-to-relay protocol (`MESH_EVENT`), not in event bodies
- [ ] An unmodified third-party relay (Citrine) and Blossom server work as Myco backends
- [ ] Settings let the user choose embedded, external, or both for relay and blob storage
- [ ] nsites reach the mesh through a dedicated mesh relay URL, usable by standard Nostr
      libraries (applesauce, nostr-tools) with no shim
- [ ] `window.myco.neighbours` convenience API layered over that URL
- [ ] Mesh publish/subscribe is explicit in nsite code — never accidental
- [ ] Myco runs napplets per the NIP-5D protocol, implemented in Rust
- [ ] Napplet v1 covers the conformant core plus the `identity`, `storage`, `outbox` and
      `relay` NAP domains — enough for a napplet to read and publish Nostr through the host
- [ ] `napplet.neighbours` API for mesh publish/subscribe from a napplet
- [ ] `neighbours` written up as a candidate NAP domain and proposed upstream
- [ ] Napplets distribute over the existing manifest + Blossom pipeline under their own
      event kind, appearing alongside nsites in one Library with a type badge

### Out of Scope

- Desktop and OpenWrt targets — Android-first; the core stays mobile-shaped this cycle
- Interop with v0.4 devices that carry TTL in the event body — clean break, small user base
- Overfitting fips to Myco — fips changes stay minimal and upstreamable, or they don't land
- Rewriting fips transports wholesale — rebase and scope, don't redesign
- Parallel-team phase structure — solo developer, phases execute sequentially

## Context

**Where the code is.** Four Rust crates in this repo (`myco-core`, `nsite-deck`,
`myco-relay`, `myco-blossom`) plus the Kotlin Android app. `fips` is a path dependency on
a local checkout at `reference/fips` (gitignored).

**The fips divergence.** `feat/platform-peer-queue` carries 19 commits — Android BLE
backend and PSM discovery, platform-pushed peer queue, app-owned TUN/DNS seams,
transport-preference roaming, UDP `sin6_scope_id` fix — and sits 232 commits behind a
heavily refactored master. Master already contains the basic Android feature gate and
custom TUN, so some of those 19 commits dissolve on rebase. Commits are already
theme-separated, which is what makes upstream extraction feasible.

**The reported failure.** Users report phones connecting to zero peers, or one or two
out of many nearby. Suspected handshake failure, possibly tiebreaker-related. FIPS#130
reports Wi-Fi AP peering stalling after reconnect on MAC-rotating phones.

**Known weak spots** (from `.planning/codebase/CONCERNS.md`): `content.rs` is 2,508 lines
with 14 mutex fields and informal lock ordering; 273 unwrap/expect calls; fire-and-forget
mesh fan-out drops messages silently; corrupted `circle.json`/`library.json` load as
empty with no warning; sync spawns unbounded concurrent tasks that swamp a BLE link;
`state()` polling locks 10+ mutexes at UI framerate.

**Field TODOs** are tracked in `reference/FIX-TODOS.md`.

**Current release is v0.4.2** (2026-08-04) — system-aware AMOLED dark mode, merged
from [#23](https://github.com/Origami74/myco/pull/23) and released outside the
roadmap. It changed nothing about the mesh, but it is the baseline that Milestone A's
"measurably better mesh quality" is measured against, and it establishes the Compose
theming rules that Milestone A's new diagnostics UI has to follow — see the Android UI
section of `.planning/codebase/CONVENTIONS.md`.

**Napplet references.** napplet.run/docs is the authority: NIP-5D
([nips#2303](https://github.com/nostr-protocol/nips/pull/2303)), the NAP capability-domain
registry ([napplet/naps](https://github.com/napplet/naps)), and Kehto's `RUNTIME-SPEC.md`
as the best conformant-host description. `jodobear/uzel` turned out **not** to be a port of
the Kehto runtime — it forks pablof7z's unrelated `nampplets`/NMP project, is Linux/Tauri
only, and uses a domain vocabulary that diverges from the ratified NAP registry. It is not a
scope floor and not a dependency.

A napplet is host-assembled verified bytes injected into a sandboxed `srcdoc` iframe
(`sandbox="allow-scripts"`, no `allow-same-origin`); the JS runs in the host's own engine, so
Android WebView is already the runtime and no JS/WASM crate is needed. Manifests are kind
35129, structurally near-identical to the nsite manifests `nsite-deck` already resolves.
`neighbours` appears in none of the 23 NAP domains — it is Myco's own contribution.

## Constraints

- **Timeline**: Demo 2026-08-04, release 2026-08-05 — the release ships only if net
  mesh quality beats the current release
- **Team**: Solo developer — phases must be sequentially executable
- **Platform**: Android only this cycle — desktop/OpenWrt deferred
- **Upstream**: fips changes must stay minimal, generic, and extractable as focused
  pull requests against fips master — no Myco-specific coupling in the fips tree
- **Compatibility**: Clean break on the mesh event protocol; no v0.4 interop shim
- **Protocol**: napplet.run docs are authoritative; Myco's only addition is mesh behaviour
- **Rebase risk**: The fips rebase is timeboxed — hard cutoff end of day one, then fall
  back to targeted fixes on `feat/platform-peer-queue` so the release date holds

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Two milestones: reliability first, then protocol + napplet | The mesh core has to be trustworthy before layering new runtimes on it | — Pending |
| Timeboxed fips rebase with a fallback to the current branch | Master's 232 commits may already fix peering, but the release date can't depend on that | — Pending |
| Ship Milestone A before starting B | Field feedback on peering should shape the protocol work | — Pending |
| TTL moves to relay-to-relay `MESH_EVENT` | Modifying event bodies blocks plugging in an unmodified relay like Citrine | — Pending |
| Clean break on the old TTL format | Small user base; a compat shim costs more than it saves | — Pending |
| Mesh exposed as a relay URL plus a `window.myco` global | The URL keeps applesauce and nostr-tools working unmodified; the global keeps simple apps simple | — Pending |
| Napplets ship on the existing manifest + Blossom pipeline, own event kind | Reuses sync, Discover, and Circle gating; the type distinction stays explicit | — Pending |
| Peer diagnostics in the release cut | Demo failures need to be explainable, and field data drives the real hardening | — Pending |
| Napplet v1 = conformant core plus `identity`, `storage`, `outbox`, `relay` | NAP domains are feature-detected, so a subset is conformant; this is the smallest set where a napplet can actually do Nostr | — Pending |
| `neighbours` proposed upstream as a NAP domain, not kept private | Same reasoning as the fips upstreaming constraint — a mesh domain designed to registry conventions outlives Myco | — Pending |
| uzel dropped as a reference | It forks pablof7z's unrelated nampplets/NMP project; treating it as a napplet.run reference would have imported a divergent vocabulary | ✓ Good |
| Instrumentation phase before any peering fix | The tiebreaker and duty-cycle root causes are inferred from code, not device logs; fixing before observing is guessing | — Pending |
| fips rebase after the release, theme by theme | 19 commits against 232 of upstream refactor is not two-day work, and the release must not depend on it | — Pending |

## Evolution

This document evolves at phase transitions and milestone boundaries.

**After each phase transition** (via `/gsd-transition`):
1. Requirements invalidated? → Move to Out of Scope with reason
2. Requirements validated? → Move to Validated with phase reference
3. New requirements emerged? → Add to Active
4. Decisions to log? → Add to Key Decisions
5. "What This Is" still accurate? → Update if drifted

**After each milestone** (via `/gsd-complete-milestone`):
1. Full review of all sections
2. Core Value check — still the right priority?
3. Audit Out of Scope — reasons still valid?
4. Update Context with current state

---
*Last updated: 2026-08-02 after initialization*
