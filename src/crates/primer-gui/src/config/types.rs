//! Config data types — every persisted GUI setting.
//!
//! These are the on-disk shapes ([`GuiConfig`] and its sub-structs). The
//! frontend-facing read/write projections live in [`super::view`]; the
//! load/save plumbing lives in [`super::persistence`].

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Top-level container for every GUI setting.
///
/// Each sub-struct groups one CLI subsystem so the settings modal can
/// render them as collapsible sections without bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GuiConfig {
    pub learner: LearnerConfig,
    pub backend: BackendConfig,
    pub classifier: SubsystemConfig,
    pub extractor: SubsystemConfig,
    pub comprehension: SubsystemConfig,
    pub embedder: EmbedderConfig,
    pub vocab: VocabConfig,
    pub breaks: BreakConfig,
    pub persistence: PersistenceConfig,
    pub ui: UiConfig,
    pub speech: SpeechSettings,
    pub diagnostics: DiagnosticsConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LearnerConfig {
    pub name: String,
    pub age: u8,
    /// Locale pack id (BCP-47 short — "en", "de", ...).
    pub locale: String,
}

impl Default for LearnerConfig {
    fn default() -> Self {
        Self {
            name: primer_core::consts::learner::DEFAULT_NAME.to_string(),
            age: 8,
            locale: "en".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackendConfig {
    /// "stub" | "cloud" | "ollama" | "openai-compat"
    pub kind: String,
    /// Model id. None means "use the CLI's per-kind default".
    pub model: Option<String>,
    pub ollama_url: String,
    /// OpenAI-compatible server URL (used when `kind == "openai-compat"`).
    /// Mirrors the CLI's `--openai-compat-url` default.
    pub openai_compat_url: String,
    /// Where to read the API key from when `kind == "cloud"`.
    pub api_key_source: ApiKeySource,
    /// Where to read the API key from when `kind == "openai-compat"`.
    /// The `Env` variant reads `OPENAI_COMPAT_API_KEY` (the CLI's
    /// env-var name); local servers (oMLX, LM Studio, vLLM) ignore it,
    /// remote providers (Together, Groq) require it. Held under the
    /// same secret discipline as the cloud key — never crosses IPC.
    pub openai_compat_api_key_source: ApiKeySource,
    /// QNN bundle directory (used when `kind == "qnn"`). Contains
    /// `genie_config.json`, `primer-meta.json`, and the per-shard
    /// context binaries. Mirrors the CLI's `--qnn-bundle-dir`. `None`
    /// here means "unset" — selecting the qnn backend without it errors
    /// at session-start via `build_qnn_backend`'s "bundle-dir required"
    /// message. Not a secret, so it passes through the IPC view/update
    /// DTOs verbatim (unlike the API keys).
    pub qnn_bundle_dir: Option<PathBuf>,
    /// QNN QAIRT runtime library directory (containing `libGenie.so`).
    /// Mirrors the CLI's `--qnn-qairt-lib-dir`. `None` falls back to the
    /// conventional `<bundle>/../qairt/lib/aarch64-android/` layout via
    /// `primer_engine::default_qairt_lib_dir`.
    pub qnn_qairt_lib_dir: Option<PathBuf>,
    /// GGUF model file path (used when `kind == "llamacpp"`). Mirrors the
    /// CLI's reuse of `--model` for the GGUF path, but the GUI carries a
    /// dedicated field. `None` here means "unset" — selecting the llamacpp
    /// backend without it errors at session-start via
    /// `build_llamacpp_backend`'s "GGUF path required" message. Not a
    /// secret, so it crosses the IPC view/update DTOs verbatim.
    #[serde(default)]
    pub gguf_path: Option<PathBuf>,
    /// llama.cpp `n_gpu_layers` override (used when `kind == "llamacpp"`).
    /// `None` ⇒ resolved by the compiled GPU feature.
    #[serde(default)]
    pub llamacpp_gpu_layers: Option<i32>,
    /// llama.cpp `n_ctx` override (used when `kind == "llamacpp"`).
    /// `None` ⇒ the model's trained default.
    #[serde(default)]
    pub llamacpp_n_ctx: Option<u32>,
    /// Raw "reasoning markers" textarea text from Settings: one
    /// `open<whitespace>close` pair per line. Parsed into `(open, close)`
    /// pairs by `crate::reasoning_markers::parse_reasoning_markers` at
    /// session-wiring time and appended to the built-in defaults for the
    /// ollama / openai-compat backends. Empty ⇒ defaults only. Stored
    /// verbatim so the textarea round-trips losslessly. Not a secret —
    /// crosses the IPC View/Update DTOs unredacted.
    pub reasoning_markers: String,
    /// Opt-in fallback inference backend name (`stub`/`cloud`/`ollama`/
    /// `openai-compat`). `None` ⇒ no fallback ⇒ local-only (the privacy
    /// default — a local-only setup never silently reaches the cloud).
    /// Mirrors the CLI's `--fallback-backend`. Consumed by
    /// `primer_engine::build_main_backend` at session-wiring time: when the
    /// primary is unavailable at startup or fails *before any token streams*,
    /// the turn is served from this secondary. Not a secret, so it crosses
    /// the IPC view/update DTOs verbatim (no Keep/Env dance).
    #[serde(default)]
    pub fallback_backend: Option<String>,
    /// Model id for the fallback secondary. Mirrors the CLI's
    /// `--fallback-model`. Resolution rules live in
    /// `primer_engine::resolve_fallback_model`: `None` is valid (cloud
    /// defaults to `claude-sonnet-4-6`; stub ignores it; ollama/openai-compat
    /// require an explicit model). Not a secret — crosses IPC verbatim.
    #[serde(default)]
    pub fallback_model: Option<String>,
    /// Phase 1.3 inference-router mode. Mirrors the CLI's `--router-mode`.
    /// `LocalOnly` (default) ⇒ no routing (today's behavior). Consumed by
    /// `primer_engine::build_main_backend` via `BackendParams.router_mode`.
    #[serde(default)]
    pub router_mode: primer_core::router::RouterMode,
    /// Phase 1.3 latency-aware routing budget (ms). Mirrors the CLI's
    /// `--primary-ttft-budget-ms`. `None` (default) ⇒ latency routing OFF.
    /// Only takes effect with `router_mode == Hybrid` AND a configured
    /// fallback. `#[serde(default)]` so existing configs load unchanged.
    #[serde(default)]
    pub primary_ttft_budget_ms: Option<u64>,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            kind: "stub".to_string(),
            model: None,
            ollama_url: "http://localhost:11434".to_string(),
            openai_compat_url: "http://localhost:8000".to_string(),
            api_key_source: ApiKeySource::default(),
            openai_compat_api_key_source: ApiKeySource::default(),
            qnn_bundle_dir: None,
            qnn_qairt_lib_dir: None,
            gguf_path: None,
            llamacpp_gpu_layers: None,
            llamacpp_n_ctx: None,
            reasoning_markers: String::new(),
            fallback_backend: None,
            fallback_model: None,
            router_mode: primer_core::router::RouterMode::LocalOnly,
            primary_ttft_budget_ms: None,
        }
    }
}

/// How the cloud backend obtains its API key.
///
/// Default is `Env` — read `ANTHROPIC_API_KEY` from the process
/// environment at session-start time. `Inline` keeps the key in the
/// config JSON (file mode 0600). The two-variant shape mirrors the
/// CLI's "`--api-key` OR env" behaviour.
///
/// **Disk-only.** This type is intentionally NOT exposed to the
/// frontend — every serialisation site that crosses the IPC boundary
/// uses [`ApiKeySourceView`] (read) or [`ApiKeyUpdate`] (write).
/// Re-exposing the inline key over IPC would let any compromised
/// frontend JS exfiltrate the secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApiKeySource {
    Env,
    Inline { key: String },
}

