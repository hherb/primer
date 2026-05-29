//! Configuration types — loaded from a TOML file on the device.
//!
//! The config determines which backends are active, where models are stored,
//! and how the pedagogical engine behaves. Changing the config file switches
//! from Snapdragon NPU to RK1828 to cloud API without recompilation.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrimerConfig {
    pub inference: InferenceConfig,
    pub speech: SpeechConfig,
    pub knowledge: KnowledgeConfig,
    pub pedagogy: PedagogyConfig,
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceConfig {
    /// Which backend to use: "llama-cpp", "qnn", "rknn", "cloud".
    pub backend: String,
    /// Path to the local model file (GGUF format for llama.cpp).
    pub model_path: Option<PathBuf>,
    /// Cloud API endpoint (used when backend = "cloud").
    pub api_endpoint: Option<String>,
    /// Cloud API key (used when backend = "cloud").
    pub api_key: Option<String>,
    /// Cloud model identifier (e.g., "claude-sonnet-4-6").
    pub cloud_model: Option<String>,
    /// Default generation parameters.
    pub default_params: Option<GenerationDefaults>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationDefaults {
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechConfig {
    /// STT backend: "whisper", "platform-native".
    pub stt_backend: String,
    /// Path to Whisper model file.
    pub whisper_model_path: Option<PathBuf>,
    /// TTS backend: "piper", "platform-native".
    pub tts_backend: String,
    /// Path to Piper voice model directory.
    pub piper_voice_path: Option<PathBuf>,
    /// Default voice profile identifier.
    pub default_voice: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeConfig {
    /// Path to the SQLite knowledge base file.
    pub db_path: PathBuf,
    /// Maximum passages to retrieve per query.
    pub default_top_k: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PedagogyConfig {
    /// How many conversation turns to include in the LLM context window
    /// for the global (cloud / large-context) path.
    pub context_window_turns: usize,
    /// Recent-turn window to use when the active backend is a small-context
    /// (≈4K-token) backend — detected via
    /// [`crate::backend::is_small_context_backend`] over the backend's
    /// `name()`. `None` disables the override so every backend uses
    /// `context_window_turns`. Default
    /// `Some(DEFAULT_CONTEXT_WINDOW_TURNS_SMALL_CONTEXT)`. Phase 1.2 step
    /// 1.2.5. Named `_small_context` rather than `_qnn` so a future
    /// 4K-bound non-Qualcomm backend reuses it without a rename.
    pub context_window_turns_small_context: Option<usize>,
    /// Fused-passage count for knowledge-base retrieval when the active
    /// backend is a small-context backend (same detection as
    /// `context_window_turns_small_context`). `None` falls back to the
    /// global `KB_FINAL_TOP_K`. Default
    /// `Some(KB_FINAL_TOP_K_SMALL_CONTEXT)`. Phase 1.2 step 1.2.5.
    pub kb_top_k_small_context: Option<usize>,
    /// Minutes between break-suggestion nudges. After this many minutes
    /// of session time (or this many minutes since the last suggestion,
    /// whichever is more recent), the next pedagogical intent is forced
    /// to `SuggestBreak`. The Primer never enforces a session halt — the
    /// child can keep going past any number of suggestions. Set to `0`
    /// to disable the gate entirely.
    pub break_suggest_after_minutes: u32,
    /// How aggressively to probe for comprehension (0.0 = gentle, 1.0 = rigorous).
    /// Adapts based on learner profile, but this sets the baseline.
    pub socratic_pressure: f32,
}

impl Default for PedagogyConfig {
    fn default() -> Self {
        Self {
            context_window_turns: crate::consts::pedagogy::DEFAULT_CONTEXT_WINDOW_TURNS,
            context_window_turns_small_context: Some(
                crate::consts::pedagogy::DEFAULT_CONTEXT_WINDOW_TURNS_SMALL_CONTEXT,
            ),
            kb_top_k_small_context: Some(crate::consts::retrieval::KB_FINAL_TOP_K_SMALL_CONTEXT),
            break_suggest_after_minutes: crate::consts::break_suggest::DEFAULT_INTERVAL_MINUTES,
            socratic_pressure: 0.5,
        }
    }
}

impl PedagogyConfig {
    /// Effective recent-turn context window for a backend with the given
    /// `name()`. Small-context backends
    /// ([`crate::backend::is_small_context_backend`]) use
    /// `context_window_turns_small_context` when it is set; every other
    /// backend — and a `None` override — uses the global
    /// `context_window_turns`.
    pub fn effective_context_window_turns(&self, backend_name: &str) -> usize {
        match self.context_window_turns_small_context {
            Some(n) if crate::backend::is_small_context_backend(backend_name) => n,
            _ => self.context_window_turns,
        }
    }

    /// Effective knowledge-base fused-passage count for a backend with the
    /// given `name()`. Small-context backends use `kb_top_k_small_context`
    /// when it is set; every other backend — and a `None` override — uses
    /// the global [`crate::consts::retrieval::KB_FINAL_TOP_K`].
    pub fn effective_kb_top_k(&self, backend_name: &str) -> usize {
        match self.kb_top_k_small_context {
            Some(k) if crate::backend::is_small_context_backend(backend_name) => k,
            _ => crate::consts::retrieval::KB_FINAL_TOP_K,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::QNN_NAME_PREFIX;
    use crate::consts::pedagogy::{
        DEFAULT_CONTEXT_WINDOW_TURNS, DEFAULT_CONTEXT_WINDOW_TURNS_SMALL_CONTEXT,
    };
    use crate::consts::retrieval::{KB_FINAL_TOP_K, KB_FINAL_TOP_K_SMALL_CONTEXT};

    #[test]
    fn default_uses_global_window_for_cloud_backend() {
        let cfg = PedagogyConfig::default();
        assert_eq!(
            cfg.effective_context_window_turns("claude-sonnet-4-6"),
            DEFAULT_CONTEXT_WINDOW_TURNS
        );
    }

    #[test]
    fn default_uses_small_context_window_for_qnn_backend() {
        let cfg = PedagogyConfig::default();
        assert_eq!(
            cfg.effective_context_window_turns(&format!("{QNN_NAME_PREFIX}Qwen3-4B")),
            DEFAULT_CONTEXT_WINDOW_TURNS_SMALL_CONTEXT
        );
    }

    #[test]
    fn none_override_falls_back_to_global_window_even_for_qnn() {
        let cfg = PedagogyConfig {
            context_window_turns_small_context: None,
            ..PedagogyConfig::default()
        };
        assert_eq!(
            cfg.effective_context_window_turns(&format!("{QNN_NAME_PREFIX}Qwen3-4B")),
            DEFAULT_CONTEXT_WINDOW_TURNS
        );
    }

    #[test]
    fn default_uses_global_kb_top_k_for_cloud_backend() {
        let cfg = PedagogyConfig::default();
        assert_eq!(cfg.effective_kb_top_k("stub"), KB_FINAL_TOP_K);
    }

    #[test]
    fn default_uses_small_context_kb_top_k_for_qnn_backend() {
        let cfg = PedagogyConfig::default();
        assert_eq!(
            cfg.effective_kb_top_k(&format!("{QNN_NAME_PREFIX}Qwen3-4B")),
            KB_FINAL_TOP_K_SMALL_CONTEXT
        );
    }

    #[test]
    fn none_kb_override_falls_back_to_global_even_for_qnn() {
        let cfg = PedagogyConfig {
            kb_top_k_small_context: None,
            ..PedagogyConfig::default()
        };
        assert_eq!(
            cfg.effective_kb_top_k(&format!("{QNN_NAME_PREFIX}Qwen3-4B")),
            KB_FINAL_TOP_K
        );
    }
}
