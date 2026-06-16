//! Content-type inference from a path extension.
//!
//! The manifest maps `path -> sha256` only — there is no stored MIME type, so the
//! extension is the type signal (`docs/design/nsite-layer.md` §4.2 step 4). This
//! is a small hand-rolled table (no `mime_guess` dependency) covering the file
//! kinds a static nsite serves; anything unknown falls back to
//! `application/octet-stream`.

/// Infer a `Content-Type` (including a `; charset=utf-8` for text types) from the
/// path's extension.
pub fn from_path(path: &str) -> &'static str {
    let ext = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext)
        .unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "txt" | "md" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        "wasm.gz" => "application/wasm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_common_types() {
        assert_eq!(from_path("/index.html"), "text/html; charset=utf-8");
        assert_eq!(from_path("/app.JS"), "text/javascript; charset=utf-8");
        assert_eq!(from_path("/a/b/style.css"), "text/css; charset=utf-8");
        assert_eq!(from_path("/favicon.ico"), "image/x-icon");
        assert_eq!(from_path("/logo.png"), "image/png");
        assert_eq!(from_path("/noext"), "application/octet-stream");
        assert_eq!(from_path("/archive.unknown"), "application/octet-stream");
    }
}
