# nsite updates: staged activation + neighborhood propagation

> Status: PARTLY BUILT. Discovery (§3), staging (§2), propagation through
> neighborhoods (§4) and auto-activation ship today. Open-instance gating (§5.2) and version
> history/pinning/revert (§6) are still proposals. Open questions are marked
> **TBD / open**.
>
> **How updates travel between phones.** Through neighborhoods, as the
> `nsite-updates` extension (and `napplet-updates` for napplets), each with a
> Get and a Share switch per neighborhood
> ([nap-neighbor/extensions.md](../nap-neighbor/extensions.md)). The update
> check itself asks public relays only.

An nsite is a set of author-signed manifest events (kinds 15128/35128) plus
content-addressed blobs ([nsite-layer.md](./nsite-layer.md),
[propagation.md](./propagation.md)). The author updates a site by publishing a
**newer manifest** (higher `created_at`) for the same replaceable slot
`(kind, author, d?)`, pointing at new/changed blob hashes. This document
specifies how an installed device:

1. **discovers** that a newer version exists (from online relays),
2. **stages** it without breaking the working copy — never switching the served
   version until *every* blob of the new version is local,
3. **activates** it atomically, respecting open instances (no mid-session swap),
4. **propagates** it into its neighborhoods (§4).

> **Scope.** Core/native work in `myco-core` + `myco-relay`, plus a small
> lifecycle signal from the Android shell. The WebView is unaware any of this
> exists — it always sees one consistent version for the life of its task.

---

## 1. The load-bearing conflict

The embedded relay collapses replaceable manifests to **newest-by-`created_at`**
([`admit`/`slot_of`](../../../myco-relay/src/lib.rs)), and today the gateway serves
whatever manifest is newest in the store
([`gateway::readiness` → `get_manifest`](../../../nsite-deck/src/gateway.rs)). If we
left that coupling in place, a newer manifest landing in the store would
**activate instantly**: its blobs aren't local yet, so `readiness()` returns
`Incomplete` and the gateway flips a *working* app to its 503 loading page — and
it would swap even under a live WebView session.

**We do not fix this by holding manifests back from the relay.** The relay must
stay a faithful NIP-01 store: a newer replaceable manifest **replaces immediately**
like any other event. Withholding new versions to protect our own serving would
make the store lie and stall propagation — exactly what we don't want.

**The fix is to decouple *local serving* from the relay store.** Two independent
views over a site's manifests:

- **Relay store (the faithful view).** Pure NIP-01 — newest replaceable wins,
  immediately. Older
  versions it replaces are **persisted separately** by core (the version history,
  §6) before they fall out of the store.
- **Gateway active pointer (the local view).** A core-owned choice of *which*
  version this device actually **serves to its own WebView**. It advances to a
  newer version only once that version's blobs are fully local (R2) and no
  instance is open (R4); the user can also pin/revert it (§6).

The gateway reads the **active pointer**, not relay-newest — implemented as a thin
[`RelayBackend`](../../../nsite-deck/src/seams.rs) wrapper whose `get_manifest`
returns core's active manifest for the slot (its blobs are, by construction, all
present). Everything else about the relay is untouched. A happy consequence:
**revert and pin become pure pointer moves** — no relay write, no force-set, the
relay keeps faithfully holding+propagating the author's newest regardless of what
*we* choose to run locally.

---

## 2. Staging

A "version we know about but don't yet serve" needs its blobs downloaded before it
can become the active pointer. `myco-core` tracks that per site:

```
pending_updates: Mutex<HashMap<SiteAddr, PendingUpdate>>

PendingUpdate {
    manifest: Event,     // newer than active; ALREADY in the relay store (NIP-01)
    total: u32,          // unique blob count
    pulled: u32,         // verified-present blobs
    source: UpdateSource // where to pull blobs from
    state: Downloading | Ready | Failed
}

enum UpdateSource { Online, Neighborhood }  // online Blossom, or the neighborhood it arrived in
```

The candidate manifest **is** in the relay store (we don't withhold it); it just
isn't the active pointer yet. Blobs go straight into Blossom as they verify —
content-addressed, so the new version's blobs coexist with the active version's and
any shared blob is already present (free dedup). The active pointer keeps the
gateway serving the working version with zero disruption until the download
completes.

Staging reuses the blob-fetch half of [`sync.rs`](../../../nsite-deck/src/sync.rs);
the only change there is to **split "fetch + verify all blobs" from "store the
manifest"** so callers can drive the two independently (today `sync_site` does
both; we add a `stage_blobs(manifest, source)` that just fetches blobs, leaving
activation — the pointer move — to core).

