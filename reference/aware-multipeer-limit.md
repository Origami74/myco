# Wi-Fi Aware: why only one peer at a time

> Investigation, 2026-08-19. Diagnostics landed on
> `fix/aware-multipeer-diagnostics` (`a86abf7`); no behaviour changed.
>
> Sibling document: [aware-transport-churn.md](./aware-transport-churn.md) covers
> why a *single* data path keeps dying. This one covers why there is only ever
> one.

## The symptom

In a room with several Myco phones, Aware carries one peer. Requests for the
others come back refused in about 3ms, while Android reports seven of eight data
paths free.

Observed on a Pixel 7 Pro over an hour: 58 data paths torn down, 31 instant
refusals, and NDP requests to three different peers — 107 to one, 77 to another,
11 to a third.

## The cause

**Myco has one UDP socket per lane, and a socket can be marked with only one
`Network`.**

[`AwareRadio.kt`](../android/app/src/main/java/app/myco/aware/AwareRadio.kt)
calls `udpPin.bindTo(network)` when a data path comes up. Android routes by the
network a socket is *marked* with, so the moment a second NDP binds that socket,
the first peer stops being reachable — its data path is still up, but nothing can
reach it.

The code says so itself:

> One socket, one mark: with several concurrent NDPs the most recent one wins.
> Each NDP is a separate `Network`, so a single socket cannot serve them all.

Given that, `dataPathBlockedBy` **deliberately** refuses to request a second NDP
while one is live. The refusal is correct behaviour for the current design.

**This limit is Myco's own.** It would persist unchanged on a chipset supporting
eight concurrent data paths.

## What this is not

The comment in `liveNdps` infers that the Pixel "has exactly one `aware_data`
interface". That inference is **unverified**. It rests on refusals happening
while `AwareResources` reported slots free — which fits two readings equally
well:

- the chipset really has one interface and the framework's count is wrong, or
- slots really are free and the refusals have another cause

Nothing in the logs separates them, because the device's *supported maximum* was
never read. It is a different API from the one being logged.

## Hypotheses, one by one

Validated four, ruled out six, six need a device attached.

| # | Hypothesis | Verdict |
|---|---|---|
| 1 | Device / firmware resource limits | Unresolved — diagnostics added |
| 2 | Stale sessions or requests | Partly ruled out |
| 3 | Discovery-session lifecycle | Ruled out |
| 4 | Requests replacing or conflicting | Ruled out |
| 5 | Process-wide vs per-socket binding | **Confirmed — the cause** |
| 6 | Peer state overwritten | Ruled out |
| 7 | Server retains only one client | **Confirmed** — same root as 5 |
| 8 | Cleanup timing / rapid reconnect | Ruled out |
| 9 | Endpoint asymmetry | Needs a device |
| 10 | Wi-Fi concurrency / power | Needs a device |
| 11 | Security config / role errors | Ruled out |
| 12 | OS or vendor firmware defect | Needs a device — one hint |

### Ruled out: per-peer state (3, 4, 6, 8, 11)

**State is genuinely per-peer.** `peerIdentities`, `ndpCallbacks`, `ndpTargets`,
`liveNdps` and `retries` are all `ConcurrentHashMap`s keyed by npub — a stable
application-level ID. There is no `currentPeer`, no `currentNetwork`, no shared
socket field, and no callback reused across peers.

**Callbacks are balanced.** Each request builds its own `NetworkCallback`;
`releaseNdp` unregisters that one and no other.

**Discovery stays open.** Publish and subscribe sessions live for the node's
lifetime, so nothing is closed before a data path can be established.

**Cleanup cannot cross peers.** `deferNdpRetry` guards on `ndpTargets` and
`ndpCallbacks`, so a timeout for peer A cannot tear down peer B. `refundAttempt`
stops a blocked interface draining a peer's retry budget.

**No role or security asymmetry.** NDPs are open — no PSK, with Noise IK as the
trust layer — and both devices publish *and* subscribe, so neither is stuck in a
single role.

