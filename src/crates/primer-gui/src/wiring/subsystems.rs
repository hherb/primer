//! Construct the dialogue manager's background subsystems — engagement
//! classifier, concept extractor, comprehension classifier, embedder — and
//! the vocab settings, bundled as a
//! [`primer_pedagogy::DialogueManagerSubsystems`].
//!
//! Construction order (classifier → extractor → comprehension → embedder)
//! matches `primer-cli`, so the first failing subsystem is the one whose
//! error surfaces.

use std::sync::Arc;
use std::time::Duration;

use primer_classifier::ClassifierSettings;
use primer_comprehension::ComprehensionSettings;
use primer_core::inference::InferenceBackend;
use primer_engine::{
    BackendParams, build_classifier, build_comprehension, build_extractor,
    build_fastembed_embedder, build_ollama_embedder, build_openai_compat_embedder,
};
use primer_extractor::ExtractorSettings;
use primer_pedagogy::DialogueManagerSubsystems;
use primer_pedagogy::vocab::VocabSettings;

use crate::config::{EmbedderConfig, GuiConfig};

/// Build every subsystem the dialogue manager needs from the GUI config.
///
/// `backend` is the (possibly fallback/router-wrapped) main backend;
/// subsystems with neither a kind nor a model override reuse it via
/// `Arc::clone`.
/// `backend_kind` / `main_model` are the *served* kind and model id
/// (`primer_engine::served_identity`: the fallback's when the primary is
/// unavailable, #351; the model already rebound from any `*-pending`
/// placeholder), so inherited subsystems are labelled by what runs them and
/// overrides build on a kind that actually constructs. The openai-compat
/// embedder reuses the key the main backend resolved in `params`.
pub(super) async fn build_subsystems(
    config: &GuiConfig,
    backend: &Arc<dyn InferenceBackend>,
    backend_kind: &str,
    main_model: &str,
    params: &BackendParams,
) -> Result<DialogueManagerSubsystems, String> {
    let classifier_settings = ClassifierSettings {
        blocking_timeout: Duration::from_millis(config.classifier.timeout_ms),
        ..ClassifierSettings::default()
    };
    let classifier = build_classifier(
        Arc::clone(backend),
        backend_kind,
        main_model,
        params,
        classifier_settings.clone(),
    )
    .await
    .map_err(|e| format!("constructing engagement classifier: {e}"))?;

    let extractor_settings = ExtractorSettings {
        blocking_timeout: Duration::from_millis(config.extractor.timeout_ms),
        ..ExtractorSettings::default()
    };
    let extractor = build_extractor(
        Arc::clone(backend),
        backend_kind,
        main_model,
        params,
        extractor_settings.clone(),
    )
    .await
    .map_err(|e| format!("constructing concept extractor: {e}"))?;

    let comprehension_settings = ComprehensionSettings {
        blocking_timeout: Duration::from_millis(config.comprehension.timeout_ms),
        ..ComprehensionSettings::default()
    };
    let comprehension = build_comprehension(
        Arc::clone(backend),
        backend_kind,
        main_model,
        params,
        comprehension_settings.clone(),
    )
    .await
    .map_err(|e| format!("constructing comprehension classifier: {e}"))?;

    let embedder = build_embedder(
        &config.embedder,
        &config.backend.openai_compat_url,
        params.openai_compat_api_key.clone(),
    )
    .await?;

    let vocab_settings = VocabSettings {
        max_per_prompt: config
            .vocab
            .max_per_prompt
            .map(|n| n as usize)
            .unwrap_or(primer_core::consts::vocab::DEFAULT_VOCAB_MAX_PER_PROMPT),
    };

    Ok(DialogueManagerSubsystems {
        classifier,
        classifier_settings,
        extractor,
        extractor_settings,
        comprehension,
        comprehension_settings,
        vocab_settings,
        embedder,
    })
}

/// The UI notice for a configured embedder that failed to start, which
/// leaves retrieval BM25-only (issue #353), or that started but failed to
/// embed the knowledge base (`embed_error`, #388). `None` when `kind` is
/// `"none"` (BM25-only by choice) or everything worked. English-only and
/// operator-facing, like [`primer_engine::BackendNotice`].
pub(super) fn retrieval_notice(
    kind: &str,
    built: bool,
    embed_error: Option<&str>,
) -> Option<String> {
    if kind != "none" && !built {
        return Some(format!(
            "Knowledge search is keyword-only (BM25): the '{kind}' embedder could not \
             start (model download or embedding server unavailable)."
        ));
    }
    embed_error.map(|e| {
        format!(
            "Knowledge search is partly keyword-only (BM25): embedding the knowledge \
             base with the '{kind}' embedder failed ({e}); it retries next session."
        )
    })
}

/// Embedder construction, mirroring `primer-cli`'s dispatch matrix.
///
/// `fastembed` / `ollama` / `openai-compat` require their respective cargo
/// features (`embedding`, `ollama-embedding`, `openai-compat-embedding`); the
/// `primer-engine` helpers return an `Err(String)` when the feature is
/// missing so this command path can surface it to the frontend instead
/// of killing the GUI process (the CLI maps the same Err to a clean
/// stderr line + exit).
async fn build_embedder(
    config: &EmbedderConfig,
    main_openai_compat_url: &str,
    openai_compat_api_key: Option<String>,
) -> Result<Option<Arc<dyn primer_core::embedder::Embedder>>, String> {
    match config.kind.as_str() {
        "none" => Ok(None),
        "stub" => Ok(Some(Arc::new(primer_embedding::StubEmbedder::new()) as _)),
        "fastembed" => build_fastembed_embedder(config.model.as_deref()),
        "ollama" => {
            build_ollama_embedder(config.ollama_url.as_deref(), config.model.as_deref()).await
        }
        "openai-compat" => {
            // The embedder URL falls back to the main backend's
            // openai-compat URL when no embedder-specific override is set
            // (mirrors the CLI's `--embedder-openai-compat-url` →
            // `--openai-compat-url` fallback). The API key is the same
            // resolved value the main backend uses.
            let url = config
                .openai_compat_url
                .as_deref()
                .or(Some(main_openai_compat_url));
            build_openai_compat_embedder(url, config.model.as_deref(), openai_compat_api_key).await
        }
        other => Err(format!(
            "unknown embedder backend {other:?}: expected one of none, stub, fastembed, ollama, openai-compat"
        )),
    }
}
