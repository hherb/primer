//! Config data types — every persisted GUI setting.
//!
//! These are the on-disk shapes ([`GuiConfig`] and its sub-structs). The
//! frontend-facing read/write projections live in [`super::view`]; the
//! load/save plumbing lives in [`super::persistence`].
//!
//! [`GuiConfig`] itself lives here; the sub-structs are grouped by
//! settings area into [`backend`], [`subsystems`], [`speech`], and
//! [`sections`] (the small single-purpose blocks). All of them are
//! re-exported below, so callers keep using the flat
//! `crate::config::Foo` path that [`super`] already publishes.

use serde::{Deserialize, Serialize};

mod backend;
mod sections;
mod speech;
mod subsystems;

pub use backend::*;
pub use sections::*;
pub use speech::*;
pub use subsystems::*;

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
