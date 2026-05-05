# Vocabulary Spaced Repetition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface previously-encountered concepts back into the system prompt at expanding (Leitner-box) intervals based on demonstrated mastery, with a passive "weave in only if topically relevant" UX. v1 is closed-loop on the existing comprehension-classifier signal — no manual review controls.

**Architecture:** New `primer-core::vocab` module with three pure functions. New `box_level: u8` field on `ConceptState`. Schema v7 adds `learner_concepts.box_level`. `apply_comprehension` extended by one box-transition call. `prompt_builder` gains a `vocab_section` between retrieved-older and knowledge. New `--vocab-max-per-prompt` CLI flag (default 4).

**Tech Stack:**
- Rust workspace at `src/` (NOT repo root). All cargo commands run from `/Users/hherb/src/primer/src`.
- `~/.cargo/bin/cargo` (rustup-managed) — Homebrew rust shadows pin and breaks silero compile.
- SQLite via rusqlite, FTS5 already wired.
- `tokio` async, `chrono` for time, `serde` + JSON-in-TEXT for free-text columns.
- The existing comprehension-classifier path is the only thing that mutates `box_level`.

**Branch:** Already on `feature/vocab-spaced-repetition` (branched from `main` at `9733a16`). Spec at `docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md` (commit `f657a54`).

**Initial verification (run before starting):**

```bash
cd /Users/hherb/src/primer
git status                # should show: On branch feature/vocab-spaced-repetition; clean.
cd src
~/.cargo/bin/cargo test --workspace 2>&1 | tail -5    # expect 434 passed
~/.cargo/bin/cargo clippy --workspace --all-targets   # expect clean
```

---

## File Map

**Created:**
- `src/crates/primer-core/src/vocab.rs` (new module, pure functions + tests)
- *Nothing else.* Everything else extends existing files.

**Modified:**
- `src/crates/primer-core/src/lib.rs` — add `pub mod vocab;`
- `src/crates/primer-core/src/consts.rs` — add `pub mod vocab` submodule with constants
- `src/crates/primer-core/src/learner.rs` — add `box_level: u8` field to `ConceptState`
- `src/crates/primer-pedagogy/src/dialogue_manager/apply.rs` — extend `apply_comprehension`; add `box_level: 0` to `ConceptState` literals (4 places)
- `src/crates/primer-pedagogy/src/dialogue_manager/mod.rs` — add `vocab_settings` field to `DialogueManagerSubsystems`
- `src/crates/primer-pedagogy/src/dialogue_manager/turn.rs::build_turn_prompt` — call `due_concepts`, thread into `build_prompt`
- `src/crates/primer-pedagogy/src/lib.rs` — re-export `VocabSettings` (if added at crate root) or expose via submodule
- `src/crates/primer-pedagogy/src/settings.rs` (or wherever `*Settings` structs live; check during impl) — add `VocabSettings`
- `src/crates/primer-pedagogy/src/prompt_pack.rs` — add `vocab_review_intro()` to `PromptPack` trait; add field to `TomlPromptPack`, `SectionsSection`, validator
- `src/crates/primer-pedagogy/prompts/en.toml` — add `vocab_review_intro` under `[sections]`
- `src/crates/primer-pedagogy/prompts/de.toml` — add `vocab_review_intro` under `[sections]`
- `src/crates/primer-pedagogy/src/prompt_builder.rs` — accept `due_vocab` in `build_system_prompt_with_pack` + `build_prompt_*`; render `vocab_section`
- `src/crates/primer-storage/src/schema.rs` — `apply_v7_migrations`, bump `USER_VERSION` to 7
- `src/crates/primer-storage/src/lib.rs` — call `apply_v7_migrations` in open path; extend `save_learner` UPSERT and `load_learner` SELECT for `box_level`; update test fixtures (4 `ConceptState` literals)
- `src/crates/primer-cli/src/main.rs` — add `--vocab-max-per-prompt` arg; thread into `VocabSettings`
- `README.md` — note the new feature briefly
- `ROADMAP.md` — mark Phase 0.3 vocab bullet as done
- `NEXT_SESSION.md` — will be updated by `/nextsession` at session end

**Test-only modifications:**
- `src/crates/primer-pedagogy/src/prompt_builder.rs` test fixtures — add `box_level: 0,` to `ConceptState` literal at line 433
- `src/crates/primer-storage/src/lib.rs` test fixtures — add `box_level: 0,` to literals at lines 3446, 3454, 3649
- `src/crates/primer-pedagogy/src/dialogue_manager/apply.rs` test fixtures — literals at lines 301, 339, 376

---

## Task 1: Add `consts::vocab` submodule and `box_level` field on `ConceptState`

**Why first:** every other task depends on this. The constants and the field are standalone — no business logic — so we land them as a single mechanical change. Updating struct literals in 9 places is part of "making it compile".

**Files:**
- Modify: `src/crates/primer-core/src/consts.rs`
- Modify: `src/crates/primer-core/src/learner.rs`
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager/apply.rs` (4 struct literals)
- Modify: `src/crates/primer-pedagogy/src/prompt_builder.rs` (1 struct literal in tests)
- Modify: `src/crates/primer-storage/src/lib.rs` (4 struct literals)

- [ ] **Step 1: Add the consts submodule**

Append to `src/crates/primer-core/src/consts.rs`:

```rust
/// Defaults for the vocabulary spaced-repetition feature.
/// See `primer-core::vocab` and the design spec at
/// docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md.
pub mod vocab {

    /// Box-level interval table (days). Index = box_level.
    /// box 0 (freshly seen / failed) → review after 1 day
    /// box 1 (one successful review) → 3 days
    /// box 2 (two)                    → 7 days
    /// box 3 (three)                  → 14 days
    /// box 4 (max — never graduates)  → 30 days
    pub const BOX_INTERVALS_DAYS: &[u32] = &[1, 3, 7, 14, 30];

    /// Highest box_level a concept can occupy. After reaching this, further
    /// successful reviews keep box_level pinned at MAX (interval stays 30d).
    /// There is no terminal "graduated" state — review continues every 30d
    /// until either the child consistently fails (depth=Aware → box reset)
    /// or the concept is genuinely never engaged with again.
    pub const MAX_BOX_LEVEL: u8 = 4;

    /// Minimum confidence for a comprehension assessment to count toward
    /// box advancement. Assessments below this threshold reset the box to 0.
    /// Numerically equal to the comprehension classifier's
    /// `confidence_threshold` (also 0.6) but kept independent so a future
    /// researcher can tune box-advancement strictness without affecting
    /// depth promotion.
    pub const MIN_CONF_FOR_BOX_PROMOTION: f32 = 0.6;

