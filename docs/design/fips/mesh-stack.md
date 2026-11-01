# The mesh stack: Myco on the mesh without a VPN

How Myco's own sockets reach the FIPS mesh with no `VpnService`, and what the
VPN is still for.

---

## The problem

The fips node runs with an **app-owned TUN**: it hands IPv6 packets to Myco
over two channels and never opens a system TUN. Something has to turn those
packets into TCP connections.

That used to be the kernel. A `VpnService` took the packets, the kernel's TCP
terminated them, and ordinary sockets did the rest:

- the relay on `[::]:4870`
- Blossom on `[::]:24243`
- pairing on `[::]:4873`
- dials to `[fd00::peer]:…`

Android has **one** VPN slot. Myco took it for traffic that never leaves the
app, so a user with a VPN of their own had to pick one.

## The fix

The packets now end inside the app, on a userspace TCP/IP stack.

- **`fips-host::netstack`** — a [smoltcp] interface holding the node's own
  `fd00::` address. It exposes a TCP listener and TCP streams with tokio's
  `AsyncRead`/`AsyncWrite`. It has no Myco types in it.
- **`myco-core::mesh_net`** — the one seam between that stack and Myco. It
  starts the stack, hands servers a listener, dials, and runs a loopback
  SOCKS5 bridge for reqwest.

`socks_exit` already runs a userspace stack, `ipstack`. That one only
*terminates* flows other apps start, which is all an exit needs. Myco's own
services also have to *open* connections, which `ipstack` can't, so this stack
is smoltcp.

The wire doesn't change: still IPv6 over FSP port 256. A phone on this stack
and a phone on the old VPN path talk to each other as before.

[smoltcp]: https://github.com/smoltcp-rs/smoltcp

## Who gets which packet

Inbound packets from the node pass through `tun_bridge`:

1. **The stack** takes TCP to its own address on a port it claims. That is a
   port it listens on (`4870`, `4873`, `24243`), or one in its ephemeral range
   (`61001–65535`, used for its own dials). It also takes an ICMPv6
   "destination unreachable" about one of its own dials, which ends that dial
   at once.
2. **The VPN's TUN** gets everything else, if it is attached.
3. **Nothing** gets it otherwise. It is dropped, not queued.

The ephemeral range sits above the kernel's `ip_local_port_range`
(32768–60999, or 32768–61000 on older kernels). The kernel's own mesh
connections, made by other apps through the VPN, never land in it.

## Servers

Each mesh server listens on the stack, where peers reach it. The stack claims
those ports whether the VPN runs or not, so peer traffic never reaches the
kernel.

Each also keeps its old kernel socket on `[::]`. Peers don't use it; it only
serves this phone's other apps dialing its own address. Losing it to another
app is logged, not shown: peers are unaffected.

The stack hands axum the peer's `fd00::` address, so the Circle gate reads it
as before.

## Clients

- **Peer relay WebSockets** dial the stack directly (`mesh_net::connect_ws`).
- **Mesh HTTP** (reqwest) can't be given a socket of ours. Mesh clients go
  through a SOCKS5 bridge on `127.0.0.1` that dials the stack. The bridge:
  - needs a per-process secret, so no other app can use it to reach the mesh
    as this device;
  - takes resolved addresses only. Names stay with the guarded resolver in
    `http.rs`;
  - refuses anything outside `fd00::/8`;
  - gives up on a dial after 20 s, or as soon as the client hangs up.

## What the VPN is still for

"**Allow other apps to use mesh**" (`share_mesh`, off by default), and the exit
proxy. With it on, the `VpnService` TUN routes `fd00::/8` for every app and
answers `.fips` names system-wide, as before.

Refusing the VPN, or another app holding the slot, now costs only that. Myco's
own mesh keeps working.

## Holding up against peers

Any mesh node can send the stack packets, so a peer can't hold a socket
forever or take everyone's. The defaults (`fips_host::netstack::Config`):

| Limit | Default | What it stops |
| --- | --- | --- |
| Handshake timeout | 10 s | half-open handshakes filling a listener's backlog of 8 |
| Connect timeout | 15 s | dials to a peer that never answers |
| Keep-alive, then reset | every 30 s, reset after 120 s of silence | connections to peers that walked away |
| Sockets per peer | 32 | one peer taking the stack's 256 sockets |
| Reserved for dials | 64 of 256 | inbound connections leaving no socket to dial out |

## Known limits

- **Loss recovery is weaker than Linux's.** smoltcp has no SACK and a 1 s floor
  on its retransmission timeout. In host tests, 1 MB took about 60 ms with no
  loss, 3 s at 1% loss, and 70 s at 5%. BLE L2CAP is itself reliable, so
  losses at the IP level should be rare. Measure on devices.
- **TCP only.** No UDP. Of ICMP, only "destination unreachable" for its own
  dials.
- **1280-byte packets.** The stack sends at most IPv6's minimum MTU, which FIPS
  carries over any path. No path-MTU discovery.
- **No TIME-WAIT.** A closed socket is freed once its close completes. Ports
  rotate through several thousand before one is reused.

## Where this goes

`fips-host` is the start of a library for hosting FIPS in any app. Its planned
scope:

- the node's lifecycle;
- the transports;
- finding a FIPS node another app already runs, and using it;
- the system TUN.

Everything Myco does on the mesh goes through `mesh_net`, so a shared node
would change that one file.
