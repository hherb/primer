//! Primer CLI — a text-mode REPL for developing and testing the Socratic
//! dialogue engine without any hardware (no microphone, no speaker, no
//! E Ink display). Just a terminal and a conversation.
//!
//! Usage:
//!   primer                                              # Stub backend (canned responses, no model needed)
//!   primer --backend cloud                              # Anthropic Claude API (default model)
//!   primer --backend cloud --model claude-opus-4-7      # Override the cloud model
//!   primer --backend ollama --model llama3.2            # Local Ollama server
//!   primer --name Binti --age 8                         # Set learner profile
//!   primer --resume <uuid>                              # Resume a past session

#[cfg(feature = "speech")]
mod speech_loop;

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use clap::Parser;
use primer_classifier::{
    ClassifierSettings, EngagementClassifier, LlmEngagementClassifier, StubEngagementClassifier,
};
use primer_core::config::PedagogyConfig;
use primer_core::error::{PrimerError, Result};
use primer_core::i18n::{Locale, render_inference_error};
use primer_core::inference::InferenceBackend;
use primer_core::knowledge::KnowledgeBase;
use primer_core::learner::*;
use primer_core::storage::{LearnerStore, SessionStore};
use primer_extractor::{
    ConceptExtractor, ExtractorSettings, LlmConceptExtractor, StubConceptExtractor,
};
use primer_inference::stub::StubBackend;
use primer_knowledge::SqliteKnowledgeBase;
use primer_pedagogy::DialogueManager;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

/// SQLite path token for an in-memory database — used as the default
/// for `--knowledge-db` when no path is given. Sessions persist to a
/// per-learner file under `~/.primer/` instead.
const IN_MEMORY: &str = ":memory:";

/// Subdirectory under `$HOME` for per-learner session databases.
const PRIMER_HOME_DIR: &str = ".primer";

#[derive(Parser, Debug)]
#[command(name = "primer", about = "The Primer — a Socratic learning companion")]
struct Cli {
    /// Inference backend: "stub", "cloud", or "ollama".
    #[arg(long, default_value = "stub")]
    backend: String,

    /// Model identifier. For cloud: Anthropic model id (default: claude-sonnet-4-6).
    /// For ollama: local model tag (e.g., "llama3.2", "qwen2.5:7b") — required.
    #[arg(long)]
    model: Option<String>,

    /// Ollama server URL (used when --backend ollama).
    #[arg(long, default_value = "http://localhost:11434")]
    ollama_url: String,

    /// Child's name (for the learner profile).
    #[arg(long, default_value = "Explorer")]
    name: String,

    /// Child's age in years.
    #[arg(long, default_value_t = 8)]
    age: u8,

    /// Locale (BCP-47 short pack id, e.g. "en"). Selects the prompt
    /// pack and (in future) the speech pipeline + per-locale knowledge
    /// index. Phase 0.1 ships only "en"; passing an unknown id is a
    /// hard error at startup. Persisted with the learner — a returning
    /// child does not re-specify their locale each session.
    #[arg(long, default_value = "en")]
    language: String,

    /// Path to knowledge base SQLite file.
    /// If omitted, uses an in-memory database.
    #[arg(long)]
    knowledge_db: Option<PathBuf>,

    /// Path to session database SQLite file.
    /// If omitted, defaults to `~/.primer/<slug-of-name>.db` (per-learner
    /// file, created if missing). Pass an explicit path only when you
    /// want a non-default location.
    #[arg(long)]
    session_db: Option<PathBuf>,

    /// Resume an existing session by UUID. Read from `--session-db`
    /// (default `~/.primer/<name>.db`). Errors if the file doesn't
    /// exist or no session with that id is stored. Works with any
    /// backend including stub: historical turns provide context, new
    /// turns get backend-appropriate responses.
    #[arg(long, value_name = "UUID", conflicts_with = "no_persist")]
    resume: Option<Uuid>,

    /// Run the session in-memory only — nothing is written to disk
    /// and the conversation evaporates on exit. Useful for quick
    /// experiments and tests; the per-learner default-path persistence
    /// stays out of the way. Mutually exclusive with `--resume` and
    /// `--session-db`.
    #[arg(long, conflicts_with_all = ["session_db", "resume"])]
    no_persist: bool,

    /// Anthropic API key (for cloud backend).
    #[arg(long, env = "ANTHROPIC_API_KEY")]
    api_key: Option<String>,

    /// Backend used for the engagement classifier. Defaults to the same
    /// backend used for the chat (--backend); `stub` forces deterministic
    /// classification regardless of main backend.
    #[arg(long)]
    classifier_backend: Option<String>,

    /// Model used for the engagement classifier. Defaults to the same
    /// model used for the chat (--model). Useful for haiku-as-classifier
    /// + sonnet-as-chat configurations on bigger machines.
    #[arg(long)]
    classifier_model: Option<String>,

    /// Maximum time to block awaiting the previous turn's classification
    /// before the next intent decision. Defaults to
    /// `primer_classifier::consts::DEFAULT_BLOCKING_TIMEOUT_MS`.
    #[arg(long, default_value_t = primer_classifier::consts::DEFAULT_BLOCKING_TIMEOUT_MS)]
    classifier_timeout_ms: u64,

    /// Backend used for the concept extractor. Defaults to the same
    /// backend used for the chat (`--backend`); `stub` forces deterministic
    /// extraction (empty concepts) regardless of main backend.
    #[arg(long)]
    extractor_backend: Option<String>,

    /// Model used for the concept extractor. Defaults to the same
    /// model used for the chat (`--model`). Useful for haiku-as-extractor
    /// + sonnet-as-chat on bigger machines.
    #[arg(long)]
    extractor_model: Option<String>,

    /// Maximum time to block awaiting the previous turn's extraction
    /// before the next intent decision. Defaults to
    /// `primer_extractor::consts::DEFAULT_BLOCKING_TIMEOUT_MS`.
    #[arg(long, default_value_t = primer_extractor::consts::DEFAULT_BLOCKING_TIMEOUT_MS)]
    extractor_timeout_ms: u64,

    /// Backend used for the comprehension classifier. Defaults to the same
    /// backend used for the chat (`--backend`); `stub` forces deterministic
    /// comprehension (empty assessments) regardless of main backend.
    #[arg(long)]
    comprehension_backend: Option<String>,

    /// Model used for the comprehension classifier. Defaults to the same
    /// model used for the chat (`--model`). Useful for haiku-as-comprehension
    /// + sonnet-as-chat configurations on bigger machines.
    #[arg(long)]
    comprehension_model: Option<String>,

    /// Maximum time to block awaiting the previous turn's
    /// extractor → comprehension chain before the next intent decision.
    /// Combined with `--extractor-timeout-ms` this caps the total
    /// background-work budget per turn. Defaults to
    /// `primer_comprehension::consts::DEFAULT_BLOCKING_TIMEOUT_MS`.
    #[arg(long, default_value_t = primer_comprehension::consts::DEFAULT_BLOCKING_TIMEOUT_MS)]
    comprehension_timeout_ms: u64,

    /// Print pedagogical decisions (intent chosen, classifier output,
    /// extractor output, comprehension output) alongside the conversation,
    /// on stderr. Stdout stays clean.
    #[arg(long)]
    verbose: bool,

    /// Run the voice REPL instead of the text REPL. Requires --whisper-model,
    /// --voice-onnx, --voice-config. Available only when the binary is built
    /// with --features speech.
    #[cfg(feature = "speech")]
    #[arg(long, requires_all = ["whisper_model", "voice_onnx", "voice_config"])]
    speech: bool,

    /// Path to the whisper.cpp GGML/GGUF model file
    /// (e.g. ~/models/ggml-small.en.bin). Required if --speech.
    #[cfg(feature = "speech")]
    #[arg(long, value_name = "PATH")]
    whisper_model: Option<PathBuf>,

