//! The dialogue manager — the Primer's conversational brain.
//!
//! The `DialogueManager` orchestrates a single conversation session:
//!
//! 1. Receives the child's input (text, post-STT).
//! 2. Decides what pedagogical intent to pursue next.
//! 3. Retrieves relevant knowledge passages for grounding.
//! 4. Constructs a prompt and sends it to the inference backend.
//! 5. Records the exchange and updates the learner model.
//!
//! It does NOT own the inference backend or knowledge base — those are
//! injected as trait objects, keeping this module testable with stubs.
//!
//! # Ownership model
//!
//! `inference` and `knowledge` are borrowed references (`&'a dyn …`):
//! they are only used synchronously inside method bodies and need no
//! cross-turn lifetime. By contrast, `storage` and `classifier` are
//! `Arc<dyn …>` because the post-response classifier task (Task 23)
//! will capture them inside a `tokio::spawn` future, which requires
//! `'static` — borrowed references cannot satisfy that bound.

use std::sync::Arc;

use primer_classifier::{ClassifierSettings, EngagementClassifier};
use primer_core::classifier::EngagementAssessment;
use primer_core::config::PedagogyConfig;
use primer_core::conversation::Session;
use primer_core::extractor::ConceptExtraction;
use primer_core::inference::InferenceBackend;
use primer_core::knowledge::KnowledgeBase;
use primer_core::learner::LearnerModel;
use primer_core::storage::{LearnerStore, SessionStore};
use primer_extractor::{ConceptExtractor, ExtractorSettings};
use tokio::task::JoinHandle;

use crate::prompt_pack::PromptPack;

mod apply;
mod background;
mod learner_update;
mod lifecycle;
mod retrieval;
mod summary;
mod turn;
use apply::{apply_assessment, apply_comprehension, apply_extraction, merge_concepts_into_turn};

#[cfg(test)]
mod test_support;

/// Optional persistence stores for a `DialogueManager`.
///
/// Both fields default to `None` — useful for tests that don't care
/// about persistence. When set, the manager saves to each store at
/// the points its docstring describes (open / resume / per-turn / close).
///
/// Bundled into one struct rather than passed as two arguments because
/// `DialogueManager::new` was already at the clippy `too_many_arguments`
/// threshold; keeping a pair of optional `Arc<dyn …>` together is also
/// the right grouping conceptually — both are "where do I write changes
/// to disk".
#[derive(Default, Clone)]
pub struct DialogueManagerStores {
    pub session: Option<Arc<dyn SessionStore>>,
    pub learner: Option<Arc<dyn LearnerStore>>,
}

/// Per-turn analysis subsystems wired into the dialogue manager.
///
/// Mirrors `DialogueManagerStores`: groups closely-related parameters
/// (the trait object plus its tunable settings) so `DialogueManager::new`
/// stays under the clippy `too_many_arguments` threshold without
/// suppressions. Both classifier and extractor follow the same
/// "spawn after the response, await with bounded timeout at the start
/// of the next turn" pattern, so co-locating them here is the right
/// grouping conceptually as well.
pub struct DialogueManagerSubsystems {
    pub classifier: Arc<dyn EngagementClassifier>,
    pub classifier_settings: ClassifierSettings,
    pub extractor: Arc<dyn ConceptExtractor>,
    pub extractor_settings: ExtractorSettings,
    pub comprehension: Arc<dyn primer_comprehension::ComprehensionClassifier>,
    pub comprehension_settings: primer_comprehension::ComprehensionSettings,
    /// Tunables for the spaced-repetition vocabulary feature
    /// (max overdue concepts injected per turn).
    pub vocab_settings: crate::vocab::VocabSettings,
}

