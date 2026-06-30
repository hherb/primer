//! Frontend-facing DTOs — the only config shapes that cross the IPC boundary.
//!
//! [`GuiConfigView`] mirrors [`GuiConfig`] but uses [`BackendConfigView`]
//! (which redacts the inline API key). [`GuiConfigUpdate`] mirrors it on
//! the write path with [`BackendConfigUpdate`] (which carries
//! [`ApiKeyUpdate::Keep`] when the frontend isn't touching the key).
//!
//! Every IPC boundary uses these — never [`GuiConfig`] directly.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::*;

/// Frontend-safe projection of [`GuiConfig`] (read path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GuiConfigView {
    pub learner: LearnerConfig,
    pub backend: BackendConfigView,
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

/// Frontend-safe projection of [`BackendConfig`] (read path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackendConfigView {
    pub kind: String,
    pub model: Option<String>,
    pub ollama_url: String,
    pub openai_compat_url: String,
    pub api_key_source: ApiKeySourceView,
    pub openai_compat_api_key_source: ApiKeySourceView,
    /// QNN bundle / QAIRT lib paths pass through verbatim — not secrets.
    pub qnn_bundle_dir: Option<PathBuf>,
    pub qnn_qairt_lib_dir: Option<PathBuf>,
    /// llama.cpp GGUF path / gpu-layers / n_ctx pass through verbatim —
    /// not secrets.
    pub gguf_path: Option<PathBuf>,
    pub llamacpp_gpu_layers: Option<i32>,
    pub llamacpp_n_ctx: Option<u32>,
    /// Raw reasoning-markers textarea text — passes through verbatim
    /// (not a secret), so the settings form can re-show it.
    pub reasoning_markers: String,
    /// Opt-in fallback backend / model — pass through verbatim (not
    /// secrets), so the settings form can re-show the chosen fallback.
    pub fallback_backend: Option<String>,
    pub fallback_model: Option<String>,
    /// Router mode as its canonical kebab-case name (e.g. "local-only").
    /// Passes through verbatim (not a secret) so the settings form can
    /// re-show the chosen routing mode.
    pub router_mode: String,
    /// Latency-aware routing budget (ms). Passes through verbatim (not a
    /// secret) so the settings form can re-show it.
    pub primary_ttft_budget_ms: Option<u64>,
}

impl From<&GuiConfig> for GuiConfigView {
    fn from(c: &GuiConfig) -> Self {
        Self {
            learner: c.learner.clone(),
            backend: BackendConfigView {
                kind: c.backend.kind.clone(),
                model: c.backend.model.clone(),
                ollama_url: c.backend.ollama_url.clone(),
                openai_compat_url: c.backend.openai_compat_url.clone(),
                api_key_source: (&c.backend.api_key_source).into(),
                openai_compat_api_key_source: (&c.backend.openai_compat_api_key_source).into(),
                qnn_bundle_dir: c.backend.qnn_bundle_dir.clone(),
                qnn_qairt_lib_dir: c.backend.qnn_qairt_lib_dir.clone(),
                gguf_path: c.backend.gguf_path.clone(),
                llamacpp_gpu_layers: c.backend.llamacpp_gpu_layers,
                llamacpp_n_ctx: c.backend.llamacpp_n_ctx,
                reasoning_markers: c.backend.reasoning_markers.clone(),
                fallback_backend: c.backend.fallback_backend.clone(),
                fallback_model: c.backend.fallback_model.clone(),
                router_mode: c.backend.router_mode.name().to_string(),
                primary_ttft_budget_ms: c.backend.primary_ttft_budget_ms,
            },
            classifier: c.classifier.clone(),
            extractor: c.extractor.clone(),
            comprehension: c.comprehension.clone(),
            embedder: c.embedder.clone(),
            vocab: c.vocab.clone(),
            breaks: c.breaks.clone(),
            persistence: c.persistence.clone(),
            ui: c.ui.clone(),
            speech: {
                let (stt, tts) = c.speech.resolve_backends();
                SpeechSettings {
                    stt_backend: stt,
                    tts_backend: tts,
                    backend: None,
                    ..c.speech.clone()
                }
            },
            diagnostics: c.diagnostics.clone(),
        }
    }
}

/// Update intent for [`GuiConfig`] (write path).
///
/// Same shape as `GuiConfig` except `backend.api_key_source` is an
/// [`ApiKeyUpdate`] — `Keep` (the common case) preserves whatever
/// secret is already on disk so the frontend never has to handle it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GuiConfigUpdate {
    pub learner: LearnerConfig,
    pub backend: BackendConfigUpdate,
    pub classifier: SubsystemConfig,
    pub extractor: SubsystemConfig,
    pub comprehension: SubsystemConfig,
    pub embedder: EmbedderConfig,
    pub vocab: VocabConfig,
    pub breaks: BreakConfig,
    pub persistence: PersistenceConfig,
    pub ui: UiConfig,
    pub speech: SpeechSettings,
    /// Developer/eval diagnostics. Unlike the backend fields, this carries a
    /// field-level `#[serde(default)]` so an update payload that omits it (an
    /// older `settings.js`, or a hand-written fixture) deserializes to the
    /// OFF default rather than failing — the default is the *safe* privacy
    /// direction (no recording), so a silent revert here can never enable
    /// telemetry. `settings.js::gather()` still sends it so the toggle
    /// persists when flipped.
    #[serde(default)]
    pub diagnostics: DiagnosticsConfig,
}

