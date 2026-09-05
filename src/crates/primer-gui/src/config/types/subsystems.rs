//! Settings for the LLM sub-services and the embedder.
//!
//! [`SubsystemConfig`] is shared by the classifier, extractor, and
//! comprehension classifier — the three per-turn background services
//! that default to the main backend unless explicitly overridden.
//! [`EmbedderConfig`] selects the retrieval embedder; its default is
//! feature-aware (see `default_embedder_kind`).

use serde::{Deserialize, Serialize};

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
