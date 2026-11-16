# Ports Reference

The localhost ports Myco's embedded services listen on, and **exactly how
each is (or is not) exposed over FIPS**. This is the line between the two worlds
the app straddles:

- **localhost / IPv4** — what the in-app WebView talks to. Never `.fips`.
- **`.fips` / IPv6 mesh** — what the *native* link client talks to, to reach a
  linked peer. Never the WebView.

**One port open to other phones.** The link service on **`4873`** is the only
listener another phone can reach. Myco's relay is loopback only, and its
Blossom store is not served on any port.

**Where the mesh socket lives.** Peers reach `4873` on Myco's in-process mesh
stack, at the node's own `fd00::` address, with or without the VPN. It also
keeps a kernel socket on `[::]`, which peers never reach: it only serves this
phone's other apps dialing its own address. See
[../design/fips/mesh-stack.md](../design/fips/mesh-stack.md).

Design context: [../design/nsite/nsite-layer.md](../design/nsite/nsite-layer.md) (§5,
sync), [../design/nsite/nsite-layer.md §3.2](../design/nsite/nsite-layer.md)
(URL scheme). Established facts cited inline.

---

## Port table

| Service | Listen address | Default port | Reached by | Exposed over FIPS? |
| --- | --- | --- | --- | --- |
| Embedded Nostr relay | `127.0.0.1:4870` (loopback only) | **4870** | the in-app WebView (`ws://localhost:4870`); napplets reach the same in-process hub | **No** |
| Link service (neighborhoods) | `[::]` (mesh only) | **4873** | link setup from an invitation; linked peers (NIP-98) | **Yes** — at `<npub>.fips:4873/nh/…`. The only port open to other phones |
| Embedded Blossom store | in-process — no socket | n/a | the gateway, sync and napplets, in-process | **No** — `24243` is no longer served |
| Gateway | in-process — no socket | n/a | the app's own WebViews, via `shouldInterceptRequest` | **No** — it is not reachable at all |
| `.fips` DNS | the TUN advertises `fd00::53`; queries are lifted out of the packet stream and answered by the node's resolver | n/a | every app on the phone (the TUN is scoped to Myco's uid) | n/a — it *produces* the addresses below |
| FIPS IPv6 adapter | FSP port **256** (internal mesh port, not a localhost socket) | 256 | FIPS session layer | n/a — this is the mesh transport that *carries* IPv6 to `fd00::` |

Myco's relay listens on **4870**, one above the nsite-deck reference default
of `4869`, so Myco doesn't squat on the port a developer's own localhost relay
already uses.

Other phones reach this phone's content only through neighborhoods: the scoped
relay and blob routes under `:4873/nh/{pk}`, open to linked peers
([nap-neighbor/](../design/nap-neighbor/README.md)). Apps and updates travel
there as extensions
([extensions.md](../design/nap-neighbor/extensions.md)).

---

## 1. Embedded Nostr relay — `4870`

- **Listen:** `ws://127.0.0.1:4870` only, for this phone's WebView. Napplets
  reach the same `RelayHub` in-process.
- **Local consumers:** the gateway queries it for manifests on the fast path;
  the loading page subscribes to it for title/description; the sync engine
  *stores* pulled manifests into it (replicating already-signed author events —
  ordinary relay behaviour, not authoring).
- **Over FIPS:** **no.** Other phones never dial it. A neighbor's manifests
  arrive over the scoped neighborhood relay on `:4873` (§1a).

## 1a. Link service — `4873`

- **Listen:** `http://[::]:4873` on the mesh only (IPV6_V6ONLY, so it cannot
  collide with a loopback squatter). There is no localhost listener.
