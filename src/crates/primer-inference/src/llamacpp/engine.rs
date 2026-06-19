//! The `LlamaEngine` seam + the real `llama-cpp-2`-backed implementation.
//!
//! The trait is ALWAYS compiled so the backend orchestration is testable
//! with a mock on the default `cargo test`. `RealLlamaEngine` and its
//! `llama-cpp-2` calls are behind the `llamacpp` cargo feature (added in a
//! later task).

use primer_core::error::Result;
use primer_core::inference::{FinishReason, GenerationParams, Prompt};

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
    /// (the consumer dropped the stream). On natural completion return the
    /// [`FinishReason`] describing *why* generation ended:
    /// [`FinishReason::Length`] when the reply was truncated by the
    /// `max_tokens` / context budget (so the dialogue manager's context-limit
    /// recovery fires), or [`FinishReason::Stop`] on a clean finish (eos / a
    /// matched stop sequence / the consumer dropping the stream). Return `Err`
    /// on a decode failure.
    fn infer(
        &self,
        rendered: &str,
        params: &GenerationParams,
        on_token: &mut dyn FnMut(&str) -> bool,
    ) -> Result<FinishReason>;
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
        parse_add_bos_metadata, resolve_n_ctx, sampler_spec, should_prepend_bos,
        validate_gguf_path, visible_prefix_before_stop,
    };

    /// GGUF metadata key carrying the tokenizer's `add_bos_token` flag.
    const ADD_BOS_TOKEN_META_KEY: &str = "tokenizer.ggml.add_bos_token";

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
        // Model-constant BOS inputs to `should_prepend_bos`, resolved once at
        // load (issue #201). `bos_piece` is the BOS token's text form (e.g.
        // `<bos>`, `<|begin_of_text|>`) used to detect a template that already
        // embeds a literal BOS; `None` when the model has no/empty BOS piece.
        // `meta_add_bos` is the parsed `tokenizer.ggml.add_bos_token` metadata;
        // `None` when absent or unparseable.
        bos_piece: Option<String>,
        meta_add_bos: Option<bool>,
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

            // Resolve the BOS inputs once. `token_to_piece` with `special=true`
            // renders the BOS token to its text form; an empty/errored piece
            // (a model with no BOS) collapses to `None` so the literal-BOS
            // guard never matches the start of every prompt.
            let mut bos_decoder = encoding_rs::UTF_8.new_decoder();
            let bos_piece = model
                .token_to_piece(model.token_bos(), &mut bos_decoder, true, None)
                .ok()
                .filter(|s| !s.is_empty());
            let meta_add_bos = model
                .meta_val_str(ADD_BOS_TOKEN_META_KEY)
                .ok()
                .and_then(|raw| parse_add_bos_metadata(&raw));

            Ok(Self {
                model_id,
                model: std::sync::Arc::new(model),
                template,
                n_ctx: resolve_n_ctx(n_ctx_override),
                bos_piece,
                meta_add_bos,
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
        ) -> Result<FinishReason> {
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

            // `apply_chat_template` returns plain text WITHOUT a prepended BOS
            // token id, but `str_to_token` parses special tokens in its input,
            // so a template that embeds a *literal* BOS marker (Gemma's
            // `<bos>`, Llama 3's `<|begin_of_text|>`) already yields one BOS.
            // Adding another via `AddBos::Always` produces a quality-degrading
            // double-BOS (issue #201). `should_prepend_bos` combines the
            // literal-BOS-in-template check with the model's `add_bos_token`
            // metadata; the common chat models (no literal BOS, no metadata)
            // keep the historical add-once behaviour. Empirical cross-model
            // confirmation stays owner-gated (the llamacpp real-model smoke).
            let add_bos =
                if should_prepend_bos(rendered, self.bos_piece.as_deref(), self.meta_add_bos) {
                    AddBos::Always
                } else {
                    AddBos::Never
                };
            let tokens = self
                .model
                .str_to_token(rendered, add_bos)
                .map_err(|e| PrimerError::Inference(format!("tokenize: {e}").into()))?;

            // `tokens` is non-empty in practice: a non-trivial rendered prompt
            // tokenizes to at least one token, and the `AddBos::Always` branch
            // additionally guarantees a leading BOS. Guard explicitly anyway —
            // since the per-model decision above can now pick `AddBos::Never`
            // (issue #201), the no-BOS path is more reachable than before. An
            // empty `tokens` would make `tokens.len() - 1` below underflow
            // (panic in debug, wrap to usize::MAX in release → no token ever
            // carries logits → garbage sampling); reject it cleanly instead.
            if tokens.is_empty() {
                return Err(PrimerError::Inference(
                    "tokenization produced no tokens (empty prompt and model has no BOS)".into(),
                ));
            }

            // Prefill the whole prompt, decoding in `n_batch`-sized chunks. A
            // fixed single batch (the previous `LlamaBatch::new(512, 1)`)
            // overflowed on realistic prompts — the Socratic system prompt +
            // retrieved KB passages + conversation history routinely exceed 512
            // tokens — failing with an opaque `batch: InsufficientSpace(512)`.
            // Chunking lets prompts up to the model's context length prefill
            // correctly. Only the final prompt token carries logits.
            let n_batch = (ctx.n_batch() as usize).max(1);
            let last_idx = tokens.len() - 1;
            let mut batch = LlamaBatch::new(n_batch, 1);
            let mut pos: i32 = 0;
            for chunk in tokens.chunks(n_batch) {
                batch.clear();
                for (j, tok) in chunk.iter().enumerate() {
                    let is_last = pos as usize + j == last_idx;
                    batch
                        .add(*tok, pos + j as i32, &[0], is_last)
                        .map_err(|e| PrimerError::Inference(format!("batch: {e}").into()))?;
                }
                ctx.decode(&mut batch)
                    .map_err(|e| PrimerError::Inference(format!("decode: {e}").into()))?;
                pos += chunk.len() as i32;
            }

            let spec = sampler_spec(params);
            // Chain order matches llama.cpp's conventional sampler pipeline:
            // truncate the distribution (top-p), then scale by temperature,
            // then sample (dist). top-k and repetition penalty are deliberately
            // omitted in this first cut; add them here if output quality needs
            // them.
            let mut sampler = LlamaSampler::chain_simple([
                LlamaSampler::top_p(spec.top_p, 1),
                LlamaSampler::temp(spec.temperature),
                LlamaSampler::dist(spec.seed),
            ]);

            // After chunked prefill, `pos` is the next absolute position and
            // `batch` holds the final chunk whose last token carries logits.
            let mut n_cur = pos;
            let mut accumulated = String::new();
            let mut decoder = encoding_rs::UTF_8.new_decoder();

            // Assume the budget will be exhausted (a `Length` truncation);
            // any clean exit (eos / matched stop sequence) flips this to
            // `Stop` before the loop breaks. If the `for` runs to its full
            // `max_tokens` count without a `break`, the assumption holds and
            // the reply was cut off by the token budget — exactly the signal
            // the dialogue manager's context-limit recovery keys off.
            let mut finish = FinishReason::Length;
            for _ in 0..params.max_tokens {
                let token = sampler.sample(&ctx, batch.n_tokens() - 1);
                sampler.accept(token);
                if token == self.model.token_eos() {
                    finish = FinishReason::Stop;
                    break;
                }
                let piece = self
                    .model
                    .token_to_piece(token, &mut decoder, false, None)
                    .map_err(|e| PrimerError::Inference(format!("detok: {e}").into()))?;
                accumulated.push_str(&piece);
                // Check for a stop sequence BEFORE forwarding so the matched
                // marker text is never shown to the child. Emit only the part
                // of this piece that precedes the marker, then stop.
                if let Some(visible) =
                    visible_prefix_before_stop(&piece, &accumulated, &params.stop_sequences)
                {
                    if !visible.is_empty() {
                        let _ = on_token(visible);
                    }
                    finish = FinishReason::Stop;
                    break;
                }
                if !on_token(&piece) {
                    // Consumer dropped the stream — not a truncation; no
                    // recovery is wanted.
                    return Ok(FinishReason::Stop);
                }
                batch.clear();
                batch
                    .add(token, n_cur, &[0], true)
                    .map_err(|e| PrimerError::Inference(format!("batch: {e}").into()))?;
                n_cur += 1;
                ctx.decode(&mut batch)
                    .map_err(|e| PrimerError::Inference(format!("decode: {e}").into()))?;
            }
            // A `max_tokens` of 0 never enters the loop and leaves `finish` at
            // its `Length` default; that degenerate config (no tokens
            // requested) is not a real truncation, so report `Stop`.
            if params.max_tokens == 0 {
                finish = FinishReason::Stop;
            }
            Ok(finish)
        }
    }
}

#[cfg(feature = "llamacpp")]
pub use real::RealLlamaEngine;
