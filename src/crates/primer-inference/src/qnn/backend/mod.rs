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
use futures::stream;
use primer_core::error::{PrimerError, Result};
use primer_core::inference::{GenerationParams, InferenceBackend, Prompt, TokenChunk, TokenStream};
use tokio::sync::Mutex;

use super::genie::{GenieCallError, GenieDialog, GenieLibrary, RealGenieLibrary};
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
/// Kept as a const so downstream consumers (the dialogue manager's
/// per-backend context-budget logic, step 1.2.5) can pattern-match on
/// the prefix without hard-coding the string in two places.
pub const QNN_NAME_PREFIX: &str = "qnn:";

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
    /// Parsed `primer-meta.json` — exposed for future tunables
    /// (context_length, stop_sequences). Held by value because the meta
    /// is small (<1 KB) and shared ownership adds nothing.
    #[allow(dead_code)]
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
            smoke_check(&*dialog).map_err(|e| PrimerError::Inference(e.into_inference_error()))?;
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

/// Run the ABI smoke check: issue one tiny throwaway query and confirm
/// it returns `Ok`. Discards the response.
///
/// Pure-ish (no I/O beyond the Genie call); kept free-standing so tests
/// don't have to construct a full backend to pin the contract.
fn smoke_check(dialog: &dyn GenieDialog) -> std::result::Result<(), GenieCallError> {
    let _ = dialog.query_blocking(SMOKE_CHECK_PROMPT)?;
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

        // Single-shot for step 1.2.2. The streaming-callback bridge from
        // step 1.2.3 will replace this body without changing the trait
        // surface or the consumer side.
        let dialog = Arc::clone(&self.dialog);
        let response = {
            let guard = dialog.lock().await;
            // The mock impl is fast (~µs); the real impl can block for
            // seconds. `spawn_blocking` keeps the tokio runtime healthy
            // when the real Genie call lands.
            let dialog_inner = &*guard;
            // `dialog_inner: &Box<dyn GenieDialog>` is not `Send`-bound by
            // its trait, but the trait does require `Send + Sync` so
            // calling through it from a single async task is safe. We
            // intentionally do NOT cross the await boundary holding the
            // dialog through `spawn_blocking` for step 1.2.2 — the synch
            // call returns quickly enough on the mock-driven test path.
            // Step 1.2.3 (real device) wraps this in `spawn_blocking`.
            dialog_inner.query_blocking(&rendered)
        }
        .map_err(|e| PrimerError::Inference(e.into_inference_error()))?;

        let chunk = TokenChunk {
            text: response,
            done: true,
        };
        Ok(Box::pin(stream::once(async { Ok(chunk) })))
    }
}

#[cfg(test)]
mod tests;
