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
fn reported_bytes_done_uses_oversize_received_offset() {
    // The cap check fires before the per-chunk progress callback,
    // so on Oversize the last-observed `on_progress` offset is one
    // chunk behind the offset that actually triggered the abort.
    // The variant's `received` field carries the true offset; the
    // failure event must surface that, not the stale callback value.
    let err = DownloadError::Oversize {
        received: 2_000,
        cap: 1_000,
    };
    assert_eq!(reported_bytes_done_on_failure(512, &err), 2_000);
}

#[test]
fn reported_bytes_done_uses_last_progress_for_non_oversize() {
    // Every other failure kind is reported AFTER `on_progress`
    // (network errors arise inside `stream.next()`; I/O after
    // `file.write_all`), so the last-observed offset is the offset
    // we want to report.
    assert_eq!(
        reported_bytes_done_on_failure(12_345, &DownloadError::Timeout),
        12_345,
    );
    assert_eq!(
        reported_bytes_done_on_failure(7, &DownloadError::HttpStatus(404)),
        7,
    );
    assert_eq!(
        reported_bytes_done_on_failure(99, &DownloadError::Network("x".into())),
        99,
    );
    assert_eq!(
        reported_bytes_done_on_failure(42, &DownloadError::Io("x".into())),
        42,
    );
    assert_eq!(
        reported_bytes_done_on_failure(0, &DownloadError::NoUrl("k".into())),
        0,
    );
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
    let (addr, captured) = spawn_one_shot(move |_req| raw_response_200(&body_for_handler)).await;

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
    // The handler asserts on the request itself; we don't need to
    // re-inspect `captured` after the call returns.
    let (addr, _captured) = spawn_one_shot(move |req| {
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
    let (addr, _captured) = spawn_one_shot(move |_req| raw_response_200(&body_for_handler)).await;

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
    let (addr, _captured) = spawn_one_shot(move |_req| raw_response_200(&body_for_handler)).await;

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
        // 500 ms is comfortably under any real-network round-trip and
        // well above the localhost connect-setup time even on a loaded
        // CI runner; the original 150 ms occasionally flaked under
        // contention.
        .timeout(Duration::from_millis(500))
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

/// Server closes the socket mid-body after writing headers + a
/// fraction of the declared Content-Length. reqwest yields a stream
/// error; we classify it as `Network` (not `Timeout`) and the partial
/// is preserved so the next attempt resumes from the byte offset we
/// reached.
#[tokio::test]
async fn stream_to_path_preserves_partial_on_midstream_network_error() {
    let dir = TempDir::new().unwrap();
    let dest = dir.path().join("model.bin");
    let partial = partial_path_for(&dest);

    // Server announces 1000 bytes but writes only 50 then closes.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let mut request = String::new();
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
        let headers = "HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nContent-Type: application/octet-stream\r\n\r\n";
        let _ = sock.write_all(headers.as_bytes()).await;
        let _ = sock.write_all(&[b'M'; 50]).await;
        // Drop the socket abruptly mid-body.
        let _ = sock.shutdown().await;
        drop(sock);
    });

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/asset.bin");
    let err = stream_to_path(&client, &url, &dest, None, |_, _| {})
        .await
        .expect_err("truncated body must surface as an error");
    assert!(
        matches!(err, DownloadError::Network(_)),
        "midstream socket close must classify as Network, got {err:?}",
    );
    assert!(
        partial.exists(),
        "partial must survive a network error so the next attempt resumes",
    );
    let preserved = tokio::fs::read(&partial).await.unwrap();
    assert_eq!(
        preserved.len(),
        50,
        "preserved partial must hold the bytes that arrived before the close",
    );
    assert_eq!(&preserved[..], &[b'M'; 50][..]);
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
