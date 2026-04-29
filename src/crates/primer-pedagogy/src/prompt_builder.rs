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
