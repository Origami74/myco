# Myco signed release notifications

The Android app queries `wss://relay.zapstore.dev` for NIP-82 kind 30063
release announcements for `app.myco`, restricted to the publisher in
`zapstore.yaml`. Filtering is not authentication: Rust's existing `nostr`
implementation verifies both the event ID and BIP-340 signature against the
pinned publisher key before Kotlin reads the announcement. Wrong publishers,
tampered events, future timestamps, other app IDs, prereleases, non-main
channels, malformed versions and missing asset references cannot trigger a
notice. The highest stable numeric version is compared with the installed build.
There is no GitHub metadata fallback that could bypass signature verification.

This authenticates a release announcement. The app does not fetch or install
an APK; Zapstore and Android handle APK hashes and package signatures. Tapping
the notice opens the fixed official Zapstore listing. No release-provided URL
is opened and no user identity or private signing key is sent to the relay.

Automatic checks default on for release builds and off for debug builds. A
unique WorkManager task wakes approximately hourly with network constraints,
sharing the foreground check's persisted daily success / hourly failure gate
and process-wide mutex. Android can delay this work for battery management.
Closing the UI and rebooting do not discard scheduled work; force-stopping the
app prevents background delivery until it is opened again. This is periodic
release discovery with an Android notification, not instant server push.

Mesh-only mode and disabling automatic checks cancel the worker. The worker
checks persisted settings before using the network, and a query already in
flight cannot post a result after those settings change. The UI retains Check
now, a version badge, View update / Later, and one system notification per
version. Notification denial or a disabled channel leaves the in-app notice
usable. Enabling notification permission later lets a cached release notify
without waiting for the next daily query. Separate signed-release preferences
avoid trusting cached candidates from the old GitHub-only checker.

The release query is bounded to 20 seconds, 100 events and 150 response frames;
individual text frames are capped at 256 KiB. Only authenticated announcements
are considered. A relay outage leaves the previous verified candidate intact
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
2. Enable notifications and turn on automatic checks in Settings. Check now
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
6. Mesh-only / automatic off: no active periodic worker. Offline: no crash;
   reconnect and retry. Toggle mesh-only during a query: no new notice.
7. A test build at the latest version should report up to date. Rust tests use
   a real public signed 0.11.0 fixture, reject content/key tampering and valid
   attacker signatures. Kotlin tests cover version ordering, wrong app/channel,
   missing assets, future timestamps and failed authentication.

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
query does not repost it, and verifies mesh-only and automatic-off gates. It
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