- **Routes** (`{pk}` is this side's link key for the link):

  ```text
  POST /nh/{pk}/setup          one-time secret proof     -> offer, ids on the link
  GET  /nh/{pk}/offer          NIP-98                    -> current offer
  POST /nh/{pk}/neighborhoods  NIP-98  {add, remove}     -> ids, pending
  POST /nh/{pk}/end            NIP-98                    -> 204
  GET  /nh/{pk}/relay?n=&ttl=  NIP-98, WebSocket         -> NIP-01 scoped to n
  GET  /nh/{pk}/blob/{sha256}?n=  NIP-98               -> a blob in n's scope
  ```

- **Reached by:** anyone holding an invitation (`/setup`), then linked peers,
  each request signed with their link key. Every refusal is a bare `403
  refused`. Bodies are capped at 8 KiB.

This is the only port open to other phones. Design:
[nap-neighbor/](../design/nap-neighbor/README.md).

`4873` is the first number clear of everything already spoken for: `4869` is what
public Nostr relays commonly take, `4870` is our relay, `4871` is the LAN / `!FIPS`
AP lane (and where pre-`4872` Wi-Fi Aware peers still listen), and `4872` is the
Aware lane.

Those neighbours are all UDP and this is TCP, so sharing a number would not have
collided at the socket level. It was moved anyway: a port number that means two
different things in one codebase is a trap for whoever next reads a packet
capture or a `netstat`. It is a Myco constant rather than something negotiated;
peers agree by running the same version.

See [nap-neighbor/](../design/nap-neighbor/README.md) and
[../../reference/thinning-custom-relay.md](../../reference/thinning-custom-relay.md) (D6).

## 2. Embedded Blossom store — no port

- **Listen:** nothing. `24243` is no longer served. The content-addressed blob
  store is used in-process only.
- **Local consumers:** the gateway reads blobs by sha256; the sync engine
  writes fetched blobs in after verifying them.
- **Over FIPS:** **no.** A neighbor's blobs come over the link service
  (`GET /nh/{pk}/blob/{sha256}?n=…`, §1a).

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
- **Over FIPS:** **no.** Peers do not talk to your gateway. The gateway serves
  direct from the local stores:
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
| `*.fips` | **AAAA** | `fd00::` ULA = `fd ‖ SHA256(npub)[0:15]`, answered by the **node's own resolver** (which also learns the peer's key while it is at it) | the link client; any app doing `curl http://<npub>.fips:port/` |
| anything else | as upstream | relayed to the real resolvers (`setUpstreamDns`) and the reply spliced back into the TUN | every other name the phone looks up |

- The TUN is **scoped to Myco's uid** (`addAllowedApplication`), routes only
  `fd00::/8`, and captures no other traffic. Other apps keep their normal
  internet.
- `.localhost` is not DNS: the WebView's requests never leave the process (§3).

## 5. FIPS IPv6 adapter / FSP port `256` — the mesh carrier

This is **not** a localhost port. It is the FIPS Session Protocol (FSP) port on
which the IPv6 adapter registers inside the mesh. It is the mechanism that makes
"the link service is reachable over FIPS" true:

1. The WebView/app stays on localhost; the link client resolves
   `<npub>.fips → fd00::<peer>` (AAAA, via §4).
2. It opens a normal IPv6 socket to `[fd00::<peer>]:4873`.
3. The OS routes `fd00::/8` to the TUN; the TUN reader compresses the IPv6
   header and prepends an FSP port header `(src=256, dst=256)`, encrypts, and
   routes the datagram through the mesh toward the destination node.
4. On the peer, FSP delivers inbound port-256 traffic to its IPv6 adapter, which
   reconstructs the IPv6 packet and hands it to *its* mesh stack — landing on
   the peer's `:4873` link service.

So the destination service port (4873) rides **inside** the
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

 link client ──IPv6─► <npub>.fips → [fd00::peer]:4873/nh/{pk}/…
                                                         │  in-process mesh stack
                                                         ▼
                                            FSP port 256 (mesh) ──► peer node
                                                         │
                                            peer mesh stack ──► peer :4873 link service
```

- The **WebView** never resolves `.fips`, never touches the mesh.
- The **link client** never serves to the WebView; it only fills the local
  stores.
- The **gateway** is the only thing the WebView sees, and it is not on any
  port at all.

---

## See also

- [../design/nsite/nsite-layer.md](../design/nsite/nsite-layer.md) — the relay/Blossom/
  gateway design and the sync-over-FIPS flow.
- [./nostr-kinds.md](./nostr-kinds.md) — the manifest kinds the relay holds.
- [../design/nsite/propagation.md](../design/nsite/propagation.md) — propagation policy over
  these channels.
- [../../reference/fips/docs/design/fips-session-layer.md](../../reference/fips/docs/design/fips-session-layer.md),
  [../../reference/fips/docs/design/fips-ipv6-adapter.md](../../reference/fips/docs/design/fips-ipv6-adapter.md)
  — FSP port dispatch and the IPv6 adapter.
