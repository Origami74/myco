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

#[tokio::test]
async fn a_shared_file_is_listed_as_ours_and_its_bytes_read_back() {
    let n = open();
    let bytes = b"holiday photo".to_vec();
    let shared = n
        .share_file_nearby(bytes.clone(), "beach.jpg", "image/jpeg")
        .await
        .unwrap();
    assert_eq!(shared.name, "beach.jpg");
    assert_eq!(shared.mime, "image/jpeg");
    assert_eq!(shared.size, bytes.len() as u64);
    assert!(shared.mine);
    assert_eq!(n.nearby_files().await, std::slice::from_ref(&shared));
    assert_eq!(n.nearby_file_bytes(&shared.sha256).await, Some(bytes));
    assert_eq!(n.nearby_file_bytes(&"0".repeat(64)).await, None);
}

#[tokio::test]
async fn each_share_is_signed_by_a_key_of_its_own() {
    let n = open();
    n.share_file_nearby(b"a".to_vec(), "a.txt", "text/plain")
        .await
        .unwrap();
    n.share_file_nearby(b"b".to_vec(), "b.txt", "text/plain")
        .await
        .unwrap();
    let id = crate::neighbor::nearby::id();
    let filter = Filter::new().kind(Kind::from(FILE_KIND));
    let authors: std::collections::BTreeSet<_> = n
        .scoped
        .backlog(id, &[filter])
        .await
        .unwrap()
        .into_iter()
        .map(|(e, _)| e.pubkey)
        .collect();
    assert_eq!(authors.len(), 2, "no key links two shares");
}

#[tokio::test]
async fn nothing_is_shared_with_nearby_off_or_too_big_or_empty() {
    let n = open();
    assert_eq!(
        n.share_file_nearby(Vec::new(), "x", "text/plain").await,
        Err(ShareError::Empty)
    );
    assert_eq!(
        n.share_file_nearby(vec![0; MAX_FILE_BYTES + 1], "x", "text/plain")
            .await,
        Err(ShareError::TooLarge)
    );
    n.set_nearby(false);
    assert_eq!(
        n.share_file_nearby(b"x".to_vec(), "x", "text/plain").await,
        Err(ShareError::NearbyOff)
    );
    assert!(n.nearby_files().await.is_empty());
}

#[test]
fn names_and_types_are_cleaned_and_odd_announcements_skipped() {
    let keys = Keys::generate();
    let sha = "a".repeat(64);
    let at = now();
    let ok = announcement(&sha, "../etc/pass\u{7}wd", "text/plain", 3, at)
        .sign_with_keys(&keys)
        .unwrap();
    let f = parse(&ok, false, at).unwrap();
    assert_eq!(f.name, "..etcpasswd");
    assert!(!f.mine);
    // Expired.
    assert!(parse(&ok, false, at + SHARED_FOR_SECS + 1).is_none());
    // Not filedrop's, or encrypted (filedrop opens those with its key).
    let other = EventBuilder::new(Kind::from(FILE_KIND), "")
        .tags([Tag::custom(TagKind::custom("x"), [sha.clone()])])
        .sign_with_keys(&keys)
        .unwrap();
    assert!(parse(&other, false, at).is_none());
    let sealed = announcement(&sha, "s.bin", "application/octet-stream", 3, at)
        .tag(Tag::custom(
            TagKind::custom("encrypted"),
            ["filedrop-aes256gcm-v1"],
        ))
        .sign_with_keys(&keys)
        .unwrap();
    assert!(parse(&sealed, false, at).is_none());
    // A bad hash.
    let bad = announcement("zz", "x", "text/plain", 1, at)
        .sign_with_keys(&keys)
        .unwrap();
    assert!(parse(&bad, false, at).is_none());
}
