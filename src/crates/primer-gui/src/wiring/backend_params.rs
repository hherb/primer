//! Translate the GUI's `BackendConfig` into `primer-engine`'s
//! [`BackendParams`] input struct, plus the small pure resolvers that feed
//! it (main-model defaults, cloud-key gating, subsystem override kinds).
//!
//! The only side effect here is reading the `ANTHROPIC_API_KEY` /
//! `OPENAI_COMPAT_API_KEY` env vars when the corresponding key source is
//! `Env` — the wiring crate's helpers never touch env vars themselves.

use primer_engine::BackendParams;

use crate::config::{ApiKeySource, GuiConfig, SubsystemConfig};

/// Assemble the [`BackendParams`] every `primer-engine` builder consumes.
///
/// The cloud `api_key` resolves Env vs Inline here so the wiring
/// crate doesn't need to touch env vars on every helper call. The
/// `ANTHROPIC_API_KEY` env var is consulted whenever the cloud backend
/// is reachable — as the primary OR as the opt-in fallback secondary
/// (issue #205 follow-up). A local-primary → cloud-fallback setup (the
/// supported fallback direction) needs the key even though the primary
/// `kind` is not "cloud"; without this the cloud secondary fails to
/// build with an Auth error and the fallback silently degrades to
/// PrimaryAlone. An Inline key is kind-agnostic (it may be entered for
/// a cloud fallback even when the primary is local).
///
/// The resolved openai-compat key lands in
/// `BackendParams::openai_compat_api_key`; `build_subsystems` reads that
/// same value back from `params` for the openai-compat embedder.
/// Read an API-key env var. Unset → `None`; a non-Unicode value is also
/// unusable but is logged rather than silently treated as unset (#337).
fn env_key(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(v) => Some(v),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            tracing::warn!("{name} is set but not valid UTF-8; ignoring it");
            None
        }
    }
}

pub(super) fn build_backend_params(config: &GuiConfig) -> BackendParams {
    let backend_config = &config.backend;

    let api_key = match &backend_config.api_key_source {
        ApiKeySource::Inline { key } => Some(key.clone()),
        ApiKeySource::Env
            if cloud_backend_in_use(
                &backend_config.kind,
                backend_config.fallback_backend.as_deref(),
            ) =>
        {
            env_key("ANTHROPIC_API_KEY")
        }
        ApiKeySource::Env => None,
    };

    // OpenAI-compat key resolves independently of the cloud key. `Env`
    // reads `OPENAI_COMPAT_API_KEY` (the CLI's env-var name); the same
    // resolved value feeds both the main backend (when kind ==
    // "openai-compat") and the openai-compat embedder, mirroring the
    // CLI's reuse of `--openai-compat-api-key` across both.
    let openai_compat_api_key = match &backend_config.openai_compat_api_key_source {
        ApiKeySource::Inline { key } => Some(key.clone()),
        ApiKeySource::Env => env_key("OPENAI_COMPAT_API_KEY"),
    };

    BackendParams {
        api_key,
        ollama_url: backend_config.ollama_url.clone(),
        openai_compat_url: backend_config.openai_compat_url.clone(),
        openai_compat_api_key,
        classifier_backend: subsystem_kind(&config.classifier),
        classifier_model: config.classifier.model.clone(),
        extractor_backend: subsystem_kind(&config.extractor),
        extractor_model: config.extractor.model.clone(),
        comprehension_backend: subsystem_kind(&config.comprehension),
        comprehension_model: config.comprehension.model.clone(),
        // QNN backend (Phase 1.2 step 1.2.4): the bundle / QAIRT-lib
        // paths come straight from Settings → Inference backend. On a
        // default (non-`qnn`-feature) GUI build, `build_qnn_backend`'s
        // `not(feature = "qnn")` arm still returns the "rebuild with
        // --features qnn" hint regardless of these values, so selecting
        // qnn surfaces a clear build-time error inline rather than
        // killing the GUI (mirrors the openai-compat-embedder pattern).
        qnn_bundle_dir: backend_config.qnn_bundle_dir.clone(),
        qnn_qairt_lib_dir: backend_config.qnn_qairt_lib_dir.clone(),
        // llama.cpp backend (Phase 1.1): the GGUF path / gpu-layers /
        // n_ctx come straight from Settings → Inference backend. The GUI
        // carries a dedicated `gguf_path` field (unlike the CLI, which
        // reuses `--model`). On a default (non-`llamacpp`-feature) build,
        // `build_llamacpp_backend`'s `not(feature = "llamacpp")` arm
        // returns the "rebuild with --features llamacpp" hint regardless
        // of these values, so selecting llamacpp surfaces a clear
        // build-time error inline rather than killing the GUI (mirrors
        // the qnn pattern).
        gguf_path: backend_config.gguf_path.clone(),
        llamacpp_gpu_layers: backend_config.llamacpp_gpu_layers,
        llamacpp_n_ctx: backend_config.llamacpp_n_ctx,
        // Custom reasoning markers from Settings → Inference backend.
        // Parsed from the raw textarea text into `(open, close)` pairs and
        // appended to the built-in defaults by the ollama / openai-compat
        // backends. Empty string ⇒ empty Vec ⇒ defaults only.
        reasoning_markers: crate::reasoning_markers::parse_reasoning_markers(
            &backend_config.reasoning_markers,
        ),
        // Opt-in local→cloud fallback (issue #205): the fallback backend /
        // model come straight from Settings → Inference backend. `None` ⇒ no
        // fallback ⇒ today's single-backend behavior (the privacy default —
        // a local-only setup never silently reaches the cloud). When set,
        // `build_main_backend` wraps the primary in a `FallbackBackend`
        // decorator that serves the turn from the secondary if the primary is
        // unavailable at startup or fails *before any token streams*. Mirrors
        // the CLI's `--fallback-backend` / `--fallback-model`.
        fallback_backend: backend_config.fallback_backend.clone(),
        fallback_model: backend_config.fallback_model.clone(),
        // Phase 1.3 inference-router mode from Settings → Inference backend.
        // `RouterMode` is `Copy`, so no `.clone()`. `LocalOnly` (the default)
        // ⇒ today's single-backend behaviour; the other modes route via the
        // configured fallback secondary in `build_main_backend`. Mirrors the
        // CLI's `--router-mode`.
        router_mode: backend_config.router_mode,
        // Phase 1.3 latency-aware routing budget from Settings → Inference
        // backend. `None` ⇒ latency routing OFF. Mirrors the CLI's
        // `--primary-ttft-budget-ms`.
        primary_ttft_budget_ms: backend_config.primary_ttft_budget_ms,
    }
}