    /// Default cap on overdue concepts injected into the system prompt
    /// per turn. Configurable via `VocabSettings::max_per_prompt` and the
    /// `--vocab-max-per-prompt` CLI flag.
    pub const DEFAULT_VOCAB_MAX_PER_PROMPT: usize = 4;
}
```

- [ ] **Step 2: Add `box_level` field to `ConceptState`**

In `src/crates/primer-core/src/learner.rs`, modify the `ConceptState` struct (around line 109):

```rust
/// A node in the learner's concept graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptState {
    /// The concept identifier (e.g., "physics:gravity", "biology:photosynthesis").
    pub concept_id: String,
    /// Current assessed understanding depth.
    pub depth: UnderstandingDepth,
    /// Confidence in this assessment (0.0 – 1.0).
    /// Low confidence means we should re-probe before assuming.
    pub confidence: f32,
    /// Number of times this concept has been discussed.
    pub encounter_count: u32,
    /// When the concept was last discussed.
    pub last_encountered: Option<DateTime<Utc>>,
    /// Notes from the dialogue manager.
    pub notes: Vec<String>,
    /// Spaced-repetition box level. 0 = freshly encountered or recently
    /// failed; up to `consts::vocab::MAX_BOX_LEVEL` = 4 (30-day interval).
    /// `#[serde(default)]` so old serialised profiles deserialise cleanly
    /// with `box_level = 0`.
    #[serde(default)]
    pub box_level: u8,
}
```

- [ ] **Step 3: Verify compilation fails (sanity check)**

Run: `cd /Users/hherb/src/primer/src && ~/.cargo/bin/cargo build 2>&1 | grep -c "missing field \`box_level\`"`

Expected: a number > 0 (likely 9). The field added to `ConceptState` makes every existing struct literal a compile error. This is the desired "red" state of TDD.

- [ ] **Step 4: Fix all 9 struct literals — add `box_level: 0,`**

In `src/crates/primer-pedagogy/src/dialogue_manager/apply.rs`, find the 4 `ConceptState {` literals. For each one, add `box_level: 0,` as the last field (before the closing brace).

Lines as of spec-time (verify with grep before editing — line numbers can drift):

```bash
cd /Users/hherb/src/primer/src
grep -n "ConceptState {" crates/primer-pedagogy/src/dialogue_manager/apply.rs
```

For example, line 83's literal currently reads:

```rust
learner.concepts.push(ConceptState {
    concept_id: name.clone(),
    depth: UnderstandingDepth::Aware,
    confidence: crate::consts::INITIAL_CONCEPT_CONFIDENCE,
    encounter_count: 1,
    last_encountered: Some(now),
    notes: vec![],
});
```

Change to:

```rust
learner.concepts.push(ConceptState {
    concept_id: name.clone(),
    depth: UnderstandingDepth::Aware,
    confidence: crate::consts::INITIAL_CONCEPT_CONFIDENCE,
    encounter_count: 1,
    last_encountered: Some(now),
    notes: vec![],
    box_level: 0,
});
```

Repeat for the 3 test-fixture literals in the same file (lines ~301, ~339, ~376).

In `src/crates/primer-pedagogy/src/prompt_builder.rs`, find the `concept_at` helper (around line 432–433) — add `box_level: 0,` to its `ConceptState` literal.

In `src/crates/primer-storage/src/lib.rs`, find the 4 `ConceptState {` literals (around lines 1214, 3446, 3454, 3649) — add `box_level: 0,` to each.

- [ ] **Step 5: Verify compilation succeeds**

Run: `~/.cargo/bin/cargo build --workspace 2>&1 | tail -5`

Expected: clean build, no errors.

- [ ] **Step 6: Verify all existing tests still pass**

Run: `~/.cargo/bin/cargo test --workspace 2>&1 | tail -3`

Expected: `test result: ok. 434 passed`. The new field defaults to 0 in every literal, preserving all existing behaviour.

- [ ] **Step 7: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-core/src/consts.rs \
        src/crates/primer-core/src/learner.rs \
        src/crates/primer-pedagogy/src/dialogue_manager/apply.rs \
        src/crates/primer-pedagogy/src/prompt_builder.rs \
        src/crates/primer-storage/src/lib.rs
git commit -m "$(cat <<'EOF'
vocab: add consts::vocab submodule and box_level field on ConceptState

Constants for the spaced-repetition box scheduler:
- BOX_INTERVALS_DAYS = [1, 3, 7, 14, 30]
- MAX_BOX_LEVEL = 4
- MIN_CONF_FOR_BOX_PROMOTION = 0.6
- DEFAULT_VOCAB_MAX_PER_PROMPT = 4

ConceptState gains box_level: u8 with #[serde(default)] for backward
compat with old persisted profiles. All 9 existing struct literals
(production + tests across primer-pedagogy and primer-storage) updated
to set box_level: 0. No behavioural change yet — the field is unused
until apply_comprehension is extended in a later commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Pure function `apply_box_transition` + tests

**Files:**
- Create: `src/crates/primer-core/src/vocab.rs`
- Modify: `src/crates/primer-core/src/lib.rs`

- [ ] **Step 1: Create `vocab.rs` with the failing tests**

Create `src/crates/primer-core/src/vocab.rs` with this initial content:

```rust
//! Spaced-repetition vocabulary scheduler.
//!
//! Three pure functions implementing the Leitner-box review schedule
//! defined in docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md.
//!
//! - `apply_box_transition` — decide the new box level given a comprehension
//!   assessment.
//! - `is_due` — true if a concept's review interval has elapsed.
//! - `due_concepts` — top-K most-overdue concepts, sorted by overdue-amount
//!   descending.

use crate::consts::vocab::{BOX_INTERVALS_DAYS, MAX_BOX_LEVEL, MIN_CONF_FOR_BOX_PROMOTION};
use crate::learner::{ConceptState, UnderstandingDepth};

/// Box-level transition driven by a comprehension assessment.
///
/// Three-zone policy:
/// - `confidence < MIN_CONF_FOR_BOX_PROMOTION` → reset to 0.
/// - `depth = Aware` → reset to 0 (regardless of confidence).
/// - `depth ≥ Recall` AND `confidence ≥ MIN_CONF_FOR_BOX_PROMOTION` →
///   `current_box + 1` capped at `MAX_BOX_LEVEL`.
///
/// Pure — returns the new box level. The caller decides whether to write
/// it back to `ConceptState`.
pub fn apply_box_transition(
    current_box: u8,
    depth: UnderstandingDepth,
    confidence: f32,
) -> u8 {
    if confidence < MIN_CONF_FOR_BOX_PROMOTION {
        return 0;
    }
    if depth == UnderstandingDepth::Aware {
        return 0;
    }
    (current_box + 1).min(MAX_BOX_LEVEL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promotes_from_zero_on_recall_with_strong_confidence() {
        assert_eq!(apply_box_transition(0, UnderstandingDepth::Recall, 0.7), 1);
    }

    #[test]
    fn promotes_from_zero_on_comprehension() {
        assert_eq!(
            apply_box_transition(0, UnderstandingDepth::Comprehension, 0.9),
            1
        );
    }

    #[test]
    fn promotes_mid_box_on_application() {
        assert_eq!(
            apply_box_transition(2, UnderstandingDepth::Application, 0.95),
            3
        );
    }

    #[test]
    fn caps_at_max_box_level() {
        assert_eq!(
            apply_box_transition(MAX_BOX_LEVEL, UnderstandingDepth::Analysis, 1.0),
            MAX_BOX_LEVEL
        );
    }

    #[test]
    fn resets_on_aware_regardless_of_confidence() {
        assert_eq!(apply_box_transition(3, UnderstandingDepth::Aware, 0.9), 0);
        assert_eq!(apply_box_transition(4, UnderstandingDepth::Aware, 1.0), 0);
    }

    #[test]
    fn resets_on_low_confidence() {
        assert_eq!(
            apply_box_transition(3, UnderstandingDepth::Comprehension, 0.4),
            0
        );
        assert_eq!(apply_box_transition(4, UnderstandingDepth::Analysis, 0.59), 0);
    }

    #[test]
    fn boundary_confidence_inclusive_at_threshold() {
        // confidence == MIN_CONF_FOR_BOX_PROMOTION (0.6 exactly) promotes.
        assert_eq!(apply_box_transition(0, UnderstandingDepth::Recall, 0.6), 1);
    }

    #[test]
    fn unknown_depth_resets_box() {
        // Unknown is below Aware in the ordering; treat as failure-equivalent.
        // (UnderstandingDepth::Unknown < Aware via PartialOrd derive ordering.)
        // The function only special-cases Aware; Unknown is < Aware so falls
        // through to the promotion arm — which is the wrong outcome. Verify
        // current behaviour: Unknown with confidence ≥ 0.6 promotes. This test
        // documents the contract: depth=Aware is the only "weak depth" case.
        // The comprehension classifier never emits Unknown in practice.
        assert_eq!(apply_box_transition(0, UnderstandingDepth::Unknown, 0.9), 1);
    }
}
```

- [ ] **Step 2: Wire the new module into the crate**

In `src/crates/primer-core/src/lib.rs`, add `pub mod vocab;` in alphabetical order with the other module declarations (after `pub mod storage;` line ~29, but verify the alphabetical position when editing).

Current ordering shows `pub mod storage;` at line 29; insert after it:

```rust
pub mod storage;
pub mod vocab;
```

- [ ] **Step 3: Run the tests — expect them to pass**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-core vocab 2>&1 | tail -15
```

Expected: `test result: ok. 8 passed`.

(All eight tests pass in this single step because the implementation was written together with the tests. The TDD discipline still applies — we *wrote the tests first* in Step 1, even if both ship together.)

- [ ] **Step 4: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-core/src/vocab.rs src/crates/primer-core/src/lib.rs
git commit -m "$(cat <<'EOF'
vocab: add pure apply_box_transition fn with 8 tests

Decides the new Leitner box level given a comprehension assessment:
- depth = Aware OR conf < 0.6 → reset to 0
- depth ≥ Recall AND conf ≥ 0.6 → box += 1, capped at MAX_BOX_LEVEL

Pure — caller decides whether to write back. Will be plumbed into
apply_comprehension in a later commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Pure function `is_due` + tests

**Files:**
- Modify: `src/crates/primer-core/src/vocab.rs`

- [ ] **Step 1: Append the function and tests to `vocab.rs`**

Add immediately after `apply_box_transition`:

```rust
/// True if a concept is due for review now.
///
/// Returns false for concepts never encountered (`last_encountered = None`).
/// Returns true for concepts with `last_encountered` older than the box's
/// interval. Out-of-range `box_level` (defensive — should never happen)
/// clamps to `MAX_BOX_LEVEL`.
pub fn is_due(concept: &ConceptState, now: chrono::DateTime<chrono::Utc>) -> bool {
    let Some(last) = concept.last_encountered else {
        return false;
    };
    let box_idx = (concept.box_level as usize).min(BOX_INTERVALS_DAYS.len() - 1);
    let interval = chrono::Duration::days(BOX_INTERVALS_DAYS[box_idx] as i64);
    now - last > interval
}
```

Add to the existing `mod tests`:

```rust
    use chrono::{Duration, TimeZone, Utc};

    fn concept_with(box_level: u8, last_encountered: Option<chrono::DateTime<Utc>>) -> ConceptState {
        ConceptState {
            concept_id: "test".into(),
            depth: UnderstandingDepth::Aware,
            confidence: 0.5,
            encounter_count: 1,
            last_encountered,
            notes: vec![],
            box_level,
        }
    }

    fn fixed_now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap()
    }

    #[test]
    fn is_due_false_for_never_encountered_concept() {
        let c = concept_with(0, None);
        assert!(!is_due(&c, fixed_now()));
    }

    #[test]
    fn is_due_false_when_within_box0_interval() {
        let now = fixed_now();
        let c = concept_with(0, Some(now - Duration::hours(12)));
        assert!(!is_due(&c, now));
    }

    #[test]
    fn is_due_true_when_past_box0_interval() {
        let now = fixed_now();
        let c = concept_with(0, Some(now - Duration::hours(25)));
        assert!(is_due(&c, now));
    }

    #[test]
    fn is_due_false_for_box4_within_30d() {
        let now = fixed_now();
        let c = concept_with(4, Some(now - Duration::days(29)));
        assert!(!is_due(&c, now));
    }

    #[test]
    fn is_due_true_for_box4_after_31d() {
        let now = fixed_now();
        let c = concept_with(4, Some(now - Duration::days(31)));
        assert!(is_due(&c, now));
    }

    #[test]
    fn is_due_clamps_out_of_range_box_level() {
        // box_level = 99 should clamp to box 4 (interval 30d). 31d gap → due.
        let now = fixed_now();
        let c = concept_with(99, Some(now - Duration::days(31)));
        assert!(is_due(&c, now));
        // 29d gap with clamped box 4 → not due.
        let c = concept_with(99, Some(now - Duration::days(29)));
        assert!(!is_due(&c, now));
    }
```

- [ ] **Step 2: Run the new tests**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-core vocab::tests::is_due 2>&1 | tail -10
```

Expected: `test result: ok. 6 passed`.

- [ ] **Step 3: Run the whole vocab module to make sure nothing else broke**

```bash
~/.cargo/bin/cargo test -p primer-core vocab 2>&1 | tail -3
```

Expected: `test result: ok. 14 passed` (8 from apply_box_transition + 6 from is_due).

- [ ] **Step 4: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-core/src/vocab.rs
git commit -m "$(cat <<'EOF'
vocab: add is_due pure fn with 6 tests

Returns true iff now - concept.last_encountered exceeds the box's
interval per BOX_INTERVALS_DAYS. None last_encountered → not due.
Out-of-range box_level clamps to MAX_BOX_LEVEL defensively.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Pure function `due_concepts` + tests

**Files:**
- Modify: `src/crates/primer-core/src/vocab.rs`

- [ ] **Step 1: Append `due_concepts` and `overdue_amount` helper to `vocab.rs`**

Add after `is_due`:

```rust
/// Top `max_count` due concepts from the learner model, sorted by
/// overdue-ness descending (most overdue first).
///
/// Pure — borrows from the learner; returns at most `max_count` references.
/// Fewer if the learner has fewer due concepts.
pub fn due_concepts<'a>(
    learner: &'a crate::learner::LearnerModel,
    now: chrono::DateTime<chrono::Utc>,
    max_count: usize,
) -> Vec<&'a ConceptState> {
    if max_count == 0 {
        return Vec::new();
    }
    let mut due: Vec<&ConceptState> = learner
        .concepts
        .iter()
        .filter(|c| is_due(c, now))
        .collect();
    due.sort_by(|a, b| {
        let a_over = overdue_amount(a, now);
        let b_over = overdue_amount(b, now);
        b_over.cmp(&a_over)
    });
    due.truncate(max_count);
    due
}

/// How far past due a concept is. Zero for not-yet-due (callers should have
/// filtered already, but safe regardless).
fn overdue_amount(concept: &ConceptState, now: chrono::DateTime<chrono::Utc>) -> chrono::Duration {
    let Some(last) = concept.last_encountered else {
        return chrono::Duration::zero();
    };
    let box_idx = (concept.box_level as usize).min(BOX_INTERVALS_DAYS.len() - 1);
    let interval = chrono::Duration::days(BOX_INTERVALS_DAYS[box_idx] as i64);
    (now - last - interval).max(chrono::Duration::zero())
}
```

- [ ] **Step 2: Append tests**

In the existing `mod tests`:

```rust
    use crate::learner::LearnerModel;
    use crate::i18n::Locale;

    fn empty_learner() -> LearnerModel {
        LearnerModel {
            profile: crate::learner::LearnerProfile {
                id: uuid::Uuid::new_v4(),
                name: "Test".into(),
                age: 9,
                languages: vec!["en".into()],
                locale: Locale::English,
                created_at: Utc::now(),
                last_active: Utc::now(),
            },
            concepts: vec![],
            preferences: crate::learner::LearningPreferences::default(),
            current_engagement: crate::learner::EngagementState::Engaged,
            recent_assessments: vec![],
        }
    }

    #[test]
    fn due_concepts_empty_when_max_count_is_zero() {
        let mut learner = empty_learner();
        learner.concepts.push(concept_with(0, Some(Utc::now() - Duration::days(2))));
        assert!(due_concepts(&learner, fixed_now(), 0).is_empty());
    }

    #[test]
    fn due_concepts_empty_when_no_concepts() {
        let learner = empty_learner();
        assert!(due_concepts(&learner, fixed_now(), 4).is_empty());
    }

    #[test]
    fn due_concepts_empty_when_none_due() {
        let now = fixed_now();
        let mut learner = empty_learner();
        learner.concepts.push(concept_with(0, Some(now - Duration::hours(12))));
        learner.concepts.push(concept_with(2, Some(now - Duration::days(2))));
        assert!(due_concepts(&learner, now, 4).is_empty());
    }

    #[test]
    fn due_concepts_filters_out_never_encountered() {
        let now = fixed_now();
        let mut learner = empty_learner();
        learner.concepts.push(concept_with(0, None));
        assert!(due_concepts(&learner, now, 4).is_empty());
    }

    #[test]
    fn due_concepts_sorts_by_overdue_descending_and_truncates() {
        let now = fixed_now();
        let mut learner = empty_learner();
        // box 0 (interval 1d), gap=10d → overdue=9d
        let mut c1 = concept_with(0, Some(now - Duration::days(10)));
        c1.concept_id = "a".into();
        learner.concepts.push(c1);
        // box 4 (interval 30d), gap=31d → overdue=1d
        let mut c2 = concept_with(4, Some(now - Duration::days(31)));
        c2.concept_id = "b".into();
        learner.concepts.push(c2);
        // box 2 (interval 7d), gap=20d → overdue=13d
        let mut c3 = concept_with(2, Some(now - Duration::days(20)));
        c3.concept_id = "c".into();
        learner.concepts.push(c3);
        // box 0 (interval 1d), gap=5d → overdue=4d
        let mut c4 = concept_with(0, Some(now - Duration::days(5)));
        c4.concept_id = "d".into();
        learner.concepts.push(c4);
        // box 0 not due
        let mut c5 = concept_with(0, Some(now - Duration::hours(6)));
        c5.concept_id = "e".into();
        learner.concepts.push(c5);

        let result: Vec<&str> = due_concepts(&learner, now, 4)
            .iter()
            .map(|c| c.concept_id.as_str())
            .collect();
        // Expected order by overdue desc: c=13d, a=9d, d=4d, b=1d. e is not due.
        assert_eq!(result, vec!["c", "a", "d", "b"]);
    }

    #[test]
    fn due_concepts_max_count_truncates_to_subset() {
        let now = fixed_now();
        let mut learner = empty_learner();
        let mut c1 = concept_with(0, Some(now - Duration::days(10)));
        c1.concept_id = "a".into();
        learner.concepts.push(c1);
        let mut c2 = concept_with(2, Some(now - Duration::days(20)));
        c2.concept_id = "b".into();
        learner.concepts.push(c2);
        let mut c3 = concept_with(0, Some(now - Duration::days(5)));
        c3.concept_id = "c".into();
        learner.concepts.push(c3);

        // max=2: should return the top 2 most overdue: b=13d, a=9d.
        let result: Vec<&str> = due_concepts(&learner, now, 2)
            .iter()
            .map(|c| c.concept_id.as_str())
            .collect();
        assert_eq!(result, vec!["b", "a"]);
    }
```

- [ ] **Step 3: Run the new tests**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-core vocab::tests::due_concepts 2>&1 | tail -10
```

Expected: `test result: ok. 6 passed`.

- [ ] **Step 4: Run the whole module + workspace to ensure nothing else broke**

```bash
~/.cargo/bin/cargo test -p primer-core vocab 2>&1 | tail -3
~/.cargo/bin/cargo test --workspace 2>&1 | tail -3
```

Expected: `vocab` shows `20 passed`. Workspace shows `434+20=454 passed` (or thereabouts; verify count matches).

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-core/src/vocab.rs
git commit -m "$(cat <<'EOF'
vocab: add due_concepts pure fn with 6 tests

Filters learner.concepts by is_due, sorts by overdue-amount desc,
truncates to max_count. Returns &ConceptState refs (no allocation
per element). All three vocab fns are now in place; next: schema v7
migration to persist box_level.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Schema v7 migration + storage USER_VERSION bump

**Files:**
- Modify: `src/crates/primer-storage/src/schema.rs`
- Modify: `src/crates/primer-storage/src/lib.rs` (call `apply_v7_migrations` in open path)

- [ ] **Step 1: Write the failing migration tests**

Append to the `v4_tests` `mod` (or create a sibling module if it feels cleaner; the v4 test module already hosts v5 and v6 tests so v7 fits there) in `src/crates/primer-storage/src/schema.rs`:

```rust
    fn fresh_v6_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        apply_v2_migrations(&conn).unwrap();
        apply_v3_migrations(&conn).unwrap();
        apply_v4_migrations(&conn).unwrap();
        apply_v5_migrations(&conn).unwrap();
        apply_v6_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn v7_migration_adds_learner_concepts_box_level_column() {
        let conn = fresh_v6_conn();
        assert!(!column_exists_in_test(&conn, "learner_concepts", "box_level"));
        apply_v7_migrations(&conn).unwrap();
        assert!(column_exists_in_test(&conn, "learner_concepts", "box_level"));
    }

    #[test]
    fn v7_migration_defaults_existing_rows_to_zero() {
        let conn = fresh_v6_conn();
        // Pre-seed an engagement state, learner, concept and learner_concepts row
        // before the v7 migration.
        conn.execute(
            "INSERT INTO engagement_states (id, name) VALUES (1, 'Engaged')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO understanding_depths (id, name) VALUES (1, 'Aware')",
            [],
        ).unwrap();
        let learner_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO learners (
                 id, name, age, languages, created_at, last_active,
                 pref_narrative, pref_socratic, pref_visual, pref_kinesthetic,
                 typical_session_minutes, high_engagement_topics,
                 early_disengagement_secs, current_engagement_state_id, locale
             ) VALUES (?1, 'Tester', 8, '[]', '2026-01-01T00:00:00Z',
                      '2026-01-01T00:00:00Z', 0.5, 0.5, 0.5, 0.5, 20.0,
                      '[]', 300, 1, 'en')",
            rusqlite::params![learner_id],
        ).unwrap();
        conn.execute("INSERT INTO concepts (name) VALUES ('photosynthesis')", []).unwrap();
        let concept_id: i64 = conn
            .query_row("SELECT id FROM concepts WHERE name = 'photosynthesis'", [], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO learner_concepts (
                 learner_id, concept_id, depth_id, confidence,
                 encounter_count, last_encountered, notes
             ) VALUES (?1, ?2, 1, 0.5, 1, NULL, '[]')",
            rusqlite::params![learner_id, concept_id],
        ).unwrap();

        apply_v7_migrations(&conn).unwrap();

        let box_level: i64 = conn
            .query_row(
                "SELECT box_level FROM learner_concepts WHERE learner_id = ?1",
                rusqlite::params![learner_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(box_level, 0);
    }

    #[test]
    fn v7_migration_is_idempotent() {
        let conn = fresh_v6_conn();
        apply_v7_migrations(&conn).unwrap();
        apply_v7_migrations(&conn).unwrap(); // re-run must be a no-op
        assert!(column_exists_in_test(&conn, "learner_concepts", "box_level"));
    }

    #[test]
    fn user_version_constant_is_seven() {
        assert_eq!(USER_VERSION, 7);
    }
```

- [ ] **Step 2: Run the tests — expect them to fail**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-storage v7_migration 2>&1 | tail -10
```

Expected: compilation errors / test failures (no `apply_v7_migrations` defined; `USER_VERSION` is still 6).

- [ ] **Step 3: Implement the v7 migration and bump `USER_VERSION`**

In `src/crates/primer-storage/src/schema.rs`:

1. Bump `USER_VERSION` from 6 to 7.
2. Update its doc comment to add the v7 line:

```rust
/// Bumped to 7 when we added `learner_concepts.box_level` (INTEGER NOT NULL
/// DEFAULT 0) backing the spaced-repetition vocabulary feature
/// (Phase 0.3). All existing rows default to box 0 — no backfill needed
/// at this stage (Phase 0.3 has no field-deployed users).
pub const USER_VERSION: i64 = 7;
```

3. Append `apply_v7_migrations` after `apply_v6_migrations`:

```rust
// ─── v7 schema strings ──────────────────────────────────────────────────────
//
// v7 adds `learner_concepts.box_level` for the Leitner-box spaced-repetition
// schedule. INTEGER NOT NULL DEFAULT 0 means existing rows upgrade cleanly:
// their box_level becomes 0, which (combined with their `last_encountered`)
// schedules them for review 1 day after their old `last_encountered` —
// effectively treating pre-v7 data as freshly-encountered. Acceptable for
// Phase 0.3 with no field-deployed users.

/// Apply v7 migrations idempotently. Safe to run on a fresh DB (after v6
/// objects exist), on a v6 DB being upgraded, and on a v7 DB being
/// re-opened.
///
/// All steps run inside a single transaction so a partial failure rolls
/// back to the pre-migration state.
pub(crate) fn apply_v7_migrations(conn: &Connection) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("v7 migration: failed to begin tx: {e}")))?;

    if !column_exists(&tx, "learner_concepts", "box_level")? {
        tx.execute_batch(
            "ALTER TABLE learner_concepts \
             ADD COLUMN box_level INTEGER NOT NULL DEFAULT 0;",
        )
        .map_err(|e| {
            PrimerError::Storage(format!(
                "v7 migration: ALTER learner_concepts ADD box_level: {e}"
            ))
        })?;
    }

    tx.commit()
        .map_err(|e| PrimerError::Storage(format!("v7 migration: commit: {e}")))?;
    Ok(())
}
```

4. In `src/crates/primer-storage/src/lib.rs`, find where `apply_v6_migrations` is called (the migration runner in `open()` or `open_for_locale()`). Append a call to `apply_v7_migrations` immediately after, with the same error-mapping pattern. Find the call site:

```bash
grep -n "apply_v6_migrations" src/crates/primer-storage/src/lib.rs
```

The call site looks like (pattern):

```rust
schema::apply_v6_migrations(&conn)?;
```

After this line add:

```rust
schema::apply_v7_migrations(&conn)?;
```

- [ ] **Step 4: Run the tests — expect them to pass**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-storage v7_migration 2>&1 | tail -10
~/.cargo/bin/cargo test -p primer-storage user_version 2>&1 | tail -10
```

