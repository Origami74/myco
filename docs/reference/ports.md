# Ports Reference

The localhost ports Myco's embedded services listen on, and **exactly how
each is (or is not) exposed over FIPS**. This is the line between the two worlds
the app straddles:

- **localhost / IPv4** — what the in-app WebView talks to. Never `.fips`.
- **`.fips` / IPv6 mesh** — what the *native* sync engine talks to, to pull a
  peer's content. Never the WebView.

Design context: [../design/nsite-layer.md](../design/nsite-layer.md) (§5,
sync-over-FIPS), [../design/nsite-layer.md §3.2](../design/nsite-layer.md)
(URL scheme). Established facts cited inline.

---

## Port table

| Service | Listen address | Default port | Reached by | Exposed over FIPS? |
| --- | --- | --- | --- | --- |
| Embedded Nostr relay | `127.0.0.1` (localhost) | **4869** | local gateway + sync engine; peer sync engines | **Yes** — at `<npub>.fips:4869` |
| Embedded Blossom server | `127.0.0.1` (localhost) | **24242** | local gateway + sync engine; peer sync engines | **Yes** — at `<npub>.fips:24242` |
| Local HTTP gateway | `127.0.0.1` (localhost) | **TBD** (e.g. 80 / 8080) | WebView only | **No** — localhost-only, never over the mesh |
| DNS interceptor | inside the VpnService/TUN reader (not a bound socket) | n/a | the whole device's resolver | n/a — it *produces* the addresses below |
| FIPS IPv6 adapter | FSP port **256** (internal mesh port, not a localhost socket) | 256 | FIPS session layer | n/a — this is the mesh transport that *carries* IPv6 to `fd00::` |

Ports 4869 (relay) and 24242 (Blossom) and the content-addressing of Blossom
blobs by sha256 are **established defaults**
([../../reference/site-deck/internal/relay/embedded.go](../../reference/site-deck/internal/relay/embedded.go),
[../../reference/site-deck/internal/blossom/embedded.go](../../reference/site-deck/internal/blossom/embedded.go)).

> **Note on bind address.** The site-deck reference binds the relay and Blossom
> on `[::]` (all interfaces) "for development" and warns against it in
> production. On a phone, Myco should bind them to **`127.0.0.1` only** — the
> mesh reaches them *through* the TUN/IPv6 adapter, not by the service binding a
> public interface. See §3.

---

## 1. Embedded Nostr relay — `4869`

- **Listen:** `ws://127.0.0.1:4869` (NIP-01 relay; also answers a NIP-11 doc on
  HTTP GET).
- **Local consumers:** the gateway queries it for manifests on the fast path;
  the loading page subscribes to it for title/description; the sync engine
  *stores* pulled manifests into it (replicating already-signed author events —
  ordinary relay behaviour, not authoring).
- **Over FIPS:** **yes.** A peer's relay is reachable at
  `ws://<npub_holder>.fips:4869`, where `<npub_holder>` is the device key of the
  peer that *holds* a copy — not the site's author. The sync engine queries that
  holder's relay for an author's site with
  `{ kinds: [15128, 35128], authors: [<author_pubkey>] }`. The site you want is
  identified by the **author** npub (the URL host); the peer you fetch it from is
  identified by the **holder's device** npub (the mesh address) — different keys.
  See
  [../design/nsite-layer.md §5.2](../design/nsite-layer.md).

## 2. Embedded Blossom server — `24242`

- **Listen:** `http://127.0.0.1:24242` (BUD-01 content-addressed blob store).
- **Local consumers:** the gateway and sync engine `GET <sha256>` for blobs; the
  sync engine `PUT`s pulled blobs in to mirror them (so this device becomes a
  source). Localhost Blossom runs **without auth** (reference behaviour).
- **Over FIPS:** **yes.** A peer's Blossom is reachable at
  `http://<npub>.fips:24242`; the sync engine pulls each manifest blob by sha256
  from there and verifies it.

## 3. Local HTTP gateway — localhost-only

- **Listen:** `http://127.0.0.1:<gateway-port>`. The WebView loads
  `http://<host>.localhost` (or `<host>.nsite`) and the request lands here.
- **Consumers:** the **WebView only**. This is the nsite-deck model: an IPv4
  localhost host works in **any** browser, including Chromium (whose AAAA/ULA
  suppression only ever affected IPv6-only `.fips`).
- **Over FIPS:** **no — deliberately.** The gateway is never exposed on the mesh.
  Peers do not talk to your gateway; they talk to your relay (4869) and Blossom
  (24242) directly. There is no separate "gateway port" on a reachable path —
  the localhost relay/Blossom *are* the mesh endpoints. The gateway is a purely
  local convenience that turns a WebView request into a cache lookup + (on miss)
  a sync.

