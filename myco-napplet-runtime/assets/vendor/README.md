# Vendored third-party assets

Code that is **not ours**, checked in verbatim so the APK ships a fixed,
reviewable artifact rather than resolving a package at build time.

## `napplet-shim-prelude.global.js`

The `window.napplet` prelude — the capability namespace NIP-5D requires be
installed before any napplet script runs. Injected into each napplet's
`srcdoc` by `crate::artifact`, because the shell cannot script into an
opaque origin from outside.

| | |
|---|---|
| Package | [`@napplet/shim`](https://www.npmjs.com/package/@napplet/shim) |
| Version | `0.29.2` |
| File | `dist/prelude.global.js` (the browser IIFE build) |
| License | MIT |
| sha256 | `d3539080c553841d0387511cd8c902f15da8122075e0343380ff82a7beab898f` |
| Retrieved | 2026-08-20 |

Upstream owns the domain surface (`window.napplet.relay`,
`window.napplet.identity`, …), so tracking their build keeps Myco's namespace
identical to every other conformant runtime's. Hand-writing a shim would mean
re-deriving their interface on every registry change.

It exposes one global, `NappletShimPrelude`, whose `install({ domains })`
takes an explicit allowlist and installs **only** the requested domains. Myco
passes the session's offered set, so a napplet that was not granted `relay`
has no `window.napplet.relay` to call — the namespace and `shell.supports()`
answer from one source and cannot disagree.

### Updating

```sh
npm pack @napplet/shim@<version>
tar xzf napplet-shim-<version>.tgz
cp package/dist/prelude.global.js \
   myco-napplet-runtime/assets/vendor/napplet-shim-prelude.global.js
```

Then update the version, sha256 and date above, and re-run the crate's tests —
they assert the global name and the install signature this runtime depends on.
Per D7 the NAP revision is pinned deliberately, so bumping this is a decision,
not a chore.