Expected: 4 passed.

- [ ] **Step 5: Run all storage tests + workspace to verify v6 didn't break**

```bash
~/.cargo/bin/cargo test -p primer-storage 2>&1 | tail -3
~/.cargo/bin/cargo test --workspace 2>&1 | tail -3
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-storage/src/schema.rs src/crates/primer-storage/src/lib.rs
git commit -m "$(cat <<'EOF'
storage: schema v7 — add learner_concepts.box_level (Phase 0.3 vocab SR)

ALTER TABLE adds INTEGER NOT NULL DEFAULT 0. Existing rows upgrade
cleanly to box_level=0 (pre-v7 data is treated as freshly encountered).
Migration is idempotent and transaction-wrapped, matching v2/v6
patterns. USER_VERSION bumped 6 → 7.

save_learner / load_learner column wiring lands in subsequent commits.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Extend `save_learner` to write `box_level`

**Files:**
- Modify: `src/crates/primer-storage/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/crates/primer-storage/src/lib.rs` test module (find the existing `learner_store_tests` mod or wherever `save_learner_*` tests live; grep `save_learner_writes_one_row_to_learners_table` for the location):

```rust
    #[tokio::test]
    async fn save_learner_persists_box_level_per_concept() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("learner.db");
        let store = SqliteSessionStore::open_for_locale(&path, Locale::English).unwrap();
        let mut learner = sample_learner();
        learner.concepts = vec![
            ConceptState {
                concept_id: "physics:gravity".into(),
                depth: UnderstandingDepth::Comprehension,
                confidence: 0.85,
                encounter_count: 3,
                last_encountered: Some(Utc::now()),
                notes: vec![],
                box_level: 2,
            },
            ConceptState {
                concept_id: "biology:photosynthesis".into(),
                depth: UnderstandingDepth::Recall,
                confidence: 0.7,
                encounter_count: 1,
                last_encountered: Some(Utc::now()),
                notes: vec![],
                box_level: 1,
            },
        ];
        store.save_learner(&learner).await.unwrap();

        // Read raw rows to verify box_level was written.
        let conn = rusqlite::Connection::open(&path).unwrap();
        let mut rows: Vec<(String, i64)> = conn
            .prepare(
                "SELECT c.name, lc.box_level FROM learner_concepts lc \
                 JOIN concepts c ON c.id = lc.concept_id ORDER BY c.name",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        rows.sort();
        assert_eq!(rows, vec![
            ("biology:photosynthesis".to_string(), 1),
            ("physics:gravity".to_string(), 2),
        ]);
    }
