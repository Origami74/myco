# Myco software update notices

Myco's own Android package is separate from installed nsites and napplets.
`SoftwareUpdates` queries the public, unauthenticated HTTPS GitHub latest-release
endpoint for `Origami74/myco`. It accepts only a non-draft, non-prerelease,
stable SemVer tag with an uploaded APK, and compares major/minor/patch numerically
with `BuildConfig.VERSION_NAME`. Build metadata does not affect ordering.
A local build derives its version from the nearest stable Git tag unless CI
supplies `MYCO_VERSION_NAME`; debug builds default automatic checks to off.

The check runs off the main thread while the app-root lifecycle is STARTED.
Automatic checks happen at most once per 24 hours after success, or once per
hour after failure, with persisted attempt times and a guard against clock
rollback. No validated internet means no request and no attempt recorded.
Mesh-only mode prevents manual and automatic requests. Changing that mode or
the automatic switch cancels the active lifecycle check. Settings offers a
manual retry, status, and the persisted automatic switch. HTTP requests have
10-second connect/read timeouts, a 256 KiB response cap and no redirects.

A newer tag is saved locally, adds a Settings badge and an in-app notice with
View update / Later. Later dismisses that tag's banner across launches; the
update remains accessible in Settings. Allowed Android notifications use a
dedicated Software updates channel and appear once per tag. Denied notification
permission or a disabled channel leaves the in-app notice usable. This feature
requests no additional runtime permission. A subsequent check that finds no
newer release clears the cached candidate and system notification.

Both update actions open the fixed official release page, never a URL supplied
by release metadata. Myco does not download an APK or request install authority.
Android and the user handle package installation and signature checks. The
GitHub request exposes the client's IP and Myco version, but sends no Nostr
identity, keys, relay configuration, or app inventory. There is no periodic
background worker; a closed app does not check for updates.

## Validation

Unit tests cover numeric version ordering, equal/older tags, prerelease and
malformed rejection, build metadata, check intervals, clock rollback, stable
release metadata and the uploaded-APK requirement.

On a physical phone: install a build with a version older than the latest
release; Check now should show the banner, Settings badge and one notification.
Repeat the check and restart: no repeated notification. Later should hide the
banner while retaining Settings access. Deny notifications: the banner still
works. Enable mesh-only mode: no release request. Check without internet:
a useful status, no crash. View update should open the official release page.
An equal/newer build should show up-to-date after a successful check.
