//! Myco's own release APK, carried by the Circle so a phone updates from the
//! phones around it (roadmap N11, the "updates from phones nearby" half).
//!
//! A release is two publisher-signed Zapstore events: the announcement
//! (kind 30063, found by [`crate::software_release`]) and the file event it
//! names with an `e` tag (kind 3063), which carries the APK's sha256, size,
//! signing certificate and download URLs. With both kept, the APK's bytes can
//! come from anywhere — every source is checked against the signed hash.
//!
//! [`prepare`] makes this phone hold the newest release APK in its kept
//! Blossom store, where the Circle can fetch it like any kept blob:
//!
//! 1. **Already kept** — nothing to do.
//! 2. **This phone runs it** — the installed APK hashes to the signed sha256
//!    (a phone updated through Zapstore), so it is copied in for free.
//! 3. **A Circle peer holds it** — fetched from their Blossom, preferring
//!    the zstd variant (about 40% of the size: the APK stores its native
//!    library uncompressed).
//! 4. **The internet**, only when the caller allows it (unmetered, not
//!    mesh-only) — the file event's own URLs.
//!
//! Once kept, the zstd variant is written for peers that fetch it, and older
//! releases' APKs are removed: one release is carried, ~25 + ~10 MB.
//!
//! Installing is Android's job ([`export`] hands it the verified bytes); it
//! checks the APK's signing certificate and version again before it asks the
//! user, and Android itself refuses an update signed by another key.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nostr::{Event, EventId, Filter, Kind, PublicKey};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::content::Content;
use crate::mesh_wire::{new_query_id, MeshMeta};
use crate::software_release::{self as release, PUBLISHER};

/// SHA-256 of the certificate Myco releases are signed with. The file event
/// must name it; Android checks the APK against it again before installing.
pub const RELEASE_CERT_SHA256: &str =
    "98432b60e463819071928666c894961dbd646f3e89197cfa654cd52afc9a7e79";

/// Zapstore's file event: one APK, by hash.
const FILE_KIND: u16 = 3063;
const APK_MIME: &str = "application/vnd.android.package-archive";
/// The largest APK carried. A release is ~25 MB; this bounds what a signed
/// event can make a phone download and hold.
pub const MAX_APK_BYTES: u64 = 64 * 1024 * 1024;
/// zstd level for the variant peers fetch: ~10 MB from 25, in seconds.
const ZSTD_LEVEL: i32 = 19;
/// How long a peer gets to start answering.
const PEER_ANSWER: Duration = Duration::from_secs(20);
/// How long a download may go without a byte before the source is dropped.
const STALL: Duration = Duration::from_secs(30);

/// The release APK a signed file event describes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReleaseFile {
    pub version: String,
    /// Android's `versionCode`, when the event names one.
    pub version_code: Option<u64>,
    pub sha256: String,
    pub size: u64,
    /// Where it can be downloaded on the internet (`https` only).
    #[serde(skip)]
    pub urls: Vec<String>,
}

/// What [`prepare`] left this phone holding.
#[derive(Clone, Debug, Serialize)]
pub struct Prepared {
    #[serde(flatten)]
    pub file: ReleaseFile,
    /// Where the bytes came from this time: `kept`, `installed`, `circle`
    /// or `internet`.
    pub source: &'static str,
}

/// The file event `announcement` points at, read as a [`ReleaseFile`] for
/// `abi` (e.g. `android-arm64-v8a`) — or `None` unless every check passes:
/// the pinned publisher signed it, the announcement names it, and it is the
/// same app and version, an APK for this ABI, signed with the release key,
/// with a well-formed hash and a size within [`MAX_APK_BYTES`].
pub(crate) fn file_from_event(
    file: &Event,
    announcement: &Event,
    abi: &str,
) -> Option<ReleaseFile> {
    let publisher = PublicKey::parse(PUBLISHER).ok()?;
    if file.pubkey != publisher || file.kind.as_u16() != FILE_KIND || file.verify().is_err() {
        return None;
    }
    file_fields(file, announcement, abi)
}

