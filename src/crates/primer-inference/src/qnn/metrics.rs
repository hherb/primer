//! Per-turn throughput metrics for the [`QnnBackend`](super::QnnBackend).
//!
//! ## Why this exists
//!
//! On the target RedMagic production ROM the Hexagon DSP is reachable **only**
//! from the packaged Tauri-Android app — a sideloaded/Termux binary is denied
//! the FastRPC node, so the standalone `examples/qnn_bench.rs` harness cannot
//! touch the NPU there. The only place real on-device throughput numbers can
//! be measured is *inside the running APK*. This module instruments the QNN
//! backend's own `generate_stream` so every turn records its time-to-first-
//! token and steady-state decode rate to an append-only JSONL file that a
//! developer reads via `run-as cat` (the same channel as `genie.log`, since
//! `logcat` is dead on that ROM).
//!
//! ## Shape
//!
//! - [`MeteredStream`] wraps the [`TokenStream`] the backend returns, times it
//!   with a shared [`StreamTimer`], and hands the final [`StreamTiming`] to a
//!   sink closure exactly once when the stream ends. It performs no I/O itself
//!   (the closure decides), so it is host-testable.
//! - [`format_metric_line`] renders one [`StreamTiming`] as a JSONL record
//!   (pure — the timestamp is injected).
//! - [`append_metric_line`] / [`record_timing`] are the never-panic file-sink
//!   glue, mirroring the `genie::log` discipline (a metrics-write failure must
//!   never break a child's turn).
//! - The output path comes from [`QNN_METRICS_PATH_ENV`], set by the GUI
//!   startup hook on mobile; unset elsewhere ⇒ recording disabled, zero cost.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use futures::Stream;
use primer_core::consts::qnn::METRICS_FILE_MAX_BYTES;
use primer_core::error::Result;
use primer_core::inference::{TokenChunk, TokenStream};

use crate::bench::metrics::{StreamTimer, StreamTiming};

/// Suffix appended to the live metrics file name when it is rotated out.
/// Single-backup rotation: the live file becomes `<name>.1`, replacing any
/// prior backup, so the on-disk footprint stays bounded at ~2× the cap.
const ROTATED_SUFFIX: &str = ".1";

/// Env var carrying the path of the per-turn QNN metrics JSONL file.
///
/// Set by the GUI's startup hook on mobile (next to `genie.log`); unset
/// elsewhere, in which case metrics recording is disabled — no file is opened
/// and `generate_stream` returns the bare receiver with zero overhead.
pub const QNN_METRICS_PATH_ENV: &str = "PRIMER_QNN_METRICS_PATH";

/// Resolve a metrics output path from a raw env value. `None`/empty ⇒
/// recording disabled. Pure — host-tested; [`metrics_path_from_env`] is the
/// thin `std::env` wrapper.
pub fn metrics_path_from_value(value: Option<OsString>) -> Option<PathBuf> {
    let value = value?;
    if value.is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

/// Resolve the metrics output path from [`QNN_METRICS_PATH_ENV`]. `None` ⇒
/// recording disabled.
pub fn metrics_path_from_env() -> Option<PathBuf> {
    metrics_path_from_value(std::env::var_os(QNN_METRICS_PATH_ENV))
}

/// Round a float to two decimal places for a readable log without precision
/// theatre. Pure.
fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Format one metrics record as a single JSONL line.
///
/// Pure: the `ts_unix_ms` wall-clock timestamp (Unix epoch milliseconds) is
/// supplied by the caller so the function is deterministic and host-testable.
/// Uses `serde_json` so any future field is escaped correctly. Fields:
/// `ts_unix_ms`, `ttft_ms`, `decode_tokens`, `decode_ms`, `tok_per_s`.
pub fn format_metric_line(ts_unix_ms: u64, timing: &StreamTiming) -> String {
    serde_json::json!({
        "ts_unix_ms": ts_unix_ms,
        "ttft_ms": round2(timing.ttft.as_secs_f64() * 1000.0),
        "decode_tokens": timing.decode_tokens,
        "decode_ms": round2(timing.decode_duration.as_secs_f64() * 1000.0),
        "tok_per_s": round2(timing.decode_tokens_per_sec()),
    })
    .to_string()
}

/// The path the live metrics file is rotated to: its name with
/// [`ROTATED_SUFFIX`] appended (e.g. `qnn_metrics.jsonl` →
/// `qnn_metrics.jsonl.1`). Pure — host-tested.
pub fn rotated_metrics_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(ROTATED_SUFFIX);
    path.with_file_name(name)
}

/// Whether a metrics file of `current_len` bytes should be rotated before the
/// next append, given the cap `max_bytes`. Pure — host-tested. The check is
/// `>=` so the file never grows past the cap by more than a single trailing
/// record (the one appended after the check).
pub fn should_rotate(current_len: u64, max_bytes: u64) -> bool {
    current_len >= max_bytes
}

