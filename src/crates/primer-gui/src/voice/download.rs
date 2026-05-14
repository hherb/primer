//! Streaming voice-asset download with timeout, resume, and size cap.
//!
//! Each asset is fetched via `reqwest::Client` and streamed to a
//! `<dest>.partial` temp path, then atomically renamed on success.
//! Progress events fire per chunk so the consent modal can render a
//! progress bar.
//!
//! Hardening (issue #92):
//! - **Timeout.** The Tauri wrapper builds a `reqwest::Client` with the
//!   overall request timeout from `SpeechSettings.download_timeout_secs`
//!   (default 30 min). A stalled TCP connection (NAT idle-timeout,
//!   captive-portal limbo) no longer leaves the consent modal spinning
//!   forever — the request aborts and a final progress event with
//!   `error: "timeout"` is emitted.
//! - **Resume.** When `<dest>.partial` already has bytes on disk, the
//!   client sends a `Range: bytes=N-` header. On a `206 Partial Content`
//!   response, new bytes are appended. On a `200 OK` (the server
//!   ignored the Range header) the partial is truncated and the
//!   download restarts — better than appending tail bytes to a stale
//!   prefix. On `416 Range Not Satisfiable` the partial is cleaned up
//!   so the next attempt restarts from a blank slate.
//! - **Max-size cap.** The cap is `approx_size_mb × 150 % × MiB`
//!   (the multiplier covers rounding headroom). A redirected URL that
//!   serves more than the cap is aborted; the partial is deleted to
//!   avoid resuming into hostile content on the next click. Assets
//!   with `approx_size_mb == None` are not capped (the user supplied
//!   the URL via Settings, so we trust the explicit choice).
//!
//! The Tauri wrapper [`download_one`] is a thin shell over
//! [`stream_to_path`], the protocol-level async core that the tests
//! drive directly against an in-process `tokio::net::TcpListener`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::commands::voice::MissingAsset;
use primer_core::consts::speech::{
    BYTES_PER_MIB, DOWNLOAD_SIZE_SAFETY_MULTIPLIER_PCT, PERCENT_DIVISOR,
};

/// Progress event delivered to the frontend over `primer://voice/download_progress`.
///
/// `error` is set only on the *final* event emitted from a failed
/// download — it carries a short failure kind so the consent modal can
/// render a specific message. Successful and in-flight events omit the
/// field entirely (`#[serde(skip_serializing_if = "Option::is_none")]`),
/// preserving the pre-issue-#92 JSON shape for the happy path.
#[derive(Serialize, Clone, Debug)]
pub struct DownloadProgressEvent {
    pub asset_id: String,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Structured download failure. The Tauri wrapper translates this into
/// a stringified `Result<(), String>` for the IPC return value AND emits
/// the kind tag on the final `download_progress` event.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("no download URL is known for asset kind {0}")]
    NoUrl(String),
    #[error("download timed out")]
    Timeout,
    #[error("upstream returned HTTP {0}")]
    HttpStatus(u16),
    #[error(
        "download exceeded size cap of {cap} bytes (received {received}); the upstream URL may have been redirected to a different resource"
    )]
    Oversize { received: u64, cap: u64 },
    #[error("network error: {0}")]
    Network(String),
    #[error("I/O error: {0}")]
    Io(String),
}

impl DownloadError {
    /// Short tag for the frontend `error` field on a failure event. Stable
    /// across versions because the consent modal switches on the value.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NoUrl(_) => "no_url",
            Self::Timeout => "timeout",
            Self::HttpStatus(_) => "http_status",
            Self::Oversize { .. } => "oversize",
            Self::Network(_) => "network",
            Self::Io(_) => "io",
        }
    }
}

// ── Pure helpers ─────────────────────────────────────────────────────

/// Build the `.partial` sibling path by *appending* `.partial` to the
/// dest filename, not by replacing the trailing extension.
/// `Path::with_extension` chops the last extension, so `foo.onnx.json`
/// would become `foo.onnx.partial` — confusing, and a latent bug for any
/// future multi-extension asset (e.g. `.tar.gz`). Appending preserves the
/// full asset name in the partial filename so concurrent downloads of
/// `foo.onnx` and `foo.onnx.json` cannot collide regardless of their
/// extensions.
pub fn partial_path_for(dest: &Path) -> PathBuf {
    let mut p = dest.as_os_str().to_owned();
    p.push(".partial");
    PathBuf::from(p)
}