    /// Path to the Piper voice ONNX file
    /// (e.g. ~/models/voices/en_GB-alba-medium.onnx). Required if --speech.
    #[cfg(feature = "speech")]
    #[arg(long, value_name = "PATH")]
    voice_onnx: Option<PathBuf>,

    /// Path to the matching Piper voice JSON sidecar
    /// (e.g. ~/models/voices/en_GB-alba-medium.onnx.json). Required if --speech.
    #[cfg(feature = "speech")]
    #[arg(long, value_name = "PATH")]
    voice_config: Option<PathBuf>,

    /// Voice id used as the VoiceProfile.model_id. Must match the file
    /// stem of --voice-onnx (Piper rejects mismatches at session open).
    #[cfg(feature = "speech")]
    #[arg(long, default_value = "en_GB-alba-medium")]
    voice: String,

    /// Override silero's min_silence_ms for --speech mode. The default
    /// (300 ms) is too aggressive given the cancel-on-resume safety net;
    /// 600 ms reduces false trips at no perceived-latency cost. Bounded
    /// to [50, 5000] ms — values below 50 ms make silero fire constantly,
    /// above 5 s defeats the purpose.
    #[cfg(feature = "speech")]
    #[arg(long, default_value_t = 600, value_parser = parse_mic_silence_ms)]
    mic_silence_ms: u32,
}

#[cfg(feature = "speech")]
fn parse_mic_silence_ms(s: &str) -> std::result::Result<u32, String> {
    let n: u32 = s.parse().map_err(|e| format!("not a u32: {e}"))?;
    if !(50..=5000).contains(&n) {
        return Err(format!(
            "mic-silence-ms must be between 50 and 5000, got {n}"
        ));
    }
    Ok(n)
}

