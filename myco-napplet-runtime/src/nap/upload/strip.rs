//! Metadata stripping for NAP-UPLOAD: what a file says about where, when and
//! on what it was made, taken out before the bytes are stored or sent.
//!
//! A phone photo carries the GPS fix it was taken at, the camera's serial
//! number and the time to the second; a 3D export names the tool and often
//! the user's paths. A napplet that posts a picture rarely means to publish
//! any of that, and once it is on a public server it cannot be taken back.
//! The spec lets a shell strip (napplet/naps PR #33, "The shell MAY strip or
//! rewrite metadata"), and Myco always does, for the formats it can do
//! safely:
//!
//! - **JPEG** — every `APPn` segment but `APP0` JFIF, `APP2` ICC profile and
//!   `APP14` Adobe (the colour transform a CMYK file needs to decode), and
//!   every `COM`. Bytes after the end-of-image marker go too: a "motion
//!   photo" hides a whole video there.
//! - **PNG** — every ancillary chunk but those that change how the picture
//!   looks (`iCCP`, `gAMA`, `cHRM`, `sRGB`, `pHYs`, transparency, background,
//!   HDR and animation). So `tEXt`, `zTXt`, `iTXt`, `tIME` and anything
//!   unknown go.
//! - **WebP** — every RIFF chunk but the image, alpha, animation and ICC
//!   ones, so `XMP ` goes; the `VP8X` flags are made to match and the RIFF
//!   size rewritten.
//!
//! One piece of EXIF is not metadata but part of how the picture looks: the
//! Orientation tag. A phone stores a portrait photo's pixels landscape and
//! says "rotate 90°" there. So where the EXIF (JPEG `APP1`, PNG `eXIf`, WebP
//! `EXIF`) holds an Orientation other than 1, it is replaced by a new,
//! minimal EXIF holding that one tag and nothing else — no GPS, camera,
//! timestamps or thumbnail. EXIF that does not parse is dropped whole.
//! - **GLB** (binary glTF) — `asset.generator`, every `extras`, and the
//!   `KHR_xmp_json_ld` extension, wherever they are; `asset.version` and
//!   `asset.copyright` (the licence) stay. Pictures embedded in the binary
//!   chunk are stripped as above and the chunk laid out again around them.
//!
//! Segments and chunks are walked and dropped; nothing is decoded or
//! re-encoded, so the pixels are the napplet's, bit for bit.
//!
//! The format is read from the bytes, and from the napplet's type only when
//! the bytes name none. A file of one of these types that does not parse is
//! refused rather than sent with whatever it carries; anything else goes as
//! it is.

use serde_json::{Map, Value};

/// A format stripped here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Jpeg,
    Png,
    Webp,
    Glb,
}

impl Kind {
    /// The format the bytes announce, by their magic number.
    fn sniff(bytes: &[u8]) -> Option<Kind> {
        if bytes.starts_with(b"\xff\xd8\xff") {
            Some(Kind::Jpeg)
        } else if bytes.starts_with(PNG_SIGNATURE) {
            Some(Kind::Png)
        } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            Some(Kind::Webp)
        } else if bytes.starts_with(b"glTF") {
            Some(Kind::Glb)
        } else {
            None
        }
    }

    /// The format a MIME type names (already lowercased, no parameters).
    fn from_mime(mime: &str) -> Option<Kind> {
        match mime {
            "image/jpeg" | "image/jpg" | "image/pjpeg" => Some(Kind::Jpeg),
            "image/png" | "image/apng" => Some(Kind::Png),
            "image/webp" => Some(Kind::Webp),
            "model/gltf-binary" => Some(Kind::Glb),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Jpeg => "JPEG",
            Kind::Png => "PNG",
            Kind::Webp => "WebP",
            Kind::Glb => "GLB",
        }
    }
}

/// A file of a stripped format that does not parse as one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed {
    pub kind: Kind,
    pub why: &'static str,
}

impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not a readable {}: {}", self.kind.name(), self.why)
    }
}

/// The bytes without their metadata. `Ok(None)` for a format not stripped
/// here: the bytes go as they are. `Ok(Some)` may equal the input when there
/// was nothing to take out.
pub fn strip(bytes: &[u8], mime: &str) -> Result<Option<Vec<u8>>, Malformed> {
    let Some(kind) = Kind::sniff(bytes).or_else(|| Kind::from_mime(mime)) else {
        return Ok(None);
    };
    let stripped = match kind {
        Kind::Jpeg => jpeg(bytes),
        Kind::Png => png(bytes),
        Kind::Webp => webp(bytes),
        Kind::Glb => glb(bytes),
    };
    stripped.map(Some).map_err(|why| Malformed { kind, why })
}

/// A picture inside another file (a GLB texture): stripped when it is a
/// format stripped here, as it is otherwise.
fn strip_embedded(bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
    match Kind::sniff(bytes) {
        Some(Kind::Jpeg) => jpeg(bytes),
        Some(Kind::Png) => png(bytes),
        Some(Kind::Webp) => webp(bytes),
        // A GLB inside a GLB is not a texture; leave it to whatever reads it.
        Some(Kind::Glb) | None => Ok(bytes.to_vec()),
    }
}

// ---------------------------------------------------------------- EXIF ----

/// The EXIF Orientation tag.
const ORIENTATION: u16 = 0x0112;
/// The TIFF field type SHORT (an unsigned 16-bit integer).
const TIFF_SHORT: u16 = 3;

