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

#[cfg(feature = "llamacpp")]
mod real {
    use super::*;
    use std::path::Path;
    use std::sync::OnceLock;

    use llama_cpp_2::context::params::LlamaContextParams;
    use llama_cpp_2::llama_backend::LlamaBackend;
    use llama_cpp_2::llama_batch::LlamaBatch;
    use llama_cpp_2::model::params::LlamaModelParams;
    use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaChatTemplate, LlamaModel};
    use llama_cpp_2::sampling::LlamaSampler;
    use primer_core::error::{PrimerError, Result};
    use primer_core::inference::{GenerationParams, Prompt, Role};
    use tokio::sync::Mutex;

    use crate::llamacpp::params::{
        resolve_n_ctx, sampler_spec, tail_matches_any_stop, validate_gguf_path,
    };

    /// Global llama.cpp backend handle. `LlamaBackend::init()` may be called
    /// only once per process; this lazily initialises it.
    static LLAMA_BACKEND: OnceLock<LlamaBackend> = OnceLock::new();

    fn backend_handle() -> Result<&'static LlamaBackend> {
        if let Some(b) = LLAMA_BACKEND.get() {
            return Ok(b);
        }
        // `LlamaBackend::init()` is a process-wide one-shot (internal CAS):
        // only the first caller gets `Ok`, any concurrent caller gets
        // `Err(BackendAlreadyInitialized)`. So we must NOT propagate the init
        // error directly — a second `RealLlamaEngine::new`/`infer` racing the
        // first would spuriously fail despite a healthy backend. Store on
        // success, ignore the error otherwise, then read back the populated
        // cell; only a genuinely-empty cell is a real failure.
        if let Ok(b) = LlamaBackend::init() {
            let _ = LLAMA_BACKEND.set(b);
        }
        LLAMA_BACKEND
            .get()
            .ok_or_else(|| PrimerError::Inference("llama.cpp backend init failed".into()))
    }

    /// Minimal fallback chat template for GGUFs that embed none.
    const GENERIC_CHAT_TEMPLATE: &str = "{% for m in messages %}{% if m.role == 'system' %}{{ m.content }}\n{% elif m.role == 'user' %}User: {{ m.content }}\n{% else %}Assistant: {{ m.content }}\n{% endif %}{% endfor %}Assistant:";

    /// Real llama-cpp-2-backed engine.
    pub struct RealLlamaEngine {
        model_id: String,
        model: std::sync::Arc<LlamaModel>,
        template: LlamaChatTemplate,
        n_ctx: u32,
        // llama.cpp forbids concurrent decode; serialise callers.
        ctx_guard: Mutex<()>,
    }

    impl RealLlamaEngine {
        /// Load a GGUF model from `gguf_path`.
        ///
        /// `n_gpu_layers` follows the llama.cpp convention where a negative
        /// value (e.g. `-1` = `LLAMACPP_GPU_LAYERS_ALL`) means "offload all
        /// layers"; we leave `LlamaModelParams`' default (which is `-1`) in
        /// that case, because the `with_n_gpu_layers` setter takes a `u32`.
        pub fn new(
            gguf_path: &Path,
            n_gpu_layers: i32,
            n_ctx_override: Option<u32>,
        ) -> Result<Self> {
            validate_gguf_path(gguf_path).map_err(PrimerError::Inference)?;
            let backend = backend_handle()?;
            let mut model_params = LlamaModelParams::default();
            if n_gpu_layers >= 0 {
                model_params = model_params.with_n_gpu_layers(n_gpu_layers as u32);
            }
            let model = LlamaModel::load_from_file(backend, gguf_path, &model_params)
                .map_err(|e| PrimerError::Inference(format!("GGUF load failed: {e}").into()))?;

            let template = model.chat_template(None).unwrap_or_else(|_| {
                LlamaChatTemplate::new(GENERIC_CHAT_TEMPLATE).expect("generic template is valid")
            });

            let model_id = gguf_path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "llamacpp-model".to_string());

            Ok(Self {
                model_id,
                model: std::sync::Arc::new(model),
                template,
                n_ctx: resolve_n_ctx(n_ctx_override),
                ctx_guard: Mutex::new(()),
            })
        }
    }

    impl LlamaEngine for RealLlamaEngine {
        fn model_id(&self) -> &str {
            &self.model_id
        }

        fn render_prompt(&self, prompt: &Prompt) -> Result<String> {
            let mut messages = Vec::with_capacity(prompt.messages.len() + 1);
            if !prompt.system.is_empty() {
                messages.push(
                    LlamaChatMessage::new("system".to_string(), prompt.system.clone())
                        .map_err(|e| PrimerError::Inference(format!("chat msg: {e}").into()))?,
                );
            }
            for m in &prompt.messages {
                let role = match m.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                };
                messages.push(
                    LlamaChatMessage::new(role.to_string(), m.content.clone())
                        .map_err(|e| PrimerError::Inference(format!("chat msg: {e}").into()))?,
                );
            }
            self.model
                .apply_chat_template(&self.template, &messages, true)
                .map_err(|e| PrimerError::Inference(format!("chat template: {e}").into()))
        }

        fn infer(
            &self,
            rendered: &str,
            params: &GenerationParams,
            on_token: &mut dyn FnMut(&str) -> bool,
        ) -> Result<()> {
            let _guard = self.ctx_guard.blocking_lock();
            let backend = backend_handle()?;

            let n_ctx = if self.n_ctx == 0 {
                None
            } else {
                std::num::NonZeroU32::new(self.n_ctx)
            };
            let mut ctx = self
                .model
                .new_context(backend, LlamaContextParams::default().with_n_ctx(n_ctx))
                .map_err(|e| PrimerError::Inference(format!("context: {e}").into()))?;

            let tokens = self
                .model
                .str_to_token(rendered, AddBos::Always)
                .map_err(|e| PrimerError::Inference(format!("tokenize: {e}").into()))?;

            let mut batch = LlamaBatch::new(512, 1);
            let last = tokens.len() - 1;
            for (i, tok) in tokens.iter().enumerate() {
                batch
                    .add(*tok, i as i32, &[0], i == last)
                    .map_err(|e| PrimerError::Inference(format!("batch: {e}").into()))?;
            }
            ctx.decode(&mut batch)
                .map_err(|e| PrimerError::Inference(format!("decode: {e}").into()))?;

            let spec = sampler_spec(params);
            let mut sampler = LlamaSampler::chain_simple([
                LlamaSampler::top_p(spec.top_p, 1),
                LlamaSampler::temp(spec.temperature),
                LlamaSampler::dist(spec.seed),
            ]);

            let mut n_cur = batch.n_tokens();
            let mut accumulated = String::new();
            let mut decoder = encoding_rs::UTF_8.new_decoder();

            for _ in 0..params.max_tokens {
                let token = sampler.sample(&ctx, batch.n_tokens() - 1);
                sampler.accept(token);
                if token == self.model.token_eos() {
                    break;
                }
                let piece = self
                    .model
                    .token_to_piece(token, &mut decoder, false, None)
                    .map_err(|e| PrimerError::Inference(format!("detok: {e}").into()))?;
                accumulated.push_str(&piece);
                if !on_token(&piece) {
                    return Ok(());
                }
                if tail_matches_any_stop(&accumulated, &params.stop_sequences) {
                    break;
                }
                batch.clear();
                batch
                    .add(token, n_cur, &[0], true)
                    .map_err(|e| PrimerError::Inference(format!("batch: {e}").into()))?;
                n_cur += 1;
                ctx.decode(&mut batch)
                    .map_err(|e| PrimerError::Inference(format!("decode: {e}").into()))?;
            }
            Ok(())
        }
    }
}

#[cfg(feature = "llamacpp")]
pub use real::RealLlamaEngine;
