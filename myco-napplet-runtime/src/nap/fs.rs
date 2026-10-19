//! NAP-FS — files the user picks, read and written in ranges.
//!
//! The spec is the upstream draft, napplet/naps PR #88
//! (<https://github.com/napplet/naps/pull/88>, `naps/NAP-FS.md`). Myco's
//! choices where it leaves room are in `docs/design/napplet/NAP-FS.md`.
//!
//! Myco implements the **picker subset**. A napplet sees nothing until the
//! user picks a file for it; then it sees that file, under a virtual path, for
//! as long as the window is open.
//!
//! - `fs.info` — one root, `/picked`, and the limits below.
//! - `fs.pickFile`, `fs.pickFiles`, `fs.pickSaveFile` — the system file
//!   picker. Only the host can show one, so a valid pick comes back as
//!   [`Outcome::FsPick`] and the host answers it with
//!   [`PickRequest::answer`] or [`PickRequest::cancelled`].
//! - `fs.stat`, `fs.read`, `fs.write` — on picked entries only.
//! - Everything else (`pickDirectory`, `list`, `mkdir`, `remove`, `move`,
//!   `watch`, `unwatch`) answers `unsupported`. Never silence: the domain is
//!   offered, so a call the napplet makes is one it waits on.
//!
//! **Paths are opaque.** Every path a napplet can use was minted here, at
//! pick time, as `/picked/<name>`. A path is checked against the spec's path
//! model and then looked up exactly; one the store does not hold is
//! `not-found`, however it is spelled. Nothing a napplet sends is ever
//! resolved against a host filesystem — there is none behind this: an entry
//! is a [`PickedBytes`] the platform opened.
//!
//! **Grants are per window.** The entries live in the session
//! ([`Session::picked`]) and go with it: the window closing drops every
//! handle, which on Android closes the descriptors. Nothing is persisted.
//!
//! **Bytes** are standard padded base64 on the wire, both ways, and every
//! limit counts decoded bytes (spec, Byte Encoding).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use serde_json::{json, Value};

use crate::dispatch::Outcome;
use crate::seams::{Envelope, PickedBytes, PickedFile};
use crate::session::Session;

/// The most bytes one `fs.read` returns. Each crosses the shell's JSON
/// channel as base64, a third larger, and is copied a few times on the way.
pub const MAX_READ_BYTES: u64 = 1024 * 1024;
/// The most decoded bytes one `fs.write` carries.
pub const MAX_WRITE_BYTES: u64 = 1024 * 1024;
/// The most decoded bytes a session's reads and writes may hold at once.
pub const MAX_IN_FLIGHT_BYTES: u64 = 4 * 1024 * 1024;
/// The most picked entries one session holds. Past it, the oldest are
/// dropped — revoked, as the spec allows at any time — to make room.
pub const MAX_PICKED_ENTRIES: usize = 64;
/// The one visible root. Every picked entry is a direct child of it.
pub const ROOT: &str = "/picked";

// The spec's `FsError` values used here.
pub const NOT_FOUND: &str = "not-found";
pub const INVALID_PATH: &str = "invalid-path";
pub const INVALID_DATA: &str = "invalid-data";
pub const PERMISSION_DENIED: &str = "permission-denied";
pub const POLICY_DENIED: &str = "policy-denied";
pub const QUOTA_EXCEEDED: &str = "quota-exceeded";
pub const TOO_LARGE: &str = "too-large";
pub const UNSUPPORTED: &str = "unsupported";
pub const CONFLICT: &str = "conflict";
pub const CANCELLED: &str = "cancelled";
pub const IO_ERROR: &str = "io-error";

/// The most MIME types handed to the platform picker for one pick.
const MAX_ACCEPT: usize = 16;
/// The longest name kept for a picked entry or a suggested save name.
const MAX_NAME_CHARS: usize = 120;

/// Handle an inbound `fs.*` message.
pub fn handle(session: &Session, message: &Envelope) -> Outcome {
    let reply = |e: Envelope| Outcome::Reply(vec![e]);
    match message.action() {
        "info" => reply(info(message)),
        "pickFile" => pick(message, PickMode::Open),
        "pickFiles" => pick(message, PickMode::OpenMultiple),
        "pickSaveFile" => pick(message, PickMode::Save),
        "stat" => reply(stat(session, message)),
        "read" => reply(read(session, message)),
        "write" => reply(write(session, message)),
        // The rest of the spec's surface: recognized, offered (the namespace
        // has them), and not built. Said so, never silence — the napplet is
        // waiting on a result.
        "pickDirectory" | "list" | "mkdir" | "remove" | "move" | "watch" | "unwatch" => {
            reply(message.to_error(UNSUPPORTED))
        }
        // An unrecognized action is silence, as NIP-5D asks.
        _ => Outcome::Reply(Vec::new()),
    }
}

/// `fs.info` — the subset, honestly: one root holding only what was picked,
/// no watches, and the byte limits.
fn info(message: &Envelope) -> Envelope {
    message.to_result().with_field(
        "info",
        json!({
            "roots": [{
                "path": ROOT,
                "name": "Files you choose",
                "permissions": ["read", "write", "create"],
                "description": "Only files you pick or save for this app, until it closes.",
            }],
            "limits": {
                "maxReadBytes": MAX_READ_BYTES,
                "maxWriteBytes": MAX_WRITE_BYTES,
                "maxWatchCount": 0,
                "maxInFlightBytes": MAX_IN_FLIGHT_BYTES,
            },
        }),
    )
}

// --- picking ---------------------------------------------------------------

/// Which system picker a pick asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickMode {
    /// One existing file (`fs.pickFile`).
    Open,
    /// One or more existing files (`fs.pickFiles`).
    OpenMultiple,
    /// One destination to save to (`fs.pickSaveFile`).
    Save,
}

impl PickMode {
    /// The name the window host reads.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::OpenMultiple => "open-multiple",
            Self::Save => "save",
        }
    }
}

/// A valid pick, awaiting the user. The host shows the picker and answers
/// with [`PickRequest::answer`] (the files the platform opened) or
/// [`PickRequest::cancelled`]; left unanswered, the napplet's call times out.
///
/// Everything here is a hint for the picker, taken from the napplet's
/// options and cleaned. None of it is authority: what the user picks, and
/// what the platform could open, decide the entry and its permissions.
#[derive(Debug, Clone, PartialEq)]
pub struct PickRequest {
    pub mode: PickMode,
    /// MIME types for the picker's filter; `["*/*"]` when the napplet gave
    /// none that mean anything. Advisory, as the spec says.
    pub mime_types: Vec<String>,
    /// The name a save picker starts with.
    pub suggested_name: Option<String>,
    /// The MIME type a save picker creates the file as.
    pub save_mime: String,
    /// Whether to ask the platform for read access to the chosen files.
    pub open_for_read: bool,
    /// Whether to ask the platform for write access to the chosen files.
    pub open_for_write: bool,
    /// The permission intent, after defaults, limited to what a picked file
    /// can ever carry here.
    requested: Vec<&'static str>,
    call: Envelope,
}

