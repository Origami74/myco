//! NAP-FS on this device: the host's half of a pick.
//!
//! The runtime validates `fs.pickFile` / `fs.pickFiles` / `fs.pickSaveFile`
//! and hands back a [`PickRequest`]; only the window host can show the
//! system picker, so it goes there as a command, and the answer comes back
//! through `nappletFsPicked`:
//!
//! ```text
//! napplet  fs.pickFile ──► runtime ──► PickRequest ──► pick-file {token, mode, mimeTypes…} ──► window host
//! window host  system picker ──► ContentResolver.openFileDescriptor ──► nappletFsPicked(token, fds, meta)
//! host  PickRequest::answer ──► entries in the session's store ──► fs.pickFile.result (pushed)
//! ```
//!
//! The platform hands over **open file descriptors**, one per picked file,
//! opened by the document provider for the chosen URIs — never a path. The
//! runtime reads and writes them by offset ([`PickedBytes`] for
//! [`std::fs::File`]); they close when the window's session goes.
//!
//! One pick at a time per window: a second while the picker is up is
//! `policy-denied` at once. A token names the pending pick and nothing else;
//! an answer for a token that is not this window's current one is dropped,
//! and so are its descriptors.
//!
//! [`PickedBytes`]: myco_napplet_runtime::PickedBytes

use std::sync::{Arc, Mutex};

use myco_napplet_runtime::nap::fs::{PickRequest, CANCELLED, IO_ERROR, POLICY_DENIED};
use myco_napplet_runtime::shell_link::ToShell;
use myco_napplet_runtime::PickedFile;

use crate::napplet::NappletHost;

/// A pick the window host has been asked to show.
pub(crate) struct PendingPick {
    token: String,
    request: PickRequest,
}

/// One window's pending pick, if any. Lives in the window's `LiveNapplet`,
/// so closing the window forgets it.
pub(crate) type PickSlot = Arc<Mutex<Option<PendingPick>>>;

/// What the platform picker came back with.
#[derive(Debug)]
pub enum PickAnswer {
    /// The files the user chose, opened.
    Picked(Vec<PickedFile>),
    /// The user backed out.
    Cancelled,
    /// The window host could not show the picker, or open what was chosen:
    /// one of the spec's `FsError` strings.
    Failed(String),
}

impl NappletHost {
    /// Hand a validated pick to `session_id`'s window host, or refuse it when
    /// that window already has one up.
    pub(crate) fn fs_pick(&self, session_id: &str, request: PickRequest) -> Vec<ToShell> {
        let slot = {
            let sessions = self.sessions.lock().unwrap();
            match sessions.get(session_id) {
                Some(live) => live.pick.clone(),
                None => return Vec::new(),
            }
        };
        let mut slot = slot.lock().unwrap();
        if slot.is_some() {
            tracing::info!(
                session = session_id,
                "napplet pick refused: one is already up"
            );
            return vec![ToShell::to_napplet(request.failed(POLICY_DENIED))];
        }
        let token = crate::napplet::new_blob_token();
        let command = ToShell::PickFile {
            token: token.clone(),
            mode: request.mode.as_str().to_string(),
            mime_types: request.mime_types.clone(),
            suggested_name: request.suggested_name.clone().unwrap_or_default(),
            save_mime: request.save_mime.clone(),
            read: request.open_for_read,
            write: request.open_for_write,
        };
        tracing::info!(
            session = session_id,
            mode = request.mode.as_str(),
            "napplet pick"
        );
        *slot = Some(PendingPick { token, request });
        vec![command]
    }

    /// The window host's answer to the pick `token`: register what was
    /// picked in the window's session and push the result to the napplet.
    /// Returns whether it answered a pending pick. Anything else — a closed
    /// window, a stale or foreign token — drops the answer, and with it the
    /// files' descriptors.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub async fn fs_picked(&self, session_id: &str, token: &str, answer: PickAnswer) -> bool {
        let (session, outbox, slot) = {
            let sessions = self.sessions.lock().unwrap();
            match sessions.get(session_id) {
                Some(live) => (live.session.clone(), live.outbox.clone(), live.pick.clone()),
                None => return false,
            }
        };
        let pending = {
            let mut slot = slot.lock().unwrap();
            match slot.as_ref() {
                Some(p) if p.token == token => slot.take(),
                _ => None,
            }
        };
        let Some(pending) = pending else {
            tracing::info!(
                session = session_id,
                "napplet pick answer for no pending pick; dropped"
            );
            return false;
        };
        let reply = match answer {
            PickAnswer::Picked(files) => {
                let session = session.lock().await;
                pending.request.answer(&session, files)
            }
            PickAnswer::Cancelled => pending.request.cancelled(),
            PickAnswer::Failed(error) => {
                // Only the spec's strings reach the napplet.
                let error = match error.as_str() {
                    POLICY_DENIED | CANCELLED => error.as_str(),
                    _ => IO_ERROR,
                };
                pending.request.failed(error)
            }
        };
        let _ = outbox.send(ToShell::to_napplet(reply));
        true
    }
}