/// Compute the maximum allowed download size in bytes from an
/// approximate MiB estimate. Returns `None` when the estimate is
/// `None` — the cap is then not enforced, deferring to the user's
/// explicit choice of override URL.
pub fn compute_max_bytes(approx_size_mb: Option<u32>) -> Option<u64> {
    approx_size_mb.map(|mb| {
        u64::from(mb)
            .saturating_mul(BYTES_PER_MIB)
            .saturating_mul(DOWNLOAD_SIZE_SAFETY_MULTIPLIER_PCT)
            / PERCENT_DIVISOR
    })
}

/// `Range:` header value for resuming from an existing partial byte offset.
pub fn range_header_value(offset: u64) -> String {
    format!("bytes={offset}-")
}

/// Parse the `total` field out of a `Content-Range: bytes start-end/total`
/// header. Returns `None` for `*` (unknown) or malformed input.
pub fn parse_content_range_total(value: &str) -> Option<u64> {
    let after_slash = value.split('/').nth(1)?.trim();
    if after_slash == "*" {
        return None;
    }
    after_slash.parse::<u64>().ok()
}

// ── Async core ───────────────────────────────────────────────────────

/// Streaming download into `dest`. Returns `Ok(())` on full success
/// after atomically renaming `<dest>.partial` → `<dest>`. Returns
/// `Err(DownloadError)` on the first failure.
///
/// The reqwest client is owned by the caller so tests can use a
/// non-timeout client while production wires a `.timeout(...)` builder.
///
/// Partial-file policy on failure:
/// - **Oversize / 416 Range-Not-Satisfiable** → the partial is deleted.
///   For oversize we don't want a redirected attacker payload to
///   persist between attempts; for 416 the partial is by definition
///   stale (its byte count exceeds the server's view of the total).
/// - **Every other failure** (Timeout, Network drop mid-stream,
///   transient HttpStatus, I/O) → the partial is preserved so the next
///   attempt resumes from the byte offset we got to. The previous
///   behaviour of always cleaning the partial defeated the entire
///   resume feature.
pub async fn stream_to_path<F>(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    max_bytes: Option<u64>,
    mut on_progress: F,
) -> Result<(), DownloadError>
where
    F: FnMut(u64, Option<u64>),
{
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;

    let partial = partial_path_for(dest);
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| DownloadError::Io(format!("mkdir {}: {e}", parent.display())))?;
    }

    let partial_size: u64 = match tokio::fs::metadata(&partial).await {
        Ok(meta) => meta.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => {
            return Err(DownloadError::Io(format!(
                "stat {}: {e}",
                partial.display()
            )));
        }
    };
    let resume = partial_size > 0;

    let mut req = client.get(url);
    if resume {
        req = req.header(reqwest::header::RANGE, range_header_value(partial_size));
    }
    let resp = req.send().await.map_err(map_reqwest_err)?;
    let status = resp.status();

    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        // Partial is stale per the server's current view of the file.
        // Drop it so the next attempt restarts from a blank slate.
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(DownloadError::HttpStatus(status.as_u16()));
    }
    if !status.is_success() {
        return Err(DownloadError::HttpStatus(status.as_u16()));
    }

    // Compute the visible total: prefer the Content-Range total (set by
    // a server honouring our Range request), fall back to Content-Length
    // adjusted for the resume offset on 206, raw Content-Length on 200.
    let bytes_total = resolve_bytes_total(&resp, partial_size);
    let appending = resume && status == reqwest::StatusCode::PARTIAL_CONTENT;

    if !appending && partial_size > 0 {
        // Server ignored the Range header (responded 200 to our 206
        // request) — the existing partial bytes are stale; drop them
        // before opening with truncate.
        if let Err(e) = tokio::fs::remove_file(&partial).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(DownloadError::Io(format!(
                    "rm stale partial {}: {e}",
                    partial.display()
                )));
            }
        }
    }

    let mut opts = tokio::fs::OpenOptions::new();
    opts.create(true);
    if appending {
        opts.append(true);
    } else {
        opts.write(true).truncate(true);
    }
    let mut file = opts
        .open(&partial)
        .await
        .map_err(|e| DownloadError::Io(format!("open {}: {e}", partial.display())))?;

    let mut bytes_done: u64 = if appending { partial_size } else { 0 };
    let stream_result: Result<(), DownloadError> = async {
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(map_reqwest_err)?;
            bytes_done = bytes_done.saturating_add(chunk.len() as u64);
            if let Some(cap) = max_bytes {
                if bytes_done > cap {
                    return Err(DownloadError::Oversize {
                        received: bytes_done,
                        cap,
                    });
                }
            }
            file.write_all(&chunk)
                .await
                .map_err(|e| DownloadError::Io(format!("write: {e}")))?;
            on_progress(bytes_done, bytes_total);
        }
        file.flush()
            .await
            .map_err(|e| DownloadError::Io(format!("flush: {e}")))?;
        Ok(())
    }
    .await;

    drop(file);

    match stream_result {
        Ok(()) => {
            tokio::fs::rename(&partial, dest).await.map_err(|e| {
                DownloadError::Io(format!(
                    "rename {} -> {}: {e}",
                    partial.display(),
                    dest.display()
                ))
            })?;
            Ok(())
        }
        Err(e) => {
            if matches!(e, DownloadError::Oversize { .. }) {
                if let Err(rm_err) = tokio::fs::remove_file(&partial).await {
                    if rm_err.kind() != std::io::ErrorKind::NotFound {
                        tracing::warn!(
                            "download failed ({e}); also failed to clean up tainted partial {}: {rm_err}",
                            partial.display(),
                        );
                    }
                }
            }
            Err(e)
        }
    }
}

