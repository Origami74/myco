# Phase 1: Make Peering Observable - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-08-04
**Phase:** 1-Make Peering Observable
**Areas discussed:** Where the peer view lives, What counts as one peer row, Failure forensics depth, Liveness and polling cost

---

## Where the peer view lives

### Q1 — Where does the user-facing peer list live?

| Option | Description | Selected |
|--------|-------------|----------|
| Grow the Circle tab (Recommended) | Circle already lists paired contacts and badges pending pair requests; peer state joins the row it belongs to. No nav slot spent. Cost: CircleScreen.kt is already 584 lines and would mix pairing with diagnostics. | |
| New dedicated Peers tab | Sixth destination. Circle stays about pairing; Peers is about connection health. Cost: a nav slot and a screen from scratch. | |
| Dev tab only | Everything behind developerMode. Fastest, zero risk to normal UX. Cost: DIAG requirements are worded "User can see". | ✓ |

**User's choice:** Dev tab only
**Notes:** Prompted an immediate follow-up on the requirement wording — see Q2.

### Q2 — How should "User can see" be read, given the Dev tab gate?

| Option | Description | Selected |
|--------|-------------|----------|
| Dev mode is the answer — record it | Read "user" as "the person holding the phone, without a debugger". developerMode is a normal Settings toggle, so the requirement is met. Recorded as a locked decision. | ✓ |
| Dev tab now, promote later | Build in Dev for speed; note promoting the user-facing subset into Circle as Phase 2/3 follow-up. | |
| Split it: thin in Circle, deep in Dev | Circle rows get plain facts; Dev gets role, latency, drops, raw radio state. | |

**User's choice:** Dev mode is the answer — record it
**Notes:** Locked as D-02 so the coverage gate and verifier do not re-open it.

### Q3 — How does peer diagnostics fit into the existing ~380-line DevScreen?

| Option | Description | Selected |
|--------|-------------|----------|
| Sub-screen off the Dev tab (Recommended) | Own composable, own file; keeps DevScreen from becoming 900 lines. | |
| New section inside DevScreen | Another collapsible section on the existing screen. | |
| Replace DevScreen's content | Peering becomes what the Dev tab is for this milestone. Cost: churns working tooling on demo day. | ✓ |

**User's choice:** Replace DevScreen's content
**Notes:** Mitigated by the discovery that DevScreen already carries `PeersOverviewCard`, BLE, WI-FI AWARE and RADIO ADVERTS cards plus reusable `DevCard`/`KeyValDot`/`PeerRow`/`AdvertRow` — so "replace" is really "rebuild around peering, reusing the parts".

### Q4 — What happens to SPEEDTEST and CACHE?

| Option | Description | Selected |
|--------|-------------|----------|
| Keep both, demote below peering (Recommended) | Nothing working gets deleted on demo day. | |
| Keep SPEEDTEST, drop CACHE | Speedtest measures a live peer link so it belongs; CACHE is content-layer state irrelevant to Phase 1. | ✓ |
| Drop both | Dev tab becomes purely peering. | |

**User's choice:** Keep SPEEDTEST, drop CACHE

### Q5 — How is deep per-peer detail reached?

| Option | Description | Selected |
|--------|-------------|----------|
| Expand in place (Recommended) | Summary line expands inline to role, latency, drops, RSSI. Good for comparing two phones side by side. | ✓ |
| Tap through to a peer detail screen | List stays scannable; one peer gets a full screen. Cost: loses at-a-glance comparison. | |
| Flat — everything always visible | No interaction. Cost: a wall of text past two or three peers. | |

**User's choice:** Expand in place

### Q6 — Should developerMode default to on?

| Option | Description | Selected |
|--------|-------------|----------|
| Leave the default off | No behaviour change; flip once per demo device. | |
| Default on for this milestone (Recommended) | Field testers reporting "zero peers" could read diagnostics without being talked through Settings. | |
| Default on only in debug builds | Release APK unchanged, debug/demo builds get it automatically. Cost: the zapstore release users install stays gated. | ✓ |

**User's choice:** Default on only in debug builds
**Notes:** Creates a recorded tension with D-02 for release-build users; captured in CONTEXT.md D-06 rather than resolved.

### Q7 — What does the screen show at zero peers?

