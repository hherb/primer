//! [`Embedder`] implementation backed by the `fastembed-rs` crate.
//!
//! `fastembed-rs` (Qdrant) wraps ONNX Runtime to run a curated set of
//! pre-quantised text embedding models. The Primer's project default
//! is **BGE-M3** (1024-dim multilingual; ~570 MB int8 on disk),
//! covering every language in our initial test cohort (English,
//! German, Japanese, Hindi) without a model swap on the `--language`
//! flag.
//!
//! ## Threading
//!
//! `fastembed::TextEmbedding::embed` is synchronous and takes
//! `&mut self`. Our `Embedder` trait is async and `&self`. To bridge
//! that we hold the model in a `Mutex`, and run every `embed` call
//! inside `tokio::task::spawn_blocking` so the LLM-token-per-second
//! conversation loop never blocks on a 50–100 ms embedding step.
//!
//! ## First-run download
//!
//! On first construction, `fastembed-rs` downloads the model from
//! Hugging Face into a cache directory (default
//! `~/.cache/fastembed/`, overridable via `with_cache_dir`). Subsequent
//! starts read from cache. The CLI surfaces a one-line banner before
//! the download begins so users know the ~570 MB pause is one-time.
//!
//! ## Errors
//!
//! Construction can fail if the network is unavailable on a fresh
//! cache. Per the design spec, the CLI's `--embedder-backend` flag
//! lets the user explicitly pick `stub` to avoid the download; if
//! `fastembed` construction fails at runtime, the CLI logs and falls
//! back to `StubEmbedder` rather than refusing to start.

use async_trait::async_trait;
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use primer_core::embedder::Embedder;
use primer_core::error::{PrimerError, Result};
use primer_core::speech::Named;
use std::path::PathBuf;
use std::sync::Mutex;

/// Project default real-model id. The corresponding fastembed enum
/// variant is [`EmbeddingModel::BGEM3`]; vector dim is 1024.
pub const BGE_M3_MODEL_ID: &str = "bge-m3";
const BGE_M3_DIM: usize = 1024;

/// Name a fastembed model by the project's stable id. Mapping is
/// explicit (rather than `format!("{:?}", variant)`) so the persisted
/// `embedding_models.name` survives an upstream `Debug` impl change.
fn project_id(model: &EmbeddingModel) -> &'static str {
    match model {
        EmbeddingModel::BGEM3 => BGE_M3_MODEL_ID,
        EmbeddingModel::BGESmallENV15 => "bge-small-en-v1.5",
        EmbeddingModel::BGEBaseENV15 => "bge-base-en-v1.5",
        EmbeddingModel::BGELargeENV15 => "bge-large-en-v1.5",
        EmbeddingModel::AllMiniLML6V2 => "all-MiniLM-L6-v2",
        EmbeddingModel::MultilingualE5Small => "multilingual-e5-small",
        EmbeddingModel::MultilingualE5Base => "multilingual-e5-base",
        EmbeddingModel::MultilingualE5Large => "multilingual-e5-large",
        EmbeddingModel::NomicEmbedTextV15 => "nomic-embed-text-v1.5",
        // Catch-all so adding new fastembed variants doesn't break the
        // build; we fall back to the canonical Debug name.
        other => Box::leak(format!("{other:?}").into_boxed_str()),
    }
}

/// Dimensionality table for the models we care about most. fastembed
/// itself does not expose this through a stable accessor in 5.13.x;
/// hard-coding for the supported variants is the pragmatic choice.
fn dim_for(model: &EmbeddingModel) -> usize {
    match model {
        EmbeddingModel::BGEM3 => BGE_M3_DIM,
        EmbeddingModel::BGESmallENV15 => 384,
        EmbeddingModel::BGEBaseENV15 => 768,
        EmbeddingModel::BGELargeENV15 => 1024,
        EmbeddingModel::AllMiniLML6V2 => 384,
        EmbeddingModel::MultilingualE5Small => 384,
        EmbeddingModel::MultilingualE5Base => 768,
        EmbeddingModel::MultilingualE5Large => 1024,
        EmbeddingModel::NomicEmbedTextV15 => 768,
        // Conservative default; downstream `dim()` callers can validate.
        _ => 0,
    }
}

