//! System prompt construction.
//!
//! The prompt builder takes the current conversation state, the learner model,
//! and any retrieved knowledge passages, and constructs the system prompt
//! that instructs the LLM how to behave.
//!
//! This is where the Socratic method is encoded — not in the model's weights,
//! but in the instructions we give it.

use primer_core::conversation::{PedagogicalIntent, Session, Speaker, Turn};
use primer_core::inference::{Message, Prompt, Role};
use primer_core::knowledge::Passage;
use primer_core::learner::{EngagementState, LearnerModel, UnderstandingDepth};

/// Build the system prompt for the next LLM call.
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
pub fn build_system_prompt(
    learner: &LearnerModel,
    intent: PedagogicalIntent,
    knowledge_context: &[Passage],
    summary: &str,
    retrieved_older: &[Turn],
) -> String {
    let age = learner.profile.age;
    let name = &learner.profile.name;
    let language_guidance = language_guidance_for_age(age);

    let base = format!(
        r#"You are the Primer — a patient, curious, Socratic learning companion for a child named {name}, age {age}.

Your core principles:
- NEVER give a direct answer when you can ask a guiding question instead.
- Ask questions that lead {name} toward discovering the answer themselves.
- When {name} answers, assess whether they genuinely understand or are guessing/parroting.
- If they understand: acknowledge it, then extend — "Good. Now what if...?"
- If they're struggling: offer a concrete example, analogy, or story. Reduce abstraction.
- If they ask a pure factual question ("How far is the moon?"): answer it directly, THEN pivot to a Socratic follow-up ("Now that you know it's 384,000 km, how long would it take to drive there?").
- Be warm. Be patient. Never condescend. Treat every question as worthy.
- You are NOT trying to keep {name} engaged. If they want to stop, let them stop. Say "That's enough for today" without guilt.
- You do not use emojis or exclamation marks excessively.

Language for a {age}-year-old — read carefully:
{language_guidance}

Vocabulary discipline (applies at every age):
- Before using any technical or unusual word (examples at this age: "plasma", "molecule", "conductor", "insulator", "shockwave", "vibration", "frequency", "voltage", "current", "atom", "particle"), first explain the idea in plain everyday words using a concrete analogy {name} already knows (food, toys, animals, weather, family, body). Only use the technical word once the plain-language idea is clear — and even then, the technical word is optional, never required.
- If {name} asks "what does X mean?" (like asking what "repel" means), that is a signal that X was introduced too soon. First, explain X in plain everyday words. For the next sentence or two, use the plain-language version on its own. Then start weaving X back in alongside the plain meaning ("the air pushes the charges away — it repels them"), so {name} ends the conversation having *gained* the new word, not had it taken away. Re-use newly-introduced words a few more times before the session ends — short, casual repetition is how vocabulary actually sticks.
- One new idea per sentence. If a sentence introduces two unfamiliar things, split it.
- After two or three sentences of explanation, stop and ask a question. Do not lecture."#
    );

    let intent_instruction = match intent {
        PedagogicalIntent::SocraticQuestion => {
            "Your next response should be a guiding question that leads toward understanding."
        }
        PedagogicalIntent::ComprehensionCheck => {
            "Your next response should probe whether the child truly understands \
             or is repeating what they've heard. Ask them to explain it differently, \
             apply it to a new situation, or find a flaw in a deliberately wrong statement."
        }
        PedagogicalIntent::Scaffolding => {
            "The child is struggling. Your next response should offer a concrete \
             example, a story, or an analogy that makes the concept tangible. \
             Reduce the abstraction level."
        }
        PedagogicalIntent::Encouragement => {
            "The child is frustrated. Your next response should be encouraging \
             without being patronising. Acknowledge the difficulty. Normalise confusion. \
             Suggest a different angle of approach."
        }
        PedagogicalIntent::Extension => {
            "The child has demonstrated understanding. Your next response should \
             extend the concept — introduce a complication, a counterexample, \
             or a connection to a different domain."
        }
        PedagogicalIntent::DirectAnswer => {
            "This is a factual question. Answer it directly and clearly, \
             then follow with a Socratic question that builds on the answer."
        }
        PedagogicalIntent::AnswerThenPivot => {
            "Provide the factual answer briefly, then pivot to a question \
             that makes the child think about *why* the fact matters or \
             what would change if it were different."
        }
        PedagogicalIntent::SessionClose => {
            "Suggest that this is a good stopping point. Summarise what was \
             explored today (not what was 'learned' — what was *explored*). \
             Leave the child with one question to think about until next time."
        }
    };

    let engagement_note = match learner.current_engagement {
        EngagementState::FrustratedStuck | EngagementState::FrustratedTrying => {
            "\n\nIMPORTANT: The child appears frustrated. Be especially gentle. \
             Offer to approach the topic differently or switch topics entirely."
        }
        EngagementState::Disengaging => {
            "\n\nNOTE: The child may be losing interest. Consider suggesting a \
             break or pivoting to a topic they find more engaging."
        }
        _ => "",
    };

    let knowledge_section = if knowledge_context.is_empty() {
        String::new()
    } else {
        let passages: String = knowledge_context
            .iter()
            .map(|p| format!("[Source: {}]\n{}", p.source, p.text))
            .collect::<Vec<_>>()
            .join("\n\n");
        format!(
            "\n\nRelevant factual context (use to ground your responses, \
             but do not quote directly — rephrase for a {age}-year-old):\n\n{passages}"
        )
    };

    let summary_section = if summary.trim().is_empty() {
        String::new()
    } else {
        format!(
            "\n\nEarlier in this conversation (long-term memory across many turns):\n\n{summary}"
        )
    };

    let retrieved_section = if retrieved_older.is_empty() {
        String::new()
    } else {
        let lines: String = retrieved_older
            .iter()
            .map(|t| {
                let who = match t.speaker {
                    Speaker::Child => "Child",
                    Speaker::Primer => "Primer",
                };
                format!("- [{who}] {}", t.text)
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "\n\nRelevant prior moments from this same session (retrieved by topic, \
             not in time order; use as background, not as the active conversation):\n\n{lines}"
        )
    };

    format!(
        "{base}\n\n{intent_instruction}{engagement_note}{summary_section}{retrieved_section}{knowledge_section}"
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

/// Assemble the complete prompt from components.
///
/// `summary` and `retrieved_older` carry long-term memory: the rolling
/// LLM-generated condensation of pre-window turns and the FTS5-retrieved
/// older turns relevant to the latest child input. Both are injected
/// into the system prompt; the chat `messages` list stays exactly equal
/// to `session.recent_turns(context_turns)` so the timeline the model
/// sees as "the conversation" is linear.
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
    Prompt {
        system: build_system_prompt(learner, intent, knowledge_context, summary, retrieved_older),
        messages: build_messages(session, context_turns),
    }
}

/// Concrete, age-banded language guidance for the system prompt.
///
/// Generic instructions like "match vocabulary to age" do not constrain a
/// modern LLM enough — it will happily use words like "plasma" or
/// "insulator" with a 7-year-old. These bands give explicit ceilings on
/// sentence length and vocabulary register, plus rules about how new
/// technical terms must be introduced.
fn language_guidance_for_age(age: u8) -> &'static str {
    match age {
        0..=6 => "\
- Use only words a young child uses at home or kindergarten.
- Sentences are short — aim for 6 to 10 words.
- Never use a word with more than three syllables unless you have just defined it through a concrete everyday example, and the child has shown they grasped the example.
- Anchor every idea to something the child can see, touch, hear, or do: food, toys, pets, body, weather, family.
- Avoid abstract nouns (\"energy\", \"matter\", \"force\") unless you have grounded them in a physical thing first.",
        7..=9 => "\
- Use everyday words a young child uses at home or in primary school.
- Short, clear sentences — usually 8 to 15 words. Break longer thoughts into separate sentences.
- Common everyday words only. Treat words like \"molecule\", \"plasma\", \"conductor\", \"insulator\", \"vibration\", \"shockwave\", \"eardrum\", \"pressure wave\", \"electron\" as TECHNICAL — they require the plain-language introduction described in the Vocabulary discipline section below.
- Anchor abstract ideas to something the child can see, touch, or do — kitchen, playground, bath, bed, pets, family — before introducing the abstract version.
- It is better to say something twice in plain words than once correctly with a hard word.",
        10..=12 => "\
- Use clear everyday vocabulary; moderate sentence length is fine.
- New technical terms are acceptable, but always define them briefly with a concrete example the first time they appear.
- You can introduce one moderately abstract idea per response, but always tie it back to something concrete.",
        _ => "\
- Adult-level vocabulary is acceptable, but still introduce specialised jargon with a brief plain-language gloss the first time it appears.
- Sentence length and complexity may match an articulate teenager.",
    }
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

/// Return `true` if `text` looks like a direct factual lookup.
///
/// Only a small set of opening phrases qualify: "what is/are/does", "what's",
/// and "how does/do/is/are". The trailing space in each prefix prevents
/// partial-word matches ("whatever", "howdy"). Exploratory forms ("what if",
/// "what about") and "why" questions are intentionally excluded — those
/// are Socratic-richer and should not be short-circuited with a direct answer.
///
/// This is a private helper for `decide_intent`; Task 19 wires it into
/// the intent-routing logic.
#[allow(dead_code)]
fn is_factual_question(text: &str) -> bool {
    const FACTUAL_PREFIXES: &[&str] = &[
        "what is ", "what are ", "what's ", "what does ",
        "how does ", "how do ",  "how is ",  "how are ",
    ];
    let lowered = text.trim().to_lowercase();
    FACTUAL_PREFIXES.iter().any(|p| lowered.starts_with(p))
}

/// Decide the next pedagogical intent based on the learner model
/// and conversation history.
///
/// This is a thin wrapper around [`decide_intent_at`] that injects
/// `chrono::Utc::now()` as the reference time. Production code calls this;
/// tests call `decide_intent_at` directly with a controlled timestamp.
pub fn decide_intent(learner: &LearnerModel, session: &Session) -> PedagogicalIntent {
    decide_intent_at(learner, session, chrono::Utc::now())
}

/// Time-aware core of [`decide_intent`].
///
/// Accepts an explicit `now` so tests can backdate sessions deterministically
/// without real-clock races. The `Disengaging` branch uses `now` together with
/// `session.started_at` to distinguish an early disengagement (encourage rather
/// than close) from a sustained one (suggest session close).
pub fn decide_intent_at(
    learner: &LearnerModel,
    session: &Session,
    now: chrono::DateTime<chrono::Utc>,
) -> PedagogicalIntent {
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
        EngagementState::Engaged
        | EngagementState::Reflecting
        | EngagementState::Unknown => { /* fall through to turn analysis */ }
    }

    // Look at the last turn — if it was a child's response, decide
    // whether to probe comprehension or extend.
    if let Some(last) = session.turns.last() {
        if last.speaker == primer_core::conversation::Speaker::Child {
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
        }
    }

    // ─── Engagement state takes precedence over turn analysis ─────────

    #[test]
    fn frustrated_stuck_returns_scaffolding() {
        let learner = learner_with(EngagementState::FrustratedStuck, vec![]);
        let session = empty_session();
        assert_eq!(decide_intent(&learner, &session), PedagogicalIntent::Scaffolding);
    }

    #[test]
    fn frustrated_stuck_overrides_short_child_turn() {
        // Without frustration, a 1-word child turn would yield ComprehensionCheck.
        // The engagement check fires first.
        let learner = learner_with(EngagementState::FrustratedStuck, vec![]);
        let mut session = empty_session();
        session.add_turn(child_turn("yes", vec![]));
        assert_eq!(decide_intent(&learner, &session), PedagogicalIntent::Scaffolding);
    }

    #[test]
    fn frustrated_trying_returns_encouragement() {
        let learner = learner_with(EngagementState::FrustratedTrying, vec![]);
        let session = empty_session();
        assert_eq!(decide_intent(&learner, &session), PedagogicalIntent::Encouragement);
    }

    #[test]
    fn frustrated_trying_overrides_short_child_turn() {
        let learner = learner_with(EngagementState::FrustratedTrying, vec![]);
        let mut session = empty_session();
        session.add_turn(child_turn("yes", vec![]));
        assert_eq!(decide_intent(&learner, &session), PedagogicalIntent::Encouragement);
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

    // ─── Currently-unreachable intents (regression guards) ───────────
    //
    // The current heuristic never returns these intents. If a future
    // change starts emitting them these guards will fail and prompt a
    // deliberate update — they are NOT a claim that the intents
    // shouldn't ever be returned.

    #[test]
    fn factual_question_pattern_does_not_currently_return_direct_answer() {
        // "what is X?" is not detected as a factual query; the heuristic
        // routes purely on engagement state and turn length.
        let learner = learner_with(EngagementState::Engaged, vec![]);
        let mut session = empty_session();
        session.add_turn(child_turn("what is gravity?", vec![]));
        let intent = decide_intent(&learner, &session);
        assert_ne!(intent, PedagogicalIntent::DirectAnswer);
        assert_ne!(intent, PedagogicalIntent::AnswerThenPivot);
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
}
