//! Safe trait abstraction over the Genie C API.
//!
//! The Phase 1.2 plan (step 1.2.2 §Tests) calls for:
//!
//! > introduce `GenieLibraryHandle` as a small trait (`open_dialog_from_config`,
//! > `query`, `set_token_callback`) so a `MockGenieLibrary` can stub out
//! > the C calls.
//!
//! This module implements that abstraction:
//!
//! - [`GenieLibrary`] — open a dialog from a `genie_config.json` path.
//! - [`GenieDialog`] — query the dialog (per-token streaming as of step 1.2.3);
//!   `Drop` releases the underlying handle.
//!
//! The real (`libGenie.so`-backed) implementation lives in [`real`] and is
//! reachable on every target — on non-Android hosts, [`RealGenieLibrary::open`]
//! returns a typed [`GenieCallError::PlatformUnsupported`] without touching
//! the filesystem (delegated to [`primer_qnn_sys::GenieLibrary::open`]).
//!
//! A [`mock`] module ships under `#[cfg(test)]` with a programmable
//! [`mock::MockGenieLibrary`] that records calls and replays scripted
//! token sequences (one body chunk per token + a final done chunk; or
//! N body chunks + a mid-stream `Err`). The [`super::backend::QnnBackend`]
//! is generic over `dyn GenieLibrary` so tests can substitute the mock
//! without touching any FFI symbol.

use std::path::Path;

use futures::channel::mpsc::UnboundedSender;
use primer_core::error::Result as PrimerResult;
use primer_core::inference::TokenChunk;
use primer_qnn_sys::{GENIE_STATUS_CONTEXT_LIMIT_EXCEEDED, GENIE_STATUS_SUCCESS, Genie_Status_t};
use thiserror::Error;

mod config;
mod log;
mod real;
pub use real::RealGenieLibrary;

#[cfg(test)]
pub(crate) mod mock;

/// Per-process handle to the loaded Genie library. Constructed once
/// (lazy dlopen) and shared across [`super::backend::QnnBackend`] instances
/// via `Arc<dyn GenieLibrary>`. Cheap to clone.
pub trait GenieLibrary: Send + Sync {
    /// Open a dialog session from a `genie_config.json` path. The returned
    /// handle owns the underlying dialog and config handles; dropping it
    /// invokes `GenieDialog_free` + `GenieDialogConfig_free`.
    fn open_dialog(&self, config_path: &Path) -> Result<Box<dyn GenieDialog>, GenieCallError>;
}

/// A live Genie dialog session. Single-threaded by C-API contract:
/// concurrent queries against the same dialog are undefined behaviour.
/// [`super::backend::QnnBackend`] wraps this in a `tokio::sync::Mutex` to
/// serialise callers.
///
/// The trait bound is `Send` only (deliberately not `Sync`): exclusive
/// access is mediated by the `tokio::sync::Mutex` in the backend, and
/// `tokio::sync::Mutex<T>: Send + Sync` already holds when `T: Send`.
/// Requiring `Sync` would be tighter than the Genie C contract actually
/// supports (it forbids concurrent queries) and would falsely advertise
/// a property the impl can't legally provide.
pub trait GenieDialog: Send {
    /// Run a streaming query against the dialog.
    ///
    /// Per-token streaming bridge (step 1.2.3): the implementation
    /// forwards generated tokens through `sender` as they're produced,
    /// then closes the channel by dropping the sender before returning.
    ///
    /// Contract:
    ///
    /// - For each token (or token chunk) produced by the model, emit
    ///   `Ok(TokenChunk { text, done: false })`.
    /// - On clean completion, emit one final
    ///   `Ok(TokenChunk { text: String::new(), done: true })` after the
    ///   last body chunk. The final chunk is the structural "stream
    ///   complete" sentinel — downstream `DialogueManager` relies on it
    ///   to commit the assistant turn.
    /// - On a Genie API error (callback registration failure, query
    ///   failure, malformed prompt), emit one
    ///   `Err(PrimerError::Inference(...))` instead of the final done
    ///   chunk and close. Any body chunks already emitted reach the
    ///   consumer; the dialogue manager's existing error path drops the
    ///   partial assistant turn.
    /// - Consumers may drop the receiver mid-stream (e.g. cancellation).
    ///   Implementations MUST tolerate `sender.unbounded_send` returning
    ///   `Err(SendError)` and continue to completion (Genie has no
    ///   cancellation API; the unsent tokens are simply discarded).
    ///
    /// This method is synchronous and potentially long-running. The
    /// real implementation is invoked from `tokio::task::spawn_blocking`
    /// in [`super::backend::QnnBackend::generate_stream`]; the mock
    /// returns essentially instantly and is safe to call from any
    /// context.
    fn query_streaming(&self, prompt: &str, sender: UnboundedSender<PrimerResult<TokenChunk>>);

