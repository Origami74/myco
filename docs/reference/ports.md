# Ports Reference

The localhost ports Myco's embedded services listen on, and **exactly how
each is (or is not) exposed over FIPS**. This is the line between the two worlds
the app straddles:

- **localhost / IPv4** — what the in-app WebView talks to. Never `.fips`.
- **`.fips` / IPv6 mesh** — what the *native* sync engine talks to, to pull a
  peer's content. Never the WebView.

Design context: [../design/nsite/nsite-layer.md](../design/nsite/nsite-layer.md) (§5,
sync-over-FIPS), [../design/nsite/nsite-layer.md §3.2](../design/nsite/nsite-layer.md)
(URL scheme). Established facts cited inline.

---

## Port table

| Service | Listen address | Default port | Reached by | Exposed over FIPS? |
| --- | --- | --- | --- | --- |
| Embedded Nostr relay | `127.0.0.1:4870` (loopback) **and** `[::]:4870` (mesh) — one hub, two sockets | **4870** | WebViews and napplets on loopback; Circle members over the mesh | **Yes** — at `<npub>.fips:4870`, Circle members only |
| Auth service (pairing) | `[::]` (mesh only) | **4873** | peers asking to pair | **Yes** — at `<npub>.fips:4873`, and the only port open to a peer we have never met |
| Embedded Blossom server | `[::]:24243` | **24243** | the gateway and sync in-process; Circle members over the mesh | **Yes** — at `<npub>.fips:24243`, Circle members only. Not bound at all when a custom Blossom is configured |
| Gateway | in-process — no socket | n/a | the app's own WebViews, via `shouldInterceptRequest` | **No** — it is not reachable at all |
| `.fips` DNS | the TUN advertises `fd00::53`; queries are lifted out of the packet stream and answered by the node's resolver | n/a | every app on the phone (the TUN is scoped to Myco's uid) | n/a — it *produces* the addresses below |
| FIPS IPv6 adapter | FSP port **256** (internal mesh port, not a localhost socket) | 256 | FIPS session layer | n/a — this is the mesh transport that *carries* IPv6 to `fd00::` |

Myco's relay listens on **4870** and Blossom on **24243** — each **one above**
the nsite-deck reference defaults of `4869` / `24242`, so Myco doesn't squat
on the ports a developer's own localhost relay/Blossom already use. The mesh
and loopback binds use the *same* number, so a peer dialling `<npub>.fips:4870`
lands on the same hub a WebView reaches at `127.0.0.1:4870`.

> **Deprecated — the mesh ports.** Reaching a phone's relay at
> `<npub>.fips:4870` and its Blossom at `<npub>.fips:24243` is how Circle
> members sync today, and it is **going away**. Both mesh listeners will be
> removed in a future version; what replaces them is a channel owned by the
> Circle layer ([circle.md](../design/circle/circle.md)), so that the relay and
> store are never a socket a peer dials. The loopback listeners stay. Until
> then the rows above are accurate; do not build anything new on the `.fips`
> ports.
>
> **Bind address.** The relay hub serves two listeners — loopback for this
> phone's WebViews and napplets, `[::]` for the mesh — and the Blossom server
> one on `[::]`. Reaching either over the mesh still requires a Circle
> membership: the gate is checked on every mesh connection, and loopback bypasses
> it. See §1 and [nsite-permissions.md](../design/nsite/nsite-permissions.md).

---

## 1. Embedded Nostr relay — `4870`

- **Listen:** `ws://127.0.0.1:4870` for this phone's WebViews and napplets,
  `ws://[::]:4870` for the mesh. One `RelayHub` behind both.
- **Local consumers:** the gateway queries it for manifests on the fast path;
  the loading page subscribes to it for title/description; the sync engine
  *stores* pulled manifests into it (replicating already-signed author events —
  ordinary relay behaviour, not authoring).