/// Slugify a learner name into a filesystem-safe filename stem.
///
/// The input is first NFC-normalized so two visually identical names
/// (e.g. precomposed `é` vs decomposed `e` + combining acute) map to
/// the same slug. Characters that Unicode classifies as alphanumeric
/// — Latin, Cyrillic, CJK, etc. — are kept (Latin is lowercased; CJK
/// has no case so it round-trips). Every other character becomes `-`;
/// runs of `-` collapse; leading/trailing `-` are stripped. An empty
/// result falls back to `default` so we always produce a valid filename.
fn slug(name: &str) -> String {
    let normalized: String = name.nfc().collect();
    let lowered = normalized.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut last_was_sep = true; // suppress leading sep
    for c in lowered.chars() {
        if c.is_alphanumeric() {
            out.push(c);
            last_was_sep = false;
        } else if !last_was_sep {
            out.push('-');
            last_was_sep = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "default".to_string()
    } else {
        out
    }
}

/// Resolve the path to use for the session database.
/// `:memory:` when `no_persist` is set; otherwise the explicit path if
/// given, falling back to `<home>/.primer/<slug(name)>.db`. The home
/// directory is taken as a parameter so callers can supply it from any
/// source (env var in production, synthetic value in tests) without
/// this function touching the process environment.
fn resolve_session_db_path(
    explicit: Option<PathBuf>,
    home: &Path,
    learner_name: &str,
    no_persist: bool,
) -> PathBuf {
    if no_persist {
        return PathBuf::from(IN_MEMORY);
    }
    explicit.unwrap_or_else(|| {
        home.join(PRIMER_HOME_DIR)
            .join(format!("{}.db", slug(learner_name)))
    })
}

/// Should we print the "we just started persisting your sessions"
/// banner? True only when the session DB is at the default path AND
/// the file did not exist before this run AND the user did not opt
/// out via `--no-persist`. The banner answers the legitimate "where
/// did my conversation go?" question that the silent default-path
/// change would otherwise raise.
fn should_show_first_run_banner(
    explicit_session_db: bool,
    no_persist: bool,
    file_existed_before: bool,
) -> bool {
    !explicit_session_db && !no_persist && !file_existed_before
}

/// Parameters needed by `build_backend` and `build_classifier` that would
/// otherwise require borrowing from `Cli`. Extracted early (before any partial
/// moves of `Cli` fields) so the helpers can be called after those moves.
struct BackendParams {
    api_key: Option<String>,
    ollama_url: String,
    classifier_backend: Option<String>,
    classifier_model: Option<String>,
    extractor_backend: Option<String>,
    extractor_model: Option<String>,
    comprehension_backend: Option<String>,
    comprehension_model: Option<String>,
}

/// Construct an `InferenceBackend` of the named type with the given model.
///
/// All three backend variants are synchronous at construction time; the
/// function signature is `async` only for uniformity with `build_classifier`.
async fn build_backend(
    backend_name: &str,
    model: String,
    params: &BackendParams,
) -> Result<Arc<dyn InferenceBackend>> {
    match backend_name {
        "stub" => Ok(Arc::new(StubBackend)),
        "cloud" => {
            let api_key = params.api_key.clone().ok_or_else(|| {
                PrimerError::Inference(
                    "--api-key or ANTHROPIC_API_KEY required for cloud backend".into(),
                )
            })?;
            Ok(Arc::new(primer_inference::cloud::CloudBackend::new(
                "https://api.anthropic.com".to_string(),
                api_key,
                model,
            )))
        }
        "ollama" => Ok(Arc::new(primer_inference::ollama::OllamaBackend::new(
            params.ollama_url.clone(),
            model,
        ))),
        other => Err(PrimerError::Inference(
            format!("unknown backend: {other}").into(),
        )),
    }
}

/// Construct the engagement classifier according to the dispatch matrix:
///
/// | main backend | --classifier-backend | --classifier-model | outcome                               |
/// |--------------|----------------------|--------------------|---------------------------------------|
/// | stub         | (unset)              | (any)              | StubEngagementClassifier              |
/// | *            | "stub"               | (any)              | StubEngagementClassifier              |
/// | *            | some(X)              | None               | error (model required for non-stub)   |
/// | *            | some(X)              | some(M)            | LlmEngagementClassifier(new backend X, model M) |
/// | non-stub     | (unset)              | None               | LlmEngagementClassifier(Arc::clone main) |
/// | non-stub     | (unset)              | some(M)            | LlmEngagementClassifier(new backend same type, model M) |
async fn build_classifier(
    main_backend: Arc<dyn InferenceBackend>,
    main_backend_name: &str,
    main_model: &str,
    params: &BackendParams,
    settings: ClassifierSettings,
) -> Result<Arc<dyn EngagementClassifier>> {
    // Warn on a flag combination the user probably didn't mean: passing
    // --classifier-model alongside an explicit --classifier-backend stub
    // looks like the model name will take effect, but stub ignores it.
    if matches!(params.classifier_backend.as_deref(), Some("stub"))
        && params.classifier_model.is_some()
    {
        tracing::warn!(
            model = ?params.classifier_model,
            "--classifier-model ignored: --classifier-backend is stub (deterministic, no model)"
        );
    }

    match (main_backend_name, params.classifier_backend.as_deref()) {
        // Explicit stub override always wins regardless of main backend.
        (_, Some("stub")) => Ok(Arc::new(StubEngagementClassifier::new())),

        // Main backend is stub and no classifier backend specified → default to stub.
        // (LlmEngagementClassifier wrapping a stub backend would silently return
        // Unknown on every classify call — that's worse than a deterministic stub.)
        ("stub", None) => Ok(Arc::new(StubEngagementClassifier::new())),

        // Explicit non-stub classifier backend → need a model too.
        (_, Some(cls_backend_name)) => {
            let model = params.classifier_model.clone().ok_or_else(|| {
                PrimerError::Inference(
                    "--classifier-model is required when --classifier-backend is set to a non-stub backend".into(),
                )
            })?;
            let cls_backend = build_backend(cls_backend_name, model.clone(), params).await?;
            Ok(Arc::new(LlmEngagementClassifier::new(
                cls_backend,
                model,
                settings,
            )))
        }

        // No --classifier-backend specified, main is not stub.
        (_, None) => {
            match params.classifier_model.as_deref() {
                // No model override → reuse the main backend via Arc::clone.
                // The classifier identifier becomes "llm:<main-model>".
                None => Ok(Arc::new(LlmEngagementClassifier::new(
                    Arc::clone(&main_backend),
                    main_model.to_string(),
                    settings,
                ))),
                // Model override only → construct a fresh backend of the same TYPE
                // with the override model, because InferenceBackend::generate() does
                // not accept a model argument — model is baked in at construction.
                Some(override_model) => {
                    let cls_backend =
                        build_backend(main_backend_name, override_model.to_string(), params)
                            .await?;
                    Ok(Arc::new(LlmEngagementClassifier::new(
                        cls_backend,
                        override_model.to_string(),
                        settings,
                    )))
                }
            }
        }
    }
}

/// Construct the concept extractor according to the same dispatch matrix
/// as `build_classifier`:
///
/// | main backend | --extractor-backend | --extractor-model | outcome                                |
/// |--------------|---------------------|-------------------|----------------------------------------|
/// | stub         | (unset)             | (any)             | StubConceptExtractor                   |
/// | *            | "stub"              | (any)             | StubConceptExtractor                   |
/// | *            | some(X)             | None              | error (model required for non-stub)    |
/// | *            | some(X)             | some(M)           | LlmConceptExtractor(new backend X, model M) |
/// | non-stub     | (unset)             | None              | LlmConceptExtractor(Arc::clone main)   |
/// | non-stub     | (unset)             | some(M)           | LlmConceptExtractor(new backend same type, model M) |
async fn build_extractor(
    main_backend: Arc<dyn InferenceBackend>,
    main_backend_name: &str,
    main_model: &str,
    params: &BackendParams,
    settings: ExtractorSettings,
) -> Result<Arc<dyn ConceptExtractor>> {
    // Warn on a flag combination the user probably didn't mean: passing
    // --extractor-model alongside an explicit --extractor-backend stub
    // looks like the model name will take effect, but stub ignores it.
    if matches!(params.extractor_backend.as_deref(), Some("stub"))
        && params.extractor_model.is_some()
    {
        tracing::warn!(
            model = ?params.extractor_model,
            "--extractor-model ignored: --extractor-backend is stub (deterministic, no model)"
        );
    }

    match (main_backend_name, params.extractor_backend.as_deref()) {
        (_, Some("stub")) => Ok(Arc::new(StubConceptExtractor::new())),
        ("stub", None) => Ok(Arc::new(StubConceptExtractor::new())),
        (_, Some(ext_backend_name)) => {
            let model = params.extractor_model.clone().ok_or_else(|| {
                PrimerError::Inference(
                    "--extractor-model is required when --extractor-backend is set to a non-stub backend".into(),
                )
            })?;
            let ext_backend = build_backend(ext_backend_name, model.clone(), params).await?;
            Ok(Arc::new(LlmConceptExtractor::new(
                ext_backend,
                model,
                settings,
            )))
        }
        (_, None) => match params.extractor_model.as_deref() {
            None => Ok(Arc::new(LlmConceptExtractor::new(
                Arc::clone(&main_backend),
                main_model.to_string(),
                settings,
            ))),
            Some(override_model) => {
                let ext_backend =
                    build_backend(main_backend_name, override_model.to_string(), params).await?;
                Ok(Arc::new(LlmConceptExtractor::new(
                    ext_backend,
                    override_model.to_string(),
                    settings,
                )))
            }
        },
    }
}

/// Construct the comprehension classifier according to the same dispatch matrix
/// as `build_extractor`:
///
/// | main backend | --comprehension-backend | --comprehension-model | outcome                                    |
/// |--------------|-------------------------|-----------------------|--------------------------------------------|
/// | stub         | (unset)                 | (any)                 | StubComprehensionClassifier                |
/// | *            | "stub"                  | (any)                 | StubComprehensionClassifier                |
/// | *            | some(X)                 | None                  | error (model required for non-stub)        |
/// | *            | some(X)                 | some(M)               | LlmComprehensionClassifier(new backend X, model M) |
/// | non-stub     | (unset)                 | None                  | LlmComprehensionClassifier(Arc::clone main) |
/// | non-stub     | (unset)                 | some(M)               | LlmComprehensionClassifier(new backend same type, model M) |
async fn build_comprehension(
    main_backend: Arc<dyn InferenceBackend>,
    main_backend_name: &str,
    main_model: &str,
    params: &BackendParams,
    settings: primer_comprehension::ComprehensionSettings,
) -> Result<Arc<dyn primer_comprehension::ComprehensionClassifier>> {
    if matches!(params.comprehension_backend.as_deref(), Some("stub"))
        && params.comprehension_model.is_some()
    {
        tracing::warn!(
            model = ?params.comprehension_model,
            "--comprehension-model ignored: --comprehension-backend is stub (deterministic, no model)"
        );
    }

    match (main_backend_name, params.comprehension_backend.as_deref()) {
        (_, Some("stub")) => Ok(Arc::new(
            primer_comprehension::StubComprehensionClassifier::new(),
        )),
        ("stub", None) => Ok(Arc::new(
            primer_comprehension::StubComprehensionClassifier::new(),
        )),
        (_, Some(comp_backend_name)) => {
            let model = params.comprehension_model.clone().ok_or_else(|| {
                PrimerError::Inference(
                    "--comprehension-model is required when --comprehension-backend is set to a non-stub backend".into(),
                )
            })?;
            let comp_backend = build_backend(comp_backend_name, model.clone(), params).await?;
            Ok(Arc::new(
                primer_comprehension::LlmComprehensionClassifier::new(
                    comp_backend,
                    model,
                    settings,
                ),
            ))
        }
        (_, None) => match params.comprehension_model.as_deref() {
            None => Ok(Arc::new(
                primer_comprehension::LlmComprehensionClassifier::new(
                    Arc::clone(&main_backend),
                    main_model.to_string(),
                    settings,
                ),
            )),
            Some(override_model) => {
                let comp_backend =
                    build_backend(main_backend_name, override_model.to_string(), params).await?;
                Ok(Arc::new(
                    primer_comprehension::LlmComprehensionClassifier::new(
                        comp_backend,
                        override_model.to_string(),
                        settings,
                    ),
                ))
            }
        },
    }
}

/// Reconcile a freshly-loaded persisted `LearnerModel` against the
/// CLI flags for this launch.
///
/// Behaviour (kept minimal so the test surface matches the production
/// branch exactly):
/// - If `cli_name` differs from the persisted name, log a `tracing::warn!`
///   AND a stderr `eprintln!` (so a parent who typos a name sees it
///   without `RUST_LOG=warn`). The persisted name **always** wins —
///   silently rewriting it would lock a child out of their own data.
/// - Update `age` from the CLI (covers the birthday case).
/// - Update `last_active` to now.
///
/// Returns the reconciled `LearnerModel`. The caller is responsible
/// for the subsequent `save_learner` call (so this helper has no I/O).
fn reconcile_persisted_learner(
    mut existing: LearnerModel,
    cli_name: &str,
    cli_age: u8,
) -> LearnerModel {
    if existing.profile.name != cli_name {
        eprintln!(
            "Note: --name {:?} differs from the persisted learner name {:?}; \
             keeping persisted (delete ~/.primer/<slug>.db to start fresh).",
            cli_name, existing.profile.name
        );
        tracing::warn!(
            "CLI --name {:?} differs from persisted learner name {:?}; using persisted",
            cli_name,
            existing.profile.name
        );
    }
    existing.profile.age = cli_age;
    existing.profile.last_active = Utc::now();
    existing
}

#[cfg(feature = "speech")]
fn validate_speech_assets(
    whisper_model: &Path,
    voice_onnx: &Path,
    voice_config: &Path,
    voice_id: &str,
) -> Result<()> {
    if !whisper_model.exists() {
        return Err(PrimerError::Speech(format!(
            "whisper model not found at {}.\n\
             Download a GGML model from https://huggingface.co/ggerganov/whisper.cpp \
             (e.g. ggml-small.en.bin) and pass --whisper-model.",
            whisper_model.display()
        )));
    }
    if !voice_onnx.exists() {
        return Err(PrimerError::Speech(format!(
            "voice ONNX not found at {}.\n\
             Download a Piper voice from https://huggingface.co/rhasspy/piper-voices \
             and pass --voice-onnx.",
            voice_onnx.display()
        )));
    }
    if !voice_config.exists() {
        return Err(PrimerError::Speech(format!(
            "voice config not found at {}.\n\
             Pass --voice-config alongside --voice-onnx (the .onnx and .onnx.json files \
             ship together).",
            voice_config.display()
        )));
    }
    let stem = voice_onnx
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if stem != voice_id {
        tracing::warn!(
            voice_id,
            onnx_stem = stem,
            "--voice id does not match --voice-onnx file stem; \
             Piper will reject the session at open time"
        );
    }
    Ok(())
}

fn create_learner_with_id(
    id: Uuid,
    name: &str,
    age: u8,
    locale: primer_core::i18n::Locale,
) -> LearnerModel {
    LearnerModel {
        profile: LearnerProfile {
            id,
            name: name.to_string(),
            age,
            languages: vec![locale.pack_id().to_string()],
            locale,
            created_at: Utc::now(),
            last_active: Utc::now(),
        },
        concepts: vec![],
        preferences: LearningPreferences::default(),
        current_engagement: EngagementState::Engaged,
        recent_assessments: vec![],
    }
}

/// Probe common system locations for an `espeak-ng-data` directory and
/// set `PIPER_ESPEAKNG_DATA_DIRECTORY` to the parent of the first complete
/// one found. `espeak-rs-sys` ships an incomplete subset (missing `phontab`
/// and other core files); without a system install Piper's phonemizer
/// fails. Skipped if the env var is already set externally.
///
/// MUST run before the tokio runtime is built — `set_var` is `unsafe`
/// because concurrent `getenv` from any other thread is UB on Unix
/// libc. By calling this from the synchronous `main()` before any
/// runtime threads exist, we satisfy that precondition.
#[cfg(feature = "speech")]
fn probe_espeak_ng_data(verbose: bool) {
    if std::env::var_os("PIPER_ESPEAKNG_DATA_DIRECTORY").is_some() {
        return;
    }
    const ESPEAK_PARENT_CANDIDATES: &[&str] = &[
        "/opt/homebrew/share", // macOS Apple Silicon (brew install espeak-ng)
        "/usr/local/share",    // macOS Intel / generic
        "/usr/share",          // Linux (apt/dnf install espeak-ng-data)
    ];
    for parent in ESPEAK_PARENT_CANDIDATES {
        let probe = std::path::Path::new(parent).join("espeak-ng-data/phontab");
        if probe.is_file() {
            if verbose {
                eprintln!("[tts] found espeak-ng-data under {parent}");
            }
            // SAFETY: we are running before the tokio runtime (and any
            // worker threads, audio threads, or third-party library
            // threads) have been started. No other thread can be
            // calling getenv concurrently, so this `set_var` is sound.
            unsafe {
                std::env::set_var("PIPER_ESPEAKNG_DATA_DIRECTORY", parent);
            }
            return;
        }
    }
}

fn main() -> anyhow::Result<()> {
    // Load env files. Project-local `.env` first (searches cwd and ancestors),
    // then a user-global `~/.primer_env` for secrets that should live outside
    // any single repo. Earlier sources win — `from_path` does not override
    // existing env vars by default. Must run before clap parses `--api-key`.
    let _ = dotenvy::dotenv();
    if let Ok(home) = std::env::var("HOME") {
        let path = std::path::PathBuf::from(home).join(".primer_env");
        let _ = dotenvy::from_path(&path);
    }

    // Probe for system espeak-ng-data BEFORE the tokio runtime spawns
    // worker threads — `set_var` requires a single-threaded context.
    // Pre-parse `--verbose` so the probe can log its hit on stderr.
    #[cfg(feature = "speech")]
    {
        let verbose = std::env::args().any(|a| a == "--verbose");
        probe_espeak_ng_data(verbose);
    }

    // Build the tokio runtime explicitly (instead of `#[tokio::main]`)
    // so the env probe above runs single-threaded.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async_main())
}

/// Default tracing filter when `RUST_LOG` is unset.
///
/// `info` keeps the Primer's own diagnostics (banners, classifier
/// identifier, dialogue manager warnings) visible. `ort=warn` quiets
/// ONNX Runtime, which emits ~200 INFO lines per silero/piper session
/// init describing graph optimisations — useful exactly once and noise
/// thereafter. `whisper_cpp_plus=warn` and `cpal=warn` silence the
/// other speech-stack libraries we don't normally want to hear from.
/// Set `RUST_LOG=debug` (or `RUST_LOG=ort=info`) for the firehose.
const DEFAULT_LOG_FILTER: &str = "info,ort=warn,whisper_cpp_plus=warn,cpal=warn";

async fn async_main() -> anyhow::Result<()> {
    // Initialise tracing (set RUST_LOG=debug for verbose output, or
    // RUST_LOG=ort=info to see ONNX Runtime session-init logs).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER)),
        )
        .init();

    let cli = Cli::parse();

    // ─── Create backends ─────────────────────────────────────────────

    // Resolve the model for the main backend early so we can report it
    // in the banner AND pass the resolved value to build_classifier later.
    let main_model: String = match cli.backend.as_str() {
        "cloud" => cli
            .model
            .clone()
            .unwrap_or_else(|| "claude-sonnet-4-6".to_string()),
        "ollama" => cli.model.clone().unwrap_or_else(|| {
            eprintln!("Error: --model required for ollama backend (e.g., --model llama3.2).");
            std::process::exit(1);
        }),
        // stub (and anything else — will error in build_backend below)
        _ => cli.model.clone().unwrap_or_else(|| "stub".to_string()),
    };

    // Extract the fields that build_backend / build_classifier need BEFORE
    // any partial moves of other Cli fields (knowledge_db, session_db etc.).
    let backend_params = BackendParams {
        api_key: cli.api_key.clone(),
        ollama_url: cli.ollama_url.clone(),
        classifier_backend: cli.classifier_backend.clone(),
        classifier_model: cli.classifier_model.clone(),
        extractor_backend: cli.extractor_backend.clone(),
        extractor_model: cli.extractor_model.clone(),
        comprehension_backend: cli.comprehension_backend.clone(),
        comprehension_model: cli.comprehension_model.clone(),
    };

    let backend: Arc<dyn InferenceBackend> =
        match build_backend(&cli.backend, main_model.clone(), &backend_params).await {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Error constructing backend: {e}");
                std::process::exit(1);
            }
        };

    match cli.backend.as_str() {
        "stub" => eprintln!("Using stub inference backend (canned Socratic responses)."),
        "cloud" => eprintln!("Using cloud inference backend (Anthropic {main_model})."),
        "ollama" => eprintln!(
            "Using ollama backend at {} with model {main_model}.",
            cli.ollama_url
        ),
        other => {
            eprintln!("Unknown backend: {other}. Use 'stub', 'cloud', or 'ollama'.");
            std::process::exit(1);
        }
    }

    // Resolve the requested locale. An unknown pack id (e.g. a typo or
    // a build that doesn't include the requested language yet) is a
    // hard error at startup rather than a silent fall-back to English.
    let cli_locale: Locale = match Locale::from_pack_id(&cli.language) {
        Some(l) => l,
        None => {
            let known: Vec<&str> = Locale::ALL.iter().map(|l| l.pack_id()).collect();
            eprintln!(
                "Error: --language {:?} is not supported. Known locales: {:?}",
                cli.language, known
            );
            std::process::exit(1);
        }
    };

    // Knowledge base — in-memory by default (empty, but functional).
    // Locale-scoped: passages are indexed in the locale's own FTS5 table
    // so BM25 statistics stay locale-pure.
    let knowledge_path = cli.knowledge_db.unwrap_or_else(|| PathBuf::from(IN_MEMORY));
    let knowledge = SqliteKnowledgeBase::open_for_locale(&knowledge_path, cli_locale)?;

    // Session store — defaults to a per-learner file under `~/.primer/`.
    // We look up HOME here (rather than inside `resolve_session_db_path`)
    // so the function stays a pure path computation that's trivial to test.
    let explicit_session_db = cli.session_db.is_some();
    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => {
            if cli.session_db.is_none() && !cli.no_persist {
                eprintln!(
                    "Error: cannot find HOME env var to default --session-db; pass it explicitly"
                );
                std::process::exit(1);
            }
            // HOME is unused when an explicit path is given or --no-persist is set.
            PathBuf::new()
        }
    };
    let session_path = resolve_session_db_path(cli.session_db, &home, &cli.name, cli.no_persist);

    // Resume requires the file to already exist; do not auto-create on
    // a typo'd path. Catch this BEFORE we open (which would create it).
    // (--resume is mutually exclusive with --no-persist via clap, so the
    // path here is always a real on-disk path.)
    if cli.resume.is_some() && !Path::new(&session_path).exists() {
        eprintln!(
            "Error: --resume requires an existing --session-db; {} does not exist.",
            session_path.display()
        );
        std::process::exit(1);
    }

    // Capture file-existed state BEFORE we open (open creates it). Used
    // for the first-run banner. The :memory: token is never a real path,
    // so its existence check is naturally false — that's fine: the banner
    // is also gated on !no_persist below.
    let file_existed_before = !cli.no_persist && Path::new(&session_path).exists();

    // Ensure parent directory exists before opening. Not needed for :memory:.
    if !cli.no_persist {
        if let Some(parent) = session_path.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    eprintln!(
                        "Error: cannot create session-db directory {}: {e}",
                        parent.display()
                    );
                    std::process::exit(1);
                }
            }
        }
    }

    let session_store: Arc<primer_storage::SqliteSessionStore> = Arc::new(
        match primer_storage::SqliteSessionStore::open(&session_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "Error: cannot open session-db {}: {e}",
                    session_path.display()
                );
                std::process::exit(1);
            }
        },
    );
    if cli.no_persist {
        eprintln!("Session DB: in-memory (no persistence; session ends when you exit).");
    } else {
        eprintln!("Session DB: {}", session_path.display());
        if should_show_first_run_banner(explicit_session_db, cli.no_persist, file_existed_before) {
            eprintln!(
                "Note: this is your first session for {name}. Conversations are now persisted\n      \
                 locally to {path}. Use `--no-persist` to opt out, or `--session-db` to relocate.",
                name = cli.name,
                path = session_path.display(),
            );
        }
    }

    // Learner model — load from persistent store or mint fresh on first run.
    let learner = match session_store.load_learner().await {
        Ok(Some(existing)) => {
            let reconciled = reconcile_persisted_learner(existing, &cli.name, cli.age);
            if let Err(e) = session_store.save_learner(&reconciled).await {
                tracing::warn!("save_learner on startup failed: {e}");
            }
            reconciled
        }
        Ok(None) => {
            // First run on this file. Two sub-cases:
            //   1. Truly fresh DB → mint a new UUID.
            //   2. v3 DB with sessions but no learners row → adopt the
            //      most-recent session's learner_id so existing sessions
            //      are not orphaned.
            let id = match session_store.most_recent_session_learner_id().await {
                Ok(Some(uuid)) => {
                    tracing::info!("adopted learner_id {uuid} from existing sessions");
                    uuid
                }
                Ok(None) => Uuid::new_v4(),
                Err(e) => {
                    tracing::warn!(
                        "most_recent_session_learner_id failed: {e}; minting fresh UUID"
                    );
                    Uuid::new_v4()
                }
            };
            let fresh = create_learner_with_id(id, &cli.name, cli.age, cli_locale);
            if let Err(e) = session_store.save_learner(&fresh).await {
                tracing::warn!("save_learner on startup failed: {e}");
            }
            fresh
        }
        Err(e) => {
            // load_learner failing on startup is catastrophic — the file
            // is unreadable or the schema is corrupt. Propagate.
            return Err(anyhow::anyhow!("load_learner failed on startup: {e}"));
        }
    };

    // Pedagogy config.
    let pedagogy_config = PedagogyConfig::default();

    let classifier_settings = ClassifierSettings {
        blocking_timeout: std::time::Duration::from_millis(cli.classifier_timeout_ms),
        ..ClassifierSettings::default()
    };

    let classifier: Arc<dyn EngagementClassifier> = match build_classifier(
        Arc::clone(&backend),
        &cli.backend,
        &main_model,
        &backend_params,
        classifier_settings.clone(),
    )
    .await
    {
        Ok(c) => {
            eprintln!("Engagement classifier: {}", c.identifier());
            c
        }
        Err(e) => {
            eprintln!("Error constructing engagement classifier: {e}");
            std::process::exit(1);
        }
    };

    let extractor_settings = ExtractorSettings {
        blocking_timeout: std::time::Duration::from_millis(cli.extractor_timeout_ms),
        ..ExtractorSettings::default()
    };

    let extractor: Arc<dyn ConceptExtractor> = match build_extractor(
        Arc::clone(&backend),
        &cli.backend,
        &main_model,
        &backend_params,
        extractor_settings.clone(),
    )
    .await
    {
        Ok(e) => {
            eprintln!("Concept extractor: {}", e.identifier());
            e
        }
        Err(e) => {
            eprintln!("Error constructing concept extractor: {e}");
            std::process::exit(1);
        }
    };

    let comprehension_settings = primer_comprehension::ComprehensionSettings {
        blocking_timeout: std::time::Duration::from_millis(cli.comprehension_timeout_ms),
        ..primer_comprehension::ComprehensionSettings::default()
    };

    let comprehension: Arc<dyn primer_comprehension::ComprehensionClassifier> =
        match build_comprehension(
            Arc::clone(&backend),
            &cli.backend,
            &main_model,
            &backend_params,
            comprehension_settings.clone(),
        )
        .await
        {
            Ok(c) => {
                eprintln!("Comprehension classifier: {}", c.identifier());
                c
            }
            Err(e) => {
                eprintln!("Error constructing comprehension classifier: {e}");
                std::process::exit(1);
            }
        };

    // ─── Dialogue manager ────────────────────────────────────────────

    let stores = primer_pedagogy::DialogueManagerStores {
        session: Some(Arc::clone(&session_store) as Arc<dyn SessionStore>),
        learner: Some(Arc::clone(&session_store) as Arc<dyn LearnerStore>),
    };
    let subsystems = primer_pedagogy::DialogueManagerSubsystems {
        classifier,
        classifier_settings,
        extractor,
        extractor_settings,
        comprehension,
        comprehension_settings,
    };
    let mut dm = DialogueManager::new(
        learner,
        backend.as_ref(),
        &knowledge as &dyn KnowledgeBase,
        stores,
        subsystems,
        pedagogy_config,
    );

    // ─── Speech branch ───────────────────────────────────────────────

    #[cfg(feature = "speech")]
    if cli.speech {
        let whisper_model = cli.whisper_model.as_ref().expect("clap requires_all");
        let voice_onnx = cli.voice_onnx.as_ref().expect("clap requires_all");
        let voice_config = cli.voice_config.as_ref().expect("clap requires_all");
        validate_speech_assets(whisper_model, voice_onnx, voice_config, &cli.voice)?;

        let cfg = speech_loop::SpeechLoopConfig {
            whisper_model,
            voice_onnx,
            voice_config,
            voice_id: &cli.voice,
            mic_silence_ms: cli.mic_silence_ms,
            verbose: cli.verbose,
        };
        // run() builds backends from cfg, wires DialogueManager via a
        // Responder adapter, drives the state machine.
        speech_loop::run(cfg, &mut dm).await?;
        return Ok(());
    }

    // ─── REPL ────────────────────────────────────────────────────────

    if let Some(resume_id) = cli.resume {
        match session_store.load_session(resume_id).await? {
            None => {
                eprintln!(
                    "Error: no session with id {resume_id} found in {}",
                    session_path.display()
                );
                std::process::exit(1);
            }
            Some(loaded) => {
                let n_turns = loaded.turns.len();
                dm.resume_session(loaded).await?;
                eprintln!("\nResumed session {resume_id} with {n_turns} prior turn(s).\n");
            }
        }
    } else {
        let greeting = dm.open_session().await?;
        println!("\nPrimer: {greeting}\n");
    }

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let reader = stdin.lock();

    for line in reader.lines() {
        let line = line?;
        let input = line.trim();

        // Exit commands.
        if input.is_empty() {
            continue;
        }
        if input.eq_ignore_ascii_case("quit")
            || input.eq_ignore_ascii_case("exit")
            || input.eq_ignore_ascii_case("bye")
        {
            dm.close_session().await;
            println!("\nPrimer: That was a good conversation. Until next time.\n");
            break;
        }

        // Generate the Primer's response, printing tokens as they arrive.
        // The "Primer: " prefix is held back until the first non-empty chunk
        // so that an immediate failure (no API key, ollama down) doesn't leave
        // a dangling "Primer: " above the error message.
        let mut prefix_printed = false;
        let result = dm
            .respond_to_streaming(input, |chunk| {
                if !prefix_printed {
                    print!("\nPrimer: ");
                    prefix_printed = true;
                }
                print!("{chunk}");
                let _ = io::stdout().flush();
            })
            .await;
        match result {
            Ok(_) => {
                if prefix_printed {
                    println!("\n");
                } else {
                    eprintln!("\n(no response generated)\n");
                }
            }
            Err(PrimerError::Inference(inf)) => {
                if prefix_printed {
                    println!();
                }
                tracing::warn!(error = %inf, "inference failed; surfacing user-friendly message");
                eprintln!("{}\n", render_inference_error(&inf, &cli_locale));
            }
            Err(other) => {
                if prefix_printed {
                    println!();
                }
                eprintln!("Error generating response: {other}\n");
            }
        }

        // Print pedagogical debug info when --verbose is set.
        if cli.verbose {
            if let Some(intent) = dm.last_intent() {
                eprintln!(
                    "[intent] {:?} -> {:?}",
                    dm.learner.current_engagement, intent
                );
            }
            if let Some(a) = dm.last_assessment() {
                let r = a.reasoning.as_deref().unwrap_or("");
                eprintln!(
                    "[classifier] {:?} conf={:.2} ({})",
                    a.state,
                    a.confidence,
                    dm.classifier_identifier()
                );
                if !r.is_empty() {
                    eprintln!("             — {r}");
                }
            }
            if let Some(e) = dm.last_extraction() {
                eprintln!(
                    "[extractor] child={:?} primer={:?} ({})",
                    e.child_concepts,
                    e.primer_concepts,
                    dm.extractor_identifier()
                );
            }
            if let Some(c) = dm.last_comprehension() {
                if c.assessments.is_empty() {
                    eprintln!("[comprehension] (none) ({})", dm.comprehension_identifier());
                } else {
                    let pairs: Vec<String> = c
                        .assessments
                        .iter()
                        .map(|a| format!("{}={}({:.2})", a.concept, a.depth, a.confidence))
                        .collect();
                    eprintln!(
                        "[comprehension] {} ({})",
                        pairs.join(" "),
                        dm.comprehension_identifier()
                    );
                }
            }
        }

        // Check if the session has run long.
        if dm.should_suggest_break() {
            println!("Primer: We've been talking for a while. Want to take a break?\n");
        }

        print!("{}: ", dm.learner.profile.name);
        stdout.flush()?;
    }

    Ok(())
}