    /// Reset the dialog's accumulated conversation/KV context to empty.
    ///
    /// **Load-bearing for the Primer's stateless prompt model.** The engine
    /// re-sends the *entire* prompt (system + recent history) on every query,
    /// and one dialog handle is shared by the chat turn AND the three
    /// background subsystems (classifier / extractor / comprehension). Genie
    /// otherwise *appends* every query to the same KV context, so without a
    /// reset before each query the 2048-token window saturates within a turn
    /// or two — the on-device "Context limit exceeded" failure. The backend
    /// calls this before every `generate_stream` query; the result surfaces a
    /// non-success status as an error so a broken reset doesn't silently let
    /// context leak between queries.
    fn reset(&self) -> PrimerResult<()>;
}

/// Errors produced by [`GenieLibrary`] or [`GenieDialog`] calls.
#[derive(Debug, Error)]
pub enum GenieCallError {
    /// The current build target has no QAIRT runtime support. Returned by
    /// [`RealGenieLibrary::open`] on non-Android hosts so the safe wrapper
    /// can route to the i18n boundary as
    /// `InferenceError::Other("local model not available")`.
    #[error("Genie runtime is only supported on Android; current platform is {0}")]
    PlatformUnsupported(&'static str),

    /// `libGenie.so` failed to load. Carries the underlying loader error
    /// for developer-facing logs.
    #[error("Genie library dlopen failed: {0}")]
    LibraryLoad(String),

    /// A required symbol was missing from the loaded library. Different
    /// from `LibraryLoad` so the safe wrapper can hint at "QAIRT version
    /// skew between what's installed and what the Primer expects".
    #[error("Genie symbol {symbol} missing from libGenie.so: {detail}")]
    SymbolMissing { symbol: String, detail: String },

    /// `genie_config.json` is missing from the bundle directory.
    #[error("genie_config.json not found at {path}")]
    ConfigNotFound { path: std::path::PathBuf },

    /// The config path could not be encoded as a NUL-terminated C string
    /// (typically because it contains an embedded NUL byte; non-UTF-8
    /// paths on macOS/Linux are fine — they become a `CString` via
    /// `OsStr`).
    #[error("genie_config.json path is not C-string-clean: {detail}")]
    BadConfigPath { detail: String },

    /// A Genie API call returned a non-success status code.
    #[error("Genie call {operation} returned non-success status {status}")]
    NonSuccess {
        /// Name of the C entry point that failed
        /// (e.g. `"GenieDialog_create"`). Static lifetime because we
        /// always pass a string literal — saves a per-error allocation.
        operation: &'static str,
        /// Raw `Genie_Status_t` value.
        status: i32,
    },
}

impl GenieCallError {
    /// Render this error into a `PrimerError::Inference(...)`. The string
    /// is dev-facing per [[project_multilingual_intent]] — user-visible
    /// translation happens via `primer_core::i18n::render_inference_error`
    /// downstream. This helper keeps the dev string consistent across the
    /// three or four call sites that wrap a Genie error.
    pub fn into_inference_error(self) -> primer_core::error::InferenceError {
        // PlatformUnsupported is the only variant the i18n layer might
        // want to discriminate ("local model not on this device"); the
        // others are all genuine inference failures. For step 1.2.2 we
        // collapse all of them into `Other(dev_string)` — step 1.2.4
        // (CLI wiring) is where we'd plumb the distinction through if
        // a translated message demands it.
        primer_core::error::InferenceError::Other(self.to_string())
    }

    /// Convert a borrowed `GenieCallError` into the `PrimerError::Inference`
    /// shape expected on the streaming channel.
    ///
    /// Used at every error site in the streaming path (real impl + mock)
    /// so the dev-facing string is identical by construction rather than
    /// by review. Takes `&self` because the mock's `Script::TokensThenError`
    /// retains ownership of the error across queries (it cannot move out
    /// of a borrowed enum variant).
    pub fn to_primer_error(&self) -> primer_core::error::PrimerError {
        primer_core::error::PrimerError::Inference(primer_core::error::InferenceError::Other(
            self.to_string(),
        ))
    }
}

/// Classification of a `GenieDialog_query` return status — drives whether
/// the real streaming path closes the turn gracefully or surfaces an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueryOutcome {
    /// Generation finished cleanly (`GENIE_STATUS_SUCCESS`).
    Complete,
    /// The context window filled mid-generation
    /// ([`GENIE_STATUS_CONTEXT_LIMIT_EXCEEDED`]). The reply already streamed
    /// in full via the token callback, so the turn completes with what was
    /// generated rather than being dropped.
    ContextLimit,
    /// A genuine failure (ABI mismatch, invalid handle, …) — propagate so a
    /// real fault is never masked as a successful turn.
    Error,
}

