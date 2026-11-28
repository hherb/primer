//! Startup step 1: build the main inference backend (with its opt-in
//! fallback or router) and print the backend banner. Moved verbatim out
//! of `async_main`.

use std::sync::Arc;

use primer_core::inference::InferenceBackend;
use primer_engine::{BackendNotice, BackendParams, build_main_backend};

use crate::Cli;
use crate::cli_support::{pair_reasoning_markers, warn_on_npu_serialisation};

/// What [`build_inference`] hands back to `async_main`.
pub(crate) struct Inference {
    pub(crate) backend: Arc<dyn InferenceBackend>,
    /// A degraded fallback / router build, already printed as a warning.
    pub(crate) notice: Option<BackendNotice>,
    /// The model the primary was built with (for qnn and llamacpp, the
    /// constructed backend's `name()`).
    pub(crate) main_model: String,
    /// Reused by the subsystem builders.
    pub(crate) backend_params: BackendParams,
}

/// Build the main backend from the flags. Exits the process with a
/// message on a flag error or a failed construction, as before the split.
pub(crate) async fn build_inference(cli: &Cli) -> Inference {
    // ─── Create backends ─────────────────────────────────────────────

    // Resolve the model for the main backend early so we can report it
    // in the banner AND pass the resolved value to build_classifier later.
    // For `qnn` the model id is read from `primer-meta.json` inside the
    // bundle and isn't known until after construction — we seed an
    // "unknown" placeholder here and overwrite from `backend.name()`
    // after `build_backend` returns. `cli.model` is intentionally
    // ignored for qnn (documented on the flag).
    let main_model: String = match cli.backend.as_str() {
        "cloud" => cli
            .model
            .clone()
            .unwrap_or_else(|| primer_core::consts::inference::DEFAULT_CLOUD_MODEL.to_string()),
        "deepseek" => cli
            .model
            .clone()
            .unwrap_or_else(|| primer_core::consts::inference::DEFAULT_DEEPSEEK_MODEL.to_string()),
        "ollama" => cli.model.clone().unwrap_or_else(|| {
            eprintln!("Error: --model required for ollama backend (e.g., --model llama3.2).");
            std::process::exit(1);
        }),
        "openai-compat" => cli.model.clone().unwrap_or_else(|| {
            eprintln!("Error: --model required for openai-compat backend.");
            std::process::exit(1);
        }),
        // For llamacpp `--model` is the GGUF path (required). The string
        // is unused by the llamacpp arm of `build_backend` (which reads
        // `gguf_path`), but seed it with the path so the pre-construction
        // value is a valid non-empty string; it's rebound to
        // `backend.name()` (= "llamacpp:<stem>") after construction.
        "llamacpp" => cli.model.clone().unwrap_or_else(|| {
            eprintln!(
                "Error: --model required for llamacpp backend (filesystem path to the .gguf file)."
            );
            std::process::exit(1);
        }),
        "qnn" => {
            // Surface a one-line note when the user passed `--model`
            // alongside `--backend qnn`. Documented behaviour is that
            // the flag is ignored (the bundle's `primer-meta.json` is
            // authoritative), but silent ignore is a UX hazard — a
            // user who explicitly typed `--model claude-opus-4-7`
            // would otherwise see `qnn:Qwen3-4B` in the banner with no
            // explanation. Emitted unconditionally so it's visible
            // even without `--verbose`.
            if cli.model.is_some() {
                eprintln!(
                    "Note: --model is ignored under --backend qnn; \
                     the model id comes from primer-meta.json inside the bundle."
                );
            }
            "qnn-pending".to_string()
        }
        // stub (and anything else — will error in build_backend below)
        _ => cli.model.clone().unwrap_or_else(|| "stub".to_string()),
    };

    // Parse and validate --router-mode before touching BackendParams.
    let router_mode: primer_core::router::RouterMode = match cli.router_mode.parse() {
        Ok(m) => m,
        Err(msg) => {
            eprintln!("error: {msg}");
            std::process::exit(2);
        }
    };
    if router_mode.uses_secondary() && cli.fallback_backend.is_none() {
        eprintln!(
            "error: --router-mode {router_mode} requires --fallback-backend (the secondary leg to route to)"
        );
        std::process::exit(2);
    }

    // Extract the fields that build_backend / build_classifier need BEFORE
    // any partial moves of other Cli fields (knowledge_db, session_db etc.).
    //
    // The two QNN fields are populated only when the `qnn` cargo
    // feature is on (the flag declarations themselves are
    // `#[cfg(feature = "qnn")]`-gated). On the default build the
    // fields stay `None`, which is exactly what
    // `wiring::build_qnn_backend`'s `not(feature = "qnn")` arm expects
    // — it returns a rebuild hint regardless of input.
    let backend_params = BackendParams {
        api_key: cli.api_key.clone(),
        ollama_url: cli.ollama_url.clone(),
        openai_compat_url: cli.openai_compat_url.clone(),
        openai_compat_api_key: cli.openai_compat_api_key.clone(),
        deepseek_api_key: cli.deepseek_api_key.clone(),
        classifier_backend: cli.classifier_backend.clone(),
        classifier_model: cli.classifier_model.clone(),
        extractor_backend: cli.extractor_backend.clone(),
        extractor_model: cli.extractor_model.clone(),
        comprehension_backend: cli.comprehension_backend.clone(),
        comprehension_model: cli.comprehension_model.clone(),
        #[cfg(feature = "qnn")]
        qnn_bundle_dir: cli.qnn_bundle_dir.clone(),
        #[cfg(not(feature = "qnn"))]
        qnn_bundle_dir: None,
        #[cfg(feature = "qnn")]
        qnn_qairt_lib_dir: cli.qnn_qairt_lib_dir.clone(),
        #[cfg(not(feature = "qnn"))]
        qnn_qairt_lib_dir: None,
        // `gguf_path` is sourced from the PRIMARY `--model` only when the
        // primary is llamacpp. A llamacpp *fallback* (`--fallback-backend
        // llamacpp`) would therefore have no GGUF path and fail to build —
        // intentional: the supported direction is local-primary → cloud-
        // fallback, not the reverse.
        gguf_path: if cli.backend == "llamacpp" {
            cli.model.clone().map(std::path::PathBuf::from)
        } else {
            None
        },
        // The `--llamacpp-*` flags are `#[cfg(feature = "llamacpp")]`-gated
        // declarations (like the qnn flags above), so the fields only exist
        // on a llamacpp build. On the default build they stay `None`, which
        // is what `build_llamacpp_backend`'s `not(feature)` stub expects.
        #[cfg(feature = "llamacpp")]
        llamacpp_gpu_layers: cli.llamacpp_gpu_layers,
        #[cfg(not(feature = "llamacpp"))]
        llamacpp_gpu_layers: None,
        #[cfg(feature = "llamacpp")]
        llamacpp_n_ctx: cli.llamacpp_n_ctx,
        #[cfg(not(feature = "llamacpp"))]
        llamacpp_n_ctx: None,
        reasoning_markers: pair_reasoning_markers(cli.reasoning_marker.clone()),
        fallback_backend: cli.fallback_backend.clone(),
        fallback_model: cli.fallback_model.clone(),
        // Phase 1.3 inference router; parsed + validated above.
        router_mode,
        primary_ttft_budget_ms: cli.primary_ttft_budget_ms,
    };

    let (backend, notice): (Arc<dyn InferenceBackend>, _) =
        match build_main_backend(&cli.backend, main_model.clone(), &backend_params).await {
            Ok((b, notice)) => {
                // A dropped fallback leg, or a primary replaced by the fallback (#347, #338).
                if let Some(notice) = &notice {
                    eprintln!("Warning: {notice}");
                }
                (b, notice)
            }
            Err(e) => {
                eprintln!("Error constructing backend: {e}");
                std::process::exit(1);
            }
        };

    // For QNN the real model id comes from `primer-meta.json`; rebind
    // `main_model` to `backend.name()` (e.g. "qnn:Qwen3-4B") so the
    // downstream classifier/extractor/comprehension identifiers carry
    // the real model id instead of the "qnn-pending" placeholder.
    let main_model: String = if cli.backend == "qnn" || cli.backend == "llamacpp" {
        backend.name().to_string()
    } else {
        main_model
    };

    match cli.backend.as_str() {
        "stub" => eprintln!("Using stub inference backend (canned Socratic responses)."),
        "cloud" => eprintln!("Using cloud inference backend (Anthropic {main_model})."),
        "deepseek" => eprintln!("Using deepseek cloud inference backend ({main_model})."),
        "ollama" => eprintln!(
            "Using ollama backend at {} with model {main_model}.",
            cli.ollama_url
        ),
        "openai-compat" => eprintln!(
            "Using openai-compat backend at {} with model {main_model}.",
            cli.openai_compat_url
        ),
        "qnn" => {
            eprintln!("Using qnn (Qualcomm NPU) backend with {main_model}.");
            // The NPU serialises only when it serves, not its fallback.
            let npu_serves = !matches!(notice, Some(BackendNotice::PrimaryUnavailable { .. }));
            warn_on_npu_serialisation(cli, npu_serves);
        }
        "llamacpp" => {
            eprintln!("Using embedded llama.cpp backend with {main_model}.");
        }
        other => {
            eprintln!(
                "Unknown backend: {other}. Use 'stub', 'cloud', 'ollama', 'openai-compat', 'deepseek', 'qnn', or 'llamacpp'."
            );
            std::process::exit(1);
        }
    }

    // The banner above names the *requested* primary `--backend`. When an
    // opt-in fallback is configured, a startup fallback may have swapped in the
    // secondary — surface the *effective* backend so the banner can't mislead.
    // `backend.name()` is the primary's name when wrapped or kept alone, and
    // the secondary's name when the primary was unavailable at startup.
    if cli.fallback_backend.is_some() {
        eprintln!("Effective inference backend: {}.", backend.name());
    }

    Inference {
        backend,
        notice,
        main_model,
        backend_params,
    }
}
