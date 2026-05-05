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
use std::sync::{Arc, Mutex};

/// Project default real-model id. The corresponding fastembed enum
/// variant is [`EmbeddingModel::BGEM3`]; vector dim is 1024.
pub const BGE_M3_MODEL_ID: &str = "bge-m3";
const BGE_M3_DIM: usize = 1024;

/// Name + dim for every fastembed variant we explicitly support.
/// Returns `None` for unsupported variants so callers can produce a
/// clean error rather than constructing an unnamed/unknown-dim
/// embedder. Mapping is explicit (rather than `format!("{:?}", v)`)
/// so the persisted `embedding_models.name` survives an upstream
/// `Debug` impl change.
fn supported(model: &EmbeddingModel) -> Option<(&'static str, usize)> {
    Some(match model {
        EmbeddingModel::BGEM3 => (BGE_M3_MODEL_ID, BGE_M3_DIM),
        EmbeddingModel::BGESmallENV15 => ("bge-small-en-v1.5", 384),
        EmbeddingModel::BGEBaseENV15 => ("bge-base-en-v1.5", 768),
        EmbeddingModel::BGELargeENV15 => ("bge-large-en-v1.5", 1024),
        EmbeddingModel::AllMiniLML6V2 => ("all-MiniLM-L6-v2", 384),
        EmbeddingModel::MultilingualE5Small => ("multilingual-e5-small", 384),
        EmbeddingModel::MultilingualE5Base => ("multilingual-e5-base", 768),
        EmbeddingModel::MultilingualE5Large => ("multilingual-e5-large", 1024),
        EmbeddingModel::NomicEmbedTextV15 => ("nomic-embed-text-v1.5", 768),
        _ => return None,
    })
}

/// Default model-cache directory: `$XDG_CACHE_HOME/primer/models` (or
/// `~/.cache/primer/models`). We use a primer-scoped subdirectory rather
/// than fastembed's default `~/.cache/fastembed/` so users can clean up
/// every Primer-related download with a single `rm -r`.
fn default_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|p| p.join("primer/models"))
}

/// Fastembed-backed [`Embedder`]. The model is held inside an
/// `Arc<Mutex<…>>` so each `embed()` call can clone the Arc into a
/// `tokio::task::spawn_blocking` worker — keeping the synchronous
/// 50-100 ms ONNX call off the conversation hot path.
pub struct FastEmbedBackend {
    inner: Arc<Mutex<TextEmbedding>>,
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
        let (model_id, dim) = supported(&model).ok_or_else(|| {
            PrimerError::Inference(
                format!(
                    "fastembed model {model:?} is not in primer's supported list; \
                     add it to fastembed_backend::supported() with its stable id and dim"
                )
                .into(),
            )
        })?;
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
            inner: Arc::new(Mutex::new(inner)),
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
        // Move owned strings + an Arc-clone into spawn_blocking so the
        // tokio runtime can schedule the synchronous embed off-thread.
        let owned: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        let inner = Arc::clone(&self.inner);
        let dim = self.dim;
        let out = tokio::task::spawn_blocking(move || -> Result<Vec<Vec<f32>>> {
            let mut guard = inner
                .lock()
                .map_err(|_| PrimerError::Inference("fastembed mutex poisoned".into()))?;
            let refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
            let vecs = guard.embed(refs, None).map_err(|e| {
                PrimerError::Inference(format!("fastembed embed failed: {e}").into())
            })?;
            for v in &vecs {
                if v.len() != dim {
                    return Err(PrimerError::Inference(
                        format!(
                            "fastembed returned vec of len {} but expected dim {dim}",
                            v.len()
                        )
                        .into(),
                    ));
                }
            }
            Ok(vecs)
        })
        .await
        .map_err(|e| PrimerError::Inference(format!("fastembed task join: {e}").into()))??;
        Ok(out)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        self.model_id
    }
}