/// The platform's answer, as the window host sends it: `files` the opened
/// descriptors — **owned from here on** — and `meta_json` what it knows of
/// each, in the same order:
///
/// ```json
/// {"status": "picked", "files": [{"name": "a.txt", "mime": "text/plain", "readable": true, "writable": false}]}
/// {"status": "cancelled"}
/// {"status": "failed", "error": "io-error"}
/// ```
///
/// A descriptor without matching metadata, or any at all on a cancel or a
/// malformed answer, is closed.
#[cfg(unix)]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn pick_answer(files: Vec<std::fs::File>, meta_json: &str) -> PickAnswer {
    let meta: serde_json::Value = match serde_json::from_str(meta_json) {
        Ok(v) => v,
        Err(_) => return PickAnswer::Failed(IO_ERROR.to_string()),
    };
    match meta.get("status").and_then(|s| s.as_str()) {
        Some("picked") => {}
        Some("cancelled") => return PickAnswer::Cancelled,
        Some("failed") => {
            let error = meta
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or(IO_ERROR);
            return PickAnswer::Failed(error.to_string());
        }
        _ => return PickAnswer::Failed(IO_ERROR.to_string()),
    }
    let described = meta
        .get("files")
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default();
    let picked = files
        .into_iter()
        .zip(described)
        .map(|(file, m)| PickedFile {
            name: m
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("file")
                .to_string(),
            mime: m
                .get("mime")
                .and_then(|n| n.as_str())
                .filter(|m| !m.is_empty())
                .map(str::to_string),
            readable: m.get("readable").and_then(|b| b.as_bool()).unwrap_or(false),
            writable: m.get("writable").and_then(|b| b.as_bool()).unwrap_or(false),
            bytes: Arc::new(file),
        })
        .collect();
    PickAnswer::Picked(picked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myco_napplet_runtime::testing::NappletBuilder;
    use nsite_deck::seams::{BlobStore, RelayBackend};
    use nsite_deck::testing::{MemBlobs, MemRelay};
    use serde_json::{json, Value};
    use std::time::Duration;

    use crate::napplet::NappletAddr;

    async fn host() -> (NappletHost, NappletAddr) {
        let napplet = NappletBuilder::new().build();
        let relay = Arc::new(MemRelay::new());
        let blobs = Arc::new(MemBlobs::new());
        for (_, bytes) in &napplet.blobs {
            blobs.put(bytes).await.unwrap();
        }
        relay.publish(napplet.manifest.clone()).await.unwrap();
        let (mut ctx, _signer) = myco_napplet_runtime::testing::test_context();
        ctx.relay = relay;
        ctx.blobs = blobs.clone();
        ctx.kept_blobs = blobs;
        let addr = NappletAddr {
            author: napplet.author,
            d_tag: Some("fixture".to_string()),
            relays: Vec::new(),
        };
        (NappletHost::new(ctx), addr)
    }

    async fn open(host: &NappletHost, addr: &NappletAddr, granted: &[&str]) -> String {
        let opened = host
            .open(addr, Some(granted.iter().map(|g| g.to_string()).collect()))
            .await
            .unwrap();
        host.frame(
            &opened.session_id,
            r#"{"channel":"napplet","message":{"type":"shell.ready"}}"#,
        )
        .await;
        opened.session_id
    }

    async fn send(host: &NappletHost, session: &str, message: Value) -> Vec<ToShell> {
        let frame = json!({"channel": "napplet", "message": message}).to_string();
        host.frame(session, &frame).await
    }

    fn message(frame: &ToShell) -> Value {
        match frame {
            ToShell::Napplet { message } => serde_json::to_value(message).unwrap(),
            other => panic!("expected a napplet frame, got {other:?}"),
        }
    }

    fn temp_file(name: &str, bytes: &[u8]) -> (std::path::PathBuf, std::fs::File) {
        let dir = std::env::temp_dir().join(format!("myco-fs-pick-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        (path, file)
    }

    /// The whole device path: the pick becomes a window command, the window
    /// host's descriptors come back, and the napplet hears the entry and
    /// reads it.
    #[tokio::test]
    async fn a_pick_round_trips_through_the_window_host() {
        let (host, addr) = host().await;
        let session = open(&host, &addr, &["fs"]).await;
        let out = send(
            &host,
            &session,
            json!({"type": "fs.pickFile", "id": "p1", "options": {"accept": [{"extension": ".md"}]}}),
        )
        .await;
        assert_eq!(out.len(), 1);
        let ToShell::PickFile {
            token,
            mode,
            mime_types,
            read,
            write,
            ..
        } = &out[0]
        else {
            panic!("expected a pick command, got {out:?}");
        };
        assert_eq!(mode, "open");
        assert_eq!(mime_types, &["text/markdown", "text/x-markdown"]);
        assert!(*read && !*write);
        // As the window host reads it.
        let wire = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(wire["channel"], "pick-file");
        assert_eq!(
            wire["mimeTypes"],
            json!(["text/markdown", "text/x-markdown"])
        );

        // A second pick while this one is up is refused at once.
        let again = send(&host, &session, json!({"type": "fs.pickFile", "id": "p2"})).await;
        assert_eq!(message(&again[0])["error"], "policy-denied");

        let (_path, file) = temp_file("notes.md", b"# hello");
        let meta = json!({"status": "picked", "files": [{"name": "notes.md", "mime": "text/markdown", "readable": true, "writable": false}]});
        // A wrong token answers nothing.
        assert!(
            !host
                .fs_picked(&session, "nope", PickAnswer::Cancelled)
                .await
        );
        assert!(
            host.fs_picked(&session, token, pick_answer(vec![file], &meta.to_string()))
                .await
        );
        let pushed = host.next_frames(&session, Duration::from_millis(50)).await;
        let result = message(&pushed[0]);
        assert_eq!(result["type"], "fs.pickFile.result");
        assert_eq!(result["id"], "p1");
        assert_eq!(result["result"]["entries"][0]["path"], "/picked/notes.md");
        assert_eq!(result["result"]["entries"][0]["size"], 7);

        let out = send(
            &host,
            &session,
            json!({"type": "fs.read", "id": "r1", "path": "/picked/notes.md", "options": {"offset": 2}}),
        )
        .await;
        let r = message(&out[0]);
        assert_eq!(r["result"]["data"], "aGVsbG8=");
        assert_eq!(r["result"]["eof"], true);

        // The token is spent.
        assert!(!host.fs_picked(&session, token, PickAnswer::Cancelled).await);
    }

    #[tokio::test]
    async fn a_cancel_and_a_failure_reach_the_napplet() {
        let (host, addr) = host().await;
        let session = open(&host, &addr, &["fs"]).await;
        for (answer, error) in [
            (PickAnswer::Cancelled, "cancelled"),
            (PickAnswer::Failed("io-error".into()), "io-error"),
            (PickAnswer::Failed("something made up".into()), "io-error"),
        ] {
            let out = send(
                &host,
                &session,
                json!({"type": "fs.pickSaveFile", "id": "s"}),
            )
            .await;
            let ToShell::PickFile {
                token,
                mode,
                write,
                save_mime,
                ..
            } = &out[0]
            else {
                panic!("expected a pick command");
            };
            assert_eq!(mode, "save");
            assert!(*write);
            assert_eq!(save_mime, "application/octet-stream");
            assert!(host.fs_picked(&session, token, answer).await);
            let pushed = host.next_frames(&session, Duration::from_millis(50)).await;
            assert_eq!(message(&pushed[0])["error"], error);
        }
    }

    /// Closing the window forgets the pick; a late answer is dropped and its
    /// descriptors closed with it.
    #[tokio::test]
    async fn a_closed_window_drops_a_late_answer() {
        let (host, addr) = host().await;
        let session = open(&host, &addr, &["fs"]).await;
        let out = send(&host, &session, json!({"type": "fs.pickFile", "id": "p"})).await;
        let ToShell::PickFile { token, .. } = &out[0] else {
            panic!("expected a pick command");
        };
        host.close(&session);
        let (_p, file) = temp_file("late.txt", b"x");
        let meta = json!({"status": "picked", "files": [{"name": "late.txt", "readable": true}]});
        assert!(
            !host
                .fs_picked(&session, token, pick_answer(vec![file], &meta.to_string()))
                .await
        );
    }

    /// Not granted: refused by dispatch, and no picker is ever shown.
    #[tokio::test]
    async fn an_ungranted_pick_never_reaches_the_window() {
        let (host, addr) = host().await;
        let session = open(&host, &addr, &["relay"]).await;
        let out = send(&host, &session, json!({"type": "fs.pickFile", "id": "p"})).await;
        assert_eq!(out.len(), 1);
        assert_eq!(message(&out[0])["error"], "policy-denied");
    }

    #[test]
    fn the_answer_json_is_read_carefully() {
        assert!(matches!(
            pick_answer(Vec::new(), r#"{"status":"cancelled"}"#),
            PickAnswer::Cancelled
        ));
        assert!(matches!(
            pick_answer(Vec::new(), "not json"),
            PickAnswer::Failed(_)
        ));
        assert!(matches!(
            pick_answer(Vec::new(), r#"{"status":"failed","error":"policy-denied"}"#),
            PickAnswer::Failed(e) if e == "policy-denied"
        ));
        // Two descriptors, one described: one file, the other closed.
        let (_a, a) = temp_file("a.txt", b"a");
        let (_b, b) = temp_file("b.txt", b"b");
        let PickAnswer::Picked(files) = pick_answer(
            vec![a, b],
            r#"{"status":"picked","files":[{"name":"a.txt","mime":"","readable":true,"writable":true}]}"#,
        ) else {
            panic!("expected files");
        };
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "a.txt");
        assert_eq!(files[0].mime, None);
        assert!(files[0].readable && files[0].writable);
    }
}