impl Default for ApiKeySource {
    fn default() -> Self {
        Self::Env
    }
}

/// Frontend-safe projection of [`ApiKeySource`].
///
/// `Inline { has_key }` carries a boolean — *whether* a key is stored,
/// not the key itself — so the settings UI can render "inline key is
/// set" without ever seeing the secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApiKeySourceView {
    Env,
    Inline { has_key: bool },
}

impl From<&ApiKeySource> for ApiKeySourceView {
    fn from(s: &ApiKeySource) -> Self {
        match s {
            ApiKeySource::Env => Self::Env,
            ApiKeySource::Inline { key } => Self::Inline {
                has_key: !key.is_empty(),
            },
        }
    }
}

/// Update intent for the inline API key on the [`update_settings`](crate::commands::settings::update_settings) write path.
///
/// `Keep` is the workhorse — the frontend rendered the redacted view
/// and isn't touching the secret, so the persisted value carries
/// through. `Env` and `Inline` switch the source explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApiKeyUpdate {
    /// Preserve whatever's already persisted on disk.
    Keep,
    Env,
    Inline {
        key: String,
    },
}

impl ApiKeyUpdate {
    /// Resolve to a concrete [`ApiKeySource`] given the currently-persisted value.
    pub fn resolve(self, current: &ApiKeySource) -> ApiKeySource {
        match self {
            Self::Keep => current.clone(),
            Self::Env => ApiKeySource::Env,
            Self::Inline { key } => ApiKeySource::Inline { key },
        }
    }
}

