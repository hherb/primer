//! System prompt construction.
//!
//! The prompt builder takes the current conversation state, the learner model,
//! and any retrieved knowledge passages, and constructs the system prompt
//! that instructs the LLM how to behave.
//!
//! This is where the Socratic method is encoded — not in the model's weights,
//! but in the instructions we give it.

use std::sync::OnceLock;

use primer_core::conversation::{PedagogicalIntent, Session, Speaker, Turn};
use primer_core::i18n::Locale;
use primer_core::inference::{Message, Prompt, Role};
use primer_core::knowledge::Passage;
use primer_core::learner::{ConceptState, LearnerModel, UnderstandingDepth};

use crate::prompt_pack::{self, PromptPack};

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
        english_pack(),
        learner,
        intent,
        knowledge_context,
        summary,
        retrieved_older,
    )
}

/// Convert a conversation session into the messages array for the LLM prompt.
pub fn build_messages(session: &Session, context_turns: usize) -> Vec<Message> {
    session
        .recent_turns(context_turns)
        .iter()
        .map(|turn| Message {
            role: match turn.speaker {
                primer_core::conversation::Speaker::Child => Role::User,
                primer_core::conversation::Speaker::Primer => Role::Assistant,
            },
            content: turn.text.clone(),
        })
        .collect()
}

/// Assemble the complete prompt from components using the supplied
/// [`PromptPack`].
///
/// `summary` and `retrieved_older` carry long-term memory: the rolling
/// LLM-generated condensation of pre-window turns and the FTS5-retrieved
/// older turns relevant to the latest child input. Both are injected
/// into the system prompt; the chat `messages` list stays exactly equal
/// to `session.recent_turns(context_turns)` so the timeline the model
/// sees as "the conversation" is linear.
#[allow(clippy::too_many_arguments)]
pub fn build_prompt_with_pack(
    pack: &dyn PromptPack,
    learner: &LearnerModel,
    session: &Session,
    intent: PedagogicalIntent,
    knowledge_context: &[Passage],
    summary: &str,
    retrieved_older: &[Turn],
    context_turns: usize,
) -> Prompt {
    build_prompt_with_pack_and_vocab(
        pack,
        learner,
        session,
        intent,
        knowledge_context,
        summary,
        retrieved_older,
        context_turns,
        &[],
        0,
    )
}

/// Like [`build_prompt_with_pack`] but threads `due_vocab` and
/// `break_minutes` through to the system-prompt builder. The dialogue
/// manager uses this variant; every other caller can keep using the
/// no-vocab wrapper.
#[allow(clippy::too_many_arguments)]
pub fn build_prompt_with_pack_and_vocab(
    pack: &dyn PromptPack,
    learner: &LearnerModel,
    session: &Session,
    intent: PedagogicalIntent,
    knowledge_context: &[Passage],
    summary: &str,
    retrieved_older: &[Turn],
    context_turns: usize,
    due_vocab: &[&ConceptState],
    break_minutes: u32,
) -> Prompt {
    Prompt {
        system: build_system_prompt_with_pack_and_vocab(
            pack,
            learner,
            intent,
            knowledge_context,
            summary,
            retrieved_older,
            due_vocab,
            break_minutes,
        ),
        messages: build_messages(session, context_turns),
    }
}

/// Like [`build_prompt_with_pack_and_vocab`] but caps the *system prompt*
/// at `system_budget` tokens for small-context backends (the Qualcomm NPU
/// `QnnBackend` runs a 2048-token Genie context). The chat `messages`
/// list is unchanged — it is already bounded by `context_turns` (which
/// the dialogue manager shrinks for small-context backends via
/// [`primer_core::config::PedagogyConfig::effective_context_window_turns`]).
/// Knowledge passages should already be truncated by the caller.
#[allow(clippy::too_many_arguments)]
pub fn build_prompt_within_budget_with_pack_and_vocab(
    pack: &dyn PromptPack,
    learner: &LearnerModel,
    session: &Session,
    intent: PedagogicalIntent,
    knowledge_context: &[Passage],
    summary: &str,
    retrieved_older: &[Turn],
    context_turns: usize,
    due_vocab: &[&ConceptState],
    break_minutes: u32,
    system_budget: usize,
) -> Prompt {
    Prompt {
        system: build_system_prompt_within_budget_with_pack_and_vocab(
            pack,
            learner,
            intent,
            knowledge_context,
            summary,
            retrieved_older,
            due_vocab,
            break_minutes,
            system_budget,
        ),
        messages: build_messages(session, context_turns),
    }
}

