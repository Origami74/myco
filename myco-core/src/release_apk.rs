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
//! 3. **A Circle peer holds it** — the peers [`crate::mesh_blobs`] finds
//!    holding it, one at a time, through [`IpPeerSource`] with its zstd
//!    variant (about 40% of the size: the APK stores its native library
//!    uncompressed).
//! 4. **The internet**, only when the caller allows it (unmetered, not
//!    mesh-only) — the file event's Blossom URLs.
//!
//! Every download is held to a time budget sized from the APK, and the whole
//! carry to [`CARRY_DEADLINE`]: a peer that drips bytes can slow a carry,
//! never hold it. A peer that failed is skipped for [`PEER_FAILED_FOR`].
//!
//! Once kept, the zstd variant is written for peers that fetch it, and older
//! releases' APKs are removed: one release is carried, ~25 + ~11 MB.
//!
//! Installing is Android's job ([`export`] hands it the verified bytes); it
//! checks the APK's signing certificate and version again before it asks the
//! user, and Android itself refuses an update signed by another key.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use myco_napplet_runtime::manifest::is_sha256_hex;
use nostr::{Event, EventId, Filter, PublicKey};
use nsite_deck::sha256_hex;
use nsite_deck::PeerSource as _;
use serde::Serialize;

use crate::content::Content;
use crate::ip_source::IpPeerSource;
use crate::mesh_wire::{new_query_id, MeshMeta};
use crate::software_release::{self as release, tag_values, PUBLISHER};

/// SHA-256 of the certificate Myco releases are signed with — the one pin:
/// the file event must name it, and Android reads it from here (over JNI) to
/// check the APK again before installing.
pub const RELEASE_CERT_SHA256: &str =
    "98432b60e463819071928666c894961dbd646f3e89197cfa654cd52afc9a7e79";

/// Zapstore's file event: one APK, by hash.
const FILE_KIND: u16 = 3063;
const APK_MIME: &str = "application/vnd.android.package-archive";
/// The largest APK carried. A release is ~25 MB; this bounds what a signed
/// event can make a phone download and hold.
pub const MAX_APK_BYTES: u64 = 64 * 1024 * 1024;
/// zstd level for the variant peers fetch. On the v0.12.1 APK: 25.0 MB to
/// 10.6 MB in 3.3 s and 86 MB of memory on a laptop (level 19: 10.1 MB,
/// 4.7 s, 118 MB; level 9: 11.3 MB, 0.2 s, 59 MB). Phones are slower; it
/// runs once per release, off the async workers.
const ZSTD_LEVEL: i32 = 17;
/// How long a variant that failed to write is not tried again.
const ZSTD_RETRY_AFTER: Duration = Duration::from_secs(6 * 60 * 60);
/// How long a download may go without a byte before the source is dropped.
const STALL: Duration = Duration::from_secs(30);
/// The slowest download allowed to finish: a source gets the time this rate
/// needs for the whole APK, plus [`STALL`] to start. Slow BLE still fits.
const MIN_RATE_BYTES_PER_SEC: u64 = 64 * 1024;
/// The most one carry may take, all sources together.
pub const CARRY_DEADLINE: Duration = Duration::from_secs(15 * 60);
/// How long a peer that failed to hand over the release is not asked again.
const PEER_FAILED_FOR: Duration = Duration::from_secs(3 * 60 * 60);

/// The release APK a signed file event describes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReleaseFile {
    pub version: String,
    /// Android's `versionCode`, when the event names one.
    pub version_code: Option<u64>,
    /// Lowercase hex.
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

/// Whether `event` is a file event the pinned publisher signed for Myco.
fn publisher_file(event: &Event) -> bool {
    PublicKey::parse(PUBLISHER).is_ok_and(|publisher| event.pubkey == publisher)
        && event.kind.as_u16() == FILE_KIND
        && tag_values(event, "i") == ["app.myco"]
        && event.verify().is_ok()
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
    if !publisher_file(file) {
        return None;
    }
    file_fields(file, announcement, abi)
}

