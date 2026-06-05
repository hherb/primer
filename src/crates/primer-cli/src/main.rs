//! Primer CLI — a text-mode REPL for developing and testing the Socratic
//! dialogue engine without any hardware (no microphone, no speaker, no
//! E Ink display). Just a terminal and a conversation.
//!
//! Usage:
//!   primer                                              # Stub backend (canned responses, no model needed)
//!   primer --backend cloud                              # Anthropic Claude API (default model)
//!   primer --backend cloud --model claude-opus-4-7      # Override the cloud model
//!   primer --backend ollama --model llama3.2            # Local Ollama server
//!   primer --backend qnn --qnn-bundle-dir <path>        # Qualcomm NPU (Android only; --features qnn)
//!   primer --name Binti --age 8                         # Set learner profile
//!   primer --resume <uuid>                              # Resume a past session

#[cfg(feature = "speech")]
mod speech_loop;

#[cfg(all(feature = "macos-native", feature = "macos-native-26"))]
compile_error!(
    "`macos-native` and `macos-native-26` are mutually exclusive — pick one \
     (`macos-native-26` for macOS 26+, `macos-native` for older macOS)"
);

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
    create_learner_with_id, parse_languages, reconcile_persisted_learner, resolve_session_db_path,
    should_show_first_run_banner, verify_resume_locale_match,
};
use primer_extractor::{ConceptExtractor, ExtractorSettings};
use primer_knowledge::SqliteKnowledgeBase;
use primer_pedagogy::DialogueManager;
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(name = "primer", about = "The Primer — a Socratic learning companion")]
// On the portable speech build `--speech` requires at least one TTS asset
// (via the `tts_assets` group below) in addition to `--whisper-model`.
// clap's `required_if_eq` is blind to default values, so the per-tts split
// can't be expressed at parse time when `--tts` defaults to piper; instead
// the group enforces "≥1 asset under --speech" and `validate_speech_assets`
// checks the chosen set is complete at runtime.
#[cfg_attr(
    all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ),
    command(group(
        clap::ArgGroup::new("tts_assets")
            .args(["voice_onnx", "voice_config", "supertonic_dir", "supertonic_voice_style"])
            .multiple(true)
            .required(false)
    ))
)]
struct Cli {
    /// Inference backend: "stub", "cloud", "ollama", "openai-compat",
    /// "qnn", or "llamacpp" (qnn requires `--features qnn` at build time
    /// and targets the Qualcomm NPU on Android; llamacpp requires a
    /// `--features llamacpp*` build).
    #[arg(long, default_value = "stub")]
    backend: String,

    /// Model identifier. For cloud: Anthropic model id (default: claude-sonnet-4-6).
    /// For ollama: local model tag (e.g., "llama3.2", "qwen2.5:7b") — required.
    /// For openai-compat: server-specific model id — required.
    /// For qnn: ignored — the model id is read from `primer-meta.json`
    /// inside the bundle and surfaced as `qnn:<model_id>`.
    /// For llamacpp: filesystem path to the .gguf file — required.
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

    /// Preferred content languages, comma-separated ISO 639-1 codes
    /// ordered by preference (e.g. `--languages de,en`). Open-vocabulary
    /// preference list persisted with the learner, distinct from the bound
    /// `--language` locale: the locale drives prompt-pack / speech /
    /// knowledge dispatch, while this list is documentation-as-data for a
    /// future content-language-hinting feature. When omitted, defaults to
    /// just the `--language` locale (the historical single-language
    /// behaviour). Used verbatim — the locale is not force-prepended, so
    /// `--language de --languages en` stores `["en"]`.
    #[arg(long, value_name = "CSV")]
    languages: Option<String>,

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

    /// Append a custom reasoning-marker pair to strip from model output.
    /// Repeatable: `--reasoning-marker '<think>' '</think>'`. The built-in
    /// defaults (`<think>…</think>`, Gemma4 `<|channel>…<channel|>`) always
    /// apply; this only adds more. Markers are matched as literal text, not
    /// regex. Applies to ollama / openai-compat backends.
    #[arg(long, num_args = 2, value_names = ["OPEN", "CLOSE"], action = clap::ArgAction::Append)]
    reasoning_marker: Vec<String>,

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

