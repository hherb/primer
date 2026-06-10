//! [`QnnBackend`] — the [`InferenceBackend`] impl for the Qualcomm NPU.
//!
//! Phase 1.2 step 1.2.2: non-streaming generation only. The trait-backed
//! [`super::genie::GenieLibrary`] / [`super::genie::GenieDialog`] split lets the
//! construction → query → drop happy-path be exercised entirely against
//! the test-only `MockGenieLibrary` on host. Streaming-token bridge
//! (the per-token C-ABI callback feeding an mpsc receiver) lands in
//! step 1.2.3.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use futures::channel::mpsc;
use primer_core::error::{PrimerError, Result};
use primer_core::inference::{GenerationParams, InferenceBackend, Prompt, TokenChunk, TokenStream};
use tokio::sync::Mutex;

use super::genie::{GenieDialog, GenieLibrary, RealGenieLibrary};
use super::meta::PrimerMeta;
use super::template::{ChatTemplate, TemplateError};

/// Filename of the Genie dialog config inside a `genie_bundle` directory.
///
/// Phase 1.2 design §1: Genie's `GenieDialogConfig_createFromJson`
/// consumes this file directly. The bundle is otherwise opaque to the
/// Primer — relative paths inside the JSON reference the per-shard
/// context binaries.
pub const GENIE_CONFIG_FILENAME: &str = "genie_config.json";

/// The "tiny throwaway" prompt used by the ABI smoke check at backend
/// construction. Phase 1.2 plan step 1.2.2 task 8:
///
/// > immediately after `GenieDialog_create` succeeds, issue a tiny
/// > throwaway `GenieDialog_query` (e.g. a single-token prompt) and verify
/// > the returned `Genie_Status_t` is `GENIE_STATUS_SUCCESS`. ... This
/// > smoke check is what makes "first real call" happen synchronously
/// > inside `QnnBackend::new` instead of mid-conversation, so an ABI
/// > mismatch surfaces at startup with a clean error rather than
/// > corrupting a child's turn.
///
/// `"."` is the most minimal valid UTF-8 input; the response is discarded
/// and never reaches the conversation.
pub const SMOKE_CHECK_PROMPT: &str = ".";

/// Default suffix prefixed to `model_id` in [`InferenceBackend::name`].
///
/// Re-exported from `primer-core` so the producer here and the dialogue
/// manager's per-backend context-budget logic (step 1.2.5, which lives in
/// `primer-pedagogy` and cannot depend on this crate) match on one shared
/// constant rather than two hand-copied literals.
pub use primer_core::backend::QNN_NAME_PREFIX;

/// Qualcomm NPU inference backend.
///
/// Construction wires together the Genie library handle, the loaded
/// [`PrimerMeta`], the compiled chat template, and an open dialog
/// session. Per-call work in `generate_stream` is: render the prompt
/// through the template, take the dialog mutex, run the query inside
/// `tokio::task::spawn_blocking`, wrap the result in a single
/// [`TokenChunk`] stream.
pub struct QnnBackend {
    /// Cached `"qnn:{model_id}"` so [`InferenceBackend::name`] can return
    /// a `&str` without allocating on the hot path.
    name: String,
    /// Parsed `primer-meta.json` — `model_id` and `context_length` are
    /// already read by [`Self::fmt`]; `vocab_size` and `stop_sequences`
    /// will be wired into the per-backend context-budget logic in step
    /// 1.2.5 and the streaming generation halt path in step 1.2.3
    /// respectively. Held by value because the meta is small (<1 KB)
    /// and shared ownership adds nothing.
    meta: PrimerMeta,
    /// Compiled chat template. Cloned per call (cheap, inner `Arc`).
    template: ChatTemplate,
    /// The single dialog session. `tokio::sync::Mutex` serialises
    /// concurrent `generate_stream` callers; Genie does not support
    /// concurrent queries on the same dialog handle.
    dialog: Arc<Mutex<Box<dyn GenieDialog>>>,
}

impl std::fmt::Debug for QnnBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QnnBackend")
            .field("name", &self.name)
            .field("model_id", &self.meta.model_id)
            .field("context_length", &self.meta.context_length)
            .finish_non_exhaustive()
    }
}

