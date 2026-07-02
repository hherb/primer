//! Pedagogical-intent decision — the Socratic brain.
//!
//! [`decide_intent_at_with_pack`] is the routing core; the `decide_intent*`
//! wrappers inject `now`/the cached English pack. The opener/assertion
//! classifiers ([`is_factual_question_with_pack`], `is_confusion_with_pack`,
//! `is_probeable_assertion_with_pack`) and [`extract_active_concepts`] feed it.

use primer_core::conversation::{PedagogicalIntent, Session};
use primer_core::learner::{LearnerModel, UnderstandingDepth};

use crate::prompt_pack::PromptPack;

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
pub(super) fn is_factual_question_with_pack(pack: &dyn PromptPack, text: &str) -> bool {
    matches_opener(pack.factual_prefixes(), text)
}

/// True when `text`'s lowercased, trimmed opening matches any prefix in
/// `prefixes`. Empty list ⇒ `false` (the locale opts out). Shared by the
/// factual-question, confusion, and request classifiers so all three use
/// one matching rule.
fn matches_opener(prefixes: &[String], text: &str) -> bool {
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
pub(super) fn is_factual_question(text: &str) -> bool {
    is_factual_question_with_pack(super::english_pack(), text)
}

/// True when `text` is an epistemic hedge / non-answer ("I don't know",
/// "I'm not sure"), per `pack.confusion_openers()`. Such a turn routes to
/// `ComprehensionCheck`, not `ProbeReasoning` — a confused child needs
/// scaffolding, not "how do you know?".
fn is_confusion_with_pack(pack: &dyn PromptPack, text: &str) -> bool {
    matches_opener(pack.confusion_openers(), text)
}

/// True when `text` is a *probe-able assertion*: a substantive declarative
/// claim worth a "how do you know?" response, rather than a question or a
/// request directed at the Primer.
///
/// A turn qualifies only when it is **not** a question (no trailing `?`
/// after trimming) **and** does not open with a request / meta-talk marker
/// from `pack.request_openers()` ("I want", "tell me", "let's"…). Questions
/// and requests stay on the `SocraticQuestion` default; only genuine claims
/// route to `ProbeReasoning`. Factual questions are already diverted earlier
/// in `decide_intent_at_with_pack`, and confusion hedges are handled by
/// `is_confusion_with_pack` before this is consulted.
///
/// The bias is deliberately toward *not* firing: the principle is also
/// stated as prose in the system prompt, so a missed claim is still nudged
/// by the LLM, whereas a false "how do you know?" at a child's story
/// request has no backstop.
fn is_probeable_assertion_with_pack(pack: &dyn PromptPack, text: &str) -> bool {
    if text.trim_end().ends_with('?') {
        return false;
    }
    !matches_opener(pack.request_openers(), text)
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
        super::english_pack(),
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

            // A long turn that opens with a confusion/non-answer marker
            // ("I don't know…") is a signal to scaffold, not to probe
            // reasoning — route it to ComprehensionCheck like a short
            // answer rather than asking "how do you know?".
            if is_confusion_with_pack(pack, &last.text) {
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

            // The child asserted a substantive claim they have not yet
            // shown they understand. Ask how they know / how they could
            // check, rather than defaulting to a fresh Socratic question.
            // Questions and requests/meta-talk ("tell me…", "I want…")
            // fail is_probeable_assertion_with_pack and stay on the
            // default path below.
            if is_probeable_assertion_with_pack(pack, &last.text) {
                return PedagogicalIntent::ProbeReasoning;
            }
        }
    }

    // Default: ask a Socratic question.
    PedagogicalIntent::SocraticQuestion
}
