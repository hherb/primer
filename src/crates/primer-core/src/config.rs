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
    /// How many conversation turns to include in the LLM context window.
    pub context_window_turns: usize,
    /// Maximum session length in minutes before the Primer suggests a break.
    pub max_session_minutes: u32,
    /// How aggressively to probe for comprehension (0.0 = gentle, 1.0 = rigorous).
    /// Adapts based on learner profile, but this sets the baseline.
    pub socratic_pressure: f32,
}

impl Default for PedagogyConfig {
    fn default() -> Self {
        Self {
            context_window_turns: 20,
            max_session_minutes: 30,
            socratic_pressure: 0.5,
        }
    }
}