| Option | Description | Selected |
|--------|-------------|----------|
| Radio self-check, then empty list (Recommended) | Leads with what this device is doing — BLE and Aware enabled/scanning, recent adverts. Answers "is it me or is it them". | ✓ |
| Plain empty message | "No peers yet"; radio cards below carry detail. | |
| Show discovered-but-unconnected devices | Surface raw adverts and Aware links as proof something is out there. | |

**User's choice:** Radio self-check, then empty list

---

## What counts as one peer row

### Q1 — What is one row?

| Option | Description | Selected |
|--------|-------------|----------|
| One row per npub, transports as attributes (Recommended) | A peer is an identity. Same pubkey-keyed model Phase 2 needs for MAC-rotation survival (PEER-03, FIPS#130). | ✓ |
| One row per transport link | Closest to what transports report today. Cost: entrenches the address-keyed model Phase 2 must replace. | |
| Grouped — npub header, transport children | Most literal view. Cost: densest layout, real grouping work. | |

**User's choice:** One row per npub, transports as attributes

### Q2 — Do devices with no resolved npub get a row?

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, as unidentified rows (Recommended) | Keyed by node address, marked not-yet-identified, collapsing into the npub row on resolve. "Seen, never resolved" is the observation Phase 2 needs. | ✓ |
| No — npub-resolved peers only | Clean model, no merge logic. Cost: the most diagnostic state of all is not a peer row. | |
| Yes, but in a separate section | Two lists; devices visibly hop between them. | |

**User's choice:** Yes, as unidentified rows

### Q3 — What is the state vocabulary?

| Option | Description | Selected |
|--------|-------------|----------|
| Five states, reusing existing colours (Recommended) | connected / reachable-via-relay / seen-unidentified / paired-offline / unreachable, mapped onto existing Theme.kt constants. Superset of DIAG-01. | ✓ |
| Exactly DIAG-01's three | Smallest vocabulary. Cost: collapses the distinction Phase 2 needs. | |
| Three states plus a separate qualifier | Orthogonal flags. Cost: a row's meaning read from a combination, slower at arm's length. | |

**User's choice:** Five states, reusing existing colours
**Notes:** No new colour constants, so `ThemeTest.kt` stays green.

### Q4 — How is the list ordered?

| Option | Description | Selected |
|--------|-------------|----------|
| By state, then most recently heard (Recommended) | Connected first, down to unreachable; each group by last-seen. A peer visibly climbs as it comes good. | ✓ |
| Most recently heard from, flat | Reads as a live activity feed. Cost: connected peers sink below noisy adverts. | |
| Stable order, state as a badge only | Rows never move. Cost: must scan the whole list to answer "is anything connected". | |

**User's choice:** By state, then most recently heard

---

## Failure forensics depth

### Q1 — How much history is kept?

| Option | Description | Selected |
|--------|-------------|----------|
| Rolling per-peer attempt log, capped (Recommended) | Last N attempts per peer with timestamp, role, discovery duration, outcome. The artefact that confirms or kills the tiebreaker-race hypothesis. | ✓ |
| Current values only | Cheapest. Cost: a race that resolved thirty seconds ago leaves no trace. | |
| Global event log, not per-peer | Best for seeing interleaving. Cost: harder to answer "what happened with this one phone". | |

**User's choice:** Rolling per-peer attempt log, capped

### Q2 — Does the log survive restart?

| Option | Description | Selected |
|--------|-------------|----------|
| In-memory only (Recommended) | No disk schema, no corruption path — CORE-03 exists because corrupt JSON already bites this app. | |
| Persist to disk | Survives crash or force-stop during a field test. Cost: new on-disk format on demo day, plus a corruption path. | ✓ |
| In-memory, with explicit export | Durability on demand. Cost: someone must remember to press it. | |

**User's choice:** Persist to disk
**Notes:** Risk explicitly surfaced in the option text and accepted. CONTEXT.md D-13 records the mitigation requirement: bounded, corruption-tolerant read path; a malformed log must degrade to "no history", never take down startup or destroy a good file.

### Q3 — What bounds the on-disk log?

| Option | Description | Selected |
|--------|-------------|----------|
| Cap per peer, prune on write (Recommended) | N-per-peer ring flushed to disk; long-unseen peers dropped. Size bounded by peer count. | ✓ |
| Single capped file, oldest-first rotation | Simplest format, crash-tolerant append. Cost: per-peer queries mean scanning. | |
| Time-window retention | Matches how you would ask the question. Cost: unbounded size in a busy room. | |

**User's choice:** Cap per peer, prune on write

### Q4 — How does the log get off the phone?

| Option | Description | Selected |
|--------|-------------|----------|
| Read on screen, no export (Recommended) | What "without attaching a debugger" asks for, nothing more. | ✓ |
| Share sheet with the peer's log | Turns a field report into evidence. Cost: a share intent and text formatter. | |
| Copy to clipboard | Cheapest way to get data off screen. Cost: Android clipboard is lossy across apps. | |

**User's choice:** Read on screen, no export

---

## Liveness and polling cost

### Q1 — How does the screen refresh?

| Option | Description | Selected |
|--------|-------------|----------|
| Own slower cadence for diagnostics (Recommended) | ~1s, separate from the UI tick. Order-of-magnitude less lock pressure; does not pre-empt UX-02. | ✓ |
| Ride the existing tick | Nothing new to build. Cost: adds the densest consumer to the polling path already flagged as a concern. | |
| Push from the core on change | Right long-term answer. Cost: a new FFI direction on demo day — the JNI boundary is poll-only. | |

**User's choice:** Own slower cadence for diagnostics
**Notes:** Push-based updates deferred to Phase 3 alongside UX-02.

### Q2 — How is last-seen displayed?

| Option | Description | Selected |
|--------|-------------|----------|
| Exact seconds, counting up (Recommended) | `3s`, `47s`, `4m 12s`. Watch it reset the instant a peer is heard from. | ✓ |
| Coarse buckets | Reads faster at arm's length. Cost: hides the 2s-vs-25s difference a duty-cycle asymmetry shows up at. | |
| Absolute timestamp | Lines up with logcat. Cost: arithmetic every time. | |

**User's choice:** Exact seconds, counting up

### Q3 — Does polling run when the Dev tab is off screen?

| Option | Description | Selected |
|--------|-------------|----------|
| Only while the screen is visible (Recommended) | Starts on show, stops on leave or background. Core instrumentation keeps recording regardless. | ✓ |
| Always while the app is foregrounded | No lifecycle wiring. Cost: pays the lock-pressure price on every other tab. | |
| Keep polling in the background too | State warm on return. Cost: battery and lock pressure for nothing. | |

**User's choice:** Only while the screen is visible

### Q4 — How do the new fields cross the JNI/JSON boundary?

| Option | Description | Selected |
|--------|-------------|----------|
| New peers array on AppState (Recommended) | npub-keyed merged view; merge happens once in Rust, not re-derived in Kotlin per frame. Becomes the surface Phase 2 reads. | ✓ |
| Extend the existing lists in place | Smallest diff to state.rs. Cost: UI still stitches three lists per poll; address-vs-npub mismatch unresolved. | |
| Separate diagnostics action | Best isolation. Cost: a second state path to keep coherent. | |

**User's choice:** New peers array on AppState

---

## Claude's Discretion

Recorded in CONTEXT.md `<decisions>` → "Claude's Discretion". Not discussed; planner
chooses consistently with the locked decisions:

- Exact field set and Rust type of an attempt record; what bounds one "attempt"
- Whether the attempt log rides the `state()` payload or is fetched on row expand
- On-disk encoding of the persisted log (subject to D-13 corruption tolerance)
- Value of N in the per-peer cap, and the unseen-peer eviction threshold
- First-frame behaviour before the first poll; row transition animation
- Collapsed summary line composition; how npub and Circle name render together
- Where the Wi-Fi Aware "actively scanning" signal is sourced from — `WifiAwareStatus`
  exposes only `enabled` and `port` today, so DIAG-05 needs a new field

## Deferred Ideas

- Promoting the user-facing subset into the Circle tab for users who never enable
  `developerMode` — revisit if field reports show release users cannot reach it
- Exporting an attempt log off the device (share sheet or clipboard) — natural home is
  alongside Phase 2's reason codes
- Push-based state updates from the core instead of polling — belongs with Phase 3's
  UX-02 work; needs a new FFI direction
- Interactive force-directed mesh topology graph — already out of scope in
  REQUIREMENTS.md
- Making `CircleContact.name` robust beyond what DIAG-07 needs — the field is commented
  "a placeholder for now"