#[cfg(test)]
mod classifier_construction_tests {
    use super::*;

    /// Build a minimal `BackendParams` for testing.
    fn params_with(
        classifier_backend: Option<&str>,
        classifier_model: Option<&str>,
    ) -> BackendParams {
        BackendParams {
            api_key: None,
            ollama_url: "http://localhost:11434".into(),
            classifier_backend: classifier_backend.map(String::from),
            classifier_model: classifier_model.map(String::from),
            extractor_backend: None,
            extractor_model: None,
            comprehension_backend: None,
            comprehension_model: None,
        }
    }

    /// With main=stub and no classifier flags, we get a stub classifier.
    #[tokio::test]
    async fn stub_main_no_flags_gives_stub_classifier() {
        let params = params_with(None, None);
        let main = build_backend("stub", "main".into(), &params).await.unwrap();
        let c = build_classifier(main, "stub", "main", &params, ClassifierSettings::default())
            .await
            .unwrap();
        assert_eq!(c.identifier(), "stub");
    }

    /// `--classifier-backend stub` wins regardless of the main backend.
    #[tokio::test]
    async fn explicit_stub_override_wins_over_any_main() {
        // Main is stub (would default to stub anyway), but the explicit flag
        // should also work when the main is declared as a different type.
        // We use stub here to avoid needing a real cloud backend in tests.
        let params_main = params_with(None, None);
        let main = build_backend("stub", "main-model".into(), &params_main)
            .await
            .unwrap();
        let params = params_with(Some("stub"), None);
        let c = build_classifier(
            main,
            "cloud",
            "main-model",
            &params,
            ClassifierSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "stub");
    }