impl PickRequest {
    /// The answer once the platform opened what the user picked: each file
    /// gets a virtual path in `session`'s store, and the napplet the entries.
    ///
    /// No files is a cancel (the spec forbids an empty success). A file the
    /// platform could not open the way it was needed is left out; if that
    /// leaves nothing, `io-error`. A session that lost its `fs` grant while
    /// the picker was up gets nothing registered.
    pub fn answer(&self, session: &Session, files: Vec<PickedFile>) -> Envelope {
        if !session.may_service("fs") {
            return self.call.to_error(POLICY_DENIED);
        }
        if files.is_empty() {
            return self.cancelled();
        }
        let wanted = match self.mode {
            PickMode::Open | PickMode::Save => 1,
            PickMode::OpenMultiple => MAX_PICKED_ENTRIES,
        };
        let mut entries = Vec::new();
        for file in files.into_iter().take(wanted) {
            let permissions = self.granted(&file);
            if permissions.is_empty() {
                tracing::info!(name = %file.name, "picked file not opened as needed; left out");
                continue;
            }
            let save_target = self.mode == PickMode::Save;
            entries.push(session.picked().register(file, permissions, save_target));
        }
        if entries.is_empty() {
            return self.call.to_error(IO_ERROR);
        }
        let entries: Vec<Value> = entries.iter().map(|e| e.picked_json()).collect();
        self.call
            .to_result()
            .with_field("result", json!({ "entries": entries }))
    }

    /// The answer when the user backed out of the picker.
    pub fn cancelled(&self) -> Envelope {
        self.call.to_error(CANCELLED)
    }

    /// The answer when the host refused or failed the pick: one of the
    /// spec's `FsError` strings.
    pub fn failed(&self, error: &str) -> Envelope {
        self.call.to_error(error)
    }

    /// The permissions `file` carries: the intent, narrowed to what the
    /// platform actually opened.
    fn granted(&self, file: &PickedFile) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for &p in &self.requested {
            let ok = match p {
                "read" => file.readable,
                "write" | "create" => file.writable,
                _ => false,
            };
            if ok {
                out.push(p);
            }
        }
        // A save target is one the napplet must be able to write; `create`
        // alone would be a destination it could never fill.
        if self.mode == PickMode::Save && !out.contains(&"write") && !out.contains(&"create") {
            return Vec::new();
        }
        out
    }
}

fn pick(message: &Envelope, mode: PickMode) -> Outcome {
    let options = message.field("options").and_then(Value::as_object);
    let asked: Vec<&str> = options
        .and_then(|o| o.get("permissions"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    // What a picked file can carry here. `list`, `delete`, `watch` never:
    // a picked file is one file, and removing it is not built.
    let carriable: &[&'static str] = match mode {
        PickMode::Open | PickMode::OpenMultiple => &["read", "write"],
        PickMode::Save => &["read", "write", "create"],
    };
    let mut requested: Vec<&'static str> = carriable
        .iter()
        .copied()
        .filter(|p| asked.contains(p))
        .collect();
    // The spec's defaults when absent — and the same when nothing asked for
    // is one this subset can give.
    if requested.is_empty() {
        requested = match mode {
            PickMode::Open | PickMode::OpenMultiple => vec!["read"],
            PickMode::Save => vec!["write", "create"],
        };
    }
    let accept = options
        .and_then(|o| o.get("accept"))
        .and_then(Value::as_array);
    let mime_types = accept_mime_types(accept.map(Vec::as_slice).unwrap_or(&[]));
    let suggested_name = options
        .and_then(|o| o.get("suggestedName"))
        .and_then(Value::as_str)
        .map(clean_name)
        .filter(|n| n != "file");
    let save_mime = mime_types
        .iter()
        .find(|m| !m.contains('*'))
        .cloned()
        .or_else(|| {
            suggested_name
                .as_deref()
                .and_then(|n| n.rsplit_once('.'))
                .and_then(|(_, ext)| mime_for_extension(ext))
                .map(str::to_string)
        })
        .unwrap_or_else(|| "application/octet-stream".to_string());
    Outcome::FsPick(PickRequest {
        mode,
        mime_types,
        suggested_name,
        save_mime,
        open_for_read: requested.contains(&"read"),
        open_for_write: requested.contains(&"write") || requested.contains(&"create"),
        requested,
        call: message.clone(),
    })
}

/// The picker's MIME filter from the napplet's `accept` rules: each `mime`
/// that is a MIME type, and each `extension` this table knows. Advisory —
/// with nothing usable, every type.
pub fn accept_mime_types(rules: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for rule in rules {
        let Some(rule) = rule.as_object() else {
            continue;
        };
        // Each type with the other names Android files the same files under:
        // a CSV in Downloads is `text/comma-separated-values`, and a filter of
        // `text/csv` alone left it greyed out in the picker.
        let mut push = |m: String| {
            for name in std::iter::once(m.as_str()).chain(mime_aliases(&m).iter().copied()) {
                if !out.iter().any(|o| o == name) && out.len() < MAX_ACCEPT {
                    out.push(name.to_string());
                }
            }
        };
        if let Some(mime) = rule
            .get("mime")
            .and_then(Value::as_str)
            .and_then(clean_mime)
        {
            push(mime);
        }
        if let Some(ext) = rule.get("extension").and_then(Value::as_str) {
            let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
            if let Some(mime) = mime_for_extension(&ext) {
                push(mime.to_string());
            }
        }
    }
    if out.is_empty() {
        out.push("*/*".to_string());
    }
    out
}

/// Other MIME types that name the same kind of file on Android.
fn mime_aliases(mime: &str) -> &'static [&'static str] {
    match mime {
        "text/csv" => &[
            "text/comma-separated-values",
            "application/csv",
            "text/x-csv",
        ],
        "text/tab-separated-values" => &["text/tsv"],
        "application/json" => &["text/json"],
        "text/markdown" => &["text/x-markdown"],
        "application/gpx+xml" => &["application/octet-stream"],
        "model/gltf-binary" => &["application/octet-stream"],
        _ => &[],
    }
}

/// A MIME type or range (`type/subtype`, `type/*`, `*/*`), lowercased and
/// without parameters; `None` for anything else.
fn clean_mime(raw: &str) -> Option<String> {
    let essence = raw.split(';').next()?.trim().to_ascii_lowercase();
    let (kind, sub) = essence.split_once('/')?;
    let token = |s: &str| {
        !s.is_empty()
            && s.len() <= 64
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    let ok = (kind == "*" && sub == "*") || (token(kind) && (sub == "*" || token(sub)));
    ok.then_some(essence)
}

/// The MIME type for a file extension (no dot, lowercase), for the ones a
/// napplet is likely to filter on. Unknown is `None`: the picker is then not
/// narrowed by it.
pub fn mime_for_extension(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "txt" | "text" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "ics" => "text/calendar",
        "vcf" => "text/vcard",
        "json" => "application/json",
        "xml" => "application/xml",
        "gpx" => "application/gpx+xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "epub" => "application/epub+zip",
        "wasm" => "application/wasm",
        "sqlite" | "sqlite3" | "db" => "application/vnd.sqlite3",
        "bin" => "application/octet-stream",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        _ => return None,
    })
}

