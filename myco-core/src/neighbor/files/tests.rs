use super::*;
use crate::neighbor::host::{Ask, Asker, PeerDirectory, PeerView, Seams};
use crate::neighbor::store::Record;
use std::sync::Arc;

struct Yes;
#[async_trait::async_trait]
impl Asker for Yes {
    async fn confirm(&self, _: Ask) -> bool {
        true
    }
}

struct Nobody;
impl PeerDirectory for Nobody {
    fn peers(&self, _: &Record) -> Vec<PeerView> {
        Vec::new()
    }
}

fn open() -> Neighbors {
    let d = std::env::temp_dir().join(format!("myco-nb-files-{}", crate::neighbor::id::token()));
    std::fs::create_dir_all(&d).unwrap();
    let n = Neighbors::open(
        &d,
        Seams {
            peers: Arc::new(Nobody),
            asker: Arc::new(Yes),
            scoped_stores: Arc::new(crate::neighbor::data::MemStores::default()),
            scoped_blobs: Arc::new(nsite_deck::testing::MemBlobs::new()),
            links: None,
        },
    );
    n.set_nearby(true);
    n
}

#[test]
fn the_envelope_opens_with_its_key_only_and_detects_damage() {
    let key = seal::new_key();
    for size in [
        0usize,
        1,
        seal::CHUNK - 1,
        seal::CHUNK,
        seal::CHUNK + 1,
        2 * seal::CHUNK + 7,
    ] {
        let plain: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let sealed = seal::seal(&plain, &key);
        assert_eq!(sealed.len(), seal::sealed_size(size), "{size}");
        assert_eq!(
            seal::open(&sealed, &key).as_deref(),
            Some(plain.as_slice()),
            "{size}"
        );
    }
    let plain = vec![7u8; seal::CHUNK + 10];
    let sealed = seal::seal(&plain, &key);
    assert_eq!(seal::open(&sealed, &seal::new_key()), None, "another key");
    let mut flipped = sealed.clone();
    flipped[20] ^= 1;
    assert_eq!(seal::open(&flipped, &key), None, "damaged");
    // Cut at the chunk boundary: the first chunk was sealed as not-final.
    let cut = &sealed[..seal::HEADER_BYTES + seal::CHUNK + seal::TAG_BYTES];
    assert_eq!(seal::open(cut, &key), None, "truncated");
    assert_eq!(seal::open(b"not a filedrop file at all", &key), None);
}

#[test]
fn a_link_round_trips_and_keeps_the_key_in_the_fragment() {
    let link = FileLink {
        x: "ab".repeat(32),
        name: "Trip photos & notes.zip".into(),
        mime: "application/zip".into(),
        size: 1234,
        encrypted: true,
        key: Some([9u8; seal::KEY_BYTES]),
    };
    let url = link.to_url();
    let (query, fragment) = url.split_once('#').unwrap();
    assert!(query.starts_with("https://napplet.link/file/download?enc=1&mime="));
    assert!(!query.contains("k="), "the key never in the query");
    assert!(fragment.starts_with("k="));
    assert_eq!(FileLink::parse(&url), Some(link.clone()));
    // An alphanumeric QR upper-cases scheme and host.
    let shouty = url.replacen("https://napplet.link", "HTTPS://NAPPLET.LINK", 1);
    assert_eq!(FileLink::parse(&shouty), Some(link));
    // Without its key an encrypted file can't be opened: not a link for us.
    assert_eq!(FileLink::parse(query), None);
    assert_eq!(
        FileLink::parse("https://napplet.link/neighborhood/join#invite=x"),
        None
    );
    assert_eq!(
        FileLink::parse("https://napplet.link/file/download?x=zz&enc=1#k=AA"),
        None
    );
    // A filedrop link to public servers is filedrop's, not Nearby's.
    let public = format!(
        "https://napplet.link/file/download?name=a&servers=blossom.example&x={}",
        "ab".repeat(32)
    );
    assert_eq!(FileLink::parse(&public), None);
    // Unencrypted: never one Myco shares, so filedrop's.
    let plain = format!(
        "https://napplet.link/file/download?name=a&size=1&x={}",
        "ab".repeat(32)
    );
    assert_eq!(FileLink::parse(&plain), None);
}

/// A sealed filedrop link shared into some other neighborhood parses like
/// a Nearby one; Nearby saying it doesn't hold it is what leaves it to the
/// napplet.
#[tokio::test]
async fn nearby_has_only_what_it_keeps() {
    let n = open();
    let link = n
        .share_file_nearby(b"kept".to_vec(), "a.txt", "text/plain")
        .await
        .unwrap();
    assert_eq!(n.nearby_has(&link.x).await, Some(true));
    assert_eq!(n.nearby_has(&"cd".repeat(32)).await, Some(false));
    n.set_nearby(false);
    assert_eq!(n.nearby_has(&link.x).await, Some(false));
}

#[tokio::test]
async fn a_shared_file_is_kept_sealed_and_opens_from_its_link() {
    let n = open();
    let bytes = b"holiday photo".to_vec();
    let link = n
        .share_file_nearby(bytes.clone(), "../beach.jpg", "image/jpeg; q=1")
        .await
        .unwrap();
    assert_eq!(link.name, ".._beach.jpg");
    assert_eq!(link.mime, "image/jpeg");
    assert_eq!(link.size, bytes.len() as u64);
    // What is kept is sealed: its hash is the link's, not the file's.
    let kept = n
        .scoped
        .blob(crate::neighbor::nearby::id(), &link.x)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(kept, bytes);
    assert_eq!(nsite_deck::sync::sha256_hex(&kept), link.x);
    let parsed = FileLink::parse(&link.to_url()).unwrap();
    assert_eq!(n.fetch_nearby_file(&parsed).await, Ok(bytes));
    let mut wrong = parsed.clone();
    wrong.key = Some(seal::new_key());
    assert_eq!(n.fetch_nearby_file(&wrong).await, Err(FileError::WrongKey));
}

/// Names are cleaned as filedrop cleans them: a bidi override can't make
/// a file look like another type, and nothing becomes a path.
#[test]
fn names_are_cleaned_as_filedrop_does() {
    assert_eq!(clean("invoice\u{202E}fdp.exe"), "invoice_fdp.exe");
    assert_eq!(clean("a\u{2066}b\u{200F}c"), "a_b_c");
    assert_eq!(clean("dir/sub\\x\n.txt"), "dir_sub_x_.txt");
    assert_eq!(clean("   "), "file");
    assert_eq!(clean(&"é".repeat(300)).chars().count(), MAX_NAME);
}

/// The largest file shared still fits through a link once sealed.
#[test]
fn a_sealed_file_at_the_cap_fits_through_a_link() {
    assert!(
        seal::sealed_size(MAX_FILE_BYTES) as u64 <= crate::neighbor::data::MAX_LINK_BLOB,
        "a 64 MB file sealed must be fetchable"
    );
}

#[tokio::test]
async fn nothing_is_shared_with_nearby_off_or_too_big_or_empty() {
    let n = open();
    assert_eq!(
        n.share_file_nearby(Vec::new(), "x", "text/plain").await,
        Err(FileError::Empty)
    );
    assert_eq!(
        n.share_file_nearby(vec![0; MAX_FILE_BYTES + 1], "x", "text/plain")
            .await,
        Err(FileError::TooLarge)
    );
    n.set_nearby(false);
    assert_eq!(
        n.share_file_nearby(b"x".to_vec(), "x", "text/plain").await,
        Err(FileError::NearbyOff)
    );
}