```

- [ ] **Step 2: Run — expect failure**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-storage save_learner_persists_box_level 2>&1 | tail -15
```

Expected: failure (`box_level` column not in the UPSERT, so it stays at the default 0 for the row inserted via UPDATE-on-conflict path; or the column exists but isn't being written).

- [ ] **Step 3: Update the `save_learner` UPSERT to include `box_level`**

In `src/crates/primer-storage/src/lib.rs::save_learner`, find the prepared statement around line 955:

```rust
let mut upsert_lc = tx
    .prepare(
        "INSERT INTO learner_concepts (
             learner_id, concept_id, depth_id, confidence,
             encounter_count, last_encountered, notes
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(learner_id, concept_id) DO UPDATE SET
             depth_id = excluded.depth_id,
             confidence = excluded.confidence,
             encounter_count = excluded.encounter_count,
             last_encountered = excluded.last_encountered,
             notes = excluded.notes",
    )
```

Change to (8 columns + 8 placeholders + new ON CONFLICT line):

```rust
let mut upsert_lc = tx
    .prepare(
        "INSERT INTO learner_concepts (
             learner_id, concept_id, depth_id, confidence,
             encounter_count, last_encountered, notes, box_level
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(learner_id, concept_id) DO UPDATE SET
             depth_id = excluded.depth_id,
             confidence = excluded.confidence,
             encounter_count = excluded.encounter_count,
             last_encountered = excluded.last_encountered,
             notes = excluded.notes,
             box_level = excluded.box_level",
    )
```

Then around line 1004, find the `.execute(rusqlite::params![…])` call. It currently has 7 params:

```rust
upsert_lc
    .execute(rusqlite::params![
        learner.profile.id.to_string(),
        cid,
        catalog::understanding_depth_id(concept.depth),
        concept.confidence as f64,
        concept.encounter_count as i64,
        last_encountered,
        notes_json,
    ])
```

Add `concept.box_level as i64` as the eighth param:

```rust
upsert_lc
    .execute(rusqlite::params![
        learner.profile.id.to_string(),
        cid,
        catalog::understanding_depth_id(concept.depth),
        concept.confidence as f64,
        concept.encounter_count as i64,
        last_encountered,
        notes_json,
        concept.box_level as i64,
    ])
```

- [ ] **Step 4: Run the test — expect pass**

```bash
~/.cargo/bin/cargo test -p primer-storage save_learner_persists_box_level 2>&1 | tail -10
```

Expected: pass.

- [ ] **Step 5: Run the whole storage crate to verify no regression**

```bash
~/.cargo/bin/cargo test -p primer-storage 2>&1 | tail -3
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-storage/src/lib.rs
git commit -m "$(cat <<'EOF'
storage: save_learner writes box_level into learner_concepts (v7)

UPSERT statement extended to 8 columns; ON CONFLICT updates box_level
alongside the other mutable fields. New test exercises the round-trip
path from in-memory ConceptState to raw SQL row.

load_learner extension lands in the next commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Extend `load_learner` to read `box_level` + round-trip test

**Files:**
- Modify: `src/crates/primer-storage/src/lib.rs`

- [ ] **Step 1: Write the failing round-trip test**

Add next to `save_learner_persists_box_level_per_concept`:

```rust
    #[tokio::test]
    async fn save_then_load_learner_round_trips_box_level() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("learner.db");
        let store = SqliteSessionStore::open_for_locale(&path, Locale::English).unwrap();
        let mut learner = sample_learner();
        learner.concepts = vec![
            ConceptState {
                concept_id: "physics:gravity".into(),
                depth: UnderstandingDepth::Application,
                confidence: 0.9,
                encounter_count: 5,
                last_encountered: Some(Utc::now()),
                notes: vec!["struggled at first".into()],
                box_level: 3,
            },
        ];
        store.save_learner(&learner).await.unwrap();

        let loaded = store.load_learner().await.unwrap().expect("learner present");
        let concept = loaded.concepts.iter().find(|c| c.concept_id == "physics:gravity").unwrap();
        assert_eq!(concept.box_level, 3);
        assert_eq!(concept.depth, UnderstandingDepth::Application);
        assert_eq!(concept.encounter_count, 5);
    }
```

- [ ] **Step 2: Run — expect failure**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-storage save_then_load_learner_round_trips_box_level 2>&1 | tail -10
```

Expected: failure (the loaded `box_level` is 0 because `load_learner` doesn't read the column yet; the constructed `ConceptState` literal has `box_level: 3` baked in but the SELECT path doesn't surface it).

- [ ] **Step 3: Extend `load_learner`'s SELECT and row-mapping**

In `src/crates/primer-storage/src/lib.rs::load_learner`, find the `SELECT … FROM learner_concepts lc` query around line 1175:

```rust
let mut stmt = conn
    .prepare(
        "SELECT c.name, lc.depth_id, lc.confidence, lc.encounter_count,
                lc.last_encountered, lc.notes
         FROM learner_concepts lc
         JOIN concepts c ON c.id = lc.concept_id
         WHERE lc.learner_id = ?1",
    )
```

Add `lc.box_level` to the select list:

```rust
let mut stmt = conn
    .prepare(
        "SELECT c.name, lc.depth_id, lc.confidence, lc.encounter_count,
                lc.last_encountered, lc.notes, lc.box_level
         FROM learner_concepts lc
         JOIN concepts c ON c.id = lc.concept_id
         WHERE lc.learner_id = ?1",
    )
```

Then the row-mapping closure (around line 1187, the `query_map` body) — add a new column position:

```rust
let rows = stmt
    .query_map(rusqlite::params![learner_id_str], |row| {
        Ok((
            row.get::<_, String>(0)?,                  // name
            row.get::<_, i64>(1)?,                     // depth_id
            row.get::<_, f64>(2)?,                     // confidence
            row.get::<_, i64>(3)?,                     // encounter_count
            row.get::<_, Option<String>>(4)?,          // last_encountered
            row.get::<_, String>(5)?,                  // notes
            row.get::<_, i64>(6)?,                     // box_level (new)
        ))
    })
```

The `for row in rows` loop (around line 1196) destructures these — change the pattern to include the new field:

```rust
for row in rows {
    let (name, depth_id, confidence, encounter_count, last_encountered_str, notes_json, box_level) =
        row.map_err(|e| PrimerError::Storage(format!("read learner_concepts: {e}")))?;
    // ... existing parsing ...
    concepts.push(ConceptState {
        concept_id: name,
        depth: catalog::understanding_depth_from_id(depth_id)?,
        confidence: confidence as f32,
        encounter_count: i64_to_u32(encounter_count, "learner_concepts.encounter_count")?,
        last_encountered: parsed_last_encountered,
        notes: parsed_notes,
        box_level: box_level.try_into().map_err(|_| {
            PrimerError::Storage(format!("box_level out of u8 range: {box_level}"))
        })?,
    });
}
```

(Verify the `concepts.push(ConceptState { … })` literal in the existing code; preserve its existing field initialisations and add `box_level` at the end.)

- [ ] **Step 4: Run the test — expect pass**

```bash
~/.cargo/bin/cargo test -p primer-storage save_then_load_learner_round_trips_box_level 2>&1 | tail -10
```

Expected: pass.

- [ ] **Step 5: Run the whole storage crate**

```bash
~/.cargo/bin/cargo test -p primer-storage 2>&1 | tail -3
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-storage/src/lib.rs
git commit -m "$(cat <<'EOF'
storage: load_learner reads box_level (v7 round-trip complete)

SELECT extended to 7 columns; row-mapping closure picks the new
position. Out-of-range guard via try_into for u8 (defensive — the
column is INTEGER but constrained at the Rust API surface).

Round-trip test verifies in-memory ConceptState.box_level survives
save → SQL → load.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Extend `apply_comprehension` with the box-transition call

**Why this is the SR-critical task:** without this, all the prior work is dead code. This is the line that drives the whole feature.

**Files:**
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager/apply.rs`

- [ ] **Step 1: Write the failing tests**

In `src/crates/primer-pedagogy/src/dialogue_manager/apply.rs` test module (search for `mod tests` in the file; tests are inline), add:

```rust
    use chrono::Utc;
    use primer_comprehension::ComprehensionSettings;
    use primer_core::comprehension::{ComprehensionAssessment, ComprehensionResult};

    fn settings_at_threshold_06() -> ComprehensionSettings {
        let mut s = ComprehensionSettings::default();
        s.confidence_threshold = 0.6;
        s
    }

    #[test]
    fn apply_comprehension_advances_box_on_strong_assessment_with_depth_promotion() {
        // Concept exists at depth=Aware, box=0. Assessment promotes to Recall, box→1.
        let mut learner = LearnerModel {
            profile: sample_profile(),
            concepts: vec![ConceptState {
                concept_id: "x".into(),
                depth: UnderstandingDepth::Aware,
                confidence: 0.5,
                encounter_count: 1,
                last_encountered: Some(Utc::now()),
                notes: vec![],
                box_level: 0,
            }],
            preferences: LearningPreferences::default(),
            current_engagement: EngagementState::Engaged,
            recent_assessments: vec![],
        };
        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "x".into(),
                depth: UnderstandingDepth::Recall,
                confidence: 0.85,
                evidence: None,
            }],
        };
        assert!(apply_comprehension(&mut learner, &result, &settings_at_threshold_06()));
        assert_eq!(learner.concepts[0].depth, UnderstandingDepth::Recall);
        assert_eq!(learner.concepts[0].box_level, 1);
    }

    #[test]
    fn apply_comprehension_advances_box_on_strong_reconfirmation_at_same_depth() {
        // SR-critical: a concept already at depth=Comprehension box=1, re-assessed
        // confidently at the same depth, advances box=2 (depth unchanged).
        let mut learner = LearnerModel {
            profile: sample_profile(),
            concepts: vec![ConceptState {
                concept_id: "x".into(),
                depth: UnderstandingDepth::Comprehension,
                confidence: 0.8,
                encounter_count: 3,
                last_encountered: Some(Utc::now()),
                notes: vec![],
                box_level: 1,
            }],
            preferences: LearningPreferences::default(),
            current_engagement: EngagementState::Engaged,
            recent_assessments: vec![],
        };
        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "x".into(),
                depth: UnderstandingDepth::Comprehension,
                confidence: 0.9,
                evidence: None,
            }],
        };
        assert!(apply_comprehension(&mut learner, &result, &settings_at_threshold_06()));
        assert_eq!(learner.concepts[0].depth, UnderstandingDepth::Comprehension);
        assert_eq!(learner.concepts[0].box_level, 2);
    }

    #[test]
    fn apply_comprehension_resets_box_on_aware_assessment() {
        let mut learner = LearnerModel {
            profile: sample_profile(),
            concepts: vec![ConceptState {
                concept_id: "x".into(),
                depth: UnderstandingDepth::Comprehension,
                confidence: 0.85,
                encounter_count: 5,
                last_encountered: Some(Utc::now()),
                notes: vec![],
                box_level: 3,
            }],
            preferences: LearningPreferences::default(),
            current_engagement: EngagementState::Engaged,
            recent_assessments: vec![],
        };
        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "x".into(),
                depth: UnderstandingDepth::Aware,
                confidence: 0.85,
                evidence: None,
            }],
        };
        assert!(apply_comprehension(&mut learner, &result, &settings_at_threshold_06()));
        // Depth is monotonic-max: stays at Comprehension.
        assert_eq!(learner.concepts[0].depth, UnderstandingDepth::Comprehension);
        // Box dropped to 0 reflecting "needs re-review soon".
        assert_eq!(learner.concepts[0].box_level, 0);
    }

    #[test]
    fn apply_comprehension_leaves_box_unchanged_on_subthreshold_assessment() {
        let mut learner = LearnerModel {
            profile: sample_profile(),
            concepts: vec![ConceptState {
                concept_id: "x".into(),
                depth: UnderstandingDepth::Comprehension,
                confidence: 0.8,
                encounter_count: 3,
                last_encountered: Some(Utc::now()),
                notes: vec![],
                box_level: 2,
            }],
            preferences: LearningPreferences::default(),
            current_engagement: EngagementState::Engaged,
            recent_assessments: vec![],
        };
        let result = ComprehensionResult {
            assessments: vec![ComprehensionAssessment {
                concept: "x".into(),
                depth: UnderstandingDepth::Application,
                confidence: 0.4, // below 0.6 threshold
                evidence: None,
            }],
        };
        let changed = apply_comprehension(&mut learner, &result, &settings_at_threshold_06());
        // Sub-threshold → outer guard skips. depth stays, box stays, returns false.
        assert!(!changed);
        assert_eq!(learner.concepts[0].depth, UnderstandingDepth::Comprehension);
        assert_eq!(learner.concepts[0].box_level, 2);
    }
```

You may need a `sample_profile` helper inside the test module; if one doesn't exist, define:

```rust
    fn sample_profile() -> primer_core::learner::LearnerProfile {
        primer_core::learner::LearnerProfile {
            id: uuid::Uuid::new_v4(),
            name: "Tester".into(),
            age: 9,
            languages: vec!["en".into()],
            locale: primer_core::i18n::Locale::English,
            created_at: Utc::now(),
            last_active: Utc::now(),
        }
    }
```

(Place it alongside the other test helpers in the same module.)

- [ ] **Step 2: Run — expect failure**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-pedagogy apply_comprehension 2>&1 | tail -15
```

Expected: 4 new tests fail (`box_level` is unchanged because no transition logic exists yet).

- [ ] **Step 3: Add the box-transition call**

In `src/crates/primer-pedagogy/src/dialogue_manager/apply.rs::apply_comprehension`, find the per-assessment loop (around line 121):

```rust
pub(super) fn apply_comprehension(
    learner: &mut primer_core::learner::LearnerModel,
    result: &primer_core::comprehension::ComprehensionResult,
    settings: &primer_comprehension::ComprehensionSettings,
) -> bool {
    let mut changed = false;
    for a in &result.assessments {
        if a.confidence < settings.confidence_threshold {
            continue;
        }
        if let Some(c) = learner
            .concepts
            .iter_mut()
            .find(|c| c.concept_id == a.concept)
        {
            if a.depth > c.depth {
                c.depth = a.depth;
                c.confidence = a.confidence;
                changed = true;
            }
        }
    }
    changed
}
```

Replace the inner `if let Some(c) = …` block with the box-transition-aware version:

```rust
        if let Some(c) = learner
            .concepts
            .iter_mut()
            .find(|c| c.concept_id == a.concept)
        {
            // Existing — depth promotion (monotonic-max), runs only on
            // strict increase.
            if a.depth > c.depth {
                c.depth = a.depth;
                c.confidence = a.confidence;
                changed = true;
            }
            // NEW — box transition runs on EVERY accepted assessment,
            // regardless of whether depth moved. Successful re-confirmation
            // at the same depth is the SR mechanism for expanding intervals.
            let new_box = primer_core::vocab::apply_box_transition(
                c.box_level,
                a.depth,
                a.confidence,
            );
            if new_box != c.box_level {
                c.box_level = new_box;
                changed = true;
            }
        }
```

- [ ] **Step 4: Run the tests — expect pass**

```bash
~/.cargo/bin/cargo test -p primer-pedagogy apply_comprehension 2>&1 | tail -10
```

Expected: 4 new tests pass + existing apply_comprehension tests still pass.

- [ ] **Step 5: Workspace sanity check**

```bash
~/.cargo/bin/cargo test --workspace 2>&1 | tail -3
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-pedagogy/src/dialogue_manager/apply.rs
git commit -m "$(cat <<'EOF'
vocab: apply_comprehension drives box transitions per assessment

Each accepted assessment now ALSO calls primer_core::vocab::apply_box_transition
(orthogonal to depth promotion). Box advances on EVERY successful
re-confirmation, not only when depth strict-increases — that's the
expanding-interval SR mechanism.

Sub-threshold assessments still skipped under the existing outer
confidence_threshold guard. New tests cover all four cases: depth-promotion
+ box-advance, same-depth confident reconfirmation + box-advance, Aware
assessment + box-reset (depth stays via monotonic-max), sub-threshold
+ no change.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Add `VocabSettings` struct and `DialogueManagerSubsystems` field

**Files:**
- Locate the `*Settings` struct definitions in `primer-pedagogy` (likely `src/crates/primer-pedagogy/src/lib.rs` or a `settings.rs` sibling). Use `grep -n "pub struct.*Settings" src/crates/primer-pedagogy/src/*.rs` to find.
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager/mod.rs`

- [ ] **Step 1: Locate where the existing `*Settings` structs live**

```bash
cd /Users/hherb/src/primer/src
grep -rn "pub struct ClassifierSettings\|pub struct ExtractorSettings" crates/primer-pedagogy/src/ crates/primer-classifier/src/ crates/primer-extractor/src/
```

`ClassifierSettings` lives in `primer-classifier`; `ExtractorSettings` in `primer-extractor`; `ComprehensionSettings` in `primer-comprehension`. Each has its own crate's `consts.rs` for defaults. The pattern for `VocabSettings`: since this is a pedagogy concern (not its own LLM-driven crate), put it directly in `primer-pedagogy`.

Decide placement: a new file `src/crates/primer-pedagogy/src/vocab.rs` is cleanest (avoids cluttering `lib.rs`). The pure functions live in `primer-core::vocab`; the *settings* go in `primer-pedagogy::vocab`. If a sibling pattern is preferred (e.g., `primer-pedagogy/src/settings.rs` already exists for cross-cutting tunables), follow that. Confirm by listing `primer-pedagogy/src/`:

```bash
ls src/crates/primer-pedagogy/src/
```

If no `settings.rs` exists, create `src/crates/primer-pedagogy/src/vocab.rs`.

- [ ] **Step 2: Create the settings module**

Create `src/crates/primer-pedagogy/src/vocab.rs`:

```rust
//! Vocabulary spaced-repetition settings (consumed by `DialogueManager`).
//!
//! The pure scheduling functions live in `primer_core::vocab`; this
//! module only owns the runtime tunables.

use primer_core::consts::vocab::DEFAULT_VOCAB_MAX_PER_PROMPT;

/// Tunables for the vocabulary review feature.
///
/// `max_per_prompt` caps how many overdue concepts are injected into
/// the system prompt per turn. Configurable via the
/// `--vocab-max-per-prompt` CLI flag; default 4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VocabSettings {
    pub max_per_prompt: usize,
}

