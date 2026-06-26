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
                tracing::warn!(
                    "failed to remove stale partial {} before restart: {e}",
                    partial.display(),
                );
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
                // Cumulative cap: `bytes_done` starts at `partial_size`
                // on resume, so this bounds the total on-disk footprint
                // across attempts, not per-attempt. A redirected URL
                // serving a >cap payload aborts even if some of the
                // bytes arrived on a prior run.
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
    // Track the most recent (bytes_done, bytes_total) so the final
    // failure event carries the progress we actually reached, not (0, None).
    // The frontend's last in-flight event already has the same numbers, but
    // a consumer that only reads the failure event (e.g. a future log
    // forwarder, or a UI that suppresses progress while a modal is dismissed)
    // needs the offset to render "stopped at X of Y" copy.
    let mut last_progress: (u64, Option<u64>) = (0, None);
    let on_progress = |bytes_done: u64, bytes_total: Option<u64>| {
        last_progress = (bytes_done, bytes_total);
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
        emit_failure(app, &asset.kind, last_progress.0, last_progress.1, e);
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

/// Pick the `bytes_done` to report on a failure event.
///
/// For [`DownloadError::Oversize`] the variant's own `received` field
/// is the offset that triggered the abort — one chunk past the most
/// recent `on_progress` call (the cap check fires before the per-chunk
/// progress callback). For every other failure kind, the last observed
/// progress offset is the right value: it reflects whatever the stream
/// loop wrote to disk before the failure.
fn reported_bytes_done_on_failure(last_progress: u64, err: &DownloadError) -> u64 {
    match err {
        DownloadError::Oversize { received, .. } => *received,
        _ => last_progress,
    }
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
        bytes_done: reported_bytes_done_on_failure(bytes_done, err),
        bytes_total,
        error: Some(err.kind().to_string()),
    };
    let _ = app.emit("primer://voice/download_progress", &evt);
}

#[cfg(test)]
mod tests;
