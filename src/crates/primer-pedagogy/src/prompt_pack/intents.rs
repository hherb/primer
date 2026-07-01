//! Intent-key mapping between `PedagogicalIntent` variants and the TOML
//! `[intent]` table keys.
//!
//! `ALL_INTENTS` is the canonical ordering; its position doubles as the
//! index into `TomlPromptPack`'s fixed-size per-intent instruction array,
//! giving O(1) lookup without requiring `Hash` on `PedagogicalIntent`. A
//! test guards that `ALL_INTENTS` stays in sync with the enum.

use primer_core::conversation::PedagogicalIntent;

pub(crate) const ALL_INTENTS: &[PedagogicalIntent] = &[
    PedagogicalIntent::SocraticQuestion,
    PedagogicalIntent::ComprehensionCheck,
    PedagogicalIntent::Scaffolding,
    PedagogicalIntent::Encouragement,
    PedagogicalIntent::Extension,
    PedagogicalIntent::DirectAnswer,
    PedagogicalIntent::AnswerThenPivot,
    PedagogicalIntent::SessionClose,
    PedagogicalIntent::SuggestBreak,
    PedagogicalIntent::ProbeReasoning,
];

pub(crate) fn intent_key(intent: PedagogicalIntent) -> &'static str {
    match intent {
        PedagogicalIntent::SocraticQuestion => "socratic_question",
        PedagogicalIntent::ComprehensionCheck => "comprehension_check",
        PedagogicalIntent::Scaffolding => "scaffolding",
        PedagogicalIntent::Encouragement => "encouragement",
        PedagogicalIntent::Extension => "extension",
        PedagogicalIntent::DirectAnswer => "direct_answer",
        PedagogicalIntent::AnswerThenPivot => "answer_then_pivot",
        PedagogicalIntent::SessionClose => "session_close",
        PedagogicalIntent::SuggestBreak => "suggest_break",
        PedagogicalIntent::ProbeReasoning => "probe_reasoning",
    }
}

/// Position of `intent` in `ALL_INTENTS`. Used as the array index for
/// the typed-but-non-Hash lookup table inside `TomlPromptPack`.
pub(crate) fn intent_index(intent: PedagogicalIntent) -> usize {
    ALL_INTENTS
        .iter()
        .position(|v| *v == intent)
        .expect("ALL_INTENTS covers every PedagogicalIntent variant")
}

pub(crate) fn parse_intent_key(s: &str) -> Option<PedagogicalIntent> {
    ALL_INTENTS.iter().find(|v| intent_key(**v) == s).copied()
}