/// A path segment from untrusted text (a provider's display name, a
/// napplet's suggested name): no separators, controls or bidi formatting,
/// trimmed and capped. `file` when nothing usable is left.
pub fn clean_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() || is_bidi_format(c) {
                '_'
            } else {
                c
            }
        })
        .take(MAX_NAME_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "file".to_string()
    } else {
        cleaned.to_string()
    }
}

/// Unicode bidirectional formatting characters — the path model forbids
/// them, since they make a name read as something else.
fn is_bidi_format(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// Check `path` against the spec's path model. Paths here are compared code
/// point for code point, case-sensitively: every valid one was minted by
/// this runtime, so there is nothing to normalize.
pub fn validate_path(path: &str) -> Result<(), &'static str> {
    if !path.starts_with('/') {
        return Err(INVALID_PATH);
    }
    if path == "/" {
        return Ok(());
    }
    if path
        .chars()
        .any(|c| c == '\\' || c.is_control() || is_bidi_format(c))
    {
        return Err(INVALID_PATH);
    }
    for segment in path[1..].split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(INVALID_PATH);
        }
    }
    Ok(())
}

// --- the picked-file store ---------------------------------------------------

/// One picked entry: a virtual path over the platform's handle.
struct Entry {
    path: String,
    name: String,
    mime: Option<String>,
    permissions: Vec<&'static str>,
    bytes: Arc<dyn PickedBytes>,
    /// Serializes writes to the entry, so concurrent appends never
    /// interleave and a replace is not split by a patch.
    write_lock: Mutex<()>,
    /// A save destination not yet written: absent, for `ifAbsent`, as the
    /// spec lets a `pickSaveFile` entry be until the napplet writes it.
    unwritten: AtomicBool,
}

impl Entry {
    fn has(&self, permission: &str) -> bool {
        self.permissions.contains(&permission)
    }

    /// The spec's `FsPickedEntry`. No timestamps: the spec asks for the
    /// least metadata a picker result can carry.
    fn picked_json(&self) -> Value {
        let mut out = json!({
            "path": self.path,
            "kind": "file",
            "name": self.name,
            "permissions": self.permissions,
        });
        if let Ok(size) = self.bytes.size() {
            out["size"] = json!(size);
        }
        out
    }
}

/// A picked file, as another feature takes it: NAP-UPLOAD sending it to
/// Blossom, mesh file sharing offering it to a peer. Holding one keeps the
/// platform handle open even after the window closed; drop it when done.
#[derive(Clone)]
pub struct PickedHandle {
    pub path: String,
    pub name: String,
    pub mime: Option<String>,
    pub permissions: Vec<String>,
    pub bytes: Arc<dyn PickedBytes>,
}

impl std::fmt::Debug for PickedHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PickedHandle")
            .field("path", &self.path)
            .field("name", &self.name)
            .field("permissions", &self.permissions)
            .finish_non_exhaustive()
    }
}

/// One session's picked entries — the whole of what NAP-FS lets a napplet
/// see. Shared by every snapshot of the session, and dropped with it.
#[derive(Default)]
pub struct PickedStore {
    entries: Mutex<VecDeque<Arc<Entry>>>,
    /// Decoded bytes held by reads and writes running now.
    in_flight: AtomicU64,
}

impl std::fmt::Debug for PickedStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PickedStore")
            .field("entries", &self.len())
            .finish_non_exhaustive()
    }
}

impl PickedStore {
    /// How many entries are held.
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every virtual path held, oldest first.
    pub fn paths(&self) -> Vec<String> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.path.clone())
            .collect()
    }

    /// The picked file at `path`, for a feature that sends it on. `None` for
    /// a path this session was never given.
    pub fn get(&self, path: &str) -> Option<PickedHandle> {
        self.entry(path).map(|e| PickedHandle {
            path: e.path.clone(),
            name: e.name.clone(),
            mime: e.mime.clone(),
            permissions: e.permissions.iter().map(|p| p.to_string()).collect(),
            bytes: e.bytes.clone(),
        })
    }

    /// Drop every entry: revoke what was picked.
    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }

    fn entry(&self, path: &str) -> Option<Arc<Entry>> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.path == path)
            .cloned()
    }

    /// Give `file` a path of its own under [`ROOT`] — its cleaned name, with
    /// ` (2)`, ` (3)`… before the extension when that is taken — and keep it.
    fn register(
        &self,
        file: PickedFile,
        permissions: Vec<&'static str>,
        save_target: bool,
    ) -> Arc<Entry> {
        let mut entries = self.entries.lock().unwrap();
        let base = clean_name(&file.name);
        let taken = |p: &str| entries.iter().any(|e| e.path == p);
        let mut name = base.clone();
        let mut n = 2;
        while taken(&format!("{ROOT}/{name}")) {
            name = match base.rsplit_once('.') {
                Some((stem, ext)) if !stem.is_empty() => format!("{stem} ({n}).{ext}"),
                _ => format!("{base} ({n})"),
            };
            n += 1;
        }
        let entry = Arc::new(Entry {
            path: format!("{ROOT}/{name}"),
            name,
            mime: file.mime,
            permissions,
            bytes: file.bytes,
            write_lock: Mutex::new(()),
            unwritten: AtomicBool::new(save_target),
        });
        while entries.len() >= MAX_PICKED_ENTRIES {
            entries.pop_front();
        }
        entries.push_back(entry.clone());
        entry
    }

    /// Hold `n` decoded bytes against [`MAX_IN_FLIGHT_BYTES`] until the
    /// guard drops; `None` when that would go over.
    fn reserve(&self, n: u64) -> Option<Reserved<'_>> {
        let mut held = self.in_flight.load(Ordering::Acquire);
        loop {
            let total = held.checked_add(n).filter(|&t| t <= MAX_IN_FLIGHT_BYTES)?;
            // Another pick may have moved the count since the load; retry
            // against what is held now.
            match self.in_flight.compare_exchange_weak(
                held,
                total,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(Reserved { store: self, n }),
                Err(now) => held = now,
            }
        }
    }
}

