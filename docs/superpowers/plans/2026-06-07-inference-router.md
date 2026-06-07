# Inference Router Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a per-turn inference router that picks between a primary (local/small) and a secondary (cloud/strong) backend based on an estimate of turn complexity, exposed as three config modes (`local-only` / `cloud-preferred` / `hybrid`), shipping in both CLI and GUI.

**Architecture:** A `RouterBackend` decorator (mirroring the existing `FallbackBackend`) implements `InferenceBackend`, so `DialogueManager` is structurally unchanged. The policy is a pure composite-score function in `primer-core`; the dialogue manager threads `RoutingSignals` (intent + retrieved-passage count) through a new optional field on `GenerationParams` that every non-router backend ignores. The router self-fails-over at the pre-stream boundary; `FallbackBackend` stays as the no-routing resilience building block. Built by the same `build_main_backend` wiring path, reusing the `--fallback-*` secondary leg.

**Tech Stack:** Rust (edition 2024, toolchain 1.88), `async-trait`, `futures`, `tokio`, `serde`, `clap` (CLI), Tauri 2.x + vanilla JS (GUI).

**Spec:** `docs/superpowers/specs/2026-06-07-inference-router-design.md`

**Conventions (read before starting):**
- Run all cargo commands from `src/` with the pinned toolchain: `~/.cargo/bin/cargo +1.88 ...`.
- No magic numbers — every tunable lives in a `consts` module.
- TDD: failing test first, minimal impl, green, commit.
- Keep files focused; this plan adds two new small modules rather than growing existing ones.

---

## File Structure

**New files:**
- `src/crates/primer-core/src/router.rs` — `RouterMode`, `RoutingSignals`, `Leg`, `LegOrder`, the pure `complexity_score` + `order_legs` functions, and their unit tests. One responsibility: the routing *policy* (pure, no I/O, no inference dep).
- `src/crates/primer-inference/src/router.rs` — `RouterBackend` decorator (impl `InferenceBackend`) + its mock-driven tests. One responsibility: the routing *mechanism* (pick a leg, self-fail-over).

**Modified files:**
- `src/crates/primer-core/src/consts.rs` — add `pub mod router` (weights, threshold, caps).
- `src/crates/primer-core/src/lib.rs` — add `pub mod router;`.
- `src/crates/primer-core/src/inference.rs` — `GenerationParams.routing: Option<RoutingSignals>` field + `Default` + the in-file `summarize` literal.
- `src/crates/primer-inference/src/lib.rs` — `pub mod router;` + `pub use router::RouterBackend;`.
- 7 other crates' `GenerationParams { ... }` literals — add `routing: None`.
- `src/crates/primer-engine/src/wiring.rs` — `BackendParams.router_mode` field; new `build_router_backend`; early branch in `build_main_backend`; update all `BackendParams` test literals.
- `src/crates/primer-cli/src/main.rs` — `--router-mode` flag, validation, `BackendParams` construction.
- `src/crates/primer-pedagogy/src/dialogue_manager/turn.rs` — thread intent + passage count into `params.routing`.
- `src/crates/primer-gui/src/config.rs` — `BackendConfig`/`BackendConfigView`/`BackendConfigUpdate` `router_mode`.
- `src/crates/primer-gui/src/wiring.rs` — pass `router_mode` into `BackendParams`.
- `src/crates/primer-gui/ui/settings.js` + the settings HTML — "Routing mode" picker.
- `CLAUDE.md`, `README.md`, `ROADMAP.md` — docs.

---

## Task 1: `consts::router` module

**Files:**
- Modify: `src/crates/primer-core/src/consts.rs`

- [ ] **Step 1: Write the failing test**

Add this test module at the end of `src/crates/primer-core/src/consts.rs` (a sibling of the existing `#[cfg(test)] mod tests`; place it inside that module or add a new `#[test]` fn there — append a new test fn to the existing `mod tests`):

```rust
    #[test]
    fn router_consts_are_sane() {
        use super::router::*;
        // Threshold sits between the cheapest and most expensive single-intent
        // weights so a high-weight intent alone can cross it but a zero-weight
        // one cannot.
        assert!(ROUTE_SECONDARY_THRESHOLD > 0.0 && ROUTE_SECONDARY_THRESHOLD < 1.0);
        assert_eq!(ROUTE_PASSAGE_CAP, 3);
        assert!(W_PASSAGE > 0.0);
        assert_eq!(MSG_LONG_WORDS, 30);
        assert!(W_MSG_LONG > 0.0);
        assert!(W_MSG_QUESTION > 0.0);
        assert_eq!(MSG_QUESTION_CAP, 2);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-core router_consts_are_sane`
Expected: FAIL — `module router is undefined` / `cannot find module`.

- [ ] **Step 3: Write minimal implementation**

Add this module to `src/crates/primer-core/src/consts.rs` (place it after the existing `pub mod inference { ... }` block, before the `#[cfg(test)]` block):

```rust
/// Tunables for the Phase 1.3 inference router (see
/// docs/superpowers/specs/2026-06-07-inference-router-design.md).
///
/// These weights and the threshold are starting values; they need
/// calibration against real usage data (like the bench numbers) and are
/// deliberately gathered here so that tuning never touches logic.
pub mod router {
    /// Route to the secondary (strong) leg when `complexity_score` reaches
    /// this value, in `hybrid` mode.
    pub const ROUTE_SECONDARY_THRESHOLD: f32 = 0.5;

    /// Retrieved-passage count is clamped to this before scoring, so a large
    /// retrieval cannot dominate the score.
    pub const ROUTE_PASSAGE_CAP: usize = 3;
    /// Per-passage score weight (after the cap).
    pub const W_PASSAGE: f32 = 0.15;

    /// A child message with more than this many words contributes the long-
    /// message weight.
    pub const MSG_LONG_WORDS: usize = 30;
    /// Weight added for a long child message.
    pub const W_MSG_LONG: f32 = 0.20;
    /// Weight added per question mark beyond the first, in the child message.
    pub const W_MSG_QUESTION: f32 = 0.10;
    /// Question marks beyond the first are counted up to this cap.
    pub const MSG_QUESTION_CAP: usize = 2;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-core router_consts_are_sane`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-core/src/consts.rs
git commit -m "feat(router): add consts::router tunables (weights, threshold, caps)"
```

---

## Task 2: `RouterMode` enum

**Files:**
- Create: `src/crates/primer-core/src/router.rs`
- Modify: `src/crates/primer-core/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `src/crates/primer-core/src/router.rs` with ONLY this content (enum + tests; the pure functions come in later tasks):