    /// Embedder backend for hybrid retrieval. Defaults to `fastembed` on a
    /// build with the `embedding` cargo feature (the default build) and to
    /// `none` on a `--no-default-features` build — so a flagless run does
    /// the right thing for whatever was compiled in and never hard-fails.
    /// `none` disables hybrid retrieval and uses BM25-only; `stub` uses the
    /// in-process deterministic hash embedder (no semantic value, only
    /// useful for testing the hybrid pipeline end-to-end); `fastembed` uses
    /// the BGE-M3 dense embedding model via `fastembed-rs` (~570 MB on first
    /// run; ships in the default `embedding` cargo feature); `ollama` uses Ollama's
    /// `/api/embeddings` (requires the `ollama-embedding` cargo feature and
    /// Ollama running locally); `openai-compat` uses a `/v1/embeddings`
    /// server (requires the `openai-compat-embedding` cargo feature).
    #[cfg_attr(
        feature = "embedding",
        arg(long, value_name = "BACKEND", default_value = "fastembed")
    )]
    #[cfg_attr(
        not(feature = "embedding"),
        arg(long, value_name = "BACKEND", default_value = "none")
    )]
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

    /// Run the voice REPL instead of the text REPL. On builds *without*
    /// `--features macos-native` or `--features macos-native-26` (the
    /// whisper+piper path) requires `--whisper-model`, `--voice-onnx`,
    /// `--voice-config`. On either Apple-native build SFSpeechRecognizer /
    /// SpeechAnalyzer + AVSpeechSynthesizer carry STT and TTS, so those
    /// three flags are not declared (closes #112). Available only when
    /// the binary is built with `--features speech`.
    #[cfg(feature = "speech")]
    #[cfg_attr(
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        )),
        arg(long, requires_all = ["whisper_model", "tts_assets"])
    )]
    #[cfg_attr(
        all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ),
        arg(long)
    )]
    speech: bool,

    /// Path to the whisper.cpp GGML/GGUF model file
    /// (e.g. ~/models/ggml-small.en.bin). Required if --speech.
    /// Not declared on the macOS-native or macOS-native-26 build (#112).
    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[arg(long, value_name = "PATH")]
    whisper_model: Option<PathBuf>,

    /// Path to the Piper voice ONNX file
    /// (e.g. ~/models/voices/en_GB-alba-medium.onnx). Required if --speech.
    /// Not declared on the macOS-native or macOS-native-26 build (#112).
    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[arg(long, value_name = "PATH")]
    voice_onnx: Option<PathBuf>,

    /// Path to the matching Piper voice JSON sidecar
    /// (e.g. ~/models/voices/en_GB-alba-medium.onnx.json). Required if --speech.
    /// Not declared on the macOS-native or macOS-native-26 build (#112).
    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[arg(long, value_name = "PATH")]
    voice_config: Option<PathBuf>,

    /// TTS backend for voice mode: `piper` (default) or `supertonic`.
    /// `supertonic` needs `--features supertonic` at build time plus the
    /// `--supertonic-dir` + `--supertonic-voice-style` asset flags.
    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[arg(long, value_enum, default_value_t = TtsChoice::Piper)]
    tts: TtsChoice,

    /// Supertonic `onnx/` asset directory (the dir holding
    /// duration_predictor.onnx, text_encoder.onnx, etc.). Required when
    /// `--tts supertonic`.
    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[arg(long, value_name = "DIR")]
    supertonic_dir: Option<PathBuf>,

    /// Supertonic voice-style JSON, e.g. voice_styles/F1.json. Required
    /// when `--tts supertonic`.
    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[arg(long, value_name = "FILE")]
    supertonic_voice_style: Option<PathBuf>,

    /// Voice id used as the VoiceProfile.model_id. Must match the file
    /// stem of --voice-onnx (Piper rejects mismatches at session open).
    /// Not declared on the macOS-native or macOS-native-26 build (#112).
    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
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

    /// Path to a QNN bundle directory containing `genie_config.json`,
    /// `primer-meta.json`, and the per-shard context binaries.
    /// Required when `--backend qnn`. Falls back to the
    /// `PRIMER_QNN_BUNDLE_DIR` env var when neither is passed.
    /// Declared only on `--features qnn` so `--help` stays uncluttered
    /// on the default text-REPL build.
    #[cfg(feature = "qnn")]
    #[arg(
        long,
        value_name = "DIR",
        env = "PRIMER_QNN_BUNDLE_DIR",
        required_if_eq("backend", "qnn")
    )]
    qnn_bundle_dir: Option<PathBuf>,

    /// Path to the QAIRT runtime library directory (containing
    /// `libGenie.so`). Optional; defaults to
    /// `<qnn_bundle_dir>/../qairt/lib/aarch64-android/` matching the
    /// AI Hub apps layout. Override when QAIRT is installed elsewhere
    /// (or via `PRIMER_QNN_QAIRT_LIB_DIR`).
    #[cfg(feature = "qnn")]
    #[arg(long, value_name = "DIR", env = "PRIMER_QNN_QAIRT_LIB_DIR")]
    qnn_qairt_lib_dir: Option<PathBuf>,

    /// llama.cpp: number of model layers to offload to GPU. Default:
    /// all layers (-1) when built with a GPU feature, else CPU (0).
    /// Only meaningful with `--backend llamacpp`. Declared only on a
    /// `--features llamacpp*` build so `--help` stays uncluttered on the
    /// default text-REPL build (mirrors the qnn flags).
    #[cfg(feature = "llamacpp")]
    #[arg(long, value_name = "N")]
    llamacpp_gpu_layers: Option<i32>,

    /// llama.cpp: context length (n_ctx). Default: the model's trained
    /// length. Only meaningful with `--backend llamacpp`. Declared only on
    /// a `--features llamacpp*` build (see `--llamacpp-gpu-layers`).
    #[cfg(feature = "llamacpp")]
    #[arg(long, value_name = "N")]
    llamacpp_n_ctx: Option<u32>,
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