/// Rotate the live metrics file to [`rotated_metrics_path`] when it has reached
/// `max_bytes`, replacing any prior backup. Best-effort and never-panic: a
/// missing/unreadable file is a no-op, and a rename failure is logged and
/// swallowed (the subsequent append simply continues on the over-cap file —
/// degraded, never broken).
fn rotate_if_oversize(path: &Path, max_bytes: u64) {
    let len = match std::fs::metadata(path) {
        Ok(meta) => meta.len(),
        // No file yet (or unreadable) — nothing to rotate.
        Err(_) => return,
    };
    if !should_rotate(len, max_bytes) {
        return;
    }
    let rotated = rotated_metrics_path(path);
    if let Err(e) = std::fs::rename(path, &rotated) {
        tracing::warn!(
            target: "primer::qnn::metrics",
            "failed to rotate QNN metrics file {path:?} -> {rotated:?}: {e}"
        );
    }
}

/// Append `line` (one JSONL record) to `path`, creating the file if missing and
/// rotating it first when it has reached [`METRICS_FILE_MAX_BYTES`].
///
/// Thin wrapper over [`append_metric_line_capped`] using the production cap.
/// Best-effort and never-panic: a rotation, open, or write failure is logged
/// via `tracing::warn!` and swallowed, mirroring the `genie::log` sink — a
/// metrics write must never break a child's turn.
pub fn append_metric_line(path: &Path, line: &str) {
    append_metric_line_capped(path, line, METRICS_FILE_MAX_BYTES);
}

/// As [`append_metric_line`] but with an explicit size cap (so the rotation
/// behaviour is host-testable with a tiny cap). Rotates the existing file when
/// it is at or over `max_bytes`, then appends — bounding the on-disk footprint
/// at roughly `2 × max_bytes` (one live file plus one rotated backup).
pub fn append_metric_line_capped(path: &Path, line: &str, max_bytes: u64) {
    use std::io::Write as _;
    rotate_if_oversize(path, max_bytes);
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        Ok(mut file) => {
            if let Err(e) = writeln!(file, "{line}") {
                tracing::warn!(
                    target: "primer::qnn::metrics",
                    "failed to write QNN metrics line to {path:?}: {e}"
                );
            }
        }
        Err(e) => tracing::warn!(
            target: "primer::qnn::metrics",
            "failed to open QNN metrics file {path:?}: {e}"
        ),
    }
}

/// Format `timing` with the current wall-clock timestamp (Unix epoch ms) and
/// append it to the metrics file at `path`. Thin glue over the pure
/// [`format_metric_line`] + [`append_metric_line`]; the only impure part is
/// reading the clock. A pre-epoch clock (impossible in practice) records `0`.
pub fn record_timing(path: &Path, timing: &StreamTiming) {
    let ts_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    append_metric_line(path, &format_metric_line(ts_unix_ms, timing));
}

/// Boxed sink invoked once with the final [`StreamTiming`] when a
/// [`MeteredStream`] completes.
pub type MetricSink = Box<dyn FnOnce(StreamTiming) + Send>;

/// A [`TokenStream`] decorator that times the wrapped stream and hands its
/// final [`StreamTiming`] to a sink closure exactly once.
///
/// The consumer drives the wrapped stream exactly as if it were the inner one;
/// each chunk is observed by an internal [`StreamTimer`] (empty chunks — e.g.
/// the trailing `done` sentinel — are ignored for decode counting). When the
/// inner stream ends (normally OR after surfacing an error) the timing is
/// finalised and the sink fires. Performs no I/O of its own, so it is fully
/// host-testable with a closure that captures a shared buffer.
pub struct MeteredStream {
    inner: TokenStream,
    /// `Some` until finalised; `take`n so finalisation is idempotent.
    timer: Option<StreamTimer>,
    /// `Some` until finalised; the boxed sink fires exactly once.
    on_complete: Option<MetricSink>,
}

impl MeteredStream {
    /// Wrap `inner`, beginning timing at `issued` (the instant the
    /// `generate_stream` call was entered, so TTFT captures prefill latency).
    /// `on_complete` is invoked exactly once with the final timing when the
    /// stream ends.
    pub fn new(inner: TokenStream, issued: Instant, on_complete: MetricSink) -> Self {
        Self {
            inner,
            timer: Some(StreamTimer::start(issued)),
            on_complete: Some(on_complete),
        }
    }

    /// Finalise the timer and fire the sink, exactly once. Subsequent calls
    /// are no-ops (both `Option`s are emptied by the first call).
    fn finalize(&mut self) {
        if let (Some(timer), Some(sink)) = (self.timer.take(), self.on_complete.take()) {
            let timing = timer.finish(Instant::now());
            sink(timing);
        }
    }
}

impl Stream for MeteredStream {
    type Item = Result<TokenChunk>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                if let Some(timer) = this.timer.as_mut() {
                    timer.observe(!chunk.text.is_empty(), Instant::now());
                }
                // Finalize on the terminal `done` chunk, not only on the inner
                // stream's `None`: the dialogue manager's consume loop breaks
                // out as soon as it sees `chunk.done` and never polls again, so
                // the wrapped stream is dropped before yielding `None`. Without
                // this the sink would never fire for the real chat path.
                // `finalize` is idempotent, so the later `None`/error arms (for
                // consumers that DO drain to completion, e.g. backends that
                // close without a done sentinel) stay correct.
                if chunk.done {
                    this.finalize();
                }
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(e))) => {
                // An error terminates the turn — the consumer may stop
                // polling, so finalise here rather than waiting for `None`.
                this.finalize();
                Poll::Ready(Some(Err(e)))
            }
            Poll::Ready(None) => {
                this.finalize();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests;