/// The Orientation in an EXIF TIFF block, with the block's byte order (true
/// for little-endian `II`). `None` when there is none, it is 1 (as stored),
/// it is not one of the eight defined values, or the block does not parse.
/// Every read is bounds-checked: the block is the napplet's.
fn exif_orientation(tiff: &[u8]) -> Option<(bool, u16)> {
    let little = match tiff.get(0..4)? {
        b"II*\0" => true,
        b"MM\0*" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b: [u8; 2] = tiff.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let ifd: [u8; 4] = tiff.get(4..8)?.try_into().ok()?;
    let ifd = if little {
        u32::from_le_bytes(ifd)
    } else {
        u32::from_be_bytes(ifd)
    } as usize;
    let count = u16_at(ifd)? as usize;
    for n in 0..count {
        let entry = ifd.checked_add(2)?.checked_add(n.checked_mul(12)?)?;
        if u16_at(entry)? != ORIENTATION {
            continue;
        }
        if u16_at(entry + 2)? != TIFF_SHORT {
            return None;
        }
        let value = u16_at(entry + 8)?;
        return (2..=8).contains(&value).then_some((little, value));
    }
    None
}

/// A TIFF block holding only IFD0 with the one Orientation entry: header,
/// IFD0 at offset 8, one entry, no next IFD. 26 bytes.
fn orientation_tiff(little: bool, orientation: u16) -> Vec<u8> {
    let u16b = |v: u16| {
        if little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    };
    let u32b = |v: u32| {
        if little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    };
    let mut t = Vec::with_capacity(26);
    t.extend_from_slice(if little { b"II*\0" } else { b"MM\0*" });
    t.extend_from_slice(&u32b(8));
    t.extend_from_slice(&u16b(1));
    t.extend_from_slice(&u16b(ORIENTATION));
    t.extend_from_slice(&u16b(TIFF_SHORT));
    t.extend_from_slice(&u32b(1));
    // The value, left-justified in the four-byte field.
    t.extend_from_slice(&u16b(orientation));
    t.extend_from_slice(&[0, 0]);
    t.extend_from_slice(&u32b(0));
    t
}

/// The orientation-only replacement for an EXIF TIFF block, if it needs one.
fn kept_exif(tiff: &[u8]) -> Option<Vec<u8>> {
    exif_orientation(tiff).map(|(little, o)| orientation_tiff(little, o))
}

/// What a JPEG `APP1` Exif segment and some WebP `EXIF` chunks put before
/// the TIFF block.
const EXIF_HEADER: &[u8] = b"Exif\0\0";

// ---------------------------------------------------------------- JPEG ----

/// Whether a JPEG marker segment stays. Only `APPn` and `COM` are
/// metadata; every other segment is the picture.
fn keep_jpeg_segment(marker: u8, body: &[u8]) -> bool {
    match marker {
        // JFIF says how to read the samples (density, the YCbCr default).
        0xE0 => body.starts_with(b"JFIF\0"),
        // The colour profile changes how the picture looks.
        0xE2 => body.starts_with(b"ICC_PROFILE\0"),
        // Adobe's transform flag: without it a CMYK/YCCK file decodes wrong.
        0xEE => body.starts_with(b"Adobe"),
        0xE1..=0xEF | 0xFE => false,
        _ => true,
    }
}

/// Walk the segments from start-of-image to end-of-image, keeping the
/// picture's. Entropy-coded data after each start-of-scan is copied as it is,
/// up to the next real marker (`FF00` is a stuffed byte, `FFD0`–`FFD7` a
/// restart marker, both part of the data).
///
/// The first Exif `APP1`'s Orientation, if it needs keeping, goes into a new
/// minimal `APP1` placed where readers look for it: right after start-of-image,
/// or after `APP0` JFIF when that comes first.
fn jpeg(b: &[u8]) -> Result<Vec<u8>, &'static str> {
    if !b.starts_with(&[0xFF, 0xD8]) {
        return Err("no start-of-image marker");
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(&[0xFF, 0xD8]);
    let mut i = 2;
    let mut exif_seen = false;
    let mut orientation: Option<Vec<u8>> = None;
    let mut app1_at = 2;
    loop {
        if b.get(i) != Some(&0xFF) {
            return Err("a segment does not start with a marker");
        }
        // Fill bytes: any number of 0xFF before the marker code.
        while b.get(i) == Some(&0xFF) {
            i += 1;
        }
        let Some(&marker) = b.get(i) else {
            return Err("ends before the end-of-image marker");
        };
        i += 1;
        match marker {
            // End of image. Anything after it is not the picture.
            0xD9 => {
                out.extend_from_slice(&[0xFF, 0xD9]);
                if let Some(tiff) = orientation {
                    let body_len = 2 + EXIF_HEADER.len() + tiff.len();
                    let mut app1 = vec![0xFF, 0xE1];
                    app1.extend_from_slice(&(body_len as u16).to_be_bytes());
                    app1.extend_from_slice(EXIF_HEADER);
                    app1.extend_from_slice(&tiff);
                    out.splice(app1_at..app1_at, app1);
                }
                return Ok(out);
            }
            // Markers with no length.
            0x01 | 0xD0..=0xD7 => {
                out.extend_from_slice(&[0xFF, marker]);
                continue;
            }
            0x00 | 0xD8 => return Err("a marker out of place"),
            _ => {}
        }
        let Some(len) = b.get(i..i + 2) else {
            return Err("a segment is cut short");
        };
        let len = u16::from_be_bytes([len[0], len[1]]) as usize;
        let end = i + len;
        if len < 2 || end > b.len() {
            return Err("a segment runs past the end");
        }
        let body = &b[i + 2..end];
        if marker == 0xE1 && !exif_seen && body.starts_with(EXIF_HEADER) {
            exif_seen = true;
            orientation = kept_exif(&body[EXIF_HEADER.len()..]);
        }
        if keep_jpeg_segment(marker, body) {
            let first = out.len() == 2;
            out.extend_from_slice(&[0xFF, marker]);
            out.extend_from_slice(&b[i..end]);
            if first && marker == 0xE0 {
                app1_at = out.len();
            }
        }
        i = end;
        if marker == 0xDA {
            let start = i;
            loop {
                match (b.get(i), b.get(i + 1)) {
                    (Some(0xFF), Some(0x00 | 0xD0..=0xD7)) => i += 2,
                    (Some(0xFF), Some(0xFF)) => i += 1,
                    (Some(0xFF), Some(_)) => break,
                    (Some(_), _) => i += 1,
                    (None, _) => return Err("ends inside the image data"),
                }
            }
            out.extend_from_slice(&b[start..i]);
        }
    }
}

// ----------------------------------------------------------------- PNG ----

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Whether a PNG chunk stays: every critical chunk (upper-case first
/// letter), and the ancillary ones that change how the picture looks or
/// moves. Everything else — text, time, EXIF, and whatever a tool invented —
/// is dropped: a list of what to keep cannot miss a new kind of metadata.
fn keep_png_chunk(kind: &[u8]) -> bool {
    kind[0].is_ascii_uppercase()
        || matches!(
            kind,
            b"iCCP"
                | b"gAMA"
                | b"cHRM"
                | b"sRGB"
                | b"pHYs"
                | b"tRNS"
                | b"sBIT"
                | b"bKGD"
                | b"cICP"
                | b"mDCV"
                | b"cLLI"
                | b"acTL"
                | b"fcTL"
                | b"fdAT"
        )
}

/// The CRC-32 a PNG chunk ends with (ISO-HDLC), over its type and data.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Walk the chunks from `IHDR` to `IEND`. A kept chunk is copied whole, its
/// CRC included; nothing after `IEND` is. The first `eXIf` is replaced, in
/// place, by an orientation-only one when it holds an Orientation to keep.
fn png(b: &[u8]) -> Result<Vec<u8>, &'static str> {
    if !b.starts_with(PNG_SIGNATURE) {
        return Err("no PNG signature");
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(PNG_SIGNATURE);
    let mut i = PNG_SIGNATURE.len();
    let mut exif_seen = false;
    loop {
        let Some(head) = b.get(i..i + 8) else {
            return Err("ends before IEND");
        };
        let len = u32::from_be_bytes(head[0..4].try_into().unwrap()) as usize;
        let kind = &head[4..8];
        if !kind.iter().all(u8::is_ascii_alphabetic) {
            return Err("a chunk has no name");
        }
        if i == PNG_SIGNATURE.len() && kind != b"IHDR" {
            return Err("does not start with IHDR");
        }
        let end = i
            .checked_add(12)
            .and_then(|n| n.checked_add(len))
            .filter(|end| *end <= b.len())
            .ok_or("a chunk runs past the end")?;
        if kind == b"eXIf" && !exif_seen {
            exif_seen = true;
            if let Some(tiff) = kept_exif(&b[i + 8..end - 4]) {
                let at = out.len();
                out.extend_from_slice(&(tiff.len() as u32).to_be_bytes());
                out.extend_from_slice(b"eXIf");
                out.extend_from_slice(&tiff);
                let crc = crc32(&out[at + 4..]);
                out.extend_from_slice(&crc.to_be_bytes());
            }
        } else if keep_png_chunk(kind) {
            out.extend_from_slice(&b[i..end]);
        }
        i = end;
        if kind == b"IEND" {
            return Ok(out);
        }
    }
}

// ---------------------------------------------------------------- WebP ----

/// `VP8X` flags for the EXIF and XMP chunks.
const VP8X_EXIF: u8 = 0x08;
const VP8X_XMP: u8 = 0x04;

/// Walk the RIFF chunks and keep the image's: the bitstreams, alpha,
/// animation and colour profile. `XMP ` and anything unknown go; the first
/// `EXIF` is replaced, in place, by an orientation-only one when it holds an
/// Orientation to keep (and a `VP8X` header came before it, as the format
/// requires), else dropped. The `VP8X` flags say which, and the RIFF size is
/// the new one.
fn webp(b: &[u8]) -> Result<Vec<u8>, &'static str> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WEBP" {
        return Err("no RIFF WEBP header");
    }
    let riff = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize;
    let end = 8 + riff;
    if riff < 4 || end > b.len() {
        return Err("the RIFF size is wrong");
    }
    let mut out = Vec::with_capacity(end);
    out.extend_from_slice(b"RIFF\0\0\0\0WEBP");
    let mut image = false;
    let mut vp8x_flags: Option<usize> = None;
    let mut exif_seen = false;
    let mut i = 12;
    while i < end {
        let Some(head) = b.get(i..i + 8).filter(|_| i + 8 <= end) else {
            return Err("a chunk is cut short");
        };
        let fourcc = &head[0..4];
        let size = u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize;
        let data_end = i + 8 + size;
        // Chunks are padded to an even length; a missing last pad byte is
        // tolerated, as decoders do.
        let next = (data_end + (size & 1)).min(end);
        if data_end > end {
            return Err("a chunk runs past the end");
        }
        let data = &b[i + 8..data_end];
        i = next;
        if fourcc == b"EXIF" && !exif_seen {
            exif_seen = true;
            let tiff = data.strip_prefix(EXIF_HEADER).unwrap_or(data);
            if let (Some(flags), Some(tiff)) = (vp8x_flags, kept_exif(tiff)) {
                out[flags] |= VP8X_EXIF;
                out.extend_from_slice(b"EXIF");
                out.extend_from_slice(&(tiff.len() as u32).to_le_bytes());
                out.extend_from_slice(&tiff);
            }
            continue;
        }
        if !matches!(
            fourcc,
            b"VP8X" | b"VP8 " | b"VP8L" | b"ALPH" | b"ANIM" | b"ANMF" | b"ICCP"
        ) {
            continue;
        }
        image |= matches!(fourcc, b"VP8 " | b"VP8L" | b"ANMF");
        out.extend_from_slice(head);
        if fourcc == b"VP8X" {
            if data.len() < 10 {
                return Err("the VP8X header is too short");
            }
            vp8x_flags = Some(out.len());
            out.push(data[0] & !(VP8X_EXIF | VP8X_XMP));
            out.extend_from_slice(&data[1..]);
        } else {
            out.extend_from_slice(data);
        }
        if size & 1 == 1 {
            out.push(0);
        }
    }
    if !image {
        return Err("holds no image");
    }
    let riff = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&riff.to_le_bytes());
    Ok(out)
}