    /// No classifier flags on a non-stub main → LlmEngagementClassifier wrapping
    /// Arc::clone of main (identifier is "llm:<main-model>").
    #[tokio::test]
    async fn non_stub_main_no_flags_gives_llm_with_main_model() {
        // We simulate a non-stub main by using "stub" type but treating it as
        // cloud in the dispatch (the type doesn't matter for the Arc::clone path;
        // what matters is that main_backend_name is NOT "stub").
        // Since we can't construct a real cloud backend without an API key,
        // we build a stub backend but pass "cloud" as the name to exercise
        // the dispatch arm that reuses main via Arc::clone.
        let params = params_with(None, None);
        let main = build_backend("stub", "claude-sonnet-4-6".into(), &params)
            .await
            .unwrap();
        let c = build_classifier(
            main,
            "cloud", // use "cloud" as name to exercise the non-stub arm
            "claude-sonnet-4-6",
            &params,
            ClassifierSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "llm:claude-sonnet-4-6");
    }

    /// `--classifier-backend ollama` without `--classifier-model` is an error.
    #[tokio::test]
    async fn classifier_backend_without_model_is_error() {
        let params_main = params_with(None, None);
        let main = build_backend("stub", "main".into(), &params_main)
            .await
            .unwrap();
        let params = params_with(Some("ollama"), None /* no model */);
        let result =
            build_classifier(main, "stub", "main", &params, ClassifierSettings::default()).await;
        assert!(
            result.is_err(),
            "should error when --classifier-backend is non-stub and --classifier-model is missing"
        );
    }

