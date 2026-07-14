# nsite updates: staged activation + mesh propagation

> Status: DESIGN / proposal for a not-yet-built feature. Voice is "the app
> will…". Open questions are marked **TBD / open**.

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
4. **propagates** it to mesh peers using the *same* push plane as ordinary app
   events ([event-gossip.md](./event-gossip.md)).

> **Scope.** Core/native work in `myco-core` + `myco-relay`, plus a small
> lifecycle signal from the Android shell. The WebView is unaware any of this
> exists — it always sees one consistent version for the life of its task.

---

## 1. The load-bearing conflict

The embedded relay collapses replaceable manifests to **newest-by-`created_at`**
([`admit`/`slot_of`](../../myco-relay/src/lib.rs)), and today the gateway serves
whatever manifest is newest in the store
([`gateway::readiness` → `get_manifest`](../../nsite-deck/src/gateway.rs)). If we
left that coupling in place, a newer manifest landing in the store would
**activate instantly**: its blobs aren't local yet, so `readiness()` returns
`Incomplete` and the gateway flips a *working* app to its 503 loading page — and
it would swap even under a live WebView session.

**We do not fix this by holding manifests back from the relay.** The relay must
stay a faithful NIP-01 store: a newer replaceable manifest **replaces immediately**
and is **served to and propagated to peers** like any other event. Withholding new
versions to protect our own serving would make us lie to the mesh and stall
propagation — exactly what we don't want.

**The fix is to decouple *local serving* from the relay store.** Two independent
views over a site's manifests:

- **Relay store (the mesh view).** Pure NIP-01 — newest replaceable wins,
  immediately, and is what we answer peers' REQs with and propagate (§4). Older
  versions it replaces are **persisted separately** by core (the version history,
  §6) before they fall out of the store.
- **Gateway active pointer (the local view).** A core-owned choice of *which*
  version this device actually **serves to its own WebView**. It advances to a
  newer version only once that version's blobs are fully local (R2) and no
  instance is open (R4); the user can also pin/revert it (§6).

The gateway reads the **active pointer**, not relay-newest — implemented as a thin
[`RelayBackend`](../../nsite-deck/src/seams.rs) wrapper whose `get_manifest`
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

enum UpdateSource { Online, Mesh(IpAddr) }  // online Blossom, or the peer that surfaced it
```

The candidate manifest **is** in the relay store (we don't withhold it); it just
isn't the active pointer yet. Blobs go straight into Blossom as they verify —
content-addressed, so the new version's blobs coexist with the active version's and
any shared blob is already present (free dedup). The active pointer keeps the
gateway serving the working version with zero disruption until the download
completes.

Staging reuses the blob-fetch half of [`sync.rs`](../../nsite-deck/src/sync.rs);
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
- **Push (a peer tells us).** Because manifests propagate over the *same*
  `event-ttl` push plane as chat (§4), a peer can gossip a newer manifest to us
  **unsolicited, up to 3 hops** away — we don't have to ask. It arrives over the
  EVENT socket, our relay stores it (NIP-01) and re-propagates it, and core
  notices it exactly as it would a polled candidate (§4.2). So a device often
  learns of an update from a nearby peer before it ever runs a check — and even
  fully offline.

The rest of §3 covers the pull channel; the push channel is §4.

### 3.1 Pull trigger

Two triggers, both feeding the same staging pipeline:

- **Manual.** A "Check for updates" affordance (app long-press sheet + a Settings
  entry) dispatches `CheckNsiteUpdates`.
- **Background.** A throttled check when the app comes to the foreground (and on
  a long interval while foregrounded), realising nsite-layer.md §5.4's "eager
  refresh of pinned sites". Throttle: at most once per site per **TBD: ~1 h**;
  never blocks the UI (spawn-not-block, like discovery).

### 3.2 Check = one bounded subscription per relay

`CheckNsiteUpdates` is **not** a per-site, per-relay fan of fetches. It is a set of
**REQ subscriptions that run until EOSE** against a *deduplicated* relay set, with
one combined filter per relay covering every site we care about. The relay set is
the union of:

- **Relays around us** — the embedded relays of connected mesh peers
  (`ws://[fd00::peer]:4870`), always reachable offline. (Reuse the existing
  peer-relay pool's live connections where possible rather than dialing afresh.)
- **The manifests' listed relays** — each active manifest's own relay hints, used
  **only when online**. The author publishes updates there, so they are the
  authoritative poll target.

**Deduplicate by URL** so overlapping relay lists across many sites collapse to a
single connection. For each unique relay, send **one** filter union'd across all
sites — `{ kinds: [15128, 35128], authors: [...all site authors...], "#d":
[...all named d-tags...] }` — then read events until **EOSE** and close. (One
combined REQ, not one per site; the relay returns each slot's newest. This is the
read/pull shape from [event-gossip.md §7](./event-gossip.md), bounded by EOSE
rather than kept live.)

A returned manifest is a candidate iff `candidate.created_at > active.created_at`
for its slot (and newer than any already-staged candidate). Each such candidate is
**stored into our own relay** (`store_event`) — which (a) keeps us NIP-01-faithful
so peers REQ-ing us get the newest, (b) propagates it onward via the normal
gossip path (§4), and (c) lets core notice it and open a `PendingUpdate` to stage
its blobs (preferring the manifest's `["server", …]` hints, then
`default_blossom_servers()` when online, or the mesh peer that surfaced it). The
active pointer does not move until staging completes (§5).

> **TBD / open:** where a manifest's relay hints live (NIP-65 author relay list
> vs. tags on the manifest event) and the fallback when a manifest lists none
> (probably `default_relays()`). See §9.

### 3.3 Surfacing

`SiteStatusView` gains:

- `updateAvailable: bool` — a staged-and-ready newer version exists but isn't yet
  active (deferred by an open instance, §5).
