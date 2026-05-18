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

use clap::Parser;
use primer_classifier::{ClassifierSettings, EngagementClassifier};
use primer_core::config::PedagogyConfig;
use primer_core::error::PrimerError;
use primer_core::i18n::{Locale, render_inference_error};
use primer_core::inference::InferenceBackend;
use primer_core::knowledge::KnowledgeBase;
use primer_core::storage::{LearnerStore, SessionStore};
use primer_engine::{
    BackendParams, IN_MEMORY, build_backend, build_classifier, build_comprehension,
    build_extractor, build_fastembed_embedder, build_ollama_embedder, build_openai_compat_embedder,
    create_learner_with_id, reconcile_persisted_learner, resolve_session_db_path,
    should_show_first_run_banner, verify_resume_locale_match,
};
use primer_extractor::{ConceptExtractor, ExtractorSettings};
use primer_knowledge::SqliteKnowledgeBase;
use primer_pedagogy::DialogueManager;
use uuid::Uuid;

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

    /// OpenAI-compatible server URL (used when --backend openai-compat).
    /// Works with oMLX, LM Studio, vLLM, llama.cpp --server, etc.
    #[arg(
        long,
        default_value = "http://localhost:8000",
        env = "OPENAI_COMPAT_URL"
    )]
    openai_compat_url: String,

    /// API key for OpenAI-compatible servers that require auth
    /// (Together, Groq, OpenRouter). Local servers like oMLX/LM Studio
    /// typically don't need one.
    #[arg(long, env = "OPENAI_COMPAT_API_KEY")]
    openai_compat_api_key: Option<String>,

    /// Child's name (for the learner profile).
    #[arg(long, default_value = primer_core::consts::learner::DEFAULT_NAME)]
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

    /// Maximum number of overdue/due concepts to inject into the system
    /// prompt per turn for spaced-repetition vocabulary review. Higher =
    /// more review pressure, more prompt bloat. Must be ≥ 1; 0 is rejected
    /// at parse time. Defaults to
    /// `primer_core::consts::vocab::DEFAULT_VOCAB_MAX_PER_PROMPT` (4).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    vocab_max_per_prompt: Option<u64>,

    /// Minutes between break-suggestion nudges. The Primer phrases the
    /// suggestion in-character; the child can keep going. Default 30.
    /// Must be >= 1.
    #[arg(
        long,
        default_value_t = primer_core::consts::break_suggest::DEFAULT_INTERVAL_MINUTES,
        value_parser = clap::value_parser!(u32).range(1..),
    )]
    session_break_after_mins: u32,

    /// Embedder backend for hybrid retrieval. `none` (the default)
    /// disables hybrid retrieval and uses BM25-only — the same behaviour
    /// as before this flag existed; `stub` uses the in-process
    /// deterministic hash embedder (no semantic value, only useful for
    /// testing the hybrid pipeline end-to-end); `fastembed` uses the
    /// BGE-M3 dense embedding model via `fastembed-rs` (~570 MB on first
    /// run; requires the `embedding` cargo feature); `ollama` uses
    /// Ollama's `/api/embeddings` (requires the `ollama-embedding`
    /// cargo feature and Ollama running locally).
    #[arg(long, value_name = "BACKEND", default_value = "none")]
    embedder_backend: String,

    /// Model name for the embedder. With `--embedder-backend fastembed`,
    /// defaults to `bge-m3` and accepts other fastembed model ids
    /// (e.g. `bge-small-en-v1.5`). With `--embedder-backend ollama`,
    /// defaults to `nomic-embed-text`.
    #[arg(long, value_name = "NAME")]
    embedder_model: Option<String>,

    /// Override the Ollama endpoint used for `--embedder-backend ollama`.
    /// Defaults to `http://localhost:11434`. Has no effect on
    /// `stub`/`fastembed` backends.
    #[arg(long, value_name = "URL")]
    embedder_ollama_url: Option<String>,

    /// Override the URL for `--embedder-backend openai-compat`.
    /// Defaults to `--openai-compat-url` if set, otherwise
    /// `http://localhost:8000`.
    #[arg(long, value_name = "URL")]
    embedder_openai_compat_url: Option<String>,

    /// Model name for `--embedder-backend openai-compat` (required).
    #[arg(long, value_name = "NAME")]
    embedder_openai_compat_model: Option<String>,

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

#[cfg(feature = "speech")]
fn validate_speech_assets(
    whisper_model: &Path,
    voice_onnx: &Path,
    voice_config: &Path,
    voice_id: &str,
) -> primer_core::error::Result<()> {
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

    run_tokio_on_main()
}