    /// `--classifier-model` override on a stub main still yields stub classifier
    /// (the stub arm fires before we reach the model-override arm).
    #[tokio::test]
    async fn classifier_model_override_on_stub_main_still_yields_stub() {
        let params = params_with(None, Some("override-model"));
        let main = build_backend("stub", "main".into(), &params).await.unwrap();
        let c = build_classifier(main, "stub", "main", &params, ClassifierSettings::default())
            .await
            .unwrap();
        // Main is stub → classifier defaults to stub regardless of model override.
        assert_eq!(c.identifier(), "stub");
    }

    /// `--classifier-model` override on a NON-stub main constructs a fresh
    /// backend of the same type with the override model (case 5 in the dispatch
    /// matrix, lines 284-292 in build_classifier).
    ///
    /// The `main_backend` Arc is a stub (we cannot unit-test real cloud/ollama
    /// without live infrastructure), but `main_backend_name` is "ollama" so the
    /// dispatch falls through to the model-override branch and calls
    /// `build_backend("ollama", "override-model", params)`.  `OllamaBackend::new`
    /// is purely constructive (no I/O), so this succeeds in unit tests.
    #[tokio::test]
    async fn classifier_model_override_on_non_stub_main_constructs_fresh_backend() {
        let params = params_with(None, Some("override-model"));
        // The Arc itself is a stub, but the *name* string "ollama" drives dispatch.
        let main = build_backend("stub", "main".into(), &params).await.unwrap();
        let c = build_classifier(
            main,
            "ollama", // non-stub name → falls through to model-override arm
            "main-model",
            &params,
            ClassifierSettings::default(),
        )
        .await
        .unwrap();
        // A fresh OllamaBackend is constructed with "override-model"; the
        // classifier identifier must reflect the override, not the main model.
        assert_eq!(c.identifier(), "llm:override-model");
    }