fn map_reqwest_err(e: reqwest::Error) -> DownloadError {
    if e.is_timeout() {
        DownloadError::Timeout
    } else {
        DownloadError::Network(e.to_string())
    }
}

fn resolve_bytes_total(resp: &reqwest::Response, offset: u64) -> Option<u64> {
    if let Some(cr) = resp.headers().get(reqwest::header::CONTENT_RANGE) {
        if let Ok(s) = cr.to_str() {
            if let Some(total) = parse_content_range_total(s) {
                return Some(total);
            }
        }
    }
    let cl = resp.content_length()?;
    if resp.status() == reqwest::StatusCode::PARTIAL_CONTENT {
        Some(cl.saturating_add(offset))
    } else {
        Some(cl)
    }
}

// ── Tauri wrapper ────────────────────────────────────────────────────

/// Download one [`MissingAsset`] to its target path.
///
/// Builds a `reqwest::Client` carrying the configured timeout, computes
/// the max-byte cap from the asset's `approx_size_mb`, and delegates to
/// [`stream_to_path`]. On success, the asset is at `asset.path`. On
/// failure, emits one final `primer://voice/download_progress` event with
/// `error: Some(<kind>)` so the consent modal renders a specific message.
pub async fn download_one<R: tauri::Runtime>(
    app: &AppHandle<R>,
    asset: &MissingAsset,
    timeout_secs: u64,
) -> Result<(), String> {
    let url = match asset.suggested_url.as_deref() {
        Some(u) => u,
        None => {
            let e = DownloadError::NoUrl(asset.kind.clone());
            emit_failure(app, &asset.kind, 0, None, &e);
            return Err(e.to_string());
        }
    };

    let client = match build_client(timeout_secs) {
        Ok(c) => c,
        Err(e) => {
            let de = DownloadError::Network(e.to_string());
            emit_failure(app, &asset.kind, 0, None, &de);
            return Err(de.to_string());
        }
    };

    let max_bytes = compute_max_bytes(asset.approx_size_mb);

    let asset_id = asset.kind.clone();
    let app_for_progress = app.clone();
    let on_progress = move |bytes_done: u64, bytes_total: Option<u64>| {
        let evt = DownloadProgressEvent {
            asset_id: asset_id.clone(),
            bytes_done,
            bytes_total,
            error: None,
        };
        let _ = app_for_progress.emit("primer://voice/download_progress", &evt);
    };

    let result = stream_to_path(&client, url, &asset.path, max_bytes, on_progress).await;
    if let Err(ref e) = result {
        emit_failure(app, &asset.kind, 0, None, e);
    }
    result.map_err(|e| e.to_string())
}

fn build_client(timeout_secs: u64) -> Result<reqwest::Client, reqwest::Error> {
    let mut b = reqwest::Client::builder();
    if timeout_secs > 0 {
        b = b.timeout(Duration::from_secs(timeout_secs));
    }
    b.build()
}

