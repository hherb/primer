//! Phase 1.3 inference-router policy: pure, I/O-free decision logic shared by
//! the wiring layer (which constructs the router) and the router decorator in
//! `primer-inference`. Kept in `primer-core` so it carries no inference
//! dependency and is unit-testable on the default `cargo test`.
//!
//! See docs/superpowers/specs/2026-06-07-inference-router-design.md.

use std::str::FromStr;

use crate::consts::router::{
    MSG_LONG_WORDS, MSG_QUESTION_CAP, ROUTE_PASSAGE_CAP, ROUTE_SECONDARY_THRESHOLD, W_MSG_LONG,
    W_MSG_QUESTION, W_PASSAGE,
};
use crate::conversation::PedagogicalIntent;
use crate::inference::Prompt;

/// How the router chooses between the primary (typically local/small) and
/// secondary (typically cloud/strong) legs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum RouterMode {
    /// Never use the secondary leg. The runtime works with zero network.
    /// This is the privacy default.
    #[default]
    LocalOnly,
    /// Always try the secondary first; fall back to the primary on a
    /// pre-stream failure.
    CloudPreferred,
    /// Score each turn; route high-complexity turns to the secondary, routine
    /// turns to the primary. Either leg covers the other on pre-stream failure.
    Hybrid,
}

impl RouterMode {
    /// Every variant, in declaration order (for CLI help / GUI pickers).
    pub const ALL: &'static [Self] = &[Self::LocalOnly, Self::CloudPreferred, Self::Hybrid];

    /// Canonical kebab-case name. Stable identifier used by CLI flags, the
    /// GUI picker values, and config serialization — do not rename.
    pub fn name(self) -> &'static str {
        match self {
            Self::LocalOnly => "local-only",
            Self::CloudPreferred => "cloud-preferred",
            Self::Hybrid => "hybrid",
        }
    }

    /// True when this mode may route to the secondary leg (i.e. NOT local-only).
    pub fn uses_secondary(self) -> bool {
        !matches!(self, Self::LocalOnly)
    }
}

impl std::fmt::Display for RouterMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for RouterMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "local-only" => Ok(Self::LocalOnly),
            "cloud-preferred" => Ok(Self::CloudPreferred),
            "hybrid" => Ok(Self::Hybrid),
            other => Err(format!(
                "unknown router mode '{other}' (expected local-only, cloud-preferred, or hybrid)"
            )),
        }
    }
}

/// Structured per-turn signals the dialogue manager knows but the bare
/// `Prompt` does not carry as data. Threaded through
/// `GenerationParams.routing`; every non-router backend ignores it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RoutingSignals {
    /// The pedagogical intent decided for this turn.
    pub intent: PedagogicalIntent,
    /// How many knowledge passages RAG retrieved for this turn.
    pub retrieved_passages: usize,
    // Reserved extension point (latency-aware switching, deferred):
    // pub recent_primary_ttft_ms: Option<u64>,
}

/// Per-intent routing weight. Higher = more likely to route to the strong
/// secondary leg. Starting values; tunable via the (documented) intent table.
/// An exhaustive `match` so a future `PedagogicalIntent` variant forces a
/// compile error here rather than silently scoring zero.
pub fn intent_weight(intent: PedagogicalIntent) -> f32 {
    match intent {
        PedagogicalIntent::Scaffolding => 0.45,
        PedagogicalIntent::DirectAnswer => 0.40,
        PedagogicalIntent::AnswerThenPivot => 0.40,
        PedagogicalIntent::Extension => 0.30,
        PedagogicalIntent::ComprehensionCheck => 0.25,
        PedagogicalIntent::SocraticQuestion => 0.15,
        PedagogicalIntent::Encouragement => 0.0,
        PedagogicalIntent::SessionClose => 0.0,
        PedagogicalIntent::SuggestBreak => 0.0,
    }
}

/// Knowledge-intensity term: `min(passages, CAP) * W_PASSAGE`.
pub fn passage_term(retrieved_passages: usize) -> f32 {
    retrieved_passages.min(ROUTE_PASSAGE_CAP) as f32 * W_PASSAGE
}

/// Message-complexity term derived from the last child (`Role::User`) message
/// in the prompt: a length component plus a (capped) question-depth component.
/// Pure string analysis — no NLP dependency.
pub fn message_term(prompt: &Prompt) -> f32 {
    let Some(last_user) = prompt
        .messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, crate::inference::Role::User))
    else {
        return 0.0;
    };
    let text = &last_user.content;

    let long = if text.split_whitespace().count() > MSG_LONG_WORDS {
        W_MSG_LONG
    } else {
        0.0
    };

    let extra_questions = text
        .matches('?')
        .count()
        .saturating_sub(1)
        .min(MSG_QUESTION_CAP);
    let question = extra_questions as f32 * W_MSG_QUESTION;

    long + question
}

/// Composite turn-complexity score. Higher ⇒ more likely to route to the
/// secondary (strong) leg in `hybrid` mode.
pub fn complexity_score(signals: &RoutingSignals, prompt: &Prompt) -> f32 {
    intent_weight(signals.intent) + passage_term(signals.retrieved_passages) + message_term(prompt)
}

/// Which physical leg the router should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leg {
    /// The `--backend` leg (typically local/small).
    Primary,
    /// The `--fallback-backend` leg (typically cloud/strong).
    Secondary,
}

/// The ordered legs the router will try: `first`, then `second` on a
/// pre-stream failure. `second` is `None` only for `LocalOnly`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegOrder {
    pub first: Leg,
    pub second: Option<Leg>,
}

