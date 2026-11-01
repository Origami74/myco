# Myco signed release notifications

The Android app looks for NIP-82 kind 30063 release announcements for
`app.myco`, restricted to the publisher in `zapstore.yaml`, in three places at
once: its own store, its Circle (one forwarded hop, the ordinary mesh pull),
and `wss://relay.zapstore.dev` unless mesh-only mode is on. Filtering is not authentication: Rust's existing `nostr`
implementation verifies both the event ID and BIP-340 signature against the
pinned publisher key before validating the announcement in Rust. Wrong publishers,
tampered events, future timestamps, other app IDs, prereleases, non-main
channels, malformed versions and missing asset references cannot trigger a
notice. The highest stable numeric version is compared with the installed build.
There is no GitHub metadata fallback that could bypass signature verification.

This authenticates a release announcement. Tapping the notice opens the fixed
official Zapstore listing, unless the Circle has already brought the APK (see
[Carrying the APK](#carrying-the-apk)). No user identity or private signing key
is sent to the relay.

## Asking the Circle

Every check keeps the newest verified announcement in the device's own relay,
so the device answers its Circle's next check. One phone that reaches Zapstore
is enough for the room: a mesh-only phone asks its peers, and their peers, and
gets the same signed event. A peer can withhold a release but cannot forge or
alter one; the event is verified against the pinned key as if it came from
Zapstore. The Circle is asked only while the core runs (the mesh service or the
UI); a background check in a process without one asks only Zapstore.

## Carrying the APK

Each phone keeps the newest release APK in its kept Blossom store, so the
Circle updates with no internet (`myco-core/src/release_apk.rs`).

### What vouches for it

The announcement names a **file event** (kind 3063) with an `e` tag. Myco
accepts one only if:

- the pinned publisher signed it, and the kept announcement names it;
- it is `app.myco`, the announced version, an APK (`m`) for this phone's ABI
  (`f`);
- its `apk_certificate_hash` is the release key
  (`98:43:2B:…:7E:79`), and `x` and `size` are well formed, size at most
  64 MiB.

The file event is kept beside the announcement, so the phone answers its
Circle's next pull for it.

### Where the bytes come from

In order, stopping at the first that works:

1. **Kept already.**
2. **This phone's installed APK**, when it hashes to `x` — a phone updated
   through Zapstore carries the release it runs for free.
3. **A Circle peer's Blossom**, one peer at a time
   (`http://<npub>.fips:24243/<x>`).
4. **The file event's `https` URLs** (Zapstore's CDN, GitHub), only on an
   unmetered connection outside mesh-only.

Every source is checked the same way: exactly `size` bytes hashing to `x`.

### Compressed for the mesh

A release APK stores its native library and dex uncompressed, so zstd at
level 19 takes 25 MB to about 10 MB. Once kept, the phone writes that
variant to `blossom/zstd/<x>` (seconds of CPU, once per release). The
Blossom server hands it out only to a client sending
`Accept-Encoding: zstd`, as `Content-Encoding: zstd`; any other client gets
the plain blob. The fetching phone decodes it bounded to `size` and checks
the result against `x`: the variant needs no signature of its own.

### One release kept

After a release is kept, the APKs of every older release the phone has a
file event for are removed, with their variants. "Clear local database"
keeps the carried release, its announcement and its file event. A phone
using a custom Blossom server carries nothing.

### Installing

The banner and Settings show **Install** when the carried release is the
available update and this build is release-signed (a debug build can't take
a release over itself). Install:

1. Asks for Android's per-app "Install unknown apps" switch, through the
   usual explain-then-ask flow (`SystemAsk.InstallUpdates`), once.
2. Writes the kept APK to the cache, re-checked against `x`.
3. Reads it as Android does: this app's package, a higher `versionCode`,
   signed with the release key. Anything else stops here.
4. Hands it to `PackageInstaller`. Android shows its own "Update?" prompt,
   and refuses an update signed with another key regardless.

This needs `REQUEST_INSTALL_PACKAGES`. Myco ships through GitHub Releases
and Zapstore, which allow it.

### Scheduling the carry

Every check carries, due or not (Circle members come and go), while the core
runs. A carry that finds nothing new is cheap: a few kept-store queries.

## Scheduling

Automatic checks default on; saved choices are preserved. Outside mesh-only
they contact `wss://relay.zapstore.dev` daily, including with the UI closed.
The relay sees the phone's IP address and a Myco release query. Check now runs
even with automatic checks off. A unique WorkManager task wakes approximately
hourly with no network constraint, since the Circle needs none,
sharing the foreground check's persisted daily success / hourly failure gate
and one process-wide SoftwareUpdates instance. Concurrent callers join the
same in-flight check, and the UI observes background progress and results
through the shared StateFlow. Android can delay this work for battery management.
Closing the UI and rebooting do not discard scheduled work; force-stopping the
app prevents background delivery until it is opened again. This is periodic
release discovery with an Android notification, not instant server push.

Disabling automatic checks cancels the worker. Each check reads mesh-only
directly from `myco_prefs/offline_only` (default false, matching the core) and
leaves Zapstore out when it is on. A query already in flight cannot post a
result after automatic checks are turned off. The UI retains Check
now, a version badge, View update / Later, and one system notification per
version. Notification denial or a disabled channel leaves the in-app notice
usable. Enabling notification permission later lets a cached release notify
without waiting for the next daily query. Separate signed-release preferences
avoid trusting cached candidates from the old GitHub-only checker.

The release query uses the shared Rust relay client, with a 20-second deadline
and a filter requesting up to 100 events. The shared client owns connection and
message limits and subscription cleanup. Only authenticated announcements are
considered. A relay outage leaves the previous verified candidate intact
and retries after an hour. A relay can withhold announcements; signatures
prevent forgery but cannot guarantee delivery or freshness.

## Rollout

Existing versions cannot acquire this code remotely. Ship one release containing
this feature, then future publisher-signed releases can notify those users.
Users remaining on older builds depend on Zapstore's own update discovery.
Publisher key rotation requires updating the pinned trust anchor deliberately.

## Testing without publishing a fake release

1. Build a separate debug test package using `MYCO_UPDATE_TEST_BUILD=1` and
   `MYCO_VERSION_NAME=0.10.0`. It installs as `app.myco.updatetest` and keeps
   the normal app and its data untouched. Use an ARM64 emulator or spare phone.
2. Enable notifications; automatic checks are on by default. Check now
   should find the real publisher-signed 0.11.0 announcement (or a later stable
   release), show the banner and badge, and post one notification. Tap it:
   the official Myco Zapstore listing should open.
3. Repeat Check now and restart: no duplicate notification. Later hides the
   banner but leaves the release accessible in Settings.
4. For background delivery, on a fresh test installation enable automatic
   checks then return Home before checking. Use `adb shell dumpsys jobscheduler`
   to find the test package's WorkManager job ID, then run
   `adb shell cmd jobscheduler run -f app.myco.updatetest <job-id>`.
   Expect one notification with the UI closed. Also test an ordinary scheduled
   run and reboot, without forcing it, because forcing bypasses OS constraints.
5. Deny notifications: the banner still works. Grant permission, reopen, and
   confirm one notice. Disable the channel: no crash or repeated alert.
6. Automatic off: no active periodic worker. Offline: no crash; reconnect and
   retry. Mesh-only with a Circle peer that has checked online: Check now finds
   the release with no internet. Mesh-only with no core running: "Start the
   mesh to ask your Circle for updates".
7. A test build at the latest version should report up to date. Rust tests use
   a real public signed 0.11.0 fixture, reject content/key tampering and valid
   attacker signatures. Rust tests cover announcement fields and authentication; Kotlin tests cover
   installed-version ordering and check throttling.

Do not uninstall or clear the production package to install a debug APK.
Debug and publisher signing keys differ, so a production APK cannot be updated
in place with this debug test build. No real phone installation is claimed by
unit tests or APK assembly.

## Reference

Amber's [ZapstoreUpdater](https://github.com/greenart7c3/Amber/blob/master/app/src/main/java/com/greenart7c3/nostrsigner/service/ZapstoreUpdater.kt)
verifies release events and emits update notifications; its
[UpdateCheckWorker](https://github.com/greenart7c3/Amber/blob/master/app/src/main/java/com/greenart7c3/nostrsigner/service/UpdateCheckWorker.kt)
waits for the check to finish inside persistent background work. Myco uses the
same discovery pattern, its own pinned publisher key and its existing Rust
signature verifier, then hands installation to Zapstore.

## Repeatable physical-device tests

`SoftwareUpdateDeviceTest` uses AndroidJUnitRunner and refuses to modify update
preferences unless the target package is `app.myco.updatetest`. Build both the
isolated debug APK and its `assembleDebugAndroidTest` APK, install both, then run:

```sh
adb shell am instrument -w -r \
  -e class 'app.myco.update.SoftwareUpdateDeviceTest#testBackgroundReleaseNotificationAndDeduplication' \
  app.myco.updatetest.test/androidx.test.runner.AndroidJUnitRunner
```

Run while the launcher is visible. This enqueues the real SoftwareUpdateWorker,
queries the live relay, checks the OS notification, confirms a second successful
query does not repost it, and verifies the default-on schedule, the mesh-only
gate without a core, and the automatic-off gate. It
resets only this test package's signed-update preferences, not identity or mesh
data. Its final state leaves automatic checks disabled.

To test denial, disable **Show notifications** for the isolated test package in
Android Settings, then run the same command with method
`testBlockedNotificationsKeepCandidateAvailable`. A successful live check must
retain the candidate while posting no notification and leaving notified_tag
unset. The test is skipped when Android reports that notifications are enabled;
a skip is not denial coverage. Re-enable the package's notifications afterward.
On the Redmi tested here, changing POST_NOTIFICATION through appops did not
change NotificationManager's enabled state; use Android's actual settings UI.

On 2026-10-16, both methods passed on a Redmi Note 8 Pro (Android 11/API 30),
including delivery while the launcher was visible. The earlier manual UI test
also confirmed that tapping the actual update notification opened Myco's listing
in the installed Zapstore app. This does not prove reboot or overnight delivery,
or Android 13+ runtime-permission behavior.
