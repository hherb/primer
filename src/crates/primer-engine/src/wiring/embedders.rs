//! `Embedder` construction for the three supported embedder backends
//! (fastembed / ollama / openai-compat). Each is a feature-gated pair:
//! the feature-on arm constructs the real backend and returns
//! `Ok(Some(..))` / `Ok(None)` on init failure (caller degrades to
//! BM25-only); the feature-off arm returns `Err(build-hint)`.
//!
//! Split out of the flat `wiring` module (behaviour-preserving). These
//! builders take their url/model/api-key directly (no `BackendParams`
//! dependency), so the file is self-contained.

use std::sync::Arc;

/// Construct a fastembed-rs-backed `Embedder`.
///
/// Return-value semantics:
/// - `Ok(Some(arc))` — feature compiled in AND init succeeded.
/// - `Ok(None)` — feature compiled in but init failed (e.g. model
///   download failed). Caller falls back to BM25-only retrieval; a
///   warning is emitted to stderr so a CLI user sees it.
/// - `Err(msg)` — feature not compiled in. The user explicitly asked
///   for fastembed; surfacing this as an error lets the binary decide
///   whether to exit (CLI) or render it inline (GUI). Earlier
///   versions called `std::process::exit(1)` here which is hostile to
///   any caller that isn't a CLI — never re-introduce that.
#[cfg(feature = "embedding")]
pub fn build_fastembed_embedder(
    model: Option<&str>,
) -> std::result::Result<Option<Arc<dyn primer_core::embedder::Embedder>>, String> {
    use primer_embedding::FastEmbedBackend;
    let m = model.unwrap_or(primer_embedding::BGE_M3_MODEL_ID);
    if m != primer_embedding::BGE_M3_MODEL_ID {
        eprintln!(
            "Note: --embedder-model {m} not yet supported by the CLI dispatch; using bge-m3."
        );
    }
    eprintln!(
        "Loading fastembed model {m}; first run downloads ~570 MB into ~/.cache/primer/models/."
    );
    match FastEmbedBackend::new() {
        Ok(b) => Ok(Some(Arc::new(b) as _)),
        Err(e) => {
            eprintln!("fastembed init failed ({e}); falling back to BM25-only retrieval.");
            Ok(None)
        }
    }
}

#[cfg(not(feature = "embedding"))]
pub fn build_fastembed_embedder(
    _model: Option<&str>,
) -> std::result::Result<Option<Arc<dyn primer_core::embedder::Embedder>>, String> {
    Err(
        "fastembed embedder requires the `embedding` cargo feature. \
         Build with `cargo run --features primer-cli/embedding -- ...` (or use embedder = none)."
            .to_string(),
    )
}

#[cfg(feature = "ollama-embedding")]
pub async fn build_ollama_embedder(
    url: Option<&str>,
    model: Option<&str>,
) -> std::result::Result<Option<Arc<dyn primer_core::embedder::Embedder>>, String> {
    use primer_embedding::{DEFAULT_OLLAMA_MODEL, DEFAULT_OLLAMA_URL, OllamaEmbedder};
    let url = url.unwrap_or(DEFAULT_OLLAMA_URL);
    let model = model.unwrap_or(DEFAULT_OLLAMA_MODEL);
    match OllamaEmbedder::with_endpoint(url, model).await {
        Ok(b) => {
            eprintln!("Embedder: ollama {model} at {url}");
            Ok(Some(Arc::new(b) as _))
        }
        Err(e) => {
            eprintln!("ollama embedder init failed ({e}); falling back to BM25-only retrieval.");
            Ok(None)
        }
    }
}

#[cfg(not(feature = "ollama-embedding"))]
pub async fn build_ollama_embedder(
    _url: Option<&str>,
    _model: Option<&str>,
) -> std::result::Result<Option<Arc<dyn primer_core::embedder::Embedder>>, String> {
    Err(
        "ollama embedder requires the `ollama-embedding` cargo feature. \
         Build with `cargo run --features primer-cli/ollama-embedding -- ...`"
            .to_string(),
    )
}

#[cfg(feature = "openai-compat-embedding")]
pub async fn build_openai_compat_embedder(
    url: Option<&str>,
    model: Option<&str>,
    api_key: Option<String>,
) -> std::result::Result<Option<Arc<dyn primer_core::embedder::Embedder>>, String> {
    use primer_embedding::{DEFAULT_OPENAI_COMPAT_URL, OpenAiCompatEmbedder};
    let url = url.unwrap_or(DEFAULT_OPENAI_COMPAT_URL);
    let model = model.ok_or_else(|| {
        "--embedder-openai-compat-model is required when --embedder-backend is openai-compat"
            .to_string()
    })?;
    match OpenAiCompatEmbedder::new(url, model, api_key).await {
        Ok(b) => {
            eprintln!("Embedder: openai-compat {model} at {url}");
            Ok(Some(Arc::new(b) as _))
        }
        Err(e) => {
            eprintln!(
                "openai-compat embedder init failed ({e}); falling back to BM25-only retrieval."
            );
            Ok(None)
        }
    }
}

#[cfg(not(feature = "openai-compat-embedding"))]
pub async fn build_openai_compat_embedder(
    _url: Option<&str>,
    _model: Option<&str>,
    _api_key: Option<String>,
) -> std::result::Result<Option<Arc<dyn primer_core::embedder::Embedder>>, String> {
    Err(
        "openai-compat embedder requires the `openai-compat-embedding` cargo feature. \
         Build with `cargo run --features primer-cli/openai-compat-embedding -- ...`"
            .to_string(),
    )
}