/// Settings for the classifier / extractor / comprehension subsystems.
///
/// `match_main = true` collapses all override fields — the subsystem
/// uses the main backend and main model. `match_main = false` requires
/// the kind/model/timeout fields to be respected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubsystemConfig {
    pub match_main: bool,
    /// "stub" | "cloud" | "ollama"
    pub kind: Option<String>,
    pub model: Option<String>,
    pub timeout_ms: u64,
}

impl SubsystemConfig {
    /// Default for the classifier — 3000 ms timeout, matching CLI.
    pub fn default_classifier() -> Self {
        Self {
            match_main: true,
            kind: None,
            model: None,
            timeout_ms: primer_classifier::consts::DEFAULT_BLOCKING_TIMEOUT_MS,
        }
    }

    /// Default for the extractor — 5000 ms timeout, matching CLI.
    pub fn default_extractor() -> Self {
        Self {
            match_main: true,
            kind: None,
            model: None,
            timeout_ms: primer_extractor::consts::DEFAULT_BLOCKING_TIMEOUT_MS,
        }
    }

    /// Default for the comprehension classifier — 5000 ms timeout, matching CLI.
    pub fn default_comprehension() -> Self {
        Self {
            match_main: true,
            kind: None,
            model: None,
            timeout_ms: primer_comprehension::consts::DEFAULT_BLOCKING_TIMEOUT_MS,
        }
    }
}

impl Default for SubsystemConfig {
    fn default() -> Self {
        Self::default_classifier()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedderConfig {
    /// "none" | "stub" | "fastembed" | "ollama" | "openai-compat"
    pub kind: String,
    pub model: Option<String>,
    pub ollama_url: Option<String>,
    /// OpenAI-compatible embedding server URL override (used when
    /// `kind == "openai-compat"`). `None` falls back to the main
    /// backend's `openai_compat_url`, mirroring the CLI's
    /// `--embedder-openai-compat-url` → `--openai-compat-url` fallback.
    pub openai_compat_url: Option<String>,
}

/// The default embedder kind tracks what is compiled in: a build with the
/// `embedding` feature (the default) defaults to hybrid retrieval via
/// fastembed; a `--no-default-features` build stays BM25-only so the GUI
/// never refuses to start. Because the config struct is `#[serde(default)]`,
/// this default is only consulted when the `kind` field is ABSENT from
/// `gui-config.json` (e.g. a config written by an older build); a config
/// that stores an explicit `kind` — including `"none"` — keeps that value
/// verbatim, so flipping the default never overrides a user's saved choice.
#[cfg(feature = "embedding")]
fn default_embedder_kind() -> &'static str {
    "fastembed"
}

#[cfg(not(feature = "embedding"))]
fn default_embedder_kind() -> &'static str {
    "none"
}

