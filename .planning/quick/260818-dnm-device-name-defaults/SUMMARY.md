---
id: 260818-dnm
slug: device-name-defaults
date: 2026-08-18
status: complete
branch: feat/status-pill-panel
---

# Quick: phone name as the default, generated name one tap away

## What changed

**`DeviceName.phoneName(context)`** reads the name the user already gave this
handset: `Settings.Global.DEVICE_NAME` first (no permission, API 25), then the
Bluetooth adapter name via `BluetoothManager` (wrapped — it throws until
`BLUETOOTH_CONNECT` is granted, which on a first run it is not), then the legacy
`Settings.Secure "bluetooth_name"`. Null when none of them has anything usable.

**`current()` now prefers it**: override → phone name → generated. The phone
name is far more recognisable across a table, and this is the trade for it
often carrying a real name.

**`suggestions()`** returns both, de-duplicated, and the new `NameSuggestions`
chip row puts each one tap away — so the pseudonymous option costs a tap rather
than typing. Wired into the Settings ▸ Identity editor (a tap saves outright,
there is nothing left to confirm) and the Circle rename dialog (a tap fills the
field, because Save is that dialog's commit and skipping it would leave Cancel
meaning nothing). The Settings "Reset" button is gone — it was the same idea
with a worse name, and the chips say what they will do.

**The generator got wider and better keyed.** 12 × 12 = 144 combinations is why
duplicates showed up: by the birthday bound a room of 14 devices was even money
for a collision. Now 32 × 64 = 2048, which needs ~53. The key moved from
`String.hashCode()` to SHA-256 with disjoint digest bytes per list — the old
code drew both the colour and the name from correlated bits of one 32-bit value,
so the space was smaller in practice than the lists implied.

## Verification

- New `DeviceNameTest`: determinism, the empty-npub case, shape (two lowercase
  words), a circle-sized run with no collision, and 1000 npubs filling >700 of
  the 2048 buckets (a correlated hash fails that while the lists stay the same).
- `:app:testDebugUnitTest`, `:app:assembleDebug` — pass. Installed on both
  attached devices.

## Not done

- The first-run gating discussed alongside this (radios and permission prompts
  firing in `onCreate` before the intro) is NOT implemented — still open.
