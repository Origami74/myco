---
quick_id: 260915-kva
description: Napplet capability calls off the main thread, overlapping, with an internet breaker
date: 2026-09-15
mode: quick
---

# "outbox.query timed out"

Logcat: `spent 7246ms processing MotionEvent` on NappletActivity while five
public relays timed out — the frame was dispatched on the main thread and
frames serialized on the session lock; a burst of queries pushed the last
past the shim's 30 s.

1. Runtime: `needs_session(message)`; hosts run stateless calls on a snapshot.
2. Core: `NappletHost::frame` holds the session only for stateful calls;
   `Content::internet_looks_down` breaker (30 s after a fully failed round)
   used by outbox lanes and the Blossom fetcher.
3. Android: frames consumed from a channel off-main, sequential until
   `shell.init`, concurrent after.