- `updateProgress: (pulled, total)` — while a staged update is downloading.

The Apps grid shows an "update" affordance on tiles with `updateAvailable`; the
long-press sheet offers "Update now" (a no-op nudge if it'll auto-apply on close).

---

## 4. Propagation over the mesh (R3)

> **Rule:** **storing** a newer manifest is always immediate and
> protocol-faithful — it enters our relay at once and is answerable to peers'
> REQs; we never withhold a version. **Forwarding** (the active `event-ttl` push to
> peers) is *interest-aware*:
> - **Not interested** (we don't have this nsite in our Library): forward
>   **immediately** — a pure relay passthrough; we won't download the blobs, so
>   there's nothing to wait for.
> - **Interested** (it's one of our installed sites): **best-effort download the
>   blobs first, then forward**, so the next hop can pull the bytes from us. This is
>   bounded — a slow/failed download still forwards eventually, never stalling the
>   wave.
>
> **Activation** (what we serve locally) is separate again, gated on *all* blobs
> being local (§5). So: never withhold a version, never serve one we can't back,
> and be a useful blob source for sites we actually run.

### 4.1 Manifests gossip like any event — with an interest check on forward

The relay change is just making manifest kinds **gossip-eligible**
([`is_gossip_eligible`](../../myco-core/src/gossip.rs)); the relay still merely
**stores** the newer manifest and hands it to the gossiper, exactly as for chat
(no interceptor, no deferral in the relay). The *forward policy* lives in core's
gossiper, which branches on interest:

- **Not interested** → build `["EVENT", { …manifest, "event-ttl": n }]` and fan to
  connected circle peers immediately, split-horizon, `event-ttl` decremented
  ([event-gossip.md §2–3](./event-gossip.md)).
- **Interested** → open a `PendingUpdate { source: Mesh(sender_ip) }`, download
  the blobs (best-effort, from the peer that sent it), and fan out when the
  download completes **or** a bound elapses — whichever first. Then activate (§5)
  if/when complete.

The loop guard is the same as chat: only a *newly-stored* (newer-than-known)
manifest is handed to the gossiper, so duplicates and older versions die on
arrival and `event-ttl` bounds the wave. Locally-originated candidates (an online
check, §3.2) are "interested" by definition — we checked because we run the site —
so they follow the download-then-forward path.

### 4.2 Blob availability is the natural gate

Forwarding the manifest never implies we can serve its blobs — the two travel on
different channels, so even the immediate (not-interested) forward is safe:

- The **manifest event** propagates per §4.1; peers learn the version exists.
- The **blobs** are only ever served from our Blossom *if we actually hold them*.
  A peer pulling a blob we don't have simply gets a 404 from us and falls back to
  its other sources — the manifest's `["server", …]` hints, or another peer who
  has it ([sync.rs](../../nsite-deck/src/sync.rs) already tries sources in order).

The interested-path download-first (§4.1) is therefore an **optimisation**, not a
correctness requirement: it makes us a ready blob source for the sites we run, so
the next hop usually succeeds against us instead of falling back.

### 4.3 Why reuse the push plane

Manifests *are* just events. Reusing `event-ttl` push + neighbour pull means an
offline cluster gets author updates the same way it gets chat: whoever next meets a
peer that holds the newer manifest gets it and re-spreads it, then each device
downloads the blobs and activates on its own schedule. No separate
update-distribution protocol, and no special-casing in the relay.

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
until it closes. We do **not** re-propagate a revert over the mesh (it's a local,
per-device choice; the author's newest is still what spreads).

### 6.4 UI — the per-app settings window

Today the long-press app sheet's **Info** row is a no-op
([AppsScreen.kt](../../android/app/src/main/java/app/myco/ui/screens/AppsScreen.kt)).
It becomes **"App settings"** — a per-app window that hosts:

- the site identity (host / npub) Info used to show,
- **Check for updates** (manual trigger; shows progress + `updateAvailable`),
- **Version** — the active version with its date, a **Pin** toggle, and the
  **history list**; tapping an older version offers **Revert** (which pins),
- (later) per-app update policy + gossip-kinds ([nsite-permissions.md](./nsite-permissions.md)).

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
   Progress`; activate (move pointer) when complete. No gossip, no history yet.
   Delivers "check for updates" end to end.
2. **P-U2 — version history + pin/revert + app-settings window.** `SiteVersions`
   store (persisted); the per-app settings window (§6.4) replacing the Info row,
   with pin/revert (pure pointer moves — no relay change).
3. **P-U3 — open-instance deferral (R4).** `SetNsiteOpen` lifecycle + `open_hosts`
   gating; defer activation/revert under a live task; UI affordances.
4. **P-U4 — mesh propagation (R3).** Make manifest kinds gossip-eligible; the
   gossiper applies the interest-aware forward policy (§4.1) — not-interested
   forwards immediately, interested downloads-then-forwards (bounded) and activates
   on completion. No relay interceptor.
5. **P-U5 — background eager refresh.** Throttled foreground checks.

---

## 9. Open questions

- **History bound N** (§6.1) — last 3–5 versions + pinned?
- **Interested-forward bound** (§4.1) — how long to best-effort download blobs
  before forwarding the manifest anyway (so the wave never stalls)?
- **Throttle/interval** for background checks (§3.1) — start ~1 h/site?
- **`no-store` sufficiency** for HTML across activation (§5.3).
- **Manifest relay hints** (§3.2) — where the author's update relays come from
  (NIP-65 list vs. tags on the manifest), and the fallback when none are listed
  (`default_relays()`?).
- **Per-app update policy** ([nsite-permissions.md](./nsite-permissions.md)):
  auto-apply vs. prompt; default auto (unless pinned) for v1.