/// The dialogue manager for a single session.
///
/// Holds references to all the subsystems it needs, plus the mutable
/// session and learner model state. The CLI (or future GUI) drives
/// the conversation by calling `respond_to()` in a loop.
///
/// `inference` and `knowledge` are borrowed references: they are used
/// only synchronously inside method bodies. `storage`, `learner_store`,
/// and `classifier` are `Arc<dyn …>` so they can be captured by the
/// post-response classifier task (`tokio::spawn` requires `'static`).
pub struct DialogueManager<'a> {
    /// The learner model — updated in place as we learn about the child.
    pub learner: LearnerModel,
    /// The current conversation session.
    pub session: Session,
    /// Inference backend (local model or cloud API).
    inference: &'a dyn InferenceBackend,
    /// Knowledge base for RAG retrieval.
    knowledge: &'a dyn KnowledgeBase,
    /// Optional session persistence. When set, the session is saved after
    /// every `respond_to_streaming` call (success or mid-stream error).
    /// Arc so the classifier task can capture it across turn boundaries.
    storage: Option<Arc<dyn SessionStore>>,
    /// Optional learner-model persistence. When set, the learner model is
    /// saved at the same four points as the session (open, resume, per-turn,
    /// close). Save failures are logged, not propagated.
    learner_store: Option<Arc<dyn LearnerStore>>,
    /// Engagement classifier — called after each Primer response to assess
    /// the child's engagement state. Arc for the same spawn-capture reason.
    classifier: Arc<dyn EngagementClassifier>,
    /// Tunable parameters for the classifier (thresholds, timeouts, etc.).
    classifier_settings: ClassifierSettings,
    /// Handle to the in-flight classifier task spawned after the previous
    /// turn. `None` when no task is running.
    classify_task: Option<JoinHandle<Option<EngagementAssessment>>>,
    /// Concept extractor — called after each Primer response to extract
    /// concepts from the just-completed exchange. Arc for the same
    /// spawn-capture reason as `classifier`.
    extractor: Arc<dyn ConceptExtractor>,
    /// Tunable parameters for the extractor.
    extractor_settings: ExtractorSettings,
    /// Handle to the in-flight post-response chained task (extractor →
    /// comprehension) spawned after the previous turn. `None` when no
    /// task is running. The result carries the (child, primer) turn
    /// indices and both extraction + comprehension outputs so
    /// `apply_post_response_outcome` can sync state back into
    /// in-memory `Session.turns` and `LearnerModel`.
    post_response_task: Option<JoinHandle<Option<PostResponseResult>>>,
    /// Comprehension classifier — invoked at the tail of each
    /// post-response chained task (after extraction). Arc for the same
    /// spawn-capture reason as `classifier`.
    comprehension: Arc<dyn primer_comprehension::ComprehensionClassifier>,
    /// Tunable parameters for the comprehension classifier.
    comprehension_settings: primer_comprehension::ComprehensionSettings,
    /// Tunables for the vocabulary review feature. Read by
    /// `build_turn_prompt` to bound how many overdue concepts go into
    /// the system prompt; never mutated after construction.
    vocab_settings: crate::vocab::VocabSettings,
    /// Most recent comprehension result applied to the learner. Cleared
    /// on session lifecycle events. Used by `--verbose`.
    last_comprehension: Option<primer_core::comprehension::ComprehensionResult>,
    /// In-memory timestamp of the last `SuggestBreak` fire. Reset on
    /// `new` and `resume_session`. Not persisted across `--resume`
    /// — see the design spec's non-goals for the rationale.
    pub(super) last_break_suggested_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Test-only clock override. When `Some`, `now()` returns this value
    /// instead of `chrono::Utc::now()`. Production paths never set this.
    #[cfg(test)]
    pub(super) clock_override: Option<chrono::DateTime<chrono::Utc>>,
    /// Pedagogical configuration.
    config: PedagogyConfig,
    /// Most recent extractor output applied to the learner. Cleared on
    /// session lifecycle events. Used by `--verbose`.
    last_extraction: Option<primer_core::extractor::ConceptExtraction>,
    /// Tracks whether `learner` has fields-that-map-to-the-`learners`-table
    /// changes that haven't been flushed yet. The per-turn save site is
    /// gated by this flag (lifecycle events at open / resume / close
    /// always save, regardless). Set to `true` whenever any persisted
    /// field is mutated; cleared after a successful save.
    ///
    /// Future-proofing: today only `current_engagement` is mutated per-turn
    /// (via `update_learner_model` and `apply_assessment`). When concept
    /// extraction lands and starts populating `learner.concepts` per-turn,
    /// it just sets the flag — no save-site changes needed.
    learner_dirty: bool,
    /// Locale-specific prompt pack used to render every system-prompt
    /// section, intent instruction, engagement note, and section intro.
    /// Selected from `learner.profile.locale` at construction time —
    /// the locale is bound for the lifetime of this manager (no
    /// in-session locale switching today).
    prompt_pack: Arc<dyn PromptPack>,
}

/// Output of the spawned post-response task: the extracted concepts
/// (and their turn indices for syncing back into in-memory
/// `Session.turns`) plus the comprehension assessments. Returned
/// through the `JoinHandle` so `apply_post_response_outcome` can apply
/// both to in-memory state at the next-turn boundary.
struct PostResponseResult {
    extraction: ExtractionPart,
    comprehension: primer_core::comprehension::ComprehensionResult,
}

/// The extraction portion of the post-response result.
struct ExtractionPart {
    child_turn_index: usize,
    primer_turn_index: usize,
    extraction: ConceptExtraction,
}

/// Outcome of `drain_classification`. `Some((abort, result))` when a task
/// was pending; `None` when not. The abort handle lets the apply step
/// abort on timeout. Aliased so the parallel-await path can name the
/// cross-future result type without spelling out the full nested Result.
type ClassificationOutcome = Option<(
    tokio::task::AbortHandle,
    std::result::Result<
        std::result::Result<Option<EngagementAssessment>, tokio::task::JoinError>,
        tokio::time::error::Elapsed,
    >,
)>;

/// Outcome of `drain_post_response`. `Some(result)` when a task was
/// pending; `None` when not. No abort handle — post-response tasks are
/// detached on timeout (the spawned DB writes still complete in the
/// background) rather than aborted.
type PostResponseOutcome = Option<
    std::result::Result<
        std::result::Result<Option<PostResponseResult>, tokio::task::JoinError>,
        tokio::time::error::Elapsed,
    >,
>;

impl<'a> DialogueManager<'a> {
    /// Returns the current wallclock for break-gate decisions. Tests
    /// can override via `clock_override`; production always reads
    /// `chrono::Utc::now()`.
    #[allow(dead_code)] // Used by Task 9 (respond_to_streaming gate).
    pub(super) fn now(&self) -> chrono::DateTime<chrono::Utc> {
        #[cfg(test)]
        if let Some(t) = self.clock_override {
            return t;
        }
        chrono::Utc::now()
    }

    #[cfg(test)]
    pub fn last_break_suggested_at_for_test(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.last_break_suggested_at
    }

    #[cfg(test)]
    pub fn set_last_break_suggested_at_for_test(
        &mut self,
        v: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        self.last_break_suggested_at = v;
    }

    #[cfg(test)]
    pub fn set_clock_for_test(&mut self, t: chrono::DateTime<chrono::Utc>) {
        self.clock_override = Some(t);
    }
}

#[cfg(test)]
mod tests {
    mod background_tests;
    mod lifecycle_tests;
    mod turn_tests;
}
