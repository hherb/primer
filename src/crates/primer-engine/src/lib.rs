//! Shared wiring helpers for the Primer binaries.
//!
//! `primer-cli` and `primer-gui` both need the same backend-construction
//! matrix (main backend → classifier → extractor → comprehension →
//! embedder), the same filesystem path resolution for the per-learner
//! session DB, and the same learner-reconciliation logic on launch.
//!
//! This crate hosts those helpers so the two binaries stay in lockstep
//! without code duplication. The helpers are pure (or near-pure) and
//! carry no behaviour change versus their previous home in `primer-cli`.

pub mod learner;
pub mod paths;
pub mod wiring;

pub use learner::{
    create_learner_with_id, reconcile_persisted_learner, verify_resume_locale_match,
};
pub use paths::{
    IN_MEMORY, PRIMER_HOME_DIR, resolve_session_db_path, should_show_first_run_banner, slug,
};
pub use wiring::{
    BackendParams, build_backend, build_classifier, build_comprehension, build_extractor,
    build_fastembed_embedder, build_ollama_embedder,
};