struct Reserved<'a> {
    store: &'a PickedStore,
    n: u64,
}

impl Drop for Reserved<'_> {
    fn drop(&mut self) {
        self.store.in_flight.fetch_sub(self.n, Ordering::AcqRel);
    }
}

// --- stat / read / write -------------------------------------------------------

/// The entry a call names, or the error to answer with.
fn named_entry(session: &Session, message: &Envelope) -> Result<Arc<Entry>, &'static str> {
    let path = message
        .field("path")
        .and_then(Value::as_str)
        .ok_or(INVALID_PATH)?;
    validate_path(path)?;
    session.picked().entry(path).ok_or(NOT_FOUND)
}

/// An optional non-negative integer option; `Err` when present and not one.
fn uint_option(
    options: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Result<Option<u64>, &'static str> {
    match options.and_then(|o| o.get(key)) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or(INVALID_DATA),
    }
}

/// The call's `options`: absent, or an object.
fn options_of(message: &Envelope) -> Result<Option<&serde_json::Map<String, Value>>, &'static str> {
    match message.field("options") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(o)) => Ok(Some(o)),
        Some(_) => Err(INVALID_DATA),
    }
}

/// `fs.stat` — the spec's `FsMetadata`, coarse: kind, size, permissions. No
/// timestamps and no `revision` (revisions are not exposed).
fn stat(session: &Session, message: &Envelope) -> Envelope {
    let entry = match named_entry(session, message) {
        Ok(entry) => entry,
        Err(e) => return message.to_error(e),
    };
    let mut metadata = json!({
        "path": entry.path,
        "kind": "file",
        "permissions": entry.permissions,
    });
    match entry.bytes.size() {
        Ok(size) => metadata["size"] = json!(size),
        Err(e) => {
            tracing::info!(error = %e, "picked file stat failed");
            return message.to_error(IO_ERROR);
        }
    }
    message.to_result().with_field("metadata", metadata)
}

/// `fs.read` — one range, at most [`MAX_READ_BYTES`], read straight from
/// the platform handle at the offset.
fn read(session: &Session, message: &Envelope) -> Envelope {
    match try_read(session, message) {
        Ok(result) => message.to_result().with_field("result", result),
        Err(e) => message.to_error(e),
    }
}

fn try_read(session: &Session, message: &Envelope) -> Result<Value, &'static str> {
    let entry = named_entry(session, message)?;
    if !entry.has("read") {
        return Err(PERMISSION_DENIED);
    }
    let options = options_of(message)?;
    let offset = uint_option(options, "offset")?.unwrap_or(0);
    let length = uint_option(options, "length")?.unwrap_or(MAX_READ_BYTES);
    if length > MAX_READ_BYTES {
        return Err(TOO_LARGE);
    }
    let size = entry.bytes.size().map_err(io_error)?;
    let want = length.min(size.saturating_sub(offset));
    let _held = session.picked().reserve(want).ok_or(POLICY_DENIED)?;
    let mut buf = vec![0u8; want as usize];
    let mut got = 0usize;
    while got < buf.len() {
        let n = entry
            .bytes
            .read_at(&mut buf[got..], offset + got as u64)
            .map_err(io_error)?;
        if n == 0 {
            break;
        }
        got += n;
    }
    buf.truncate(got);
    // The size now, not before: a file that grew under the read is not at
    // its end yet.
    let size = entry.bytes.size().unwrap_or(size);
    Ok(json!({
        "data": base64::engine::general_purpose::STANDARD.encode(&buf),
        "offset": offset,
        "bytesRead": got,
        "eof": offset + got as u64 >= size,
        "size": size,
    }))
}

/// How a write lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteMode {
    Replace,
    Append,
    Patch(u64),
}

/// `fs.write` — replace, append or patch, at most [`MAX_WRITE_BYTES`].
fn write(session: &Session, message: &Envelope) -> Envelope {
    match try_write(session, message) {
        Ok(result) => message.to_result().with_field("result", result),
        Err(e) => message.to_error(e),
    }
}

fn try_write(session: &Session, message: &Envelope) -> Result<Value, &'static str> {
    let entry = named_entry(session, message)?;
    let options = options_of(message)?;
    let offset = uint_option(options, "offset")?;
    let mode = match options.and_then(|o| o.get("mode")) {
        None | Some(Value::Null) => "replace",
        Some(Value::String(m)) => m.as_str(),
        Some(_) => return Err(INVALID_DATA),
    };
    // The spec's combinations: `offset` with `patch` and only with it.
    let mode = match (mode, offset) {
        ("replace", None) => WriteMode::Replace,
        ("append", None) => WriteMode::Append,
        ("patch", Some(at)) => WriteMode::Patch(at),
        _ => return Err(INVALID_DATA),
    };
    // No revisions are exposed, so a revision precondition cannot be met.
    if options.is_some_and(|o| o.get("ifRevision").is_some_and(|v| !v.is_null())) {
        return Err(UNSUPPORTED);
    }
    let if_absent = match options.and_then(|o| o.get("ifAbsent")) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(INVALID_DATA),
    };

    let Some(Value::String(b64)) = message.field("data") else {
        return Err(INVALID_DATA);
    };
    // Refused on length before the work of decoding it: four characters per
    // three bytes, padded.
    if b64.len() as u64 > MAX_WRITE_BYTES.div_ceil(3) * 4 {
        return Err(TOO_LARGE);
    }
    // STANDARD: the RFC 4648 alphabet, padding required, no whitespace, and
    // non-canonical trailing bits refused — the spec's rules exactly.
    let data = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|_| INVALID_DATA)?;
    if data.len() as u64 > MAX_WRITE_BYTES {
        return Err(TOO_LARGE);
    }

    // Writing an existing file needs `write`; filling an absent save target
    // is creating it, which `create` covers too.
    let _locked = entry.write_lock.lock().unwrap();
    let absent = entry.unwritten.load(Ordering::Acquire);
    let allowed = entry.has("write") || (absent && entry.has("create"));
    if !allowed {
        return Err(PERMISSION_DENIED);
    }
    if if_absent && !absent {
        return Err(CONFLICT);
    }
    let _held = session
        .picked()
        .reserve(data.len() as u64)
        .ok_or(POLICY_DENIED)?;

    let bytes = &entry.bytes;
    let size = bytes.size().map_err(io_error)?;
    match mode {
        WriteMode::Replace => {
            bytes.write_all_at(&data, 0).map_err(io_error)?;
            bytes.set_len(data.len() as u64).map_err(io_error)?;
        }
        WriteMode::Append => bytes.write_all_at(&data, size).map_err(io_error)?,
        // A hole past the end is not something every provider can make.
        WriteMode::Patch(at) if at > size => return Err(UNSUPPORTED),
        WriteMode::Patch(at) => bytes.write_all_at(&data, at).map_err(io_error)?,
    }
    bytes.sync().map_err(io_error)?;
    entry.unwritten.store(false, Ordering::Release);
    let size = bytes.size().map_err(io_error)?;
    Ok(json!({ "bytesWritten": data.len(), "size": size }))
}

