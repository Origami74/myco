CRUCIAL:
- Default enable wifi-aware
- Show pending status peering request
- not always connecting with peers? think handshake fails (tiebreaker related?)

- no auto download of other people's apps
    - when clicking ANY of the apps in discover, it somehow automatically loads/pins ALL of them (in apps list)



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
- TOP bar usage should be opt-in, not opt-out like it is now (for nsites)
- Can't see my own identity, no idea which 'circle's peer name' belongs to me
- immediately offer to save to homescreen when downloading app from peer
- camera sometimes doesn't focus...
- slow UI (thread blocking?)


FEATURE REQUESTS:
- pasting link in search == pasting in the 'add'



SMALL_STUFF_WHEN_TIME
- do pull to refresh, instead of refresh button in discover tab
