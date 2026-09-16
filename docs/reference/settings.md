# Settings and persisted state

What Myco keeps on disk, where, and who owns it. There is no `config.toml`
and no settings framework: a handful of plain files in the app's private data
dir (Rust), and one `SharedPreferences` file (Kotlin). Source of truth is the
code linked from each row.

---

## Rust — the app data dir

Everything the core persists lives under Android's private files dir for
`app.myco`. A missing or corrupt file means "defaults", never "refuse to
start".

| File | What | Owner |
| --- | --- | --- |
| `identity.nsec` | the **device key** — mesh identity, link auth, `<npub>.fips`; generated on first launch | `identity_store.rs` |
| `user.nsec`, `user-guest.json` | the **user key** napplets publish as, and its guest name; generated the first time a napplet runs | `user_key.rs` |
| `settings.json` | the persisted settings below | `settings_store.rs` |
| `library.json` | the Apps grid: every installed nsite and napplet, with `kind`, `pinned`, a napplet's `granted` capabilities and the `pointer` it was added by | `content.rs` (`LibraryItem`) |
| `circle.json` | the Circle: paired peers (`npub`, `name`, `addedAt`, per-peer `perms`) | `content.rs` (`CircleContact`) |
| `outbound_pairs.json` | invites sent and not yet accepted | `content.rs` |
| `active.json` | which version of each nsite is active (staged updates) | `content.rs`, [nsite-updates.md](../design/nsite/nsite-updates.md) |
| `relay/lmdb/` | the embedded relay's durable events (manifests, relay lists, profiles, notes) — an LMDB database; expiring events (chat) are memory-only. A pre-LMDB `relay/events.json` is migrated in on first open and left as `events.json.migrated` | `myco-relay` |
| `blossom/` | content-addressed blobs, named by sha256 | `myco-blossom` |
| `file_transfers.json`, `file-outbox/`, `received/` | native file sharing between Circle members | `file_transfer.rs` |
| `ble-attempts.jsonl` | the BLE connect-attempt log the Dev tab shows | `attempt_store.rs` |
| `fips-control.sock` | the node's control socket (runtime, not state) | `control_client.rs` |
| `seeded-defaults`, `profileInstalled` | one-shot markers | `runtime.rs`, `user_key.rs` |

### `settings.json`

[`settings_store.rs`](../../myco-core/src/settings_store.rs). Persisted because
each decides how something is *constructed*, so it must be on disk before the
thing is built.

| Key | Type | Default | Meaning | Set by |
| --- | --- | --- | --- | --- |
| `customRelayUrl` | string? | built-in | a NIP-01 relay to use as the event store instead of the embedded one. Applied at the next launch. | `SetCustomRelay` (Settings › Storage) |
| `customBlossomUrl` | string? | built-in | a Blossom server to use as the blob store instead of the embedded one. Applied at the next launch. | `SetCustomBlossom` |
| `awareDataPaths` | u8? | unknown | how many concurrent Wi-Fi Aware data paths the chipset reports; sizes the Aware UDP socket pool at node start | Kotlin, whenever it can read it |
| `nappletMeshPublishTtl` | u8? | 3 | the most hops a napplet's `mesh.publish` may ask for; never above `EVENT_TTL` | `SetNappletMeshReach` (Settings › App reach) |
| `nappletMeshSubscribeTtl` | u8? | 2 | the most hops a napplet's `mesh.subscribe` backlog pull may ask for; never above `MAX_REQ_TTL` | `SetNappletMeshReach` |

### Held in memory only

- **Offline only** (`SetOfflineOnly`): never use the internet fallback. Kotlin
  persists the switch and re-sends it on launch.
- **Device name** (`SetDeviceName`): stamped on pairing events. Kotlin owns and
  persists it.
- **Internet breaker**: after a round in which every public relay failed,
  napplet internet lanes are skipped for 30 s. Not persisted.

---

## Kotlin — `myco_prefs`

One `SharedPreferences` file. Radios and switches the UI owns; the core is told
on every launch.

| Key | Default | Meaning |
| --- | --- | --- |
| `mesh_enabled` | on | the master switch: the app-owned VPN/TUN and the node |
| `ble_enabled` | on | the BLE lane |
| `wifi_aware_enabled` | on | the Wi-Fi Aware lane |
| `lan_discovery_enabled` | on | same-network peer discovery (mDNS browse and advert) |
| `offline_only` | off | mesh only, no internet fallback (Dev) |
| `device_name` | the phone's own name | the memorable name peers see |
| `name_chosen` | — | the user has confirmed a name once |
| `intro_seen` | — | the first-run intro has been shown; radios wait for it |
| `home_screen_offered` | — | the home-screen pin has been offered once |
| `developer_mode` | debug builds | the Dev tab |
| `exit_proxy` | — | the experimental exit-node proxy (Dev) |

Issued pairing secrets (`PairSecrets.kt`) and pending deep links
(`PendingDeepLinks.kt`) also live here, keyed per secret / per link.

---

## Not persisted, on purpose

- **Grants are never widened by anything but the user.** Install review and the
  Manage permissions sheet are the only writers of `granted`; an open window
  learns of a change by relaunching.
- **Peer permissions** (`circle.json` → `perms`) have no UI yet; every peer gets
  the defaults. See [nsite-permissions.md](../design/nsite/nsite-permissions.md).
- **Ports** are fixed: relay `4870`, Blossom `24243`, auth `4873`. See
  [ports.md](./ports.md).