- **Over FIPS:** **yes.** A peer's relay is reachable at
  `ws://<npub_holder>.fips:4870`, where `<npub_holder>` is the device key of the
  peer that *holds* a copy — not the site's author. The sync engine queries that
  holder's relay for an author's site with
  `{ kinds: [15128, 35128], authors: [<author_pubkey>] }`. The site you want is
  identified by the **author** npub (the URL host); the peer you fetch it from is
  identified by the **holder's device** npub (the mesh address) — different keys.
  See
  [../design/nsite/nsite-layer.md §5.2](../design/nsite/nsite-layer.md).

## 1a. Auth service — `4873`

- **Listen:** `http://[::]:4873` on the mesh only (IPV6_V6ONLY, so it cannot
  collide with a loopback squatter). There is no localhost listener: nothing on
  this device needs to pair with itself.
- **Route:** `POST /pair`, body a signed pairing event (kinds 9101 / 9102 /
  9103). Answers `200 paired`, `202 pending` (a request waiting on the user), or
  `403 declined`.
- **Reached by:** any peer, paired or not. This is deliberate and it is the only
  such port — pairing is what *creates* the circle that gates everything else,
  so it cannot itself require membership.

Because bootstrap lives here, the relay and Blossom ports have **no exceptions**:
an unpaired peer is refused before the WebSocket upgrade, and the relay rejects
the pairing kinds from every source so they never reach the event store.

`4873` is the first number clear of everything already spoken for: `4869` is what
public Nostr relays commonly take, `4870` is our relay, `4871` is the LAN / `!FIPS`
AP lane (and where pre-`4872` Wi-Fi Aware peers still listen), and `4872` is the
Aware lane.

Those neighbours are all UDP and this is TCP, so sharing a number would not have
collided at the socket level. It was moved anyway: a port number that means two
different things in one codebase is a trap for whoever next reads a packet
capture or a `netstat`. It is a Myco constant rather than something negotiated;
peers agree by running the same version.

See [../design/core/identity-pairing.md](../design/core/identity-pairing.md) and
[../../reference/thinning-custom-relay.md](../../reference/thinning-custom-relay.md) (D6).

## 2. Embedded Blossom server — `24243`

- **Listen:** `http://127.0.0.1:24243` (BUD-01 content-addressed blob store).
- **Local consumers:** the gateway and sync engine `GET <sha256>` for blobs; the
  sync engine `PUT`s pulled blobs in to mirror them (so this device becomes a
  source). Localhost Blossom runs **without auth** (reference behaviour).
- **Over FIPS:** **yes.** A peer's Blossom is reachable at
  `http://<npub>.fips:24243`; the sync engine pulls each manifest blob by sha256
  from there and verifies it.

## 3. Gateway — in-process, no port

- **Listen:** nothing. An nsite's WebView loads `http://<host>.localhost/` and
  every request it makes is intercepted (`WebViewClient.shouldInterceptRequest`)
  and answered by `NativeCore.gatewayGet` — a JNI call into the gateway in
  `nsite-deck`, which resolves host → manifest → path → sha256 → bytes from the
  local relay and Blossom. No socket is bound, no DNS is consulted, and the TUN
  is not involved; serving works with the VPN off.
- **Why `.localhost`:** Chromium treats `*.localhost` as loopback and a secure
  context, which is what lets a page open `ws://localhost:4870` to the relay.
  The design first had a bound gateway on `:80` behind a system-wide
  `*.nsite → 127.0.0.1` interceptor so that any browser could load a site; that
  path was dropped with the `.nsite` TLD. Browsers outside the app cannot reach
  a site today (the NAT46 / external-browser item on the roadmap).
- **Over FIPS:** **no.** Peers do not talk to your gateway; they talk to your
  relay (4870) and Blossom (24243). The gateway serves direct from those stores:
  manifest from the relay, `<path> → sha256`, blob from Blossom, content-type
  from the extension. There is no derived htdocs cache; the content-addressed
  store is the only retained store.

## 4. `.fips` DNS — a sentinel resolver on the TUN

There is no DNS server socket. The `VpnService` advertises **`fd00::53`** as the
only resolver on the tunnel — an address inside the routed `fd00::/8` that is not
the node's own — so the OS resolver's query packets are handed to the TUN pump.
`dns_intercept.rs` lifts each query out of the packet stream:

| Query suffix | Record | Answer | Who uses it |
| --- | --- | --- | --- |
| `*.fips` | **AAAA** | `fd00::` ULA = `fd ‖ SHA256(npub)[0:15]`, answered by the **node's own resolver** (which also learns the peer's key while it is at it) | sync to Circle members; any app doing `curl http://<npub>.fips:port/` |
| anything else | as upstream | relayed to the real resolvers (`setUpstreamDns`) and the reply spliced back into the TUN | every other name the phone looks up |

- The TUN is **scoped to Myco's uid** (`addAllowedApplication`), routes only
  `fd00::/8`, and captures no other traffic. Other apps keep their normal
  internet.
- `.localhost` is not DNS: the WebView's requests never leave the process (§3).

## 5. FIPS IPv6 adapter / FSP port `256` — the mesh carrier

This is **not** a localhost port. It is the FIPS Session Protocol (FSP) port on
which the IPv6 adapter registers inside the mesh. It is the mechanism that makes
"§1 relay and §2 Blossom are reachable over FIPS" true:

1. The WebView/app stays on localhost; the sync engine resolves
   `<npub>.fips → fd00::<peer>` (AAAA, via §4).
2. It opens a normal IPv6 socket to `[fd00::<peer>]:4870` (or `:24243`).
3. The OS routes `fd00::/8` to the TUN; the TUN reader compresses the IPv6
   header and prepends an FSP port header `(src=256, dst=256)`, encrypts, and
   routes the datagram through the mesh toward the destination node.
4. On the peer, FSP delivers inbound port-256 traffic to its IPv6 adapter, which
   reconstructs the IPv6 packet and hands it to *its* TUN — landing on the peer's
   `127.0.0.1:4870` / `:24243` localhost service.

So the destination service port (4870 / 24243) rides **inside** the
mesh-tunneled IPv6/TCP packet; FSP port **256** is only the outer mesh
multiplexing port shared by all IPv6 traffic. Sources:
[../../reference/fips/docs/design/fips-session-layer.md](../../reference/fips/docs/design/fips-session-layer.md)
("Port-Based Service Dispatch… IPv6 adapter runs on port 256"),
[../../reference/fips/docs/design/fips-ipv6-adapter.md](../../reference/fips/docs/design/fips-ipv6-adapter.md)
("Prepend port header (src_port=256, dst_port=256)" / "Inbound mesh traffic on
port 256… delivered as complete IPv6 packets"). This is the FIPS IPv6 adapter
delivering `curl http://<npub>.fips:port/` end to end
([../../reference/fips/docs/design/fips-ipv6-adapter.md](../../reference/fips/docs/design/fips-ipv6-adapter.md)).

---

## The two worlds, in one picture

```
 WebView ──► http://<host>.localhost ──► shouldInterceptRequest ──► gateway (in-process)
                                                                         │
                                                                  cache hit → serve
                                                                  cache miss → sync ↓

 sync engine ──IPv6─► <npub>.fips → [fd00::peer]:4870  ─┐
 sync engine ──IPv6─► <npub>.fips → [fd00::peer]:24243 ─┤  routed via TUN
                                                         ▼
                                            FSP port 256 (mesh) ──► peer node
                                                         │
                                            peer TUN ──► peer 127.0.0.1:4870 / :24243
```

- The **WebView** never resolves `.fips`, never touches the mesh.
- The **sync engine** never serves to the WebView; it only fills the cache.
- The **gateway** is the only thing the WebView sees, and it is not on any
  port at all.

---

## See also

- [../design/nsite/nsite-layer.md](../design/nsite/nsite-layer.md) — the relay/Blossom/
  gateway design and the sync-over-FIPS flow.
- [./nostr-kinds.md](./nostr-kinds.md) — the manifest kinds queried on port 4870.
- [../design/nsite/propagation.md](../design/nsite/propagation.md) — propagation policy over
  these channels.
- [../../reference/fips/docs/design/fips-session-layer.md](../../reference/fips/docs/design/fips-session-layer.md),
  [../../reference/fips/docs/design/fips-ipv6-adapter.md](../../reference/fips/docs/design/fips-ipv6-adapter.md)
  — FSP port dispatch and the IPv6 adapter.
