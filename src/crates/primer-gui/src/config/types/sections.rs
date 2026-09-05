//! The small single-purpose settings sections.
//!
//! Each of these is one collapsible block in the settings modal with no
//! internal branching worth a file of its own: who is learning
//! ([`LearnerConfig`]), how the session is paced ([`VocabConfig`],
//! [`BreakConfig`]), where data lives ([`PersistenceConfig`]), how the
//! window looks ([`UiConfig`]), and what a developer may opt into
//! recording ([`DiagnosticsConfig`]).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
