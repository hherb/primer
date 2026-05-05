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

    format!(
        "{base}\n\n{intent_instruction}{engagement_note}\
         {summary_section}{retrieved_section}{vocab_section}{knowledge_section}"
    )
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
    )
}

/// Like [`build_prompt_with_pack`] but threads `due_vocab` through to
/// the system-prompt builder. The dialogue manager uses this variant;
/// every other caller can keep using the no-vocab wrapper.
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
    decide_intent_at_with_pack(pack, learner, session, chrono::Utc::now())
}

/// Time-aware core of [`decide_intent`].
pub fn decide_intent_at(
    learner: &LearnerModel,
    session: &Session,
    now: chrono::DateTime<chrono::Utc>,
) -> PedagogicalIntent {
    decide_intent_at_with_pack(english_pack(), learner, session, now)
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
mod tests {
    //! Characterization tests for `decide_intent`.
    //!
    //! These pin down the heuristic's *current* behaviour, not its ideal
    //! behaviour. The brief in `primer_next_session.md` lists several
    //! pedagogical cases (e.g. Encouragement on frustration, DirectAnswer
    //! on "what is X?") that the current implementation does **not** cover;
    //! those gaps are flagged in the session report rather than encoded as
    //! failing tests here.
    use super::*;
    use chrono::Utc;
    use primer_core::conversation::{PedagogicalIntent, Session, Speaker, Turn};
    use primer_core::learner::{
        ConceptState, EngagementState, LearnerModel, LearnerProfile, LearningPreferences,
        UnderstandingDepth,
    };
    use uuid::Uuid;

    fn learner_with(engagement: EngagementState, concepts: Vec<ConceptState>) -> LearnerModel {
        LearnerModel {
            profile: LearnerProfile {
                id: Uuid::new_v4(),
                name: "Tester".to_string(),
                age: 8,
                languages: vec!["en".to_string()],
                locale: primer_core::i18n::Locale::English,
                created_at: Utc::now(),
                last_active: Utc::now(),
            },
            concepts,
            preferences: LearningPreferences::default(),
            current_engagement: engagement,
            recent_assessments: vec![],
        }
    }

    fn empty_session() -> Session {
        Session::new(Uuid::new_v4())
    }

    fn make_session_started_seconds_ago(seconds_ago: i64) -> Session {
        let mut s = Session::new(Uuid::new_v4());
        s.started_at = Utc::now() - chrono::Duration::seconds(seconds_ago);
        s
    }

    fn child_turn(text: &str, concepts: Vec<String>) -> Turn {
        Turn {
            speaker: Speaker::Child,
            text: text.to_string(),
            timestamp: Utc::now(),
            intent: None,
            concepts,
        }
    }

    fn primer_turn(text: &str, concepts: Vec<String>) -> Turn {
        Turn {
            speaker: Speaker::Primer,
            text: text.to_string(),
            timestamp: Utc::now(),
            intent: Some(PedagogicalIntent::SocraticQuestion),
            concepts,
        }
    }

    fn concept_at(id: &str, depth: UnderstandingDepth) -> ConceptState {
        ConceptState {
            concept_id: id.to_string(),
            depth,
            confidence: 0.8,
            encounter_count: 1,
            last_encountered: Some(Utc::now()),
            notes: vec![],
            box_level: 0,
        }
    }

    // ─── Engagement state takes precedence over turn analysis ─────────

    #[test]
    fn frustrated_stuck_returns_scaffolding() {
        let learner = learner_with(EngagementState::FrustratedStuck, vec![]);
        let session = empty_session();
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::Scaffolding
        );
    }

    #[test]
    fn frustrated_stuck_overrides_short_child_turn() {
        // Without frustration, a 1-word child turn would yield ComprehensionCheck.
        // The engagement check fires first.
        let learner = learner_with(EngagementState::FrustratedStuck, vec![]);
        let mut session = empty_session();
        session.add_turn(child_turn("yes", vec![]));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::Scaffolding
        );
    }

    #[test]
    fn frustrated_trying_returns_encouragement() {
        let learner = learner_with(EngagementState::FrustratedTrying, vec![]);
        let session = empty_session();
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::Encouragement
        );
    }

    #[test]
    fn frustrated_trying_overrides_short_child_turn() {
        let learner = learner_with(EngagementState::FrustratedTrying, vec![]);
        let mut session = empty_session();
        session.add_turn(child_turn("yes", vec![]));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::Encouragement
        );
    }

    #[test]
    fn disengaging_late_returns_session_close() {
        let learner = learner_with(EngagementState::Disengaging, vec![]);
        let session = make_session_started_seconds_ago(60 * 60); // 1 hour ago
        let now = Utc::now();
        assert_eq!(
            decide_intent_at(&learner, &session, now),
            PedagogicalIntent::SessionClose,
        );
    }

    #[test]
    fn disengaging_early_returns_encouragement() {
        let learner = learner_with(EngagementState::Disengaging, vec![]);
        let session = make_session_started_seconds_ago(60); // 1 minute ago
        let now = Utc::now();
        assert_eq!(
            decide_intent_at(&learner, &session, now),
            PedagogicalIntent::Encouragement,
        );
    }

    #[test]
    fn disengaging_at_threshold_returns_session_close() {
        use primer_core::learner::DEFAULT_EARLY_DISENGAGEMENT_SECS;
        let learner = learner_with(EngagementState::Disengaging, vec![]);
        let session = make_session_started_seconds_ago(DEFAULT_EARLY_DISENGAGEMENT_SECS as i64);
        let now = Utc::now();
        assert_eq!(
            decide_intent_at(&learner, &session, now),
            PedagogicalIntent::SessionClose,
        );
    }

    #[test]
    fn disengaging_just_after_threshold_returns_session_close() {
        use primer_core::learner::DEFAULT_EARLY_DISENGAGEMENT_SECS;
        let learner = learner_with(EngagementState::Disengaging, vec![]);
        let session =
            make_session_started_seconds_ago(DEFAULT_EARLY_DISENGAGEMENT_SECS as i64 + 60);
        let now = Utc::now();
        assert_eq!(
            decide_intent_at(&learner, &session, now),
            PedagogicalIntent::SessionClose,
        );
    }

    #[test]
    fn disengaging_late_overrides_short_child_turn_branch() {
        let learner = learner_with(EngagementState::Disengaging, vec![]);
        let mut session = make_session_started_seconds_ago(60 * 60); // 1 hour ago
        session.add_turn(child_turn("ok", vec![]));
        assert_eq!(
            decide_intent_at(&learner, &session, Utc::now()),
            PedagogicalIntent::SessionClose,
        );
    }

    // ─── Default path: engaged with no last child turn ────────────────

    #[test]
    fn empty_session_returns_socratic_question() {
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let session = empty_session();
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::SocraticQuestion,
        );
    }

    #[test]
    fn last_turn_primer_returns_socratic_question() {
        // The turn-based branches only fire when the last turn was the
        // child's. A bare Primer greeting falls through to the default.
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let mut session = empty_session();
        session.add_turn(primer_turn(
            "Hello, what are you curious about today?",
            vec![],
        ));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::SocraticQuestion,
        );
    }

    #[test]
    fn reflecting_engagement_falls_through_to_turn_logic() {
        // Reflecting is not Frustrated/Disengaging, so the heuristic
        // proceeds to inspect the last turn.
        let learner = learner_with(EngagementState::Reflecting, vec![]);
        let mut session = empty_session();
        session.add_turn(child_turn("yes", vec![]));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::ComprehensionCheck,
        );
    }

    // ─── Short child turn → ComprehensionCheck (boundary <10 words) ──

    #[test]
    fn short_child_turn_returns_comprehension_check() {
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let mut session = empty_session();
        session.add_turn(child_turn("I think it's a star", vec![]));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::ComprehensionCheck,
        );
    }

    #[test]
    fn nine_word_child_turn_is_short() {
        // 9 < 10 → short branch fires.
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let mut session = empty_session();
        session.add_turn(child_turn(
            "one two three four five six seven eight nine",
            vec![],
        ));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::ComprehensionCheck,
        );
    }

    #[test]
    fn ten_word_child_turn_is_not_short() {
        // 10 < 10 is false → falls through to the concept-depth check,
        // and with no understood concepts it lands on the default.
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let mut session = empty_session();
        session.add_turn(child_turn(
            "one two three four five six seven eight nine ten",
            vec![],
        ));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::SocraticQuestion,
        );
    }

    #[test]
    fn empty_child_turn_treated_as_short() {
        // split_whitespace() on "" yields 0; 0 < 10 → short branch.
        // Documents the current behaviour even though an empty input
        // arguably deserves a different response.
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let mut session = empty_session();
        session.add_turn(child_turn("", vec![]));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::ComprehensionCheck,
        );
    }

    // ─── Extension when an active concept is at Comprehension depth ──

    #[test]
    fn long_child_turn_with_understood_concept_returns_extension() {
        let learner = learner_with(
            EngagementState::Engaged,
            vec![concept_at("gravity", UnderstandingDepth::Comprehension)],
        );
        let mut session = empty_session();
        session.add_turn(child_turn(
            "gravity pulls everything down toward the centre of the earth always",
            vec!["gravity".to_string()],
        ));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::Extension,
        );
    }

    #[test]
    fn long_child_turn_with_concept_below_comprehension_returns_socratic_question() {
        // Recall < Comprehension, so the Extension gate stays closed.
        let learner = learner_with(
            EngagementState::Engaged,
            vec![concept_at("gravity", UnderstandingDepth::Recall)],
        );
        let mut session = empty_session();
        session.add_turn(child_turn(
            "gravity pulls everything down toward the centre of the earth always",
            vec!["gravity".to_string()],
        ));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::SocraticQuestion,
        );
    }

    #[test]
    fn long_child_turn_with_concept_at_analysis_returns_extension() {
        // Analysis > Comprehension also opens the Extension gate.
        let learner = learner_with(
            EngagementState::Engaged,
            vec![concept_at("gravity", UnderstandingDepth::Analysis)],
        );
        let mut session = empty_session();
        session.add_turn(child_turn(
            "gravity is what makes apples fall and keeps the moon in orbit too",
            vec!["gravity".to_string()],
        ));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::Extension,
        );
    }

    #[test]
    fn long_child_turn_with_unrelated_concept_returns_socratic_question() {
        // Active concept doesn't match any tracked concept_id → no Extension.
        let learner = learner_with(
            EngagementState::Engaged,
            vec![concept_at(
                "photosynthesis",
                UnderstandingDepth::Application,
            )],
        );
        let mut session = empty_session();
        session.add_turn(child_turn(
            "I think gravity is what holds the planets together in space somehow",
            vec!["gravity".to_string()],
        ));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::SocraticQuestion,
        );
    }

    #[test]
    fn extension_picks_up_concept_attached_to_recent_primer_turn() {
        // extract_active_concepts scans the last 4 turns regardless of
        // speaker, so a concept tagged on a Primer turn is still "active"
        // for the purposes of the Extension check.
        let learner = learner_with(
            EngagementState::Engaged,
            vec![concept_at("gravity", UnderstandingDepth::Comprehension)],
        );
        let mut session = empty_session();
        session.add_turn(primer_turn(
            "So gravity makes things fall down. Why do you think that is?",
            vec!["gravity".to_string()],
        ));
        // Long child turn with no concepts of its own.
        session.add_turn(child_turn(
            "because the earth is heavy and pulls everything toward its centre always",
            vec![],
        ));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::Extension,
        );
    }

    // ─── Factual-question routing (Gap 2) ────────────────────────────

    #[test]
    fn factual_question_what_is_returns_direct_answer() {
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let mut session = empty_session();
        session.add_turn(child_turn("What is gravity?", vec![]));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::DirectAnswer,
        );
    }

    #[test]
    fn factual_question_how_does_returns_direct_answer() {
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let mut session = empty_session();
        session.add_turn(child_turn("How does it work?", vec![]));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::DirectAnswer,
        );
    }

    #[test]
    fn factual_question_after_direct_answer_returns_answer_then_pivot() {
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let mut session = empty_session();
        session.add_turn(child_turn("What is gravity?", vec![]));
        let mut primer_t = primer_turn("Gravity is...", vec![]);
        primer_t.intent = Some(PedagogicalIntent::DirectAnswer);
        session.add_turn(primer_t);
        session.add_turn(child_turn("What is mass?", vec![]));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::AnswerThenPivot,
        );
    }

    #[test]
    fn factual_question_with_frustrated_state_still_routes_via_engagement() {
        // Engagement-state precedence preserved: frustrated kid asking
        // "what is X?" still gets the engagement branch (Scaffolding).
        let learner = learner_with(EngagementState::FrustratedStuck, vec![]);
        let mut session = empty_session();
        session.add_turn(child_turn("what is gravity?", vec![]));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::Scaffolding,
        );
    }

    #[test]
    fn non_factual_short_turn_still_returns_comprehension_check() {
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let mut session = empty_session();
        session.add_turn(child_turn("yes", vec![]));
        assert_eq!(
            decide_intent(&learner, &session),
            PedagogicalIntent::ComprehensionCheck,
        );
    }

    // ─── Long-term memory injection (summary + retrieved older) ──────

    fn build_default_prompt(
        summary: &str,
        retrieved_older: &[Turn],
    ) -> primer_core::inference::Prompt {
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let session = empty_session();
        build_prompt(
            &learner,
            &session,
            PedagogicalIntent::SocraticQuestion,
            &[],
            summary,
            retrieved_older,
            20,
        )
    }

    #[test]
    fn build_prompt_includes_summary_section_when_non_empty() {
        let prompt = build_default_prompt(
            "Earlier we explored why the sky is blue and what gravity feels like.",
            &[],
        );
        assert!(
            prompt.system.contains("Earlier in this conversation"),
            "summary section header should appear in system prompt"
        );
        assert!(
            prompt.system.contains("why the sky is blue"),
            "summary content should be in system prompt: {}",
            prompt.system
        );
    }

    #[test]
    fn build_prompt_omits_summary_section_when_empty() {
        let prompt = build_default_prompt("", &[]);
        assert!(
            !prompt.system.contains("Earlier in this conversation"),
            "no summary section when summary is empty"
        );
    }

    #[test]
    fn build_prompt_omits_summary_section_when_whitespace_only() {
        let prompt = build_default_prompt("   \n\t  ", &[]);
        assert!(
            !prompt.system.contains("Earlier in this conversation"),
            "whitespace-only summary should be treated as empty"
        );
    }

    #[test]
    fn build_prompt_includes_retrieved_prior_moments() {
        let retrieved = vec![
            child_turn("we talked about lightning last week", vec![]),
            primer_turn("yes, you wondered why thunder follows", vec![]),
        ];
        let prompt = build_default_prompt("", &retrieved);
        assert!(
            prompt.system.contains("Relevant prior moments"),
            "retrieved-moments section header should appear"
        );
        assert!(prompt.system.contains("lightning last week"));
        assert!(prompt.system.contains("[Child]"));
        assert!(prompt.system.contains("[Primer]"));
    }

    #[test]
    fn build_prompt_omits_retrieved_section_when_empty() {
        let prompt = build_default_prompt("", &[]);
        assert!(!prompt.system.contains("Relevant prior moments"));
    }

    // ─── is_factual_question ─────────────────────────────────────────────

    #[test]
    fn is_factual_question_matches_what_is() {
        assert!(is_factual_question("What is gravity?"));
        assert!(is_factual_question("what is gravity?"));
        assert!(is_factual_question("  WHAT IS gravity?  "));
    }

    #[test]
    fn is_factual_question_matches_how_does() {
        assert!(is_factual_question("how does it work"));
        assert!(is_factual_question("How do plants eat?"));
    }

    #[test]
    fn is_factual_question_does_not_match_partial_words() {
        // "whatever" must NOT trigger "what" — the prefix list uses trailing space.
        assert!(!is_factual_question("whatever"));
        assert!(!is_factual_question("howdy"));
    }

    #[test]
    fn is_factual_question_does_not_match_open_ended_what() {
        // "What if" / "What about" are exploratory, not factual lookups.
        assert!(!is_factual_question("what if we tried"));
        assert!(!is_factual_question("what about us"));
    }

    #[test]
    fn is_factual_question_drops_why_questions() {
        // "why" forms are deliberately left out — Socratic-richer.
        assert!(!is_factual_question("why is the sky blue"));
        assert!(!is_factual_question("why does it rain"));
    }

    /// Locales whose pack ships `factual_prefixes = []` opt out of the
    /// prefix-matching short-circuit; `is_factual_question_with_pack`
    /// must return `false` for every input so `decide_intent` falls
    /// through to the LLM-based engagement classifier.
    #[test]
    fn is_factual_question_with_pack_returns_false_for_empty_prefix_list() {
        use crate::prompt_pack::TomlPromptPack;
        // Synthetic pack with `factual_prefixes = []` — represents a
        // future locale (e.g. Japanese) where prefix matching doesn't
        // apply.
        let body = r#"
[meta]
language = "en"
language_name = "English"
bcp47 = "en-US"

[system_prompt]
base = "x"

[language_guidance]
ages_0_6 = ""
ages_7_9 = ""
ages_10_12 = ""
ages_13_plus = ""

[intent]
socratic_question = "x"
comprehension_check = "x"
scaffolding = "x"
encouragement = "x"
extension = "x"
direct_answer = "x"
answer_then_pivot = "x"
session_close = "x"
suggest_break = "x"

[engagement]
frustrated = ""
disengaging = ""

[sections]
knowledge_intro = ""
summary_intro = ""
retrieved_intro = ""
vocab_review_intro = ""

[labels]
child = "Child"
primer = "Primer"

[question_detection]
factual_prefixes = []
"#;
        let pack = TomlPromptPack::from_toml_str(Locale::English, body)
            .expect("synthetic pack with empty prefixes loads");
        // Inputs that the English pack would classify as factual must
        // now return false because the prefix list is empty.
        assert!(!is_factual_question_with_pack(&pack, "what is gravity?"));
        assert!(!is_factual_question_with_pack(&pack, "how does it work"));
        // And ordinary inputs still return false.
        assert!(!is_factual_question_with_pack(&pack, "why is the sky blue"));
        assert!(!is_factual_question_with_pack(&pack, ""));
    }

    #[test]
    fn build_prompt_chat_messages_remain_recent_window_only() {
        // Long-term memory (summary + retrieved older) lives in the
        // system prompt, NOT in the messages list. The messages stay
        // exactly equal to session.recent_turns(window).
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let mut session = empty_session();
        for i in 0..30 {
            session.add_turn(child_turn(&format!("turn {i}"), vec![]));
        }
        let retrieved = vec![child_turn("retrieved", vec![])];
        let prompt = build_prompt(
            &learner,
            &session,
            PedagogicalIntent::SocraticQuestion,
            &[],
            "summary text",
            &retrieved,
            20, // window
        );
        // 30 turns total, window 20 → messages are turns 10..30.
        assert_eq!(prompt.messages.len(), 20);
        assert_eq!(prompt.messages[0].content, "turn 10");
        assert_eq!(prompt.messages[19].content, "turn 29");
        // Summary and retrieved appeared in system prompt — not as messages.
        assert!(prompt.system.contains("summary text"));
        assert!(prompt.system.contains("retrieved"));
    }

    // ─── engagement_note coverage for new EngagementState variants ───

    #[test]
    fn build_prompt_includes_engagement_note_for_frustrated_stuck() {
        let learner = learner_with(EngagementState::FrustratedStuck, vec![]);
        let session = empty_session();
        let prompt = build_prompt(
            &learner,
            &session,
            PedagogicalIntent::Scaffolding,
            &[],
            "",
            &[],
            20,
        );
        assert!(
            prompt.system.contains("appears frustrated"),
            "engagement note should appear for FrustratedStuck"
        );
    }

    // ─── Snapshot tests: lock byte-identical English prompt output ────
    //
    // These tests are the regression guard for the prompt-pack refactor.
    // They lock both the precise length and the exact content of the
    // rendered system prompt for a representative matrix of inputs. If
    // anyone edits `prompts/en.toml` (or the rendering logic) and
    // changes the output, these tests fail loudly with the offending
    // before/after lengths or the offending differing substring.
    //
    // The locked lengths were measured against the pre-refactor
    // hardcoded strings; a passing test means the TOML pack reproduces
    // them character-for-character.
    //
    // Use the helpers below to add new matrix points: build a prompt
    // with the desired (age, intent, engagement, with_passages,
    // with_summary, with_retrieved) tuple and assert the substring
    // markers appear / don't appear as expected. The full-text snapshot
    // (`snapshot_canonical_prompt_locks_full_text`) keeps every byte of
    // one canonical scenario locked.

    fn snapshot_learner(age: u8, engagement: EngagementState) -> LearnerModel {
        LearnerModel {
            profile: LearnerProfile {
                id: Uuid::nil(),
                name: "Tester".to_string(),
                age,
                languages: vec!["en".to_string()],
                locale: primer_core::i18n::Locale::English,
                created_at: Utc::now(),
                last_active: Utc::now(),
            },
            concepts: vec![],
            preferences: LearningPreferences::default(),
            current_engagement: engagement,
            recent_assessments: vec![],
        }
    }

    fn snapshot_passage() -> primer_core::knowledge::Passage {
        primer_core::knowledge::Passage {
            id: "test-id".to_string(),
            source: "test-source".to_string(),
            text: "Test passage body.".to_string(),
            score: 1.0,
        }
    }

    #[test]
    fn snapshot_basic_socratic_question_for_8_year_old() {
        let learner = snapshot_learner(8, EngagementState::Engaged);
        let session = empty_session();
        let prompt = build_prompt(
            &learner,
            &session,
            PedagogicalIntent::SocraticQuestion,
            &[],
            "",
            &[],
            20,
        );
        // Markers from the base block.
        assert!(prompt.system.starts_with("You are the Primer"));
        assert!(prompt.system.contains("named Tester, age 8"));
        // The 7-9 age band's signature line.
        assert!(prompt.system.contains("primary school"));
        assert!(
            prompt
                .system
                .contains("Vocabulary discipline (applies at every age):")
        );
        // Intent instruction appended exactly once.
        assert_eq!(
            prompt.system.matches("guiding question").count(),
            2,
            "expected exactly two mentions: one in base principles (\"guiding question\"), one in intent instruction"
        );
        // No engagement note, no sections.
        assert!(!prompt.system.contains("appears frustrated"));
        assert!(!prompt.system.contains("losing interest"));
        assert!(!prompt.system.contains("Earlier in this conversation"));
        assert!(!prompt.system.contains("Relevant prior moments"));
        assert!(!prompt.system.contains("Relevant factual context"));
    }

    #[test]
    fn snapshot_scaffolding_for_5_year_old_with_passages_and_summary() {
        let learner = snapshot_learner(5, EngagementState::FrustratedStuck);
        let session = empty_session();
        let prompt = build_prompt(
            &learner,
            &session,
            PedagogicalIntent::Scaffolding,
            &[snapshot_passage()],
            "Earlier we explored gravity.",
            &[child_turn("we talked about lightning", vec![])],
            20,
        );
        // 0-6 age band signature line.
        assert!(prompt.system.contains("kindergarten"));
        // Frustrated note present.
        assert!(prompt.system.contains("appears frustrated"));
        // Scaffolding intent.
        assert!(prompt.system.contains("offer a concrete"));
        assert!(prompt.system.contains("Reduce the abstraction"));
        // All three optional sections present.
        assert!(prompt.system.contains("Earlier in this conversation"));
        assert!(prompt.system.contains("Relevant prior moments"));
        assert!(prompt.system.contains("Relevant factual context"));
        assert!(prompt.system.contains("rephrase for a 5-year-old"));
        assert!(prompt.system.contains("[Source: test-source]"));
        assert!(prompt.system.contains("[Child]"));
    }

    #[test]
    fn snapshot_extension_for_15_year_old() {
        let learner = snapshot_learner(15, EngagementState::Engaged);
        let session = empty_session();
        let prompt = build_prompt(
            &learner,
            &session,
            PedagogicalIntent::Extension,
            &[],
            "",
            &[],
            20,
        );
        // 13+ age band.
        assert!(prompt.system.contains("Adult-level vocabulary"));
        assert!(prompt.system.contains("articulate teenager"));
        // Extension instruction.
        assert!(prompt.system.contains("introduce a complication"));
        assert!(prompt.system.contains("counterexample"));
    }

    #[test]
    fn snapshot_disengaging_for_11_year_old() {
        let learner = snapshot_learner(11, EngagementState::Disengaging);
        let session = empty_session();
        let prompt = build_prompt(
            &learner,
            &session,
            PedagogicalIntent::SessionClose,
            &[],
            "",
            &[],
            20,
        );
        // 10-12 age band.
        assert!(prompt.system.contains("moderate sentence length"));
        // Disengaging note.
        assert!(prompt.system.contains("losing interest"));
        // SessionClose intent.
        assert!(prompt.system.contains("good stopping point"));
        assert!(prompt.system.contains("explored today"));
    }

    /// Full-text snapshot. The exact bytes of this rendered prompt
    /// are the regression boundary — any change to `en.toml` or the
    /// renderer that alters this output will fail this test loudly.
    #[test]
    fn snapshot_canonical_prompt_locks_full_text() {
        let learner = snapshot_learner(8, EngagementState::Engaged);
        let session = empty_session();
        let prompt = build_prompt(
            &learner,
            &session,
            PedagogicalIntent::SocraticQuestion,
            &[],
            "",
            &[],
            20,
        );
        let want = "You are the Primer — a patient, curious, Socratic learning companion for a child named Tester, age 8.\n\nYour core principles:\n- NEVER give a direct answer when you can ask a guiding question instead.\n- Ask questions that lead Tester toward discovering the answer themselves.\n- When Tester answers, assess whether they genuinely understand or are guessing/parroting.\n- If they understand: acknowledge it, then extend — \"Good. Now what if...?\"\n- If they're struggling: offer a concrete example, analogy, or story. Reduce abstraction.\n- If they ask a pure factual question (\"How far is the moon?\"): answer it directly, THEN pivot to a Socratic follow-up (\"Now that you know it's 384,000 km, how long would it take to drive there?\").\n- Be warm. Be patient. Never condescend. Treat every question as worthy.\n- You are NOT trying to keep Tester engaged. If they want to stop, let them stop. Say \"That's enough for today\" without guilt.\n- You do not use emojis or exclamation marks excessively.\n\nLanguage for a 8-year-old — read carefully:\n- Use everyday words a young child uses at home or in primary school.\n- Short, clear sentences — usually 8 to 15 words. Break longer thoughts into separate sentences.\n- Common everyday words only. Treat words like \"molecule\", \"plasma\", \"conductor\", \"insulator\", \"vibration\", \"shockwave\", \"eardrum\", \"pressure wave\", \"electron\" as TECHNICAL — they require the plain-language introduction described in the Vocabulary discipline section below.\n- Anchor abstract ideas to something the child can see, touch, or do — kitchen, playground, bath, bed, pets, family — before introducing the abstract version.\n- It is better to say something twice in plain words than once correctly with a hard word.\n\nVocabulary discipline (applies at every age):\n- Before using any technical or unusual word (examples at this age: \"plasma\", \"molecule\", \"conductor\", \"insulator\", \"shockwave\", \"vibration\", \"frequency\", \"voltage\", \"current\", \"atom\", \"particle\"), first explain the idea in plain everyday words using a concrete analogy Tester already knows (food, toys, animals, weather, family, body). Only use the technical word once the plain-language idea is clear — and even then, the technical word is optional, never required.\n- If Tester asks \"what does X mean?\" (like asking what \"repel\" means), that is a signal that X was introduced too soon. First, explain X in plain everyday words. For the next sentence or two, use the plain-language version on its own. Then start weaving X back in alongside the plain meaning (\"the air pushes the charges away — it repels them\"), so Tester ends the conversation having *gained* the new word, not had it taken away. Re-use newly-introduced words a few more times before the session ends — short, casual repetition is how vocabulary actually sticks.\n- One new idea per sentence. If a sentence introduces two unfamiliar things, split it.\n- After two or three sentences of explanation, stop and ask a question. Do not lecture.\n\nYour next response should be a guiding question that leads toward understanding.";
        if prompt.system != want {
            // Print a diagnostic: first divergence + lengths.
            let got = prompt.system.as_str();
            let mut idx = 0;
            for (g, w) in got.bytes().zip(want.bytes()) {
                if g != w {
                    break;
                }
                idx += 1;
            }
            panic!(
                "canonical snapshot diverged at byte {idx}; got len={}, want len={}\n--- want[..idx+40] ---\n{:?}\n--- got[..idx+40]  ---\n{:?}",
                got.len(),
                want.len(),
                &want
                    .get(..idx.min(want.len()).saturating_add(40).min(want.len()))
                    .unwrap_or(""),
                &got.get(..idx.min(got.len()).saturating_add(40).min(got.len()))
                    .unwrap_or(""),
            );
        }
    }

    // ─── German locale: prompt rendering smoke + locale-dispatch ──────

    #[test]
    fn snapshot_german_pack_renders_native_german_prompt() {
        // Build a system prompt under Locale::German and assert the
        // markers that pin native-German rendering. Not a byte-exact
        // snapshot — translators may iterate on the wording — but it
        // catches: (a) accidental English fragments from a pack-dispatch
        // bug, (b) placeholder substitution failures, (c) section-intro
        // dispatch errors.
        use crate::prompt_pack;
        let pack = prompt_pack::load(primer_core::i18n::Locale::German).expect("german pack loads");
        let learner = snapshot_learner(8, EngagementState::FrustratedStuck);
        let prompt = super::build_system_prompt_with_pack(
            &*pack,
            &learner,
            PedagogicalIntent::Scaffolding,
            &[snapshot_passage()],
            "Wir haben über die Schwerkraft gesprochen.",
            &[child_turn("über Blitze geredet", vec![])],
        );
        // Base block markers.
        assert!(prompt.starts_with("Du bist der Primer"));
        assert!(prompt.contains("namens Tester"));
        assert!(prompt.contains("8 Jahre alt"));
        // Age-7-9 band marker.
        assert!(prompt.contains("Grundschule"));
        // Intent: Scaffolding instruction in German.
        assert!(prompt.contains("Schwierigkeiten"));
        assert!(prompt.contains("Verringere die Abstraktion"));
        // Engagement note (FrustratedStuck → frustrated note).
        assert!(prompt.contains("WICHTIG"));
        assert!(prompt.contains("frustriert"));
        // Knowledge intro (with {age} substituted).
        assert!(prompt.contains("8-jähriges Kind umformulieren"));
        // Summary intro.
        assert!(prompt.contains("Früher in diesem Gespräch"));
        // Retrieved-moments intro.
        assert!(prompt.contains("Relevante frühere Momente"));
        assert!(prompt.contains("[Kind]"));
        // No accidental English fragments.
        assert!(!prompt.contains("You are the Primer"));
        assert!(!prompt.contains("Your next response"));
        assert!(!prompt.contains("Earlier in this conversation"));
        assert!(!prompt.contains("Relevant prior moments"));
    }

    #[test]
    fn build_prompt_includes_engagement_note_for_frustrated_trying() {
        let learner = learner_with(EngagementState::FrustratedTrying, vec![]);
        let session = empty_session();
        let prompt = build_prompt(
            &learner,
            &session,
            PedagogicalIntent::Encouragement,
            &[],
            "",
            &[],
            20,
        );
        assert!(
            prompt.system.contains("appears frustrated"),
            "engagement note should appear for FrustratedTrying"
        );
    }

    // ─── Vocab review section (build_system_prompt_with_pack_and_vocab) ──

    fn vocab_concept(id: &str, depth: UnderstandingDepth, days_ago: i64) -> ConceptState {
        ConceptState {
            concept_id: id.to_string(),
            depth,
            confidence: 0.8,
            encounter_count: 2,
            last_encountered: Some(Utc::now() - chrono::Duration::days(days_ago)),
            notes: vec![],
            box_level: 1,
        }
    }

    #[test]
    fn build_system_prompt_includes_vocab_section_when_due_vocab_non_empty() {
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let due_vocab = [
            vocab_concept("physics:gravity", UnderstandingDepth::Recall, 5),
            vocab_concept(
                "biology:photosynthesis",
                UnderstandingDepth::Comprehension,
                12,
            ),
        ];
        let due_refs: Vec<&ConceptState> = due_vocab.iter().collect();
        let prompt = build_system_prompt_with_pack_and_vocab(
            english_pack(),
            &learner,
            PedagogicalIntent::SocraticQuestion,
            &[],
            "",
            &[],
            &due_refs,
        );
        assert!(
            prompt.contains("topically relevant"),
            "expected vocab intro in prompt, got: {prompt}"
        );
        assert!(
            prompt.contains("physics:gravity"),
            "expected first concept in prompt, got: {prompt}"
        );
        assert!(
            prompt.contains("biology:photosynthesis"),
            "expected second concept in prompt, got: {prompt}"
        );
    }

    #[test]
    fn build_system_prompt_omits_vocab_section_when_due_vocab_empty() {
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let prompt = build_system_prompt_with_pack_and_vocab(
            english_pack(),
            &learner,
            PedagogicalIntent::SocraticQuestion,
            &[],
            "",
            &[],
            &[],
        );
        assert!(
            !prompt.contains("topically relevant"),
            "vocab intro should not appear when due_vocab is empty: {prompt}"
        );
    }

    #[test]
    fn build_system_prompt_places_vocab_after_retrieved_before_knowledge() {
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let retrieved = vec![Turn {
            speaker: Speaker::Child,
            text: "remember when we talked about clouds".into(),
            timestamp: Utc::now(),
            intent: None,
            concepts: vec![],
        }];
        let knowledge = vec![Passage {
            id: "test:cloud".into(),
            text: "Clouds are condensed water vapor".into(),
            source: "test".into(),
            score: 1.0,
        }];
        let due_vocab = [vocab_concept(
            "weather:cloud",
            UnderstandingDepth::Recall,
            5,
        )];
        let due_refs: Vec<&ConceptState> = due_vocab.iter().collect();
        let prompt = build_system_prompt_with_pack_and_vocab(
            english_pack(),
            &learner,
            PedagogicalIntent::SocraticQuestion,
            &knowledge,
            "Earlier we talked about water cycles.",
            &retrieved,
            &due_refs,
        );
        let retrieved_idx = prompt
            .find("clouds")
            .expect("retrieved snippet must appear");
        let vocab_idx = prompt
            .find("weather:cloud")
            .expect("vocab concept must appear");
        let knowledge_idx = prompt
            .find("condensed water vapor")
            .expect("knowledge snippet must appear");
        assert!(
            retrieved_idx < vocab_idx,
            "retrieved must precede vocab: {prompt}"
        );
        assert!(
            vocab_idx < knowledge_idx,
            "vocab must precede knowledge: {prompt}"
        );
    }
}
