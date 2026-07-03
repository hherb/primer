//! Pedagogy-engine defaults that aren't specific to one feature module.
//!
//! The two context-window constants back [`crate::config::PedagogyConfig`]:
//! the global value is the large-context (cloud) default; the
//! `_SMALL_CONTEXT` value is used when the active backend is detected as a
//! small-context (≈4K-token) backend via
//! [`crate::backend::is_small_context_backend`] (Phase 1.2 step 1.2.5).

/// Recent-turn window for the global (cloud / large-context) path:
/// how many of the most recent conversation turns are sent to the LLM
/// as chat messages each turn. Pre-window turns reach the model only
/// through the rolling summary and long-term-memory retrieval.
pub const DEFAULT_CONTEXT_WINDOW_TURNS: usize = 20;

/// Recent-turn window for small-context (≈4K-token) backends. A
/// 4K-token budget must hold the system prompt, retrieved passages,
/// the rolling summary, *and* the recent turns — ~12 turns of
/// child+Primer exchange leaves headroom for the rest where the
/// 20-turn default would overflow. Phase 1.2 step 1.2.5.
pub const DEFAULT_CONTEXT_WINDOW_TURNS_SMALL_CONTEXT: usize = 8;