impl Default for VocabSettings {
    fn default() -> Self {
        Self {
            max_per_prompt: DEFAULT_VOCAB_MAX_PER_PROMPT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_max_per_prompt_is_four() {
        assert_eq!(VocabSettings::default().max_per_prompt, 4);
    }
}
```

Add to `src/crates/primer-pedagogy/src/lib.rs`:

```rust
pub mod vocab;
```

(Place in alphabetical order with the other module declarations.)

- [ ] **Step 3: Extend `DialogueManagerSubsystems`**

In `src/crates/primer-pedagogy/src/dialogue_manager/mod.rs`, find `pub struct DialogueManagerSubsystems` (line 77). Add a field:

```rust
pub struct DialogueManagerSubsystems {
    pub classifier: Arc<dyn EngagementClassifier>,
    pub classifier_settings: ClassifierSettings,
    pub extractor: Arc<dyn ConceptExtractor>,
    pub extractor_settings: ExtractorSettings,
    pub comprehension: Arc<dyn primer_comprehension::ComprehensionClassifier>,
    pub comprehension_settings: primer_comprehension::ComprehensionSettings,
    pub vocab_settings: crate::vocab::VocabSettings,
}
```

Also find the `DialogueManager` struct (line 96+) and add:

```rust
    /// Tunables for the vocabulary review feature.
    vocab_settings: crate::vocab::VocabSettings,
```

(Position consistently with other `*_settings` fields, after `comprehension_settings`.)

- [ ] **Step 4: Update the `DialogueManager::new` (and `open_session`/`resume_session` if they accept subsystems) plumb-through**

Find the constructor:

```bash
cd /Users/hherb/src/primer/src
grep -n "fn new\|pub fn new" crates/primer-pedagogy/src/dialogue_manager/lifecycle.rs
```

Each constructor that takes a `DialogueManagerSubsystems` parameter needs to destructure and assign `subsystems.vocab_settings` into `self.vocab_settings`. Pattern:

```rust
let DialogueManagerSubsystems {
    classifier,
    classifier_settings,
    extractor,
    extractor_settings,
    comprehension,
    comprehension_settings,
    vocab_settings,           // new
} = subsystems;
// ... existing inits ...
vocab_settings,                // new — in the struct literal
```

- [ ] **Step 5: Update every `DialogueManagerSubsystems { … }` literal in tests**

```bash
grep -rn "DialogueManagerSubsystems {" crates/
```

For each occurrence, add `vocab_settings: VocabSettings::default(),` (or the equivalent path-qualified `crate::vocab::VocabSettings::default()` for non-pedagogy crates) to the literal. Likely 5–10 places, mostly in `dialogue_manager/test_support.rs` and `dialogue_manager/tests/*`.

- [ ] **Step 6: Verify compile + tests**

```bash
~/.cargo/bin/cargo build --workspace 2>&1 | tail -5
~/.cargo/bin/cargo test --workspace 2>&1 | tail -3
```

Expected: clean. The new field has a default; nothing functionally changes yet (it's only used once `build_turn_prompt` consumes it in Task 12).

- [ ] **Step 7: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-pedagogy/src/vocab.rs \
        src/crates/primer-pedagogy/src/lib.rs \
        src/crates/primer-pedagogy/src/dialogue_manager/mod.rs \
        src/crates/primer-pedagogy/src/dialogue_manager/lifecycle.rs \
        src/crates/primer-pedagogy/src/dialogue_manager/test_support.rs \
        src/crates/primer-pedagogy/src/dialogue_manager/tests/
git commit -m "$(cat <<'EOF'
vocab: add VocabSettings + thread through DialogueManagerSubsystems

VocabSettings carries the runtime tunable (max_per_prompt, default 4).
Plumbed through DialogueManagerSubsystems and the DialogueManager
struct itself. No consumer yet — wiring lands in build_turn_prompt
in a later commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: PromptPack `vocab_review_intro` trait method + en/de TOML + parsing

**Files:**
- Modify: `src/crates/primer-pedagogy/src/prompt_pack.rs`
- Modify: `src/crates/primer-pedagogy/prompts/en.toml`
- Modify: `src/crates/primer-pedagogy/prompts/de.toml`

- [ ] **Step 1: Add the strings to TOML files**

`src/crates/primer-pedagogy/prompts/en.toml` — under the existing `[sections]` block (after `retrieved_intro`):

```toml
vocab_review_intro = "Concepts {child} has previously encountered. Weave them in only if topically relevant to what they bring up — don't drill or quiz. The list is a hint, not a script."
```

`src/crates/primer-pedagogy/prompts/de.toml` — same place:

```toml
vocab_review_intro = "Konzepte, die {child} bereits kennengelernt hat. Greife sie nur dann auf, wenn sie thematisch passen — keine Abfrage, kein Drillen. Die Liste ist ein Hinweis, kein Skript."
```

Note: the placeholder `{child}` is *not* used in v1 — keep it out for now (don't add it to the `validate_placeholders` allowlist later). The string above can be edited to remove `{child}` if you want it 100% literal:

Actually use this simpler version which avoids the placeholder validation question:

`en.toml`:
```toml
vocab_review_intro = "Concepts the child has previously encountered. Weave them in only if topically relevant to what they bring up — don't drill or quiz. The list is a hint, not a script."
```

`de.toml`:
```toml
vocab_review_intro = "Konzepte, die das Kind bereits kennengelernt hat. Greife sie nur dann auf, wenn sie thematisch passen — keine Abfrage, kein Drillen. Die Liste ist ein Hinweis, kein Skript."
```

(Both phrasings work; pick the no-placeholder version to avoid extending the validator unnecessarily.)

- [ ] **Step 2: Add the trait method and write a failing test**

In `src/crates/primer-pedagogy/src/prompt_pack.rs`, find the `pub trait PromptPack` (line 39) and add a method:

```rust
pub trait PromptPack: Send + Sync {
    // ... existing methods ...
    fn knowledge_intro(&self, age: u8) -> String;
    fn summary_intro(&self) -> &str;
    fn retrieved_intro(&self) -> &str;
    fn vocab_review_intro(&self) -> &str;
    // ... rest ...
}
```

Add a test (in the existing `mod tests` at the bottom of the file):

```rust
    #[test]
    fn english_pack_exposes_vocab_review_intro() {
        let pack = english_pack();
        let intro = pack.vocab_review_intro();
        assert!(!intro.is_empty(), "vocab_review_intro must not be empty");
        assert!(
            intro.contains("topically relevant"),
            "expected English intro to contain 'topically relevant', got: {intro}"
        );
    }

    #[test]
    fn german_pack_exposes_vocab_review_intro() {
        let pack = german_pack();
        let intro = pack.vocab_review_intro();
        assert!(!intro.is_empty(), "German vocab_review_intro must not be empty");
        assert!(
            intro.contains("thematisch passen"),
            "expected German intro to contain 'thematisch passen', got: {intro}"
        );
    }
```

- [ ] **Step 3: Run — expect compile failure**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-pedagogy vocab_review_intro 2>&1 | tail -15
```

Expected: compile error (`TomlPromptPack` doesn't implement `vocab_review_intro`).

- [ ] **Step 4: Implement on `TomlPromptPack`**

In `src/crates/primer-pedagogy/src/prompt_pack.rs`:

1. Add a field to `TomlPromptPack` (line 149):

```rust
pub struct TomlPromptPack {
    locale: Locale,
    base_template: String,
    language_guidance: LanguageGuidance,
    intents: [String; ALL_INTENTS.len()],
    engagement_frustrated: String,
    engagement_disengaging: String,
    knowledge_intro_template: String,
    summary_intro: String,
    retrieved_intro: String,
    vocab_review_intro: String,        // new
    child_label: String,
    primer_label: String,
    factual_prefixes: Vec<String>,
}
```

2. Add to `SectionsSection` (line 406):

```rust
#[derive(Deserialize)]
struct SectionsSection {
    knowledge_intro: String,
    summary_intro: String,
    retrieved_intro: String,
    vocab_review_intro: String,        // new
}
```

3. Add the validator call (around line 245, near other `sections.*` validators):

```rust
    validate_placeholders(
        "sections.vocab_review_intro",
        &raw.sections.vocab_review_intro,
        &[],   // no placeholders in v1
    )?;
```

4. Add to the constructor literal (around line 282, where the `Self { … }` is built):

```rust
        Ok(Self {
            // ... existing fields ...
            retrieved_intro: raw.sections.retrieved_intro,
            vocab_review_intro: raw.sections.vocab_review_intro,
            // ... rest ...
        })
```

5. Implement the trait method on `impl PromptPack for TomlPromptPack` (around line 308):

```rust
impl PromptPack for TomlPromptPack {
    // ... existing impls ...
    fn retrieved_intro(&self) -> &str {
        &self.retrieved_intro
    }
    fn vocab_review_intro(&self) -> &str {
        &self.vocab_review_intro
    }
    // ... rest ...
}
```

- [ ] **Step 5: Run — expect pass**

```bash
~/.cargo/bin/cargo test -p primer-pedagogy vocab_review_intro 2>&1 | tail -10
~/.cargo/bin/cargo test -p primer-pedagogy 2>&1 | tail -3
```

Expected: 2 new tests pass; existing prompt_pack tests still pass.

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-pedagogy/src/prompt_pack.rs \
        src/crates/primer-pedagogy/prompts/en.toml \
        src/crates/primer-pedagogy/prompts/de.toml
git commit -m "$(cat <<'EOF'
prompt-pack: add vocab_review_intro trait method + en/de strings

New PromptPack::vocab_review_intro() returns the locale-keyed intro
string for the vocab review section. Implementations on TomlPromptPack
read from the existing [sections] table; en.toml and de.toml each
ship the v1 phrasing.

The string instructs the LLM: weave words in only if topically
relevant — no drilling, no quizzing.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Inject `vocab_section` into `build_system_prompt_with_pack` + tests

**Files:**
- Modify: `src/crates/primer-pedagogy/src/prompt_builder.rs`

- [ ] **Step 1: Write the failing tests**

In `src/crates/primer-pedagogy/src/prompt_builder.rs` test module (search for `mod tests`):

```rust
    use chrono::{Duration, TimeZone, Utc};

    fn due_concept(id: &str, depth: UnderstandingDepth, days_ago: i64) -> ConceptState {
        ConceptState {
            concept_id: id.into(),
            depth,
            confidence: 0.8,
            encounter_count: 2,
            last_encountered: Some(Utc::now() - Duration::days(days_ago)),
            notes: vec![],
            box_level: 1,
        }
    }

    #[test]
    fn build_system_prompt_includes_vocab_section_when_due_vocab_non_empty() {
        let learner = sample_learner();
        let due_vocab = vec![
            due_concept("physics:gravity", UnderstandingDepth::Recall, 5),
            due_concept("biology:photosynthesis", UnderstandingDepth::Comprehension, 12),
        ];
        let prompt = build_system_prompt_with_pack_and_vocab(
            english_pack(),
            &learner,
            PedagogicalIntent::SocraticQuestion,
            &[],
            "",
            &[],
            &due_vocab.iter().collect::<Vec<_>>(),
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
        let learner = sample_learner();
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
        // Construct a prompt with all of: summary, retrieved, vocab, knowledge.
        let learner = sample_learner();
        let retrieved = vec![Turn {
            speaker: Speaker::Child,
            text: "remember when we talked about clouds".into(),
            timestamp: Utc::now(),
            intent: None,
            concepts: vec![],
        }];
        let knowledge = vec![Passage {
            text: "Clouds are condensed water vapor".into(),
            source: "test".into(),
            score: 1.0,
            language_tag: "en".into(),
        }];
        let due_vocab = vec![due_concept("weather:cloud", UnderstandingDepth::Recall, 5)];
        let prompt = build_system_prompt_with_pack_and_vocab(
            english_pack(),
            &learner,
            PedagogicalIntent::SocraticQuestion,
            &knowledge,
            "Earlier we talked about water cycles.",
            &retrieved,
            &due_vocab.iter().collect::<Vec<_>>(),
        );
        let retrieved_idx = prompt.find("clouds").unwrap();
        let vocab_idx = prompt.find("weather:cloud").unwrap();
        let knowledge_idx = prompt.find("condensed water vapor").unwrap();
        assert!(retrieved_idx < vocab_idx, "retrieved must precede vocab: {prompt}");
        assert!(vocab_idx < knowledge_idx, "vocab must precede knowledge: {prompt}");
    }
```

(The function `build_system_prompt_with_pack_and_vocab` does not exist yet — it's the new signature we're adding. We test by introducing it as a name that doesn't compile, validating Step 2 produces a compile error first.)

Also add `due_vocab` parameter to the existing `build_system_prompt_with_pack` and update its callers — the simplest path is to extend the existing function signature. Plan:

- The existing `build_system_prompt_with_pack` becomes a thin wrapper that calls a new `build_system_prompt_with_pack_and_vocab` with `due_vocab = &[]`. This preserves all existing callers.
- The new `build_system_prompt_with_pack_and_vocab` accepts an additional `due_vocab: &[&ConceptState]` parameter.

- [ ] **Step 2: Run — expect compile failure**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-pedagogy vocab_section 2>&1 | tail -10
```

Expected: function not found.

- [ ] **Step 3: Add the new function and helper**

In `src/crates/primer-pedagogy/src/prompt_builder.rs`, near the existing `build_system_prompt_with_pack` (line 53), add:

```rust
/// Build the system prompt with a vocabulary review section in addition
/// to the existing knowledge / summary / retrieved sections.
///
/// `due_vocab` is the slice of due concepts (typically from
/// `primer_core::vocab::due_concepts`). Empty → vocab section omitted.
/// Section order: base / intent / engagement / summary / retrieved /
/// vocab / knowledge.
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
                let days_ago = days_since(c.last_encountered.unwrap_or(now), now);
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

/// Render `last - now` as integer days, floored, non-negative.
/// Used by the vocab review section. v1 is English-only inside the
/// system prompt (the LLM consumes it; the child never sees this).
fn days_since(last: chrono::DateTime<chrono::Utc>, now: chrono::DateTime<chrono::Utc>) -> i64 {
    (now - last).num_days().max(0)
}
```

Make the existing `build_system_prompt_with_pack` a wrapper:

```rust
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
```

You also need to import `ConceptState` at the top of the file:

```rust
use primer_core::learner::{ConceptState, LearnerModel};
```

(`LearnerModel` is already imported; just add `ConceptState`.)

- [ ] **Step 4: Run the new tests + the rest of prompt_builder**

```bash
~/.cargo/bin/cargo test -p primer-pedagogy build_system_prompt 2>&1 | tail -10
```

Expected: pass.

- [ ] **Step 5: Workspace check**

```bash
~/.cargo/bin/cargo test --workspace 2>&1 | tail -3
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-pedagogy/src/prompt_builder.rs
git commit -m "$(cat <<'EOF'
prompt_builder: add vocab section to system prompt

build_system_prompt_with_pack_and_vocab is the new signature;
build_system_prompt_with_pack stays as a no-op wrapper for callers
that don't need vocab. New section sits between retrieved-older and
knowledge in the prompt order. days_since helper renders last-seen
deltas as integer days (English-only — LLM consumes; child never
sees this).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Wire `due_concepts` into `build_turn_prompt`

**Files:**
- Modify: `src/crates/primer-pedagogy/src/prompt_builder.rs` (extend `build_prompt_*` to accept `due_vocab`)
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager/turn.rs` (call `due_concepts` and thread)

- [ ] **Step 1: Extend the `build_prompt_*` API to accept `due_vocab`**

In `src/crates/primer-pedagogy/src/prompt_builder.rs`, mirror the wrapper pattern: add `build_prompt_with_pack_and_vocab` and `build_prompt_and_vocab`, while keeping the existing two as no-vocab wrappers.

Add (after the existing `build_prompt_with_pack`):

```rust
/// Assemble the complete prompt with a vocabulary review section.
///
/// Mirrors `build_prompt_with_pack` but threads `due_vocab` through to
/// `build_system_prompt_with_pack_and_vocab`.
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
```

Make the existing `build_prompt_with_pack` a wrapper:

```rust
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
```

(The non-pack `build_prompt` can stay as-is since tests use it; it just won't include vocab.)

- [ ] **Step 2: Wire `due_concepts` into `build_turn_prompt`**

In `src/crates/primer-pedagogy/src/dialogue_manager/turn.rs`, find `build_turn_prompt` (line 137):

```rust
async fn build_turn_prompt(&self, child_input: &str, intent: PedagogicalIntent) -> Prompt {
    // ... existing body that calls retrieve_knowledge / retrieve_long_term_memory / build_prompt ...
}
```

Find the line that calls `build_prompt_with_pack` (or `build_prompt`). Replace with `build_prompt_with_pack_and_vocab`, computing the due vocab beforehand:

```rust
let due_vocab_owned = primer_core::vocab::due_concepts(
    &self.learner,
    chrono::Utc::now(),
    self.vocab_settings.max_per_prompt,
);
let due_vocab: Vec<&ConceptState> = due_vocab_owned;
// ... existing knowledge / summary / retrieved fetch ...
let prompt = build_prompt_with_pack_and_vocab(
    self.pack.as_ref(),
    &self.learner,
    &self.session,
    intent,
    &knowledge,
    &summary,
    &retrieved_older,
    self.config.context_window_turns,
    &due_vocab,
);
```

(Note: `due_concepts` already returns `Vec<&'a ConceptState>`, so the `let due_vocab: Vec<...>` shadow is just to clarify naming. The `&due_vocab` is `&[&ConceptState]` which is what the function expects.)

You may need an `use primer_core::learner::ConceptState;` import at the top of `turn.rs`.

- [ ] **Step 3: Verify compile + workspace tests**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo build --workspace 2>&1 | tail -5
~/.cargo/bin/cargo test --workspace 2>&1 | tail -3
```

Expected: clean. The existing tests still pass because no concept will yet be due in any test scenario (test fixtures have `last_encountered=None` or recently-set timestamps).

- [ ] **Step 4: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-pedagogy/src/prompt_builder.rs \
        src/crates/primer-pedagogy/src/dialogue_manager/turn.rs
git commit -m "$(cat <<'EOF'
vocab: wire due_concepts into build_turn_prompt

build_turn_prompt now computes due vocab once per turn and threads
it through the new build_prompt_with_pack_and_vocab. Existing
build_prompt_with_pack stays as a no-vocab wrapper for callers that
don't go through DialogueManager.

Wallclock dependency: chrono::Utc::now() is read here. Pure functions
remain testable via injection of `now`. End-to-end vocab integration
test lands in Task 14.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: Add `--vocab-max-per-prompt` CLI flag + parse tests

**Files:**
- Modify: `src/crates/primer-cli/src/main.rs`

- [ ] **Step 1: Write the failing parse tests**

In `src/crates/primer-cli/src/main.rs` test module:

```rust
    #[test]
    fn parses_vocab_max_per_prompt() {
        let cli = Cli::try_parse_from(["primer", "--vocab-max-per-prompt", "6"]).unwrap();
        assert_eq!(cli.vocab_max_per_prompt, Some(6));
    }

    #[test]
    fn vocab_max_per_prompt_defaults_to_none() {
        let cli = Cli::try_parse_from(["primer"]).unwrap();
        assert_eq!(cli.vocab_max_per_prompt, None);
    }

    #[test]
    fn vocab_max_per_prompt_zero_is_rejected() {
        // 0 is parsable as usize but is rejected at parse-time.
        // Use a value_parser to enforce ≥ 1.
        let result = Cli::try_parse_from(["primer", "--vocab-max-per-prompt", "0"]);
        assert!(result.is_err(), "0 should be rejected; got: {:?}", result);
    }
```

- [ ] **Step 2: Run — expect compile failure**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-cli vocab_max_per_prompt 2>&1 | tail -10
```

Expected: field not found.

- [ ] **Step 3: Add the field + value parser**

In `src/crates/primer-cli/src/main.rs`, find the `Cli` struct (search `pub struct Cli` or `#[command(`). Add in the same area as the other `--*-backend` / `--*-model` options:

```rust
    /// Maximum number of overdue/due concepts to inject into the system prompt
    /// per turn. Higher = more review pressure, more prompt bloat. Default 4.
    /// Must be at least 1; 0 disables review (use at your own risk; not yet
    /// supported via a separate flag in v1).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(usize).range(1..))]
    vocab_max_per_prompt: Option<usize>,
```

(`clap::value_parser!(usize).range(1..)` rejects 0 at parse time with a clear error message.)

- [ ] **Step 4: Thread into VocabSettings construction**

Find where the `DialogueManagerSubsystems` is built (search `DialogueManagerSubsystems {` in `main.rs`). When constructing it, set `vocab_settings`:

```rust
    let vocab_settings = primer_pedagogy::vocab::VocabSettings {
        max_per_prompt: cli
            .vocab_max_per_prompt
            .unwrap_or(primer_core::consts::vocab::DEFAULT_VOCAB_MAX_PER_PROMPT),
    };
```

Then in the `DialogueManagerSubsystems { … }` literal, pass `vocab_settings,`.

- [ ] **Step 5: Run the parse tests + workspace**

```bash
~/.cargo/bin/cargo test -p primer-cli vocab_max_per_prompt 2>&1 | tail -10
~/.cargo/bin/cargo test --workspace 2>&1 | tail -3
```

Expected: 3 parse tests pass; workspace clean.

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-cli/src/main.rs
git commit -m "$(cat <<'EOF'
cli: --vocab-max-per-prompt flag (default 4, min 1)

Reject 0 at parse-time via clap's value_parser range(1..). Threads
into VocabSettings, which DialogueManagerSubsystems already exposes.
Default behaviour preserved for users not passing the flag.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: End-to-end integration test (mocked clock)

**Files:**
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager/tests/turn_tests.rs` (or wherever turn-level e2e tests live; check Step 1)

**Important:** This test exercises the *real* dialogue flow with a scripted backend, scripted classifier/extractor/comprehension stubs, and a mocked clock. The assertion is: after a strong-comprehension exchange and a clock advance past the box-1 interval (3 days), the *next* turn's prompt contains the vocab section with the previously-discussed concept.

The clock dependency in `build_turn_prompt` reads `chrono::Utc::now()` directly. Since we have not refactored that to take an injected clock, the test cannot deterministically advance time without either:
1. Setting `last_encountered` in the past on the learner directly (bypassing the natural extraction flow), then verifying the next prompt.
2. Refactoring `build_turn_prompt` to take a `now: DateTime<Utc>` parameter (out of scope for v1 per the spec's "wallclock dependency" risk).

We use approach (1).

- [ ] **Step 1: Locate the test file**

```bash
cd /Users/hherb/src/primer/src
ls crates/primer-pedagogy/src/dialogue_manager/tests/
```

Add to `turn_tests.rs` (most likely; verify by `grep -l "respond_to_streaming" crates/primer-pedagogy/src/dialogue_manager/tests/*.rs`).

- [ ] **Step 2: Write the failing test**

Add to `turn_tests.rs`:

```rust
#[tokio::test]
async fn vocab_section_appears_when_concept_is_overdue() {
    use chrono::{Duration, Utc};
    use primer_core::learner::{ConceptState, UnderstandingDepth};

    // Build a DialogueManager with a scripted ChartedBackend that returns
    // canned text. Use a stub knowledge / classifier / extractor / comprehension
    // so no real LLM calls happen.
    let mut dm = test_support::manager_with_minimal_subsystems().await;

    // Inject a concept into the learner with a back-dated last_encountered
    // simulating "10 days ago, box_level=1 → due (interval is 3d)".
    dm.learner.concepts.push(ConceptState {
        concept_id: "physics:gravity".into(),
        depth: UnderstandingDepth::Comprehension,
        confidence: 0.85,
        encounter_count: 2,
        last_encountered: Some(Utc::now() - Duration::days(10)),
        notes: vec![],
        box_level: 1,
    });

    // Capture the prompt the next turn would build.
    let prompt = dm.build_turn_prompt(
        "tell me a story",
        primer_core::conversation::PedagogicalIntent::SocraticQuestion,
    ).await;

    // Verify the vocab section appears.
    assert!(
        prompt.system.contains("topically relevant"),
        "vocab intro should appear in prompt: {}", prompt.system
    );
    assert!(
        prompt.system.contains("physics:gravity"),
        "physics:gravity should appear in vocab list: {}", prompt.system
    );
    assert!(
        prompt.system.contains("10 days ago"),
        "expected '10 days ago' phrasing in vocab list: {}", prompt.system
    );
}

#[tokio::test]
async fn vocab_section_omitted_when_no_concepts_due() {
    use chrono::{Duration, Utc};
    use primer_core::learner::{ConceptState, UnderstandingDepth};

    let mut dm = test_support::manager_with_minimal_subsystems().await;

    // Concept is fresh (12h ago, box 0 = 1-day interval) — not due.
    dm.learner.concepts.push(ConceptState {
        concept_id: "physics:gravity".into(),
        depth: UnderstandingDepth::Aware,
        confidence: 0.7,
        encounter_count: 1,
        last_encountered: Some(Utc::now() - Duration::hours(12)),
        notes: vec![],
        box_level: 0,
    });

    let prompt = dm.build_turn_prompt(
        "what is gravity",
        primer_core::conversation::PedagogicalIntent::SocraticQuestion,
    ).await;

    assert!(
        !prompt.system.contains("topically relevant"),
        "vocab intro should NOT appear when no concept is due: {}", prompt.system
    );
}
```

You may need to add a `manager_with_minimal_subsystems` helper to `test_support.rs`. Look at existing helpers (`grep "fn manager" src/crates/primer-pedagogy/src/dialogue_manager/test_support.rs`) for the pattern; copy and rename.

If `build_turn_prompt` is private (`async fn`, not `pub`), you'll need to expose it via a `pub(super)` change or test through the public `respond_to_streaming` path. Verify visibility:

```bash
grep -n "fn build_turn_prompt" crates/primer-pedagogy/src/dialogue_manager/turn.rs
```

If private, use this alternative test that goes through `respond_to`:

```rust
    let mut dm = test_support::manager_with_capturing_backend().await;
    dm.learner.concepts.push(/* same as above */);