/// Convenience wrapper using the process-wide cached English pack.
#[allow(clippy::too_many_arguments)]
pub fn build_prompt(
    learner: &LearnerModel,
    session: &Session,
    intent: PedagogicalIntent,
    knowledge_context: &[Passage],
    summary: &str,
    retrieved_older: &[Turn],
    context_turns: usize,
) -> Prompt {
    build_prompt_with_pack(
        english_pack(),
        learner,
        session,
        intent,
        knowledge_context,
        summary,
        retrieved_older,
        context_turns,
    )
}

// ─── Concept-depth helpers (used by dialogue manager) ─────────────────

/// Estimate what concepts are active in the current conversation,
/// based on simple keyword extraction from recent turns.
/// This is a placeholder — a production version would use embeddings.
pub fn extract_active_concepts(session: &Session, last_n: usize) -> Vec<String> {
    let _recent_text: String = session
        .recent_turns(last_n)
        .iter()
        .map(|t| t.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    // Placeholder: extract concepts mentioned in turn metadata.
    session
        .recent_turns(last_n)
        .iter()
        .flat_map(|t| t.concepts.iter().cloned())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect()
}

/// Return `true` if `text` looks like a direct factual lookup,
/// using `pack`'s `factual_prefixes()` list. Returns `false` if the
/// list is empty (e.g. for languages where prefix matching doesn't
/// apply — Japanese, Mandarin) and `decide_intent` falls back to the
/// LLM-based classifier in that case.
///
/// Only a small set of opening phrases qualify in English: "what
/// is/are/does", "what's", and "how does/do/is/are". The trailing
/// space in each prefix prevents partial-word matches ("whatever",
/// "howdy"). Exploratory forms ("what if", "what about") and "why"
/// questions are intentionally excluded — those are Socratic-richer
/// and should not be short-circuited with a direct answer.
fn is_factual_question_with_pack(pack: &dyn PromptPack, text: &str) -> bool {
    let prefixes = pack.factual_prefixes();
    if prefixes.is_empty() {
        return false;
    }
    let lowered = text.trim().to_lowercase();
    prefixes.iter().any(|p| lowered.starts_with(p.as_str()))
}

/// Convenience wrapper using the process-wide cached English pack.
/// Used only by tests today; the production path goes through
/// `is_factual_question_with_pack`.
#[cfg(test)]
fn is_factual_question(text: &str) -> bool {
    is_factual_question_with_pack(english_pack(), text)
}

/// Decide the next pedagogical intent based on the learner model
/// and conversation history.
///
/// This is a thin wrapper around [`decide_intent_at`] that injects
/// `chrono::Utc::now()` as the reference time. Production code calls
/// `decide_intent_at_with_pack` (locale-aware); this no-pack variant
/// uses the cached English pack for tests and English-only call paths.
pub fn decide_intent(learner: &LearnerModel, session: &Session) -> PedagogicalIntent {
    decide_intent_at(learner, session, chrono::Utc::now())
}

/// Locale-aware variant of [`decide_intent`].
pub fn decide_intent_with_pack(
    pack: &dyn PromptPack,
    learner: &LearnerModel,
    session: &Session,
) -> PedagogicalIntent {
    decide_intent_at_with_pack(
        pack,
        learner,
        session,
        chrono::Utc::now(),
        primer_core::session_timing::BreakGate::disabled(),
    )
}

/// Time-aware core of [`decide_intent`].
pub fn decide_intent_at(
    learner: &LearnerModel,
    session: &Session,
    now: chrono::DateTime<chrono::Utc>,
) -> PedagogicalIntent {
    decide_intent_at_with_pack(
        english_pack(),
        learner,
        session,
        now,
        primer_core::session_timing::BreakGate::disabled(),
    )
}

/// Time-aware, locale-aware core. Accepts an explicit `now` so tests
/// can backdate sessions deterministically without real-clock races.
/// The `Disengaging` branch uses `now` together with `session.started_at`
/// to distinguish an early disengagement (encourage rather than close)
/// from a sustained one (suggest session close).
pub fn decide_intent_at_with_pack(
    pack: &dyn PromptPack,
    learner: &LearnerModel,
    session: &Session,
    now: chrono::DateTime<chrono::Utc>,
    break_gate: primer_core::session_timing::BreakGate,
) -> PedagogicalIntent {
    use primer_core::learner::EngagementState;
    // Engagement-state overrides fire before turn analysis.
    match learner.current_engagement {
        EngagementState::FrustratedStuck => return PedagogicalIntent::Scaffolding,
        EngagementState::FrustratedTrying => return PedagogicalIntent::Encouragement,
        EngagementState::Disengaging => {
            let elapsed = now.signed_duration_since(session.started_at);
            let elapsed_secs = elapsed.num_seconds().max(0) as u64;
            let threshold = learner.preferences.early_disengagement_threshold;
            return if std::time::Duration::from_secs(elapsed_secs) < threshold {
                PedagogicalIntent::Encouragement
            } else {
                PedagogicalIntent::SessionClose
            };
        }
        EngagementState::Engaged | EngagementState::Reflecting | EngagementState::Unknown => { /* fall through to turn analysis */
        }
    }

    // Break-suggestion gate: fires after engagement-state overrides
    // (a frustrated child past 30 minutes still gets Scaffolding,
    // not SuggestBreak — fix the frustration first) but before turn
    // analysis so it overrides the natural Socratic flow.
    if primer_core::session_timing::should_suggest_break_now(
        now,
        session.started_at,
        break_gate.last_suggested_at,
        break_gate.interval_minutes,
    ) {
        return PedagogicalIntent::SuggestBreak;
    }

    // Look at the last turn — if it was a child's response, decide
    // whether to probe comprehension or extend.
    if let Some(last) = session.turns.last() {
        if last.speaker == primer_core::conversation::Speaker::Child {
            // Gap 2: factual-question pattern routing
            if is_factual_question_with_pack(pack, &last.text) {
                let prior_was_direct_answer = session
                    .turns
                    .iter()
                    .rev()
                    .skip(1)
                    .find(|t| t.speaker == primer_core::conversation::Speaker::Primer)
                    .and_then(|t| t.intent)
                    .map(|i| i == PedagogicalIntent::DirectAnswer)
                    .unwrap_or(false);
                return if prior_was_direct_answer {
                    PedagogicalIntent::AnswerThenPivot
                } else {
                    PedagogicalIntent::DirectAnswer
                };
            }

            // Simple heuristic: short responses likely need probing,
            // longer responses might demonstrate understanding.
            if last.text.split_whitespace().count() < crate::consts::SHORT_TURN_WORD_BOUNDARY {
                return PedagogicalIntent::ComprehensionCheck;
            }

            // Check if any active concepts are at Comprehension level
            // or above — if so, extend.
            let active = extract_active_concepts(session, crate::consts::ACTIVE_CONCEPT_LOOKBACK);
            let has_understood = active.iter().any(|c| {
                learner
                    .concepts
                    .iter()
                    .any(|cs| &cs.concept_id == c && cs.depth >= UnderstandingDepth::Comprehension)
            });

            if has_understood {
                return PedagogicalIntent::Extension;
            }
        }
    }

    // Default: ask a Socratic question.
    PedagogicalIntent::SocraticQuestion
}

#[cfg(test)]
mod tests;