/// Update intent for [`BackendConfig`] (write path).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BackendConfigUpdate {
    pub kind: String,
    pub model: Option<String>,
    pub ollama_url: String,
    pub openai_compat_url: String,
    pub api_key_source: ApiKeyUpdate,
    pub openai_compat_api_key_source: ApiKeyUpdate,
    /// QNN bundle / QAIRT lib paths. Not secrets, so they cross IPC
    /// verbatim. Like every other `BackendConfigUpdate` field, these are
    /// **mandatory** in the `update_settings` payload — the struct has no
    /// `#[serde(default)]`, so `settings.js::gather()` must always send
    /// them (as `null` when unset).
    pub qnn_bundle_dir: Option<PathBuf>,
    pub qnn_qairt_lib_dir: Option<PathBuf>,
    /// llama.cpp GGUF path / gpu-layers / n_ctx. Not secrets, so they
    /// cross IPC verbatim. Like every other `BackendConfigUpdate` field,
    /// these are **mandatory** in the `update_settings` payload (the
    /// struct has no `#[serde(default)]`), so `settings.js::gather()` must
    /// always send them (as `null` when unset).
    pub gguf_path: Option<PathBuf>,
    pub llamacpp_gpu_layers: Option<i32>,
    pub llamacpp_n_ctx: Option<u32>,
    /// Raw reasoning-markers textarea text. Like every other
    /// `BackendConfigUpdate` field, this is **mandatory** in the
    /// `update_settings` payload (the struct has no `#[serde(default)]`),
    /// so `settings.js::gather()` must always send it (empty string when
    /// the textarea is blank). Not a secret — no Keep/Env dance.
    pub reasoning_markers: String,
    /// Opt-in fallback backend / model. Not secrets, so they cross IPC
    /// verbatim. Like every other `BackendConfigUpdate` field, these are
    /// **mandatory** in the `update_settings` payload (the struct has no
    /// `#[serde(default)]`), so `settings.js::gather()` must always send
    /// them (as `null` when unset).
    pub fallback_backend: Option<String>,
    pub fallback_model: Option<String>,
    /// Router mode kebab-case name. Parsed via `RouterMode::from_str` in
    /// `into_config` (invalid ⇒ `LocalOnly` with a `tracing::warn!`). Like
    /// every other `BackendConfigUpdate` field, this is **mandatory** in the
    /// `update_settings` payload (the struct has no `#[serde(default)]`), so
    /// `settings.js::gather()` must always send it. Not a secret.
    pub router_mode: String,
    /// Latency-aware routing budget (ms). Like every other
    /// `BackendConfigUpdate` field, this is **mandatory** in the
    /// `update_settings` payload (the struct has no `#[serde(default)]`), so
    /// `settings.js::gather()` must always send it (`null` when blank). Not a
    /// secret.
    pub primary_ttft_budget_ms: Option<u64>,
}

impl GuiConfigUpdate {
    /// Resolve to a concrete [`GuiConfig`] using the currently-persisted
    /// value to fill in any field the update keeps (today only the
    /// inline API key).
    pub fn into_config(self, current: &GuiConfig) -> GuiConfig {
        GuiConfig {
            learner: self.learner,
            backend: BackendConfig {
                kind: self.backend.kind,
                model: self.backend.model,
                ollama_url: self.backend.ollama_url,
                openai_compat_url: self.backend.openai_compat_url,
                api_key_source: self
                    .backend
                    .api_key_source
                    .resolve(&current.backend.api_key_source),
                openai_compat_api_key_source: self
                    .backend
                    .openai_compat_api_key_source
                    .resolve(&current.backend.openai_compat_api_key_source),
                qnn_bundle_dir: self.backend.qnn_bundle_dir,
                qnn_qairt_lib_dir: self.backend.qnn_qairt_lib_dir,
                gguf_path: self.backend.gguf_path,
                llamacpp_gpu_layers: self.backend.llamacpp_gpu_layers,
                llamacpp_n_ctx: self.backend.llamacpp_n_ctx,
                reasoning_markers: self.backend.reasoning_markers,
                fallback_backend: self.backend.fallback_backend,
                fallback_model: self.backend.fallback_model,
                router_mode: self
                    .backend
                    .router_mode
                    .parse()
                    .unwrap_or_else(|e: String| {
                        tracing::warn!(
                            target: "primer::gui::config",
                            "invalid router_mode in settings update: {e}; defaulting to local-only",
                        );
                        primer_core::router::RouterMode::default()
                    }),
                primary_ttft_budget_ms: self.backend.primary_ttft_budget_ms,
            },
            classifier: self.classifier,
            extractor: self.extractor,
            comprehension: self.comprehension,
            embedder: self.embedder,
            vocab: self.vocab,
            breaks: self.breaks,
            persistence: self.persistence,
            ui: self.ui,
            speech: self.speech,
            diagnostics: self.diagnostics,
        }
    }
}
