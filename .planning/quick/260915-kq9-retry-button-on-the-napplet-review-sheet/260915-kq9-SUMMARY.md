---
quick_id: 260915-kq9
status: complete
date: 2026-09-15
commit: 441b2c2dfa07b59d8a2fd165b7b3ae8d0c4b153b
branch: feat/napplet-runtime
---

# Summary

`NappletReview` carries `holder`; the error branch of `NappletReviewSheet`
gains a **Try again** button that dispatches the same `FetchNapplet` with the
same holder (sharer's phone first). Verified: cargo test, clippy,
`compileDebugKotlin`. Not on device.