    /// An unknown `--classifier-backend` value must return an error (the
    /// explicit-backend arm calls `build_backend`, which errors on unknown names).
    #[tokio::test]
    async fn unknown_classifier_backend_returns_error() {
        let params = params_with(Some("nonexistent"), Some("any-model"));
        let main = build_backend("stub", "main".into(), &params).await.unwrap();
        let result =
            build_classifier(main, "stub", "main", &params, ClassifierSettings::default()).await;
        assert!(
            result.is_err(),
            "should error when --classifier-backend names an unknown backend"
        );
    }
}

#[cfg(test)]
mod extractor_construction_tests {
    use super::*;

    fn params(extractor_backend: Option<&str>, extractor_model: Option<&str>) -> BackendParams {
        BackendParams {
            api_key: Some("k".into()),
            ollama_url: "http://localhost:11434".into(),
            classifier_backend: None,
            classifier_model: None,
            extractor_backend: extractor_backend.map(String::from),
            extractor_model: extractor_model.map(String::from),
            comprehension_backend: None,
            comprehension_model: None,
        }
    }

    fn stub_main_backend() -> Arc<dyn InferenceBackend> {
        Arc::new(primer_inference::stub::StubBackend)
    }

    #[tokio::test]
    async fn stub_main_no_flags_gives_stub_extractor() {
        let e = build_extractor(
            stub_main_backend(),
            "stub",
            "stub",
            &params(None, None),
            ExtractorSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(e.identifier(), "stub");
    }

    #[tokio::test]
    async fn explicit_stub_override_wins() {
        let e = build_extractor(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(Some("stub"), None),
            ExtractorSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(e.identifier(), "stub");
    }

    #[tokio::test]
    async fn nonstub_backend_without_model_errors() {
        let r = build_extractor(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(Some("ollama"), None),
            ExtractorSettings::default(),
        )
        .await;
        assert!(r.is_err());
    }

    // Note: `stub_main_backend()` always returns a StubBackend (whose
    // `name()` is "stub"), even when `main_backend_name` says otherwise.
    // The identifier therefore reads as `llm:stub:<model>` in these
    // tests — what they actually verify is that the *model* name flows
    // through correctly. In production the main_backend_name and the
    // backend's own `name()` agree by construction.
    #[tokio::test]
    async fn no_flags_reuses_main_model_id() {
        let e = build_extractor(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(None, None),
            ExtractorSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(e.identifier(), "llm:stub:llama3.2");
    }

    #[tokio::test]
    async fn model_only_override_uses_main_backend_type() {
        // With `--extractor-model haiku` and main backend `ollama`,
        // `build_backend("ollama", "haiku", ...)` constructs a real
        // OllamaBackend, so the identifier carries the real backend
        // name (unlike the `_reuses_main_model_id` test above which
        // Arc::clones the stub harness).
        let e = build_extractor(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(None, Some("haiku")),
            ExtractorSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(e.identifier(), "llm:ollama:haiku");
    }
}

#[cfg(test)]
mod comprehension_construction_tests {
    use super::*;

    fn params(
        comprehension_backend: Option<&str>,
        comprehension_model: Option<&str>,
    ) -> BackendParams {
        BackendParams {
            api_key: Some("k".into()),
            ollama_url: "http://localhost:11434".into(),
            classifier_backend: None,
            classifier_model: None,
            extractor_backend: None,
            extractor_model: None,
            comprehension_backend: comprehension_backend.map(String::from),
            comprehension_model: comprehension_model.map(String::from),
        }
    }

    fn stub_main_backend() -> Arc<dyn InferenceBackend> {
        Arc::new(primer_inference::stub::StubBackend)
    }

    #[tokio::test]
    async fn stub_main_no_flags_gives_stub_comprehension() {
        let c = build_comprehension(
            stub_main_backend(),
            "stub",
            "stub",
            &params(None, None),
            primer_comprehension::ComprehensionSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "stub");
    }

    #[tokio::test]
    async fn explicit_stub_override_wins() {
        let c = build_comprehension(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(Some("stub"), None),
            primer_comprehension::ComprehensionSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "stub");
    }

    #[tokio::test]
    async fn nonstub_backend_without_model_errors() {
        let r = build_comprehension(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(Some("ollama"), None),
            primer_comprehension::ComprehensionSettings::default(),
        )
        .await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn no_flags_reuses_main_model_id() {
        let c = build_comprehension(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(None, None),
            primer_comprehension::ComprehensionSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "llm:stub:llama3.2");
    }

    #[tokio::test]
    async fn model_only_override_uses_main_backend_type() {
        let c = build_comprehension(
            stub_main_backend(),
            "ollama",
            "llama3.2",
            &params(None, Some("haiku")),
            primer_comprehension::ComprehensionSettings::default(),
        )
        .await
        .unwrap();
        assert_eq!(c.identifier(), "llm:ollama:haiku");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "speech")]
    #[test]
    fn parse_mic_silence_ms_accepts_in_range_values() {
        assert_eq!(parse_mic_silence_ms("50"), Ok(50));
        assert_eq!(parse_mic_silence_ms("600"), Ok(600));
        assert_eq!(parse_mic_silence_ms("5000"), Ok(5000));
    }

    #[cfg(feature = "speech")]
    #[test]
    fn parse_mic_silence_ms_rejects_out_of_range() {
        assert!(parse_mic_silence_ms("0").is_err());
        assert!(parse_mic_silence_ms("49").is_err());
        assert!(parse_mic_silence_ms("5001").is_err());
        assert!(parse_mic_silence_ms("100000").is_err());
    }

    #[cfg(feature = "speech")]
    #[test]
    fn parse_mic_silence_ms_rejects_non_numeric() {
        assert!(parse_mic_silence_ms("abc").is_err());
        assert!(parse_mic_silence_ms("").is_err());
        assert!(parse_mic_silence_ms("-100").is_err());
    }

    #[test]
    fn slug_lowercases_and_keeps_alphanumerics() {
        assert_eq!(slug("Explorer"), "explorer");
        assert_eq!(slug("Binti7"), "binti7");
    }

    #[test]
    fn slug_replaces_special_chars_with_dash() {
        assert_eq!(slug("Anna Maria"), "anna-maria");
        assert_eq!(slug("Lee/Davis"), "lee-davis");
    }

    #[test]
    fn slug_keeps_unicode_letters_lowercased() {
        // The previous ASCII-only rule collapsed `José`, `Łukasz`, `Соня`
        // and `美咲` into ambiguous or empty stems. Children's names are
        // a load-bearing input here — accept anything Unicode considers
        // alphanumeric, lowercased where that exists.
        assert_eq!(slug("José"), "josé");
        assert_eq!(slug("Łukasz"), "łukasz");
        assert_eq!(slug("Соня"), "соня");
        // No case-folding for CJK; the chars round-trip as-is.
        assert_eq!(slug("美咲"), "美咲");
    }

    #[test]
    fn slug_normalizes_nfc_so_decomposed_equals_precomposed() {
        // Same visible name, two Unicode encodings: precomposed `é`
        // (U+00E9) vs decomposed `e` + combining acute (U+0301). Without
        // NFC normalization these slug to different filenames, so two
        // copies of the same child get two session DBs.
        let nfc = "Jos\u{00E9}"; // José (NFC)
        let nfd = "Jose\u{0301}"; // José (NFD)
        assert_eq!(slug(nfc), slug(nfd));
    }

    #[test]
    fn slug_collapses_runs_of_separators() {
        assert_eq!(slug("a   b"), "a-b");
        assert_eq!(slug("a---b"), "a-b");
    }

    #[test]
    fn slug_strips_leading_and_trailing_separators() {
        assert_eq!(slug("  hello  "), "hello");
        assert_eq!(slug("___world___"), "world");
    }

    #[test]
    fn slug_empty_input_falls_back_to_default() {
        assert_eq!(slug(""), "default");
        assert_eq!(slug("!!!"), "default");
    }

    #[test]
    fn resolve_session_db_path_passes_explicit_through() {
        // The home arg is unused when an explicit path is given.
        let home = Path::new("/this/should/be/ignored");
        let p = resolve_session_db_path(
            Some(PathBuf::from("/tmp/explicit.db")),
            home,
            "Anyone",
            false,
        );
        assert_eq!(p, PathBuf::from("/tmp/explicit.db"));
    }

    #[test]
    fn resolve_session_db_path_default_uses_home_and_slug() {
        let home = Path::new("/synthetic/home");
        let p = resolve_session_db_path(None, home, "Binti", false);
        assert_eq!(p, PathBuf::from("/synthetic/home/.primer/binti.db"));
    }

    #[test]
    fn resolve_session_db_path_no_persist_returns_in_memory() {
        // `--no-persist` short-circuits everything: no slug, no home
        // join, no explicit path. The session is throwaway.
        let home = Path::new("/some/home");
        assert_eq!(
            resolve_session_db_path(None, home, "Anyone", true),
            PathBuf::from(IN_MEMORY)
        );
    }

    #[test]
    fn no_persist_conflicts_with_resume_at_parse_time() {
        // clap should reject a `--no-persist --resume <uuid>` invocation
        // before we ever try to open anything. In-memory + resume is
        // a contradiction (nothing to resume from).
        let result = Cli::try_parse_from([
            "primer",
            "--no-persist",
            "--resume",
            "00000000-0000-0000-0000-000000000000",
        ]);
        assert!(result.is_err(), "expected clap to reject the combination");
    }

    #[test]
    fn no_persist_conflicts_with_session_db_at_parse_time() {
        // Naming a session DB while asking for in-memory is also a
        // contradiction; clap should reject it up front.
        let result = Cli::try_parse_from(["primer", "--no-persist", "--session-db", "/tmp/x.db"]);
        assert!(result.is_err(), "expected clap to reject the combination");
    }

    #[test]
    fn first_run_banner_shows_only_for_default_path_first_run() {
        // Default path + brand-new file → show banner (the user just
        // started persisting without explicitly opting in).
        assert!(should_show_first_run_banner(false, false, false));
        // Default path but file already existed → silent (not first run).
        assert!(!should_show_first_run_banner(false, false, true));
        // Explicit path → silent (the user knows where their data is).
        assert!(!should_show_first_run_banner(true, false, false));
        // No-persist → silent (no file is being created at all).
        assert!(!should_show_first_run_banner(false, true, false));
    }

    #[test]
    fn resolve_session_db_path_default_handles_unicode_name() {
        // Confirms the slug + path composition round-trip a non-ASCII
        // name without env mutation. The same name in NFC vs NFD must
        // produce the same path so we don't end up with two DB files.
        let home = Path::new("/h");
        assert_eq!(
            resolve_session_db_path(None, home, "José", false),
            PathBuf::from("/h/.primer/josé.db")
        );
        let nfd = "Jose\u{0301}";
        assert_eq!(
            resolve_session_db_path(None, home, nfd, false),
            PathBuf::from("/h/.primer/josé.db")
        );
    }

    #[tokio::test]
    async fn cli_birthday_case_updates_age_and_keeps_uuid() {
        // Save a learner with age=8, simulate startup with --age=9 by
        // calling the SAME helper main() uses, then verify the persisted
        // row has age=9 with the same UUID and created_at preserved.
        use chrono::Utc;
        use primer_core::storage::LearnerStore;
        use primer_storage::SqliteSessionStore;
        use std::sync::Arc;

        let store = Arc::new(SqliteSessionStore::open(std::path::Path::new(":memory:")).unwrap());
        let original_id = Uuid::new_v4();
        let original_created = Utc::now() - chrono::Duration::days(365);
        let mut original =
            create_learner_with_id(original_id, "Binti", 8, primer_core::i18n::Locale::English);
        original.profile.created_at = original_created;
        store.save_learner(&original).await.unwrap();

        // Reload + reconcile via the production helper.
        let existing = store.load_learner().await.unwrap().expect("learner row");
        let reconciled = reconcile_persisted_learner(existing, "Binti", 9);
        store.save_learner(&reconciled).await.unwrap();

        assert_eq!(
            reconciled.profile.id, original_id,
            "UUID stable across launches"
        );
        assert_eq!(reconciled.profile.age, 9, "age updated to CLI value");
        assert_eq!(
            reconciled.profile.created_at.timestamp(),
            original_created.timestamp(),
            "created_at preserved",
        );
    }

    #[tokio::test]
    async fn cli_name_mismatch_keeps_persisted_name() {
        // Save with name="Binti", call reconcile_persisted_learner with
        // --name="Other" — the SAME helper main() uses — and verify the
        // persisted name stays "Binti". The tracing::warn! / eprintln!
        // emission is intentionally NOT asserted (subscriber capture
        // would over-couple the test); the data invariant is what
        // matters here, and exercising the production helper proves we
        // are testing the actual production branch rather than a stub.
        use primer_core::storage::LearnerStore;
        use primer_storage::SqliteSessionStore;
        use std::sync::Arc;

        let store = Arc::new(SqliteSessionStore::open(std::path::Path::new(":memory:")).unwrap());
        let original = create_learner_with_id(
            Uuid::new_v4(),
            "Binti",
            8,
            primer_core::i18n::Locale::English,
        );
        store.save_learner(&original).await.unwrap();

        let existing = store.load_learner().await.unwrap().expect("learner row");
        let reconciled = reconcile_persisted_learner(existing, "Other", 8);
        store.save_learner(&reconciled).await.unwrap();

        assert_eq!(
            reconciled.profile.name, "Binti",
            "persisted name wins over CLI"
        );

        // Round-trip through the store too — proves the saved row also
        // keeps the persisted name (i.e. the helper didn't mutate name
        // before save_learner committed it).
        let round_trip = store.load_learner().await.unwrap().expect("learner row");
        assert_eq!(round_trip.profile.name, "Binti");
    }

    #[test]
    fn reconcile_persisted_learner_preserves_name_and_id_on_match() {
        // The non-mismatch path: same name should be a pure age/last_active
        // refresh with no warn (covered by absence of stderr in this test).
        let original_id = Uuid::new_v4();
        let original =
            create_learner_with_id(original_id, "Binti", 8, primer_core::i18n::Locale::English);
        let result = reconcile_persisted_learner(original, "Binti", 9);
        assert_eq!(result.profile.name, "Binti");
        assert_eq!(result.profile.id, original_id);
        assert_eq!(result.profile.age, 9);
    }
}
