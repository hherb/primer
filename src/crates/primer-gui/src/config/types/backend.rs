//! Inference-backend settings and the API-key secret types.
//!
//! [`BackendConfig`] is the disk shape for Settings → Inference backend:
//! the primary backend, the optional fallback leg, the router mode, and
//! the per-backend asset paths (GGUF, QNN bundle, QAIRT libs).
//!
//! The two API-key secrets (cloud, openai-compat) each resolve through
//! [`ApiKeySource`] on disk, [`ApiKeySourceView`] on the IPC read path,
//! and [`ApiKeyUpdate`] on the IPC write path. Only the latter two ever
//! cross the frontend boundary — see the module doc on [`super`].

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