/// [`file_from_event`]'s checks of the tags, past the signature.
fn file_fields(file: &Event, announcement: &Event, abi: &str) -> Option<ReleaseFile> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let version = release::announcement_tag(announcement, now)?;
    let named = tag_values(announcement, "e");
    if !named.contains(&file.id.to_hex().as_str()) {
        return None;
    }
    if tag_values(file, "i") != ["app.myco"]
        || tag_values(file, "version") != [version.as_str()]
        || tag_values(file, "m") != [APK_MIME]
        || !tag_values(file, "f").contains(&abi)
        || tag_values(file, "apk_certificate_hash") != [RELEASE_CERT_SHA256]
    {
        return None;
    }
    let sha256 = match tag_values(file, "x").as_slice() {
        [x] if is_sha256(x) => x.to_string(),
        _ => return None,
    };
    let size = match tag_values(file, "size").as_slice() {
        [size] => size
            .parse::<u64>()
            .ok()
            .filter(|s| (1..=MAX_APK_BYTES).contains(s))?,
        _ => return None,
    };
    let version_code = match tag_values(file, "version_code").as_slice() {
        [code] => Some(code.parse().ok()?),
        _ => None,
    };
    let urls = tag_values(file, "url")
        .into_iter()
        .filter(|u| u.starts_with("https://"))
        .map(str::to_owned)
        .collect();
    Some(ReleaseFile {
        version,
        version_code,
        sha256,
        size,
        urls,
    })
}

fn tag_values<'a>(event: &'a Event, name: &str) -> Vec<&'a str> {
    event
        .tags
        .iter()
        .filter_map(|tag| {
            let fields = tag.as_slice();
            (fields.first().map(String::as_str) == Some(name))
                .then(|| fields.get(1).map(String::as_str))
                .flatten()
        })
        .collect()
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The newest announcement this phone keeps, if any.
async fn kept_announcement(content: &Content) -> Option<Event> {
    let filter = serde_json::from_value::<Filter>(release::release_filter().ok()?).ok()?;
    let events = content.relay().query(&[filter]).await.ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    release::newest(events, now).map(|(_, event)| event)
}

/// The file events an announcement names, as a filter.
fn file_filter(announcement: &Event) -> Option<serde_json::Value> {
    let ids: Vec<&str> = tag_values(announcement, "e")
        .into_iter()
        .filter(|id| is_sha256(id))
        .collect();
    (!ids.is_empty()).then(|| serde_json::json!({ "ids": ids, "kinds": [FILE_KIND] }))
}

/// The release file for the newest kept announcement: the file event from the
/// kept relay, else the Circle, else (when `online`) Zapstore. A file event
/// found elsewhere is kept, so this phone answers its Circle in turn.
async fn release_file(
    content: &Arc<Content>,
    online: bool,
    abi: &str,
) -> anyhow::Result<ReleaseFile> {
    let announcement = kept_announcement(content)
        .await
        .ok_or_else(|| anyhow::anyhow!("No signed Myco release known yet"))?;
    let filter =
        file_filter(&announcement).ok_or_else(|| anyhow::anyhow!("The release names no file"))?;
    let pick = |events: Vec<Event>| {
        events
            .into_iter()
            .find_map(|e| file_from_event(&e, &announcement, abi).map(|f| (f, e)))
    };
    let kept = match serde_json::from_value::<Filter>(filter.clone()) {
        Ok(f) => content.relay().query(&[f]).await.unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    if let Some((file, _)) = pick(kept) {
        return Ok(file);
    }
    let meta = MeshMeta::pull(1, new_query_id(), crate::content::PULL_BUDGET_MS);
    let mut found = pick(
        content
            .pull_from_peers(vec![filter.clone()], meta, None)
            .await,
    );
    if found.is_none() && online {
        found = pick(release::from_zapstore(filter).await.unwrap_or_default());
    }
    let (file, event) =
        found.ok_or_else(|| anyhow::anyhow!("No signed release file for {abi} found"))?;
    if let Err(error) = content.relay().publish(event).await {
        tracing::debug!(%error, "release apk: file event not kept");
    }
    Ok(file)
}

/// Make this phone hold the newest signed release APK for `abi` in its kept
/// Blossom store (see the module docs for where it looks). `internet` allows
/// downloading it from the file event's URLs; `installed` is this phone's own
/// APK, used when it is that release.
pub async fn prepare(
    content: Arc<Content>,
    internet: bool,
    installed: Option<PathBuf>,
    abi: &str,
) -> anyhow::Result<Prepared> {
    let store = content.blobs_local().ok_or_else(|| {
        anyhow::anyhow!("A custom Blossom server is configured; updates are not carried")
    })?;
    let file = release_file(&content, internet, abi).await?;
    let max = file.size as usize;
    let source = if nsite_deck::seams::BlobStore::has(&*store, &file.sha256).await {
        "kept"
    } else {
        let (bytes, source) = if let Some(bytes) = from_installed(installed, &file).await {
            (bytes, "installed")
        } else if let Some(bytes) = from_circle(&content, &file).await {
            (bytes, "circle")
        } else if let Some(bytes) = from_internet(&file, internet).await {
            (bytes, "internet")
        } else {
            anyhow::bail!("Myco {} isn't on any phone nearby yet", file.version);
        };
        debug_assert_eq!(bytes.len(), max);
        nsite_deck::seams::BlobStore::put(&*store, &bytes).await?;
        tracing::info!(version = %file.version, source, "release apk: kept");
        source
    };
    // The variant peers fetch, and only one release carried.
    let (zstd_store, sha) = (store.clone(), file.sha256.clone());
    match tokio::task::spawn_blocking(move || zstd_store.store_zstd(&sha, ZSTD_LEVEL)).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => tracing::warn!(%error, "release apk: zstd variant not written"),
        Err(error) => tracing::warn!(%error, "release apk: zstd variant task failed"),
    }
    prune_older(&content, &store, &file.sha256).await;
    Ok(Prepared { file, source })
}

