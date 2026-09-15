---
quick_id: 260915-kh3
status: complete
date: 2026-09-15
branch: feat/napplet-runtime
commits:
  - 5e78bce942030b9496b59bdaaa2a6d1ce056df01 (runtime + shell)
  - 4d81496fde7edf86a6ad8f20d37281843f57469d (core fetcher, docs)
---

# Summary: NAP-RESOURCE (blossom only)

- `resource.info` / `bytes` / `bytesMany` / `cancel`; `blossom:sha256:<hex>`
  (and bare `blossom:<hex>`) enabled; other schemes `unsupported-scheme`.
- Local Blossom first; miss → `BlobFetcher` (Circle mesh stores in parallel,
  8s; then public servers unless offline-only, 20s) → hash verified → stored
  → delivered. Errors per spec codes; 10 MiB / 100-URL caps; mime sniffed;
  raw SVG `blocked-by-policy`.
- Bytes as base64 on the JSON wire; `shell.html` materializes a `Blob` typed
  by `mime` for `bytes.result` and `bytesMany.result.items`.
- `NappletHost::new` now takes a `NapContext`.

Verified: fmt, clippy, `cargo test` (376 passed). Not run on device.

Not done: `https:`/`nostr:`/`htree:` schemes; SVG rasterization; sidecar
pre-resolution on relay events; per-napplet blob quota/rate limits (SHOULD).