impl Default for EmbedderConfig {
    fn default() -> Self {
        Self {
            kind: default_embedder_kind().to_string(),
            model: None,
            ollama_url: None,
            openai_compat_url: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VocabConfig {
    /// Top-K most-overdue concepts to inject into the system prompt as
    /// passive review hints. `None` keeps the CLI default.
    pub max_per_prompt: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BreakConfig {
    /// Minutes between break-suggestion nudges. Must be >= 1.
    pub after_mins: u32,
}

impl Default for BreakConfig {
    fn default() -> Self {
        Self {
            after_mins: primer_core::consts::break_suggest::DEFAULT_INTERVAL_MINUTES,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistenceConfig {
    /// Explicit session DB path. `None` → default to
    /// `~/.primer/<slug(name)>.db` at session-start time.
    pub session_db: Option<PathBuf>,
    /// Knowledge DB path. `None` → `:memory:`.
    pub knowledge_db: Option<PathBuf>,
    /// When true, neither DB is written to disk and `session_db` /
    /// `knowledge_db` are ignored.
    pub no_persist: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Right sidebar default-open state. Step 5+ remembers this across launches.
    pub sidebar_open: bool,
    /// Last-active sidebar section: "current_turn" | "learner" | "session".
    /// Free-text on disk so adding a section in a later step doesn't break older
    /// configs.
    pub last_section: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            sidebar_open: true,
            last_section: "current_turn".to_string(),
        }
    }
}

/// Developer/eval diagnostics. Every field defaults OFF so a production
/// child device records no telemetry of any kind (issue #228).
///
/// Not a secret, so this section passes through the IPC View/Update DTOs
/// verbatim (like [`UiConfig`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DiagnosticsConfig {
    /// When `true`, the Android startup hook enables the on-device QNN
    /// per-turn throughput metrics file (`<app_data>/.primer/
    /// qnn_metrics.jsonl`: TTFT + decode tok/s, read via `run-as cat`).
    ///
    /// **OFF by default.** Only a developer running a throughput-capture
    /// session flips it on; a child's device never records by default. The
    /// file itself is size-capped and single-backup rotated
    /// (`primer_inference::qnn::metrics`) so even when enabled it cannot grow
    /// without bound. No effect on desktop (the metrics path is mobile-only).
    pub qnn_metrics_enabled: bool,
}

/// Which speech backend stack to use. `WhisperPiper` is the default and
/// works on every supported OS. `MacosNative` is macOS-only and requires
/// building with `--features primer-gui/macos-native`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SpeechBackend {
    #[default]
    WhisperPiper,
    MacosNative,
}

/// STT half of the voice stack (GUI-owned mirror of
/// `primer_speech::voice_loop::SttBackend`; converted at the speech-gated
/// wiring boundary in `voice/backends.rs`). Defined locally because
/// `config.rs` is always compiled but `primer-speech` is an optional dep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SttBackend {
    #[default]
    Whisper,
    MacosNative,
}

/// TTS half of the voice stack (GUI-owned mirror of
/// `primer_speech::voice_loop::TtsBackend`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TtsBackend {
    #[default]
    Piper,
    Supertonic,
    MacosNative,
}

/// Voice-mode settings.
///
/// `voice_mode_enabled` is the sticky toggle (per device, not per
/// learner — see spec rationale). `overrides` is keyed by
/// `Locale::pack_id()` so switching locales doesn't clobber the path
/// the user typed in for the other one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpeechSettings {
    pub voice_mode_enabled: bool,
    pub disable_auto_download: bool,
    /// STT half of the voice stack. Defaults to `whisper`.
    #[serde(default)]
    pub stt_backend: SttBackend,
    /// TTS half of the voice stack. Defaults to `piper`.
    #[serde(default)]
    pub tts_backend: TtsBackend,
    /// Pre-Stage-C coupled selector (#189). Deserialized only so an older
    /// `gui-config.json` that stored `backend` migrates via
    /// [`SpeechSettings::resolve_backends`]; never written back out.
    #[serde(default, skip_serializing)]
    pub backend: Option<SpeechBackend>,
    /// Milliseconds of post-end-of-speech silence the VAD waits before
    /// firing SpeechEnd. Default reads from
    /// `primer_core::consts::speech::DEFAULT_MIC_SILENCE_MS`.
    pub mic_silence_ms: u32,
    /// Overall request timeout, in seconds, for each voice-asset
    /// download. `0` means "no timeout" (NOT recommended — a stalled
    /// connection then locks the consent modal indefinitely). Default
    /// reads from `primer_core::consts::speech::DEFAULT_DOWNLOAD_TIMEOUT_SECS`.
    #[serde(default = "default_download_timeout_secs")]
    pub download_timeout_secs: u64,
    /// Per-locale path / voice-id overrides. Keyed by `Locale::pack_id()`.
    pub overrides: std::collections::BTreeMap<String, SpeechLocaleOverride>,
}