> **Open question — gateway port.** nsite-deck listens on `:80`. On Android,
> binding `:80` needs no privilege for an app's own loopback, but a high port
> (e.g. `:8080`) avoids any ambiguity and is simpler to reason about. The
> WebView would then load `http://<host>.localhost:8080`. **TBD / open.**

## 4. DNS interceptor — not a port, a TUN-resident resolver

The DNS interceptor is **not** a bound socket on a port; it lives inside the
VpnService/TUN reader thread and inspects every DNS packet the device emits
(it sees the whole device's DNS, not just the app's). It answers two namespaces
and refuses everything else so the system falls through to normal DNS:

| Query suffix | Record | Answer | Who uses it |
| --- | --- | --- | --- |
| `*.fips` | **AAAA** | `fd00::` ULA = `fd + SHA256(npub)[0:15]` | the **native sync engine** (and any app doing `curl http://<npub>.fips:port/`) |
| `*.nsite` | **A** | `127.0.0.1` | the **WebView** (loads nsites at localhost) |
| anything else | — | **RCODE=REFUSED** | system resolver falls through to real DNS |

- The `.fips` → AAAA behaviour and the REFUSED-fallthrough are specified in
  [../../reference/fips/docs/design/fips-ipv6-adapter.md](../../reference/fips/docs/design/fips-ipv6-adapter.md).
- The `*.nsite → 127.0.0.1` A-record behaviour is the site-deck DNS server
  ([../../reference/site-deck/internal/dns/server.go](../../reference/site-deck/internal/dns/server.go)).
  Myco folds both namespaces into the one TUN-resident interceptor.
- The TUN routes only `fd00::/8` (the mesh ULA). It does **not** capture
  `0.0.0.0/0` — there is no tunnel-all-internet.

> **`.fips` is AAAA-only / IPv6; `.nsite` is A / IPv4.** This split is why the
> WebView uses `.nsite` (or `.localhost`) and never `.fips`: Chromium suppresses
> IPv6-ULA resolution for typed hostnames, so an IPv6-only `.fips` host would not
> load in the WebView, while an IPv4 `127.0.0.1` host always does.

## 5. FIPS IPv6 adapter / FSP port `256` — the mesh carrier

This is **not** a localhost port. It is the FIPS Session Protocol (FSP) port on
which the IPv6 adapter registers inside the mesh. It is the mechanism that makes
"§1 relay and §2 Blossom are reachable over FIPS" true:

1. The WebView/app stays on localhost; the sync engine resolves
   `<npub>.fips → fd00::<peer>` (AAAA, via §4).
2. It opens a normal IPv6 socket to `[fd00::<peer>]:4869` (or `:24242`).
3. The OS routes `fd00::/8` to the TUN; the TUN reader compresses the IPv6
   header and prepends an FSP port header `(src=256, dst=256)`, encrypts, and
   routes the datagram through the mesh toward the destination node.
4. On the peer, FSP delivers inbound port-256 traffic to its IPv6 adapter, which
   reconstructs the IPv6 packet and hands it to *its* TUN — landing on the peer's
   `127.0.0.1:4869` / `:24242` localhost service.

So the destination service port (4869 / 24242) rides **inside** the
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
 WebView ──IPv4─► http://<host>.nsite|.localhost ──► 127.0.0.1:<gateway>   (localhost only)
                                                          │
                                                   cache hit → serve
                                                   cache miss → sync engine ↓

 sync engine ──IPv6─► <npub>.fips → [fd00::peer]:4869  ─┐
 sync engine ──IPv6─► <npub>.fips → [fd00::peer]:24242 ─┤  routed via TUN
                                                         ▼
                                            FSP port 256 (mesh) ──► peer node
                                                         │
                                            peer TUN ──► peer 127.0.0.1:4869 / :24242
```

- The **WebView** never resolves `.fips`, never touches the mesh.
- The **sync engine** never serves to the WebView; it only fills the cache.
- The **gateway** is the only thing the WebView sees, and it is never on the
  mesh.

---

## See also

- [../design/nsite-layer.md](../design/nsite-layer.md) — the relay/Blossom/
  gateway design and the sync-over-FIPS flow.
- [./nostr-kinds.md](./nostr-kinds.md) — the manifest kinds queried on port 4869.
- [../design/propagation.md](../design/propagation.md) — propagation policy over
  these channels.
- [../../reference/fips/docs/design/fips-session-layer.md](../../reference/fips/docs/design/fips-session-layer.md),
  [../../reference/fips/docs/design/fips-ipv6-adapter.md](../../reference/fips/docs/design/fips-ipv6-adapter.md)
  — FSP port dispatch and the IPv6 adapter.
- [../../reference/fips/docs/design/fips-ipv6-adapter.md](../../reference/fips/docs/design/fips-ipv6-adapter.md)
  — the `.fips` DNS interceptor (REFUSED-fallthrough).
