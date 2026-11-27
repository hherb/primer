//! The `primer` command line: every flag the REPL accepts. Split out of
//! `main.rs` (behaviour-preserving); `main.rs` re-exports it as
//! `crate::Cli`.

use std::path::PathBuf;

use clap::Parser;
use uuid::Uuid;

#[cfg(all(
    feature = "speech",
    not(all(
        target_os = "macos",
        any(feature = "macos-native", feature = "macos-native-26")
    ))
))]
use crate::cli_support::TtsChoice;
#[cfg(feature = "speech")]
use crate::cli_support::parse_mic_silence_ms;

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
pub(crate) struct Cli {
    /// Inference backend: "stub", "cloud", "ollama", "openai-compat",
    /// "deepseek", "qnn", or "llamacpp" (qnn requires `--features qnn` at build time
    /// and targets the Qualcomm NPU on Android; llamacpp requires a
    /// `--features llamacpp*` build).
    #[arg(long, default_value = "stub")]
    pub(crate) backend: String,

    /// Model identifier. For cloud: Anthropic model id (default: claude-sonnet-4-6).
    /// For ollama: local model tag (e.g., "llama3.2", "qwen2.5:7b") — required.
    /// For openai-compat: server-specific model id — required.
    /// For deepseek: DeepSeek model id (default: deepseek-flash).
    /// For qnn: ignored — the model id is read from `primer-meta.json`
    /// inside the bundle and surfaced as `qnn:<model_id>`.
    /// For llamacpp: filesystem path to the .gguf file — required.
    #[arg(long)]
    pub(crate) model: Option<String>,

    /// Opt-in fallback inference backend: "stub", "cloud", "ollama",
    /// "openai-compat", or "deepseek". When set, a failing/unavailable primary `--backend`
    /// falls back to this one at startup and before any token streams (never
    /// mid-stream). Absent ⇒ no fallback ⇒ local-only. Setting cloud or
    /// deepseek here is the explicit consent to send a child's turn to that
    /// cloud provider on fallback.
    #[arg(long)]
    pub(crate) fallback_backend: Option<String>,

    /// Model for `--fallback-backend`. For cloud and deepseek, defaults to
    /// that provider's standard model; for ollama/openai-compat a model is required; stub ignores
    /// it.
    #[arg(long)]
    pub(crate) fallback_model: Option<String>,

    /// Inference router mode (Phase 1.3): "local-only" (default; never uses the
    /// cloud — the privacy default), "cloud-preferred" (always try the
    /// secondary first), or "hybrid" (route complex turns to the secondary).
    /// Routing modes require --fallback-backend (the secondary leg).
    #[arg(long, default_value = "local-only")]
    pub(crate) router_mode: String,

    /// Phase 1.3 latency-aware routing budget in milliseconds. Absent ⇒ latency
    /// routing OFF (the default). Only takes effect with `--router-mode hybrid`
    /// AND `--fallback-backend` set: when the local (primary) leg's recent
    /// time-to-first-token exceeds this budget, complex-enough turns are nudged
    /// to the secondary. Set the real value from your accelerator's bench
    /// numbers. Must be ≥ 1 (0 is rejected — that would mean "always nudge",
    /// which is what omitting the flag-plus-`local-only` is for); mirrors the
    /// GUI's `min="1"`.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub(crate) primary_ttft_budget_ms: Option<u64>,

    /// Ollama server URL (used when --backend ollama).
    #[arg(long, default_value = "http://localhost:11434")]
    pub(crate) ollama_url: String,

