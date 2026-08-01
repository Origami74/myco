# External Integrations

**Analysis Date:** 2026-08-01

## APIs & External Services

**Public Nostr Relays (IP Fallback - Optional):**
- **damus.io** (`wss://relay.damus.io`) - Public relay for nsite manifest discovery over IP
- **nos.lol** (`wss://nos.lol`) - Public relay fallback
- **relay.nostr.band** (`wss://relay.nostr.band`) - Public relay with NIP-50 search support
- **relay.primal.net** (`wss://relay.primal.net`) - Public relay by Primal
- **purplepag.es** (`wss://purplepag.es`) - Public relay
  - SDK/Client: `tokio-tungstenite` (WebSocket), `nostr` crate (event validation)
  - Auth: None (Nostr relays are permissionless) — all data is author-signed, relay-agnostic
  - **Purpose**: Side-load nsite links over normal IP internet; mesh uses embedded relay instead
  - **Configuration**: `myco-core/src/ip_source.rs:default_relays()` — defaults are hardcoded, overridable at runtime

**Public Blossom Blob Servers (IP Fallback - Optional):**
- **blossom.primal.net** (`https://blossom.primal.net`) - Public Blossom server for blob storage
- **cdn.satellite.earth** (`https://cdn.satellite.earth`) - Public Blossom CDN
- **blossom.band** (`https://blossom.band`) - Public Blossom server
  - SDK/Client: `reqwest` (HTTP with `rustls-tls`, 60s timeout for blob transfers)
  - Auth: None (Blossom is permissionless) — blobs referenced by content hash
  - **Purpose**: Fetch nsite app files (HTML, CSS, JS, assets) when syncing from public internet
  - **Configuration**: `myco-core/src/ip_source.rs:default_blossom_servers()` — defaults hardcoded, overridable
  - **Note**: For mesh-only mode (no internet), uses embedded Blossom on peer device at `http://<npub>.fips:24243`

## Data Storage

**Local Storage:**
- **Filesystem (app-private)** - On Android, stored in app's private `filesDir`
  - **Identity**: `identity.nsec` — device's Nostr keypair (secret key, never shared)
  - **Relay Events**: In-memory EventStore via nsite-deck, persisted via embedded relay's store
  - **Blobs**: Filesystem via `myco-blossom::FsBlobStore` at app-provided path
  - **Library**: Metadata for installed nsites, synced offline
  - Client: Standard Rust `std::fs` — no ORM, no database server

**No Remote Databases:**
- All persistent state is on-device only
- Relay events are synchronized P2P via embedded Nostr relay (WebSocket or mesh)
- No cloud sync or external database backends

**Caching:**
- **In-Memory**: EventStore cache for fast nsite queries
- **Blob Cache**: Filesystem (`FsBlobStore`) — blobs are not purged until user manually clears cache
- **No dedicated cache service** (Redis, Memcached) — all caching is embedded

## Authentication & Identity

**Auth Provider:**
- **Nostr Native** (decentralized, no central auth provider)
  - **Implementation**: Single persistent device keypair (Nostr nsec) generated on first launch and stored locally
  - **Key derivation**: `fips::Identity::generate()` → `fips::encode_nsec()` (NIP-19 nsec format)
  - **No passwords, no OAuth, no API keys for authentication**
  - **Per-device identity**: Each phone has one Nostr keypair (`npub` = public key, `nsec` = secret key)
  - **Signature**: All app data (nsite manifests, messages) signed with device's keypair
  - **Verification**: Remote peers verify signatures using author's public key

**Pairing Protocol:**
- **One-time invite codes** embedded in QR codes — when scanned, peers exchange identity info and add each other to "Circle"
- **Mutual acceptance** — pairing always goes both directions
- No central identity service; identity is purely cryptographic

**Access Control:**
- **Content-addressed blobs** — identified by hash, no access control needed (all data is published)
- **Signed manifests** — only accept updates from original author's key
- **Mesh addresses** — `<npub>.fips` (IPv6 address derived from public key)