/// This phone's installed APK, when it is the release (a phone that updated
/// through Zapstore carries the release it runs without downloading it).
async fn from_installed(installed: Option<PathBuf>, file: &ReleaseFile) -> Option<Vec<u8>> {
    let path = installed?;
    if std::fs::metadata(&path).ok()?.len() != file.size {
        return None;
    }
    let sha = file.sha256.clone();
    tokio::task::spawn_blocking(move || {
        let bytes = std::fs::read(&path).ok()?;
        (sha256_hex(&bytes) == sha).then_some(bytes)
    })
    .await
    .ok()
    .flatten()
}

/// The release from a Circle peer's Blossom, one peer at a time (racing a
/// 10 MB body across every peer would fetch it from all of them, over BLE).
async fn from_circle(content: &Content, file: &ReleaseFile) -> Option<Vec<u8>> {
    let http = crate::http::HttpClient::mesh(
        reqwest::Client::builder().connect_timeout(Duration::from_secs(10)),
    );
    for npub in content.reachable_npubs() {
        let url = format!(
            "{}/{}",
            crate::ip_source::mesh_blossom_url(&npub),
            file.sha256
        );
        match fetch_verified(&http, &url, file, true).await {
            Some(bytes) => return Some(bytes),
            None => tracing::debug!(%npub, "release apk: not from this peer"),
        }
    }
    None
}

/// The release from the file event's own URLs, when the internet is allowed.
async fn from_internet(file: &ReleaseFile, allowed: bool) -> Option<Vec<u8>> {
    if !allowed {
        return None;
    }
    let http = crate::http::HttpClient::internet(
        reqwest::Client::builder().connect_timeout(Duration::from_secs(10)),
    );
    for url in &file.urls {
        if let Some(bytes) = fetch_verified(&http, url, file, false).await {
            return Some(bytes);
        }
        tracing::debug!(%url, "release apk: not from this url");
    }
    None
}

/// GET `url`, accept only bytes that are exactly the release (`size`,
/// `sha256`). With `zstd`, the peer's precompressed variant is asked for and
/// decoded, bounded to the release's size.
async fn fetch_verified(
    http: &crate::http::HttpClient,
    url: &str,
    file: &ReleaseFile,
    zstd: bool,
) -> Option<Vec<u8>> {
    let mut request = http.get(url).ok()?;
    if zstd {
        request = request.header(reqwest::header::ACCEPT_ENCODING, "zstd");
    }
    let resp = tokio::time::timeout(PEER_ANSWER, request.send())
        .await
        .ok()?
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let encoded = resp
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("zstd"));
    let size = file.size as usize;
    // A zstd body of an incompressible file can be a little larger than it.
    let body_max = if encoded {
        size + size / 64 + 4096
    } else {
        size
    };
    let body = crate::ip_source::read_body_bounded(resp, Some(body_max), Some(STALL)).await?;
    let bytes = if encoded {
        myco_blossom::decode_zstd_bounded(&body, size)?
    } else {
        body
    };
    (bytes.len() == size && sha256_hex(&bytes) == file.sha256).then_some(bytes)
}

/// Remove the APKs of every other release this phone keeps a file event for.
async fn prune_older(content: &Content, store: &myco_blossom::FsBlobStore, keep: &str) {
    let Ok(publisher) = PublicKey::parse(PUBLISHER) else {
        return;
    };
    let filter = Filter::new()
        .kind(Kind::Custom(FILE_KIND))
        .author(publisher)
        .custom_tag(
            nostr::SingleLetterTag::lowercase(nostr::Alphabet::I),
            "app.myco",
        );
    let Ok(files) = content.relay().query(&[filter]).await else {
        return;
    };
    for event in files.iter().filter(|e| e.verify().is_ok()) {
        for x in tag_values(event, "x") {
            if x != keep && is_sha256(x) && store.remove(x) {
                tracing::info!(sha256 = %x, "release apk: older release removed");
            }
        }
    }
}

