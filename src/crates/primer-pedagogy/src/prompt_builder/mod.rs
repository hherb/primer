//! System prompt construction.
//!
//! The prompt builder takes the current conversation state, the learner model,
//! and any retrieved knowledge passages, and constructs the system prompt
//! that instructs the LLM how to behave.
//!
//! This is where the Socratic method is encoded — not in the model's weights,
//! but in the instructions we give it.
//!
//! Split by responsibility (behaviour-preserving):
//! - [`system_prompt`] — the `build_system_prompt_*` family + the shared
//!   `assemble_system_prompt` core (base/intent/engagement/summary/retrieved/
//!   vocab/knowledge assembly, budgeted and unbudgeted) + `truncate_passages`.
//! - [`prompt`] — `build_messages` + the `build_prompt_*` family that pairs a
//!   system prompt with the chat-message timeline.
//! - [`intent`] — `decide_intent*` (the Socratic brain) + `extract_active_concepts`
//!   + the opener/assertion classification helpers.
//!
//! `mod.rs` owns the process-wide cached English pack ([`english_pack`]) that all
//! three submodules share, and re-exports every public function so the external
//! `prompt_builder::<name>` paths are unchanged.

use std::sync::OnceLock;

use primer_core::i18n::Locale;

use crate::prompt_pack::{self, PromptPack};

mod intent;
mod prompt;
mod system_prompt;

pub use intent::{
    decide_intent, decide_intent_at, decide_intent_at_with_pack, decide_intent_with_pack,
    extract_active_concepts,
};
pub use prompt::{
    build_messages, build_prompt, build_prompt_with_pack, build_prompt_with_pack_and_vocab,
    build_prompt_within_budget_with_pack_and_vocab,
};
pub use system_prompt::{
    build_system_prompt, build_system_prompt_with_pack, build_system_prompt_with_pack_and_vocab,
    build_system_prompt_within_budget_with_pack_and_vocab, truncate_passages,
};

// The (untouched) `tests.rs` reaches these intent classifiers and the `Passage`
// type by their bare names via `use super::*`. They lived at module scope in the
// pre-split flat file; re-import them here (test-only) so the move into the
// `intent` / `system_prompt` submodules stays invisible to the test file.
#[cfg(test)]
use intent::{is_factual_question, is_factual_question_with_pack};
#[cfg(test)]
use primer_core::knowledge::Passage;

/// Process-wide cached English pack used by the no-pack convenience
/// wrappers (`decide_intent`, `is_factual_question`, and the
/// existing-signature `build_system_prompt` / `build_prompt`). The
/// dialogue manager constructs and threads its own locale-specific
/// pack through `*_with_pack` variants instead of consulting this
/// singleton — same code, different entry point.
///
/// Lifetime note: the `Arc<dyn PromptPack>` lives in a function-scoped
/// `static`, so it has `'static` lifetime. `Arc::as_ref` returns a
/// reference whose lifetime is tied to the `Arc`'s — here, also
/// `'static`. The returned `&dyn PromptPack` is therefore safe to hand
/// to call sites that don't retain the `Arc`.
fn english_pack() -> &'static dyn PromptPack {
    static CELL: OnceLock<std::sync::Arc<dyn PromptPack>> = OnceLock::new();
    CELL.get_or_init(|| prompt_pack::load_cached(Locale::English).expect("english pack loads"))
        .as_ref()
}

#[cfg(test)]
mod tests;