/// Is the Anthropic cloud backend reachable in this config — either as the
/// primary backend or as the opt-in fallback secondary? Determines whether
/// the `ANTHROPIC_API_KEY` env var is worth resolving for `Env` key mode.
/// A local-primary → cloud-fallback setup (the supported fallback direction,
/// issue #205) needs the key even though the primary `kind` is not `"cloud"`.
pub(super) fn cloud_backend_in_use(primary_kind: &str, fallback_backend: Option<&str>) -> bool {
    primary_kind == "cloud" || fallback_backend == Some("cloud")
}

/// Resolve the main backend's model id from the GUI's optional override.
/// Mirrors `primer-cli`: cloud defaults to
/// [`DEFAULT_CLOUD_MODEL`](primer_core::consts::inference::DEFAULT_CLOUD_MODEL),
/// ollama and openai-compat require an explicit value, stub falls back to
/// the literal "stub", and qnn / llamacpp return a `*-pending` placeholder
/// that `build_with_strategy` rebinds to `backend.name()`.
pub(super) fn resolve_main_model(kind: &str, model: Option<&str>) -> Result<String, String> {
    match kind {
        "cloud" => Ok(model
            .map(String::from)
            .unwrap_or_else(|| primer_core::consts::inference::DEFAULT_CLOUD_MODEL.to_string())),
        "ollama" => model.map(String::from).ok_or_else(|| {
            "ollama backend requires a model name (e.g. \"llama3.2\") in settings".to_string()
        }),
        "openai-compat" => model.map(String::from).ok_or_else(|| {
            "openai-compat backend requires a model name (the server's model id) in settings"
                .to_string()
        }),
        "stub" => Ok(model
            .map(String::from)
            .unwrap_or_else(|| "stub".to_string())),
        "qnn" => {
            // The model id is read from the bundle's `primer-meta.json`
            // at construction time, so the `model` override is ignored.
            // Return a placeholder that `build_with_strategy` rebinds to
            // `backend.name()` once the QNN backend constructs (mirrors
            // the CLI's "qnn-pending" placeholder).
            Ok("qnn-pending".to_string())
        }
        "llamacpp" => {
            // The real model id comes from the GGUF stem at construction
            // time (`backend.name()` == "llamacpp:<stem>"). The `model`
            // override is ignored — the GGUF path lives in its own config
            // field. Return a placeholder that `build_with_strategy`
            // rebinds to `backend.name()` once the backend constructs
            // (mirrors the CLI's "llamacpp" rebind).
            Ok("llamacpp-pending".to_string())
        }
        other => Err(format!(
            "unknown backend kind {other:?}: expected one of stub, cloud, ollama, openai-compat, qnn, llamacpp"
        )),
    }
}

/// For a subsystem set to "match the main backend" we leave the *kind*
/// override empty. Only an explicit override produces a non-None kind
/// here. Note the `*_model` fields are still copied through regardless
/// of `match_main` (issue #335), so `primer-engine` reuses the main
/// backend via `Arc::clone` only when the model override is also unset.
fn subsystem_kind(s: &SubsystemConfig) -> Option<String> {
    if s.match_main { None } else { s.kind.clone() }
}
