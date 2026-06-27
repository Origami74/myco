# Publish Myco (GitHub Releases + Zapstore)

Myco ships as a signed arm64 APK on **GitHub Releases**, which **Zapstore** then
ingests. Two independent keys are involved — don't conflate them:

| Key | Signs | Where it lives |
| --- | --- | --- |
| **Android keystore** (`.jks`) | the APK (so it installs + updates) | your disk / CI secret |
| **Nostr key** (nsec / `bunker://`) | the Zapstore release *event* | a remote signer (NIP-46) |

Zapstore links your **Android signing certificate** to your **nostr identity** on
the first publish, so the keystore must be stable forever — losing it means you
can no longer ship updates.

## 1. One-time: create the release keystore

```sh
keytool -genkeypair -v \
  -keystore android/myco-release.jks \
  -alias myco -keyalg RSA -keysize 4096 -validity 10000 -storetype PKCS12
```

Back `myco-release.jks` up offline. It is gitignored (`*.jks`).

Inspect the certificate fingerprint (what Zapstore pins):

```sh
keytool -list -v -keystore android/myco-release.jks -alias myco
```

## 2. Local signed build

Copy the template and fill it in (gitignored):

```sh
cp android/keystore.properties.example android/keystore.properties
```

```properties
storeFile=../myco-release.jks
storePassword=…
keyAlias=myco
keyPassword=…
```

```sh
cd android && ./gradlew :app:assembleRelease
# -> app/build/outputs/apk/release/app-release.apk  (signed)
```

Without `keystore.properties` (or the `MYCO_RELEASE_*` env vars) the release
build is left **unsigned**.

## 3. CI release (recommended)

Push a tag and the [`release.yml`](../../.github/workflows/release.yml) workflow
builds, signs, and publishes to GitHub Releases:

```sh
git tag v0.1.0 && git push origin v0.1.0
```

It clones the `fips` core (`jmcorgan/fips@ble-v2`, gitignored locally), sets up
the Rust + cargo-ndk + NDK toolchain, derives `versionName`/`versionCode` from
the tag, and signs **if** these repo secrets are set (Settings → Secrets →
Actions):

| Secret | Value |
| --- | --- |
| `RELEASE_KEYSTORE_BASE64` | `base64 -i android/myco-release.jks` |
| `RELEASE_STORE_PASSWORD` | keystore password |
| `RELEASE_KEY_ALIAS` | `myco` |
| `RELEASE_KEY_PASSWORD` | key password |

## 4. Publish to Zapstore

Config lives in [`zapstore.yaml`](../../zapstore.yaml) (`repository`, `pubkey`,
`release_source: github`, `metadata_sources: github`). Set `pubkey` to your npub
first.

Sign the Zapstore event with a **remote signer** so no nsec touches disk/CI:

```sh
go install github.com/zapstore/zsp@latest
SIGN_WITH='bunker://<pubkey>?relay=wss://…&secret=…' zsp publish
```

`zsp publish --wizard` walks first-time setup (and can populate `zapstore.yaml`).
On the first publish it links your APK certificate to your nostr identity. To run
it from CI instead, uncomment the **Publish to Zapstore** step in `release.yml`
and store the bunker URL as the `ZAPSTORE_BUNKER_URL` secret.
