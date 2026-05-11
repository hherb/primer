//! Long-lived Tauri-managed app state.
//!
//! `AppState` is the single value registered with `Builder::manage()`;
//! every Tauri command takes `tauri::State<'_, AppState>` and locks
//! the appropriate field. The state holds:
//!
//! 1. The persisted `GuiConfig` (mutable across "Save" actions).
//! 2. The home directory path so config save/load are filesystem-pure.
//! 3. An optional `ActiveSession` carrying the constructed inference /
//!    knowledge / classifier / extractor / comprehension / embedder /
//!    store Arcs once `start_session` succeeds.
//!
//! Per the implementation plan, the `DialogueManager` itself is *not*
//! held long-lived — it's constructed lazily on each send-message
//! command from the Arcs in `ActiveSession`. That choice keeps the
//! lifetime story of DM's `&'a dyn` borrows compatible with Tauri's
//! `'static + Send + Sync` state model without refactoring
//! `primer-pedagogy`.

use std::path::PathBuf;
use std::sync::Arc;

use primer_classifier::{ClassifierSettings, EngagementClassifier};
use primer_comprehension::{ComprehensionClassifier, ComprehensionSettings};
use primer_core::config::PedagogyConfig;
use primer_core::embedder::Embedder;
use primer_core::i18n::Locale;
use primer_core::inference::InferenceBackend;
use primer_core::learner::LearnerModel;
use primer_core::storage::{LearnerStore, SessionStore};
use primer_extractor::{ConceptExtractor, ExtractorSettings};
use primer_knowledge::SqliteKnowledgeBase;
use primer_pedagogy::vocab::VocabSettings;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::config::GuiConfig;

/// Tauri-managed application state.
pub struct AppState {
    /// User's home directory (resolved at startup). Used by config
    /// load/save and the default session-DB path resolver. Held as
    /// owned so commands don't need to re-read `$HOME` on every call.
    pub home: PathBuf,

    /// Persisted settings, kept in memory so `get_settings` doesn't
    /// hit disk and `update_settings` can stage the new value before
    /// the JSON save completes.
    pub config: Mutex<GuiConfig>,

    /// The currently open session, if any. `None` between
    /// `close_session` and the next `start_session` / `resume_session`.
    pub session: Mutex<Option<ActiveSession>>,
}

impl AppState {
    /// Build a fresh state from a home directory and an initial config.
    /// The config is taken by value so callers can mutate their own
    /// `GuiConfig` separately from what gets registered.
    pub fn new(home: PathBuf, config: GuiConfig) -> Self {
        Self {
            home,
            config: Mutex::new(config),
            session: Mutex::new(None),
        }
    }
}

/// Everything `DialogueManager::new` needs to be constructed
/// per-command, plus the live `LearnerModel` that mutates across turns.
///
/// All trait objects are `Arc<dyn ...>` so a command can clone them out
/// of the state guard, drop the guard, and run the (potentially slow)
/// dialogue turn outside the mutex — preventing concurrent commands
/// from blocking on each other unnecessarily.
pub struct ActiveSession {
    /// Identifier of the underlying `Session` row in the session DB.
    pub session_id: Uuid,

    /// The session's locale (matches the learner's stored locale and
    /// the knowledge base's per-locale partition).
    pub locale: Locale,

    /// The currently-loaded `LearnerModel`. Wrapped in a Mutex because
    /// the dialogue turn mutates it (engagement state, concept depths,
    /// vocab box transitions) and may run while other commands inspect
    /// the snapshot for sidebar updates.
    pub learner: Mutex<LearnerModel>,

    pub backend: Arc<dyn InferenceBackend>,
    /// Name string used by `primer-engine::build_*` dispatch. Held
    /// alongside the Arc because `InferenceBackend::name()` cannot be
    /// called through a borrow once the Arc is moved into a builder.
    pub backend_name: String,
    pub main_model: String,

    pub knowledge: Arc<SqliteKnowledgeBase>,

    pub session_store: Arc<dyn SessionStore>,
    pub learner_store: Arc<dyn LearnerStore>,

    pub classifier: Arc<dyn EngagementClassifier>,
    pub classifier_settings: ClassifierSettings,
    pub extractor: Arc<dyn ConceptExtractor>,
    pub extractor_settings: ExtractorSettings,
    pub comprehension: Arc<dyn ComprehensionClassifier>,
    pub comprehension_settings: ComprehensionSettings,

    pub vocab_settings: VocabSettings,
    pub embedder: Option<Arc<dyn Embedder>>,
    pub pedagogy_config: PedagogyConfig,
}
