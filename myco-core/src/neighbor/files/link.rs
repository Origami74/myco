//! A shared file's link: filedrop's download link, so the napplet that takes
//! this over (roadmap N13) opens what Myco shares.
//!
//! ```text
//! https://napplet.link/file/download?enc=1&mime=…&name=…&size=…&x=<sha256>#k=<base64url key>
//! ```
//!
//! `x` is the hash of the sealed blob, `name`, `mime` and `size` the file's
//! own. The key rides only in the fragment, which no browser sends to
//! napplet.link's web host.

use base64::Engine as _;

pub const PREFIX: &str = "https://napplet.link/file/download";

/// One shared file, as its link says.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileLink {
    /// sha256 of the blob as stored (sealed when `encrypted`).
    pub x: String,
    pub name: String,
    pub mime: String,
    /// The file's own size, bytes.
    pub size: u64,
    pub encrypted: bool,
    #[serde(skip)]
    pub key: Option<[u8; super::seal::KEY_BYTES]>,
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

impl FileLink {
    /// The link, params sorted as filedrop writes them.
    pub fn to_url(&self) -> String {
        let size = self.size.to_string();
        let mut params = vec![
            ("mime", self.mime.as_str()),
            ("name", self.name.as_str()),
            ("size", size.as_str()),
            ("x", self.x.as_str()),
        ];
        if self.encrypted {
            params.push(("enc", "1"));
        }
        params.sort();
        let query: Vec<String> = params
            .iter()
            .map(|(k, v)| format!("{k}={}", encode(v)))
            .collect();
        let mut url = format!("{PREFIX}?{}", query.join("&"));
        if let Some(key) = &self.key {
            url.push_str("#k=");
            url.push_str(&base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key));
        }
        url
    }

    /// Read a link (`https://napplet.link/file/download?…#k=…`, any case of
    /// scheme and host), or `None` if it isn't one Myco shares: sealed, with
    /// its key, and held on no public server. Such a link may still be a
    /// filedrop file in another neighborhood; whether Nearby can serve it is
    /// [`Neighbors::nearby_has`](crate::neighbor::Neighbors::nearby_has).
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let head = text.get(..PREFIX.len())?;
        if !head.eq_ignore_ascii_case(PREFIX) {
            return None;
        }
        let rest = &text[PREFIX.len()..];
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        let rest = rest.strip_prefix('?')?;
        let (query, fragment) = rest.split_once('#').unwrap_or((rest, ""));
        let mut x = None;
        let mut name = None;
        let mut mime = None;
        let mut size = None;
        let mut encrypted = false;
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let v = decode(v)?;
            match k {
                "x" => x = Some(v.to_ascii_lowercase()),
                "name" => name = Some(v),
                "mime" => mime = Some(v),
                "size" => size = v.parse().ok(),
                "enc" => encrypted = v == "1",
                // Held on public servers: filedrop's link, not a Nearby
                // file. Left to whichever app takes `file/download`.
                "servers" | "url" => return None,
                _ => {}
            }
        }
        let key = fragment
            .split('&')
            .find_map(|p| p.strip_prefix("k="))
            .and_then(|k| {
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(k.trim_end_matches('='))
                    .ok()
            })
            .and_then(|k| <[u8; super::seal::KEY_BYTES]>::try_from(k).ok());
        let x = x.filter(|x| x.len() == 64 && x.bytes().all(|b| b.is_ascii_hexdigit()))?;
        // Myco only ever shares sealed files, with their key. Anything else
        // (unencrypted, or without its key) is filedrop's, for whichever
        // app takes `file/download`.
        if !encrypted || key.is_none() {
            return None;
        }
        Some(Self {
            x,
            name: super::clean(&name.unwrap_or_default()),
            mime: mime
                .as_deref()
                .and_then(super::clean_mime)
                .unwrap_or_else(|| "application/octet-stream".into()),
            size: size.unwrap_or(0),
            encrypted,
            key,
        })
    }
}