### Partly ruled out: stale state (2)

App-side, no leak found. `releaseNdp` removes from `liveNdps`, unregisters the
callback, and is reached from `onLost`, `onUnavailable`, and teardown.

Framework-side is a different matter. This appeared at **error** level in the
A52s log:

```text
WifiAwareNetworkFactory.releaseNetworkFor: networkRequest=… not in cache!?
```

That is the OS's own bookkeeping disagreeing with itself. It survives our
process, which makes it a candidate for refusals-with-slots-free, and repeated
force-installs during testing are exactly how it would be provoked. Unresolved.

### Needs a device (1, 9, 10, 12)

Not answerable from the code or from logs already captured. The diagnostics below
settle 1 on the next attach; 9, 10 and 12 need `dumpsys wifiaware`, both
endpoints, and a controlled run.

## Diagnostics added

`a86abf7`, no behaviour change.

**Capability separated from availability.** `availableDataPathsCount` is what is
free *now*, and it has been seen reporting 7 through a run of instant refusals.
The supported maximum comes from `Characteristics` — `getNumberOfSupportedDataPaths()`
and friends, **API 33**, above this app's `minSdk` of 29, so version-guarded.
Logged once per attach next to the availability count:

```text
event=aware_capability maxDataPaths=N maxPublish=N maxSubscribe=N (dataPaths=M pub=M sub=M)
```

**One structured line per NDP event** — `ndp_up`, `ndp_lost`,
`ndp_blocked_locally`, `ndp_refused_by_framework` — each carrying
`activeNdpCount`, `activeRequestCount`, `activeTargetCount`, `boundNetwork` and
the thread.

`boundNetwork` is there specifically because there is one socket per lane: it
answers which peer can carry traffic at all, independently of how many data paths
are up.

### What the next run decides

- `maxDataPaths=1` → hardware limit. The comment was right, the single socket is
  the correct design, and multi-peer over Aware is not available on this chipset.
- `maxDataPaths>1` with refusals → the limit is ours, and the fix below is worth
  building.

## If concurrent peers turn out to be possible

### Preferred: a pool of named UDP instances

fips already runs **named UDP transport instances**, and Myco already declares
two ([`runtime.rs:596-597`](../myco-core/src/runtime.rs#L596-L597)):

```rust
(LAN_UDP_INSTANCE.to_string(),   udp(LAN_UDP_PORT)),
(AWARE_UDP_INSTANCE.to_string(), udp(AWARE_UDP_PORT)),
```

Each is a real socket, independently pinnable, and peer addresses are already
qualified by instance. Declaring `aware0…aware3` instead of one `aware`, running
a `UdpSocketPin` per instance, and assigning each NDP a free one gives several
concurrent peers with **no proxy and no fips change**.

The constraint the code cites — fips cannot configure transports at runtime — is
real: `create_transports` builds instances from config at node start
([`mod.rs:979`](../reference/fips/src/node/mod.rs#L979)). But that only rules out
an *unbounded* number of peers. A fixed pool sidesteps it.

**Cost:** a fixed ceiling, and each idle instance holds a socket. Aware carries a
handful of nearby phones in practice, so 3–4 looks right.

### Considered and not recommended: proxying to one socket

Relay between N per-NDP sockets and the single core socket.

It works, but every packet crosses userspace twice on the lane whose entire
purpose is bulk speed. Worse, fips identifies peers by `[fe80::x%ifindex]:port`,
and proxying through localhost destroys that — so it needs a per-peer port map,
which is a userspace NAT, with its own MTU and path-MTU consequences.

More machinery than the pool, and slower.

### Not recommended: runtime transports in fips

Cleanest in principle, largest change, and upstream fips work. Only worth it if
an unbounded peer count turns out to matter.

## Next steps

1. Attach a device, read `event=aware_capability`. That decides everything below
   it.
2. If the ceiling is above 1, capture `dumpsys wifiaware` alongside a two-peer
   run to close hypotheses 9, 10 and 12.
3. Only then choose between the instance pool and leaving the design as it is.
