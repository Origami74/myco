//! Filedrop's envelope, `filedrop-aes256gcm-v1`, so a file Myco shares is
//! one the filedrop napplet opens, and the other way round
//! (`reference/napplet-filedrop/src/crypto.ts`):
//!
//! ```text
//! header  = "FDROP" 0x01 | chunkSize (u32 BE)              10 bytes
//! body    = chunk_0 | chunk_1 | … | chunk_n                 each = ct + 16-byte tag
//! nonce_i = 0x00 × 7 | i (u32 BE) | final (0x01 on the last chunk)
//! aad     = header
//! ```
//!
//! One random key per file, so a nonce never repeats. The final flag makes
//! truncation at a chunk boundary fail; the counter makes reordering fail.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};

pub const MAGIC: &[u8; 6] = b"FDROP\x01";
pub const HEADER_BYTES: usize = 10;
pub const TAG_BYTES: usize = 16;
pub const CHUNK: usize = 1024 * 1024;
pub const KEY_BYTES: usize = 32;

/// A fresh key for one file.
pub fn new_key() -> [u8; KEY_BYTES] {
    use chacha20poly1305::aead::rand_core::RngCore as _;
    let mut key = [0u8; KEY_BYTES];
    chacha20poly1305::aead::OsRng.fill_bytes(&mut key);
    key
}

fn header(chunk: usize) -> [u8; HEADER_BYTES] {
    let mut h = [0u8; HEADER_BYTES];
    h[..6].copy_from_slice(MAGIC);
    h[6..].copy_from_slice(&(chunk as u32).to_be_bytes());
    h
}

fn nonce(i: u32, last: bool) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[7..11].copy_from_slice(&i.to_be_bytes());
    n[11] = last as u8;
    n
}

/// The sealed size of a `plain`-byte file.
pub fn sealed_size(plain: usize) -> usize {
    let chunks = plain.div_ceil(CHUNK).max(1);
    HEADER_BYTES + plain + chunks * TAG_BYTES
}

/// Seal `plain` under `key`.
pub fn seal(plain: &[u8], key: &[u8; KEY_BYTES]) -> Vec<u8> {
    seal_chunked(plain, key, CHUNK)
}

fn seal_chunked(plain: &[u8], key: &[u8; KEY_BYTES], chunk: usize) -> Vec<u8> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let hdr = header(chunk);
    let mut out = Vec::with_capacity(sealed_size(plain.len()));
    out.extend_from_slice(&hdr);
    let mut chunks: Vec<&[u8]> = plain.chunks(chunk).collect();
    if chunks.is_empty() {
        chunks.push(&[]);
    }
    let last = chunks.len() - 1;
    for (i, chunk) in chunks.into_iter().enumerate() {
        let n = nonce(i as u32, i == last);
        let ct = cipher
            .encrypt(
                Nonce::from_slice(&n),
                Payload {
                    msg: chunk,
                    aad: &hdr,
                },
            )
            .expect("AES-GCM seals any chunk under 64 GiB");
        out.extend_from_slice(&ct);
    }
    out
}

/// Open `sealed` with `key`, or `None` for a wrong key, damage, truncation,
/// or anything that isn't the envelope.
pub fn open(sealed: &[u8], key: &[u8; KEY_BYTES]) -> Option<Vec<u8>> {
    if sealed.len() < HEADER_BYTES + TAG_BYTES || &sealed[..6] != MAGIC {
        return None;
    }
    let hdr = &sealed[..HEADER_BYTES];
    let chunk = u32::from_be_bytes(hdr[6..10].try_into().ok()?) as usize;
    if chunk == 0 {
        return None;
    }
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let piece = chunk + TAG_BYTES;
    let body = &sealed[HEADER_BYTES..];
    let mut out = Vec::with_capacity(body.len());
    let mut off = 0usize;
    let mut i = 0u32;
    while off < body.len() {
        let end = (off + piece).min(body.len());
        if end - off < TAG_BYTES {
            return None;
        }
        let n = nonce(i, end == body.len());
        let pt = cipher
            .decrypt(
                Nonce::from_slice(&n),
                Payload {
                    msg: &body[off..end],
                    aad: hdr,
                },
            )
            .ok()?;
        out.extend_from_slice(&pt);
        off = end;
        i = i.checked_add(1)?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Filedrop's own envelope, sealed by WebCrypto as `crypto.ts` does
    /// (key 0..31, chunk 4, "filedrop!"): Rust opens it, and seals the same
    /// bytes. The two sides read each other's files.
    #[test]
    fn matches_filedrop_byte_for_byte() {
        let vector = hex::decode(
            "4644524f50010000000468d5d9bb7900077c336c5b46fc2f796e85f49a54449e1a4ba11d1\
             2dc4ce1160a6b9ff0b85851813569c7659898ead117c675e509531eb7aa71",
        )
        .unwrap();
        let mut key = [0u8; KEY_BYTES];
        for (i, b) in key.iter_mut().enumerate() {
            *b = i as u8;
        }
        assert_eq!(open(&vector, &key).as_deref(), Some(&b"filedrop!"[..]));
        assert_eq!(seal_chunked(b"filedrop!", &key, 4), vector);
    }
}