/// Pure classification of a raw `GenieDialog_query` status. Kept separate
/// from the FFI call in [`real`] so the graceful-vs-error decision is
/// unit-testable on any host without a device. Only the specific
/// context-limit code is treated as a graceful completion; every other
/// non-success code stays an error.
pub(crate) fn classify_query_status(status: Genie_Status_t) -> QueryOutcome {
    match status {
        GENIE_STATUS_SUCCESS => QueryOutcome::Complete,
        GENIE_STATUS_CONTEXT_LIMIT_EXCEEDED => QueryOutcome::ContextLimit,
        _ => QueryOutcome::Error,
    }
}

/// Emit the terminal stream item for a classified `GenieDialog_query`
/// outcome on `sender`. Extracted from the FFI path in [`real`] so the
/// graceful-vs-error emission is host-tested without an FFI round-trip
/// (the real path is `#[cfg(target_os = "android")]`-effective only).
///
/// - [`QueryOutcome::Complete`] / [`QueryOutcome::ContextLimit`]: the reply
///   already streamed in full via the token callback, so both close the
///   turn with a done chunk. `ContextLimit` additionally warns so the
///   small-context prompt budget (`primer-pedagogy`) can be tuned against
///   the overflow. (A context-limit reply can stop mid-sentence — softening
///   that for children is tracked in
///   <https://github.com/hherb/primer/issues/224>.)
/// - [`QueryOutcome::Error`]: surface the failure and emit **no** done
///   chunk — a genuine fault must drop the partial turn (see the
///   [`GenieDialog::query_streaming`] trait docs for the rationale).
pub(crate) fn emit_query_outcome(
    outcome: QueryOutcome,
    status: Genie_Status_t,
    sender: &UnboundedSender<PrimerResult<TokenChunk>>,
) {
    match outcome {
        QueryOutcome::Complete | QueryOutcome::ContextLimit => {
            if outcome == QueryOutcome::ContextLimit {
                tracing::warn!(
                    target: "primer::qnn",
                    "GenieDialog_query hit the context limit (status {status}); completing the turn with the streamed reply"
                );
            }
            let _ = sender.unbounded_send(Ok(TokenChunk {
                text: String::new(),
                done: true,
            }));
        }
        QueryOutcome::Error => {
            let err = GenieCallError::NonSuccess {
                operation: "GenieDialog_query",
                status,
            };
            let _ = sender.unbounded_send(Err(err.to_primer_error()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_success_status_is_complete() {
        assert_eq!(
            classify_query_status(GENIE_STATUS_SUCCESS),
            QueryOutcome::Complete
        );
    }

    #[test]
    fn classify_context_limit_status_is_graceful() {
        assert_eq!(
            classify_query_status(GENIE_STATUS_CONTEXT_LIMIT_EXCEEDED),
            QueryOutcome::ContextLimit
        );
    }

    #[test]
    fn classify_other_nonsuccess_statuses_are_errors() {
        // -1 (general), -2/-3 (invalid arg / mem), and arbitrary non-4
        // positives must all stay hard errors so a real ABI fault is never
        // silently treated as a completed turn.
        for status in [-1, -2, -3, 1, 2, 3, 5, 99] {
            assert_eq!(
                classify_query_status(status),
                QueryOutcome::Error,
                "status {status} should be an error"
            );
        }
    }

    #[test]
    fn emit_complete_outcome_sends_one_done_chunk() {
        use futures::StreamExt;
        use futures::executor::block_on;
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<PrimerResult<TokenChunk>>();
        emit_query_outcome(QueryOutcome::Complete, GENIE_STATUS_SUCCESS, &tx);
        drop(tx); // close the channel so the second `next()` resolves to None
        let chunk = block_on(rx.next())
            .expect("a terminal item")
            .expect("complete is Ok, not Err");
        assert!(chunk.done, "complete must emit a done chunk");
        assert_eq!(chunk.text, "", "the done chunk carries no text");
        assert!(
            block_on(rx.next()).is_none(),
            "exactly one terminal item, then the channel closes"
        );
    }

    #[test]
    fn emit_context_limit_outcome_completes_gracefully() {
        // The reply already streamed via the token callback, so a
        // context-limit return closes the turn with a done chunk (NOT an
        // error) — the same shape as Complete.
        use futures::StreamExt;
        use futures::executor::block_on;
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<PrimerResult<TokenChunk>>();
        emit_query_outcome(
            QueryOutcome::ContextLimit,
            GENIE_STATUS_CONTEXT_LIMIT_EXCEEDED,
            &tx,
        );
        drop(tx);
        let chunk = block_on(rx.next())
            .expect("a terminal item")
            .expect("context-limit completes gracefully (Ok), not Err");
        assert!(chunk.done, "context limit must still emit a done chunk");
    }

    #[test]
    fn emit_error_outcome_sends_err_and_no_done_chunk() {
        // A genuine fault surfaces an Err and emits NO done chunk, so the
        // dialogue manager drops the partial turn.
        use futures::StreamExt;
        use futures::executor::block_on;
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<PrimerResult<TokenChunk>>();
        emit_query_outcome(QueryOutcome::Error, -1, &tx);
        drop(tx);
        let item = block_on(rx.next()).expect("a terminal item");
        assert!(item.is_err(), "error outcome must surface an Err");
        assert!(
            block_on(rx.next()).is_none(),
            "no done chunk follows a genuine error"
        );
    }

    #[test]
    fn real_library_open_returns_platform_unsupported_on_host() {
        // The real library defers to `primer_qnn_sys::GenieLibrary::open`,
        // which short-circuits to `PlatformUnsupported` on every
        // non-Android target. Pin that mapping so a future refactor that
        // accidentally returned a `LibraryLoad` error on the host (e.g.
        // by trying to dlopen first) would fail this test.
        let result = RealGenieLibrary::open(Path::new("/non/existent/qairt/lib"));
        match result {
            Ok(_) => {
                // On a hypothetical Android host with QAIRT installed,
                // this could succeed; the test isn't checking the
                // negative case there.
                #[cfg(not(target_os = "android"))]
                panic!("RealGenieLibrary::open unexpectedly succeeded on non-Android host");
            }
            Err(e) => {
                #[cfg(not(target_os = "android"))]
                assert!(
                    matches!(e, GenieCallError::PlatformUnsupported(_)),
                    "expected PlatformUnsupported on host, got {e:?}",
                );
                // On Android with no QAIRT installed, the error variant
                // is LibraryLoad — also acceptable shape.
                #[cfg(target_os = "android")]
                assert!(matches!(e, GenieCallError::LibraryLoad(_)), "{e:?}");
            }
        }
    }

    #[test]
    fn genie_call_error_into_inference_error_lands_in_other() {
        // Step 1.2.2: all variants collapse into `Other(dev_string)`.
        // Pinning the contract so a future i18n change is a deliberate
        // edit, not an accidental drop of the developer message.
        let err = GenieCallError::NonSuccess {
            operation: "GenieDialog_query",
            status: 7,
        };
        let inf = err.into_inference_error();
        match inf {
            primer_core::error::InferenceError::Other(msg) => {
                assert!(msg.contains("GenieDialog_query"), "{msg}");
                assert!(msg.contains("7"), "{msg}");
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mock_library_records_open_and_query_in_order() {
        use futures::StreamExt;
        use futures::channel::mpsc;
        use mock::{MockEvent, MockGenieLibrary};

        let lib = MockGenieLibrary::new_with_response("hello from the mock model");
        let dialog = lib
            .open_dialog(Path::new("/some/genie_config.json"))
            .unwrap();
        let (tx, rx) = mpsc::unbounded();
        dialog.query_streaming("Why is the sky blue?", tx);
        let chunks: Vec<_> = rx.collect::<Vec<_>>().await;
        drop(dialog);
        // Single canned response → 1 body chunk + 1 done chunk = 2 chunks.
        assert_eq!(chunks.len(), 2, "{chunks:?}");
        let body = chunks[0]
            .as_ref()
            .expect("body chunk should be Ok on canned-response path");
        assert_eq!(body.text, "hello from the mock model");
        assert!(!body.done);
        let done = chunks[1]
            .as_ref()
            .expect("final chunk should be Ok on canned-response path");
        assert_eq!(done.text, "");
        assert!(done.done);

        let events = lib.events();
        assert_eq!(events.len(), 3);
        assert!(matches!(
            &events[0],
            MockEvent::OpenDialog { config_path } if config_path.to_str() == Some("/some/genie_config.json")
        ));
        assert!(
            matches!(&events[1], MockEvent::Query { prompt } if prompt == "Why is the sky blue?")
        );
        assert!(matches!(&events[2], MockEvent::DropDialog));
    }

    #[test]
    fn mock_library_propagates_open_error() {
        use mock::MockGenieLibrary;
        let lib = MockGenieLibrary::new_failing_open(GenieCallError::ConfigNotFound {
            path: "/missing".into(),
        });
        let result = lib.open_dialog(Path::new("/missing"));
        assert!(matches!(result, Err(GenieCallError::ConfigNotFound { .. })));
    }
}