    /// OpenAI-compatible server URL (used when --backend openai-compat).
    /// Works with oMLX, LM Studio, vLLM, llama.cpp --server, etc.
    #[arg(
        long,
        default_value = "http://localhost:8000",
        env = "OPENAI_COMPAT_URL"
    )]
    pub(crate) openai_compat_url: String,

    /// API key for OpenAI-compatible servers that require auth
    /// (Together, Groq, OpenRouter). Local servers like oMLX/LM Studio
    /// typically don't need one.
    #[arg(long, env = "OPENAI_COMPAT_API_KEY", hide_env_values = true)]
    pub(crate) openai_compat_api_key: Option<String>,

    /// DeepSeek API key (used when --backend or --fallback-backend is
    /// deepseek).
    #[arg(long, env = "DEEPSEEK_API_KEY", hide_env_values = true)]
    pub(crate) deepseek_api_key: Option<String>,

    /// Child's name (for the learner profile).
    #[arg(long, default_value = primer_core::consts::learner::DEFAULT_NAME)]
    pub(crate) name: String,

    /// Child's age in years.
    #[arg(long, default_value_t = 8)]
    pub(crate) age: u8,

    /// Locale (BCP-47 short pack id, e.g. "en"). Selects the prompt
    /// pack and (in future) the speech pipeline + per-locale knowledge
    /// index. Phase 0.1 ships only "en"; passing an unknown id is a
    /// hard error at startup. Persisted with the learner — a returning
    /// child does not re-specify their locale each session.
    #[arg(long, default_value = "en")]
    pub(crate) language: String,

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
    pub(crate) languages: Option<String>,

    /// Path to knowledge base SQLite file (seeded when empty; never
    /// embedded at startup — use `primer-kb-load --reembed`). If omitted:
    /// with an embedder, an embedded cache under `~/.primer/kb/`, one per
    /// model and seed-corpus folder; without one (or with `--no-persist`),
    /// an in-memory database.
    #[arg(long)]
    pub(crate) knowledge_db: Option<PathBuf>,

    /// Path to session database SQLite file.
    /// If omitted, defaults to `~/.primer/<slug-of-name>.db` (per-learner
    /// file, created if missing). Pass an explicit path only when you
    /// want a non-default location.
    #[arg(long)]
    pub(crate) session_db: Option<PathBuf>,

    /// Resume an existing session by UUID. Read from `--session-db`
    /// (default `~/.primer/<name>.db`). Errors if the file doesn't
    /// exist or no session with that id is stored. Works with any
    /// backend including stub: historical turns provide context, new
    /// turns get backend-appropriate responses.
    #[arg(long, value_name = "UUID", conflicts_with = "no_persist")]
    pub(crate) resume: Option<Uuid>,

    /// Run the session in-memory only — nothing is written to disk
    /// and the conversation evaporates on exit. Useful for quick
    /// experiments and tests; the per-learner default-path persistence
    /// stays out of the way. With an embedder the in-memory knowledge
    /// base is re-embedded on every start (minutes with BGE-M3; add
    /// `--embedder-backend none` to skip it). Mutually exclusive with
    /// `--resume` and `--session-db`.
    #[arg(long, conflicts_with_all = ["session_db", "resume"])]
    pub(crate) no_persist: bool,

    /// Anthropic API key (for cloud backend).
    #[arg(long, env = "ANTHROPIC_API_KEY", hide_env_values = true)]
    pub(crate) api_key: Option<String>,

    /// Backend used for the engagement classifier. Defaults to the same
    /// backend used for the chat (--backend); `stub` forces deterministic
    /// classification regardless of main backend.
    #[arg(long)]
    pub(crate) classifier_backend: Option<String>,

    /// Model used for the engagement classifier. Defaults to the same
    /// model used for the chat (--model). Useful for haiku-as-classifier
    /// + sonnet-as-chat configurations on bigger machines.
    #[arg(long)]
    pub(crate) classifier_model: Option<String>,

    /// Maximum time to block awaiting the previous turn's classification
    /// before the next intent decision. Defaults to
    /// `primer_classifier::consts::DEFAULT_BLOCKING_TIMEOUT_MS`.
    #[arg(long, default_value_t = primer_classifier::consts::DEFAULT_BLOCKING_TIMEOUT_MS)]
    pub(crate) classifier_timeout_ms: u64,

    /// Backend used for the concept extractor. Defaults to the same
    /// backend used for the chat (`--backend`); `stub` forces deterministic
    /// extraction (empty concepts) regardless of main backend.
    #[arg(long)]
    pub(crate) extractor_backend: Option<String>,

    /// Model used for the concept extractor. Defaults to the same
    /// model used for the chat (`--model`). Useful for haiku-as-extractor
    /// + sonnet-as-chat on bigger machines.
    #[arg(long)]
    pub(crate) extractor_model: Option<String>,

    /// Maximum time to block awaiting the previous turn's extraction
    /// before the next intent decision. Defaults to
    /// `primer_extractor::consts::DEFAULT_BLOCKING_TIMEOUT_MS`.
    #[arg(long, default_value_t = primer_extractor::consts::DEFAULT_BLOCKING_TIMEOUT_MS)]
    pub(crate) extractor_timeout_ms: u64,

    /// Backend used for the comprehension classifier. Defaults to the same
    /// backend used for the chat (`--backend`); `stub` forces deterministic
    /// comprehension (empty assessments) regardless of main backend.
    #[arg(long)]
    pub(crate) comprehension_backend: Option<String>,

    /// Model used for the comprehension classifier. Defaults to the same
    /// model used for the chat (`--model`). Useful for haiku-as-comprehension
    /// + sonnet-as-chat configurations on bigger machines.
    #[arg(long)]
    pub(crate) comprehension_model: Option<String>,

    /// Append a custom reasoning-marker pair to strip from model output.
    /// Repeatable: `--reasoning-marker '<think>' '</think>'`. The built-in
    /// defaults (`<think>…</think>`, Gemma4 `<|channel>…<channel|>`) always
    /// apply; this only adds more. Markers are matched as literal text, not
    /// regex. Applies to ollama / openai-compat backends.
    #[arg(long, num_args = 2, value_names = ["OPEN", "CLOSE"], action = clap::ArgAction::Append)]
    pub(crate) reasoning_marker: Vec<String>,

    /// Maximum time to block awaiting the previous turn's
    /// extractor → comprehension chain before the next intent decision.
    /// Combined with `--extractor-timeout-ms` this caps the total
    /// background-work budget per turn. Defaults to
    /// `primer_comprehension::consts::DEFAULT_BLOCKING_TIMEOUT_MS`.
    #[arg(long, default_value_t = primer_comprehension::consts::DEFAULT_BLOCKING_TIMEOUT_MS)]
    pub(crate) comprehension_timeout_ms: u64,

    /// Maximum number of overdue/due concepts to inject into the system
    /// prompt per turn for spaced-repetition vocabulary review. Higher =
    /// more review pressure, more prompt bloat. Must be ≥ 1; 0 is rejected
    /// at parse time. Defaults to
    /// `primer_core::consts::vocab::DEFAULT_VOCAB_MAX_PER_PROMPT` (4).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    pub(crate) vocab_max_per_prompt: Option<u64>,

    /// Minutes between break-suggestion nudges. The Primer phrases the
    /// suggestion in-character; the child can keep going. Default 30.
    /// Must be >= 1.
    #[arg(
        long,
        default_value_t = primer_core::consts::break_suggest::DEFAULT_INTERVAL_MINUTES,
        value_parser = clap::value_parser!(u32).range(1..),
    )]
    pub(crate) session_break_after_mins: u32,

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
    pub(crate) embedder_backend: String,

    /// Model name for the embedder. With `--embedder-backend fastembed`,
    /// defaults to `bge-m3` and accepts other fastembed model ids
    /// (e.g. `bge-small-en-v1.5`). With `--embedder-backend ollama`,
    /// defaults to `nomic-embed-text`.
    #[arg(long, value_name = "NAME")]
    pub(crate) embedder_model: Option<String>,

    /// Override the Ollama endpoint used for `--embedder-backend ollama`.
    /// Defaults to `http://localhost:11434`. Has no effect on
    /// `stub`/`fastembed` backends.
    #[arg(long, value_name = "URL")]
    pub(crate) embedder_ollama_url: Option<String>,

    /// Override the URL for `--embedder-backend openai-compat`.
    /// Defaults to `--openai-compat-url` if set, otherwise
    /// `http://localhost:8000`.
    #[arg(long, value_name = "URL")]
    pub(crate) embedder_openai_compat_url: Option<String>,

    /// Model name for `--embedder-backend openai-compat` (required).
    #[arg(long, value_name = "NAME")]
    pub(crate) embedder_openai_compat_model: Option<String>,

    /// Print pedagogical decisions (intent chosen, classifier output,
    /// extractor output, comprehension output) alongside the conversation,
    /// on stderr. Stdout stays clean.
    #[arg(long)]
    pub(crate) verbose: bool,

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
    pub(crate) speech: bool,

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
    pub(crate) whisper_model: Option<PathBuf>,

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
    pub(crate) voice_onnx: Option<PathBuf>,

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
    pub(crate) voice_config: Option<PathBuf>,

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
    pub(crate) tts: TtsChoice,

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
    pub(crate) supertonic_dir: Option<PathBuf>,

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
    pub(crate) supertonic_voice_style: Option<PathBuf>,

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
    pub(crate) voice: String,

    /// Override silero's min_silence_ms for --speech mode. The default
    /// (300 ms) is too aggressive given the cancel-on-resume safety net;
    /// 600 ms reduces false trips at no perceived-latency cost. Bounded
    /// to [50, 5000] ms — values below 50 ms make silero fire constantly,
    /// above 5 s defeats the purpose.
    #[cfg(feature = "speech")]
    #[arg(long, default_value_t = 600, value_parser = parse_mic_silence_ms)]
    pub(crate) mic_silence_ms: u32,

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
    pub(crate) qnn_bundle_dir: Option<PathBuf>,

    /// Path to the QAIRT runtime library directory (containing
    /// `libGenie.so`). Optional; defaults to
    /// `<qnn_bundle_dir>/../qairt/lib/aarch64-android/` matching the
    /// AI Hub apps layout. Override when QAIRT is installed elsewhere
    /// (or via `PRIMER_QNN_QAIRT_LIB_DIR`).
    #[cfg(feature = "qnn")]
    #[arg(long, value_name = "DIR", env = "PRIMER_QNN_QAIRT_LIB_DIR")]
    pub(crate) qnn_qairt_lib_dir: Option<PathBuf>,

    /// llama.cpp: number of model layers to offload to GPU. Default:
    /// all layers (-1) when built with a GPU feature, else CPU (0).
    /// Only meaningful with `--backend llamacpp`. Declared only on a
    /// `--features llamacpp*` build so `--help` stays uncluttered on the
    /// default text-REPL build (mirrors the qnn flags).
    #[cfg(feature = "llamacpp")]
    #[arg(long, value_name = "N")]
    pub(crate) llamacpp_gpu_layers: Option<i32>,

    /// llama.cpp: context length (n_ctx). Default: the model's trained
    /// length. Only meaningful with `--backend llamacpp`. Declared only on
    /// a `--features llamacpp*` build (see `--llamacpp-gpu-layers`).
    #[cfg(feature = "llamacpp")]
    #[arg(long, value_name = "N")]
    pub(crate) llamacpp_n_ctx: Option<u32>,
}
