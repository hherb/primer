//! Programmable mock implementation of [`super::GenieLibrary`] /
//! [`super::GenieDialog`] for unit tests.
//!
//! Records every method invocation so a test can pin both the call
//! sequence and the per-call arguments. `#[cfg(test)]` only — never
//! reaches release builds.
//!
//! Step 1.2.3 widened the contract from single-shot string responses to
//! per-token streaming. The mock supports three response shapes,
//! selected at construction time:
//!
//! - [`MockGenieLibrary::new_with_response`] (one body chunk + one done
//!   chunk). Back-compat with step 1.2.2 callers that only care that
//!   the response text reaches the consumer.
//! - [`MockGenieLibrary::new_with_tokens`] (N body chunks + one done
//!   chunk). Pins the new "tokens arrive in order, terminated by a
//!   single `done = true` sentinel" contract.
//! - [`MockGenieLibrary::new_with_tokens_then_error`] (N body chunks +
//!   one `Err` chunk, no done). Simulates Genie's `dialog_query`
//!   returning a non-success status after N callback fires.
//!
//! Plus the existing one-shot error injection knobs:
//!
//! - [`MockGenieLibrary::new_failing_open`] — the next `open_dialog`
//!   returns `Err`.
//! - [`MockGenieLibrary::new_failing_query`] — the next streaming query
//!   emits a single `Err` chunk and closes (no body chunks, no done).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures::channel::mpsc::UnboundedSender;
use primer_core::error::Result as PrimerResult;
use primer_core::inference::TokenChunk;

use super::{GenieCallError, GenieDialog, GenieLibrary};

/// Recorded events produced by [`MockGenieLibrary`] / [`MockGenieDialog`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockEvent {
    OpenDialog { config_path: PathBuf },
    Query { prompt: String },
    DropDialog,
}

/// Script controlling what the mock emits during `query_streaming`.
/// Selected at construction; immutable across the mock's lifetime so
/// every query gets the same shape (matches Genie's stateless-call
/// shape for our purposes).
#[derive(Debug)]
enum Script {
    /// Emit each token as a body chunk, then a single done chunk.
    Tokens(Vec<String>),
    /// Emit each token as a body chunk, then a single `Err` chunk.
    /// Used to simulate `dialog_query` returning non-success after N
    /// callback fires (mid-stream error).
    TokensThenError {
        tokens: Vec<String>,
        err: GenieCallError,
    },
}

/// Programmable mock library. Holds the script + one-shot error knobs
/// in an `Arc<MockInner>` so dialogs returned by `open_dialog` can
/// access the same recorded events and shared error queue.
pub struct MockGenieLibrary {
    inner: Arc<MockInner>,
}

struct MockInner {
    events: Mutex<Vec<MockEvent>>,
    /// `Some(err)` → the next `open_dialog` returns `Err(err)` and
    /// resets to `None`; `None` → `open_dialog` returns a fresh
    /// `MockGenieDialog`. One-shot to keep behaviour predictable —
    /// `GenieCallError` is not `Clone` (transitively through
    /// `std::io::Error`) so a persistent error would force a
    /// `Box::leak`-style workaround for marginal value.
    open_error: Mutex<Option<GenieCallError>>,
    /// `Some(err)` → the next `query_streaming` against any dialog
    /// returned by this library emits a single `Err(...)` chunk on the
    /// channel and closes (no body chunks, no done). Resets to `None`
    /// after the first take. Models the smoke-check path:
    /// [`super::super::backend::QnnBackend::new_with_library`] runs one
    /// throwaway query at construction; if that fails the construction
    /// fails too.
    query_error: Mutex<Option<GenieCallError>>,
    /// Script selecting what `query_streaming` emits when no
    /// `query_error` is set. Immutable across the mock's lifetime —
    /// re-used on every query.
    script: Script,
}

impl MockGenieLibrary {
    /// Mock that emits the canned response as a single body chunk
    /// (`done = false`) followed by one done chunk (`done = true`).
    /// Two chunks total per query.
    ///
    /// Back-compat shim for tests that don't care about the per-token
    /// shape but want to pin "the response text reaches the consumer".
    pub fn new_with_response(canned_response: impl Into<String>) -> Self {
        Self::new_internal(
            Script::Tokens(vec![canned_response.into()]),
            /*open_error=*/ None,
            /*query_error=*/ None,
        )
    }

    /// Mock that emits each token as a body chunk (`done = false`),
    /// then one final done chunk (`done = true`). `tokens.len() + 1`
    /// chunks total per query.
    ///
    /// This is the canonical step-1.2.3 shape: the C-ABI callback in
    /// the real impl fires once per token (or token chunk), each
    /// forwarding into the sender; the wrapper sends the final done
    /// after `dialog_query` returns.
    pub fn new_with_tokens<I, S>(tokens: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let tokens: Vec<String> = tokens.into_iter().map(Into::into).collect();
        Self::new_internal(
            Script::Tokens(tokens),
            /*open_error=*/ None,
            /*query_error=*/ None,
        )
    }