```rust
//! Phase 1.3 inference-router policy: pure, I/O-free decision logic shared by
//! the wiring layer (which constructs the router) and the router decorator in
//! `primer-inference`. Kept in `primer-core` so it carries no inference
//! dependency and is unit-testable on the default `cargo test`.
//!
//! See docs/superpowers/specs/2026-06-07-inference-router-design.md.

use std::str::FromStr;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
```

Then add the module to `src/crates/primer-core/src/lib.rs` — insert `pub mod router;` in alphabetical position (between `pub mod retry;` and `pub mod rrf;`):

```rust
pub mod retry;
pub mod router;
pub mod rrf;
```

- [ ] **Step 2: Run test to verify it fails (then passes — this task is the whole enum)**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-core router::tests`
Expected: PASS (the enum + tests are added together; if it fails, fix compile errors).

- [ ] **Step 3: (covered by Step 1 — enum is the minimal impl)**

No additional implementation; the enum in Step 1 is complete.

- [ ] **Step 4: Verify clippy is clean for the new file**

Run: `cd src && ~/.cargo/bin/cargo +1.88 clippy -p primer-core --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-core/src/router.rs src/crates/primer-core/src/lib.rs
git commit -m "feat(router): add RouterMode enum (local-only/cloud-preferred/hybrid)"
```

---

## Task 3: `RoutingSignals` + composite `complexity_score`

**Files:**
- Modify: `src/crates/primer-core/src/router.rs`

- [ ] **Step 1: Write the failing test**

Append these tests to the `#[cfg(test)] mod tests` block in `src/crates/primer-core/src/router.rs`:

```rust
    use crate::conversation::PedagogicalIntent;
    use crate::inference::{Message, Prompt, Role};

    fn prompt_with_last_user(msg: &str) -> Prompt {
        Prompt {
            system: String::new(),
            messages: vec![Message { role: Role::User, content: msg.to_string() }],
        }
    }

    #[test]
    fn intent_weight_covers_all_variants_monotonically() {
        // Scaffolding is the heaviest; the trivial intents are zero.
        assert!(intent_weight(PedagogicalIntent::Scaffolding) >= intent_weight(PedagogicalIntent::DirectAnswer));
        assert!(intent_weight(PedagogicalIntent::DirectAnswer) > intent_weight(PedagogicalIntent::SocraticQuestion));
        assert_eq!(intent_weight(PedagogicalIntent::Encouragement), 0.0);
        assert_eq!(intent_weight(PedagogicalIntent::SessionClose), 0.0);
        assert_eq!(intent_weight(PedagogicalIntent::SuggestBreak), 0.0);
    }

    #[test]
    fn passage_term_is_capped() {
        use crate::consts::router::{ROUTE_PASSAGE_CAP, W_PASSAGE};
        assert_eq!(passage_term(0), 0.0);
        assert_eq!(passage_term(ROUTE_PASSAGE_CAP), ROUTE_PASSAGE_CAP as f32 * W_PASSAGE);
        // Beyond the cap is flat.
        assert_eq!(passage_term(ROUTE_PASSAGE_CAP + 5), ROUTE_PASSAGE_CAP as f32 * W_PASSAGE);
    }

    #[test]
    fn message_term_rewards_length_and_questions() {
        let short = prompt_with_last_user("why?");
        let long = prompt_with_last_user(&"word ".repeat(40));
        let many_q = prompt_with_last_user("what? why? how? when?");
        assert_eq!(message_term(&short), 0.0); // short, single question
        assert!(message_term(&long) > 0.0);
        assert!(message_term(&many_q) > 0.0);
    }

    #[test]
    fn message_term_zero_when_no_user_message() {
        let empty = Prompt { system: "x".into(), messages: vec![] };
        assert_eq!(message_term(&empty), 0.0);
    }

    #[test]
    fn complexity_score_routes_hard_turn_above_threshold() {
        use crate::consts::router::ROUTE_SECONDARY_THRESHOLD;
        let s = RoutingSignals { intent: PedagogicalIntent::Scaffolding, retrieved_passages: 2 };
        let hard = prompt_with_last_user(&"explain ".repeat(40));
        assert!(complexity_score(&s, &hard) >= ROUTE_SECONDARY_THRESHOLD);
    }

    #[test]
    fn complexity_score_keeps_routine_turn_below_threshold() {
        use crate::consts::router::ROUTE_SECONDARY_THRESHOLD;
        let s = RoutingSignals { intent: PedagogicalIntent::Encouragement, retrieved_passages: 0 };
        let easy = prompt_with_last_user("ok");
        assert!(complexity_score(&s, &easy) < ROUTE_SECONDARY_THRESHOLD);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-core router::tests`
Expected: FAIL — `RoutingSignals` / `intent_weight` / `passage_term` / `message_term` / `complexity_score` not found.

- [ ] **Step 3: Write minimal implementation**

Add to `src/crates/primer-core/src/router.rs`, after the `RouterMode` impl block and before the `#[cfg(test)]` module. Also add the imports `use crate::conversation::PedagogicalIntent;` and `use crate::inference::Prompt;` near the top (after the existing `use std::str::FromStr;`):

```rust
use crate::consts::router::{
    MSG_LONG_WORDS, MSG_QUESTION_CAP, ROUTE_PASSAGE_CAP, W_MSG_LONG, W_MSG_QUESTION, W_PASSAGE,
};
use crate::conversation::PedagogicalIntent;
use crate::inference::Prompt;

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

    let extra_questions = text.matches('?').count().saturating_sub(1).min(MSG_QUESTION_CAP);
    let question = extra_questions as f32 * W_MSG_QUESTION;

    long + question
}

/// Composite turn-complexity score. Higher ⇒ more likely to route to the
/// secondary (strong) leg in `hybrid` mode.
pub fn complexity_score(signals: &RoutingSignals, prompt: &Prompt) -> f32 {
    intent_weight(signals.intent) + passage_term(signals.retrieved_passages) + message_term(prompt)
}
```