## Monitoring & Observability

**Error Tracking:**
- Not detected — no Sentry, Rollbar, or similar integration

**Logs:**
- **Framework**: `tracing` 0.1 crate (structured logging)
- **Sink**: Android logcat (via `paranoid-android` 0.2 on Android-only)
  - All tracing from myco-core and fips are labeled with tag `myco`
  - Command: `adb logcat -s myco` to view during development
- **On-device**: Logs are ephemeral (not persisted by default)
- **No external log aggregation** (Splunk, DataDog, CloudWatch)

## CI/CD & Deployment

**Hosting:**
- **Mesh-only (no servers)** — apps and metadata are served peer-to-peer via embedded Nostr relay + Blossom
- **Optional IP fallback**: Public relays + Blossom servers for initial nsite discovery (overridable)
- **No app server** — no backend needed

**App Deployment:**
- **Android**: Built via Gradle in `android/app/`, distributed via direct APK or app store (mechanism not specified)
- **Web nsites**: Published to Nostr as kind 15128/35128 events (author-signed manifests)
  - Deployment tool: `nsite-cli upload dist` (uploads built artifacts to Blossom, publishes manifest event to relay)
  - No traditional hosting (no CDN, no app store required for apps; they spread P2P)

**CI Pipeline:**
- Not detected — no GitHub Actions, GitLab CI, or other CI config files present

## Environment Configuration

**Required env vars:**
- `.env` file exists but contents are not read (secrets policy)
- Inferred from codebase:
  - Likely needed for local development: Android SDK path, NDK path, Gradle config
  - Runtime: Data directory path (app-private filesDir on Android)
  - Optional: Public relay/Blossom server overrides (can be configured at runtime)

**Secrets location:**
- `.env` file (local, git-ignored)
- No cloud secret manager detected
- Device keypair (`nsec`) stored in local filesystem (app-private on Android)

## Webhooks & Callbacks

**Incoming:**
- **NIP-01 WebSocket subscriptions** — clients can subscribe to relay for events
- **Mesh gossip**: Peers automatically receive nsite updates via embedded relay
- No traditional webhook endpoints (HTTP POST callbacks)

**Outgoing:**
- **NIP-01 event publication** — device publishes its own nsite events to embedded relay, which gossips to peers
- **Mesh relay sync**: Periodic sync with connected peer relays to exchange events
- No external API calls for business logic (no Stripe webhooks, GitHub webhooks, etc.)

## Third-Party Services Explicitly Avoided

**By Design:**
- **No centralized app store** — apps spread P2P via mesh
- **No backend server** — all services embedded or distributed
- **No cloud storage** — all data stays on-device
- **No remote authentication** — identity is cryptographic, not account-based
- **No analytics** — no tracking, no user profiling
- **No push notifications** — communication is direct and synchronous

## Protocol Standards (NIPs - Nostr Improvement Proposals)

Used by embedded relay and nsite apps:
- **NIP-01** — Basic protocol (WebSocket, event publishing, filtering)
- **NIP-19** — bech32 encoding (npub, nsec, naddr)
- Inferred from nsite-deck and relay implementation:
  - **NIP-15** or **NIP-35** — Nsite manifest events (kind 15128 or 35128)
  - Blob storage via Blossom (HTTP complement to Nostr)

## Summary: Minimal External Dependencies

This system is **designed to be maximally offline and decentralized**:
- Device identity is self-generated (no KYC, no account registration)
- Data is signed locally (no central authority)
- Apps are installed P2P (no app store, no server)
- Communication is mesh-first, with optional IP fallback to public relays
- All persistent state is on-device (no cloud sync, no backend)

The only external integration points are **optional IP fallbacks** to well-known public Nostr relays and Blossom servers when the device has internet connectivity and chooses to sync with the broader Nostr network.

---

*Integration audit: 2026-08-01*
