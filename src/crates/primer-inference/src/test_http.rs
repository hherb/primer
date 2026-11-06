//! One-shot local HTTP server for driving a real backend's
//! `generate_stream` end to end (issue #313). The buffer unit tests
//! cover parsing; these cover the spawn loop's EOF handling, which only
//! a closing connection exercises.

use futures::StreamExt;
use primer_core::inference::{GenerationParams, InferenceBackend, Prompt, TokenChunk, TokenStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Answer one request with `200 OK` and `body`, then close the
/// connection. No `Content-Length`: the body ends at EOF, exactly like a
/// server that closes right after flushing its last bytes. Returns the
/// base URL (`http://127.0.0.1:<port>`).
pub(crate) async fn serve_once(body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        read_request(&mut sock).await;
        let head =
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
        sock.write_all(head.as_bytes()).await.expect("write head");
        sock.write_all(body.as_bytes()).await.expect("write body");
        sock.shutdown().await.expect("shutdown");
    });
    format!("http://{addr}")
}

/// Answer every request with a bare `503 Service Unavailable` (a
/// retryable status for every HTTP backend), counting the requests.
/// Returns the base URL and the shared counter.
pub(crate) async fn serve_503_counting() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&count);
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            read_request(&mut sock).await;
            seen.fetch_add(1, Ordering::SeqCst);
            let resp = "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });
    (format!("http://{addr}"), count)
}

/// Build a backend against an always-`503` server (via `make(url)`),
/// call `generate_stream` once with `params`, assert it fails pre-stream,
/// and return how many HTTP requests it made — i.e. how many retry
/// attempts the backend spent (issue #312).
pub(crate) async fn requests_until_pre_stream_error<B: InferenceBackend>(
    make: impl FnOnce(String) -> B,
    params: &GenerationParams,
) -> usize {
    let (url, count) = serve_503_counting().await;
    let prompt = Prompt {
        system: String::new(),
        messages: vec![],
    };
    assert!(
        make(url).generate_stream(&prompt, params).await.is_err(),
        "an always-503 server must fail pre-stream"
    );
    count.load(Ordering::SeqCst)
}

/// [`GenerationParams`] carrying a one-attempt retry cap, as
/// `FallbackBackend` / `RouterBackend` hand a leg with a failover behind it.
pub(crate) fn failover_leg_params() -> GenerationParams {
    GenerationParams::default().for_failover_leg()
}

/// Consume the request (headers + `Content-Length` body) so closing the
/// socket doesn't reset the connection over unread bytes.
async fn read_request(sock: &mut tokio::net::TcpStream) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = sock.read(&mut chunk).await.expect("read request");
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
        let body_len: usize = headers
            .lines()
            .find_map(|l| l.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        if buf.len() >= end + 4 + body_len {
            return;
        }
    }
}

/// Drain a stream, panicking on any error item.
pub(crate) async fn collect(mut stream: TokenStream) -> Vec<TokenChunk> {
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        out.push(item.expect("stream item"));
    }
    out
}
