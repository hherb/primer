//! System-prompt assembly: the `build_system_prompt_*` builder family plus the
//! shared [`assemble_system_prompt`] core that lays out the base prompt, intent
//! instruction, engagement note, break suggestion, and the optional
//! summary / retrieved / vocab / knowledge sections (budgeted and unbudgeted).

use primer_core::conversation::{PedagogicalIntent, Speaker, Turn};
use primer_core::knowledge::Passage;
use primer_core::learner::{ConceptState, LearnerModel};

use crate::prompt_pack::PromptPack;

/// Build the system prompt for the next LLM call using the locale's
/// [`PromptPack`] for every piece of pedagogical text.
///
/// The system prompt varies based on:
/// - The child's age and developmental stage
/// - Their current engagement state
/// - What concepts are active in the conversation
/// - What the dialogue manager wants to accomplish next
/// - Long-term memory: a rolling summary of pre-window turns plus
///   FTS5-retrieved older turns relevant to the current input
///
/// `summary` and `retrieved_older` may be empty: short sessions stay
/// inside the active window so neither is needed. When non-empty they
/// live as system-prompt sections so the chat-message timeline (the
/// last N turns) stays linear and coherent.
pub fn build_system_prompt_with_pack(
    pack: &dyn PromptPack,
    learner: &LearnerModel,
    intent: PedagogicalIntent,
    knowledge_context: &[Passage],
    summary: &str,
    retrieved_older: &[Turn],
) -> String {
    build_system_prompt_with_pack_and_vocab(
        pack,
        learner,
        intent,
        knowledge_context,
        summary,
        retrieved_older,
        &[],
        0,
    )
}

/// Build the system prompt with a vocabulary review section in addition
/// to the existing knowledge / summary / retrieved sections.
///
/// `due_vocab` is the slice of due concepts (typically from
/// [`primer_core::vocab::due_concepts`]). Empty → vocab section omitted.
/// Section order: base / intent / engagement / summary / retrieved /
/// vocab / knowledge.
///
/// The vocab section is the LLM-facing hint list for the spaced-repetition
/// scheduler. It is rendered in English regardless of locale (the LLM
/// consumes it; the child never sees this) and explicitly tells the
/// model to weave words in only if topically relevant — no drilling.
#[allow(clippy::too_many_arguments)]
pub fn build_system_prompt_with_pack_and_vocab(
    pack: &dyn PromptPack,
    learner: &LearnerModel,
    intent: PedagogicalIntent,
    knowledge_context: &[Passage],
    summary: &str,
    retrieved_older: &[Turn],
    due_vocab: &[&ConceptState],
    break_minutes: u32,
) -> String {
    assemble_system_prompt(
        pack,
        learner,
        intent,
        knowledge_context,
        summary,
        retrieved_older,
        due_vocab,
        break_minutes,
        None,
    )
}

/// Like [`build_system_prompt_with_pack_and_vocab`] but caps the system
/// prompt at `system_budget` tokens (estimated via
/// [`primer_core::prompt_budget::estimate_tokens`]).
///
/// Used by the dialogue manager for small-context backends (the Qualcomm
/// NPU `QnnBackend` runs a 2048-token Genie context). The **pedagogical
/// core** — base prompt + intent instruction + engagement note + break
/// suggestion — is always kept; the optional sections are dropped to fit,
/// in ascending pedagogical value (vocab review first, then retrieved
/// turns, then the rolling summary, then knowledge passages). Knowledge
/// passages should already be truncated by the caller (see
/// [`primer_core::prompt_budget::truncate_to_tokens`]); this function only
/// decides which whole sections fit.
#[allow(clippy::too_many_arguments)]
pub fn build_system_prompt_within_budget_with_pack_and_vocab(
    pack: &dyn PromptPack,
    learner: &LearnerModel,
    intent: PedagogicalIntent,
    knowledge_context: &[Passage],
    summary: &str,
    retrieved_older: &[Turn],
    due_vocab: &[&ConceptState],
    break_minutes: u32,
    system_budget: usize,
) -> String {
    assemble_system_prompt(
        pack,
        learner,
        intent,
        knowledge_context,
        summary,
        retrieved_older,
        due_vocab,
        break_minutes,
        Some(system_budget),
    )
}

/// Truncate each passage's body to at most `max_tokens` tokens
/// (sentence-boundary aware, via
/// [`primer_core::prompt_budget::truncate_to_tokens`]), leaving the id,
/// source, and score untouched. Used by the dialogue manager to shrink
/// whole wiki/seed passages to their relevant lead before injecting them
/// into a small-context system prompt.
pub fn truncate_passages(passages: &[Passage], max_tokens: usize) -> Vec<Passage> {
    passages
        .iter()
        .map(|p| Passage {
            text: primer_core::prompt_budget::truncate_to_tokens(&p.text, max_tokens),
            ..p.clone()
        })
        .collect()
}

