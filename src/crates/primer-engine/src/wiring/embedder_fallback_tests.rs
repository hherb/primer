//! Issue #336: an embedder that fails to initialise must report the
//! BM25-only fallback through `tracing` (which reaches whatever sink
//! the binary's subscriber installs), not a bare `eprintln!` (which
//! bypasses it). Each test points a real embedder at a closed local
//! port so the construction probe fails fast with no network.

use std::io::Write;
use std::sync::{Arc, Mutex};

/// An address nothing listens on: port 1 is reserved and refuses fast.
const UNREACHABLE_URL: &str = "http://127.0.0.1:1";

/// `MakeWriter` that appends every formatted event into a shared buffer.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Capture;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Install a thread-local capturing subscriber; the returned guard
/// uninstalls it. `#[tokio::test]` is current-thread, so the awaited
/// construction stays on this thread and its events are captured.
fn capture() -> (Capture, tracing::subscriber::DefaultGuard) {
    let cap = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(cap.clone())
        .with_ansi(false)
        .finish();
    (cap.clone(), tracing::subscriber::set_default(subscriber))
}

fn assert_warned_fallback(cap: &Capture) {
    let out = String::from_utf8(cap.0.lock().unwrap().clone()).unwrap();
    assert!(
        out.contains("WARN") && out.contains("falling back to BM25-only"),
        "expected a tracing WARN about the BM25-only fallback, got: {out:?}"
    );
}

#[cfg(feature = "ollama-embedding")]
#[tokio::test]
async fn ollama_init_failure_warns_via_tracing() {
    let (cap, _guard) = capture();
    let built = super::build_ollama_embedder(Some(UNREACHABLE_URL), Some("m"))
        .await
        .expect("feature compiled in: init failure degrades, never errs");
    assert!(built.is_none());
    assert_warned_fallback(&cap);
}

#[cfg(feature = "openai-compat-embedding")]
#[tokio::test]
async fn openai_compat_init_failure_warns_via_tracing() {
    let (cap, _guard) = capture();
    let built = super::build_openai_compat_embedder(Some(UNREACHABLE_URL), Some("m"), None)
        .await
        .expect("feature compiled in: init failure degrades, never errs");
    assert!(built.is_none());
    assert_warned_fallback(&cap);
}