/// What "Clear local database" must keep for the carried release: the
/// announcement and file event, and the APK (its zstd variant goes with it).
pub(crate) async fn kept_release(content: &Content) -> (Vec<EventId>, Option<String>) {
    let Some(announcement) = kept_announcement(content).await else {
        return (Vec::new(), None);
    };
    let mut ids = vec![announcement.id];
    let mut sha = None;
    if let Some(filter) =
        file_filter(&announcement).and_then(|f| serde_json::from_value::<Filter>(f).ok())
    {
        for event in content.relay().query(&[filter]).await.unwrap_or_default() {
            if PublicKey::parse(PUBLISHER).is_ok_and(|p| p == event.pubkey)
                && event.kind.as_u16() == FILE_KIND
            {
                ids.push(event.id);
                if let [x] = tag_values(&event, "x").as_slice() {
                    if is_sha256(x) {
                        sha = Some(x.to_string());
                    }
                }
            }
        }
    }
    (ids, sha)
}

/// Write the kept release APK `sha256` to `dest` for the installer, checked
/// against its name once more on the way out.
pub async fn export(content: &Content, sha256: &str, dest: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(is_sha256(sha256), "not a sha256: {sha256}");
    let store = content
        .blobs_local()
        .ok_or_else(|| anyhow::anyhow!("No embedded blob store"))?;
    let bytes = nsite_deck::seams::BlobStore::get(&*store, sha256)
        .await?
        .ok_or_else(|| anyhow::anyhow!("The update isn't on this phone"))?;
    anyhow::ensure!(sha256_hex(&bytes) == sha256, "The kept update is damaged");
    let dest = dest.to_path_buf();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = dest.with_extension("part");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &dest)?;
        Ok(())
    })
    .await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Tag};

    const ABI: &str = "android-arm64-v8a";

    fn signed(keys: &Keys, kind: u16, tags: Vec<Vec<String>>) -> Event {
        let tags = tags.into_iter().map(|t| Tag::parse(t).unwrap());
        EventBuilder::new(Kind::Custom(kind), "")
            .tags(tags)
            .sign_with_keys(keys)
            .unwrap()
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn file_tags(x: &str) -> Vec<Vec<String>> {
        vec![
            s(&["i", "app.myco"]),
            s(&["x", x]),
            s(&["version", "0.12.1"]),
            s(&["url", "https://cdn.example/apk"]),
            s(&["url", "http://plain.example/apk"]),
            s(&["m", APK_MIME]),
            s(&["size", "25017988"]),
            s(&["f", ABI]),
            s(&["version_code", "22"]),
            s(&["apk_certificate_hash", RELEASE_CERT_SHA256]),
        ]
    }

    fn announcement(keys: &Keys, file_id: &str) -> Event {
        signed(
            keys,
            30063,
            vec![
                s(&["i", "app.myco"]),
                s(&["version", "0.12.1"]),
                s(&["d", "app.myco@0.12.1"]),
                s(&["c", "main"]),
                s(&["e", file_id]),
            ],
        )
    }

    /// The real v0.12.1 file event's tags, signed by a stand-in key: the
    /// checks other than the publisher's signature are what is under test.
    #[test]
    fn reads_a_file_event_only_from_the_publisher() {
        let keys = Keys::generate();
        let x = "06f39fc91f11bf44adecee3b00bb7ca777b59f96dc6fe4957135107752b75fca";
        let file = signed(&keys, FILE_KIND, file_tags(x));
        let ann = announcement(&keys, &file.id.to_hex());
        // Not the pinned publisher: refused however well-formed.
        assert_eq!(file_from_event(&file, &ann, ABI), None);
    }

    #[test]
    fn checks_every_field_against_the_announcement() {
        // `file_from_event` refuses a non-publisher first (above); the tag
        // rules behind that gate are `file_fields`.
        let keys = Keys::generate();
        let x = "06f39fc91f11bf44adecee3b00bb7ca777b59f96dc6fe4957135107752b75fca";
        let ok = signed(&keys, FILE_KIND, file_tags(x));
        let ann = announcement(&keys, &ok.id.to_hex());
        let read = file_fields;
        let got = read(&ok, &ann, ABI).unwrap();
        assert_eq!(got.sha256, x);
        assert_eq!(got.size, 25017988);
        assert_eq!(got.version, "0.12.1");
        assert_eq!(got.version_code, Some(22));
        assert_eq!(got.urls, vec!["https://cdn.example/apk".to_string()]);

        // Another ABI, another app, a version or cert mismatch, a bad hash,
        // an oversized file, or a file the announcement doesn't name: no.
        assert!(read(&ok, &ann, "android-armeabi-v7a").is_none());
        let other_ann = announcement(&keys, &"b".repeat(64));
        assert!(read(&ok, &other_ann, ABI).is_none());
        for (name, value) in [
            ("i", "app.other"),
            ("version", "0.12.0"),
            ("apk_certificate_hash", &"c".repeat(64)),
            ("x", "XYZ"),
            ("size", "999999999999"),
            ("m", "text/plain"),
        ] {
            let mut tags = file_tags(x);
            for tag in tags.iter_mut().filter(|t| t[0] == name) {
                tag[1] = value.to_string();
            }
            let bad = signed(&keys, FILE_KIND, tags);
            let ann = announcement(&keys, &bad.id.to_hex());
            assert!(read(&bad, &ann, ABI).is_none(), "{name}={value} accepted");
        }
    }

    /// The real v0.12.1 release, as Zapstore serves it: the publisher-signed
    /// announcement and file event pass, and name the APK on GitHub
    /// Releases (whose sha256 this is). A changed byte fails the signature.
    #[test]
    fn the_real_signed_release_passes() {
        let ann: Event =
            serde_json::from_str(include_str!("release-0.12.1-announcement.json")).unwrap();
        let file: Event = serde_json::from_str(include_str!("release-0.12.1-file.json")).unwrap();
        let got = file_from_event(&file, &ann, ABI).unwrap();
        assert_eq!(got.version, "0.12.1");
        assert_eq!(
            got.sha256,
            "06f39fc91f11bf44adecee3b00bb7ca777b59f96dc6fe4957135107752b75fca"
        );
        assert_eq!(got.size, 25_017_988);
        assert_eq!(got.version_code, Some(22));
        assert_eq!(got.urls.len(), 2);

        let mut tampered: serde_json::Value =
            serde_json::from_str(include_str!("release-0.12.1-file.json")).unwrap();
        for tag in tampered["tags"].as_array_mut().unwrap() {
            if tag[0] == "x" {
                tag[1] = serde_json::json!("0".repeat(64));
            }
        }
        let tampered: Event = serde_json::from_value(tampered).unwrap();
        assert_eq!(file_from_event(&tampered, &ann, ABI), None);
    }

    /// A peer's Blossom hands out the zstd variant; the fetch decodes it and
    /// accepts it only as exactly the signed release.
    #[tokio::test]
    async fn fetches_the_zstd_variant_and_checks_it() {
        let dir = std::env::temp_dir().join(format!("myco-release-apk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Arc::new(myco_blossom::FsBlobStore::open(&dir).unwrap());
        let apk = b"not really an apk, but compressible ".repeat(2048);
        let sha = nsite_deck::seams::BlobStore::put(&*store, &apk)
            .await
            .unwrap();
        let variant = store.store_zstd(&sha, 3).unwrap().unwrap();
        assert!(variant < apk.len() as u64 / 4);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let open: myco_blossom::server::AccessFn = Arc::new(|_, _| true);
        tokio::spawn(myco_blossom::server::serve_on_guarded(
            store.clone(),
            listener,
            open,
            Some(store.zstd_dir()),
        ));
        let http = crate::http::HttpClient::mesh(reqwest::Client::builder());
        let url = format!("http://{addr}/{sha}");
        let file = ReleaseFile {
            version: "0.12.1".into(),
            version_code: None,
            sha256: sha.clone(),
            size: apk.len() as u64,
            urls: Vec::new(),
        };
        assert_eq!(fetch_verified(&http, &url, &file, true).await.unwrap(), apk);
        assert_eq!(
            fetch_verified(&http, &url, &file, false).await.unwrap(),
            apk
        );

        // The signed size is smaller than what the peer serves: refused.
        let short = ReleaseFile {
            size: file.size - 1,
            ..file.clone()
        };
        assert!(fetch_verified(&http, &url, &short, true).await.is_none());
        // The bytes don't hash to the signed name: refused.
        let other = ReleaseFile {
            sha256: "0".repeat(64),
            ..file.clone()
        };
        let wrong = format!("http://{addr}/{sha}");
        assert!(fetch_verified(&http, &wrong, &other, true).await.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