/// CLI value for `--tts`. Mirrors `primer_speech::voice_loop::TtsBackend`
/// minus the macOS-native arm (D2: the CLI native build keeps AVSpeech and
/// is compiled separately).
#[cfg(all(
    feature = "speech",
    not(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    ))
))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
enum TtsChoice {
    Piper,
    Supertonic,
}

#[cfg(all(
    feature = "speech",
    not(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    ))
))]
impl From<TtsChoice> for primer_speech::voice_loop::TtsBackend {
    fn from(c: TtsChoice) -> Self {
        match c {
            TtsChoice::Piper => Self::Piper,
            TtsChoice::Supertonic => Self::Supertonic,
        }
    }
}

#[cfg(all(
    feature = "speech",
    not(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    ))
))]
fn validate_speech_assets(
    whisper_model: &Path,
    tts: primer_speech::voice_loop::TtsBackend,
    voice_onnx: Option<&Path>,
    voice_config: Option<&Path>,
    supertonic_dir: Option<&Path>,
    supertonic_voice_style: Option<&Path>,
    voice_id: &str,
) -> primer_core::error::Result<()> {
    use primer_speech::voice_loop::TtsBackend;

    if !whisper_model.exists() {
        return Err(PrimerError::Speech(format!(
            "whisper model not found at {}.\n\
             Download a GGML model from https://huggingface.co/ggerganov/whisper.cpp \
             (e.g. ggml-small.en.bin) and pass --whisper-model.",
            whisper_model.display()
        )));
    }

    match tts {
        TtsBackend::Piper => {
            let voice_onnx = voice_onnx.ok_or_else(|| {
                PrimerError::Speech(
                    "piper TTS requires --voice-onnx (clap should enforce this with --speech)"
                        .to_string(),
                )
            })?;
            let voice_config = voice_config.ok_or_else(|| {
                PrimerError::Speech(
                    "piper TTS requires --voice-config (clap should enforce this with --speech)"
                        .to_string(),
                )
            })?;
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
        }
        TtsBackend::Supertonic => {
            let dir = supertonic_dir.ok_or_else(|| {
                PrimerError::Speech(
                    "supertonic TTS requires --supertonic-dir (clap should enforce this \
                     with --speech)"
                        .to_string(),
                )
            })?;
            let style = supertonic_voice_style.ok_or_else(|| {
                PrimerError::Speech(
                    "supertonic TTS requires --supertonic-voice-style (clap should enforce \
                     this with --speech)"
                        .to_string(),
                )
            })?;
            if !dir.exists() {
                return Err(PrimerError::Speech(format!(
                    "supertonic model dir not found at {}.\n\
                     Download from https://huggingface.co/Supertone/supertonic-3 and pass \
                     --supertonic-dir.",
                    dir.display()
                )));
            }
            if !style.exists() {
                return Err(PrimerError::Speech(format!(
                    "supertonic voice-style not found at {}.\n\
                     Pass --supertonic-voice-style (e.g. voice_styles/F1.json from the \
                     Supertone/supertonic-3 release).",
                    style.display()
                )));
            }
        }
        TtsBackend::MacosNative => {
            // Unreachable on the portable build (the CLI's TtsChoice has no
            // MacosNative arm), but matched exhaustively.
        }
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

/// Emit best-effort startup-time warnings about subsystem-backend
/// combinations under `--backend qnn`:
///
/// - **All-qnn** (classifier / extractor / comprehension either
///   defaulted to the main backend or explicitly set to qnn): every
///   background LLM call serialises through the dialog mutex along
///   with the main chat turn. Functionally correct, but on a
///   memory-constrained device this means classifier work piles up
///   behind a multi-second decode. The warning is informational —
///   nothing is rejected.
/// - **All-stub** (every subsystem explicitly stubbed): the
///   conversation loses classifier-driven features (engagement
///   detection, concept extraction, comprehension depth promotion).
///   This is sometimes a deliberate choice for offline smoke tests;
///   surfaced as a warning, not an error.
///
/// The "cloud-backed subsystem with missing `ANTHROPIC_API_KEY`" case
/// from the plan is already covered structurally by the
/// `build_classifier` / `build_extractor` / `build_comprehension`
/// builders — they call `build_backend("cloud", ...)` which errors
/// when `api_key` is `None`. No extra check needed here.
///
/// Pure inspection of the `Cli` struct — no I/O. Kept as a free
/// function so we can unit-test the decision logic via the small
/// `npu_serialisation_warning` helper below.
fn warn_on_npu_serialisation(cli: &Cli) {
    let decision = npu_serialisation_warning(
        cli.classifier_backend.as_deref(),
        cli.extractor_backend.as_deref(),
        cli.comprehension_backend.as_deref(),
    );
    if let Some(msg) = decision {
        eprintln!("Warning: {msg}");
    }
}

/// Decide whether to warn about NPU serialisation or feature loss
/// given the explicit subsystem-backend overrides. Returns `None`
/// when the configuration is mixed (some NPU, some not) — the most
/// reasonable case, no warning needed.
///
/// Inputs are `Option<&str>` because each subsystem flag defaults to
/// "unset → reuse the main backend". Under `--backend qnn`, "unset"
/// effectively means "qnn".
fn npu_serialisation_warning(
    classifier: Option<&str>,
    extractor: Option<&str>,
    comprehension: Option<&str>,
) -> Option<String> {
    // Resolve each subsystem to its effective backend name under
    // `--backend qnn`: None → "qnn" (inherit), explicit value wins.
    let resolved: [&str; 3] = [
        classifier.unwrap_or("qnn"),
        extractor.unwrap_or("qnn"),
        comprehension.unwrap_or("qnn"),
    ];

    if resolved.iter().all(|&b| b == "qnn") {
        return Some(
            "every subsystem (classifier, extractor, comprehension) is set to qnn — \
             all background LLM work will serialise behind the chat turn through the \
             dialog mutex. Consider --classifier-backend stub or a separate small model."
                .to_string(),
        );
    }
    if resolved.iter().all(|&b| b == "stub") {
        return Some(
            "every subsystem (classifier, extractor, comprehension) is stub — \
             the conversation runs without engagement detection, concept extraction, \
             or comprehension depth promotion. This is fine for smoke tests."
                .to_string(),
        );
    }
    None
}

/// Pair clap's flat `--reasoning-marker OPEN CLOSE` values (a `Vec` of length
/// `2 × N`) into `(open, close)` tuples. A trailing unpaired value is dropped
/// (clap's `num_args = 2` makes that impossible in practice, but be defensive).
fn pair_reasoning_markers(flat: Vec<String>) -> Vec<(String, String)> {
    let mut it = flat.into_iter();
    let mut out = Vec::new();
    while let (Some(open), Some(close)) = (it.next(), it.next()) {
        out.push((open, close));
    }
    out
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
    // Both macOS-native builds (macos-native and macos-native-26) require a
    // current-thread runtime on the OS main thread so that AVSpeechSynthesizer
    // synthesis runs on main (the path that drives NSRunLoop::runUntilDate).
    // See the doc-comment on this function for the full rationale.
    #[cfg(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    ))]
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    #[cfg(not(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    )))]
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
    // For `qnn` the model id is read from `primer-meta.json` inside the
    // bundle and isn't known until after construction — we seed an
    // "unknown" placeholder here and overwrite from `backend.name()`
    // after `build_backend` returns. `cli.model` is intentionally
    // ignored for qnn (documented on the flag).
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
    };

    let backend: Arc<dyn InferenceBackend> =
        match build_backend(&cli.backend, main_model.clone(), &backend_params).await {
            Ok(b) => b,
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
            warn_on_npu_serialisation(&cli);
        }
        "llamacpp" => {
            eprintln!("Using embedded llama.cpp backend with {main_model}.");
        }
        other => {
            eprintln!(
                "Unknown backend: {other}. Use 'stub', 'cloud', 'ollama', 'openai-compat', 'qnn', or 'llamacpp'."
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
            let languages = parse_languages(cli.languages.as_deref(), cli_locale);
            let fresh = create_learner_with_id(id, &cli.name, cli.age, cli_locale, languages);
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
        // On either macOS-native build the four whisper/piper flags are not
        // declared at all (#112) — SFSpeechRecognizer / SpeechAnalyzer +
        // AVSpeechSynthesizer carry STT and TTS and the corresponding
        // `SpeechLoopConfig` fields are likewise cfg-gated out.
        #[cfg(not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        )))]
        let cfg = {
            let whisper_model = cli
                .whisper_model
                .as_ref()
                .expect("clap requires whisper_model with --speech");
            let tts: primer_speech::voice_loop::TtsBackend = cli.tts.into();
            validate_speech_assets(
                whisper_model,
                tts,
                cli.voice_onnx.as_deref(),
                cli.voice_config.as_deref(),
                cli.supertonic_dir.as_deref(),
                cli.supertonic_voice_style.as_deref(),
                &cli.voice,
            )?;
            speech_loop::SpeechLoopConfig {
                whisper_model: whisper_model.clone(),
                voice_onnx: cli.voice_onnx.clone(),
                voice_config: cli.voice_config.clone(),
                voice_id: cli.voice.clone(),
                tts,
                supertonic_dir: cli.supertonic_dir.clone(),
                supertonic_voice_style: cli.supertonic_voice_style.clone(),
                mic_silence_ms: cli.mic_silence_ms,
                verbose: cli.verbose,
                locale: cli_locale,
            }
        };
        #[cfg(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))]
        let cfg = speech_loop::SpeechLoopConfig {
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

    // ─── --speech requires_all gating (issue #112) ──────────────────────
    //
    // On the macOS-native build (`--features speech,macos-native` on
    // macOS) the whisper/piper asset flags are not declared at all —
    // SFSpeechRecognizer + AVSpeechSynthesizer carry the STT and TTS
    // halves of the loop and the corresponding clap fields disappear.
    // On every other speech build the existing `requires_all` contract
    // still applies, because `build_local_backends` needs all three
    // model paths to open whisper + piper.

    #[cfg(all(
        feature = "speech",
        all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        )
    ))]
    #[test]
    fn speech_alone_parses_on_macos_native_without_whisper_piper_flags() {
        let result = Cli::try_parse_from(["primer", "--speech"]);
        assert!(
            result.is_ok(),
            "expected --speech alone to parse on macos-native/macos-native-26; got: {result:?}"
        );
    }

    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[test]
    fn speech_alone_still_rejected_off_macos_native() {
        let result = Cli::try_parse_from(["primer", "--speech"]);
        assert!(
            result.is_err(),
            "expected clap to reject --speech without whisper/piper flags on \
             non-macos-native builds; got: {result:?}"
        );
    }

    // ─── --tts piper|supertonic conditional requirements (issue #170) ────

    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[test]
    fn plain_repl_without_speech_parses_without_voice_assets() {
        // The default --tts is piper, but with no --speech the voice asset
        // flags must NOT be required (regression guard for the
        // required_if_eq_all gating).
        let res = Cli::try_parse_from(["primer"]);
        assert!(
            res.is_ok(),
            "plain REPL must parse with no speech flags: {res:?}"
        );
    }

    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[test]
    fn speech_piper_default_requires_piper_assets() {
        let res = Cli::try_parse_from(["primer", "--speech", "--whisper-model", "/m.bin"]);
        assert!(
            res.is_err(),
            "--speech with default --tts piper still needs --voice-onnx/--voice-config"
        );
    }

    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[test]
    fn speech_supertonic_requires_supertonic_assets() {
        let res = Cli::try_parse_from([
            "primer",
            "--speech",
            "--whisper-model",
            "/m.bin",
            "--tts",
            "supertonic",
        ]);
        assert!(
            res.is_err(),
            "--tts supertonic needs --supertonic-dir/--supertonic-voice-style"
        );
    }

    /// Runtime backstop: even if the `tts_assets` ArgGroup is satisfied by the
    /// wrong assets (clap can't express the per-tts split — it only knows
    /// "≥1 asset"), `validate_speech_assets` rejects a Supertonic session with
    /// no supertonic dir, naming the missing flag. Uses the test binary's own
    /// path as the (always-existing) whisper stand-in so validation gets past
    /// the whisper check and reaches the Supertonic arm.
    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[test]
    fn validate_rejects_supertonic_without_dir() {
        let existing = std::env::current_exe().expect("test binary path exists");
        let err = validate_speech_assets(
            &existing,
            primer_speech::voice_loop::TtsBackend::Supertonic,
            None,
            None,
            None, // supertonic_dir missing
            None,
            "ignored-voice-id",
        )
        .expect_err("supertonic with no dir must fail validation");
        let msg = format!("{err}");
        assert!(
            msg.contains("supertonic-dir"),
            "error must name the missing flag: {msg}"
        );
    }

    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[test]
    fn speech_supertonic_parses_with_assets() {
        let res = Cli::try_parse_from([
            "primer",
            "--speech",
            "--whisper-model",
            "/m.bin",
            "--tts",
            "supertonic",
            "--supertonic-dir",
            "/sup/onnx",
            "--supertonic-voice-style",
            "/sup/voice_styles/F1.json",
        ]);
        assert!(res.is_ok(), "supertonic with assets should parse: {res:?}");
    }

    #[cfg(all(
        feature = "speech",
        not(all(
            target_os = "macos",
            any(feature = "macos-native", feature = "macos-native-26")
        ))
    ))]
    #[test]
    fn speech_piper_parses_with_assets() {
        let res = Cli::try_parse_from([
            "primer",
            "--speech",
            "--whisper-model",
            "/m.bin",
            "--voice-onnx",
            "/v.onnx",
            "--voice-config",
            "/v.onnx.json",
        ]);
        assert!(res.is_ok(), "piper with assets should parse: {res:?}");
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

    // ─── NPU serialisation warning (Phase 1.2 step 1.2.4) ────────────────

    #[test]
    fn warn_when_every_subsystem_inherits_main_qnn() {
        // No overrides → each subsystem inherits the main backend.
        // Under --backend qnn this means everything runs on the NPU
        // and serialises through the dialog mutex.
        let w = npu_serialisation_warning(None, None, None);
        assert!(w.is_some(), "expected a warning; got None");
        let msg = w.unwrap();
        assert!(
            msg.contains("serialise") || msg.contains("serialize"),
            "expected serialisation hint; got: {msg}"
        );
    }

    #[test]
    fn warn_when_every_subsystem_is_explicitly_qnn() {
        // Equivalent semantically to "all None under --backend qnn"
        // (both resolve to `"qnn"` per the inherit-the-main-backend
        // rule), but pinned separately because a future refactor
        // that handled the explicit case differently from the
        // inherit case would silently break this contract.
        let w = npu_serialisation_warning(Some("qnn"), Some("qnn"), Some("qnn"));
        assert!(w.is_some(), "expected a warning; got None");
        let msg = w.unwrap();
        assert!(
            msg.contains("serialise") || msg.contains("serialize"),
            "expected serialisation hint; got: {msg}"
        );
    }

    #[test]
    fn warn_when_every_subsystem_is_stub() {
        // All-stub means the conversation runs without classifier-
        // driven features. Deliberate for smoke tests, but worth
        // calling out so a fresh user doesn't think it's broken.
        let w = npu_serialisation_warning(Some("stub"), Some("stub"), Some("stub"));
        assert!(w.is_some(), "expected a warning; got None");
        let msg = w.unwrap();
        assert!(msg.contains("stub"), "expected stub hint; got: {msg}");
    }

    #[test]
    fn no_warning_for_mixed_subsystem_config() {
        // One subsystem stubbed, two inherited → reasonable shape,
        // no warning. Specifically: classifier-on-stub is the
        // canonical "let me focus the NPU on chat" configuration.
        let w = npu_serialisation_warning(Some("stub"), None, None);
        assert!(w.is_none(), "mixed config should not warn; got: {w:?}");
    }

    #[test]
    fn no_warning_when_subsystems_use_external_backend() {
        // Classifier on cloud, others inherit qnn → mixed shape.
        let w = npu_serialisation_warning(Some("cloud"), None, None);
        assert!(
            w.is_none(),
            "cloud-classifier config should not warn; got: {w:?}"
        );
    }

    // ─── --backend qnn clap parse acceptance ─────────────────────────────

    #[cfg(feature = "qnn")]
    #[test]
    fn qnn_backend_requires_qnn_bundle_dir_at_parse() {
        // `--backend qnn` without `--qnn-bundle-dir` (and without the
        // `PRIMER_QNN_BUNDLE_DIR` env var) is rejected by clap before
        // any backend construction is attempted.
        // Use `std::env::remove_var` indirectly by capturing the
        // missing-env scenario: clap's required_if_eq fires when no
        // value source resolved a value. We can't reliably scrub the
        // env from a test (it's process-wide), so this test only
        // asserts the clap-required path. Running this test with
        // PRIMER_QNN_BUNDLE_DIR set in the environment will produce
        // an Ok parse — that's an acceptable degenerate case.
        if std::env::var_os("PRIMER_QNN_BUNDLE_DIR").is_some() {
            // Env var is set externally — the env fallback applies and
            // the required_if_eq check is satisfied. Print the skip so
            // a passing-but-skipped result is visible under `--nocapture`
            // rather than indistinguishable from a real green.
            eprintln!(
                "[skip] qnn_backend_requires_qnn_bundle_dir_at_parse: \
                 PRIMER_QNN_BUNDLE_DIR is set; clap's env fallback satisfies \
                 required_if_eq so we cannot assert the rejection path."
            );
            return;
        }
        let result = Cli::try_parse_from(["primer", "--backend", "qnn"]);
        assert!(
            result.is_err(),
            "expected clap to reject --backend qnn without --qnn-bundle-dir; got: {result:?}"
        );
    }

    #[cfg(feature = "qnn")]
    #[test]
    fn qnn_backend_with_bundle_dir_parses() {
        // Happy path: clap accepts `--backend qnn --qnn-bundle-dir <p>`.
        // Construction itself happens later in async_main and is the
        // engine's responsibility — this test pins the parse contract.
        //
        // Env-var defensive skip: clap's `env = "..."` resolves the
        // optional `--qnn-qairt-lib-dir` from `PRIMER_QNN_QAIRT_LIB_DIR`
        // if set in the test runner's environment, which would make
        // the `cli.qnn_qairt_lib_dir.is_none()` assertion fail. Skip
        // visibly so a developer with QAIRT installed locally doesn't
        // chase a misleading red.
        if std::env::var_os("PRIMER_QNN_QAIRT_LIB_DIR").is_some() {
            eprintln!(
                "[skip] qnn_backend_with_bundle_dir_parses: PRIMER_QNN_QAIRT_LIB_DIR is set; \
                 clap's env fallback would populate qnn_qairt_lib_dir."
            );
            return;
        }
        let cli = Cli::try_parse_from([
            "primer",
            "--backend",
            "qnn",
            "--qnn-bundle-dir",
            "/tmp/bundle",
        ])
        .expect("expected --backend qnn --qnn-bundle-dir to parse");
        assert_eq!(cli.backend, "qnn");
        assert_eq!(
            cli.qnn_bundle_dir.as_deref(),
            Some(Path::new("/tmp/bundle"))
        );
        assert!(
            cli.qnn_qairt_lib_dir.is_none(),
            "--qnn-qairt-lib-dir should default to None (engine resolves it)"
        );
    }

    #[cfg(feature = "qnn")]
    #[test]
    fn qnn_backend_accepts_optional_qairt_lib_dir() {
        let cli = Cli::try_parse_from([
            "primer",
            "--backend",
            "qnn",
            "--qnn-bundle-dir",
            "/tmp/bundle",
            "--qnn-qairt-lib-dir",
            "/opt/qairt/lib/aarch64-android",
        ])
        .expect("expected --qnn-qairt-lib-dir to be accepted alongside --qnn-bundle-dir");
        assert_eq!(
            cli.qnn_qairt_lib_dir.as_deref(),
            Some(Path::new("/opt/qairt/lib/aarch64-android"))
        );
    }

    #[cfg(feature = "qnn")]
    #[test]
    fn qnn_backend_compatible_with_no_persist_at_parse() {
        // --no-persist conflicts with --resume and --session-db, but
        // --backend qnn is orthogonal. Pin the compatibility so a
        // future conflicts-with bug fails this test instead of leaking
        // to runtime.
        let result = Cli::try_parse_from([
            "primer",
            "--backend",
            "qnn",
            "--qnn-bundle-dir",
            "/tmp/bundle",
            "--no-persist",
        ]);
        assert!(
            result.is_ok(),
            "expected --backend qnn + --no-persist to parse; got: {result:?}"
        );
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

#[cfg(test)]
mod reasoning_marker_tests {
    use super::Cli;
    use super::pair_reasoning_markers;
    use clap::Parser;

    #[test]
    fn pairs_flat_args_into_tuples() {
        let flat = vec![
            "<a>".to_string(),
            "</a>".to_string(),
            "<b>".to_string(),
            "</b>".to_string(),
        ];
        assert_eq!(
            pair_reasoning_markers(flat),
            vec![
                ("<a>".to_string(), "</a>".to_string()),
                ("<b>".to_string(), "</b>".to_string()),
            ]
        );
    }

    #[test]
    fn empty_is_empty() {
        assert_eq!(
            pair_reasoning_markers(vec![]),
            Vec::<(String, String)>::new()
        );
    }

    #[test]
    fn odd_trailing_value_is_dropped() {
        // clap's num_args=2 makes odd counts impossible in practice, but the
        // helper must not panic if handed one.
        let flat = vec!["<a>".to_string(), "</a>".to_string(), "<stray>".to_string()];
        assert_eq!(
            pair_reasoning_markers(flat),
            vec![("<a>".to_string(), "</a>".to_string())]
        );
    }

    #[test]
    fn cli_parses_repeated_reasoning_marker_flags_into_pairs() {
        // Two repeated occurrences → a flat Vec of 4, paired into 2 tuples.
        let cli = Cli::parse_from([
            "primer",
            "--reasoning-marker",
            "<a>",
            "</a>",
            "--reasoning-marker",
            "<b>",
            "</b>",
        ]);
        assert_eq!(
            pair_reasoning_markers(cli.reasoning_marker),
            vec![
                ("<a>".to_string(), "</a>".to_string()),
                ("<b>".to_string(), "</b>".to_string()),
            ]
        );
    }

    #[test]
    fn cli_without_reasoning_marker_flag_is_empty() {
        let cli = Cli::parse_from(["primer"]);
        assert!(pair_reasoning_markers(cli.reasoning_marker).is_empty());
    }
}

#[cfg(test)]
mod embedder_backend_default_tests {
    use super::*;
    use clap::Parser;

    /// On a build with the `embedding` feature (the default), a flagless
    /// invocation defaults to hybrid retrieval via fastembed.
    #[cfg(feature = "embedding")]
    #[test]
    fn default_is_fastembed_with_embedding_feature() {
        let cli = Cli::try_parse_from(["primer", "--name", "Ada", "--age", "9"]).unwrap();
        assert_eq!(cli.embedder_backend, "fastembed");
    }

    /// On a `--no-default-features` build (embedding off), the default
    /// stays BM25-only so the binary never hard-fails on a flagless run.
    #[cfg(not(feature = "embedding"))]
    #[test]
    fn default_is_none_without_embedding_feature() {
        let cli = Cli::try_parse_from(["primer", "--name", "Ada", "--age", "9"]).unwrap();
        assert_eq!(cli.embedder_backend, "none");
    }

    /// An explicit value always overrides the default, regardless of which
    /// feature build is active. `stub` is used because it is a valid value
    /// in BOTH build configurations (no cargo feature required) and differs
    /// from either feature-aware default (`fastembed` / `none`), so the
    /// assertion proves a real override on every build.
    #[test]
    fn explicit_value_overrides_default() {
        let cli = Cli::try_parse_from([
            "primer",
            "--name",
            "Ada",
            "--age",
            "9",
            "--embedder-backend",
            "stub",
        ])
        .unwrap();
        assert_eq!(cli.embedder_backend, "stub");
    }
}