/// Map a mode + complexity score to the ordered leg pair. Pure.
pub fn order_legs(mode: RouterMode, score: f32) -> LegOrder {
    match mode {
        RouterMode::LocalOnly => LegOrder {
            first: Leg::Primary,
            second: None,
        },
        RouterMode::CloudPreferred => LegOrder {
            first: Leg::Secondary,
            second: Some(Leg::Primary),
        },
        RouterMode::Hybrid if score >= ROUTE_SECONDARY_THRESHOLD => LegOrder {
            first: Leg::Secondary,
            second: Some(Leg::Primary),
        },
        RouterMode::Hybrid => LegOrder {
            first: Leg::Primary,
            second: Some(Leg::Secondary),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::{Message, Role};

    #[test]
    fn default_is_local_only() {
        assert_eq!(RouterMode::default(), RouterMode::LocalOnly);
    }

    #[test]
    fn name_and_from_str_round_trip() {
        for &m in RouterMode::ALL {
            assert_eq!(RouterMode::from_str(m.name()).unwrap(), m);
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!(RouterMode::from_str("nonsense").is_err());
    }

    #[test]
    fn only_local_only_skips_secondary() {
        assert!(!RouterMode::LocalOnly.uses_secondary());
        assert!(RouterMode::CloudPreferred.uses_secondary());
        assert!(RouterMode::Hybrid.uses_secondary());
    }

    fn prompt_with_last_user(msg: &str) -> Prompt {
        Prompt {
            system: String::new(),
            messages: vec![Message {
                role: Role::User,
                content: msg.to_string(),
            }],
        }
    }

    #[test]
    fn intent_weight_covers_all_variants_monotonically() {
        assert!(
            intent_weight(PedagogicalIntent::Scaffolding)
                >= intent_weight(PedagogicalIntent::DirectAnswer)
        );
        assert!(
            intent_weight(PedagogicalIntent::DirectAnswer)
                > intent_weight(PedagogicalIntent::SocraticQuestion)
        );
        assert_eq!(intent_weight(PedagogicalIntent::Encouragement), 0.0);
        assert_eq!(intent_weight(PedagogicalIntent::SessionClose), 0.0);
        assert_eq!(intent_weight(PedagogicalIntent::SuggestBreak), 0.0);
    }

    #[test]
    fn passage_term_is_capped() {
        use crate::consts::router::{ROUTE_PASSAGE_CAP, W_PASSAGE};
        assert_eq!(passage_term(0), 0.0);
        assert_eq!(
            passage_term(ROUTE_PASSAGE_CAP),
            ROUTE_PASSAGE_CAP as f32 * W_PASSAGE
        );
        assert_eq!(
            passage_term(ROUTE_PASSAGE_CAP + 5),
            ROUTE_PASSAGE_CAP as f32 * W_PASSAGE
        );
    }

    #[test]
    fn message_term_rewards_length_and_questions() {
        let short = prompt_with_last_user("why?");
        let long = prompt_with_last_user(&"word ".repeat(40));
        let many_q = prompt_with_last_user("what? why? how? when?");
        assert_eq!(message_term(&short), 0.0);
        assert!(message_term(&long) > 0.0);
        assert!(message_term(&many_q) > 0.0);
    }

    #[test]
    fn message_term_zero_when_no_user_message() {
        let empty = Prompt {
            system: "x".into(),
            messages: vec![],
        };
        assert_eq!(message_term(&empty), 0.0);
    }

    #[test]
    fn complexity_score_routes_hard_turn_above_threshold() {
        use crate::consts::router::ROUTE_SECONDARY_THRESHOLD;
        let s = RoutingSignals {
            intent: PedagogicalIntent::Scaffolding,
            retrieved_passages: 2,
        };
        let hard = prompt_with_last_user(&"explain ".repeat(40));
        assert!(complexity_score(&s, &hard) >= ROUTE_SECONDARY_THRESHOLD);
    }

    #[test]
    fn complexity_score_keeps_routine_turn_below_threshold() {
        use crate::consts::router::ROUTE_SECONDARY_THRESHOLD;
        let s = RoutingSignals {
            intent: PedagogicalIntent::Encouragement,
            retrieved_passages: 0,
        };
        let easy = prompt_with_last_user("ok");
        assert!(complexity_score(&s, &easy) < ROUTE_SECONDARY_THRESHOLD);
    }

    #[test]
    fn local_only_never_has_a_second_leg() {
        let o = order_legs(RouterMode::LocalOnly, 1.0);
        assert_eq!(o.first, Leg::Primary);
        assert_eq!(o.second, None);
    }

    #[test]
    fn cloud_preferred_is_secondary_first_regardless_of_score() {
        let lo = order_legs(RouterMode::CloudPreferred, 0.0);
        let hi = order_legs(RouterMode::CloudPreferred, 1.0);
        assert_eq!(lo.first, Leg::Secondary);
        assert_eq!(lo.second, Some(Leg::Primary));
        assert_eq!(hi.first, Leg::Secondary);
    }

    #[test]
    fn hybrid_routes_by_threshold() {
        use crate::consts::router::ROUTE_SECONDARY_THRESHOLD;
        let below = order_legs(RouterMode::Hybrid, ROUTE_SECONDARY_THRESHOLD - 0.01);
        let at = order_legs(RouterMode::Hybrid, ROUTE_SECONDARY_THRESHOLD);
        assert_eq!(below.first, Leg::Primary);
        assert_eq!(below.second, Some(Leg::Secondary));
        assert_eq!(at.first, Leg::Secondary);
        assert_eq!(at.second, Some(Leg::Primary));
    }
}