---

## 3. Discovering an update (R1)

A newer manifest reaches us through **two channels**, both feeding the same staging
pipeline (§2) and treated identically once a candidate is in hand:

- **Pull (we poll).** A bounded subscription we initiate — manual or background
  (§3.1–3.2).
- **Push (a neighbor shares it).** A device that activates an update shares
  it into neighborhoods where it shares `nsite-updates`; it travels the
  neighborhood's publish hops (§4). Where this device gets `nsite-updates`, the
  candidate is staged exactly as a polled one. So a device often learns of an
  update from a neighbor before it ever runs a check — and even fully offline.

The rest of §3 covers the pull channel; the push channel is §4.

### 3.1 Pull trigger

Two triggers, both feeding the same staging pipeline:

- **Manual.** A "Check for updates" affordance (app long-press sheet + a Settings
  entry) dispatches `CheckNsiteUpdates`.
- **Background.** An automatic check when the app comes to the foreground and
  every **6 h** while the process lives, realising nsite-layer.md §5.4's "eager
  refresh of pinned sites". Both dispatch `CheckNsiteUpdates { auto: true }`
  from `UpdateChecks.kt`. Never blocks the UI (spawn-not-block, like
  discovery).

Both triggers pass through **one gate** in the core (`update_gate.rs`), so the
rules hold however many triggers fire:

- **Never two at once.** While a check runs, another is not started.
- **Automatic checks are throttled.** One is skipped if any check started
  within the last **30 min**. Foregrounding happens on every app switch; the
  throttle caps the automatic load on public relays at two checks an hour.
- **Manual always runs.** It bypasses the throttle. If a check is already
  running, the press joins it instead of starting a second: that check then
  reports its result.
- **Only manual checks report.** An automatic check applies what it finds but
  fires no toast; the result toast is for the user who asked.

Timing choices:

- **Foreground: 20 s after coming to the foreground**, cancelled if the app
  leaves first. A quick app switch costs nothing.
- **Periodic: 6 h.** Updates are rare, deliberate publishes, and a neighbor
  often shares one before any poll would (§4). Four checks a day keep a
  long-lived process current without polling from a phone in a pocket.
- **Per process.** The gate lives in memory, so a new process starts open:
  a cold start always checks, 20 s after the app comes up.
- **Background runs.** The BLE foreground service (`BleService`) keeps the
  process alive while the app is in the background, so the 6 h timer keeps
  running there too. Without it, the process dies and the next foreground
  check is the next one.
- **No wake-ups.** There is no alarm or WorkManager job. The timer is a
  coroutine delay, which stops while the phone sleeps. The throttle uses wall
  time, so a check before bed does not look recent in the morning.
- **Offline-only** has nowhere to ask: the check reads public relays only.
  Updates still arrive from neighborhoods (§4).

### 3.2 Check = one bounded subscription per relay

`CheckNsiteUpdates` is **not** a per-site, per-relay fan of fetches. It is a set of
**REQ subscriptions that run until EOSE** against a *deduplicated* relay set, with
one combined filter per relay covering every site we care about. The relay set is
the union of (public relays only — peers are never asked):

- **The authors' relays** — each Library author's NIP-65 write relays (kind
  `10002` as stored in the local relay; at most 12 in total), then
  `default_relays()`, used **only when online**. The author publishes updates
  there, so they are the authoritative poll target. The check never waits on a
  list: the authors' `10002`s ride along in the same `REQ` to the defaults, the
  indexers are asked for missing ones alongside, and whatever arrives is stored
  for the next check (an author with no list anywhere is not asked about again
  for 10 minutes).