/// The spec's error for a backing-store failure: `quota-exceeded` when the
/// storage is full, `io-error` otherwise. The detail goes to the log only.
fn io_error(e: std::io::Error) -> &'static str {
    tracing::info!(error = %e, "picked file I/O failed");
    // ENOSPC on Linux/Android and macOS alike.
    if e.raw_os_error() == Some(28) {
        QUOTA_EXCEEDED
    } else {
        IO_ERROR
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::dispatch;
    use crate::session::NappletIdentity;
    use crate::testing::{test_context, MemFile};

    fn session(granted: &[&str]) -> Session {
        let mut s = Session::new(NappletIdentity::new("notes", "agg"), granted.to_vec());
        s.on_ready();
        s
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn unb64(v: &Value) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(v.as_str().unwrap())
            .unwrap()
    }

    async fn call(s: &mut Session, e: Envelope) -> Value {
        let (ctx, _signer) = test_context();
        let out = dispatch(&ctx, s, &e).await;
        let sent = out.envelopes();
        assert_eq!(sent.len(), 1, "{out:?}");
        serde_json::to_value(&sent[0]).unwrap()
    }

    async fn pick_request(s: &mut Session, e: Envelope) -> PickRequest {
        let (ctx, _signer) = test_context();
        match dispatch(&ctx, s, &e).await {
            Outcome::FsPick(request) => request,
            other => panic!("expected a pick, got {other:?}"),
        }
    }

    fn file(name: &str, file: &Arc<MemFile>, writable: bool) -> PickedFile {
        PickedFile {
            name: name.to_string(),
            mime: Some("text/plain".to_string()),
            readable: true,
            writable,
            bytes: file.clone(),
        }
    }

    fn read_msg(path: &str, options: Value) -> Envelope {
        let e = Envelope::new("fs.read")
            .with_id("r")
            .with_field("path", path);
        if options.is_null() {
            e
        } else {
            e.with_field("options", options)
        }
    }

    fn write_msg(path: &str, data: &str, options: Value) -> Envelope {
        let e = Envelope::new("fs.write")
            .with_id("w")
            .with_field("path", path)
            .with_field("data", data);
        if options.is_null() {
            e
        } else {
            e.with_field("options", options)
        }
    }

    /// Pick one file through the host round trip and return its path.
    async fn picked(s: &mut Session, name: &str, f: &Arc<MemFile>, writable: bool) -> String {
        let req = pick_request(s, Envelope::new("fs.pickFile").with_id("p")).await;
        let out = serde_json::to_value(req.answer(s, vec![file(name, f, writable)])).unwrap();
        out["result"]["entries"][0]["path"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The headline flow: pick, the picker's answer registers the file under
    /// a virtual path, then range reads walk it to the end.
    #[tokio::test]
    async fn pick_then_read_in_ranges() {
        let mut s = session(&["fs"]);
        let req = pick_request(
            &mut s,
            Envelope::new("fs.pickFile").with_id("p1").with_field(
                "options",
                json!({"permissions": ["read"], "accept": [{"mime": "text/plain"}, {"extension": ".md"}]}),
            ),
        )
        .await;
        assert_eq!(req.mode, PickMode::Open);
        assert_eq!(
            req.mime_types,
            ["text/plain", "text/markdown", "text/x-markdown"]
        );
        assert!(req.open_for_read && !req.open_for_write);

        let f = Arc::new(MemFile::new(b"Hello world!"));
        let out = serde_json::to_value(req.answer(&s, vec![file("report.md", &f, false)])).unwrap();
        assert_eq!(
            out,
            json!({
                "type": "fs.pickFile.result",
                "id": "p1",
                "result": {"entries": [{
                    "path": "/picked/report.md",
                    "kind": "file",
                    "name": "report.md",
                    "permissions": ["read"],
                    "size": 12,
                }]},
            })
        );

        let r = call(
            &mut s,
            read_msg("/picked/report.md", json!({"offset": 6, "length": 5})),
        )
        .await;
        assert_eq!(r["type"], "fs.read.result");
        assert_eq!(unb64(&r["result"]["data"]), b"world");
        assert_eq!(r["result"]["offset"], 6);
        assert_eq!(r["result"]["bytesRead"], 5);
        assert_eq!(r["result"]["eof"], false);
        assert_eq!(r["result"]["size"], 12);

        // The rest, by default length: to the end, and eof.
        let r = call(&mut s, read_msg("/picked/report.md", json!({"offset": 11}))).await;
        assert_eq!(unb64(&r["result"]["data"]), b"!");
        assert_eq!(r["result"]["eof"], true);

        // Past the end: nothing, at eof — never an error.
        let r = call(&mut s, read_msg("/picked/report.md", json!({"offset": 40}))).await;
        assert_eq!(r["result"]["data"], "");
        assert_eq!(r["result"]["bytesRead"], 0);
        assert_eq!(r["result"]["eof"], true);

        // Whole file, no options.
        let r = call(&mut s, read_msg("/picked/report.md", Value::Null)).await;
        assert_eq!(unb64(&r["result"]["data"]), b"Hello world!");

        // A read only touches the range asked for.
        assert_eq!(f.largest_read(), 12);

        let st = call(
            &mut s,
            Envelope::new("fs.stat")
                .with_id("s")
                .with_field("path", "/picked/report.md"),
        )
        .await;
        assert_eq!(
            st["metadata"],
            json!({"path": "/picked/report.md", "kind": "file", "permissions": ["read"], "size": 12})
        );
    }

    /// A range read reads that range and no more — never the whole file.
    #[tokio::test]
    async fn a_range_read_reads_only_the_range() {
        let mut s = session(&["fs"]);
        let big = Arc::new(MemFile::new(&vec![7u8; 3 * 1024 * 1024]));
        let path = picked(&mut s, "big.bin", &big, false).await;
        let r = call(
            &mut s,
            read_msg(&path, json!({"offset": 2_000_000, "length": 100})),
        )
        .await;
        assert_eq!(r["result"]["bytesRead"], 100);
        assert!(
            big.largest_read() <= 100,
            "read {} bytes",
            big.largest_read()
        );

        // Over the cap: too-large, nothing read.
        let r = call(
            &mut s,
            read_msg(&path, json!({"length": MAX_READ_BYTES + 1})),
        )
        .await;
        assert_eq!(r["error"], "too-large");
        assert!(r.get("result").is_none());
    }

    /// Save: a writable entry, filled by replace, appended to and patched,
    /// each answering the bytes written and the size after.
    #[tokio::test]
    async fn save_then_replace_append_patch() {
        let mut s = session(&["fs"]);
        let req = pick_request(
            &mut s,
            Envelope::new("fs.pickSaveFile").with_id("p3").with_field(
                "options",
                json!({"suggestedName": "export.json", "accept": [{"mime": "application/json"}]}),
            ),
        )
        .await;
        assert_eq!(req.mode, PickMode::Save);
        assert_eq!(req.suggested_name.as_deref(), Some("export.json"));
        assert_eq!(req.save_mime, "application/json");
        assert!(req.open_for_write);

        let f = Arc::new(MemFile::new(b""));
        let out =
            serde_json::to_value(req.answer(&s, vec![file("export.json", &f, true)])).unwrap();
        let entry = &out["result"]["entries"][0];
        assert_eq!(entry["path"], "/picked/export.json");
        assert_eq!(entry["permissions"], json!(["write", "create"]));
        let path = "/picked/export.json";

        // Never written: absent, so a create-only write goes through.
        let r = call(
            &mut s,
            write_msg(path, &b64(b"Hello world!"), json!({"ifAbsent": true})),
        )
        .await;
        assert_eq!(r["result"], json!({"bytesWritten": 12, "size": 12}));
        // And now it exists.
        let r = call(
            &mut s,
            write_msg(path, &b64(b"x"), json!({"ifAbsent": true})),
        )
        .await;
        assert_eq!(r["error"], "conflict");
        assert_eq!(f.contents(), b"Hello world!");

        let r = call(
            &mut s,
            write_msg(path, &b64(b"!!"), json!({"mode": "append"})),
        )
        .await;
        assert_eq!(r["result"], json!({"bytesWritten": 2, "size": 14}));
        let r = call(
            &mut s,
            write_msg(path, "AAECAw==", json!({"mode": "patch", "offset": 1})),
        )
        .await;
        assert_eq!(r["result"], json!({"bytesWritten": 4, "size": 14}));
        assert_eq!(f.contents(), b"H\0\x01\x02\x03 world!!!");
        let r = call(&mut s, write_msg(path, &b64(b"short"), Value::Null)).await;
        assert_eq!(r["result"], json!({"bytesWritten": 5, "size": 5}));
        assert_eq!(f.contents(), b"short");

        // Write-only: not readable unless asked for.
        let r = call(&mut s, read_msg(path, Value::Null)).await;
        assert_eq!(r["error"], "permission-denied");
    }

    /// The spec's write option rules, each refused without touching the file.
    #[tokio::test]
    async fn bad_writes_are_refused_and_change_nothing() {
        let mut s = session(&["fs"]);
        let f = Arc::new(MemFile::new(b"keep"));
        let path = picked(&mut s, "a.txt", &f, true).await;
        // pickFile asked for read only: no write.
        let r = call(&mut s, write_msg(&path, &b64(b"x"), Value::Null)).await;
        assert_eq!(r["error"], "permission-denied");

        let req = pick_request(
            &mut s,
            Envelope::new("fs.pickFile")
                .with_id("p")
                .with_field("options", json!({"permissions": ["read", "write"]})),
        )
        .await;
        assert!(req.open_for_write);
        let out = serde_json::to_value(req.answer(&s, vec![file("a.txt", &f, true)])).unwrap();
        let path = out["result"]["entries"][0]["path"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            path, "/picked/a (2).txt",
            "a second pick of one name gets its own path"
        );

        for (data, options, error) in [
            ("eA", Value::Null, "invalid-data"),     // no padding
            ("eA==\n", Value::Null, "invalid-data"), // whitespace
            ("-_8=", Value::Null, "invalid-data"),   // base64url
            ("eB==", Value::Null, "invalid-data"),   // non-canonical bits
            ("eA==", json!({"mode": "patch"}), "invalid-data"),
            (
                "eA==",
                json!({"mode": "replace", "offset": 0}),
                "invalid-data",
            ),
            (
                "eA==",
                json!({"mode": "append", "offset": 1}),
                "invalid-data",
            ),
            ("eA==", json!({"mode": "truncate"}), "invalid-data"),
            (
                "eA==",
                json!({"mode": "patch", "offset": -1}),
                "invalid-data",
            ),
            (
                "eA==",
                json!({"mode": "patch", "offset": 99}),
                "unsupported",
            ),
            ("eA==", json!({"ifRevision": "r1"}), "unsupported"),
            ("eA==", json!({"ifAbsent": true}), "conflict"),
        ] {
            let r = call(&mut s, write_msg(&path, data, options.clone())).await;
            assert_eq!(r["error"], error, "{data} {options}");
            assert!(r.get("result").is_none());
            assert_eq!(f.contents(), b"keep", "{data} {options} changed the file");
        }
        let r = call(&mut s, write_msg(&path, "", Value::Null)).await;
        assert_eq!(
            r["result"],
            json!({"bytesWritten": 0, "size": 0}),
            "empty bytes are \"\""
        );

        let too_big = b64(&vec![0u8; MAX_WRITE_BYTES as usize + 1]);
        let r = call(&mut s, write_msg(&path, &too_big, Value::Null)).await;
        assert_eq!(r["error"], "too-large");
    }

    /// A path is opaque: only what was picked resolves, and a malformed one
    /// is invalid before it is looked up.
    #[tokio::test]
    async fn only_picked_paths_resolve() {
        let mut s = session(&["fs"]);
        let f = Arc::new(MemFile::new(b"secret"));
        picked(&mut s, "a.txt", &f, false).await;
        for path in [
            "/picked/b.txt",
            "/picked",
            "/",
            "/picked/A.txt",
            "/etc/passwd",
            "/sdcard/a.txt",
        ] {
            let r = call(&mut s, read_msg(path, Value::Null)).await;
            assert_eq!(r["error"], "not-found", "{path}");
        }
        for path in [
            "",
            "picked/a.txt",
            "/picked/../picked/a.txt",
            "/picked/./a.txt",
            "/picked//a.txt",
            "/picked/a.txt/",
            "C:\\picked\\a.txt",
            "/picked\\a.txt",
            "file:///picked/a.txt",
            "/picked/a\u{202E}txt.exe",
            "/picked/a\u{0}.txt",
        ] {
            let r = call(&mut s, read_msg(path, Value::Null)).await;
            assert_eq!(r["error"], "invalid-path", "{path:?}");
        }
        let r = call(&mut s, Envelope::new("fs.read").with_id("r")).await;
        assert_eq!(r["error"], "invalid-path");
    }

    /// A hostile display name never becomes a path that escapes or lies.
    #[test]
    fn display_names_are_cleaned_into_one_segment() {
        assert_eq!(clean_name("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(clean_name("a\u{202E}gpj.exe"), "a_gpj.exe");
        assert_eq!(clean_name(".."), "file");
        assert_eq!(clean_name("  "), "file");
        assert_eq!(clean_name("x\\y\n"), "x_y_");
        assert!(validate_path(&format!("{ROOT}/{}", clean_name("../x"))).is_ok());
        assert_eq!(clean_name(&"n".repeat(500)).chars().count(), MAX_NAME_CHARS);
    }

    /// Cancel, and the spec's rule that an empty pick is a cancel.
    #[tokio::test]
    async fn a_cancelled_pick_is_cancelled() {
        let mut s = session(&["fs"]);
        let req = pick_request(&mut s, Envelope::new("fs.pickFile").with_id("p4")).await;
        assert_eq!(
            serde_json::to_value(req.cancelled()).unwrap(),
            json!({"type": "fs.pickFile.result", "id": "p4", "error": "cancelled"})
        );
        let empty = serde_json::to_value(req.answer(&s, Vec::new())).unwrap();
        assert_eq!(empty["error"], "cancelled");
        assert!(s.picked().is_empty());
    }

    /// pickFiles keeps every file; pickFile exactly one.
    #[tokio::test]
    async fn pick_files_returns_each_and_pick_file_one() {
        let mut s = session(&["fs"]);
        let a = Arc::new(MemFile::new(b"a"));
        let b = Arc::new(MemFile::new(b"bb"));
        let req = pick_request(&mut s, Envelope::new("fs.pickFiles").with_id("m")).await;
        assert_eq!(req.mode, PickMode::OpenMultiple);
        assert_eq!(req.mime_types, ["*/*"]);
        let out = serde_json::to_value(
            req.answer(&s, vec![file("a.txt", &a, false), file("b.txt", &b, false)]),
        )
        .unwrap();
        assert_eq!(out["type"], "fs.pickFiles.result");
        assert_eq!(out["result"]["entries"].as_array().unwrap().len(), 2);

        let req = pick_request(&mut s, Envelope::new("fs.pickFile").with_id("o")).await;
        let out = serde_json::to_value(
            req.answer(&s, vec![file("c.txt", &a, false), file("d.txt", &b, false)]),
        )
        .unwrap();
        assert_eq!(out["result"]["entries"].as_array().unwrap().len(), 1);
        assert_eq!(s.picked().len(), 3);
    }

    /// A file the platform could not open as the pick needed is left out.
    #[tokio::test]
    async fn an_unopenable_pick_is_an_io_error() {
        let mut s = session(&["fs"]);
        let req = pick_request(&mut s, Envelope::new("fs.pickSaveFile").with_id("p")).await;
        let f = Arc::new(MemFile::new(b""));
        // The provider would not open the destination for writing.
        let out = serde_json::to_value(req.answer(&s, vec![file("x.bin", &f, false)])).unwrap();
        assert_eq!(out["error"], "io-error");
        assert!(s.picked().is_empty());
    }

    /// Not granted: refused before any picker, and no read either.
    #[tokio::test]
    async fn an_ungranted_fs_is_policy_denied() {
        let mut s = session(&["relay"]);
        for msg in [
            Envelope::new("fs.pickFile").with_id("p"),
            Envelope::new("fs.info").with_id("i"),
            read_msg("/picked/a.txt", Value::Null),
        ] {
            let r = call(&mut s, msg).await;
            assert_eq!(r["error"], "policy-denied", "{r}");
        }

        // A grant revoked while the picker was up registers nothing.
        let mut s = session(&["fs"]);
        let req = pick_request(&mut s, Envelope::new("fs.pickFile").with_id("p")).await;
        s.set_granted(Vec::<String>::new());
        let f = Arc::new(MemFile::new(b"a"));
        let out = serde_json::to_value(req.answer(&s, vec![file("a.txt", &f, false)])).unwrap();
        assert_eq!(out["error"], "policy-denied");
        assert!(s.picked().is_empty());
    }

    /// What is not built says `unsupported`, never silence.
    #[tokio::test]
    async fn the_rest_of_the_surface_is_unsupported() {
        let mut s = session(&["fs"]);
        for t in [
            "fs.pickDirectory",
            "fs.list",
            "fs.mkdir",
            "fs.remove",
            "fs.move",
            "fs.watch",
            "fs.unwatch",
        ] {
            let r = call(
                &mut s,
                Envelope::new(t).with_id("x").with_field("path", "/picked"),
            )
            .await;
            assert_eq!(r["type"], format!("{t}.result"));
            assert_eq!(r["id"], "x");
            assert_eq!(r["error"], "unsupported", "{t}");
        }
        let (ctx, _signer) = test_context();
        let out = dispatch(&ctx, &mut s, &Envelope::new("fs.teleport").with_id("x")).await;
        assert!(out.envelopes().is_empty(), "an unknown action is silence");
    }

    /// `fs.info` says what is there: one root, no watches, the limits.
    #[tokio::test]
    async fn info_reports_the_subset() {
        let mut s = session(&["fs"]);
        let r = call(&mut s, Envelope::new("fs.info").with_id("i1")).await;
        assert_eq!(r["type"], "fs.info.result");
        let info = &r["info"];
        assert_eq!(info["roots"].as_array().unwrap().len(), 1);
        assert_eq!(info["roots"][0]["path"], "/picked");
        assert_eq!(info["limits"]["maxReadBytes"], MAX_READ_BYTES);
        assert_eq!(info["limits"]["maxWriteBytes"], MAX_WRITE_BYTES);
        assert_eq!(info["limits"]["maxWatchCount"], 0);
        assert_eq!(info["limits"]["maxInFlightBytes"], MAX_IN_FLIGHT_BYTES);
    }

    /// The grants live in the session and die with it: once every clone is
    /// gone, so is the platform handle (on Android, the descriptor closes).
    #[tokio::test]
    async fn closing_the_session_drops_the_handles() {
        let mut s = session(&["fs"]);
        let f = Arc::new(MemFile::new(b"a"));
        let weak = Arc::downgrade(&f);
        let path = picked(&mut s, "a.txt", &f, false).await;
        drop(f);
        // A snapshot shares the store: a read on it sees the pick.
        let mut snapshot = s.clone();
        let r = call(&mut snapshot, read_msg(&path, Value::Null)).await;
        assert_eq!(unb64(&r["result"]["data"]), b"a");
        // A new session for the same napplet sees none of it.
        let mut fresh = session(&["fs"]);
        let r = call(&mut fresh, read_msg(&path, Value::Null)).await;
        assert_eq!(r["error"], "not-found");

        drop(snapshot);
        assert!(weak.upgrade().is_some());
        drop(s);
        assert!(weak.upgrade().is_none(), "the handle outlived its session");
    }

    /// The in-flight cap: bytes held by running reads and writes, counted
    /// decoded, refused past [`MAX_IN_FLIGHT_BYTES`] and released after.
    #[test]
    fn in_flight_bytes_are_capped_and_released() {
        let store = PickedStore::default();
        let a = store.reserve(MAX_IN_FLIGHT_BYTES - 10).unwrap();
        assert!(store.reserve(11).is_none());
        let b = store.reserve(10).unwrap();
        drop(a);
        drop(b);
        assert!(store.reserve(MAX_IN_FLIGHT_BYTES).is_some());
    }

    /// A full store makes room by dropping the oldest pick.
    #[tokio::test]
    async fn the_store_is_bounded() {
        let mut s = session(&["fs"]);
        let f = Arc::new(MemFile::new(b"a"));
        let first = picked(&mut s, "first.txt", &f, false).await;
        for i in 0..MAX_PICKED_ENTRIES {
            picked(&mut s, &format!("{i}.txt"), &f, false).await;
        }
        assert_eq!(s.picked().len(), MAX_PICKED_ENTRIES);
        assert!(s.picked().get(&first).is_none());
    }

    /// The accessor other features take a picked file by.
    #[tokio::test]
    async fn a_picked_file_can_be_taken_whole() {
        let mut s = session(&["fs"]);
        let f = Arc::new(MemFile::new(b"whole file"));
        let path = picked(&mut s, "w.txt", &f, false).await;
        let handle = s.picked().get(&path).unwrap();
        assert_eq!(handle.name, "w.txt");
        assert_eq!(handle.mime.as_deref(), Some("text/plain"));
        assert_eq!(handle.permissions, ["read"]);
        assert_eq!(handle.bytes.read_all(1024).unwrap(), b"whole file");
        assert!(handle.bytes.read_all(3).is_err());
    }

    #[test]
    fn accept_rules_map_to_mime_types() {
        let m = |v: Value| accept_mime_types(v.as_array().unwrap());
        assert_eq!(m(json!([])), ["*/*"]);
        assert_eq!(
            m(json!([{"extension": "PNG"}, {"extension": ".jpg"}])),
            ["image/png", "image/jpeg"]
        );
        assert_eq!(
            m(json!([{"mime": "image/*"}, {"mime": "Text/Plain; charset=utf-8"}])),
            ["image/*", "text/plain"]
        );
        assert_eq!(
            m(json!([{"mime": "not a type"}, {"extension": ".nope"}, 7])),
            ["*/*"]
        );
        assert_eq!(
            m(json!([{"mime": "text/plain"}, {"extension": "txt"}])),
            ["text/plain"]
        );
        // A CSV in Android's Downloads is `text/comma-separated-values`:
        // asked for as `.csv` or `text/csv`, it must still be choosable.
        let csv = m(json!([{"extension": ".csv"}, {"mime": "text/csv"}]));
        assert_eq!(csv[0], "text/csv");
        assert!(csv.iter().any(|t| t == "text/comma-separated-values"));
        assert_eq!(
            csv.len(),
            csv.iter().collect::<std::collections::HashSet<_>>().len()
        );
    }

    /// The save picker's type: a concrete accepted one, else the suggested
    /// name's, else octet-stream.
    #[tokio::test]
    async fn the_save_type_comes_from_accept_or_name() {
        let mut s = session(&["fs"]);
        let save = |options: Value| {
            Envelope::new("fs.pickSaveFile")
                .with_id("p")
                .with_field("options", options)
        };
        let r = pick_request(&mut s, save(json!({"suggestedName": "track.gpx"}))).await;
        assert_eq!(r.save_mime, "application/gpx+xml");
        let r = pick_request(
            &mut s,
            save(json!({"accept": [{"mime": "image/*"}], "suggestedName": "x"})),
        )
        .await;
        assert_eq!(r.save_mime, "application/octet-stream");
        let r = pick_request(&mut s, save(json!({"suggestedName": "../../evil.txt"}))).await;
        assert_eq!(r.suggested_name.as_deref(), Some(".._.._evil.txt"));
    }

    /// A failing backing store is an `io-error`, and a full one is
    /// `quota-exceeded`.
    #[tokio::test]
    async fn storage_failures_map_to_spec_errors() {
        let mut s = session(&["fs"]);
        let req = pick_request(&mut s, Envelope::new("fs.pickSaveFile").with_id("p")).await;
        let full = Arc::new(MemFile::failing_writes(28));
        let broken = Arc::new(MemFile::failing_writes(5));
        let out =
            serde_json::to_value(req.answer(&s, vec![file("full.bin", &full, true)])).unwrap();
        let full_path = out["result"]["entries"][0]["path"]
            .as_str()
            .unwrap()
            .to_string();
        let req = pick_request(&mut s, Envelope::new("fs.pickSaveFile").with_id("p")).await;
        let out =
            serde_json::to_value(req.answer(&s, vec![file("broken.bin", &broken, true)])).unwrap();
        let broken_path = out["result"]["entries"][0]["path"]
            .as_str()
            .unwrap()
            .to_string();
        let r = call(&mut s, write_msg(&full_path, &b64(b"x"), Value::Null)).await;
        assert_eq!(r["error"], "quota-exceeded");
        let r = call(&mut s, write_msg(&broken_path, &b64(b"x"), Value::Null)).await;
        assert_eq!(r["error"], "io-error");
    }

    /// The same flow over a real file, the way Android hands one over.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_real_file_reads_and_writes_by_offset() {
        let dir = std::env::temp_dir().join(format!("nap-fs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("real.txt");
        std::fs::write(&p, b"0123456789").unwrap();
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&p)
            .unwrap();
        let mut s = session(&["fs"]);
        let req = pick_request(
            &mut s,
            Envelope::new("fs.pickFile")
                .with_id("p")
                .with_field("options", json!({"permissions": ["read", "write"]})),
        )
        .await;
        let out = serde_json::to_value(req.answer(
            &s,
            vec![PickedFile {
                name: "real.txt".into(),
                mime: None,
                readable: true,
                writable: true,
                bytes: Arc::new(handle),
            }],
        ))
        .unwrap();
        let path = out["result"]["entries"][0]["path"]
            .as_str()
            .unwrap()
            .to_string();
        let r = call(&mut s, read_msg(&path, json!({"offset": 3, "length": 4}))).await;
        assert_eq!(unb64(&r["result"]["data"]), b"3456");
        let r = call(
            &mut s,
            write_msg(&path, &b64(b"ab"), json!({"mode": "patch", "offset": 8})),
        )
        .await;
        assert_eq!(r["result"]["size"], 10);
        let r = call(
            &mut s,
            write_msg(&path, &b64(b"!"), json!({"mode": "append"})),
        )
        .await;
        assert_eq!(r["result"]["size"], 11);
        drop(s);
        assert_eq!(std::fs::read(&p).unwrap(), b"01234567ab!");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