/// [`file_from_event`]'s checks of the tags, past the signature.
fn file_fields(file: &Event, announcement: &Event, abi: &str) -> Option<ReleaseFile> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let version = release::announcement_tag(announcement, now)?;
    if !tag_values(announcement, "e").contains(&file.id.to_hex().as_str()) {
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
        [x] if is_sha256_hex(x) => x.to_ascii_lowercase(),
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
        .filter(|id| is_sha256_hex(id))
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
/// APK, used when it is that release. Bounded by [`CARRY_DEADLINE`].
pub async fn prepare(
    content: Arc<Content>,
    internet: bool,
    installed: Option<PathBuf>,
    abi: &str,
) -> anyhow::Result<Prepared> {
    tokio::time::timeout(
        CARRY_DEADLINE,
        prepare_unbounded(content, internet, installed, abi),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Carrying the update took too long; trying again later"))?
}

async fn prepare_unbounded(
    content: Arc<Content>,
    internet: bool,
    installed: Option<PathBuf>,
    abi: &str,
) -> anyhow::Result<Prepared> {
    let store = content.blobs_local().ok_or_else(|| {
        anyhow::anyhow!("A custom Blossom server is configured; updates are not carried")
    })?;
    let file = release_file(&content, internet, abi).await?;
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
        nsite_deck::seams::BlobStore::put(&*store, &bytes).await?;
        tracing::info!(version = %file.version, source, "release apk: kept");
        source
    };
    write_variant(&store, &file.sha256).await;
    let files = publisher_files(&content).await;
    for x in older_release_blobs(&files, &file.sha256) {
        if store.remove(&x) {
            tracing::info!(sha256 = %x, "release apk: older release removed");
        }
    }
    Ok(Prepared { file, source })
}

/// The zstd variant peers fetch, unless it is there or failed lately.
async fn write_variant(store: &Arc<myco_blossom::FsBlobStore>, sha256: &str) {
    static FAILED: Mutex<Option<(String, Instant)>> = Mutex::new(None);
    if store.has_zstd(sha256) {
        return;
    }
    if let Some((sha, at)) = FAILED.lock().unwrap().as_ref() {
        if sha == sha256 && at.elapsed() < ZSTD_RETRY_AFTER {
            return;
        }
    }
    let (zstd_store, sha) = (store.clone(), sha256.to_string());
    let written = tokio::task::spawn_blocking(move || zstd_store.store_zstd(&sha, ZSTD_LEVEL))
        .await
        .map_err(anyhow::Error::from)
        .and_then(|r| r);
    if let Err(error) = written {
        tracing::warn!(%error, "release apk: zstd variant not written");
        *FAILED.lock().unwrap() = Some((sha256.to_string(), Instant::now()));
    }
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

/// The time one source gets for `size` bytes: [`STALL`] to start, then
/// [`MIN_RATE_BYTES_PER_SEC`].
fn download_budget(size: u64) -> Duration {
    STALL + Duration::from_secs(size / MIN_RATE_BYTES_PER_SEC + 1)
}

/// Peers that failed to hand over a release lately: (npub, sha256) → when.
static PEER_FAILURES: Mutex<Option<HashMap<(String, String), Instant>>> = Mutex::new(None);

fn peer_failed_lately(npub: &str, sha256: &str) -> bool {
    let mut failures = PEER_FAILURES.lock().unwrap();
    let map = failures.get_or_insert_with(HashMap::new);
    map.retain(|_, at| at.elapsed() < PEER_FAILED_FOR);
    map.contains_key(&(npub.to_string(), sha256.to_string()))
}

fn remember_peer_failure(npub: &str, sha256: &str) {
    PEER_FAILURES
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert((npub.to_string(), sha256.to_string()), Instant::now());
}

/// The release from a Circle peer's Blossom. Only peers `mesh_blobs` finds
/// holding it are asked, one at a time (racing a 10 MB body across every
/// peer would fetch it from all of them, over BLE), starting at a rotating
/// offset so one bad peer is not always first; a peer that fails is skipped
/// for a while.
async fn from_circle(content: &Arc<Content>, file: &ReleaseFile) -> Option<Vec<u8>> {
    let probe = crate::mesh_blobs::MeshBlobs::new(Arc::new(crate::mesh_blobs::HttpProbe::new(
        content.clone(),
    )));
    let mut holders: Vec<String> = probe
        .holders(&file.sha256)
        .await
        .into_iter()
        .filter(|npub| !peer_failed_lately(npub, &file.sha256))
        .collect();
    if holders.is_empty() {
        return None;
    }
    let offset = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0)
        % holders.len();
    holders.rotate_left(offset);
    // No total timeout on the client: [`download_budget`] bounds each fetch.
    let http = crate::http::HttpClient::mesh(
        reqwest::Client::builder().connect_timeout(Duration::from_secs(10)),
    );
    let pool = content.peer_relays();
    for npub in holders {
        let Ok(source) = crate::ip_source::mesh_source_for(pool.clone(), &npub) else {
            continue;
        };
        let source = source
            .with_http_client(http.clone())
            .with_max_blob_bytes(file.size as usize)
            .with_stall_timeout(STALL)
            .with_total_timeout(download_budget(file.size))
            .accept_zstd();
        match source.fetch_blob(&file.sha256, &[]).await {
            Ok(Some(bytes)) => return Some(bytes),
            _ => {
                tracing::debug!(%npub, "release apk: not from this peer");
                remember_peer_failure(&npub, &file.sha256);
            }
        }
    }
    None
}

/// The Blossom servers among the file event's URLs: `https://…/<sha256>`.
/// (Zapstore's CDN is one; a plain download link is not, and is skipped.)
fn blossom_servers(file: &ReleaseFile) -> Vec<String> {
    let suffix = format!("/{}", file.sha256);
    file.urls
        .iter()
        .filter_map(|url| url.strip_suffix(&suffix).map(str::to_owned))
        .collect()
}

/// The release from the file event's Blossom URLs, when the internet is
/// allowed.
async fn from_internet(file: &ReleaseFile, allowed: bool) -> Option<Vec<u8>> {
    let servers = blossom_servers(file);
    if !allowed || servers.is_empty() {
        return None;
    }
    let http = crate::http::HttpClient::internet(
        reqwest::Client::builder().connect_timeout(Duration::from_secs(10)),
    );
    IpPeerSource::new(Vec::new(), servers)
        .with_http_client(http)
        .ignoring_manifest_servers()
        .with_max_blob_bytes(file.size as usize)
        .with_stall_timeout(STALL)
        .with_total_timeout(download_budget(file.size))
        .fetch_blob(&file.sha256, &[])
        .await
        .ok()
        .flatten()
}

/// Every publisher-signed Myco file event this phone keeps.
async fn publisher_files(content: &Content) -> Vec<Event> {
    let Ok(publisher) = PublicKey::parse(PUBLISHER) else {
        return Vec::new();
    };
    let filter = Filter::new()
        .kind(nostr::Kind::Custom(FILE_KIND))
        .author(publisher);
    content.relay().query(&[filter]).await.unwrap_or_default()
}

/// The APKs of every other release among `files` — only publisher-signed
/// Myco file events count, so no one else's event can name a blob for
/// removal — never `keep`.
fn older_release_blobs(files: &[Event], keep: &str) -> Vec<String> {
    files
        .iter()
        .filter(|e| publisher_file(e))
        .flat_map(|e| tag_values(e, "x"))
        .filter(|x| is_sha256_hex(x))
        .map(str::to_ascii_lowercase)
        .filter(|x| x != keep)
        .collect()
}

/// What "Clear local database" must keep for the carried release, from the
/// kept announcement and the publisher-signed file events it names: those
/// events, and every APK they name (each ABI's; variants go with them).
fn release_keep_set(announcement: &Event, files: &[Event]) -> (Vec<EventId>, Vec<String>) {
    let named = tag_values(announcement, "e");
    let mut ids = vec![announcement.id];
    let mut blobs = Vec::new();
    for file in files
        .iter()
        .filter(|f| named.contains(&f.id.to_hex().as_str()) && publisher_file(f))
    {
        ids.push(file.id);
        blobs.extend(
            tag_values(file, "x")
                .into_iter()
                .filter(|x| is_sha256_hex(x))
                .map(str::to_ascii_lowercase),
        );
    }
    (ids, blobs)
}

/// [`release_keep_set`] for what this phone keeps.
pub(crate) async fn kept_release(content: &Content) -> (Vec<EventId>, Vec<String>) {
    let Some(announcement) = kept_announcement(content).await else {
        return (Vec::new(), Vec::new());
    };
    release_keep_set(&announcement, &publisher_files(content).await)
}

/// Write the kept release APK `sha256` to `dest` for the installer, checked
/// against its name once more on the way out.
pub async fn export(content: &Content, sha256: &str, dest: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(is_sha256_hex(sha256), "not a sha256: {sha256}");
    let sha256 = sha256.to_ascii_lowercase();
    let store = content
        .blobs_local()
        .ok_or_else(|| anyhow::anyhow!("No embedded blob store"))?;
    let bytes = nsite_deck::seams::BlobStore::get(&*store, &sha256)
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
    use nostr::{EventBuilder, Keys, Kind, Tag};

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

    fn real_release() -> (Event, Event) {
        let ann = serde_json::from_str(include_str!("release-0.12.1-announcement.json")).unwrap();
        let file = serde_json::from_str(include_str!("release-0.12.1-file.json")).unwrap();
        (ann, file)
    }

    /// The real file event's tags under another key: what an attacker can
    /// publish about the release.
    fn forged_copy(file: &Event) -> Event {
        let tags = file
            .tags
            .iter()
            .map(|t| t.as_slice().to_vec())
            .collect::<Vec<_>>();
        signed(&Keys::generate(), FILE_KIND, tags)
    }

    const REAL_X: &str = "06f39fc91f11bf44adecee3b00bb7ca777b59f96dc6fe4957135107752b75fca";

    #[test]
    fn prune_names_only_publisher_signed_other_releases() {
        let (_, file) = real_release();
        let forged = forged_copy(&file);
        // Another release kept: the real file event's APK goes.
        assert_eq!(
            older_release_blobs(&[file.clone(), forged.clone()], &"a".repeat(64)),
            vec![REAL_X.to_string()]
        );
        // The carried release itself never goes.
        assert!(older_release_blobs(&[file], REAL_X).is_empty());
        // A forged event names nothing for removal.
        assert!(older_release_blobs(&[forged], &"a".repeat(64)).is_empty());
    }

    #[test]
    fn keep_set_is_the_announcement_its_files_and_their_apks() {
        let (ann, file) = real_release();
        let forged = forged_copy(&file);
        let (ids, blobs) = release_keep_set(&ann, &[file.clone(), forged.clone()]);
        assert_eq!(ids, vec![ann.id, file.id]);
        assert_eq!(blobs, vec![REAL_X.to_string()]);
        // Not named by the announcement (and not the publisher's): not kept.
        let (ids, blobs) = release_keep_set(&ann, &[forged]);
        assert_eq!(ids, vec![ann.id]);
        assert!(blobs.is_empty());
    }

    #[test]
    fn only_blossom_urls_are_internet_sources_and_budgets_scale() {
        let (ann, file) = real_release();
        let release = file_from_event(&file, &ann, ABI).unwrap();
        // The CDN URL is `<server>/<x>`; the GitHub download is not.
        assert_eq!(
            blossom_servers(&release),
            vec!["https://cdn.zapstore.dev".to_string()]
        );
        // 25 MB at the floor rate fits in well under the carry deadline.
        let budget = download_budget(release.size);
        assert!(budget > Duration::from_secs(380) && budget < CARRY_DEADLINE);
    }

    /// A Blossom server with one blob and its zstd variant, on loopback.
    async fn serve_blob(apk: &[u8]) -> (Arc<myco_blossom::FsBlobStore>, String, String) {
        let dir = std::env::temp_dir().join(format!(
            "myco-release-apk-{}-{}",
            std::process::id(),
            sha256_hex(apk)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Arc::new(myco_blossom::FsBlobStore::open(&dir).unwrap());
        let sha = nsite_deck::seams::BlobStore::put(&*store, apk)
            .await
            .unwrap();
        store.store_zstd(&sha, 3).unwrap().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let open: myco_blossom::server::AccessFn = Arc::new(|_, _| true);
        tokio::spawn(myco_blossom::server::serve_on_guarded(
            store.clone(),
            listener,
            open,
            Some(store.zstd_dir()),
        ));
        (store, sha, format!("http://{addr}"))
    }

    fn mesh_source(server: &str, max: usize) -> IpPeerSource {
        IpPeerSource::new(Vec::new(), vec![server.to_string()])
            .on_mesh()
            .with_max_blob_bytes(max)
            .with_stall_timeout(Duration::from_secs(5))
            .with_total_timeout(Duration::from_secs(10))
            .accept_zstd()
    }

    /// The variant is fetched and decoded; a corrupt variant falls back to
    /// the plain blob; a blob over the signed size is refused either way.
    #[tokio::test]
    async fn fetches_the_zstd_variant_and_falls_back_from_a_bad_one() {
        let apk = b"not really an apk, but compressible ".repeat(2048);
        let (store, sha, server) = serve_blob(&apk).await;
        let got = mesh_source(&server, apk.len()).fetch_blob(&sha, &[]).await;
        assert_eq!(got.unwrap().unwrap(), apk);

        // A variant that decodes to other bytes: the plain blob instead.
        let other = zstd::encode_all(&b"something else entirely"[..], 3).unwrap();
        std::fs::write(store.zstd_dir().join(&sha), other).unwrap();
        let got = mesh_source(&server, apk.len()).fetch_blob(&sha, &[]).await;
        assert_eq!(got.unwrap().unwrap(), apk);

        // Larger than the signed size: refused, variant or not.
        let got = mesh_source(&server, apk.len() - 1)
            .fetch_blob(&sha, &[])
            .await;
        assert!(got.unwrap().is_none());
    }

    /// A server that sends a byte just inside the stall timeout is cut off by
    /// the total budget, not waited on for good.
    #[tokio::test]
    async fn a_dripping_server_is_cut_off_by_the_total_budget() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    let _ = socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\n\r\n")
                        .await;
                    loop {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        if socket.write_all(b"x").await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        let source = IpPeerSource::new(Vec::new(), vec![format!("http://{addr}")])
            .on_mesh()
            .with_max_blob_bytes(1_000_000)
            .with_stall_timeout(Duration::from_secs(1))
            .with_total_timeout(Duration::from_millis(600));
        let started = Instant::now();
        let got = source.fetch_blob(&"a".repeat(64), &[]).await.unwrap();
        assert!(got.is_none());
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
