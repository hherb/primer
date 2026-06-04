//! The `LlamaEngine` seam + the real `llama-cpp-2`-backed implementation.
//!
//! The trait is ALWAYS compiled so the backend orchestration is testable
//! with a mock on the default `cargo test`. `RealLlamaEngine` and its
//! `llama-cpp-2` calls are behind the `llamacpp` cargo feature (added in a
//! later task).

use primer_core::error::Result;
use primer_core::inference::{GenerationParams, Prompt};

/// Abstraction over the blocking llama.cpp generation surface.
///
/// Implemented for real by `RealLlamaEngine` (feature-gated, later task) and
/// by a test-only mock in the backend module. The single non-trivial method,
/// [`LlamaEngine::infer`], owns the blocking decode loop and emits RAW token
/// text — reasoning-marker stripping is the backend's job.
pub trait LlamaEngine: Send + Sync {
    /// Model identifier used to build `LlamaCppBackend::name()`
    /// (e.g. the GGUF file stem).
    fn model_id(&self) -> &str;

    /// Render a [`Prompt`] into the model's prompt string via its chat
    /// template. Cheap CPU; runs on the calling task.
    fn render_prompt(&self, prompt: &Prompt) -> Result<String>;

    /// Run the blocking decode loop over `rendered`. For each detokenized
    /// RAW piece, call `on_token(piece)`; stop early if it returns `false`
    /// (the consumer dropped the stream). Return `Ok(())` on natural
    /// completion (eos / max_tokens / a matched stop sequence), or `Err`
    /// on a decode failure.
    fn infer(
        &self,
        rendered: &str,
        params: &GenerationParams,
        on_token: &mut dyn FnMut(&str) -> bool,
    ) -> Result<()>;
}