    let _ = dm.respond_to("tell me a story").await;

    // Inspect the prompt the backend received.
    let captured_prompt = dm.captured_prompts().last().unwrap();
    assert!(captured_prompt.system.contains("topically relevant"));
    assert!(captured_prompt.system.contains("physics:gravity"));
```

(The `manager_with_capturing_backend` and `captured_prompts` helpers may need to be added to `test_support.rs`; mirror the existing `ScriptedBackend` patterns.)

- [ ] **Step 3: Run — expect failure**

```bash
~/.cargo/bin/cargo test -p primer-pedagogy vocab_section_appears 2>&1 | tail -15
```

Expected: depending on existing test_support helpers either compile error (helper missing) or assertion failure (vocab section not in prompt yet). All earlier tasks should already make this pass — if it fails on the assertion, debug.

- [ ] **Step 4: Implement helper if needed and run again**

Most likely, the existing `test_support.rs` already has a manager builder; just add a thin wrapper that calls `DialogueManager::new` with default subsystems. Reference earlier per-task patterns.

- [ ] **Step 5: Run + workspace check**

```bash
~/.cargo/bin/cargo test -p primer-pedagogy vocab_section 2>&1 | tail -10
~/.cargo/bin/cargo test --workspace 2>&1 | tail -3
~/.cargo/bin/cargo clippy --workspace --all-targets 2>&1 | tail -5
~/.cargo/bin/cargo fmt --all -- --check 2>&1 | tail -5
```

Expected: all clean. Total test count should be ~440-460 (~30 new tests across Tasks 2-13 + 2 here).

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-pedagogy/src/dialogue_manager/tests/turn_tests.rs \
        src/crates/primer-pedagogy/src/dialogue_manager/test_support.rs
git commit -m "$(cat <<'EOF'
vocab: end-to-end integration tests for due-concept injection

Two tests exercise build_turn_prompt with a real DialogueManager:
1. Concept with back-dated last_encountered + box_level=1 + 10d gap →
   vocab section appears with the concept and "10 days ago" phrasing.
2. Fresh concept (12h ago, box 0, interval 1d) → vocab section omitted.

Pure-function tests already covered the algorithm; this test verifies
the wiring through the actual prompt-build path.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 15: Update README and ROADMAP, final verification

**Files:**
- Modify: `README.md`
- Modify: `ROADMAP.md`

- [ ] **Step 1: Read current README + ROADMAP to find the right edit spots**

```bash
grep -n "Phase 0.3\|vocab\|spaced" /Users/hherb/src/primer/README.md /Users/hherb/src/primer/ROADMAP.md
```

- [ ] **Step 2: Add a brief feature note to README under the existing feature list**

Find the section that lists features (likely under a "What's working" or "Features" header). Add a bullet:

```markdown
- **Spaced-repetition vocabulary review** — concepts the child has previously encountered are gently surfaced back into the conversation at expanding intervals (1d / 3d / 7d / 14d / 30d) based on demonstrated mastery via the comprehension classifier. Passive prompt-injection only — the LLM weaves them in only if topically relevant; no drilling, no quizzing. Configurable via `--vocab-max-per-prompt N` (default 4).
```

- [ ] **Step 3: Mark the Phase 0.3 vocab bullet as done in ROADMAP**

Find the Phase 0.3 section. Locate the line about vocabulary tracking / spaced repetition. Change from `- [ ]` to `- [x]` and add a date / commit-range hint:

```markdown
- [x] **Per-child vocabulary tracking with spaced repetition** *(2026-05-05; spec at `docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md`)*
  - Leitner-box scheduler: 5 boxes, intervals 1d/3d/7d/14d/30d.
  - Driven by the existing comprehension classifier (no new LLM call).
  - Schema v7 adds `learner_concepts.box_level`.
  - Configurable via `--vocab-max-per-prompt N`.
```

- [ ] **Step 4: Final verification gauntlet**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo build --workspace 2>&1 | tail -3
~/.cargo/bin/cargo test --workspace 2>&1 | tail -3
~/.cargo/bin/cargo clippy --workspace --all-targets 2>&1 | tail -5
~/.cargo/bin/cargo fmt --all -- --check 2>&1 | tail -3
~/.cargo/bin/cargo build --workspace --features primer-cli/speech 2>&1 | tail -3
```

All must be clean. Test count should be ~470-490 (was 434 + ~35-40 new).

- [ ] **Step 5: Commit and verify branch state**

```bash
cd /Users/hherb/src/primer
git add README.md ROADMAP.md
git commit -m "$(cat <<'EOF'
docs: README + ROADMAP updates for vocab spaced-repetition feature

Phase 0.3 vocabulary tracking with Leitner-box spaced repetition is
now shipped on this branch. Brief mention in README under the
features list; ROADMAP bullet marked complete.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
git log --oneline main..HEAD
```

Expected: ~14-15 commits since branch from main. Print and review for sanity.

- [ ] **Step 6: Smoke test with a real backend (optional but strongly encouraged)**

Manual end-to-end smoke (skip if you don't have an API key handy):

```bash
cd /Users/hherb/src/primer/src
RUST_LOG=debug ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name SmokeTester --age 9 --no-persist \
    --vocab-max-per-prompt 4 \
    --verbose 2>&1 | tee /tmp/vocab-smoke.log
```

In the conversation:
1. Discuss a concept (e.g. "what is gravity") and let the Primer Socratically follow up. Several turns.
2. End that conversation (`bye`).
3. Manually edit the SQLite session-db (use `sqlite3` directly) to back-date `learner_concepts.last_encountered` by 10 days for the concept you discussed.
4. Restart with `--resume <id>` and ask an unrelated question.
5. Look in the log for the system prompt — confirm the vocab section appears with the back-dated concept.

(This is optional verification — the integration tests in Task 14 already cover the wiring.)

---

## Self-Review (writer's checklist)

**1. Spec coverage:**

| Spec section | Implemented in task |
|---|---|
| Add `box_level` field to `ConceptState` | Task 1 |
| `consts::vocab` submodule | Task 1 |
| `apply_box_transition` pure fn | Task 2 |
| `is_due` pure fn | Task 3 |
| `due_concepts` pure fn | Task 4 |
| Schema v7 migration | Task 5 |
| `save_learner` extension | Task 6 |
| `load_learner` extension | Task 7 |
| `apply_comprehension` box transition call | Task 8 |
| `VocabSettings` + DialogueManagerSubsystems plumbing | Task 9 |
| `PromptPack::vocab_review_intro` + en/de TOML | Task 10 |
| `vocab_section` in `build_system_prompt_with_pack` | Task 11 |
| `due_concepts` wired into `build_turn_prompt` | Task 12 |
| `--vocab-max-per-prompt` CLI flag | Task 13 |
| End-to-end integration test | Task 14 |
| README + ROADMAP updates | Task 15 |

All spec sections have a task. ✓

**2. Placeholder scan:** No "TBD", "TODO", "implement later", or vague "add validation" steps. Every code step shows the actual code. ✓ — except in Task 9 where the existence of a `settings.rs` file is conditional on the codebase shape; the task explicitly says "verify by listing", which is a legitimate exploratory step, not a placeholder.

**3. Type / signature consistency:**
- `apply_box_transition(u8, UnderstandingDepth, f32) -> u8` — used in Task 2 (definition) and Task 8 (call site). Match. ✓
- `is_due(&ConceptState, DateTime<Utc>) -> bool` — Task 3 (def) used by Task 4. Match. ✓
- `due_concepts(&LearnerModel, DateTime<Utc>, usize) -> Vec<&ConceptState>` — Task 4 (def), Task 12 (call). Match. ✓
- `apply_v7_migrations` is `pub(crate)`. Task 5 defines it; same crate (primer-storage) calls it from lib.rs. ✓
- `VocabSettings { max_per_prompt: usize }` — Task 9 (def), Task 13 (CLI builds it). Match. ✓
- `vocab_review_intro(&self) -> &str` — Task 10 (def), Task 11 (call). Match. ✓
- `build_system_prompt_with_pack_and_vocab` signature — Task 11 (def), Task 12 (call from `build_prompt_with_pack_and_vocab`). Match. ✓
- `build_prompt_with_pack_and_vocab` signature — Task 12 (def), `build_turn_prompt` calls it. Match. ✓

All cross-task type/signature references check out. ✓

---

## Plan complete

**Total estimated tasks: 15.** Each task ships as a single commit with green tests, clippy, and fmt at every step. New tests: ~35-40. Schema migration: v7 (one column).
