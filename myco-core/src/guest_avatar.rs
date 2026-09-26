//! The **guest avatar**: the Myco logo, tinted from the user's npub.
//!
//! Every guest starts with the same picture, so on its own it says nothing
//! about who is who. A gradient of two hues drawn from the pubkey sets guests
//! apart at a glance, the way generated avatars do elsewhere, while still
//! reading as Myco.
//!
//! The base is bundled (one compressed JPEG, `assets/guest-avatar-base.jpg`)
//! rather than fetched, so a guest has a picture on a phone that has never
//! been online. The output is deterministic: the same npub always yields the
//! same bytes, so its Blossom hash — and the `picture` URL in the profile —
//! can be recomputed on any later launch instead of stored.

use anyhow::Context;
use nostr::PublicKey;

/// The bundled base image: `docs/myco-logo-bg.png` at 256 px.
const BASE: &[u8] = include_bytes!("../assets/guest-avatar-base.jpg");

/// The output's media type, for the upload and the URL's extension.
pub const MIME: &str = "image/jpeg";
pub const EXTENSION: &str = "jpg";

const QUALITY: u8 = 82;

/// The avatar for `pubkey`, as JPEG bytes.
pub fn render(pubkey: &PublicKey) -> anyhow::Result<Vec<u8>> {
    let mut decoder = zune_jpeg::JpegDecoder::new(BASE);
    let mut pixels = decoder
        .decode()
        .map_err(|e| anyhow::anyhow!("base avatar: {e:?}"))?;
    let info = decoder.info().context("base avatar has no header")?;
    let (w, h) = (info.width as usize, info.height as usize);
    anyhow::ensure!(
        pixels.len() == w * h * 3,
        "base avatar is not RGB ({} bytes for {w}x{h})",
        pixels.len()
    );

    let (from, to) = hues(pubkey);
    let span = (w + h - 2).max(1) as f32;
    // One matrix per diagonal: the hue only varies along x + y.
    let matrices: Vec<[f32; 9]> = (0..w + h - 1)
        .map(|d| hue_rotation(from + (to - from) * (d as f32 / span)))
        .collect();
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 3;
            let m = &matrices[x + y];
            let (r, g, b) = (pixels[i] as f32, pixels[i + 1] as f32, pixels[i + 2] as f32);
            pixels[i] = clamp(m[0] * r + m[1] * g + m[2] * b);
            pixels[i + 1] = clamp(m[3] * r + m[4] * g + m[5] * b);
            pixels[i + 2] = clamp(m[6] * r + m[7] * g + m[8] * b);
        }
    }

    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, QUALITY)
        .encode(&pixels, w as u16, h as u16, jpeg_encoder::ColorType::Rgb)
        .map_err(|e| anyhow::anyhow!("encode avatar: {e}"))?;
    Ok(out)
}

/// The two ends of the gradient, in degrees of rotation from the logo's cyan.
///
/// The second end is always 90–270° from the first, so every avatar is a
/// visible gradient rather than a flat tint that happens to have two ends.
fn hues(pubkey: &PublicKey) -> (f32, f32) {
    let b = pubkey.to_bytes();
    let from = b[0] as f32 / 256.0 * 360.0;
    let span = 90.0 + (b[1] as f32 / 256.0) * 180.0;
    (from, from + span)
}

/// The luminance-preserving hue rotation matrix (as CSS `hue-rotate()`), so
/// the dark background stays dark and the glow keeps its brightness.
fn hue_rotation(degrees: f32) -> [f32; 9] {
    let (s, c) = degrees.to_radians().sin_cos();
    [
        0.213 + c * 0.787 - s * 0.213,
        0.715 - c * 0.715 - s * 0.715,
        0.072 - c * 0.072 + s * 0.928,
        0.213 - c * 0.213 + s * 0.143,
        0.715 + c * 0.285 + s * 0.140,
        0.072 - c * 0.072 - s * 0.283,
        0.213 - c * 0.213 - s * 0.787,
        0.715 - c * 0.715 + s * 0.715,
        0.072 + c * 0.928 + s * 0.072,
    ]
}

fn clamp(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::Keys;

    #[test]
    fn the_same_npub_always_gets_the_same_picture() {
        let pk = Keys::generate().public_key();
        let a = render(&pk).unwrap();
        let b = render(&pk).unwrap();
        assert_eq!(a, b, "the Blossom hash must be recomputable");
        assert!(a.starts_with(&[0xFF, 0xD8]), "not a JPEG");
        assert!(a.len() < 80_000, "avatar is {} bytes", a.len());
    }

    #[test]
    fn different_npubs_get_different_pictures() {
        let a = render(&Keys::generate().public_key()).unwrap();
        let b = render(&Keys::generate().public_key()).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn a_zero_rotation_leaves_colours_alone() {
        let m = hue_rotation(0.0);
        let px = [30.0f32, 200.0, 220.0];
        for row in 0..3 {
            let v = m[row * 3] * px[0] + m[row * 3 + 1] * px[1] + m[row * 3 + 2] * px[2];
            assert!((v - px[row]).abs() < 0.5, "row {row}: {v}");
        }
    }
}