// ----------------------------------------------------------------- GLB ----

const GLB_JSON: u32 = 0x4E4F_534A;
const GLB_BIN: u32 = 0x004E_4942;
/// glTF's XMP extension: a whole metadata document, anywhere in the tree.
const XMP_EXTENSION: &str = "KHR_xmp_json_ld";

/// Read the GLB container, clean its JSON, strip its embedded pictures, and
/// write it again: chunks padded to four bytes (JSON with spaces, the binary
/// chunk with zeros), lengths recomputed. A chunk after the binary one is
/// not part of glTF 2 and is dropped.
fn glb(b: &[u8]) -> Result<Vec<u8>, &'static str> {
    if b.len() < 20 || &b[0..4] != b"glTF" {
        return Err("no glTF header");
    }
    let le = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    if le(4) != 2 {
        return Err("only glTF 2 is read");
    }
    let total = le(8) as usize;
    if total < 20 || total > b.len() {
        return Err("the length in the header is wrong");
    }
    let json_len = le(12) as usize;
    let json_end = 20 + json_len;
    if le(16) != GLB_JSON || json_end > total {
        return Err("the first chunk is not the JSON");
    }
    let raw = &b[20..json_end];
    // Some writers pad with zeros rather than spaces.
    let raw = &raw[..raw.iter().rposition(|c| *c != 0).map_or(0, |p| p + 1)];
    let mut json: Value = serde_json::from_slice(raw).map_err(|_| "the JSON does not parse")?;
    if !json.is_object() {
        return Err("the JSON is not an object");
    }
    let bin = if json_end + 8 <= total && le(json_end + 4) == GLB_BIN {
        let len = le(json_end) as usize;
        let start = json_end + 8;
        if start + len > total {
            return Err("the binary chunk runs past the end");
        }
        Some(&b[start..start + len])
    } else {
        None
    };

    clean_gltf(&mut json);
    let bin = embedded_images(&mut json, bin)?;

    let mut json = serde_json::to_vec(&json).map_err(|_| "the JSON does not serialize")?;
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let mut bin = bin;
    if let Some(bin) = bin.as_mut() {
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
    }
    let total = 12 + 8 + json.len() + bin.as_ref().map_or(0, |b| 8 + b.len());
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&GLB_JSON.to_le_bytes());
    out.extend_from_slice(&json);
    if let Some(bin) = bin {
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(&GLB_BIN.to_le_bytes());
        out.extend_from_slice(&bin);
    }
    Ok(out)
}

