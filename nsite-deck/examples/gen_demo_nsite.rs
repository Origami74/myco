//! Generate a signed demo nsite **bundle** (a `manifest.json` signed by a
//! throwaway key + a `blobs/<sha256>` set) into an output directory, for the dev
//! side-load path (`ImportNsite`). The app never authors/signs — this is external
//! tooling standing in for a real nsite author.
//!
//! Run: `cargo run -p nsite-deck --features testing --example gen_demo_nsite -- <outdir>`

use std::path::Path;

use nsite_deck::testing::build_test_site;

fn main() {
    let out = std::env::args()
        .nth(1)
        .expect("usage: gen_demo_nsite <outdir>");
    let dir = Path::new(&out);
    std::fs::create_dir_all(dir.join("blobs")).expect("create blobs dir");

    let index = br#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Myco demo nsite</title>
<link rel="stylesheet" href="/style.css">
</head>
<body>
<main>
  <h1>Myco nsite &#10003;</h1>
  <p>This page was served <strong>direct from the local relay + Blossom</strong>
     over the in-process gateway &mdash; no network involved.</p>
  <p id="clock">&hellip;</p>
  <a href="/about.html">About this demo</a>
</main>
<script src="/app.js"></script>
</body>
</html>
"#;

    let about = br#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>About</title><link rel="stylesheet" href="/style.css"></head>
<body><main>
  <h1>About</h1>
  <p>A side-loaded, content-addressed static site. Every byte was verified by
     sha256 against the signed manifest before it was served.</p>
  <a href="/index.html">&larr; Back</a>
</main></body>
</html>
"#;

    let style = br#":root { color-scheme: light dark; }
body { font-family: system-ui, sans-serif; margin: 0; display: grid; min-height: 100vh; place-items: center; }
main { max-width: 32rem; padding: 2rem; line-height: 1.5; }
h1 { color: #5b8a3a; }
a { color: #5b8a3a; }
#clock { font-variant-numeric: tabular-nums; opacity: 0.7; }
"#;

    let app = br#"// Proves JS runs and the page is live, served entirely offline.
function tick() {
  var el = document.getElementById('clock');
  if (el) el.textContent = 'Local time: ' + new Date().toLocaleTimeString();
}
tick();
setInterval(tick, 1000);
"#;

    let site = build_test_site(
        &[
            ("/index.html", index),
            ("/about.html", about),
            ("/style.css", style),
            ("/app.js", app),
        ],
        None,
        Some("Myco demo nsite"),
    );

    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec_pretty(&site.manifest).expect("serialize manifest"),
    )
    .expect("write manifest");
    for (hash, bytes) in &site.blobs {
        std::fs::write(dir.join("blobs").join(hash), bytes).expect("write blob");
    }

    let npub = {
        use nostr::nips::nip19::ToBech32;
        site.author.to_bech32().unwrap()
    };
    println!("wrote demo nsite bundle to {}", dir.display());
    println!("author npub: {npub}");
    println!("host: {npub}.nsite");
}