    /// Mock that emits each token as a body chunk, then a single
    /// `Err(PrimerError::Inference(err.into_inference_error()))` chunk
    /// and closes (no done chunk). `tokens.len() + 1` chunks total.
    ///
    /// Simulates the real impl's behaviour when `GenieDialog_query`
    /// returns a non-success status after N callbacks have fired:
    /// partial body reaches the consumer, the dialogue manager's
    /// existing error path drops the partial assistant turn.
    pub fn new_with_tokens_then_error<I, S>(tokens: I, err: GenieCallError) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let tokens: Vec<String> = tokens.into_iter().map(Into::into).collect();
        Self::new_internal(
            Script::TokensThenError { tokens, err },
            /*open_error=*/ None,
            /*query_error=*/ None,
        )
    }

    /// Mock that returns the given error from the next `open_dialog`
    /// call (one-shot), then returns `Ok` for any subsequent calls.
    /// Used to exercise the construction-error path of the safe
    /// wrapper.
    pub fn new_failing_open(err: GenieCallError) -> Self {
        Self::new_internal(
            Script::Tokens(Vec::new()),
            /*open_error=*/ Some(err),
            /*query_error=*/ None,
        )
    }

    /// Mock whose first `query_streaming` emits one `Err` chunk on the
    /// channel and closes (no body chunks, no done). Subsequent queries
    /// follow the empty-tokens script (just a done chunk).
    ///
    /// Used to exercise the smoke-check failure path in
    /// [`super::super::backend::QnnBackend::new_with_library`]: the
    /// smoke check is the first query against a freshly-opened dialog,
    /// so a one-shot failing query is exactly the right shape.
    pub fn new_failing_query(err: GenieCallError) -> Self {
        Self::new_internal(
            Script::Tokens(Vec::new()),
            /*open_error=*/ None,
            /*query_error=*/ Some(err),
        )
    }

    fn new_internal(
        script: Script,
        open_error: Option<GenieCallError>,
        query_error: Option<GenieCallError>,
    ) -> Self {
        Self {
            inner: Arc::new(MockInner {
                events: Mutex::new(Vec::new()),
                open_error: Mutex::new(open_error),
                query_error: Mutex::new(query_error),
                script,
            }),
        }
    }

    /// Snapshot of the events recorded so far, in order. Cheap clone
    /// of the inner Vec; tests pin call sequences via this.
    pub fn events(&self) -> Vec<MockEvent> {
        self.inner
            .events
            .lock()
            .expect("mock events mutex poisoned")
            .clone()
    }
}

impl GenieLibrary for MockGenieLibrary {
    fn open_dialog(&self, config_path: &Path) -> Result<Box<dyn GenieDialog>, GenieCallError> {
        self.inner
            .events
            .lock()
            .expect("mock events mutex poisoned")
            .push(MockEvent::OpenDialog {
                config_path: config_path.to_path_buf(),
            });
        if let Some(err) = self
            .inner
            .open_error
            .lock()
            .expect("mock open_error mutex poisoned")
            .take()
        {
            return Err(err);
        }
        Ok(Box::new(MockGenieDialog {
            inner: Arc::clone(&self.inner),
        }))
    }
}

pub struct MockGenieDialog {
    inner: Arc<MockInner>,
}

impl GenieDialog for MockGenieDialog {
    fn query_streaming(&self, prompt: &str, sender: UnboundedSender<PrimerResult<TokenChunk>>) {
        self.inner
            .events
            .lock()
            .expect("mock events mutex poisoned")
            .push(MockEvent::Query {
                prompt: prompt.to_string(),
            });

        // One-shot "smoke-check fails" path: emit one Err and close
        // (no body chunks, no done). Mirrors the real impl's behaviour
        // when `dialog_set_token_callback` or the smoke-check query
        // itself surfaces a non-success status before any tokens
        // arrive.
        if let Some(err) = self
            .inner
            .query_error
            .lock()
            .expect("mock query_error mutex poisoned")
            .take()
        {
            // `unbounded_send` failing means the receiver was already
            // dropped; ignore — Genie has no cancellation API and the
            // contract documented on the trait requires tolerating
            // this.
            let _ = sender.unbounded_send(Err(err.to_primer_error()));
            // sender drops here, closing the channel.
            return;
        }

        // Drive the configured script. We can't move out of the
        // `Script` because the mock may be queried multiple times in
        // serialisation tests; `GenieCallError::to_primer_error` takes
        // `&self` precisely so the borrowed error in `TokensThenError`
        // can be reused across queries without requiring `Clone` on
        // the variant (which transitively requires `Clone` on
        // `std::io::Error`).
        match &self.inner.script {
            Script::Tokens(tokens) => emit_tokens_then_done(tokens, &sender),
            Script::TokensThenError { tokens, err } => {
                emit_tokens(tokens, &sender);
                let _ = sender.unbounded_send(Err(err.to_primer_error()));
                // No done chunk after an error: the dialogue manager
                // drops partial turns on stream-error per existing
                // contract.
            }
        }
        // sender drops here when this function returns, closing the
        // channel and signalling end-of-stream to the receiver.
    }
}

fn emit_tokens(tokens: &[String], sender: &UnboundedSender<PrimerResult<TokenChunk>>) {
    for token in tokens {
        let _ = sender.unbounded_send(Ok(TokenChunk {
            text: token.clone(),
            done: false,
        }));
    }
}

fn emit_tokens_then_done(tokens: &[String], sender: &UnboundedSender<PrimerResult<TokenChunk>>) {
    emit_tokens(tokens, sender);
    let _ = sender.unbounded_send(Ok(TokenChunk {
        text: String::new(),
        done: true,
    }));
}

impl Drop for MockGenieDialog {
    fn drop(&mut self) {
        // Best-effort: a panicking thread may leave the mutex
        // poisoned; in that case we can't record the drop but we
        // also don't want to panic again during unwind.
        if let Ok(mut events) = self.inner.events.lock() {
            events.push(MockEvent::DropDialog);
        }
    }
}
