//! Full-prompt assembly: [`build_messages`] converts a session into the chat
//! timeline, and the `build_prompt_*` family pairs that timeline with a system
//! prompt from [`super::system_prompt`].

use primer_core::conversation::{PedagogicalIntent, Session, Turn};
use primer_core::inference::{Message, Prompt, Role};
use primer_core::knowledge::Passage;
use primer_core::learner::{ConceptState, LearnerModel};

use crate::prompt_pack::PromptPack;

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
        system: super::build_system_prompt_with_pack_and_vocab(
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
        system: super::build_system_prompt_within_budget_with_pack_and_vocab(
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
        super::english_pack(),
        learner,
        session,
        intent,
        knowledge_context,
        summary,
        retrieved_older,
        context_turns,
    )
}