Remove the now-duplicate `use crate::conversation::PedagogicalIntent;` / `use crate::inference::...` lines if they conflict with the test-module `use` statements (the test module's `use super::*;` already brings these into scope; keep the module-level `use` for the implementation and delete any redundant test-local `use crate::conversation::PedagogicalIntent;` if the compiler flags it as unused — the test block still needs `Message`, `Prompt`, `Role` which are imported there).

- [ ] **Step 4: Run test to verify it passes**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-core router::tests`
Expected: PASS (all router tests).

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-core/src/router.rs
git commit -m "feat(router): RoutingSignals + pure composite complexity_score"
```

---

## Task 4: `Leg` / `LegOrder` / `order_legs`

**Files:**
- Modify: `src/crates/primer-core/src/router.rs`

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `src/crates/primer-core/src/router.rs`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-core router::tests::local_only_never_has_a_second_leg`
Expected: FAIL — `Leg` / `LegOrder` / `order_legs` not found.

- [ ] **Step 3: Write minimal implementation**

Add to `src/crates/primer-core/src/router.rs` after `complexity_score` (before the test module). Add `use crate::consts::router::ROUTE_SECONDARY_THRESHOLD;` to the existing `consts::router` import line:

```rust
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
        RouterMode::LocalOnly => LegOrder { first: Leg::Primary, second: None },
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
```

Update the import line near the top of the file to include `ROUTE_SECONDARY_THRESHOLD`:

```rust
use crate::consts::router::{
    MSG_LONG_WORDS, MSG_QUESTION_CAP, ROUTE_PASSAGE_CAP, ROUTE_SECONDARY_THRESHOLD, W_MSG_LONG,
    W_MSG_QUESTION, W_PASSAGE,
};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-core router::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-core/src/router.rs
git commit -m "feat(router): Leg/LegOrder + pure order_legs mode mapping"
```

---

## Task 5: `GenerationParams.routing` field

**Files:**
- Modify: `src/crates/primer-core/src/inference.rs`
- Modify (add `routing: None,` to each literal): `src/crates/primer-inference/examples/qnn_bench.rs:154`, `src/crates/primer-inference/examples/llamacpp_bench.rs:135`, `src/crates/primer-inference/tests/llamacpp_smoke.rs:34`, `src/crates/primer-inference/src/llamacpp/params.rs:130`, `src/crates/primer-comprehension/src/llm.rs:60`, `src/crates/primer-classifier/src/llm.rs:47`, `src/crates/primer-extractor/src/llm.rs:57`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `src/crates/primer-core/src/inference.rs` (create the test module if absent — search for an existing one first; if none, add `#[cfg(test)] mod gen_params_tests { use super::*; ... }`):

```rust
    #[test]
    fn default_generation_params_have_no_routing() {
        let p = GenerationParams::default();
        assert!(p.routing.is_none());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-core default_generation_params_have_no_routing`
Expected: FAIL — no field `routing` on `GenerationParams`.

- [ ] **Step 3: Write minimal implementation**

In `src/crates/primer-core/src/inference.rs`, add the field to `GenerationParams` (after `stop_sequences`):

```rust
    /// Stop sequences — generation halts when any of these appear.
    pub stop_sequences: Vec<String>,
    /// Optional per-turn routing signals (Phase 1.3 inference router). Set by
    /// the dialogue manager; read ONLY by `RouterBackend`. Every other backend
    /// ignores it. `None` ⇒ no routing context (router scores it 0.0).
    pub routing: Option<crate::router::RoutingSignals>,
```

Add `routing: None` to the `Default` impl (after `stop_sequences: vec![],`):

```rust
            stop_sequences: vec![],
            routing: None,
```

Add `routing: None,` to the in-file `summarize` literal (after `stop_sequences: vec![],`):

```rust
            stop_sequences: vec![],
            routing: None,
        };
        self.generate(&prompt, &params).await
```

Then add `routing: None,` to each of these 7 external `GenerationParams { ... }` literals (each currently ends with `stop_sequences: ...,` — add the line right after it):
- `src/crates/primer-inference/examples/qnn_bench.rs:154`
- `src/crates/primer-inference/examples/llamacpp_bench.rs:135`
- `src/crates/primer-inference/tests/llamacpp_smoke.rs:34`
- `src/crates/primer-inference/src/llamacpp/params.rs:130`
- `src/crates/primer-comprehension/src/llm.rs:60`
- `src/crates/primer-classifier/src/llm.rs:47`
- `src/crates/primer-extractor/src/llm.rs:57`

(Note: leaving subsystem literals at `routing: None` is intentional — classifier/extractor/comprehension always run on the primary leg, the documented non-goal. No independent subsystem routing.)

- [ ] **Step 4: Run test + full build to verify all literals updated**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-core default_generation_params_have_no_routing`
Expected: PASS.

Run: `cd src && ~/.cargo/bin/cargo +1.88 build --workspace`
Expected: clean build (proves every default-feature literal got the field).

Run: `cd src && ~/.cargo/bin/cargo +1.88 clippy -p primer-inference --features llamacpp --all-targets -- -D warnings`
Expected: clean (proves the `llamacpp_bench` example + `llamacpp_smoke` test + `params.rs` literals compile with the new field).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(router): add GenerationParams.routing field (ignored by non-router backends)"
```

---

## Task 6: `RouterBackend` decorator

**Files:**
- Create: `src/crates/primer-inference/src/router.rs`
- Modify: `src/crates/primer-inference/src/lib.rs`

- [ ] **Step 1: Write the failing test (and the implementation together — module + tests)**

Create `src/crates/primer-inference/src/router.rs`. This mirrors `fallback.rs`'s mock harness. Full file:

```rust
//! Inference router decorator — picks between a primary (typically local/small)
//! and a secondary (typically cloud/strong) backend per turn, based on the
//! pure policy in `primer_core::router`, and self-fails-over at the pre-stream
//! boundary.
//!
//! See docs/superpowers/specs/2026-06-07-inference-router-design.md.
//!
//! Trigger policy: identical pre-stream boundary to `FallbackBackend`. The
//! router picks an ordered (first, second) leg pair via
//! `primer_core::router::order_legs`; if `first.generate_stream().await`
//! returns `Ok(stream)`, that stream is returned verbatim (mid-stream errors
//! propagate and the partial turn drops at the dialogue-manager layer — NO
//! mid-stream re-routing). If `first` returns `Err` pre-stream, `second` (when
//! present) is tried.

use async_trait::async_trait;
use std::sync::Arc;

use primer_core::error::Result;
use primer_core::inference::{GenerationParams, InferenceBackend, Prompt, TokenStream};
use primer_core::router::{complexity_score, order_legs, Leg, RouterMode};

/// Decorator wrapping a primary and a secondary backend with a routing mode.
pub struct RouterBackend {
    primary: Arc<dyn InferenceBackend>,
    secondary: Arc<dyn InferenceBackend>,
    mode: RouterMode,
}

impl RouterBackend {
    /// Construct a router. `primary` is the `--backend` leg; `secondary` is the
    /// `--fallback-backend` leg. `mode` selects the policy.
    pub fn new(
        primary: Arc<dyn InferenceBackend>,
        secondary: Arc<dyn InferenceBackend>,
        mode: RouterMode,
    ) -> Self {
        Self { primary, secondary, mode }
    }

    fn leg(&self, leg: Leg) -> &Arc<dyn InferenceBackend> {
        match leg {
            Leg::Primary => &self.primary,
            Leg::Secondary => &self.secondary,
        }
    }
}

#[async_trait]
impl InferenceBackend for RouterBackend {
    /// Returns the **primary's** name verbatim. Load-bearing: the per-backend
    /// context budget (`primer_core::backend::is_small_context_backend`) keys
    /// off `name()`. Mirrors `FallbackBackend`.
    fn name(&self) -> &str {
        self.primary.name()
    }

    async fn is_available(&self) -> bool {
        self.primary.is_available().await || self.secondary.is_available().await
    }

    async fn generate_stream(
        &self,
        prompt: &Prompt,
        params: &GenerationParams,
    ) -> Result<TokenStream> {
        let score = params
            .routing
            .as_ref()
            .map(|s| complexity_score(s, prompt))
            .unwrap_or(0.0);
        let order = order_legs(self.mode, score);
        let first = self.leg(order.first);

        match first.generate_stream(prompt, params).await {
            Ok(stream) => Ok(stream),
            Err(e) => match order.second {
                Some(second_leg) => {
                    let second = self.leg(second_leg);
                    tracing::warn!(
                        target: "primer::router",
                        first = first.name(),
                        second = second.name(),
                        error = %e,
                        "routed leg failed pre-stream; falling back to other leg"
                    );
                    second.generate_stream(prompt, params).await
                }
                None => Err(e),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{stream, StreamExt};
    use primer_core::conversation::PedagogicalIntent;
    use primer_core::error::PrimerError;
    use primer_core::inference::TokenChunk;
    use primer_core::router::RoutingSignals;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone)]
    enum Behavior {
        Ok(String),
        PreStreamErr,
        MidStreamErr,
    }

    struct MockBackend {
        name: String,
        calls: Arc<AtomicUsize>,
        behavior: Behavior,
    }

    impl MockBackend {
        fn new(name: &str, behavior: Behavior) -> (Arc<Self>, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            let b = Arc::new(Self { name: name.to_string(), calls: calls.clone(), behavior });
            (b, calls)
        }
    }

    #[async_trait]
    impl InferenceBackend for MockBackend {
        fn name(&self) -> &str {
            &self.name
        }
        async fn is_available(&self) -> bool {
            !matches!(self.behavior, Behavior::PreStreamErr)
        }
        async fn generate_stream(
            &self,
            _prompt: &Prompt,
            _params: &GenerationParams,
        ) -> Result<TokenStream> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match &self.behavior {
                Behavior::Ok(text) => {
                    let chunk = TokenChunk { text: text.clone(), done: true };
                    Ok(Box::pin(stream::once(async move { Ok(chunk) })))
                }
                Behavior::PreStreamErr => Err(PrimerError::Inference("primary down".into())),
                Behavior::MidStreamErr => Ok(Box::pin(stream::once(async {
                    Err(PrimerError::Inference("mid-stream boom".into()))
                }))),
            }
        }
    }

    fn prompt() -> Prompt {
        Prompt { system: String::new(), messages: vec![] }
    }

    fn params_with(intent: PedagogicalIntent, passages: usize) -> GenerationParams {
        GenerationParams {
            routing: Some(RoutingSignals { intent, retrieved_passages: passages }),
            ..GenerationParams::default()
        }
    }

    async fn drive(mut s: TokenStream) -> Result<String> {
        let mut out = String::new();
        while let Some(item) = s.next().await {
            let chunk = item?;
            out.push_str(&chunk.text);
            if chunk.done {
                break;
            }
        }
        Ok(out)
    }

    #[tokio::test]
    async fn hybrid_high_score_routes_to_secondary() {
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
        let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
        // Scaffolding (0.45) + 3 passages (0.45) clears the 0.5 threshold.
        let out = drive(
            r.generate_stream(&prompt(), &params_with(PedagogicalIntent::Scaffolding, 3))
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(out, "CLOUD");
        assert_eq!(scalls.load(Ordering::SeqCst), 1);
        assert_eq!(pcalls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn hybrid_low_score_routes_to_primary() {
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
        let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
        let out = drive(
            r.generate_stream(&prompt(), &params_with(PedagogicalIntent::Encouragement, 0))
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(out, "LOCAL");
        assert_eq!(pcalls.load(Ordering::SeqCst), 1);
        assert_eq!(scalls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cloud_preferred_always_tries_secondary_first() {
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
        let r = RouterBackend::new(primary, secondary, RouterMode::CloudPreferred);
        // Even a trivial turn goes to the secondary first.
        let out = drive(
            r.generate_stream(&prompt(), &params_with(PedagogicalIntent::Encouragement, 0))
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(out, "CLOUD");
        assert_eq!(scalls.load(Ordering::SeqCst), 1);
        assert_eq!(pcalls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn pre_stream_failure_falls_over_to_other_leg() {
        // Hybrid high score → secondary first; secondary is down → primary serves.
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::PreStreamErr);
        let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
        let out = drive(
            r.generate_stream(&prompt(), &params_with(PedagogicalIntent::Scaffolding, 3))
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(out, "LOCAL");
        assert_eq!(scalls.load(Ordering::SeqCst), 1, "secondary attempted first");
        assert_eq!(pcalls.load(Ordering::SeqCst), 1, "primary served the fallover");
    }

    #[tokio::test]
    async fn mid_stream_error_propagates_without_reroute() {
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::MidStreamErr);
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
        let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
        // Low score → primary first; primary streams then errors mid-stream.
        let stream = r
            .generate_stream(&prompt(), &params_with(PedagogicalIntent::Encouragement, 0))
            .await
            .unwrap();
        assert!(drive(stream).await.is_err());
        assert_eq!(pcalls.load(Ordering::SeqCst), 1);
        assert_eq!(scalls.load(Ordering::SeqCst), 0, "no mid-stream reroute");
    }

    #[tokio::test]
    async fn name_returns_primary_name() {
        let (primary, _) = MockBackend::new("qnn:Qwen3-4B", Behavior::Ok("x".into()));
        let (secondary, _) = MockBackend::new("cloud", Behavior::Ok("y".into()));
        let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
        assert_eq!(r.name(), "qnn:Qwen3-4B");
    }

    #[tokio::test]
    async fn missing_routing_signals_scores_zero_and_uses_primary_in_hybrid() {
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
        let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
        // routing: None ⇒ score 0.0 ⇒ primary.
        let out = drive(
            r.generate_stream(&prompt(), &GenerationParams::default())
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(out, "LOCAL");
        assert_eq!(pcalls.load(Ordering::SeqCst), 1);
        assert_eq!(scalls.load(Ordering::SeqCst), 0);
    }
}
```

Add to `src/crates/primer-inference/src/lib.rs`: insert `pub mod router;` after `pub mod openai_compat;` (line 25-ish) and `pub use router::RouterBackend;` after `pub use openai_compat::OpenAiCompatBackend;`:

```rust
pub mod openai_compat;
mod reasoning_stream;
pub mod router;
pub mod stub;
```
```rust
pub use openai_compat::OpenAiCompatBackend;
pub use router::RouterBackend;
pub use stub::StubBackend;
```

- [ ] **Step 2: Run test to verify it fails first (before lib.rs wiring) then passes**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-inference router::tests`
Expected: PASS (module + tests landed together).

- [ ] **Step 3: (impl is in Step 1)**

No further implementation.

- [ ] **Step 4: Clippy**

Run: `cd src && ~/.cargo/bin/cargo +1.88 clippy -p primer-inference --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-inference/src/router.rs src/crates/primer-inference/src/lib.rs
git commit -m "feat(router): RouterBackend decorator with pre-stream self-failover"
```

---

## Task 7: Wire `build_main_backend` to build the router

**Files:**
- Modify: `src/crates/primer-engine/src/wiring.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod build_main_backend_tests` in `wiring.rs`. First, update the `params(...)` helper in that test module to set `router_mode` (add a parameter; mirror the existing two-arg shape by adding a third). Replace the existing `fn params(...)` in `mod build_main_backend_tests` with:

```rust
    fn params(
        fallback_backend: Option<&str>,
        fallback_model: Option<&str>,
        router_mode: primer_core::router::RouterMode,
    ) -> BackendParams {
        BackendParams {
            api_key: None,
            ollama_url: "http://localhost:11434".into(),
            openai_compat_url: "http://localhost:8000".into(),
            openai_compat_api_key: None,
            classifier_backend: None,
            classifier_model: None,
            extractor_backend: None,
            extractor_model: None,
            comprehension_backend: None,
            comprehension_model: None,
            qnn_bundle_dir: None,
            qnn_qairt_lib_dir: None,
            gguf_path: None,
            llamacpp_gpu_layers: None,
            llamacpp_n_ctx: None,
            reasoning_markers: Vec::new(),
            fallback_backend: fallback_backend.map(String::from),
            fallback_model: fallback_model.map(String::from),
            router_mode,
        }
    }
```

Update the existing calls in that module to pass `RouterMode::LocalOnly` as the third arg (e.g. `params(None, None, RouterMode::LocalOnly)`), and add `use primer_core::router::RouterMode;` to the module. Then add these new tests:

```rust
    /// Hybrid mode + a buildable secondary ⇒ a RouterBackend whose name() is
    /// the primary's (load-bearing for the small-context budget).
    #[tokio::test]
    async fn hybrid_with_secondary_builds_router_named_after_primary() {
        let p = params(Some("stub"), None, RouterMode::Hybrid);
        let b = build_main_backend("stub", "m".into(), &p).await.unwrap();
        // Both legs are stub; RouterBackend::name() == primary.name() == "stub".
        assert_eq!(b.name(), "stub");
    }

    /// Routing mode with NO secondary configured ⇒ a clear error.
    #[tokio::test]
    async fn hybrid_without_secondary_errors() {
        let p = params(None, None, RouterMode::Hybrid);
        let r = build_main_backend("stub", "m".into(), &p).await;
        let err = r.err().expect("routing without a secondary must error");
        assert!(
            format!("{err}").to_lowercase().contains("secondary")
                || format!("{err}").to_lowercase().contains("fallback"),
            "expected a 'needs a secondary leg' error; got: {err}"
        );
    }

    /// local-only mode is byte-for-byte the existing fallback behavior.
    #[tokio::test]
    async fn local_only_with_fallback_is_unchanged_fallback() {
        let p = params(Some("stub"), None, RouterMode::LocalOnly);
        let b = build_main_backend("stub", "m".into(), &p).await.unwrap();
        assert_eq!(b.name(), "stub");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-engine hybrid_with_secondary_builds_router_named_after_primary`
Expected: FAIL — `BackendParams` has no field `router_mode` (compile error).

- [ ] **Step 3: Write minimal implementation**

In `wiring.rs`, add the field to `BackendParams` (after `fallback_model`):

```rust
    pub fallback_model: Option<String>,
    /// Phase 1.3 inference-router mode. `LocalOnly` (the default) ⇒ today's
    /// behavior (primary or `FallbackBackend`). `Hybrid`/`CloudPreferred` ⇒
    /// `build_main_backend` produces a `RouterBackend` over the primary +
    /// secondary legs. Consumed only by [`build_main_backend`].
    pub router_mode: primer_core::router::RouterMode,
```

Add an early branch at the TOP of `build_main_backend` (right after the same-backend warn block, before `let primary = build_backend(...)`):

```rust
    // Routing modes build a RouterBackend over the same two legs the fallback
    // uses; LocalOnly falls through to today's primary/fallback logic verbatim.
    if params.router_mode.uses_secondary() {
        return build_router_backend(primary_name, primary_model, params).await;
    }
```

Add the new free function after `build_main_backend`:

```rust
/// Construct a `RouterBackend` for a non-`LocalOnly` mode. Requires a
/// configured secondary leg (`--fallback-backend`); both legs are built and
/// wrapped. Degrades gracefully if exactly one leg fails to build (mirrors
/// `plan_main_backend`): primary-only (routing disabled, warn) or
/// secondary-only (warn); both-fail surfaces the primary's error.
async fn build_router_backend(
    primary_name: &str,
    primary_model: String,
    params: &BackendParams,
) -> Result<Arc<dyn InferenceBackend>> {
    use primer_inference::RouterBackend;

    let Some(fb_name) = params.fallback_backend.clone() else {
        return Err(PrimerError::Inference(
            format!(
                "router mode '{}' requires a secondary leg; set --fallback-backend \
                 (and --fallback-model where required)",
                params.router_mode
            )
            .into(),
        ));
    };

    let primary = build_backend(primary_name, primary_model, params).await;
    let secondary = match resolve_fallback_model(&fb_name, params.fallback_model.clone()) {
        Ok(fb_model) => build_backend(&fb_name, fb_model.unwrap_or_default(), params).await,
        Err(msg) => Err(PrimerError::Inference(msg.into())),
    };

    match plan_main_backend(primary.is_ok(), true, secondary.is_ok()) {
        MainBackendPlan::Wrapped => {
            let primary = primary.expect("Wrapped implies primary built");
            let secondary = secondary.expect("Wrapped implies secondary built");
            Ok(Arc::new(RouterBackend::new(primary, secondary, params.router_mode)))
        }
        MainBackendPlan::SecondaryAlone => {
            tracing::warn!(
                primary = primary_name,
                secondary = %fb_name,
                "router: primary unavailable at startup; using secondary alone (no routing)"
            );
            secondary
        }
        MainBackendPlan::PrimaryAlone => {
            if let Err(ref e) = secondary {
                tracing::warn!(
                    secondary = %fb_name,
                    error = %e,
                    "router: secondary unusable; using primary alone (routing disabled)"
                );
            }
            primary
        }
        MainBackendPlan::Fail => primary,
    }
}
```

Now update EVERY other `BackendParams { ... }` literal in `wiring.rs` (the test helpers in `classifier_construction_tests`, `extractor_construction_tests`, `comprehension_construction_tests`, `qnn_dispatch_tests`) to add `router_mode: primer_core::router::RouterMode::LocalOnly,` after `fallback_model: None,`. There are 4 such helper literals plus any inline `BackendParams { ... }` (e.g. the `..params()` spreads need no change; only full literals do).

- [ ] **Step 4: Run test to verify it passes + full crate test**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-engine`
Expected: PASS (all wiring tests, including the 3 new ones).

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-engine/src/wiring.rs
git commit -m "feat(router): wire build_main_backend to build RouterBackend for routing modes"
```

---

## Task 8: CLI `--router-mode` flag

**Files:**
- Modify: `src/crates/primer-cli/src/main.rs`

- [ ] **Step 1: Add the flag, validation, and BackendParams field**

In `src/crates/primer-cli/src/main.rs`, add the flag to the `Cli` struct near the fallback flags (after `fallback_model`):

```rust
    /// Inference router mode (Phase 1.3): "local-only" (default; never uses the
    /// cloud — the privacy default), "cloud-preferred" (always try the
    /// secondary first), or "hybrid" (route complex turns to the secondary).
    /// Routing modes require --fallback-backend (the secondary leg).
    #[arg(long, default_value = "local-only")]
    router_mode: String,
```

Parse the flag ONCE into a local, immediately after the CLI is parsed and before `BackendParams` is constructed (so the same value feeds both the param and the validation — no double parse). Add this right after the `Cli::parse()` / args resolution, before the `BackendParams { ... }` block (~line 910):

```rust
    let router_mode: primer_core::router::RouterMode = match cli.router_mode.parse() {
        Ok(m) => m,
        Err(msg) => {
            eprintln!("error: {msg}");
            std::process::exit(2);
        }
    };
    if router_mode.uses_secondary() && cli.fallback_backend.is_none() {
        eprintln!(
            "error: --router-mode {router_mode} requires --fallback-backend (the secondary leg to route to)"
        );
        std::process::exit(2);
    }
```

Then in the `BackendParams { ... }` construction (around line 918), add after `fallback_model: cli.fallback_model.clone(),`:

```rust
        fallback_model: cli.fallback_model.clone(),
        router_mode,
```

(`router_mode` is the local parsed above; the field name matches so this is a shorthand init.)

- [ ] **Step 2: Build the CLI**

Run: `cd src && ~/.cargo/bin/cargo +1.88 build -p primer-cli`
Expected: clean build.

- [ ] **Step 3: Verify the flag appears and validation works**

Run: `cd src && ~/.cargo/bin/cargo +1.88 run -p primer-cli --bin primer -- --help 2>&1 | grep -A2 router-mode`
Expected: shows `--router-mode` with the three choices in the help text.

Run: `cd src && ~/.cargo/bin/cargo +1.88 run -p primer-cli --bin primer -- --backend stub --router-mode hybrid 2>&1 | head -3`
Expected: the `error: --router-mode hybrid requires --fallback-backend ...` message (exit 2), because no secondary was configured.

- [ ] **Step 4: Sanity — local-only default still runs**

Run: `cd src && echo "quit" | ~/.cargo/bin/cargo +1.88 run -p primer-cli --bin primer -- --backend stub 2>&1 | tail -3`
Expected: the REPL starts and exits cleanly (no router error; default mode is local-only).

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-cli/src/main.rs
git commit -m "feat(router): CLI --router-mode flag + secondary-required validation"
```

---

## Task 9: Dialogue-manager threads RoutingSignals

**Files:**
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager/turn.rs`

- [ ] **Step 1: Write the failing test**

Add a test to the dialogue-manager turn tests. First locate the turn-test file: `src/crates/primer-pedagogy/src/dialogue_manager/tests/turn_tests.rs` (per CLAUDE.md). Add a test using a recording mock backend that captures the `GenerationParams.routing` it receives. If `test_support.rs` already has a configurable mock backend, extend it to expose the last-seen `routing`; otherwise add a minimal capturing backend in the test file:

```rust
    /// The dialogue manager populates GenerationParams.routing with the turn's
    /// intent and the retrieved-passage count, so a RouterBackend can route.
    #[tokio::test]
    async fn respond_to_streaming_threads_routing_signals() {
        use std::sync::{Arc, Mutex};
        use primer_core::inference::{GenerationParams, InferenceBackend, Prompt, TokenChunk, TokenStream};
        use primer_core::error::Result as PResult;

        #[derive(Default)]
        struct CapturingBackend {
            last_routing: Arc<Mutex<Option<primer_core::router::RoutingSignals>>>,
        }
        #[async_trait::async_trait]
        impl InferenceBackend for CapturingBackend {
            fn name(&self) -> &str { "capture" }
            async fn is_available(&self) -> bool { true }
            async fn generate_stream(&self, _p: &Prompt, params: &GenerationParams) -> PResult<TokenStream> {
                *self.last_routing.lock().unwrap() = params.routing.clone();
                use futures::stream;
                Ok(Box::pin(stream::once(async {
                    Ok(TokenChunk { text: "ok".into(), done: true })
                })))
            }
        }

        let captured = Arc::new(Mutex::new(None));
        let backend = CapturingBackend { last_routing: captured.clone() };
        // Build a DialogueManager around `backend` + an empty/stub KB using the
        // existing test_support builder, then:
        // let mut dm = test_support::dm_with_backend(&backend, &kb, ...);
        // dm.open_session(...).await;
        // dm.respond_to("why is the sky blue?").await.unwrap();
        // let routing = captured.lock().unwrap().clone();
        // assert!(routing.is_some());
        // (intent will be whatever decide_intent picks; just assert presence +
        //  that retrieved_passages matches the KB's returned count, which is 0
        //  for an empty KB.)
        // assert_eq!(routing.unwrap().retrieved_passages, 0);
    }
```

NOTE TO IMPLEMENTER: adapt the construction lines to the actual `test_support` builder signature in `src/crates/primer-pedagogy/src/dialogue_manager/tests/test_support.rs` (read it first). The assertion that matters: after one `respond_to`, the backend saw `params.routing == Some(_)` with `retrieved_passages` equal to the KB's returned passage count (0 for the empty/stub KB used in tests).

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-pedagogy respond_to_streaming_threads_routing_signals`
Expected: FAIL — `params.routing` is `None` (the DM does not set it yet).

- [ ] **Step 3: Write minimal implementation**

In `turn.rs`, change `build_turn_prompt` to also return the retrieved-passage count. Update its signature and body:

```rust
    pub(super) async fn build_turn_prompt(
        &self,
        child_input: &str,
        intent: PedagogicalIntent,
    ) -> (Prompt, usize) {
        let knowledge_context = self.retrieve_knowledge(child_input).await;
        let passage_count = knowledge_context.len();
        let (summary, retrieved_older) = self.retrieve_long_term_memory(child_input).await;
        let due_vocab = primer_core::vocab::due_concepts(
            &self.learner,
            chrono::Utc::now(),
            self.vocab_settings.max_per_prompt,
        );
        let prompt = prompt_builder::build_prompt_with_pack_and_vocab(
            &*self.prompt_pack,
            &self.learner,
            &self.session,
            intent,
            &knowledge_context,
            &summary,
            &retrieved_older,
            self.config
                .effective_context_window_turns(self.inference.name()),
            &due_vocab,
            self.config.break_suggest_after_minutes,
        );
        (prompt, passage_count)
    }
```

(Check the type of `knowledge_context` — if it is not a `Vec`/slice with `.len()`, derive the count from however `retrieve_knowledge` reports its results. Grep `fn retrieve_knowledge` in `retrieval.rs` to confirm the return type; it returns the retrieved-passages context, so `.len()` or a `.passages.len()` is the count.)

Update the caller in `respond_to_streaming`:

```rust
        let (prompt, passage_count) = self.build_turn_prompt(child_input, intent).await;

        // 3. Stream the response, accumulating into a single String.
        let result = self
            .stream_inference_response(&prompt, intent, passage_count, on_chunk)
            .await;
```

Update `stream_inference_response` to accept the signals and set `params.routing`:

```rust
    async fn stream_inference_response<F>(
        &self,
        prompt: &Prompt,
        intent: PedagogicalIntent,
        passage_count: usize,
        mut on_chunk: F,
    ) -> Result<String>
    where
        F: FnMut(&str),
    {
        let params = GenerationParams {
            routing: Some(primer_core::router::RoutingSignals {
                intent,
                retrieved_passages: passage_count,
            }),
            ..GenerationParams::default()
        };
        let mut stream = self.inference.generate_stream(prompt, &params).await?;

        let mut accumulated = String::new();
        while let Some(item) = stream.next().await {
            let chunk = item.inspect_err(|e| {
                tracing::warn!("Stream error mid-generation: {e}");
            })?;
            if !chunk.text.is_empty() {
                on_chunk(&chunk.text);
                accumulated.push_str(&chunk.text);
            }
            if chunk.done {
                break;
            }
        }
        Ok(accumulated)
    }
```

- [ ] **Step 4: Run test + the whole pedagogy crate**

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-pedagogy`
Expected: PASS (the new test plus all existing turn/lifecycle/background tests — the `build_turn_prompt` return-type change must not break any caller; it has exactly one caller).

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-pedagogy/src/dialogue_manager/turn.rs
git commit -m "feat(router): thread RoutingSignals (intent + passage count) into inference params"
```

---

## Task 10: GUI router-mode mirror

**Files:**
- Modify: `src/crates/primer-gui/src/config.rs`
- Modify: `src/crates/primer-gui/src/wiring.rs`
- Modify: `src/crates/primer-gui/ui/settings.js`
- Modify: the settings HTML (find it: `grep -rl "f-backend-fallback-backend" src/crates/primer-gui/ui/`)

- [ ] **Step 1: Add the config field + View/Update + defaults**

In `src/crates/primer-gui/src/config.rs`, add to `BackendConfig` (after `fallback_model`):

```rust
    pub fallback_model: Option<String>,
    /// Phase 1.3 inference-router mode. Mirrors the CLI's `--router-mode`.
    /// `LocalOnly` (default) ⇒ no routing (today's behavior). Consumed by
    /// `primer_engine::build_main_backend` via `BackendParams.router_mode`.
    #[serde(default)]
    pub router_mode: primer_core::router::RouterMode,
```

Add to the `BackendConfig` `Default` impl (after `fallback_model: None,`):

```rust
            fallback_model: None,
            router_mode: primer_core::router::RouterMode::LocalOnly,
```

In `BackendConfigView` (struct at line ~655), add a string field (after the fallback fields):

```rust
    pub fallback_model: Option<String>,
    /// Router mode as its canonical kebab-case name (e.g. "local-only").
    pub router_mode: String,
```

In `BackendConfigUpdate` (struct at line ~742; NO `#[serde(default)]` — every field mandatory), add:

```rust
    pub fallback_model: Option<String>,
    /// Router mode kebab-case name. Parsed via `RouterMode::from_str`.
    pub router_mode: String,
```

Find where `BackendConfigView` is built from `BackendConfig` (grep `BackendConfigView {` in config.rs) and add `router_mode: self.router_mode.to_string(),` (or `cfg.backend.router_mode.name().to_string()` depending on the builder shape). Find where `BackendConfigUpdate` is applied to `BackendConfig` (grep `fn apply` / `BackendConfigUpdate`) and add:

```rust
            self.router_mode = update
                .router_mode
                .parse()
                .map_err(|e: String| e)?;
```

(Match the existing error-propagation style of the apply method — if it returns `Result<_, String>`, the `.map_err` above fits; if it returns `()`, fall back to `unwrap_or_default()` with a `tracing::warn!` on parse failure, mirroring how the apply method handles other invalid fields.)

- [ ] **Step 2: Pass router_mode into BackendParams in wiring**

In `src/crates/primer-gui/src/wiring.rs`, in the `BackendParams { ... }` construction (after `fallback_model: backend_config.fallback_model.clone(),` at line ~204):

```rust
        fallback_model: backend_config.fallback_model.clone(),
        router_mode: backend_config.router_mode,
```

- [ ] **Step 3: Build the GUI crate (config + wiring compile)**

Run: `cd src && ~/.cargo/bin/cargo +1.88 build -p primer-gui`
Expected: clean build (proves the new field threads through View/Update/wiring; any IPC test literal of `BackendConfigUpdate` that now misses `router_mode` will fail here — fix each by adding `router_mode: "local-only".to_string(),`).

- [ ] **Step 4: Frontend — picker + apply + gather**

In the settings HTML file, add a `<select id="f-backend-router-mode">` with three `<option>`s (`local-only` / `cloud-preferred` / `hybrid`, default `local-only`) inside the Inference-backend section, mirroring the existing `f-backend-fallback-backend` select markup.

In `src/crates/primer-gui/ui/settings.js`:
- Register the element near the other backend fields (after line ~66):
  ```js
  backendRouterMode: document.getElementById("f-backend-router-mode"),
  ```
- In the apply-view function (after line ~305 where fallback fields are set):
  ```js
  f.backendRouterMode.value = view.backend.router_mode ?? "local-only";
  ```
- In `gather()` (after the `fallback_model:` line ~784):
  ```js
  router_mode: f.backendRouterMode.value || "local-only",
  ```

Run: `cd src && ~/.cargo/bin/cargo +1.88 test -p primer-gui`
Expected: PASS (config round-trip + any IPC tests, now carrying `router_mode`).

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-gui/src/config.rs src/crates/primer-gui/src/wiring.rs src/crates/primer-gui/ui/settings.js src/crates/primer-gui/ui/*.html
git commit -m "feat(router): GUI router-mode mirror (config + view/update + settings picker)"
```

---

## Task 11: Documentation

**Files:**
- Modify: `CLAUDE.md`, `README.md`, `ROADMAP.md`

- [ ] **Step 1: ROADMAP**

In `ROADMAP.md`, update the 1.3 section: mark the router as shipped for complexity-based routing + config modes, note latency-aware switching is the remaining deferred piece:

```markdown
### 1.3 — Hybrid inference 🟡

- [x] Inference router (`RouterBackend` decorator + `--router-mode local-only|cloud-preferred|hybrid`; pure composite-complexity policy in `primer_core::router`; CLI + GUI). Routes routine turns to the local primary and complex/knowledge-intensive turns to the cloud secondary, self-failing-over at the pre-stream boundary; reuses the `--fallback-*` secondary leg. **Latency-aware switching is a designed extension point, deferred** until the owner-gated bench numbers exist.
```

- [ ] **Step 2: README**

In `README.md`, add the router to the status/usage section and **explicitly document the absolute-privacy path** (per the spec's privacy section). Add near the backend/fallback docs:

```markdown
### Inference routing (hybrid local + cloud)

`--router-mode` chooses how turns are routed between your local `--backend` and
an optional cloud `--fallback-backend`:

- `local-only` (default) — **never** uses the cloud. The runtime works with zero
  network.
- `cloud-preferred` — always tries the cloud secondary first, falling back to
  local if it is unreachable.
- `hybrid` — routes routine turns to local and complex/knowledge-intensive turns
  (a struggling child, hard factual questions) to the stronger cloud model.

**For absolute privacy, do nothing:** leave `--router-mode` at its `local-only`
default and/or never provide an API key. With no API key the cloud leg cannot be
built, so even a mis-set `hybrid` degrades to local-only. Routing to the cloud
happens only when you explicitly choose `hybrid`/`cloud-preferred` AND configure
a cloud secondary — that choice is the consent.
```

- [ ] **Step 3: CLAUDE.md**

In `CLAUDE.md`, add a bullet to the conventions/gotchas section describing the router (mirror the `FallbackBackend` bullet's depth): the decorator + threaded `RoutingSignals` on `GenerationParams`, the pure policy in `primer_core::router`, the same-`build_main_backend` construction path, `name()` returns the primary's name, subsystems always run on the primary (routing:None ⇒ score 0), and the GUI mirror's `Update`-has-no-`serde(default)` gotcha for `router_mode`. Also update the CLI-flags paragraph to list `--router-mode`.

- [ ] **Step 4: Verify docs build/links (no code change — just a sanity grep)**

Run: `grep -n "router_mode\|router-mode\|RouterBackend" CLAUDE.md README.md ROADMAP.md`
Expected: each file shows the new references.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md README.md ROADMAP.md
git commit -m "docs(router): document inference router + absolute-privacy path (CLAUDE/README/ROADMAP)"
```

---

## Final verification (run before opening the PR)

- [ ] **Full workspace verify loop (from `src/`, pinned toolchain):**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo +1.88 test --workspace --no-fail-fast
# feature-gated paths the workspace clippy/test don't cover:
~/.cargo/bin/cargo +1.88 clippy -p primer-inference --features llamacpp --all-targets -- -D warnings
~/.cargo/bin/cargo +1.88 test -p primer-inference --features qnn
```
Expected: all clean / 0 failures.

- [ ] **Open the PR (branch protection is on `main`):**

```bash
git push -u origin feat/inference-router
gh pr create --base main --title "feat(inference): Phase 1.3 inference router (complexity routing + config modes)" \
  --body "Implements the Phase 1.3 inference router per docs/superpowers/specs/2026-06-07-inference-router-design.md. RouterBackend decorator + threaded RoutingSignals + pure composite-complexity policy + three config modes (CLI + GUI). Latency-aware switching deferred as a designed extension point. 🤖 Generated with [Claude Code](https://claude.com/claude-code)"
gh pr checks <n>
```
Expected: `cargo test (default features)` green, then squash-merge.

---

## Self-review notes (spec coverage)

- Spec §"Architecture" → Tasks 1–7 (core policy, decorator, wiring).
- Spec §"Routing policy" (score, intent table, modes) → Tasks 1, 3, 4.
- Spec §"Dialogue-manager integration" → Task 9.
- Spec §"Latency extension point" → reserved comment in Task 3's `RoutingSignals` (no code, by design).
- Spec §"CLI surface" → Task 8.
- Spec §"GUI surface" → Task 10.
- Spec §"Privacy posture" (incl. absolute-privacy doc requirement) → Task 11 Steps 2–3.
- Spec §"Testing" → tests embedded in Tasks 1–7, 9, 10.
- `FallbackBackend` left intact (no-routing path) → Task 7 keeps `LocalOnly` byte-identical.