fn default_download_timeout_secs() -> u64 {
    primer_core::consts::speech::DEFAULT_DOWNLOAD_TIMEOUT_SECS
}

impl Default for SpeechSettings {
    fn default() -> Self {
        Self {
            voice_mode_enabled: false,
            disable_auto_download: false,
            stt_backend: SttBackend::default(),
            tts_backend: TtsBackend::default(),
            backend: None,
            mic_silence_ms: primer_core::consts::speech::DEFAULT_MIC_SILENCE_MS,
            download_timeout_secs: default_download_timeout_secs(),
            overrides: std::collections::BTreeMap::new(),
        }
    }
}

impl SpeechSettings {
    /// The effective `(stt, tts)` choice. Applies the one-time legacy
    /// `backend` migration: when the new fields are still at their defaults
    /// AND a legacy `backend` value is present, map the old coupled stack to
    /// the two halves. Otherwise the new fields win.
    ///
    /// "At default" can't distinguish "explicitly chose `whisper`/`piper`"
    /// from "never set," so a config carrying BOTH a legacy `backend` and
    /// new fields pinned to their defaults would migrate to the legacy
    /// stack. That state can't arise from the real save path — old configs
    /// never have the new keys (so migration is correct), and saved configs
    /// never have the legacy key (gather drops it; `backend` is
    /// `skip_serializing`). It is reachable only by hand-editing
    /// `gui-config.json`, where the legacy-wins behaviour is acceptable.
    pub fn resolve_backends(&self) -> (SttBackend, TtsBackend) {
        if let Some(legacy) = self.backend {
            if self.stt_backend == SttBackend::default()
                && self.tts_backend == TtsBackend::default()
            {
                return match legacy {
                    SpeechBackend::WhisperPiper => (SttBackend::Whisper, TtsBackend::Piper),
                    SpeechBackend::MacosNative => {
                        (SttBackend::MacosNative, TtsBackend::MacosNative)
                    }
                };
            }
        }
        (self.stt_backend, self.tts_backend)
    }
}

/// Per-locale path/voice override for `SpeechSettings`. `None` on any
/// field means "fall through to the locale default" (see
/// `primer_speech::locale_defaults::voice_default_for`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SpeechLocaleOverride {
    pub piper_onnx_path: Option<PathBuf>,
    pub piper_config_path: Option<PathBuf>,
    pub whisper_model_path: Option<PathBuf>,
    pub voice_id: Option<String>,
    pub supertonic_onnx_dir: Option<PathBuf>,
    pub supertonic_voice_style_path: Option<PathBuf>,
}