fn emit_failure<R: tauri::Runtime>(
    app: &AppHandle<R>,
    asset_id: &str,
    bytes_done: u64,
    bytes_total: Option<u64>,
    err: &DownloadError,
) {
    let evt = DownloadProgressEvent {
        asset_id: asset_id.to_string(),
        bytes_done,
        bytes_total,
        error: Some(err.kind().to_string()),
    };
    let _ = app.emit("primer://voice/download_progress", &evt);
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Pure-helper unit tests ───────────────────────────────────────

    #[test]
    fn download_progress_event_serialises_with_snake_case_fields() {
        let evt = DownloadProgressEvent {
            asset_id: "whisper_model".into(),
            bytes_done: 12_345_678,
            bytes_total: Some(490_000_000),
            error: None,
        };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["asset_id"], "whisper_model");
        assert_eq!(json["bytes_done"], 12_345_678);
        assert_eq!(json["bytes_total"], 490_000_000);
        assert!(
            json.get("error").is_none(),
            "error field must be omitted on success / progress: {json}"
        );
    }

    #[test]
    fn download_progress_event_omits_unknown_total_as_null() {
        let evt = DownloadProgressEvent {
            asset_id: "piper_onnx".into(),
            bytes_done: 1,
            bytes_total: None,
            error: None,
        };
        let json = serde_json::to_value(&evt).unwrap();
        assert!(
            json["bytes_total"].is_null(),
            "missing content-length must serialise as null, not be skipped: {json}"
        );
    }

    #[test]
    fn download_progress_event_carries_error_kind_on_failure() {
        let evt = DownloadProgressEvent {
            asset_id: "whisper_model".into(),
            bytes_done: 42,
            bytes_total: Some(100),
            error: Some("timeout".into()),
        };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(
            json["error"], "timeout",
            "error kind must reach the frontend so the consent modal can branch on it: {json}"
        );
    }

    #[test]
    fn partial_path_for_appends_partial_to_full_filename() {
        let dest = Path::new("/cache/voice/en/en_GB-alba-medium.onnx");
        let p = partial_path_for(dest);
        assert_eq!(
            p,
            PathBuf::from("/cache/voice/en/en_GB-alba-medium.onnx.partial"),
        );
    }

    /// Multi-extension assets (e.g. `.onnx.json`) must NOT have the inner
    /// extension chopped by `Path::with_extension`. The append-style
    /// helper preserves the full filename so concurrent downloads of
    /// `foo.onnx` and `foo.onnx.json` cannot collide on the same partial.
    #[test]
    fn partial_path_for_preserves_multi_extension_filenames() {
        let dest = Path::new("/cache/voice/en/en_GB-alba-medium.onnx.json");
        let p = partial_path_for(dest);
        assert_eq!(
            p,
            PathBuf::from("/cache/voice/en/en_GB-alba-medium.onnx.json.partial"),
        );
    }

    #[test]
    fn compute_max_bytes_applies_safety_multiplier() {
        // 100 MiB × 150 % = 150 MiB = 157_286_400 bytes.
        assert_eq!(compute_max_bytes(Some(100)), Some(157_286_400));
    }

    #[test]
    fn compute_max_bytes_none_means_no_cap() {
        assert_eq!(compute_max_bytes(None), None);
    }

    #[test]
    fn compute_max_bytes_zero_estimate_yields_zero_cap() {
        assert_eq!(
            compute_max_bytes(Some(0)),
            Some(0),
            "degenerate zero-MB estimate produces zero cap; not normally reachable but defined",
        );
    }

    #[test]
    fn range_header_uses_open_ended_bytes_form() {
        assert_eq!(range_header_value(0), "bytes=0-");
        assert_eq!(range_header_value(12_345), "bytes=12345-");
        assert_eq!(range_header_value(u64::MAX), format!("bytes={}-", u64::MAX));
    }

    #[test]
    fn parse_content_range_total_handles_known_total() {
        assert_eq!(parse_content_range_total("bytes 50-99/100"), Some(100));
        assert_eq!(parse_content_range_total("bytes 0-999/12345"), Some(12345));
    }

    #[test]
    fn parse_content_range_total_returns_none_for_unknown() {
        assert_eq!(parse_content_range_total("bytes 50-99/*"), None);
        assert_eq!(parse_content_range_total("malformed"), None);
        assert_eq!(parse_content_range_total(""), None);
    }

    #[test]
    fn download_error_kind_strings_are_stable_for_frontend() {
        assert_eq!(DownloadError::NoUrl("x".into()).kind(), "no_url");
        assert_eq!(DownloadError::Timeout.kind(), "timeout");
        assert_eq!(DownloadError::HttpStatus(404).kind(), "http_status");
        assert_eq!(
            DownloadError::Oversize {
                received: 1,
                cap: 0
            }
            .kind(),
            "oversize"
        );
        assert_eq!(DownloadError::Network("x".into()).kind(), "network");
        assert_eq!(DownloadError::Io("x".into()).kind(), "io");
    }

    // ── HTTP integration tests (in-process TcpListener server) ───────
    //
    // No external dev-dep: we drive a tiny one-shot HTTP/1.1 server
    // off `tokio::net::TcpListener`. Each test spawns the server on a
    // random port, runs `stream_to_path` against it, and asserts on
    // the resulting on-disk state.

    use std::net::SocketAddr;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt as _};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    /// Spawn a one-shot HTTP server that accepts a single connection,
    /// hands the read request to `handler`, writes the returned bytes,
    /// then drops the socket. Returns the bound address and a handle
    /// that resolves to the request line+headers the server saw.
    async fn spawn_one_shot<F>(handler: F) -> (SocketAddr, Arc<Mutex<Option<String>>>)
    where
        F: FnOnce(String) -> Vec<u8> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let captured_clone = captured.clone();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let mut request = String::new();
            // Read until end-of-headers (CRLFCRLF) or socket close.
            loop {
                let n = match sock.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                request.push_str(&String::from_utf8_lossy(&buf[..n]));
                if request.contains("\r\n\r\n") {
                    break;
                }
            }
            *captured_clone.lock().await = Some(request.clone());
            let response = handler(request);
            let _ = sock.write_all(&response).await;
            let _ = sock.shutdown().await;
        });
        (addr, captured)
    }

    /// Spawn a server that accepts a connection but never replies — the
    /// client must hit its timeout.
    async fn spawn_stalled_server() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            // Hold the connection open by parking the future.
            // Drop only when the test task drops the listener.
            let _hold = sock;
            std::future::pending::<()>().await;
        });
        addr
    }

    fn raw_response_200(body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    fn raw_response_206(body: &[u8], start: u64, total: u64) -> Vec<u8> {
        let end = start + body.len() as u64 - 1;
        let mut out = format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\n\r\n",
            body.len(),
            start,
            end,
            total,
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    fn raw_response_status(status: u16, reason: &str) -> Vec<u8> {
        format!("HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\n\r\n").into_bytes()
    }

    #[tokio::test]
    async fn stream_to_path_writes_full_body_on_200() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("model.bin");
        let body = vec![0xABu8; 256];
        let body_for_handler = body.clone();
        let (addr, captured) =
            spawn_one_shot(move |_req| raw_response_200(&body_for_handler)).await;

        let client = reqwest::Client::new();
        let url = format!("http://{addr}/asset.bin");
        let mut events: Vec<(u64, Option<u64>)> = Vec::new();
        stream_to_path(&client, &url, &dest, None, |b, t| events.push((b, t)))
            .await
            .expect("stream succeeds");

        let on_disk = tokio::fs::read(&dest).await.unwrap();
        assert_eq!(on_disk, body);
        assert!(
            !partial_path_for(&dest).exists(),
            "partial must be renamed away on success"
        );
        let req = captured.lock().await.clone().unwrap();
        assert!(
            !req.contains("Range:"),
            "no resume → no Range header: {req}"
        );
        assert!(!events.is_empty(), "at least one progress event fires");
        assert_eq!(events.last().unwrap().0, body.len() as u64);
        assert_eq!(events.last().unwrap().1, Some(body.len() as u64));
    }

    #[tokio::test]
    async fn stream_to_path_resumes_from_partial_with_range_header_and_206() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("model.bin");
        let partial = partial_path_for(&dest);
        // Pre-create 50 bytes of "A".
        tokio::fs::write(&partial, vec![b'A'; 50]).await.unwrap();
        // Server returns 50 bytes of "B" with Content-Range 50-99/100.
        let tail = vec![b'B'; 50];
        let tail_for_handler = tail.clone();
        let (addr, captured) = spawn_one_shot(move |req| {
            assert!(
                req.to_lowercase().contains("range: bytes=50-"),
                "client must send Range header on resume; got: {req}"
            );
            raw_response_206(&tail_for_handler, 50, 100)
        })
        .await;

        let client = reqwest::Client::new();
        let url = format!("http://{addr}/asset.bin");
        stream_to_path(&client, &url, &dest, None, |_, _| {})
            .await
            .expect("resume succeeds");

        let on_disk = tokio::fs::read(&dest).await.unwrap();
        assert_eq!(on_disk.len(), 100);
        assert_eq!(&on_disk[..50], &[b'A'; 50][..]);
        assert_eq!(&on_disk[50..], &[b'B'; 50][..]);
        drop(captured);
    }

    #[tokio::test]
    async fn stream_to_path_overwrites_when_server_ignores_range_and_returns_200() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("model.bin");
        let partial = partial_path_for(&dest);
        // Pre-create 50 bytes of "A" — stale from a previous attempt.
        tokio::fs::write(&partial, vec![b'A'; 50]).await.unwrap();
        // Server ignores the Range header and replies with full 100 bytes.
        let body = vec![b'Z'; 100];
        let body_for_handler = body.clone();
        let (addr, _captured) =
            spawn_one_shot(move |_req| raw_response_200(&body_for_handler)).await;

        let client = reqwest::Client::new();
        let url = format!("http://{addr}/asset.bin");
        stream_to_path(&client, &url, &dest, None, |_, _| {})
            .await
            .expect("overwrite succeeds");

        let on_disk = tokio::fs::read(&dest).await.unwrap();
        assert_eq!(
            on_disk,
            vec![b'Z'; 100],
            "stale partial must be replaced, NOT prepended to the server body",
        );
    }

    #[tokio::test]
    async fn stream_to_path_aborts_on_oversize_and_cleans_partial() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("model.bin");
        // Server returns 2000 bytes; cap is 1000.
        let body = vec![0x55u8; 2000];
        let body_for_handler = body.clone();
        let (addr, _captured) =
            spawn_one_shot(move |_req| raw_response_200(&body_for_handler)).await;

        let client = reqwest::Client::new();
        let url = format!("http://{addr}/asset.bin");
        let err = stream_to_path(&client, &url, &dest, Some(1000), |_, _| {})
            .await
            .expect_err("must abort on oversize");
        match err {
            DownloadError::Oversize { received, cap } => {
                assert!(received > cap);
                assert_eq!(cap, 1000);
            }
            other => panic!("expected Oversize, got {other:?}"),
        }
        assert!(
            !dest.exists(),
            "dest must not be created when oversize aborts"
        );
        assert!(
            !partial_path_for(&dest).exists(),
            "partial must be cleaned up to avoid resuming into hostile content"
        );
    }

    #[tokio::test]
    async fn stream_to_path_preserves_partial_on_timeout() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("model.bin");
        // Pre-create 50 bytes — should survive a timeout failure.
        let partial = partial_path_for(&dest);
        tokio::fs::write(&partial, vec![b'X'; 50]).await.unwrap();

        let addr = spawn_stalled_server().await;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(150))
            .build()
            .unwrap();
        let url = format!("http://{addr}/asset.bin");
        let err = stream_to_path(&client, &url, &dest, None, |_, _| {})
            .await
            .expect_err("must time out");
        assert!(
            matches!(err, DownloadError::Timeout),
            "must classify reqwest timeout as Timeout, got {err:?}",
        );
        assert!(
            partial.exists(),
            "partial must survive timeout so next attempt resumes"
        );
        let preserved = tokio::fs::read(&partial).await.unwrap();
        assert_eq!(preserved, vec![b'X'; 50]);
    }

    #[tokio::test]
    async fn stream_to_path_maps_404_to_http_status_error() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("model.bin");
        let (addr, _captured) = spawn_one_shot(|_req| raw_response_status(404, "Not Found")).await;

        let client = reqwest::Client::new();
        let url = format!("http://{addr}/asset.bin");
        let err = stream_to_path(&client, &url, &dest, None, |_, _| {})
            .await
            .expect_err("4xx must surface as HttpStatus");
        match err {
            DownloadError::HttpStatus(code) => assert_eq!(code, 404),
            other => panic!("expected HttpStatus(404), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stream_to_path_cleans_partial_on_416_range_not_satisfiable() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("model.bin");
        // Pre-create 200 bytes; server says total is now 100 (file shrunk).
        let partial = partial_path_for(&dest);
        tokio::fs::write(&partial, vec![b'Q'; 200]).await.unwrap();
        let (addr, _captured) = spawn_one_shot(|req| {
            assert!(req.to_lowercase().contains("range: bytes=200-"));
            raw_response_status(416, "Range Not Satisfiable")
        })
        .await;

        let client = reqwest::Client::new();
        let url = format!("http://{addr}/asset.bin");
        let err = stream_to_path(&client, &url, &dest, None, |_, _| {})
            .await
            .expect_err("416 must surface");
        match err {
            DownloadError::HttpStatus(416) => {}
            other => panic!("expected HttpStatus(416), got {other:?}"),
        }
        assert!(
            !partial.exists(),
            "stale partial must be cleaned up on 416 so the next attempt restarts fresh"
        );
    }
}
