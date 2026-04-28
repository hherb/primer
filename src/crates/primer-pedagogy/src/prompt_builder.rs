//! System prompt construction.
//!
//! The prompt builder takes the current conversation state, the learner model,
//! and any retrieved knowledge passages, and constructs the system prompt
//! that instructs the LLM how to behave.
//!
//! This is where the Socratic method is encoded — not in the model's weights,
//! but in the instructions we give it.

use primer_core::conversation::{PedagogicalIntent, Session};
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
pub fn build_system_prompt(
    learner: &LearnerModel,
    intent: PedagogicalIntent,
    knowledge_context: &[Passage],
) -> String {
    let age = learner.profile.age;
    let name = &learner.profile.name;

    let base = format!(
        r#"You are the Primer — a patient, curious, Socratic learning companion for a child named {name}, age {age}.

Your core principles:
- NEVER give a direct answer when you can ask a guiding question instead.
- Ask questions that lead {name} toward discovering the answer themselves.
- When {name} answers, assess whether they genuinely understand or are guessing/parroting.
- If they understand: acknowledge it, then extend — "Good. Now what if...?"
- If they're struggling: offer a concrete example, analogy, or story. Reduce abstraction.
- If they ask a pure factual question ("How far is the moon?"): answer it directly, THEN pivot to a Socratic follow-up ("Now that you know it's 384,000 km, how long would it take to drive there?").
- Match your vocabulary and sentence complexity to a {age}-year-old.
- Be warm. Be patient. Never condescend. Treat every question as worthy.
- You are NOT trying to keep {name} engaged. If they want to stop, let them stop. Say "That's enough for today" without guilt.
- You do not use emojis or exclamation marks excessively."#
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
        EngagementState::Frustrated => {
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

    format!("{base}\n\n{intent_instruction}{engagement_note}{knowledge_section}")
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
pub fn build_prompt(
    learner: &LearnerModel,
    session: &Session,
    intent: PedagogicalIntent,
    knowledge_context: &[Passage],
    context_turns: usize,
) -> Prompt {
    Prompt {
        system: build_system_prompt(learner, intent, knowledge_context),
        messages: build_messages(session, context_turns),
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

/// Decide the next pedagogical intent based on the learner model
/// and conversation history.
pub fn decide_intent(
    learner: &LearnerModel,
    session: &Session,
) -> PedagogicalIntent {
    // If the child is frustrated, scaffold.
    if learner.current_engagement == EngagementState::Frustrated {
        return PedagogicalIntent::Scaffolding;
    }

    // If the child is disengaging, consider closing.
    if learner.current_engagement == EngagementState::Disengaging {
        return PedagogicalIntent::SessionClose;
    }

    // Look at the last turn — if it was a child's response, decide
    // whether to probe comprehension or extend.
    if let Some(last) = session.turns.last() {
        if last.speaker == primer_core::conversation::Speaker::Child {
            // Simple heuristic: short responses likely need probing,
            // longer responses might demonstrate understanding.
            if last.text.split_whitespace().count() < 10 {
                return PedagogicalIntent::ComprehensionCheck;
            }

            // Check if any active concepts are at Comprehension level
            // or above — if so, extend.
            let active = extract_active_concepts(session, 4);
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
