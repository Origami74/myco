CRUCIAL:
- [DONE 2026-08-06] Default enable wifi-aware
    PREF_AWARE now defaults true at both MainActivity read sites. AwareService
    already dispatches setWifiAwareEnabled(true) on start, so the core's flag and
    the Settings toggle follow the radio — no displayed/real mismatch.
- Show pending status peering request
    (closes as part of phase 01 plan 01-04 — DIAG-06)
- not always connecting with peers? think handshake fails (tiebreaker related?)
    (this is the tiebreaker-race hypothesis. 01-03 built the per-peer attempt log
    that answers it; reading it needs two phones — see DEVICE-TEST-BATCH.md D-1)

- [DONE 2026-08-06] no auto download of other people's apps
    - when clicking ANY of the apps in discover, it somehow automatically loads/pins ALL of them (in apps list)
    Root cause was not the click. DiscoverTile's LaunchedEffect fetches a favicon
    for EVERY tile via gatewayGet; gateway_get_framed spawned open_site() on a 503,
    and open_site calls add_to_library(pinned: true). So merely OPENING Discover
    pinned every app. gatewayGet gained an allowSync flag: WebView loads keep the
    self-healing sync, icon probes serve local-only and never sync.



GENERAL:

- deep linking, pass parameters into myco app
- share logs with developer = nip17 DM
- BLE transport can report itself operational while neither advertising nor scanning
  actually started (advertise/scan failures are logged and swallowed in
  BleTransport::start_async); restart supervision only recovers a listener failure —
  see the Limitations note on start_async in reference/fips
- Wi-Fi Aware's AwareRadio.kt onAttachFailed() is a one-shot with no retry/backoff
  (unlike BleRadio.kt's scheduleScanRetry()); deliberately not covered by the fips-side
  transport restart supervisor since Aware rides the ordinary UDP transport and can't
  fail a fips transport start

UI:
- [DONE 2026-08-06] TOP bar usage should be opt-in, not opt-out like it is now (for nsites)
    NsiteActivity drew edge-to-edge with the top explicitly full-bleed, so nsites
    written for a browser with its own top chrome rendered their header under the
    Android status icons. The status bar (and display cutout) is now reserved by
    default; a page opts into the full height with viewport-fit=cover, the standard
    signal that it handles env(safe-area-inset-*) itself. Re-probed per page load.
    Verified on device: bitchat's #mesh header now sits below the status bar.
- Can't see my own identity, no idea which 'circle's peer name' belongs to me
- immediately offer to save to homescreen when downloading app from peer
- [DONE 2026-08-06] camera sometimes doesn't focus...
    zxing-android-embedded defaults to autofocus ON but continuous focus OFF, so
    the preview focuses once on open and never refocuses. CameraScanner now sets
    CameraSettings{isAutoFocusEnabled, isContinuousFocusEnabled} = true. Covers
    both scanners (AppsScreen reuses PairScreen's ScanPanel). NEEDS DEVICE CHECK —
    the symptom is intermittent and cannot be reproduced on a build host.
- slow UI (thread blocking?)


FEATURE REQUESTS:
- pasting link in search == pasting in the 'add'



SMALL_STUFF_WHEN_TIME
- do pull to refresh, instead of refresh button in discover tab