/// Take out what a glTF says about its making: `asset.generator`, every
/// `extras` (free-form application data, by definition), and the XMP
/// extension with its mention in `extensionsUsed` / `extensionsRequired`.
fn clean_gltf(json: &mut Value) {
    fn walk(value: &mut Value) {
        match value {
            Value::Object(map) => {
                map.remove("extras");
                if let Some(Value::Object(ext)) = map.get_mut("extensions") {
                    ext.remove(XMP_EXTENSION);
                    if ext.is_empty() {
                        map.remove("extensions");
                    }
                }
                map.values_mut().for_each(walk);
            }
            Value::Array(items) => items.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(json);
    let root = json.as_object_mut().expect("checked an object");
    if let Some(Value::Object(asset)) = root.get_mut("asset") {
        asset.remove("generator");
    }
    for list in ["extensionsUsed", "extensionsRequired"] {
        if let Some(Value::Array(names)) = root.get_mut(list) {
            names.retain(|n| n.as_str() != Some(XMP_EXTENSION));
            if names.is_empty() {
                root.remove(list);
            }
        }
    }
}

/// The binary chunk with every picture an `image` keeps in it stripped.
///
/// A stripped picture is shorter, so the chunk is laid out again: each
/// buffer view on it copied in its old order, at an offset with the same
/// remainder mod 4 as before (so every accessor stays as aligned as it
/// was), its `byteOffset` and `byteLength` rewritten, and `buffers[0]`'s
/// length. Bytes no view names are dropped; nothing can reach them.
///
/// Left as it is — pictures and all — when that layout cannot be trusted
/// to be complete: views that overlap, or meshopt compression, whose
/// extension addresses the buffer by offset itself. A napplet that cares
/// strips its textures first (lightertrek re-encodes them anyway).
fn embedded_images(json: &mut Value, bin: Option<&[u8]>) -> Result<Option<Vec<u8>>, &'static str> {
    let unchanged = Ok(bin.map(<[u8]>::to_vec));
    let Some(bin) = bin else {
        return unchanged;
    };
    let image_views: Vec<usize> = json["images"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|image| image.get("bufferView")?.as_u64())
        .map(|v| v as usize)
        .collect();
    if image_views.is_empty() {
        return unchanged;
    }
    let meshopt = json["extensionsUsed"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|n| {
            matches!(
                n.as_str(),
                Some("EXT_meshopt_compression" | "KHR_meshopt_compression")
            )
        });
    // The binary chunk is buffer 0, and only when that buffer has no URI.
    if meshopt || json["buffers"][0].get("uri").is_some() {
        return unchanged;
    }
    let Some(views) = json["bufferViews"].as_array() else {
        return Err("an image names a buffer view that is not there");
    };

    // (view index, offset, length) for every view on the binary chunk.
    let mut on_bin = Vec::new();
    for (index, view) in views.iter().enumerate() {
        if view.get("buffer").and_then(Value::as_u64) != Some(0) {
            continue;
        }
        let offset = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let len = view
            .get("byteLength")
            .and_then(Value::as_u64)
            .ok_or("a buffer view has no length")? as usize;
        if offset.checked_add(len).is_none_or(|end| end > bin.len()) {
            return Err("a buffer view runs past the binary chunk");
        }
        on_bin.push((index, offset, len));
    }
    on_bin.sort_by_key(|&(_, offset, _)| offset);
    if on_bin.windows(2).any(|w| w[0].1 + w[0].2 > w[1].1) {
        return unchanged;
    }

    let mut changed = false;
    let mut out = Vec::with_capacity(bin.len());
    let mut placed = Vec::with_capacity(on_bin.len());
    for &(index, offset, len) in &on_bin {
        let old = &bin[offset..offset + len];
        let new = if image_views.contains(&index) {
            strip_embedded(old).map_err(|_| "an embedded picture does not parse")?
        } else {
            old.to_vec()
        };
        changed |= new.len() != old.len() || new != old;
        while out.len() % 4 != offset % 4 {
            out.push(0);
        }
        placed.push((index, out.len(), new.len()));
        out.extend_from_slice(&new);
    }
    if !changed {
        return unchanged;
    }
    let views = json["bufferViews"].as_array_mut().expect("read above");
    for (index, offset, len) in placed {
        let view: &mut Map<String, Value> = views[index]
            .as_object_mut()
            .ok_or("a buffer view is not an object")?;
        view.insert("byteOffset".into(), offset.into());
        view.insert("byteLength".into(), len.into());
    }
    if let Some(buffer) = json["buffers"][0].as_object_mut() {
        buffer.insert("byteLength".into(), out.len().into());
    }
    Ok(Some(out))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use serde_json::json;

    // ---- EXIF ----

    /// A phone-like EXIF TIFF block: IFD0 with Make, (optionally)
    /// Orientation, DateTime and a GPS IFD pointer; the GPS IFD with a
    /// latitude reference; and the strings they point at.
    fn phone_exif(little: bool, orientation: Option<u16>) -> Vec<u8> {
        let u16b = |v: u16| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let u32b = |v: u32| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let entry = |tag: u16, kind: u16, count: u32, value: [u8; 4]| {
            let mut e = u16b(tag).to_vec();
            e.extend_from_slice(&u16b(kind));
            e.extend_from_slice(&u32b(count));
            e.extend_from_slice(&value);
            e
        };
        let n = 3 + orientation.is_some() as u16;
        let ifd0_end = 8 + 2 + 12 * n as u32 + 4;
        let gps_at = ifd0_end;
        let gps_end = gps_at + 2 + 12 + 4;
        let make_at = gps_end;
        let date_at = make_at + 6;
        let mut t = if little {
            b"II*\0".to_vec()
        } else {
            b"MM\0*".to_vec()
        };
        t.extend_from_slice(&u32b(8));
        t.extend_from_slice(&u16b(n));
        t.extend(entry(0x010F, 2, 6, u32b(make_at)));
        if let Some(o) = orientation {
            let mut v = [0; 4];
            v[..2].copy_from_slice(&u16b(o));
            t.extend(entry(ORIENTATION, TIFF_SHORT, 1, v));
        }
        t.extend(entry(0x0132, 2, 20, u32b(date_at)));
        t.extend(entry(0x8825, 4, 1, u32b(gps_at)));
        t.extend_from_slice(&u32b(0));
        t.extend_from_slice(&u16b(1));
        t.extend(entry(0x0001, 2, 2, *b"N\0\0\0"));
        t.extend_from_slice(&u32b(0));
        t.extend_from_slice(b"Pixel\0");
        t.extend_from_slice(b"2026:10:19 12:34:56\0");
        t
    }

    #[test]
    fn exif_orientation_reads_both_byte_orders_and_rebuilds_minimal() {
        for little in [true, false] {
            let t = phone_exif(little, Some(6));
            assert_eq!(exif_orientation(&t), Some((little, 6)));
            let kept = kept_exif(&t).unwrap();
            assert_eq!(kept.len(), 26);
            assert_eq!(exif_orientation(&kept), Some((little, 6)));
            assert_eq!(kept_exif(&kept).unwrap(), kept, "rebuilding is stable");
            assert_eq!(kept_exif(&phone_exif(little, Some(1))), None);
            assert_eq!(kept_exif(&phone_exif(little, None)), None);
        }
    }

    #[test]
    fn malformed_exif_keeps_no_orientation_and_does_not_panic() {
        let good = phone_exif(false, Some(8));
        let mut huge_offset = good.clone();
        huge_offset[4..8].copy_from_slice(&[0xFF; 4]);
        // More entries than the block holds, Orientation not among those there.
        let mut huge_count = phone_exif(false, None);
        huge_count[8..10].copy_from_slice(&[0xFF; 2]);
        let mut wrong_type = good.clone();
        // Orientation is the second entry; its type field.
        wrong_type[10 + 12 + 2..10 + 12 + 4].copy_from_slice(&[0, 4]);
        let mut bad_value = good.clone();
        bad_value[10 + 12 + 8..10 + 12 + 10].copy_from_slice(&[0, 9]);
        for bad in [
            &b""[..],
            b"MM\0*",
            b"XX\0*\0\0\0\x08",
            &good[..20],
            &huge_offset,
            &huge_count,
            &wrong_type,
            &bad_value,
        ] {
            assert_eq!(exif_orientation(bad), None, "{bad:?}");
        }
        for len in 0..good.len() {
            let _ = exif_orientation(&good[..len]);
        }
    }

    // ---- JPEG ----

    fn segment(marker: u8, body: &[u8]) -> Vec<u8> {
        let mut s = vec![0xFF, marker];
        s.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
        s.extend_from_slice(body);
        s
    }

    /// A JPEG with every kind of metadata segment, a scan whose data holds a
    /// stuffed byte and a restart marker, and a "motion photo" trailer.
    fn jpeg_with_metadata() -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8];
        j.extend(segment(0xE0, b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0"));
        j.extend(segment(0xE1, b"Exif\0\0MM\0*GPS 52.37N 4.89E"));
        j.extend(segment(0xE1, b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta/>"));
        j.extend(segment(0xE2, b"ICC_PROFILE\0\x01\x01profile"));
        j.extend(segment(0xE2, b"MPF\0secret thumbnail"));
        j.extend(segment(0xED, b"Photoshop 3.0\0IPTC byline"));
        j.extend(segment(0xEE, b"Adobe\0\x64\0\0\0\0\x01"));
        j.extend(segment(0xFE, b"shot on a Pixel by alice"));
        j.extend(segment(0xDB, &[0; 65]));
        j.extend(segment(0xC0, b"\x08\0\x02\0\x03\x01\x01\x11\0"));
        j.extend(segment(0xC4, &[0; 20]));
        j.extend(segment(0xDA, b"\x01\x01\0\0\x3f\0"));
        j.extend_from_slice(&[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56]);
        j.extend_from_slice(&[0xFF, 0xD9]);
        j.extend_from_slice(b"ftypmp42 a whole video");
        j
    }

    /// The markers in order, read back with the same walker.
    fn jpeg_markers(b: &[u8]) -> Vec<u8> {
        let again = jpeg(b).expect("still a JPEG");
        assert_eq!(again, b, "stripping twice changes nothing");
        let mut markers = Vec::new();
        let mut i = 2;
        while i + 1 < b.len() {
            if b[i] == 0xFF && !matches!(b[i + 1], 0x00 | 0xFF | 0xD0..=0xD7) {
                markers.push(b[i + 1]);
                if b[i + 1] == 0xD9 {
                    break;
                }
                let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
                i += 2 + len;
            } else {
                i += 1;
            }
        }
        markers
    }

    #[test]
    fn a_jpeg_loses_exif_xmp_iptc_and_comments() {
        let original = jpeg_with_metadata();
        let out = strip(&original, "image/jpeg").unwrap().unwrap();
        assert_eq!(
            jpeg_markers(&out),
            vec![0xE0, 0xE2, 0xEE, 0xDB, 0xC0, 0xC4, 0xDA, 0xD9]
        );
        for gone in [&b"Exif"[..], b"xmpmeta", b"MPF", b"IPTC", b"Pixel", b"ftyp"] {
            assert!(
                !out.windows(gone.len()).any(|w| w == gone),
                "{gone:?} survived"
            );
        }
        // The scan data, stuffing and restart marker included, is untouched.
        let data = [0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56, 0xFF, 0xD9];
        assert!(out.ends_with(&data));
        assert!(out.windows(14).any(|w| w == b"ICC_PROFILE\0\x01\x01"));
    }

    /// A JPEG whose Exif `APP1` is the given TIFF block.
    fn jpeg_with_exif(tiff: &[u8]) -> Vec<u8> {
        let whole = jpeg_with_metadata();
        let mut body = EXIF_HEADER.to_vec();
        body.extend_from_slice(tiff);
        let mut j = whole[..2 + 18].to_vec(); // SOI + APP0
        j.extend(segment(0xE1, &body));
        j.extend(segment(0xFE, b"a comment"));
        j.extend_from_slice(&whole[2 + 18..]);
        j
    }

    #[test]
    fn a_jpeg_keeps_only_its_orientation() {
        for little in [true, false] {
            let out = strip(&jpeg_with_exif(&phone_exif(little, Some(6))), "image/jpeg")
                .unwrap()
                .unwrap();
            assert_eq!(
                jpeg_markers(&out),
                vec![0xE0, 0xE1, 0xE2, 0xEE, 0xDB, 0xC0, 0xC4, 0xDA, 0xD9]
            );
            // Right after SOI + APP0, the new APP1 and nothing more.
            let app1 = segment(0xE1, &[EXIF_HEADER, &orientation_tiff(little, 6)].concat());
            assert_eq!(&out[20..20 + app1.len()], &app1[..]);

            for gone in [&b"Pixel"[..], b"2026:10", b"GPS", b"xmpmeta", b"comment"] {
                assert!(
                    !out.windows(gone.len()).any(|w| w == gone),
                    "{gone:?} survived"
                );
            }
        }
    }

    #[test]
    fn a_jpeg_without_an_orientation_to_keep_has_no_exif() {
        let malformed = [&b"MM\0*\xff\xff\xff\xff"[..], b"", b"II"];
        let upright = phone_exif(true, Some(1));
        let none = phone_exif(false, None);
        for tiff in malformed.iter().copied().chain([&upright[..], &none[..]]) {
            let out = strip(&jpeg_with_exif(tiff), "image/jpeg").unwrap().unwrap();
            assert_eq!(
                jpeg_markers(&out),
                vec![0xE0, 0xE2, 0xEE, 0xDB, 0xC0, 0xC4, 0xDA, 0xD9]
            );
            assert!(!out.windows(4).any(|w| w == b"Exif"));
        }
    }

    #[test]
    fn a_jpeg_without_jfif_gets_its_orientation_right_after_soi() {
        let mut j = vec![0xFF, 0xD8];
        j.extend(segment(
            0xE1,
            &[EXIF_HEADER, &phone_exif(false, Some(8))].concat(),
        ));
        j.extend_from_slice(&jpeg_with_metadata()[2 + 18..]);
        let out = strip(&j, "image/jpeg").unwrap().unwrap();
        assert_eq!(jpeg_markers(&out)[0], 0xE1);
        assert_eq!(&out[2..4], &[0xFF, 0xE1]);
        assert_eq!(exif_orientation(&out[12..38]), Some((false, 8)));
    }

    #[test]
    fn a_broken_jpeg_is_refused() {
        let whole = jpeg_with_metadata();
        let eoi = whole.windows(2).rposition(|w| w == [0xFF, 0xD9]).unwrap();
        for broken in [&whole[..eoi], &whole[..30], &b"\xff\xd8\xff"[..]] {
            let e = strip(broken, "image/jpeg").unwrap_err();
            assert_eq!(e.kind, Kind::Jpeg);
        }
        // Said to be a JPEG, and is not one.
        assert!(strip(b"hello", "image/jpeg").is_err());
    }

    // ---- PNG ----

    pub(crate) fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(kind);
        c.extend_from_slice(data);
        let crc = crc32(&c[4..]);
        c.extend_from_slice(&crc.to_be_bytes());
        c
    }

    /// A 1x1 PNG, real down to the CRCs, with the given chunks between the
    /// header and the image data.
    pub(crate) fn png_with(extra: &[Vec<u8>]) -> Vec<u8> {
        let mut p = PNG_SIGNATURE.to_vec();
        p.extend(chunk(b"IHDR", b"\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0"));
        for c in extra {
            p.extend_from_slice(c);
        }
        p.extend(chunk(b"IDAT", b"\x78\x9c\x63\x60\0\0\0\x02\0\x01"));
        p.extend(chunk(b"IEND", b""));
        p
    }

    fn png_chunks(b: &[u8]) -> Vec<String> {
        assert_eq!(
            png(b).expect("still a PNG"),
            b,
            "stripping twice changes nothing"
        );
        let mut kinds = Vec::new();
        let mut i = 8;
        while i < b.len() {
            let len = u32::from_be_bytes(b[i..i + 4].try_into().unwrap()) as usize;
            let kind = &b[i + 4..i + 8];
            let crc = u32::from_be_bytes(b[i + 8 + len..i + 12 + len].try_into().unwrap());
            assert_eq!(crc, crc32(&b[i + 4..i + 8 + len]), "CRC kept intact");
            kinds.push(String::from_utf8(kind.to_vec()).unwrap());
            i += 12 + len;
        }
        kinds
    }

    #[test]
    fn a_png_loses_its_text_time_and_exif() {
        let original = png_with(&[
            chunk(b"iCCP", b"sRGB\0\0profile"),
            chunk(b"gAMA", &[0, 0, 0xB1, 0x8F]),
            chunk(b"tEXt", b"Author\0alice"),
            chunk(b"zTXt", b"Comment\0\0xx"),
            chunk(b"iTXt", b"XML:com.adobe.xmp\0\0\0\0\0<x:xmpmeta/>"),
            chunk(b"eXIf", b"MM\0*GPS"),
            chunk(b"tIME", &[7, 0xEA, 10, 15, 12, 0, 0]),
            chunk(b"pHYs", &[0, 0, 0x0B, 0x13, 0, 0, 0x0B, 0x13, 1]),
            chunk(b"tRNS", &[0, 0]),
            chunk(b"vpAg", b"made up by some tool"),
        ]);
        let mut trailing = original.clone();
        trailing.extend_from_slice(b"appended after IEND");
        let out = strip(&trailing, "application/octet-stream")
            .unwrap()
            .unwrap();
        assert_eq!(
            png_chunks(&out),
            ["IHDR", "iCCP", "gAMA", "pHYs", "tRNS", "IDAT", "IEND"]
        );
        // A clean file comes out as it went in.
        let clean = png_with(&[]);
        assert_eq!(strip(&clean, "image/png").unwrap().unwrap(), clean);
    }

    #[test]
    fn a_png_keeps_only_its_orientation() {
        for little in [true, false] {
            let original = png_with(&[
                chunk(b"eXIf", &phone_exif(little, Some(6))),
                chunk(b"tEXt", b"Author\0alice"),
            ]);
            let out = strip(&original, "image/png").unwrap().unwrap();
            assert_eq!(png_chunks(&out), ["IHDR", "eXIf", "IDAT", "IEND"]);
            let kept = chunk(b"eXIf", &orientation_tiff(little, 6));
            assert!(out.windows(kept.len()).any(|w| w == kept));
            assert!(!out.windows(5).any(|w| w == b"Pixel"));
        }
        for tiff in [phone_exif(true, Some(1)), b"MM\0*GPS".to_vec()] {
            let out = strip(&png_with(&[chunk(b"eXIf", &tiff)]), "image/png")
                .unwrap()
                .unwrap();
            assert_eq!(png_chunks(&out), ["IHDR", "IDAT", "IEND"]);
        }
    }

    #[test]
    fn a_broken_png_is_refused() {
        let whole = png_with(&[chunk(b"tEXt", b"a\0b")]);
        for broken in [&whole[..whole.len() - 12], &whole[..20], PNG_SIGNATURE] {
            assert_eq!(strip(broken, "image/png").unwrap_err().kind, Kind::Png);
        }
        let mut no_ihdr = PNG_SIGNATURE.to_vec();
        no_ihdr.extend(chunk(b"IEND", b""));
        assert!(strip(&no_ihdr, "image/png").is_err());
    }

    // ---- WebP ----

    fn riff(fourcc: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = fourcc.to_vec();
        c.extend_from_slice(&(data.len() as u32).to_le_bytes());
        c.extend_from_slice(data);
        if data.len() % 2 == 1 {
            c.push(0);
        }
        c
    }

    fn webp_of(chunks: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = chunks.concat();
        let mut w = b"RIFF".to_vec();
        w.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
        w.extend_from_slice(b"WEBP");
        w.extend(body);
        w
    }

    fn webp_chunks(b: &[u8]) -> Vec<String> {
        assert_eq!(
            webp(b).expect("still a WebP"),
            b,
            "stripping twice changes nothing"
        );
        let riff = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize;
        assert_eq!(riff + 8, b.len(), "RIFF size matches");
        let mut kinds = Vec::new();
        let mut i = 12;
        while i < b.len() {
            let size = u32::from_le_bytes(b[i + 4..i + 8].try_into().unwrap()) as usize;
            kinds.push(String::from_utf8(b[i..i + 4].to_vec()).unwrap());
            i += 8 + size + (size & 1);
        }
        kinds
    }

    #[test]
    fn a_webp_loses_exif_and_xmp_and_says_so_in_vp8x() {
        // Flags: ICC, alpha, EXIF, XMP.
        let vp8x = [
            0x20 | 0x10 | VP8X_EXIF | VP8X_XMP,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        let original = webp_of(&[
            riff(b"VP8X", &vp8x),
            riff(b"ICCP", b"profile"),
            riff(b"ALPH", b"\0alpha"),
            riff(b"VP8L", b"\x2f\0\0\0\0lossless"),
            riff(b"EXIF", b"MM\0*GPS 52.37N"),
            riff(b"XMP ", b"<x:xmpmeta>alice</x:xmpmeta>"),
        ]);
        let out = strip(&original, "image/webp").unwrap().unwrap();
        assert_eq!(webp_chunks(&out), ["VP8X", "ICCP", "ALPH", "VP8L"]);
        assert_eq!(
            out[20],
            0x20 | 0x10,
            "EXIF and XMP flags cleared, the rest kept"
        );
        assert!(!out.windows(5).any(|w| w == b"alice"));
        assert!(!out.windows(3).any(|w| w == b"GPS"));
    }

    #[test]
    fn a_webp_keeps_only_its_orientation() {
        let vp8x = [0x10 | VP8X_EXIF | VP8X_XMP, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        for (little, prefix) in [(true, &b""[..]), (false, EXIF_HEADER)] {
            let original = webp_of(&[
                riff(b"VP8X", &vp8x),
                riff(b"VP8L", b"\x2f\0\0\0\0lossless"),
                riff(b"EXIF", &[prefix, &phone_exif(little, Some(6))].concat()),
                riff(b"XMP ", b"<x:xmpmeta>alice</x:xmpmeta>"),
            ]);
            let out = strip(&original, "image/webp").unwrap().unwrap();
            assert_eq!(webp_chunks(&out), ["VP8X", "VP8L", "EXIF"]);
            assert_eq!(out[20], 0x10 | VP8X_EXIF, "EXIF still announced, XMP not");
            assert!(out.ends_with(&riff(b"EXIF", &orientation_tiff(little, 6))));
            assert!(!out.windows(5).any(|w| w == b"Pixel"));
        }
        // Upright: no EXIF chunk, no flag.
        let upright = webp_of(&[
            riff(b"VP8X", &vp8x),
            riff(b"VP8L", b"\x2f\0\0\0\0lossless"),
            riff(b"EXIF", &phone_exif(true, Some(1))),
        ]);
        let out = strip(&upright, "image/webp").unwrap().unwrap();
        assert_eq!(webp_chunks(&out), ["VP8X", "VP8L"]);
        assert_eq!(out[20], 0x10);
        // A simple-format file cannot carry EXIF; it is dropped.
        let simple = webp_of(&[
            riff(b"VP8L", b"\x2f\0\0\0\0lossless"),
            riff(b"EXIF", &phone_exif(true, Some(6))),
        ]);
        let out = strip(&simple, "image/webp").unwrap().unwrap();
        assert_eq!(webp_chunks(&out), ["VP8L"]);
    }

    #[test]
    fn a_broken_webp_is_refused() {
        let whole = webp_of(&[riff(b"VP8 ", b"\0\0\0\x9d\x01\x2a")]);
        let mut wrong_size = whole.clone();
        wrong_size[4] = 0xFF;
        let no_image = webp_of(&[riff(b"EXIF", b"x")]);
        for broken in [&whole[..whole.len() - 3], &wrong_size[..], &no_image[..]] {
            assert_eq!(strip(broken, "image/webp").unwrap_err().kind, Kind::Webp);
        }
        assert!(strip(&whole, "image/webp").is_ok());
    }

    // ---- GLB ----

    fn glb_of(json: &Value, bin: Option<&[u8]>) -> Vec<u8> {
        let mut j = serde_json::to_vec(json).unwrap();
        while !j.len().is_multiple_of(4) {
            j.push(b' ');
        }
        let mut out = b"glTF".to_vec();
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(j.len() as u32).to_le_bytes());
        out.extend_from_slice(&GLB_JSON.to_le_bytes());
        out.extend(j);
        if let Some(bin) = bin {
            let mut bin = bin.to_vec();
            while !bin.len().is_multiple_of(4) {
                bin.push(0);
            }
            out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
            out.extend_from_slice(&GLB_BIN.to_le_bytes());
            out.extend(bin);
        }
        let total = out.len() as u32;
        out[8..12].copy_from_slice(&total.to_le_bytes());
        out
    }

    /// Read a GLB back: its JSON and binary chunk, with the container's
    /// lengths and padding checked.
    fn read_glb(b: &[u8]) -> (Value, Vec<u8>) {
        let le = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
        assert_eq!(&b[0..4], b"glTF");
        assert_eq!(le(8), b.len(), "total length");
        let json_len = le(12);
        assert_eq!(json_len % 4, 0, "JSON padded");
        let json = serde_json::from_slice(&b[20..20 + json_len]).unwrap();
        let at = 20 + json_len;
        let bin = if at < b.len() {
            assert_eq!(le(at) % 4, 0, "BIN padded");
            assert_eq!(at + 8 + le(at), b.len());
            b[at + 8..].to_vec()
        } else {
            Vec::new()
        };
        (json, bin)
    }

    #[test]
    fn a_glb_loses_its_generator_extras_and_xmp() {
        let gltf = json!({
            "asset": {
                "version": "2.0",
                "copyright": "CC-BY-4.0 alice",
                "generator": "Blender 4.2 on /home/alice",
                "extras": {"author": "alice"},
                "extensions": {"KHR_xmp_json_ld": {"packet": 0}},
            },
            "extensionsUsed": ["KHR_xmp_json_ld", "KHR_materials_unlit"],
            "extensions": {"KHR_xmp_json_ld": {"packets": [{"dc:creator": "alice"}]}},
            "nodes": [{"name": "trail", "extras": {"gps": [52.37, 4.89]}}],
            "materials": [{"extensions": {"KHR_materials_unlit": {}}, "extras": {}}],
        });
        let out = strip(&glb_of(&gltf, None), "model/gltf-binary")
            .unwrap()
            .unwrap();
        let (json, bin) = read_glb(&out);
        assert!(bin.is_empty());
        assert_eq!(
            json,
            json!({
                "asset": {"version": "2.0", "copyright": "CC-BY-4.0 alice"},
                "extensionsUsed": ["KHR_materials_unlit"],
                "nodes": [{"name": "trail"}],
                "materials": [{"extensions": {"KHR_materials_unlit": {}}}],
            })
        );
    }

    #[test]
    fn a_glbs_embedded_pictures_are_stripped_and_the_buffer_laid_out_again() {
        let picture = png_with(&[chunk(b"tEXt", b"Author\0alice")]);
        let clean_picture = png_with(&[]);
        let positions: Vec<u8> = (0u8..12).collect();
        // positions at 0, the picture at 12, indices after it at an odd
        // remainder of 2 mod 4 (two-byte indices).
        let pic_at = 12;
        let idx_at = (pic_at + picture.len()).next_multiple_of(4) + 2;
        let mut bin = vec![0u8; idx_at + 6];
        bin[..12].copy_from_slice(&positions);
        bin[pic_at..pic_at + picture.len()].copy_from_slice(&picture);
        bin[idx_at..idx_at + 6].copy_from_slice(&[0, 0, 1, 0, 2, 0]);
        let gltf = json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": bin.len()}],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 12},
                {"buffer": 0, "byteOffset": pic_at, "byteLength": picture.len()},
                {"buffer": 0, "byteOffset": idx_at, "byteLength": 6},
            ],
            "images": [{"bufferView": 1, "mimeType": "image/png"}],
        });
        let out = strip(&glb_of(&gltf, Some(&bin)), "model/gltf-binary")
            .unwrap()
            .unwrap();
        let (json, bin) = read_glb(&out);
        let view = |i: usize| {
            let v = &json["bufferViews"][i];
            let at = v["byteOffset"].as_u64().unwrap() as usize;
            &bin[at..at + v["byteLength"].as_u64().unwrap() as usize]
        };
        assert_eq!(view(0), &positions[..]);
        assert_eq!(
            view(1),
            &clean_picture[..],
            "the picture lost its text chunk"
        );
        assert_eq!(view(2), &[0, 0, 1, 0, 2, 0]);
        assert_eq!(
            json["bufferViews"][2]["byteOffset"].as_u64().unwrap() % 4,
            2
        );
        assert!(json["buffers"][0]["byteLength"].as_u64().unwrap() as usize <= bin.len());
        assert!(!bin.windows(5).any(|w| w == b"alice"));
    }

    #[test]
    fn a_broken_glb_is_refused() {
        let good = glb_of(&json!({"asset": {"version": "2.0"}}), None);
        let mut bad_json = good.clone();
        bad_json[20] = b'[';
        let mut long = good.clone();
        long[8] = 0xFF;
        for broken in [&good[..16], &bad_json[..], &long[..]] {
            assert_eq!(
                strip(broken, "model/gltf-binary").unwrap_err().kind,
                Kind::Glb
            );
        }
        // A picture view that runs off the end of the binary chunk.
        let gltf = json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": 4}],
            "bufferViews": [{"buffer": 0, "byteLength": 400}],
            "images": [{"bufferView": 0}],
        });
        assert!(strip(&glb_of(&gltf, Some(&[0; 4])), "model/gltf-binary").is_err());
    }

    #[test]
    fn other_files_go_as_they_are() {
        assert_eq!(strip(b"GIF89a\x01\0\x01\0", "image/gif").unwrap(), None);
        assert_eq!(strip(b"plain text", "text/plain").unwrap(), None);
        assert_eq!(strip(&[0, 159, 146, 150], "").unwrap(), None);
    }
}