/// Build the tokio runtime that drives `async_main()`.
///
/// **Non-macos-native (default):** multi-thread runtime on the OS main
/// thread — historical behaviour. tokio's worker pool handles
/// `tokio::spawn` background tasks (classifier, extractor,
/// comprehension, embedding) in parallel with the dialogue's main turn.
///
/// **macos-native:** current-thread runtime on the OS main thread. The
/// voice loop's `MacosTtsSession::push_text` is a synchronous fn that
/// dispatches AVSpeechSynthesizer synthesis on whatever thread it's
/// called from. With current-thread tokio on main, all awaits stay on
/// main, so `push_text` runs on main and takes the main-thread
/// synthesis path ([`primer_speech::macos::tts::synthesize_to_chunks_main_thread`]) —
/// the path that drives `NSRunLoop::runUntilDate` from inside the call
/// and drains the GCD main queue (AVFoundation primes the
/// queue→runloop integration on its first AVSpeechSynthesizer
/// instantiation; nobody else needs to).
///
/// The background-thread synthesis path (worker → `dispatch_async_f`
/// to the main queue → wait on `dispatch_semaphore`) is **not used**
/// on macos-native CLI: it would only work if AppKit's
/// `NSApplicationMain` (or an explicit `dispatch_main()`) had wired
/// the main queue to the main run loop, and we can't pull AppKit
/// into a text-mode CLI.
///
/// Cost: background tokio tasks (classifier, extractor, etc.) are
/// blocked while `push_text` runs (typically 1–2 s per phrase). They
/// catch up at the start of the next turn via
/// `await_pending_post_response`, so the user-visible behaviour is
/// unchanged.
fn run_tokio_on_main() -> anyhow::Result<()> {
    #[cfg(all(target_os = "macos", feature = "macos-native"))]
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    #[cfg(not(all(target_os = "macos", feature = "macos-native")))]
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
        "openai-compat" => cli.model.clone().unwrap_or_else(|| {
            eprintln!("Error: --model required for openai-compat backend.");
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
        openai_compat_url: cli.openai_compat_url.clone(),
        openai_compat_api_key: cli.openai_compat_api_key.clone(),
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
        "openai-compat" => eprintln!(
            "Using openai-compat backend at {} with model {main_model}.",
            cli.openai_compat_url
        ),
        other => {
            eprintln!(
                "Unknown backend: {other}. Use 'stub', 'cloud', 'ollama', or 'openai-compat'."
            );
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
    // Auto-seed-on-empty: if the KB has no passages and a seed JSONL is
    // discoverable for this locale, load it. Returns Ok(None) and logs at
    // info level when no seed file is found — empty KB is a valid runtime
    // state and the rest of the system handles it gracefully.
    if let Some(stats) = primer_kb_load::auto_seed_if_empty(&knowledge, cli_locale).await? {
        tracing::info!(
            target = "primer::startup",
            inserted = stats.inserted,
            sources = stats.sources_seen,
            "auto-seeded knowledge base for locale {}",
            cli_locale.pack_id()
        );
    }

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
        match primer_storage::SqliteSessionStore::open_for_locale(&session_path, cli_locale) {
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

    // Locale guard for --resume: error before any new turn could insert a
    // mistagged concept. Done HERE rather than at session-store open time
    // so the check sees the actual learner row that --resume will hydrate.
    if let Some(resume_id) = cli.resume {
        if let Err(msg) = verify_resume_locale_match(cli_locale, learner.profile.locale, resume_id)
        {
            eprintln!("Error: {msg}");
            std::process::exit(1);
        }
    }

    // Pedagogy config.
    let pedagogy_config = PedagogyConfig {
        break_suggest_after_minutes: cli.session_break_after_mins,
        ..PedagogyConfig::default()
    };

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

    // ─── Embedder ────────────────────────────────────────────────────
    //
    // `none` (the default) skips embedder construction entirely so the
    // dialogue manager runs BM25-only retrieval — the pre-Phase-0.2.5
    // behaviour. `stub` constructs a deterministic hash embedder useful
    // only for testing the hybrid pipeline; with no semantic signal it
    // dilutes BM25 with noise, so it is not the production default.
    // `fastembed` and `ollama` need their respective cargo features;
    // if missing, the dispatch helpers return Err and the CLI exits
    // with the message (the GUI surfaces the same Err inline).
    //
    // Real-backend construction failures fall back to BM25-only with a
    // tracing warn — the conversation still works, which is strictly
    // better than refusing to start.
    let embedder: Option<Arc<dyn primer_core::embedder::Embedder>> = match cli
        .embedder_backend
        .as_str()
    {
        "none" => None,
        "stub" => Some(Arc::new(primer_embedding::StubEmbedder::new()) as _),
        "fastembed" => match build_fastembed_embedder(cli.embedder_model.as_deref()) {
            Ok(opt) => opt,
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        },
        "ollama" => match build_ollama_embedder(
            cli.embedder_ollama_url.as_deref(),
            cli.embedder_model.as_deref(),
        )
        .await
        {
            Ok(opt) => opt,
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        },
        "openai-compat" => {
            let url = cli
                .embedder_openai_compat_url
                .as_deref()
                .or(Some(cli.openai_compat_url.as_str()));
            match build_openai_compat_embedder(
                url,
                cli.embedder_openai_compat_model
                    .as_deref()
                    .or(cli.embedder_model.as_deref()),
                cli.openai_compat_api_key.clone(),
            )
            .await
            {
                Ok(opt) => opt,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }
        other => {
            eprintln!(
                "Error: unknown --embedder-backend {other:?}; expected one of none, stub, fastembed, ollama, openai-compat"
            );
            std::process::exit(1);
        }
    };

    // ─── Dialogue manager ────────────────────────────────────────────

    let stores = primer_pedagogy::DialogueManagerStores {
        session: Some(Arc::clone(&session_store) as Arc<dyn SessionStore>),
        learner: Some(Arc::clone(&session_store) as Arc<dyn LearnerStore>),
    };
    let vocab_settings = primer_pedagogy::VocabSettings {
        max_per_prompt: cli
            .vocab_max_per_prompt
            .map(|n| n as usize)
            .unwrap_or(primer_core::consts::vocab::DEFAULT_VOCAB_MAX_PER_PROMPT),
    };
    let subsystems = primer_pedagogy::DialogueManagerSubsystems {
        classifier,
        classifier_settings,
        extractor,
        extractor_settings,
        comprehension,
        comprehension_settings,
        vocab_settings,
        embedder: embedder.clone(),
    };
    let knowledge_arc: Arc<dyn KnowledgeBase> = Arc::new(knowledge);
    let mut dm = DialogueManager::new(
        learner,
        Arc::clone(&backend),
        Arc::clone(&knowledge_arc),
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
            locale: cli_locale,
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

        print!("{}: ", dm.learner.profile.name);
        stdout.flush()?;
    }

    Ok(())
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

    // ─── --vocab-max-per-prompt parse tests ─────────────────────────────

    #[test]
    fn parses_vocab_max_per_prompt_explicit_value() {
        let cli = Cli::try_parse_from(["primer", "--vocab-max-per-prompt", "6"]).unwrap();
        assert_eq!(cli.vocab_max_per_prompt, Some(6));
    }

    #[test]
    fn vocab_max_per_prompt_defaults_to_none_when_not_passed() {
        let cli = Cli::try_parse_from(["primer"]).unwrap();
        assert_eq!(cli.vocab_max_per_prompt, None);
    }

    #[test]
    fn vocab_max_per_prompt_zero_is_rejected_at_parse() {
        // 0 is a valid usize value but is meaningless for this flag —
        // clap's range(1..) rejects it with a clear error.
        let result = Cli::try_parse_from(["primer", "--vocab-max-per-prompt", "0"]);
        assert!(result.is_err(), "0 should be rejected; got: {result:?}");
    }
}

#[cfg(test)]
mod break_suggest_flag_tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn explicit_value_overrides_default() {
        let cli = Cli::try_parse_from([
            "primer",
            "--name",
            "Ada",
            "--age",
            "9",
            "--session-break-after-mins",
            "15",
        ])
        .unwrap();
        assert_eq!(cli.session_break_after_mins, 15);
    }

    #[test]
    fn default_is_30() {
        let cli = Cli::try_parse_from(["primer", "--name", "Ada", "--age", "9"]).unwrap();
        assert_eq!(
            cli.session_break_after_mins,
            primer_core::consts::break_suggest::DEFAULT_INTERVAL_MINUTES,
        );
    }

    #[test]
    fn zero_is_rejected() {
        let result = Cli::try_parse_from([
            "primer",
            "--name",
            "Ada",
            "--age",
            "9",
            "--session-break-after-mins",
            "0",
        ]);
        assert!(result.is_err(), "0 should be rejected by the value parser");
    }
}