impl QnnBackend {
    /// Real-library constructor.
    ///
    /// Opens `libGenie.so` from `qairt_lib_dir` and a dialog from
    /// `bundle_dir`. On non-Android hosts this returns
    /// `PrimerError::Inference(InferenceError::Other("...PlatformUnsupported..."))`
    /// via [`RealGenieLibrary::open`] — exactly the shape step 1.2.4 will
    /// surface to the CLI.
    ///
    /// Includes the ABI smoke check by default. Use
    /// [`Self::new_with_library`] with `run_smoke_check = false` to skip it.
    pub async fn new(bundle_dir: PathBuf, qairt_lib_dir: PathBuf) -> Result<Self> {
        let lib = RealGenieLibrary::open(&qairt_lib_dir)
            .map_err(|e| PrimerError::Inference(e.into_inference_error()))?;
        Self::new_with_library(bundle_dir, lib, /*run_smoke_check=*/ true).await
    }

    /// Test-facing constructor — accepts any [`GenieLibrary`] impl.
    ///
    /// `run_smoke_check` controls whether the ABI smoke check fires after
    /// the dialog is opened. Production callers (via [`Self::new`]) pass
    /// `true`; the mock-driven test path can pass `false` to exercise the
    /// "constructed but never queried" lifecycle.
    pub async fn new_with_library(
        bundle_dir: PathBuf,
        lib: Arc<dyn GenieLibrary>,
        run_smoke_check: bool,
    ) -> Result<Self> {
        validate_bundle_dir(&bundle_dir).map_err(PrimerError::Inference)?;
        let meta = PrimerMeta::load_or_fallback(&bundle_dir)
            .map_err(|e| PrimerError::Inference(format!("primer-meta.json: {e}").into()))?;
        let template = ChatTemplate::compile(&meta.chat_template).map_err(|e| {
            PrimerError::Inference(format!("invalid chat_template in primer-meta.json: {e}").into())
        })?;

        let config_path = bundle_dir.join(GENIE_CONFIG_FILENAME);
        let dialog = lib
            .open_dialog(&config_path)
            .map_err(|e| PrimerError::Inference(e.into_inference_error()))?;

        if run_smoke_check {
            // Fire the smoke-check query *synchronously* so the
            // `&dyn GenieDialog` borrow does not escape into an async
            // body. Then drain the receiver asynchronously — the
            // receiver is owned, so the resulting future captures
            // only `UnboundedReceiver`, not the dialog reference.
            // Keeping the borrow off the async path is what lets
            // `QnnBackend::new`'s future remain `Send` (and therefore
            // satisfies the Tauri-command Send bound in `primer-gui`)
            // even though `dyn GenieDialog` deliberately omits the
            // `Sync` bound per the trait-design comment.
            let rx = fire_smoke_check_query(&*dialog);
            drain_smoke_check_receiver(rx).await?;
        }

        let name = format!("{QNN_NAME_PREFIX}{}", meta.model_id);
        Ok(Self {
            name,
            meta,
            template,
            dialog: Arc::new(Mutex::new(dialog)),
        })
    }

    /// Render `prompt` through the model's chat template. Exposed for
    /// tests that want to pin template output without going through a
    /// full `generate_stream` round-trip.
    pub fn render_prompt(&self, prompt: &Prompt) -> std::result::Result<String, TemplateError> {
        self.template.render(prompt)
    }
}

/// Validate `bundle_dir` has the artefacts Genie needs. Returns a
/// dev-facing [`primer_core::error::InferenceError`] on failure.
///
/// Pure helper — exposed at module level so tests can pin its behaviour
/// without constructing a backend.
pub fn validate_bundle_dir(
    bundle_dir: &Path,
) -> std::result::Result<(), primer_core::error::InferenceError> {
    if !bundle_dir.is_dir() {
        return Err(format!(
            "QNN bundle directory does not exist or is not a directory: {}",
            bundle_dir.display()
        )
        .into());
    }
    let config = bundle_dir.join(GENIE_CONFIG_FILENAME);
    if !config.is_file() {
        return Err(format!(
            "QNN bundle is missing {GENIE_CONFIG_FILENAME}: {}",
            config.display()
        )
        .into());
    }
    Ok(())
}