/// Shared implementation behind the budgeted and unbudgeted system-prompt
/// builders. `system_budget = None` reproduces the original unbounded
/// behaviour byte-for-byte; `Some(budget)` drops optional sections to fit
/// (see [`build_system_prompt_within_budget_with_pack_and_vocab`]).
#[allow(clippy::too_many_arguments)]
fn assemble_system_prompt(
    pack: &dyn PromptPack,
    learner: &LearnerModel,
    intent: PedagogicalIntent,
    knowledge_context: &[Passage],
    summary: &str,
    retrieved_older: &[Turn],
    due_vocab: &[&ConceptState],
    break_minutes: u32,
    system_budget: Option<usize>,
) -> String {
    let age = learner.profile.age;
    let name = &learner.profile.name;

    let base = pack.render_base(name, age);
    let intent_instruction = pack.intent_instruction(intent);

    let engagement_note_body = pack.engagement_note(learner.current_engagement);
    let engagement_note: String = if engagement_note_body.is_empty() {
        String::new()
    } else {
        format!("\n\n{engagement_note_body}")
    };

    let break_suggestion_section = if intent == PedagogicalIntent::SuggestBreak {
        let intro = pack.break_suggestion_intro(break_minutes);
        format!("\n\n{intro}")
    } else {
        String::new()
    };

    let knowledge_section = if knowledge_context.is_empty() {
        String::new()
    } else {
        let passages: String = knowledge_context
            .iter()
            .map(|p| format!("[Source: {}]\n{}", p.source, p.text))
            .collect::<Vec<_>>()
            .join("\n\n");
        let intro = pack.knowledge_intro(age);
        format!("\n\n{intro}\n\n{passages}")
    };

    let summary_section = if summary.trim().is_empty() {
        String::new()
    } else {
        let intro = pack.summary_intro();
        format!("\n\n{intro}\n\n{summary}")
    };

    let retrieved_section = if retrieved_older.is_empty() {
        String::new()
    } else {
        let lines: String = retrieved_older
            .iter()
            .map(|t| {
                let who = match t.speaker {
                    Speaker::Child => pack.child_label(),
                    Speaker::Primer => pack.primer_label(),
                };
                format!("- [{who}] {}", t.text)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let intro = pack.retrieved_intro();
        format!("\n\n{intro}\n\n{lines}")
    };

    let vocab_section = if due_vocab.is_empty() {
        String::new()
    } else {
        let now = chrono::Utc::now();
        let lines: String = due_vocab
            .iter()
            .map(|c| {
                let days_ago = c
                    .last_encountered
                    .map(|last| days_since(last, now))
                    .unwrap_or(0);
                format!(
                    "- {} (depth: {}, last seen {} day{} ago)",
                    c.concept_id,
                    c.depth,
                    days_ago,
                    if days_ago == 1 { "" } else { "s" }
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let intro = pack.vocab_review_intro();
        format!("\n\n{intro}\n\n{lines}")
    };

    // The pedagogical core is never dropped — only the optional
    // memory/knowledge/vocab sections are gated by the budget.
    let core = format!("{base}\n\n{intent_instruction}{engagement_note}{break_suggestion_section}");

    let (summary_section, retrieved_section, vocab_section, knowledge_section) = match system_budget
    {
        None => (
            summary_section,
            retrieved_section,
            vocab_section,
            knowledge_section,
        ),
        Some(budget) => {
            use primer_core::prompt_budget::{estimate_tokens, select_sections};
            let remaining = budget.saturating_sub(estimate_tokens(&core));
            // Value order (most valuable first): knowledge grounds the
            // answer, the summary carries cross-window memory, retrieved
            // turns add session context, vocab hints are the least
            // critical. `select_sections` keeps the prefix that fits.
            let costs = [
                estimate_tokens(&knowledge_section),
                estimate_tokens(&summary_section),
                estimate_tokens(&retrieved_section),
                estimate_tokens(&vocab_section),
            ];
            let keep = select_sections(remaining, &costs);
            let gate = |keep: bool, s: String| if keep { s } else { String::new() };
            (
                gate(keep[1], summary_section),
                gate(keep[2], retrieved_section),
                gate(keep[3], vocab_section),
                gate(keep[0], knowledge_section),
            )
        }
    };

    format!("{core}{summary_section}{retrieved_section}{vocab_section}{knowledge_section}")
}

/// Render `now - last` as integer days, floored, non-negative.
/// Used by the vocab review section. Returns 0 for `now <= last`.
fn days_since(last: chrono::DateTime<chrono::Utc>, now: chrono::DateTime<chrono::Utc>) -> i64 {
    (now - last).num_days().max(0)
}

/// Convenience wrapper consulting the process-wide cached English pack.
/// Used by tests and by any caller that hasn't been threaded a locale.
pub fn build_system_prompt(
    learner: &LearnerModel,
    intent: PedagogicalIntent,
    knowledge_context: &[Passage],
    summary: &str,
    retrieved_older: &[Turn],
) -> String {
    build_system_prompt_with_pack(
        super::english_pack(),
        learner,
        intent,
        knowledge_context,
        summary,
        retrieved_older,
    )
}