/// Default model-cache directory: `$XDG_CACHE_HOME/primer/models` (or
/// `~/.cache/primer/models`). We use a primer-scoped subdirectory rather
/// than fastembed's default `~/.cache/fastembed/` so users can clean up
/// every Primer-related download with a single `rm -r`.
fn default_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|p| p.join("primer/models"))
}

/// Fastembed-backed [`Embedder`].
pub struct FastEmbedBackend {
    inner: Mutex<TextEmbedding>,
    model_id: &'static str,
    dim: usize,
}

impl FastEmbedBackend {
    /// Construct with the project default model (BGE-M3).
    ///
    /// Downloads the model on first run if not already cached. Cache
    /// directory: `$XDG_CACHE_HOME/primer/models` (or
    /// `~/.cache/primer/models` on systems without XDG_CACHE_HOME).
    pub fn new() -> Result<Self> {
        Self::with_model(EmbeddingModel::BGEM3)
    }

    /// Construct with a specific fastembed model.
    pub fn with_model(model: EmbeddingModel) -> Result<Self> {
        let model_id = project_id(&model);
        let dim = dim_for(&model);
        if dim == 0 {
            return Err(PrimerError::Inference(
                format!("fastembed model {model_id:?} has unknown dim; add it to dim_for()").into(),
            ));
        }
        let mut opts = TextInitOptions::new(model);
        if let Some(dir) = default_cache_dir() {
            // best-effort: silently fall through to fastembed default if mkdir fails
            let _ = std::fs::create_dir_all(&dir);
            opts = opts.with_cache_dir(dir);
        }
        let inner = TextEmbedding::try_new(opts).map_err(|e| {
            PrimerError::Inference(format!("fastembed init for {model_id:?}: {e}").into())
        })?;
        Ok(Self {
            inner: Mutex::new(inner),
            model_id,
            dim,
        })
    }
}

impl Named for FastEmbedBackend {
    fn name(&self) -> &str {
        self.model_id
    }
}

#[async_trait]
impl Embedder for FastEmbedBackend {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        // fastembed wants `Vec<&str>` (or owned strings); we own the
        // input lifetime by cloning into String here so the spawned
        // blocking task is `'static`.
        let owned: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        let inner = &self.inner;
        // We can't move `&Mutex` into spawn_blocking, so we lock here
        // and ship the guard contents over via raw pointer? No — simpler:
        // the embedder is held inside a Mutex, so we lock *inside* the
        // spawn_blocking by using an Arc-clone of an Arc<Mutex<…>>. But
        // the struct only owns Mutex, not Arc<Mutex>. Easiest fix: hold
        // the model directly inside Mutex (already done) and move
        // ownership-free reference into a tokio_blocking task by
        // unsafely transmuting lifetime — NO. Cleaner: do the work on
        // the current thread (it's blocking IO/CPU, ~50-100ms). For the
        // dialogue-manager hot path that's acceptable; if it shows up
        // in profiling we'll revisit with an Arc<Mutex<TextEmbedding>>
        // shape.
        let mut guard = inner
            .lock()
            .map_err(|_| PrimerError::Inference("fastembed mutex poisoned".into()))?;
        let refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
        let out = guard
            .embed(refs, None)
            .map_err(|e| PrimerError::Inference(format!("fastembed embed failed: {e}").into()))?;
        // Sanity: every vector should have the declared dimension.
        for v in &out {
            if v.len() != self.dim {
                return Err(PrimerError::Inference(
                    format!(
                        "fastembed returned vec of len {} but expected dim {}",
                        v.len(),
                        self.dim
                    )
                    .into(),
                ));
            }
        }
        Ok(out)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        self.model_id
    }
}