/// Fire the smoke-check streaming query synchronously and return the
/// receiver end. Borrow on `dialog` ends with this function's return.
///
/// Split from [`smoke_check`] so the `&dyn GenieDialog` borrow does
/// NOT cross an `.await` boundary — that's what lets
/// [`QnnBackend::new`]'s future remain `Send` even though
/// `dyn GenieDialog` deliberately omits the `Sync` bound (the trait
/// is `Send` only by design; concurrent access is mediated by the
/// `tokio::sync::Mutex` in the backend, never by sharing `&dyn` across
/// threads). Without this split, holding the parameter in
/// `smoke_check`'s `async fn` state past the `rx.next().await` would
/// force `&dyn GenieDialog: Send`, which in turn forces
/// `dyn GenieDialog: Sync` — a contract the C-side cannot legally
/// provide.
fn fire_smoke_check_query(dialog: &dyn GenieDialog) -> mpsc::UnboundedReceiver<Result<TokenChunk>> {
    let (tx, rx) = mpsc::unbounded::<Result<TokenChunk>>();
    dialog.query_streaming(SMOKE_CHECK_PROMPT, tx);
    // `query_streaming` consumed the sender; the channel closes when
    // it returns. The caller drains.
    rx
}

/// Drain the smoke-check receiver, surfacing the first `Err` chunk
/// as the construction failure.
///
/// Takes ownership of the receiver so the resulting future captures
/// only `UnboundedReceiver<Result<TokenChunk>>` (which is `Send`) and
/// NOT any reference to the dialog. Together with
/// [`fire_smoke_check_query`] this two-step shape replaces an earlier
/// single-async-fn that held `&dyn GenieDialog` across its `await`,
/// which would have forced `dyn GenieDialog: Sync` and conflicted
/// with the trait's deliberate `Send`-only design.
///
/// Pure inputs/outputs from the caller's perspective: feed in a
/// receiver, get back `Ok(())` on success or the first `Err` chunk.
/// Step 1.2.3: a successful smoke check exercises exactly the FFI
/// sequence (`dialog_query` with the token callback → callback fires →
/// final done chunk) the real conversation will hit. (QAIRT 2.45 folds
/// callback registration into `dialog_query`; there is no separate
/// `setTokenCallback` step.)
async fn drain_smoke_check_receiver(
    mut rx: mpsc::UnboundedReceiver<Result<TokenChunk>>,
) -> Result<()> {
    while let Some(chunk_result) = rx.next().await {
        chunk_result?;
    }
    Ok(())
}

#[async_trait]
impl InferenceBackend for QnnBackend {
    fn name(&self) -> &str {
        &self.name
    }

    async fn is_available(&self) -> bool {
        // Construction succeeded ⇒ library loaded, dialog opened, smoke
        // check (if enabled) passed. Genie has no health-probe call, so
        // there's no useful per-call refinement.
        true
    }

    async fn generate_stream(
        &self,
        prompt: &Prompt,
        _params: &GenerationParams,
    ) -> Result<TokenStream> {
        // Render once on the calling task — pure CPU, cheap.
        let rendered = self.template.render(prompt).map_err(|e| {
            PrimerError::Inference(format!("chat template render failed: {e}").into())
        })?;

        // Phase 1.2.3 streaming bridge:
        //
        // 1. Build an mpsc channel for the token receiver path.
        // 2. Clone the dialog `Arc<Mutex>` into a tokio blocking task.
        // 3. The blocking task acquires the mutex (Genie does not
        //    support concurrent queries on the same dialog handle;
        //    this serialises concurrent `generate_stream` callers), then
        //    drives `query_streaming` which forwards each Genie callback
        //    fire into the sender. When `query_streaming` returns, the
        //    sender is dropped and the receiver sees end-of-stream.
        // 4. Return immediately with the receiver wrapped as a
        //    `TokenStream`; the consumer drives it without blocking the
        //    runtime even when the underlying Genie call takes seconds.
        let (tx, rx) = mpsc::unbounded::<Result<TokenChunk>>();
        let dialog = Arc::clone(&self.dialog);
        tokio::task::spawn_blocking(move || {
            // `blocking_lock` is the documented way to acquire a
            // `tokio::sync::Mutex` from inside `spawn_blocking`; it
            // does not poll the runtime so we cannot deadlock on the
            // current-thread runtime even if the runtime has no idle
            // worker.
            let guard = dialog.blocking_lock();
            guard.query_streaming(&rendered, tx);
            // Sender dropped on guard drop / fn return → channel closes.
        });
        Ok(Box::pin(rx))
    }
}

#[cfg(test)]
mod tests;