**Deduplicate by URL** so overlapping relay lists across many sites collapse to a
single connection. For each unique relay, send **one** filter union'd across all
sites — `{ kinds: [15128, 35128], authors: [...all site authors...], "#d":
[...all named d-tags...] }` — then read events until **EOSE** and close. (One
combined REQ, not one per site; the relay returns each slot's newest.)

**The check is core-driven, not client-driven.** It goes straight to the
internet relay pool rather than through the loopback relay socket. A `REQ`
from an nsite never fans out to peers.

A returned manifest is a candidate iff `candidate.created_at > active.created_at`
for its slot (and newer than any already-staged candidate). Each such candidate is
**published into our own relay** — which (a) keeps the local store holding the
newest, (b) once activated, is shared into neighborhoods (§4), and (c) lets
core notice it and open a `PendingUpdate` to stage its blobs (preferring the
manifest's `["server", …]` hints, then `default_blossom_servers()` when
online). The
active pointer does not move until staging completes (§5).

> **Settled:** an author's update relays are their NIP-65 write relays, with
> `default_relays()` always asked too as the fallback
> ([nsite-layer.md §5](./nsite-layer.md)).

### 3.3 Surfacing

`SiteStatusView` gains:

- `updateAvailable: bool` — a staged-and-ready newer version exists but isn't yet
  active (deferred by an open instance, §5).
- `updateProgress: (pulled, total)` — while a staged update is downloading.

The Apps grid shows an "update" affordance on tiles with `updateAvailable`; the
long-press sheet offers "Update now" (a no-op nudge if it'll auto-apply on close).

---

## 4. Propagation through neighborhoods (R3)

Updates travel as the `nsite-updates` extension; napplets use
`napplet-updates`. The full rules are in
[nap-neighbor/extensions.md](../nap-neighbor/extensions.md); in short:

- **Sharing.** When an update is activated here, the manifest and every file
  it names are put in the scope of each neighborhood where this device
  **shares** `nsite-updates`, and the manifest is published there with the
  neighborhood's publish hops.
- **Getting.** Where this device **gets** `nsite-updates`, a newer manifest for
  a site in the Library is staged (§2): files come from that neighborhood (its
  scope, then its links), then the public servers unless offline-only. Once
  activated, it is shared onward by the same rule.
- **Not in the Library.** The manifest is kept and passed on like any
  neighborhood event; nothing is fetched.

**Blob availability is the natural gate.** A manifest never implies a source
can serve its blobs. A link that doesn't hold a blob answers 404 and the next
source is tried ([sync.rs](../../../nsite-deck/src/sync.rs) tries sources in
order). Activation stays gated on every blob being local (§5).

---

## 5. Activation + cache invalidation (R2 + R4)

### 5.1 Atomic swap = a pointer move

Activation is **moving the gateway active pointer** to the staged version and
appending it to the version history (§6) — *not* a relay write (the manifest is
already in the relay store from §3.2/§4). The swap is atomic: the next gateway
request resolves the new version; the previously-active version stays in history
(its blobs retained, not evicted). We drop the `PendingUpdate` and update the
Library title. **Activation only ever happens once `pulled == total`** — and only
when the site is not pinned (§6.2) and has no open instance (§5.2). The relay
store, meanwhile, already holds (and has propagated) the author's newest
regardless of whether we've activated it locally.

### 5.2 No mid-session swap

A running WebView must keep one consistent version for its whole task lifetime
(its in-page caches, open sockets, and asset hashes all assume one manifest).
So core tracks **open instances** per host and gates activation on it:

- The Android shell signals lifecycle: `NsiteActivity` → a `SetNsiteOpen(host,
  bool)` action (on resume/destroy), maintaining a core-side `open_hosts` set.
- Activation for host *H* proceeds immediately **iff** *H* has no open instance.
- If *H* is open, the version stays staged + `updateAvailable = true`; core
  activates when the **last** instance for *H* closes. A re-open after that gets
  the new version; the still-running task never changes under the user.

This makes "update available if one open instance has that app" fall out
naturally: open ⇒ deferred + flagged; closed ⇒ applied.

### 5.3 Serving consistency

Blobs are immutable (content-addressed), so they need no cache busting. The HTML
entry points are served by path→hash, which *does* change across versions; within
a session we serve a single manifest, so it's internally consistent. To be safe
against the WebView caching an entry page across an activation, the gateway sets
`Cache-Control: no-store` on the manifest-resolved HTML responses (blobs may stay
cacheable). **TBD / open:** whether `no-store` on HTML is enough or we also
version the loopback origin per activation.

---

## 6. Version history, pinning & revert

Activation never throws the old version away. Each site keeps a **local version
history** of the manifests it has fully downloaded, so the user can always step
back to a version that still works on their device — even fully offline, since the
old version's blobs are retained.

### 6.1 The history store

A per-site record in `myco-core` (persisted alongside the Library, e.g.
`versions.json`):

```
SiteVersions {
    addr: SiteAddr,
    active: EventId,            // the gateway active pointer (what we serve locally)
    pinned: Option<EventId>,    // if set, auto-update will not change `active`
    history: Vec<VersionRec>,   // every fully-downloaded version we still retain
}

VersionRec { manifest: Event, created_at: u64, title: String, blob_count: u32 }
```

Every retained version's blobs are **GC roots**: eviction (P5) must keep any blob
referenced by *any* `history` entry. History is bounded — **TBD: keep the last N
(≈3–5) versions** plus the pinned one; versions trimmed past the bound release
their (unreferenced) blobs. Shared blobs across versions are already dedup'd in
Blossom, so history is cheap when changes are incremental.

### 6.2 Pinning

`pinned = Some(v)` locks the active version to `v`: an update check still *stages*
and flags `updateAvailable`, but core **does not auto-activate** — the user stays
on the pinned version until they unpin (which then activates the newest staged
version, respecting §5.2). Pinning is how "don't move me off this version" and
"hold here while I test the new one" are expressed. Unpinned sites auto-activate
the newest complete version as today.

### 6.3 Revert

Reverting points `active` at a chosen `history` version. Its blobs are already
local (it's in history), so revert is instant and works offline. Crucially,
because activation is a **core-owned pointer** (§1), revert needs **no relay
change at all** — we don't touch the relay store, which keeps faithfully holding
and propagating the author's newest. Revert just moves the gateway pointer and, by
default, **pins** the reverted version so the next check doesn't immediately pull
the device forward again. Revert obeys §5.2 — a running instance keeps its version
until it closes. We do **not** share a revert into neighborhoods (it's a local,
per-device choice; the author's newest is still what spreads).

### 6.4 UI — the per-app settings window

The long-press app sheet had a no-op **Info** row; it is gone, and the sheet
shows the identifier in its header instead
([AppsScreen.kt](../../../android/app/src/main/java/app/myco/ui/screens/AppsScreen.kt)).
The proposal stands as a new entry: **"App settings"** — a per-app window that hosts:

- the site identity (host / npub) Info used to show,
- **Check for updates** (manual trigger; shows progress + `updateAvailable`),
- **Version** — the active version with its date, a **Pin** toggle, and the
  **history list**; tapping an older version offers **Revert** (which pins),
- (later) per-app update policy ([nsite-permissions.md](./nsite-permissions.md)).

"Remove app" (§ delete-nsite) stays on the sheet or moves here — **TBD**.

---

## 7. Failure & eviction

- **Staging fails** (source unreachable / blob mismatch): the `PendingUpdate`
  goes `Failed`, the active version is untouched, and the next check/​push retries.
  Partial blobs already in Blossom are harmless (dedup credit for the retry).
- **Eviction (P5):** blobs referenced by the active version *or any retained
  history version* (§6.1) are GC roots and never evicted; only blobs orphaned by
  trimming history past the bound are reclaimable. Until the global eviction pass
  exists, orphans linger (acceptable; same as today's forget-site behaviour).
- **Rollback:** first-class — revert to any retained version (§6.3). Newest-by
  -`created_at` remains the author's *default*, but the device chooses what it
  runs.

---

## 8. Phasing

1. **P-U1 — gateway active pointer + staging + manual check (online only).** The
   foundational decoupling: a core-owned active-version pointer behind a
   `RelayBackend` wrapper the gateway reads (§1); split `sync.rs` fetch/store; add
   `pending_updates`, `CheckNsiteUpdates`, `SiteStatusView.updateAvailable/
   Progress`; activate (move pointer) when complete. No propagation, no history yet.
   Delivers "check for updates" end to end.
2. **P-U2 — version history + pin/revert + app-settings window.** `SiteVersions`
   store (persisted); the per-app settings window (§6.4) replacing the Info row,
   with pin/revert (pure pointer moves — no relay change).
3. **P-U3 — open-instance deferral (R4).** `SetNsiteOpen` lifecycle + `open_hosts`
   gating; defer activation/revert under a live task; UI affordances.
4. **P-U4 — propagation (R3).** Built as the `nsite-updates` neighborhood
   extension (§4).
5. **P-U5 — background eager refresh.** Throttled foreground and periodic
   checks (§3.1). Built.

---

## 9. Open questions

- **History bound N** (§6.1) — last 3–5 versions + pinned?
- **Throttle/interval** for background checks (§3.1) — settled at 30 min
  global throttle, 6 h period. Per-site throttling is not needed: one check
  covers every site in one REQ per relay.
- **`no-store` sufficiency** for HTML across activation (§5.3).
- ~~**Manifest relay hints** (§3.2)~~ — settled: the author's NIP-65 list, plus
  `default_relays()`.
- **Per-app update policy** ([nsite-permissions.md](./nsite-permissions.md)):
  auto-apply vs. prompt; default auto (unless pinned) for v1.
