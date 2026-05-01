# Engagement classifier — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the placeholder word-count engagement assessor with a real LLM-backed classifier that routes the three Phase 0.3 `decide_intent` gaps (FrustratedStuck/FrustratedTrying split, factual-question pattern detection, session-length-aware Disengaging), plus the persistence of every classification for offline analysis.

**Architecture:** New `primer-classifier` crate with an `EngagementClassifier` trait, an LLM-backed implementation wrapping `Arc<dyn InferenceBackend>`, and a stub. Sequential post-response classification in the natural inter-turn pause (memory-constrained-friendly). Schema v3 in `primer-storage` adds `engagement_states`, `classifiers`, `turn_classifications` tables. `EngagementState` splits `Frustrated → FrustratedStuck/FrustratedTrying`; `decide_intent_at(now)` adds `now`-aware Disengaging routing. New CLI flags `--classifier-backend`, `--classifier-model`, `--classifier-timeout-ms`, `--verbose`.

**Tech Stack:** Rust 2024, `async_trait`, `tokio` (spawn + abort_handle + time::timeout), `rusqlite` (bundled SQLite, FTS5), `tracing` for soft-failure logging, `clap` for CLI, `serde_json` for classifier output parsing.

**Spec:** [docs/superpowers/specs/2026-05-01-engagement-classifier-design.md](../specs/2026-05-01-engagement-classifier-design.md)

---

## File structure

### Created

| Path | Responsibility |
|---|---|
| `src/crates/primer-classifier/Cargo.toml` | New workspace member, deps: `primer-core`, `async-trait`, `tokio`, `tracing`, `serde`, `serde_json`, `chrono` |
| `src/crates/primer-classifier/src/lib.rs` | `EngagementClassifier` trait, re-exports |
| `src/crates/primer-classifier/src/consts.rs` | Default values for ClassifierSettings |
| `src/crates/primer-classifier/src/settings.rs` | `ClassifierSettings` struct + `Default` impl |
| `src/crates/primer-classifier/src/stub.rs` | `StubEngagementClassifier` (3 constructors) |
| `src/crates/primer-classifier/src/llm.rs` | `LlmEngagementClassifier` (prompt build, output parse, soft-fail) |
| `src/crates/primer-pedagogy/src/consts.rs` | `SHORT_TURN_WORD_BOUNDARY`, `ACTIVE_CONCEPT_LOOKBACK`, `DEFAULT_EARLY_DISENGAGEMENT_SECS` |
| `src/crates/primer-core/src/classifier.rs` | `EngagementAssessment`, `EngagementContext` types |

### Modified

| Path | Change |
|---|---|
| `src/Cargo.toml` | Add `crates/primer-classifier` to `[workspace] members`, `primer-classifier.workspace = true` to deps, `serde_json.workspace = true` |
| `src/crates/primer-core/src/error.rs` | Add `Classification(String)` variant |
| `src/crates/primer-core/src/lib.rs` | `pub mod classifier;` re-export |
| `src/crates/primer-core/src/learner.rs` | Split `EngagementState::Frustrated` → `FrustratedStuck`/`FrustratedTrying`; update `ALL`; add `early_disengagement_threshold` field on `LearningPreferences`; add `recent_assessments: Vec<EngagementAssessment>` field on `LearnerModel` |
| `src/crates/primer-storage/src/schema.rs` | Bump `USER_VERSION` to 3; add `apply_v3_migrations`; new schema strings for `engagement_states`, `classifiers`, `turn_classifications` |
| `src/crates/primer-storage/src/catalog.rs` | Add `engagement_state_id`, `engagement_state_from_id`, `expected_engagement_states`; add lazy `get_or_create_classifier_id` helper |
| `src/crates/primer-storage/src/lib.rs` | Implement `save_classification` and `load_recent_assessments` on `SqliteSessionStore` |
| `src/crates/primer-core/src/storage.rs` | Add `save_classification` and `load_recent_assessments` to `SessionStore` trait |
| `src/crates/primer-pedagogy/Cargo.toml` | Add `primer-classifier.workspace = true` |
| `src/crates/primer-pedagogy/src/lib.rs` | `pub mod consts;` |
| `src/crates/primer-pedagogy/src/prompt_builder.rs` | New `is_factual_question` helper; `decide_intent_at(now)` refactor; engagement_note for new variants; characterization-test updates |
| `src/crates/primer-pedagogy/src/dialogue_manager.rs` | New fields on `DialogueManager` (`classifier`, `classifier_settings`, `classify_task`); `await_pending_classification` + `apply_assessment`; spawn classify task at end of `respond_to_streaming`; resume rehydration |
| `src/crates/primer-cli/Cargo.toml` | Add `primer-classifier.workspace = true` |
| `src/crates/primer-cli/src/main.rs` | New CLI flags, classifier construction matrix, `--verbose` stderr output |
| `ROADMAP.md` | Tick three Phase 0.3 bullets (`Encouragement` reachable, factual-question detection, session-length-aware Disengaging) |
| `CLAUDE.md` | Document new crate, classifier flow, schema v3, new conventions |
| `primer_next_session.md` | Refresh post-merge brief |

---

## Conventions

- Run all `cargo` commands from `src/` (workspace root, not repo root).
- Test count baseline at start of plan: **119**. Final target: ~170. Each task that adds tests notes the running total in its commit message.
- Commit after every task. Subjects use the existing repo convention (`feat:`, `test:`, `refactor:`, `docs:`).
- Working on feature branch `feature/engagement-classifier` (created at start of execution; main stays clean during implementation).
- Every `#[tokio::test]` uses `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` only when spawn+abort behavior is exercised; otherwise default `#[tokio::test]`.
- Tests live in `#[cfg(test)] mod tests { ... }` inside the same file as the code under test, mirroring the existing pattern.
- TDD discipline per task: write failing test → run → verify FAIL → implement → run → verify PASS → commit.

---

## Phase 1 — Core types

### Task 1: Add `PrimerError::Classification` variant

**Files:**
- Modify: `src/crates/primer-core/src/error.rs`

- [ ] **Step 1: Add the variant**

Append to the `PrimerError` enum (after the `Storage` variant):

```rust
#[error("Classification error: {0}")]
Classification(String),
```

- [ ] **Step 2: Verify the workspace still compiles**

```bash
cargo build --workspace
```
Expected: clean compile.

- [ ] **Step 3: Commit**

```bash
git add src/crates/primer-core/src/error.rs
git commit -m "feat(core): add PrimerError::Classification variant"
```

---

### Task 2: Split `EngagementState::Frustrated` into `FrustratedStuck`/`FrustratedTrying`

**Files:**
- Modify: `src/crates/primer-core/src/learner.rs`

- [ ] **Step 1: Update the `EngagementState` enum**

Replace the existing `Frustrated` variant with two:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngagementState {
    Engaged,
    Reflecting,
    /// Frustrated and stuck — no progress, gives up. Routes to Scaffolding.
    FrustratedStuck,
    /// Frustrated but still articulating an attempt. Routes to Encouragement.
    FrustratedTrying,
    Disengaging,
    Unknown,
}

impl EngagementState {
    pub const ALL: &'static [Self] = &[
        Self::Engaged,
        Self::Reflecting,
        Self::FrustratedStuck,
        Self::FrustratedTrying,
        Self::Disengaging,
        Self::Unknown,
    ];
}
```

- [ ] **Step 2: Verify the test for the ALL array passes**

The existing test (in `primer-core/src/conversation.rs` or `primer-core/src/learner.rs` if present) should still pass. Add or update an explicit one:

```rust
#[test]
fn engagement_state_all_lists_every_variant() {
    assert_eq!(EngagementState::ALL.len(), 6);
    assert!(EngagementState::ALL.contains(&EngagementState::FrustratedStuck));
    assert!(EngagementState::ALL.contains(&EngagementState::FrustratedTrying));
}
```

Run:
```bash
cargo test -p primer-core engagement_state_all_lists_every_variant
```
Expected: PASS.

- [ ] **Step 3: Update downstream callers that match on `EngagementState::Frustrated`**

The compiler will surface every site. Expected sites:
- `src/crates/primer-pedagogy/src/prompt_builder.rs` — `decide_intent` (matches `Frustrated`) and `build_system_prompt` (`engagement_note`)
- `src/crates/primer-pedagogy/src/dialogue_manager.rs::update_learner_model` (the placeholder heuristic that currently sets `Frustrated`)
- `src/crates/primer-pedagogy/src/prompt_builder.rs::tests` — multiple tests use `EngagementState::Frustrated`

For now, **map every `Frustrated` reference to `FrustratedStuck`** (preserves the existing routing — Scaffolding). Tests that explicitly test the (about-to-be-flipped) "Frustrated returns Scaffolding" behavior keep passing because `FrustratedStuck` returns the same intent. This keeps the build green between Phase 1 and Phase 4 where the routing changes properly.

- [ ] **Step 4: Run the full workspace test suite**

```bash
cargo test --workspace
```
Expected: all 119 existing tests pass + 1 new (`engagement_state_all_lists_every_variant` if it didn't already exist). Total: 119 or 120.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(core,pedagogy): split EngagementState::Frustrated into FrustratedStuck/FrustratedTrying

Existing routing preserved: Frustrated->Scaffolding becomes FrustratedStuck->Scaffolding.
Phase 4 adds the FrustratedTrying->Encouragement branch."
```

---

### Task 3: Create `EngagementAssessment` and `EngagementContext` types

**Files:**
- Create: `src/crates/primer-core/src/classifier.rs`
- Modify: `src/crates/primer-core/src/lib.rs`

- [ ] **Step 1: Write the new module**

Create `src/crates/primer-core/src/classifier.rs`:

```rust
//! Classifier types — output and input for the engagement classifier.
//!
//! `EngagementAssessment` is one classifier output. `EngagementContext`
//! is its input — newtype-wrapped so future fields (learner_age,
//! session_started_at, etc.) can be added without breaking the trait
//! signature.

use serde::{Deserialize, Serialize};

use crate::conversation::Turn;
use crate::learner::EngagementState;

/// One classifier output. State + confidence; `reasoning` is optional and
/// populated only by classifier impls that produce one (LLM impls can
/// generate it cheaply; BERT/stub do not).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngagementAssessment {
    pub state: EngagementState,
    /// Confidence in the assessment, in [0.0, 1.0].
    pub confidence: f32,
    /// Optional one-sentence rationale. Populated by LLM impls; None
    /// for stub and (future) BERT classifiers.
    pub reasoning: Option<String>,
}

impl EngagementAssessment {
    /// Construct a low-confidence Unknown assessment, used as the
    /// soft-fail return on classifier errors (parse failure, network
    /// error, invalid output).
    pub fn unknown_low_confidence(reason: impl Into<String>) -> Self {
        Self {
            state: EngagementState::Unknown,
            confidence: 0.0,
            reasoning: Some(reason.into()),
        }
    }
}

/// Input to the classifier. Newtype'd so future fields can be added
/// without breaking the trait signature.
pub struct EngagementContext<'a> {
    pub recent_child_turns: &'a [Turn],
    pub prior_assessments: &'a [EngagementAssessment],
    // Reserved for extension. Adding fields here is non-breaking
    // because the trait method takes EngagementContext by value.
}
```

- [ ] **Step 2: Re-export from `lib.rs`**

Add to `src/crates/primer-core/src/lib.rs`:

```rust
pub mod classifier;
```

(Place it alphabetically near the other `pub mod` declarations.)

- [ ] **Step 3: Write a smoke test**

In `src/crates/primer-core/src/classifier.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_low_confidence_factory_returns_unknown_state() {
        let a = EngagementAssessment::unknown_low_confidence("parse failure");
        assert_eq!(a.state, EngagementState::Unknown);
        assert_eq!(a.confidence, 0.0);
        assert_eq!(a.reasoning.as_deref(), Some("parse failure"));
    }
}
```

Run:
```bash
cargo test -p primer-core unknown_low_confidence_factory_returns_unknown_state
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(core): add EngagementAssessment and EngagementContext types"
```

---

### Task 4: Extend `LearningPreferences` and `LearnerModel`

**Files:**
- Modify: `src/crates/primer-core/src/learner.rs`

- [ ] **Step 1: Add `early_disengagement_threshold` to `LearningPreferences`**

Add a new field plus a constants module-level constant. In `learner.rs`:

```rust
use std::time::Duration;

// Near the existing imports.

/// Default threshold for the session-length-aware Disengaging branch.
/// Below this, Disengaging routes to Encouragement; at or above, SessionClose.
/// Per-child tunable via `LearningPreferences::early_disengagement_threshold`.
pub const DEFAULT_EARLY_DISENGAGEMENT_SECS: u64 = 5 * 60;
```

Modify the struct:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningPreferences {
    pub narrative: f32,
    pub socratic: f32,
    pub visual: f32,
    pub kinesthetic: f32,
    pub typical_session_minutes: f32,
    pub high_engagement_topics: Vec<String>,
    /// Below this duration into the session, Disengaging routes to
    /// Encouragement; at or above, SessionClose. Per-child tunable.
    /// Persistence (when learner-model persistence lands) round-trips
    /// as u64 seconds; not a v3 schema concern.
    #[serde(with = "duration_secs", default = "default_early_disengagement")]
    pub early_disengagement_threshold: Duration,
}

fn default_early_disengagement() -> Duration {
    Duration::from_secs(DEFAULT_EARLY_DISENGAGEMENT_SECS)
}

impl Default for LearningPreferences {
    fn default() -> Self {
        Self {
            narrative: 0.5,
            socratic: 0.5,
            visual: 0.5,
            kinesthetic: 0.5,
            typical_session_minutes: 20.0,
            high_engagement_topics: vec![],
            early_disengagement_threshold: default_early_disengagement(),
        }
    }
}

mod duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_secs())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        u64::deserialize(d).map(Duration::from_secs)
    }
}
```

- [ ] **Step 2: Add `recent_assessments` to `LearnerModel`**

Modify the struct:

```rust
use crate::classifier::EngagementAssessment;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnerModel {
    pub profile: LearnerProfile,
    pub concepts: Vec<ConceptState>,
    pub preferences: LearningPreferences,
    pub current_engagement: EngagementState,
    /// Bounded buffer of the most recent classifier outputs for this
    /// learner. Capacity = `ClassifierSettings::history_depth` (default 3).
    /// Ephemeral in active use; rehydrated from `turn_classifications`
    /// on resume.
    #[serde(default)]
    pub recent_assessments: Vec<EngagementAssessment>,
}
```

- [ ] **Step 3: Write a smoke test verifying defaults**

Add to `learner.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_learning_preferences_includes_early_disengagement_threshold() {
        let prefs = LearningPreferences::default();
        assert_eq!(
            prefs.early_disengagement_threshold,
            Duration::from_secs(DEFAULT_EARLY_DISENGAGEMENT_SECS),
        );
    }

    #[test]
    fn learning_preferences_round_trips_through_serde_with_seconds() {
        let prefs = LearningPreferences {
            early_disengagement_threshold: Duration::from_secs(123),
            ..Default::default()
        };
        let json = serde_json::to_string(&prefs).unwrap();
        assert!(json.contains("\"early_disengagement_threshold\":123"));
        let back: LearningPreferences = serde_json::from_str(&json).unwrap();
        assert_eq!(back.early_disengagement_threshold, Duration::from_secs(123));
    }
}
```

(Note: this assumes `serde_json` is in `dev-dependencies`. If not, add it: `serde_json.workspace = true` to primer-core's `Cargo.toml` `[dev-dependencies]`.)

- [ ] **Step 4: Run the new tests**

```bash
cargo test -p primer-core default_learning_preferences_includes_early_disengagement_threshold
cargo test -p primer-core learning_preferences_round_trips_through_serde_with_seconds
```
Expected: both PASS.

- [ ] **Step 5: Update `LearnerModel` constructors in CLI/tests if needed**

The compiler will flag any literal `LearnerModel { ... }` that doesn't include `recent_assessments`. Fix each by adding `recent_assessments: vec![]`. Likely sites:
- `src/crates/primer-cli/src/main.rs` (constructs the LearnerModel from CLI flags)
- `src/crates/primer-pedagogy/src/prompt_builder.rs::tests::learner_with` helper
- `src/crates/primer-pedagogy/src/dialogue_manager.rs::tests` (any test fixtures)

Run:
```bash
cargo build --workspace
cargo test --workspace
```
Expected: clean build, all 121+ tests pass.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(core): extend LearningPreferences and LearnerModel for classifier

LearningPreferences gains early_disengagement_threshold (Duration, serialised
as seconds). LearnerModel gains recent_assessments (Vec, ephemeral) for the
classifier trajectory buffer. Defaults preserved for existing call sites."
```

---

## Phase 2 — Storage v3 migration

### Task 5: Add `engagement_states` catalogue

**Files:**
- Modify: `src/crates/primer-storage/src/catalog.rs`

- [ ] **Step 1: Write characterization tests for the new ID mapping**

Add to `catalog.rs`:

```rust
#[cfg(test)]
mod engagement_state_tests {
    use super::*;
    use primer_core::learner::EngagementState;

    #[test]
    fn engagement_state_id_is_stable_for_every_variant() {
        // Document the canonical id->variant mapping. If you ever
        // need to change these ids, retired ones must NEVER be reused.
        assert_eq!(engagement_state_id(EngagementState::Engaged), 1);
        assert_eq!(engagement_state_id(EngagementState::Reflecting), 2);
        assert_eq!(engagement_state_id(EngagementState::FrustratedStuck), 3);
        assert_eq!(engagement_state_id(EngagementState::FrustratedTrying), 4);
        assert_eq!(engagement_state_id(EngagementState::Disengaging), 5);
        assert_eq!(engagement_state_id(EngagementState::Unknown), 6);
    }

    #[test]
    fn engagement_state_from_id_is_inverse_of_id() {
        for state in EngagementState::ALL {
            let id = engagement_state_id(*state);
            assert_eq!(engagement_state_from_id(id), Some(*state));
        }
    }

    #[test]
    fn engagement_state_from_id_returns_none_on_unknown_id() {
        assert!(engagement_state_from_id(999).is_none());
    }

    #[test]
    fn expected_engagement_states_covers_all_variants() {
        let pairs = expected_engagement_states();
        assert_eq!(pairs.len(), EngagementState::ALL.len());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p primer-storage engagement_state_id_is_stable_for_every_variant
```
Expected: FAIL with "cannot find function `engagement_state_id`".

- [ ] **Step 3: Implement the catalog functions**

Add to `catalog.rs` (mirror the existing `intent_id`/`speaker_id` pattern):

```rust
use primer_core::learner::EngagementState;

pub(crate) fn engagement_state_id(state: EngagementState) -> i64 {
    match state {
        EngagementState::Engaged => 1,
        EngagementState::Reflecting => 2,
        EngagementState::FrustratedStuck => 3,
        EngagementState::FrustratedTrying => 4,
        EngagementState::Disengaging => 5,
        EngagementState::Unknown => 6,
    }
}

pub(crate) fn engagement_state_from_id(id: i64) -> Option<EngagementState> {
    Some(match id {
        1 => EngagementState::Engaged,
        2 => EngagementState::Reflecting,
        3 => EngagementState::FrustratedStuck,
        4 => EngagementState::FrustratedTrying,
        5 => EngagementState::Disengaging,
        6 => EngagementState::Unknown,
        _ => return None,
    })
}

pub(crate) fn expected_engagement_states() -> Vec<(i64, &'static str)> {
    EngagementState::ALL
        .iter()
        .map(|s| (engagement_state_id(*s), engagement_state_name(*s)))
        .collect()
}

fn engagement_state_name(state: EngagementState) -> &'static str {
    match state {
        EngagementState::Engaged => "Engaged",
        EngagementState::Reflecting => "Reflecting",
        EngagementState::FrustratedStuck => "FrustratedStuck",
        EngagementState::FrustratedTrying => "FrustratedTrying",
        EngagementState::Disengaging => "Disengaging",
        EngagementState::Unknown => "Unknown",
    }
}
```

- [ ] **Step 4: Run all four tests**

```bash
cargo test -p primer-storage engagement_state
```
Expected: 4 PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(storage): add engagement_states catalog with stable ID mapping"
```

---

### Task 6: Schema v3 migration — tables + USER_VERSION bump

**Files:**
- Modify: `src/crates/primer-storage/src/schema.rs`

- [ ] **Step 1: Write the v3 migration test (positive case)**

Add to `schema.rs`:

```rust
#[cfg(test)]
mod v3_tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn apply_v3_migrations_creates_all_three_tables() {
        let conn = Connection::open_in_memory().unwrap();
        // Set up v2 baseline: existing schema must be valid before v3 runs.
        apply_v1_schema(&conn).unwrap();
        apply_v2_migrations(&conn).unwrap();

        apply_v3_migrations(&conn).unwrap();

        // engagement_states
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='engagement_states'",
            [], |r| r.get(0)
        ).unwrap();
        assert_eq!(count, 1);

        // classifiers
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='classifiers'",
            [], |r| r.get(0)
        ).unwrap();
        assert_eq!(count, 1);

        // turn_classifications
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='turn_classifications'",
            [], |r| r.get(0)
        ).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn apply_v3_migrations_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        apply_v1_schema(&conn).unwrap();
        apply_v2_migrations(&conn).unwrap();
        apply_v3_migrations(&conn).unwrap();
        apply_v3_migrations(&conn).unwrap();  // second call must succeed
    }

    #[test]
    fn user_version_is_three() {
        assert_eq!(USER_VERSION, 3);
    }
}
```

(Adjust `apply_v1_schema` / `apply_v2_migrations` to whatever those helpers are actually named in the existing module.)

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p primer-storage apply_v3_migrations_creates_all_three_tables user_version_is_three
```
Expected: FAIL — `apply_v3_migrations` not defined and `USER_VERSION != 3`.

- [ ] **Step 3: Implement the migration**

Update `USER_VERSION`:

```rust
pub const USER_VERSION: i64 = 3;
```

Add the schema strings:

```rust
const CREATE_ENGAGEMENT_STATES_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS engagement_states (
        id   INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE
    )
";

const CREATE_CLASSIFIERS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS classifiers (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        identifier TEXT NOT NULL UNIQUE
    )
";

const CREATE_TURN_CLASSIFICATIONS_TABLE: &str = "
    CREATE TABLE IF NOT EXISTS turn_classifications (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        turn_id             INTEGER NOT NULL REFERENCES turns(id),
        engagement_state_id INTEGER NOT NULL REFERENCES engagement_states(id),
        classifier_id       INTEGER NOT NULL REFERENCES classifiers(id),
        confidence          REAL NOT NULL,
        reasoning           TEXT,
        classified_at       TIMESTAMP NOT NULL,
        UNIQUE(turn_id, classifier_id)
    )
";

const CREATE_TURN_CLASSIFICATIONS_INDEX: &str = "
    CREATE INDEX IF NOT EXISTS idx_turn_classifications_turn_id
        ON turn_classifications(turn_id)
";
```

Add the migration function (mirroring `apply_v2_migrations`'s transaction-wrapped, idempotent pattern):

```rust
pub(crate) fn apply_v3_migrations(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("v3 migration: failed to begin tx: {e}")))?;
    tx.execute(CREATE_ENGAGEMENT_STATES_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v3 migration: engagement_states: {e}")))?;
    tx.execute(CREATE_CLASSIFIERS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v3 migration: classifiers: {e}")))?;
    tx.execute(CREATE_TURN_CLASSIFICATIONS_TABLE, [])
        .map_err(|e| PrimerError::Storage(format!("v3 migration: turn_classifications: {e}")))?;
    tx.execute(CREATE_TURN_CLASSIFICATIONS_INDEX, [])
        .map_err(|e| PrimerError::Storage(format!("v3 migration: index: {e}")))?;
    tx.commit()
        .map_err(|e| PrimerError::Storage(format!("v3 migration: commit: {e}")))?;
    Ok(())
}
```

- [ ] **Step 4: Wire `apply_v3_migrations` into the `open()` flow**

Find the existing `open()` method in `src/crates/primer-storage/src/lib.rs`. It already calls `apply_v1_schema` and `apply_v2_migrations` based on `USER_VERSION` checks. Extend the chain:

```rust
match existing_version {
    0 => {
        apply_v1_schema(&conn)?;
        apply_v2_migrations(&conn)?;
        apply_v3_migrations(&conn)?;
        bump_user_version(&conn, USER_VERSION)?;
    }
    1 => {
        apply_v2_migrations(&conn)?;
        apply_v3_migrations(&conn)?;
        bump_user_version(&conn, USER_VERSION)?;
    }
    2 => {
        apply_v3_migrations(&conn)?;
        bump_user_version(&conn, USER_VERSION)?;
    }
    3 => { /* current */ }
    v if v > USER_VERSION => {
        return Err(PrimerError::Storage(format!(
            "DB schema version {v} is newer than this build's {USER_VERSION}"
        )));
    }
    other => {
        return Err(PrimerError::Storage(format!(
            "Unexpected schema version {other}"
        )));
    }
}
```

(Adjust to the exact shape of the existing match.)

- [ ] **Step 5: Run the tests**

```bash
cargo test -p primer-storage v3
```
Expected: all 3 PASS.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(storage): schema v3 — engagement_states/classifiers/turn_classifications tables"
```

---

### Task 7: Validate-and-seed for `engagement_states`

**Files:**
- Modify: `src/crates/primer-storage/src/schema.rs` (or wherever the existing `validate_and_seed_lookup` lives)
- Modify: `src/crates/primer-storage/src/lib.rs` (the open() call site)

- [ ] **Step 1: Write tests for the new validate-and-seed pass**

Add to `schema.rs` or `lib.rs` tests (mirror existing tests for `speakers` / `pedagogical_intents`):

```rust
#[test]
fn open_seeds_engagement_states_on_fresh_db() {
    let store = SqliteSessionStore::open_in_memory().unwrap();
    let conn = store.connection_for_test();  // or however the existing tests access the connection
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM engagement_states", [], |r| r.get(0)
    ).unwrap();
    assert_eq!(count, EngagementState::ALL.len() as i64);
}

#[test]
fn open_validates_engagement_states_against_enum() {
    // Insert a row that doesn't match — must fail on next open().
    let conn = Connection::open_in_memory().unwrap();
    apply_v1_schema(&conn).unwrap();
    apply_v2_migrations(&conn).unwrap();
    apply_v3_migrations(&conn).unwrap();
    bump_user_version(&conn, USER_VERSION).unwrap();
    // Pre-seed something wrong:
    conn.execute("INSERT INTO engagement_states (id, name) VALUES (1, 'Wrong')", []).unwrap();
    // Reopen via SqliteSessionStore — should error.
    let result = SqliteSessionStore::from_existing_connection(conn);
    assert!(result.is_err());
}
```

(Use the existing test patterns in `primer-storage` for setting up an in-memory DB with a hand-rolled state. The exact helper names depend on the existing module's pattern.)

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p primer-storage open_seeds_engagement_states open_validates_engagement_states
```
Expected: FAIL — validate-and-seed not yet wired for engagement_states.

- [ ] **Step 3: Wire validate_and_seed for engagement_states into `open()`**

After `apply_v3_migrations` succeeds (or as part of the post-migration validation pass that already exists for speakers/intents), add:

```rust
validate_and_seed_lookup(
    conn,
    "engagement_states",
    &expected_engagement_states(),
)?;
```

(Call this in the same place the existing speakers/intents validation pass runs — refer to `apply_v2_migrations` or wherever `validate_and_seed_lookup` is called in the existing code.)

- [ ] **Step 4: Run the tests**

```bash
cargo test -p primer-storage open_seeds_engagement_states open_validates_engagement_states
```
Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(storage): validate-and-seed engagement_states against EngagementState::ALL"
```

---

### Task 8: Lazy-create `classifiers` row helper

**Files:**
- Modify: `src/crates/primer-storage/src/catalog.rs`

- [ ] **Step 1: Write a test**

```rust
#[test]
fn get_or_create_classifier_id_creates_on_first_call() {
    let conn = open_v3_db_in_memory();
    let id1 = get_or_create_classifier_id(&conn, "stub").unwrap();
    let id2 = get_or_create_classifier_id(&conn, "stub").unwrap();
    assert_eq!(id1, id2, "second call must return the same id");
}

#[test]
fn get_or_create_classifier_id_handles_distinct_identifiers() {
    let conn = open_v3_db_in_memory();
    let stub = get_or_create_classifier_id(&conn, "stub").unwrap();
    let llm = get_or_create_classifier_id(&conn, "llm:claude-haiku-4-5").unwrap();
    assert_ne!(stub, llm);
}
```

(Define `open_v3_db_in_memory()` as a test helper that opens an in-memory SqliteSessionStore at v3.)

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p primer-storage get_or_create_classifier_id
```
Expected: FAIL — function not defined.

- [ ] **Step 3: Implement**

In `catalog.rs`:

```rust
use rusqlite::{params, Connection, OptionalExtension};

pub(crate) fn get_or_create_classifier_id(conn: &Connection, identifier: &str) -> Result<i64> {
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM classifiers WHERE identifier = ?1",
            params![identifier],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| PrimerError::Storage(format!("classifier lookup: {e}")))?
    {
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO classifiers (identifier) VALUES (?1)",
        params![identifier],
    )
    .map_err(|e| PrimerError::Storage(format!("classifier insert: {e}")))?;
    Ok(conn.last_insert_rowid())
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p primer-storage get_or_create_classifier_id
```
Expected: 2 PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(storage): lazy classifier-row creation helper"
```

---

### Task 9: `save_classification` and `load_recent_assessments` on `SessionStore`

**Files:**
- Modify: `src/crates/primer-core/src/storage.rs`
- Modify: `src/crates/primer-storage/src/lib.rs`

- [ ] **Step 1: Add the trait methods**

In `src/crates/primer-core/src/storage.rs`, extend the `SessionStore` trait:

```rust
use crate::classifier::EngagementAssessment;
use crate::conversation::SessionId;

#[async_trait]
pub trait SessionStore: Send + Sync {
    // ... existing methods ...

    /// Persist one classification of one child turn. Resolves
    /// (session_id, turn_index) -> turn_id internally; lazily creates
    /// the `classifiers` lookup row if `classifier_identifier` is new.
    async fn save_classification(
        &self,
        session_id: SessionId,
        turn_index: usize,
        assessment: &EngagementAssessment,
        classifier_identifier: &str,
    ) -> Result<()>;

    /// Load the most recent K classifications for this session, filtered
    /// by classifier identifier. Ordered by `classified_at` ascending
    /// (oldest first, so callers can directly use as the trajectory
    /// buffer).
    async fn load_recent_assessments(
        &self,
        session_id: SessionId,
        classifier_identifier: &str,
        k: usize,
    ) -> Result<Vec<EngagementAssessment>>;
}
```

- [ ] **Step 2: Write tests for both methods**

In `src/crates/primer-storage/src/lib.rs` test module:

```rust
#[tokio::test]
async fn save_classification_persists_to_table() {
    let store = SqliteSessionStore::open_in_memory().await.unwrap();
    // First save a session with one child turn so turn_id exists.
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(Turn {
        speaker: Speaker::Child,
        text: "what is gravity?".into(),
        timestamp: Utc::now(),
        intent: None,
        concepts: vec![],
    });
    store.save_session(&session).await.unwrap();

    let assessment = EngagementAssessment {
        state: EngagementState::Engaged,
        confidence: 0.92,
        reasoning: Some("child curious".into()),
    };
    store.save_classification(session.id, 0, &assessment, "stub").await.unwrap();

    let loaded = store.load_recent_assessments(session.id, "stub", 10).await.unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].state, EngagementState::Engaged);
    assert!((loaded[0].confidence - 0.92).abs() < 1e-6);
    assert_eq!(loaded[0].reasoning.as_deref(), Some("child curious"));
}

#[tokio::test]
async fn save_classification_handles_null_reasoning() {
    let store = SqliteSessionStore::open_in_memory().await.unwrap();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(Turn {
        speaker: Speaker::Child, text: "ok".into(),
        timestamp: Utc::now(), intent: None, concepts: vec![],
    });
    store.save_session(&session).await.unwrap();

    let assessment = EngagementAssessment {
        state: EngagementState::Reflecting,
        confidence: 0.5,
        reasoning: None,
    };
    store.save_classification(session.id, 0, &assessment, "stub").await.unwrap();

    let loaded = store.load_recent_assessments(session.id, "stub", 10).await.unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].reasoning, None);
}

#[tokio::test]
async fn save_classification_unique_constraint_fires_on_duplicate() {
    let store = SqliteSessionStore::open_in_memory().await.unwrap();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(Turn { speaker: Speaker::Child, text: "x".into(),
        timestamp: Utc::now(), intent: None, concepts: vec![] });
    store.save_session(&session).await.unwrap();

    let a = EngagementAssessment {
        state: EngagementState::Engaged, confidence: 0.5, reasoning: None,
    };
    store.save_classification(session.id, 0, &a, "stub").await.unwrap();
    // Same classifier on same turn — must error (logic bug to surface)
    let err = store.save_classification(session.id, 0, &a, "stub").await;
    assert!(err.is_err());
}

#[tokio::test]
async fn load_recent_assessments_filters_by_classifier_identifier() {
    let store = SqliteSessionStore::open_in_memory().await.unwrap();
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(Turn { speaker: Speaker::Child, text: "x".into(),
        timestamp: Utc::now(), intent: None, concepts: vec![] });
    store.save_session(&session).await.unwrap();

    let a = EngagementAssessment { state: EngagementState::Engaged,
        confidence: 0.5, reasoning: None };
    store.save_classification(session.id, 0, &a, "stub").await.unwrap();
    store.save_classification(session.id, 0, &a, "llm:haiku").await.unwrap();

    let stub_only = store.load_recent_assessments(session.id, "stub", 10).await.unwrap();
    assert_eq!(stub_only.len(), 1);

    let llm_only = store.load_recent_assessments(session.id, "llm:haiku", 10).await.unwrap();
    assert_eq!(llm_only.len(), 1);
}

#[tokio::test]
async fn load_recent_assessments_respects_k_limit() {
    // Set up a session with 5 child turns, save 5 classifications,
    // load with k=2 — get the most recent 2.
    let store = SqliteSessionStore::open_in_memory().await.unwrap();
    let mut session = Session::new(Uuid::new_v4());
    for i in 0..5 {
        session.add_turn(Turn { speaker: Speaker::Child, text: format!("t{i}"),
            timestamp: Utc::now(), intent: None, concepts: vec![] });
    }
    store.save_session(&session).await.unwrap();
    for i in 0..5 {
        let confidence = 0.1 + (i as f32) * 0.1;
        let a = EngagementAssessment { state: EngagementState::Engaged,
            confidence, reasoning: None };
        store.save_classification(session.id, i, &a, "stub").await.unwrap();
        // Tiny sleep so classified_at is monotonic.
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    let loaded = store.load_recent_assessments(session.id, "stub", 2).await.unwrap();
    assert_eq!(loaded.len(), 2);
    // Must be the most recent two, ordered oldest-first within the result.
    assert!((loaded[0].confidence - 0.4).abs() < 1e-6);
    assert!((loaded[1].confidence - 0.5).abs() < 1e-6);
}

#[tokio::test]
async fn load_recent_assessments_returns_empty_when_no_classifications() {
    let store = SqliteSessionStore::open_in_memory().await.unwrap();
    let session = Session::new(Uuid::new_v4());
    store.save_session(&session).await.unwrap();
    let loaded = store.load_recent_assessments(session.id, "stub", 10).await.unwrap();
    assert!(loaded.is_empty());
}
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p primer-storage save_classification load_recent_assessments
```
Expected: FAIL — methods not implemented on SqliteSessionStore.

- [ ] **Step 4: Implement on `SqliteSessionStore`**

In `src/crates/primer-storage/src/lib.rs`:

```rust
async fn save_classification(
    &self,
    session_id: SessionId,
    turn_index: usize,
    assessment: &EngagementAssessment,
    classifier_identifier: &str,
) -> Result<()> {
    let conn = self.conn.lock().await;
    // Resolve turn_id
    let turn_id: i64 = conn.query_row(
        "SELECT id FROM turns WHERE session_id = ?1 AND turn_index = ?2",
        params![session_id, turn_index as i64],
        |r| r.get(0),
    ).map_err(|e| PrimerError::Storage(
        format!("save_classification: turn_id lookup ({session_id}, {turn_index}): {e}")
    ))?;

    let classifier_id = get_or_create_classifier_id(&conn, classifier_identifier)?;
    let state_id = engagement_state_id(assessment.state);

    conn.execute(
        "INSERT INTO turn_classifications
            (turn_id, engagement_state_id, classifier_id, confidence, reasoning, classified_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            turn_id,
            state_id,
            classifier_id,
            assessment.confidence,
            assessment.reasoning.as_deref(),
            Utc::now(),
        ],
    ).map_err(|e| PrimerError::Storage(
        format!("save_classification: insert: {e}")
    ))?;
    Ok(())
}

async fn load_recent_assessments(
    &self,
    session_id: SessionId,
    classifier_identifier: &str,
    k: usize,
) -> Result<Vec<EngagementAssessment>> {
    let conn = self.conn.lock().await;
    // Resolve classifier_id; if it doesn't exist yet, return empty.
    let classifier_id: Option<i64> = conn.query_row(
        "SELECT id FROM classifiers WHERE identifier = ?1",
        params![classifier_identifier],
        |r| r.get(0),
    ).optional().map_err(|e| PrimerError::Storage(
        format!("load_recent_assessments: classifier lookup: {e}")
    ))?;
    let Some(classifier_id) = classifier_id else { return Ok(vec![]) };

    // Fetch the most recent k, oldest-first within the result window
    // (so callers can directly use it as the trajectory buffer).
    let mut stmt = conn.prepare(
        "SELECT engagement_state_id, confidence, reasoning
         FROM turn_classifications tc
         JOIN turns t ON t.id = tc.turn_id
         WHERE t.session_id = ?1 AND tc.classifier_id = ?2
         ORDER BY tc.classified_at DESC
         LIMIT ?3"
    ).map_err(|e| PrimerError::Storage(format!("load_recent_assessments: prepare: {e}")))?;

    let mut rows: Vec<EngagementAssessment> = stmt
        .query_map(
            params![session_id, classifier_id, k as i64],
            |r| {
                let state_id: i64 = r.get(0)?;
                let confidence: f32 = r.get(1)?;
                let reasoning: Option<String> = r.get(2)?;
                Ok((state_id, confidence, reasoning))
            },
        )
        .map_err(|e| PrimerError::Storage(format!("load_recent_assessments: query: {e}")))?
        .filter_map(|res| {
            let (state_id, confidence, reasoning) = res.ok()?;
            let state = engagement_state_from_id(state_id)?;
            Some(EngagementAssessment { state, confidence, reasoning })
        })
        .collect();
    rows.reverse();  // SELECT was DESC; reverse to oldest-first
    Ok(rows)
}
```

(Note: the `turn_index` column on `turns` is assumed to exist per the existing schema — verify in `primer-storage/src/lib.rs`'s save_session insert. If the column is named differently, adjust.)

- [ ] **Step 5: Run the tests**

```bash
cargo test -p primer-storage save_classification load_recent_assessments
```
Expected: 6 PASS.

- [ ] **Step 6: Run the full workspace test suite**

```bash
cargo test --workspace
```
Expected: all existing tests + 6 new pass. Running total ~127.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(storage): save_classification and load_recent_assessments

Resolves (session_id, turn_index)->turn_id via a join. Lazily creates
classifiers rows. UNIQUE constraint on (turn_id, classifier_id) surfaces
double-classifications by the same classifier as logic bugs."
```

---

## Phase 3 — `primer-classifier` crate

### Task 10: Create the crate skeleton

**Files:**
- Create: `src/crates/primer-classifier/Cargo.toml`
- Create: `src/crates/primer-classifier/src/lib.rs`
- Modify: `src/Cargo.toml`

- [ ] **Step 1: Add the crate to the workspace**

Modify `src/Cargo.toml`:
- Add `"crates/primer-classifier"` to `[workspace] members`.
- Add `primer-classifier = { path = "crates/primer-classifier" }` to `[workspace.dependencies]`.
- Add `serde_json.workspace = true` if not already present (used by Llm classifier output parsing).

- [ ] **Step 2: Create the crate's `Cargo.toml`**

```toml
[package]
name = "primer-classifier"
version = "0.1.0"
edition = "2024"
description = "Engagement classifier trait + implementations for the Primer."

[dependencies]
primer-core.workspace = true
async-trait.workspace = true
tokio = { workspace = true, features = ["sync"] }
tracing.workspace = true
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt", "sync", "time"] }
```

- [ ] **Step 3: Create `src/lib.rs` skeleton**

```rust
//! Engagement classifier — trait + implementations.
//!
//! Classifies a child's engagement state given recent turns and the
//! prior assessment trajectory. One LLM-backed implementation
//! (`LlmEngagementClassifier`) wraps an `Arc<dyn InferenceBackend>` so
//! the crate stays free of `primer-inference` as a build dependency.
//! A stub implementation supports tests.

use async_trait::async_trait;

use primer_core::classifier::{EngagementAssessment, EngagementContext};
use primer_core::error::Result;

pub mod consts;
pub mod settings;
pub mod stub;
pub mod llm;

pub use settings::ClassifierSettings;
pub use stub::StubEngagementClassifier;
pub use llm::LlmEngagementClassifier;

/// Classifier of the child's engagement state.
///
/// Implementations may be LLM-backed (via `Arc<dyn InferenceBackend>`),
/// a fine-tuned local model, or a stub for tests. Soft-fail policy:
/// classifier errors must NEVER propagate up and abort the conversation.
/// Implementations log via `tracing::warn!` and return
/// `EngagementAssessment::unknown_low_confidence(...)` on any failure.
#[async_trait]
pub trait EngagementClassifier: Send + Sync {
    /// Stable identifier carrying impl + model-version. Stored in the
    /// classifiers lookup table; lets us re-classify the archive with a
    /// different classifier later without overwriting old labels.
    /// Examples: "llm:claude-haiku-4-5", "stub", "bert-engagement-v1".
    fn identifier(&self) -> &str;

    async fn classify(&self, ctx: EngagementContext<'_>) -> Result<EngagementAssessment>;
}
```

- [ ] **Step 4: Verify the crate compiles**

```bash
cargo build -p primer-classifier
```
Expected: clean compile (modules don't yet exist; this step requires creating empty stubs first or doing this concurrently with Tasks 11-15).

To make this step pass standalone, create empty placeholder files for the modules:

```rust
// src/crates/primer-classifier/src/consts.rs
// (empty for now)
```

```rust
// src/crates/primer-classifier/src/settings.rs
// (empty for now)
```

```rust
// src/crates/primer-classifier/src/stub.rs
// (empty for now)
```

```rust
// src/crates/primer-classifier/src/llm.rs
// (empty for now)
```

And remove the `pub use` lines from `lib.rs` until the types exist. They'll be added back in subsequent tasks.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(classifier): create primer-classifier crate skeleton with EngagementClassifier trait"
```

---

### Task 11: `ClassifierSettings` and consts

**Files:**
- Modify: `src/crates/primer-classifier/src/consts.rs`
- Modify: `src/crates/primer-classifier/src/settings.rs`

- [ ] **Step 1: Write `consts.rs`**

```rust
//! Default values for `ClassifierSettings`. Per the no-magic-numbers
//! convention, every numeric used by the classifier subsystem is
//! defined here (or in a sibling settings struct field).

pub const DEFAULT_HISTORY_DEPTH: usize = 3;
pub const DEFAULT_BLOCKING_TIMEOUT_MS: u64 = 500;
pub const DEFAULT_CONFIDENCE_THRESHOLD: f32 = 0.6;
pub const DEFAULT_RECENT_CHILD_TURNS_FOR_CLASSIFICATION: usize = 3;
pub const DEFAULT_MAX_CLASSIFIER_OUTPUT_CHARS: usize = 512;
```

- [ ] **Step 2: Write `settings.rs`**

```rust
//! Tunable settings for the classifier subsystem.

use std::time::Duration;

use crate::consts;

#[derive(Debug, Clone)]
pub struct ClassifierSettings {
    pub history_depth: usize,
    pub blocking_timeout: Duration,
    pub confidence_threshold: f32,
    pub recent_child_turns: usize,
    pub max_output_chars: usize,
}

impl Default for ClassifierSettings {
    fn default() -> Self {
        Self {
            history_depth: consts::DEFAULT_HISTORY_DEPTH,
            blocking_timeout: Duration::from_millis(consts::DEFAULT_BLOCKING_TIMEOUT_MS),
            confidence_threshold: consts::DEFAULT_CONFIDENCE_THRESHOLD,
            recent_child_turns: consts::DEFAULT_RECENT_CHILD_TURNS_FOR_CLASSIFICATION,
            max_output_chars: consts::DEFAULT_MAX_CLASSIFIER_OUTPUT_CHARS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_consts() {
        let s = ClassifierSettings::default();
        assert_eq!(s.history_depth, consts::DEFAULT_HISTORY_DEPTH);
        assert_eq!(s.blocking_timeout, Duration::from_millis(consts::DEFAULT_BLOCKING_TIMEOUT_MS));
        assert!((s.confidence_threshold - consts::DEFAULT_CONFIDENCE_THRESHOLD).abs() < 1e-6);
    }
}
```

- [ ] **Step 3: Re-export `ClassifierSettings` from `lib.rs`**

Add to the top of `src/crates/primer-classifier/src/lib.rs`:
```rust
pub use settings::ClassifierSettings;
```

- [ ] **Step 4: Verify**

```bash
cargo test -p primer-classifier defaults_match_consts
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(classifier): ClassifierSettings struct with consts-backed defaults"
```

---

### Task 12: `StubEngagementClassifier`

**Files:**
- Modify: `src/crates/primer-classifier/src/stub.rs`

- [ ] **Step 1: Write the tests first**

```rust
//! Stub classifier — deterministic, test-controllable.

use std::collections::VecDeque;

use async_trait::async_trait;
use tokio::sync::Mutex;

use primer_core::classifier::{EngagementAssessment, EngagementContext};
use primer_core::error::Result;
use primer_core::learner::EngagementState;

use crate::EngagementClassifier;

pub struct StubEngagementClassifier {
    fallback: EngagementAssessment,
    script: Mutex<Option<VecDeque<EngagementAssessment>>>,
}

impl StubEngagementClassifier {
    pub fn new() -> Self {
        Self {
            fallback: EngagementAssessment {
                state: EngagementState::Engaged,
                confidence: 1.0,
                reasoning: None,
            },
            script: Mutex::new(None),
        }
    }

    pub fn with_response(response: EngagementAssessment) -> Self {
        Self { fallback: response, script: Mutex::new(None) }
    }

    pub fn with_script(script: Vec<EngagementAssessment>) -> Self {
        Self {
            fallback: EngagementAssessment {
                state: EngagementState::Engaged,
                confidence: 1.0,
                reasoning: None,
            },
            script: Mutex::new(Some(script.into())),
        }
    }
}

impl Default for StubEngagementClassifier {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl EngagementClassifier for StubEngagementClassifier {
    fn identifier(&self) -> &str { "stub" }

    async fn classify(&self, _ctx: EngagementContext<'_>) -> Result<EngagementAssessment> {
        let mut script = self.script.lock().await;
        if let Some(q) = script.as_mut() {
            if let Some(next) = q.pop_front() {
                return Ok(next);
            }
        }
        Ok(self.fallback.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>() -> EngagementContext<'a> {
        EngagementContext { recent_child_turns: &[], prior_assessments: &[] }
    }

    #[tokio::test]
    async fn default_returns_engaged_with_full_confidence() {
        let c = StubEngagementClassifier::new();
        let a = c.classify(ctx()).await.unwrap();
        assert_eq!(a.state, EngagementState::Engaged);
        assert!((a.confidence - 1.0).abs() < 1e-6);
        assert!(a.reasoning.is_none());
    }

    #[tokio::test]
    async fn with_response_returns_configured() {
        let configured = EngagementAssessment {
            state: EngagementState::FrustratedTrying,
            confidence: 0.7,
            reasoning: Some("test".into()),
        };
        let c = StubEngagementClassifier::with_response(configured.clone());
        let a = c.classify(ctx()).await.unwrap();
        assert_eq!(a.state, configured.state);
    }

    #[tokio::test]
    async fn with_script_returns_in_order_then_falls_back() {
        let c = StubEngagementClassifier::with_script(vec![
            EngagementAssessment {
                state: EngagementState::Reflecting, confidence: 0.5, reasoning: None,
            },
            EngagementAssessment {
                state: EngagementState::FrustratedStuck, confidence: 0.9, reasoning: None,
            },
        ]);
        let a1 = c.classify(ctx()).await.unwrap();
        assert_eq!(a1.state, EngagementState::Reflecting);
        let a2 = c.classify(ctx()).await.unwrap();
        assert_eq!(a2.state, EngagementState::FrustratedStuck);
        // Exhausted — falls back to default Engaged
        let a3 = c.classify(ctx()).await.unwrap();
        assert_eq!(a3.state, EngagementState::Engaged);
    }

    #[tokio::test]
    async fn identifier_is_stub() {
        let c = StubEngagementClassifier::new();
        assert_eq!(c.identifier(), "stub");
    }
}
```

- [ ] **Step 2: Re-export from `lib.rs`**

Add:
```rust
pub use stub::StubEngagementClassifier;
```

- [ ] **Step 3: Run the tests**

```bash
cargo test -p primer-classifier stub
```
Expected: 4 PASS.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(classifier): StubEngagementClassifier with default/with_response/with_script"
```

---

### Task 13: `LlmEngagementClassifier` — scaffolding + identifier

**Files:**
- Modify: `src/crates/primer-classifier/src/llm.rs`

- [ ] **Step 1: Write the scaffolding**

```rust
//! LLM-backed engagement classifier. Wraps any `InferenceBackend`
//! (via Arc<dyn ...>) so memory-constrained devices can reuse the chat
//! backend, while bigger machines can use a separate small model.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use primer_core::classifier::{EngagementAssessment, EngagementContext};
use primer_core::error::Result;
use primer_core::inference::InferenceBackend;
use primer_core::learner::EngagementState;

use crate::settings::ClassifierSettings;
use crate::EngagementClassifier;

pub struct LlmEngagementClassifier {
    backend: Arc<dyn InferenceBackend>,
    model: String,
    settings: ClassifierSettings,
    identifier: String,
}

impl LlmEngagementClassifier {
    pub fn new(
        backend: Arc<dyn InferenceBackend>,
        model: String,
        settings: ClassifierSettings,
    ) -> Self {
        let identifier = format!("llm:{model}");
        Self { backend, model, settings, identifier }
    }
}

#[async_trait]
impl EngagementClassifier for LlmEngagementClassifier {
    fn identifier(&self) -> &str { &self.identifier }

    async fn classify(&self, ctx: EngagementContext<'_>) -> Result<EngagementAssessment> {
        // Implemented in Task 14.
        let _ = (ctx, &self.backend, &self.model, &self.settings);
        Ok(EngagementAssessment::unknown_low_confidence("not yet implemented"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A tiny test backend that returns canned text for `generate`.
    struct CannedBackend(String);

    #[async_trait]
    impl InferenceBackend for CannedBackend {
        async fn generate(
            &self,
            _prompt: &primer_core::inference::Prompt,
            _model: &str,
        ) -> Result<String> {
            Ok(self.0.clone())
        }
        // ... any other required methods get default impls or trivial returns;
        // refer to the actual InferenceBackend trait shape and stub the rest.
    }

    #[test]
    fn identifier_includes_model_name() {
        let backend = Arc::new(CannedBackend("{}".into())) as Arc<dyn InferenceBackend>;
        let c = LlmEngagementClassifier::new(
            backend,
            "claude-haiku-4-5".into(),
            ClassifierSettings::default(),
        );
        assert_eq!(c.identifier(), "llm:claude-haiku-4-5");
    }
}
```

(The `CannedBackend` test fixture must implement every method on `InferenceBackend`. Refer to `primer-core/src/inference.rs` for the full trait shape and stub each method appropriately.)

- [ ] **Step 2: Re-export from `lib.rs`**

```rust
pub use llm::LlmEngagementClassifier;
```

- [ ] **Step 3: Run the test**

```bash
cargo test -p primer-classifier identifier_includes_model_name
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(classifier): LlmEngagementClassifier scaffolding (identifier only)"
```

---

### Task 14: `LlmEngagementClassifier::classify` — prompt build, parse, soft-fail

**Files:**
- Modify: `src/crates/primer-classifier/src/llm.rs`

- [ ] **Step 1: Write the implementation tests first**

Add to `llm.rs::tests`:

```rust
#[tokio::test]
async fn classify_parses_well_formed_json() {
    let backend = Arc::new(CannedBackend(
        r#"{"state": "FrustratedStuck", "confidence": 0.85, "reasoning": "child says i don't know"}"#.into()
    )) as Arc<dyn InferenceBackend>;
    let c = LlmEngagementClassifier::new(
        backend, "test-model".into(), ClassifierSettings::default()
    );
    let ctx = EngagementContext { recent_child_turns: &[], prior_assessments: &[] };
    let a = c.classify(ctx).await.unwrap();
    assert_eq!(a.state, EngagementState::FrustratedStuck);
    assert!((a.confidence - 0.85).abs() < 1e-6);
    assert_eq!(a.reasoning.as_deref(), Some("child says i don't know"));
}

#[tokio::test]
async fn classify_extracts_json_from_wrapper_text() {
    let backend = Arc::new(CannedBackend(
        r#"Here is my analysis: {"state": "Engaged", "confidence": 0.9, "reasoning": "good"} — hope this helps!"#.into()
    )) as Arc<dyn InferenceBackend>;
    let c = LlmEngagementClassifier::new(
        backend, "test-model".into(), ClassifierSettings::default()
    );
    let ctx = EngagementContext { recent_child_turns: &[], prior_assessments: &[] };
    let a = c.classify(ctx).await.unwrap();
    assert_eq!(a.state, EngagementState::Engaged);
}

#[tokio::test]
async fn classify_returns_unknown_on_unparseable_output() {
    let backend = Arc::new(CannedBackend("not json at all".into()))
        as Arc<dyn InferenceBackend>;
    let c = LlmEngagementClassifier::new(
        backend, "test-model".into(), ClassifierSettings::default()
    );
    let ctx = EngagementContext { recent_child_turns: &[], prior_assessments: &[] };
    let a = c.classify(ctx).await.unwrap();
    assert_eq!(a.state, EngagementState::Unknown);
    assert_eq!(a.confidence, 0.0);
    assert!(a.reasoning.is_some());
}

#[tokio::test]
async fn classify_returns_unknown_on_invalid_state_name() {
    let backend = Arc::new(CannedBackend(
        r#"{"state": "EnthusiastiCallyEngaged", "confidence": 1.0, "reasoning": ""}"#.into()
    )) as Arc<dyn InferenceBackend>;
    let c = LlmEngagementClassifier::new(
        backend, "test-model".into(), ClassifierSettings::default()
    );
    let ctx = EngagementContext { recent_child_turns: &[], prior_assessments: &[] };
    let a = c.classify(ctx).await.unwrap();
    assert_eq!(a.state, EngagementState::Unknown);
}

#[tokio::test]
async fn classify_returns_unknown_on_out_of_range_confidence() {
    let backend = Arc::new(CannedBackend(
        r#"{"state": "Engaged", "confidence": 2.5, "reasoning": ""}"#.into()
    )) as Arc<dyn InferenceBackend>;
    let c = LlmEngagementClassifier::new(
        backend, "test-model".into(), ClassifierSettings::default()
    );
    let ctx = EngagementContext { recent_child_turns: &[], prior_assessments: &[] };
    let a = c.classify(ctx).await.unwrap();
    assert_eq!(a.state, EngagementState::Unknown);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p primer-classifier classify_parses_well_formed_json classify_extracts_json_from_wrapper_text classify_returns_unknown_on_unparseable_output classify_returns_unknown_on_invalid_state_name classify_returns_unknown_on_out_of_range_confidence
```
Expected: FAIL — all five.

- [ ] **Step 3: Implement `classify`**

Replace the placeholder body:

```rust
async fn classify(&self, ctx: EngagementContext<'_>) -> Result<EngagementAssessment> {
    let prompt = build_classification_prompt(&ctx);
    let raw = match self.backend.generate(&prompt, &self.model).await {
        Ok(r) => r,
        Err(e) => {
            warn!(classifier = %self.identifier, error = ?e, "classifier backend call failed");
            return Ok(EngagementAssessment::unknown_low_confidence(
                format!("backend error: {e}")
            ));
        }
    };

    let truncated = if raw.len() > self.settings.max_output_chars {
        &raw[..self.settings.max_output_chars]
    } else {
        &raw[..]
    };

    match parse_classification_output(truncated) {
        Ok(a) => Ok(a),
        Err(reason) => {
            let snippet: String = truncated.chars().take(200).collect();
            warn!(classifier = %self.identifier, raw = %snippet, %reason, "classifier output unparseable");
            Ok(EngagementAssessment::unknown_low_confidence(reason))
        }
    }
}
```

Add the helpers at module level:

```rust
use primer_core::inference::{Message, Prompt, Role};
use primer_core::conversation::Speaker;

fn build_classification_prompt(ctx: &EngagementContext) -> Prompt {
    let trajectory = if ctx.prior_assessments.is_empty() {
        "(no prior trajectory)".to_string()
    } else {
        ctx.prior_assessments
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let turns_ago = ctx.prior_assessments.len() - i;
                let r = a.reasoning.as_deref().unwrap_or("");
                format!("{turns_ago} turn(s) ago: {:?} ({:.2}) — {r}", a.state, a.confidence)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let recent = ctx.recent_child_turns
        .iter()
        .filter(|t| t.speaker == Speaker::Child)
        .map(|t| t.text.as_str())
        .collect::<Vec<_>>()
        .join("\n---\n");

    let system = "You classify a child's engagement state in a Socratic learning \
        conversation. Output ONLY valid JSON of the form: \
        {\"state\": \"<one of: Engaged, Reflecting, FrustratedStuck, FrustratedTrying, \
        Disengaging, Unknown>\", \"confidence\": 0.0-1.0, \"reasoning\": \
        \"<one short sentence>\"} — no other text.".to_string();

    let user = format!(
        "Recent trajectory (oldest first):\n{trajectory}\n\nMost recent child responses:\n{recent}\n\nClassify the CURRENT engagement state."
    );

    Prompt {
        system,
        messages: vec![Message { role: Role::User, content: user }],
    }
}

#[derive(serde::Deserialize)]
struct ClassifierOutput {
    state: String,
    confidence: f32,
    #[serde(default)]
    reasoning: Option<String>,
}

fn parse_classification_output(raw: &str) -> std::result::Result<EngagementAssessment, String> {
    let json_str = extract_first_json_object(raw)
        .ok_or_else(|| "no JSON object found in output".to_string())?;
    let parsed: ClassifierOutput = serde_json::from_str(json_str)
        .map_err(|e| format!("JSON parse error: {e}"))?;

    if !(0.0..=1.0).contains(&parsed.confidence) {
        return Err(format!("confidence {} out of range [0.0, 1.0]", parsed.confidence));
    }

    let state = match parsed.state.as_str() {
        "Engaged" => EngagementState::Engaged,
        "Reflecting" => EngagementState::Reflecting,
        "FrustratedStuck" => EngagementState::FrustratedStuck,
        "FrustratedTrying" => EngagementState::FrustratedTrying,
        "Disengaging" => EngagementState::Disengaging,
        "Unknown" => EngagementState::Unknown,
        other => return Err(format!("unknown state name: {other:?}")),
    };

    Ok(EngagementAssessment {
        state,
        confidence: parsed.confidence,
        reasoning: parsed.reasoning,
    })
}

/// Extract the first balanced JSON object from `raw`. Tolerates wrapper
/// text before/after, and JSON containing nested braces. Returns the
/// substring including the outer braces, or None if not found.
fn extract_first_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let bytes = raw.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if escape { escape = false; continue; }
        if in_string {
            match b {
                b'\\' => escape = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&raw[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p primer-classifier classify
```
Expected: 5 PASS.

- [ ] **Step 5: Run the full test suite**

```bash
cargo test --workspace
```
Expected: all pass. Running total ~136.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(classifier): LlmEngagementClassifier with prompt building, parsing, and soft-fail

Soft-fail on backend error, JSON parse failure, invalid state name, or
out-of-range confidence — returns Unknown(0.0) and warns. Tolerates
wrapper text via balanced-brace JSON extraction."
```

---

## Phase 4 — `decide_intent` and `prompt_builder` updates

### Task 15: Extract magic numbers into `primer-pedagogy::consts`

**Files:**
- Create: `src/crates/primer-pedagogy/src/consts.rs`
- Modify: `src/crates/primer-pedagogy/src/lib.rs`
- Modify: `src/crates/primer-pedagogy/src/prompt_builder.rs`

- [ ] **Step 1: Write `consts.rs`**

```rust
//! Pedagogy-layer constants. Per the no-magic-numbers convention,
//! invariant numeric values live here (not inline).

/// Word-count threshold below which a child's last turn routes to
/// ComprehensionCheck (instead of Extension or SocraticQuestion).
/// Currently the same heuristic the placeholder used; kept stable here
/// until comprehension assessment lands as a separate Phase 0.3 piece.
pub const SHORT_TURN_WORD_BOUNDARY: usize = 10;

/// How many recent turns `extract_active_concepts` scans for active
/// concepts when deciding Extension intent.
pub const ACTIVE_CONCEPT_LOOKBACK: usize = 4;
```

- [ ] **Step 2: Add `pub mod consts;` to `lib.rs`**

```rust
pub mod consts;
```

- [ ] **Step 3: Replace inline literals in `prompt_builder.rs`**

In `decide_intent`:
```rust
if last.text.split_whitespace().count() < crate::consts::SHORT_TURN_WORD_BOUNDARY {
```

In `extract_active_concepts`:
```rust
session.recent_turns(crate::consts::ACTIVE_CONCEPT_LOOKBACK)
```

- [ ] **Step 4: Verify**

```bash
cargo test --workspace
```
Expected: all 119+ tests still pass.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(pedagogy): extract magic numbers into consts module"
```

---

### Task 16: `is_factual_question` helper

**Files:**
- Modify: `src/crates/primer-pedagogy/src/prompt_builder.rs`

- [ ] **Step 1: Write the tests**

Add to `prompt_builder.rs::tests`:

```rust
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
```

- [ ] **Step 2: Run tests to verify failure**

```bash
cargo test -p primer-pedagogy is_factual_question
```
Expected: FAIL — function not defined.

- [ ] **Step 3: Implement**

Add to `prompt_builder.rs` (private helper, near `decide_intent`):

```rust
fn is_factual_question(text: &str) -> bool {
    const FACTUAL_PREFIXES: &[&str] = &[
        "what is ", "what are ", "what's ", "what does ",
        "how does ", "how do ",  "how is ",  "how are ",
    ];
    let lowered = text.trim().to_lowercase();
    FACTUAL_PREFIXES.iter().any(|p| lowered.starts_with(p))
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p primer-pedagogy is_factual_question
```
Expected: 5 PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(pedagogy): is_factual_question helper for Gap 2"
```

---

### Task 17: `decide_intent_at` — Gap 1 (FrustratedStuck/FrustratedTrying routing)

**Files:**
- Modify: `src/crates/primer-pedagogy/src/prompt_builder.rs`

- [ ] **Step 1: Update existing tests + add new positive case**

Tests to update (rename, adjust assertions):

```rust
// Existing test renamed:
#[test]
fn frustrated_stuck_returns_scaffolding() {
    let learner = learner_with(EngagementState::FrustratedStuck, vec![]);
    let session = empty_session();
    assert_eq!(decide_intent(&learner, &session), PedagogicalIntent::Scaffolding);
}

#[test]
fn frustrated_stuck_overrides_short_child_turn() {
    let learner = learner_with(EngagementState::FrustratedStuck, vec![]);
    let mut session = empty_session();
    session.add_turn(child_turn("yes", vec![]));
    assert_eq!(decide_intent(&learner, &session), PedagogicalIntent::Scaffolding);
}

// NEW positive cases for FrustratedTrying:
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
```

Tests to **delete**:
- `frustrated_does_not_currently_return_encouragement` — replaced by the positive case above.

Tests to update reference (any test still using `EngagementState::Frustrated`): change to `EngagementState::FrustratedStuck` to preserve current routing.

- [ ] **Step 2: Run the tests to verify failures**

```bash
cargo test -p primer-pedagogy frustrated
```
Expected: `frustrated_trying_*` FAIL because decide_intent doesn't yet route FrustratedTrying.

- [ ] **Step 3: Update `decide_intent`**

Replace the existing `match learner.current_engagement` block:

```rust
match learner.current_engagement {
    EngagementState::FrustratedStuck => return PedagogicalIntent::Scaffolding,
    EngagementState::FrustratedTrying => return PedagogicalIntent::Encouragement,
    EngagementState::Disengaging => return PedagogicalIntent::SessionClose,  // Task 18 changes this
    EngagementState::Engaged
    | EngagementState::Reflecting
    | EngagementState::Unknown => { /* fall through */ }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p primer-pedagogy frustrated
```
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(pedagogy): decide_intent routes FrustratedTrying -> Encouragement (Gap 1)"
```

---

### Task 18: `decide_intent_at` — Gap 3 (session-length-aware Disengaging)

**Files:**
- Modify: `src/crates/primer-pedagogy/src/prompt_builder.rs`

- [ ] **Step 1: Refactor `decide_intent` to wrap a `_at` variant**

Replace the existing function:

```rust
pub fn decide_intent(learner: &LearnerModel, session: &Session) -> PedagogicalIntent {
    decide_intent_at(learner, session, chrono::Utc::now())
}

pub fn decide_intent_at(
    learner: &LearnerModel,
    session: &Session,
    now: chrono::DateTime<chrono::Utc>,
) -> PedagogicalIntent {
    // ... existing body, with `now` available for Disengaging branch ...
}
```

- [ ] **Step 2: Update the Disengaging branch**

Replace:
```rust
EngagementState::Disengaging => return PedagogicalIntent::SessionClose,
```
With:
```rust
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
```

- [ ] **Step 3: Update existing Disengaging tests + add new ones**

Replace `disengaging_returns_session_close`:

```rust
fn make_session_started(seconds_ago: i64) -> Session {
    let mut s = Session::new(Uuid::new_v4());
    s.started_at = Utc::now() - chrono::Duration::seconds(seconds_ago);
    s
}

#[test]
fn disengaging_late_returns_session_close() {
    let learner = learner_with(EngagementState::Disengaging, vec![]);
    let session = make_session_started(60 * 60);  // 1 hour ago
    let now = Utc::now();
    assert_eq!(
        decide_intent_at(&learner, &session, now),
        PedagogicalIntent::SessionClose,
    );
}

#[test]
fn disengaging_early_returns_encouragement() {
    let learner = learner_with(EngagementState::Disengaging, vec![]);
    let session = make_session_started(60);  // 1 minute ago
    let now = Utc::now();
    assert_eq!(
        decide_intent_at(&learner, &session, now),
        PedagogicalIntent::Encouragement,
    );
}

#[test]
fn disengaging_at_threshold_returns_session_close() {
    // At exactly the threshold (5 min default) — should go SessionClose
    // because the comparison is `<` (strict).
    let learner = learner_with(EngagementState::Disengaging, vec![]);
    let started = Utc::now() - chrono::Duration::seconds(
        primer_core::learner::DEFAULT_EARLY_DISENGAGEMENT_SECS as i64
    );
    let mut session = empty_session();
    session.started_at = started;
    assert_eq!(
        decide_intent_at(&learner, &session, Utc::now()),
        PedagogicalIntent::SessionClose,
    );
}

#[test]
fn disengaging_just_after_threshold_returns_session_close() {
    let learner = learner_with(EngagementState::Disengaging, vec![]);
    let started = Utc::now() - chrono::Duration::seconds(
        primer_core::learner::DEFAULT_EARLY_DISENGAGEMENT_SECS as i64 + 60
    );
    let mut session = empty_session();
    session.started_at = started;
    assert_eq!(
        decide_intent_at(&learner, &session, Utc::now()),
        PedagogicalIntent::SessionClose,
    );
}
```

Update `disengaging_overrides_short_child_turn_branch` similarly to use a backdated start.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p primer-pedagogy disengaging
```
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(pedagogy): decide_intent_at session-length-aware Disengaging routing (Gap 3)"
```

---

### Task 19: `decide_intent_at` — Gap 2 (factual-question routing)

**Files:**
- Modify: `src/crates/primer-pedagogy/src/prompt_builder.rs`

- [ ] **Step 1: Write the tests**

```rust
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
    // "what is X?" still gets the engagement branch.
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
```

Delete `factual_question_pattern_does_not_currently_return_direct_answer`.

- [ ] **Step 2: Verify tests fail**

```bash
cargo test -p primer-pedagogy factual_question
```
Expected: positive cases FAIL (no factual-question routing yet).

- [ ] **Step 3: Add the factual-question branch to `decide_intent_at`**

Insert after the engagement-state match, before the existing short-turn branch:

```rust
if let Some(last) = session.turns.last() {
    if last.speaker == Speaker::Child {
        // Gap 2: factual-question pattern routing
        if is_factual_question(&last.text) {
            let prior_was_direct_answer = session.turns
                .iter()
                .rev()
                .skip(1)
                .find(|t| t.speaker == Speaker::Primer)
                .and_then(|t| t.intent)
                .map(|i| i == PedagogicalIntent::DirectAnswer)
                .unwrap_or(false);
            return if prior_was_direct_answer {
                PedagogicalIntent::AnswerThenPivot
            } else {
                PedagogicalIntent::DirectAnswer
            };
        }

        // ... existing short-turn branch follows ...
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p primer-pedagogy
```
Expected: all PASS, including all `factual_question_*` and the unchanged tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(pedagogy): decide_intent factual-question pattern routing (Gap 2)

DirectAnswer on first factual question; AnswerThenPivot when the
immediately previous Primer turn already used DirectAnswer.
Engagement-state precedence preserved."
```

---

### Task 20: `prompt_builder` engagement_note for new variants

**Files:**
- Modify: `src/crates/primer-pedagogy/src/prompt_builder.rs`

- [ ] **Step 1: Write tests**

```rust
#[test]
fn build_prompt_includes_engagement_note_for_frustrated_stuck() {
    let learner = learner_with(EngagementState::FrustratedStuck, vec![]);
    let session = empty_session();
    let prompt = build_prompt(
        &learner, &session,
        PedagogicalIntent::Scaffolding,
        &[], "", &[], 20,
    );
    assert!(prompt.system.contains("appears frustrated"),
        "engagement note should appear for FrustratedStuck");
}

#[test]
fn build_prompt_includes_engagement_note_for_frustrated_trying() {
    let learner = learner_with(EngagementState::FrustratedTrying, vec![]);
    let session = empty_session();
    let prompt = build_prompt(
        &learner, &session,
        PedagogicalIntent::Encouragement,
        &[], "", &[], 20,
    );
    assert!(prompt.system.contains("appears frustrated"),
        "engagement note should appear for FrustratedTrying");
}
```

- [ ] **Step 2: Run tests to verify failure**

```bash
cargo test -p primer-pedagogy build_prompt_includes_engagement_note_for_frustrated
```
Expected: FAIL — current match arms don't cover the new variants.

- [ ] **Step 3: Update `engagement_note` match in `build_system_prompt`**

```rust
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
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p primer-pedagogy build_prompt_includes_engagement_note_for_frustrated
```
Expected: 2 PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(pedagogy): engagement note covers FrustratedStuck and FrustratedTrying"
```

---

## Phase 5 — DialogueManager flow

### Task 21: `DialogueManager` struct changes

**Files:**
- Modify: `src/crates/primer-pedagogy/Cargo.toml`
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager.rs`

- [ ] **Step 1: Add the dependency**

In `src/crates/primer-pedagogy/Cargo.toml`:
```toml
[dependencies]
primer-classifier.workspace = true
```

- [ ] **Step 2: Add fields to `DialogueManager`**

In `dialogue_manager.rs`. **Important:** `classifier` and `storage` need to outlive any single turn (spawned classifier task captures them) — so they MUST be `Arc<dyn ...>`, not `&dyn ...`. This is the divergence from the spec's "held by reference" wording: `tokio::spawn` requires `'static` futures, and there's no scoped-spawn dep in this workspace. Backend and knowledge stay as borrowed references (they're used only synchronously inside the DialogueManager method body; never captured into the spawn).

Convert `storage` from `Option<&'a dyn SessionStore>` to `Option<Arc<dyn SessionStore>>` as part of this task — the CLI changes accordingly.

```rust
use std::sync::Arc;
use primer_classifier::{ClassifierSettings, EngagementClassifier};
use primer_core::classifier::EngagementAssessment;
use primer_core::storage::SessionStore;
use tokio::task::JoinHandle;

pub struct DialogueManager<'a> {
    // Synchronous-only deps stay borrowed:
    backend: &'a dyn InferenceBackend,
    knowledge: &'a dyn KnowledgeBase,
    // Captured by the spawned classifier task — Arc:
    storage: Option<Arc<dyn SessionStore>>,
    classifier: Arc<dyn EngagementClassifier>,
    classifier_settings: ClassifierSettings,
    classify_task: Option<JoinHandle<Option<EngagementAssessment>>>,
    // ... whatever existing session/learner state fields exist ...
}
```

- [ ] **Step 3: Update the constructor**

```rust
pub fn new(
    backend: &'a dyn InferenceBackend,
    knowledge: &'a dyn KnowledgeBase,
    storage: Option<Arc<dyn SessionStore>>,
    classifier: Arc<dyn EngagementClassifier>,
    classifier_settings: ClassifierSettings,
) -> Self {
    Self {
        backend,
        knowledge,
        storage,
        classifier,
        classifier_settings,
        classify_task: None,
        // ... existing field initializers ...
    }
}
```

- [ ] **Step 4: Update every call site**

The compiler will surface them. Likely sites:
- `src/crates/primer-cli/src/main.rs` (storage construction now wraps in `Arc::new(...)`)
- `src/crates/primer-pedagogy/src/dialogue_manager.rs::tests` — every test that constructs a `DialogueManager`

For tests:
```rust
let classifier: Arc<dyn EngagementClassifier> = Arc::new(StubEngagementClassifier::new());
let storage: Option<Arc<dyn SessionStore>> = Some(Arc::new(SqliteSessionStore::open_in_memory().await.unwrap()));
let mut dm = DialogueManager::new(&backend, &knowledge, storage, classifier, ClassifierSettings::default());
```

- [ ] **Step 5: Verify build**

```bash
cargo build --workspace
cargo test --workspace
```
Expected: clean build, all 130+ tests pass (no behavior change yet).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(pedagogy): DialogueManager holds Arc classifier + Arc storage

Both classifier and storage need to outlive any single turn (the post-response
classifier task captures them), and tokio::spawn requires 'static futures —
so they must be Arc<dyn ...> rather than borrowed references. Backend and
knowledge stay as &dyn since they're only used synchronously inside method
bodies. CLI updated to wrap storage in Arc::new at construction."
```

---

### Task 22: `apply_assessment` and `await_pending_classification`

**Files:**
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager.rs`

- [ ] **Step 1: Write tests**

```rust
#[tokio::test]
async fn apply_assessment_pushes_to_recent_assessments() {
    let mut learner = test_learner();
    let settings = ClassifierSettings::default();
    let a = EngagementAssessment {
        state: EngagementState::Reflecting, confidence: 0.9, reasoning: None,
    };
    apply_assessment(&mut learner, a.clone(), &settings);
    assert_eq!(learner.recent_assessments.len(), 1);
    assert_eq!(learner.recent_assessments[0].state, EngagementState::Reflecting);
}

#[tokio::test]
async fn apply_assessment_evicts_oldest_when_buffer_full() {
    let mut learner = test_learner();
    let settings = ClassifierSettings { history_depth: 2, ..Default::default() };
    for state in [EngagementState::Engaged, EngagementState::Reflecting, EngagementState::FrustratedStuck] {
        apply_assessment(&mut learner, EngagementAssessment {
            state, confidence: 0.9, reasoning: None,
        }, &settings);
    }
    assert_eq!(learner.recent_assessments.len(), 2);
    assert_eq!(learner.recent_assessments[0].state, EngagementState::Reflecting);
    assert_eq!(learner.recent_assessments[1].state, EngagementState::FrustratedStuck);
}

#[tokio::test]
async fn apply_assessment_updates_current_engagement_when_confident() {
    let mut learner = test_learner();
    let settings = ClassifierSettings { confidence_threshold: 0.6, ..Default::default() };
    apply_assessment(&mut learner, EngagementAssessment {
        state: EngagementState::FrustratedTrying, confidence: 0.8, reasoning: None,
    }, &settings);
    assert_eq!(learner.current_engagement, EngagementState::FrustratedTrying);
}

#[tokio::test]
async fn apply_assessment_keeps_current_engagement_when_low_confidence() {
    let mut learner = test_learner();
    let initial = learner.current_engagement;
    let settings = ClassifierSettings { confidence_threshold: 0.6, ..Default::default() };
    apply_assessment(&mut learner, EngagementAssessment {
        state: EngagementState::FrustratedTrying, confidence: 0.3, reasoning: None,
    }, &settings);
    assert_eq!(learner.current_engagement, initial,
        "low-confidence assessment must NOT change current_engagement");
    assert_eq!(learner.recent_assessments.len(), 1,
        "low-confidence assessment IS still recorded in history");
}

fn test_learner() -> LearnerModel {
    LearnerModel {
        profile: LearnerProfile {
            id: Uuid::new_v4(),
            name: "Test".into(),
            age: 8,
            languages: vec!["en".into()],
            created_at: Utc::now(),
            last_active: Utc::now(),
        },
        concepts: vec![],
        preferences: LearningPreferences::default(),
        current_engagement: EngagementState::Engaged,
        recent_assessments: vec![],
    }
}
```

- [ ] **Step 2: Run tests to verify failure**

```bash
cargo test -p primer-pedagogy apply_assessment
```
Expected: FAIL — function not defined.

- [ ] **Step 3: Implement `apply_assessment`**

In `dialogue_manager.rs`:

```rust
pub(crate) fn apply_assessment(
    learner: &mut LearnerModel,
    a: EngagementAssessment,
    settings: &ClassifierSettings,
) {
    learner.recent_assessments.push(a.clone());
    while learner.recent_assessments.len() > settings.history_depth {
        learner.recent_assessments.remove(0);
    }
    if a.confidence >= settings.confidence_threshold {
        learner.current_engagement = a.state;
    }
}
```

- [ ] **Step 4: Implement `await_pending_classification`**

```rust
impl<'a> DialogueManager<'a> {
    async fn await_pending_classification(&mut self, learner: &mut LearnerModel) {
        let Some(task) = self.classify_task.take() else { return };
        let abort = task.abort_handle();
        let timeout = self.classifier_settings.blocking_timeout;
        match tokio::time::timeout(timeout, task).await {
            Ok(Ok(Some(assessment))) => apply_assessment(learner, assessment, &self.classifier_settings),
            Ok(Ok(None)) => { /* soft failure; nothing to apply */ }
            Ok(Err(e)) => tracing::warn!(error = ?e, "classifier task panicked"),
            Err(_) => {
                abort.abort();
                tracing::debug!("classifier exceeded blocking timeout — proceeding with stale state");
            }
        }
    }
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p primer-pedagogy apply_assessment
```
Expected: 4 PASS.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(pedagogy): apply_assessment and await_pending_classification helpers"
```

---

### Task 23: Wire classifier into `respond_to_streaming`

**Files:**
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager.rs`

- [ ] **Step 1: Write integration tests**

```rust
#[tokio::test]
async fn respond_to_streaming_spawns_classify_task() {
    let backend = StubBackend::new();
    let knowledge = StubKnowledge::new();
    let storage = SqliteSessionStore::open_in_memory().await.unwrap();
    let classifier = StubEngagementClassifier::with_response(EngagementAssessment {
        state: EngagementState::FrustratedTrying, confidence: 0.9, reasoning: Some("test".into()),
    });
    let mut dm = DialogueManager::new(
        &backend, &knowledge, Some(&storage), &classifier, ClassifierSettings::default(),
    );
    let mut learner = test_learner();
    dm.open_session(&mut learner).await.unwrap();
    let _response = dm.respond_to(&mut learner, "what is this").await.unwrap();
    // Force the classify task to complete
    dm.await_pending_classification_for_test(&mut learner).await;
    // Assessment must be persisted
    let recent = storage.load_recent_assessments(
        dm.session_id_for_test(), "stub", 10
    ).await.unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].state, EngagementState::FrustratedTrying);
}

#[tokio::test]
async fn next_turn_uses_previous_classification_for_intent() {
    let backend = StubBackend::new();
    let knowledge = StubKnowledge::new();
    let storage = SqliteSessionStore::open_in_memory().await.unwrap();
    // Scripted classifier: first reads child as FrustratedTrying.
    let classifier = StubEngagementClassifier::with_script(vec![
        EngagementAssessment {
            state: EngagementState::FrustratedTrying, confidence: 0.9, reasoning: None,
        }
    ]);
    let mut dm = DialogueManager::new(
        &backend, &knowledge, Some(&storage), &classifier,
        ClassifierSettings { blocking_timeout: Duration::from_secs(5), ..Default::default() },
    );
    let mut learner = test_learner();
    dm.open_session(&mut learner).await.unwrap();
    let _r1 = dm.respond_to(&mut learner, "uhh i don't know").await.unwrap();
    // Classifier task must complete before we send the second turn
    dm.await_pending_classification_for_test(&mut learner).await;
    assert_eq!(learner.current_engagement, EngagementState::FrustratedTrying);
}
```

(Add a `pub(crate) async fn await_pending_classification_for_test` and `pub(crate) fn session_id_for_test` to expose the internals to the test module — they go behind `#[cfg(test)]`.)

- [ ] **Step 2: Run tests to verify failure**

```bash
cargo test -p primer-pedagogy respond_to_streaming_spawns next_turn_uses_previous
```
Expected: FAIL — task not yet spawned.

- [ ] **Step 3: Modify `respond_to_streaming`**

At the very start (before the existing logic), call:
```rust
self.await_pending_classification(learner).await;
```

After `save_session(&session)` (existing line near the end), spawn the classifier task. Both `classifier` and `storage` are already `Arc` (per Task 21's struct shape), so cloning into the spawn is straightforward:

```rust
let store = self.storage.clone();
let classifier = Arc::clone(&self.classifier);
let session_id = session.id;
let child_turn_index = session.turns.len() - 2;  // child turn was just before primer turn
let recent_child_turns: Vec<Turn> = session.turns
    .iter()
    .rev()
    .filter(|t| t.speaker == Speaker::Child)
    .take(self.classifier_settings.recent_child_turns)
    .cloned()
    .collect::<Vec<_>>()
    .into_iter()
    .rev()
    .collect();
let prior_assessments = learner.recent_assessments.clone();

let task = tokio::spawn(async move {
    let ctx = EngagementContext {
        recent_child_turns: &recent_child_turns,
        prior_assessments: &prior_assessments,
    };
    match classifier.classify(ctx).await {
        Ok(a) => {
            if let Some(store) = store {
                if let Err(e) = store.save_classification(
                    session_id, child_turn_index, &a, classifier.identifier(),
                ).await {
                    tracing::warn!(error = ?e, "save_classification failed");
                }
            }
            Some(a)
        }
        Err(e) => {
            tracing::warn!(error = ?e, "classifier returned error");
            None
        }
    }
});
self.classify_task = Some(task);
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p primer-pedagogy
```
Expected: all pass, including the two new tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(pedagogy): wire classifier into respond_to_streaming flow

Each child turn produces one classification, run after the Primer's
response completes, persisted via save_classification, and applied to
the next turn's intent decision via await_pending_classification."
```

---

### Task 24: `resume_session` rehydration

**Files:**
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager.rs`

- [ ] **Step 1: Write a test**

```rust
#[tokio::test]
async fn resume_session_rehydrates_recent_assessments() {
    let backend = StubBackend::new();
    let knowledge = StubKnowledge::new();
    let storage = Arc::new(SqliteSessionStore::open_in_memory().await.unwrap());
    let classifier = Arc::new(StubEngagementClassifier::new()) as Arc<dyn EngagementClassifier>;
    let settings = ClassifierSettings::default();

    // Pre-seed: save a session with one turn and one classification.
    let mut session = Session::new(Uuid::new_v4());
    session.add_turn(Turn { speaker: Speaker::Child, text: "x".into(),
        timestamp: Utc::now(), intent: None, concepts: vec![] });
    storage.save_session(&session).await.unwrap();
    storage.save_classification(session.id, 0, &EngagementAssessment {
        state: EngagementState::FrustratedTrying, confidence: 0.9, reasoning: Some("r".into()),
    }, "stub").await.unwrap();

    // Now resume.
    let mut dm = DialogueManager::new(&backend, &knowledge, Some(storage.clone()), classifier, settings);
    let mut learner = test_learner();
    let loaded = storage.load_session(session.id).await.unwrap().expect("must load");
    dm.resume_session(&mut learner, loaded).await.unwrap();

    assert_eq!(learner.recent_assessments.len(), 1);
    assert_eq!(learner.recent_assessments[0].state, EngagementState::FrustratedTrying);
    assert_eq!(learner.current_engagement, EngagementState::FrustratedTrying);
}
```

- [ ] **Step 2: Run test to verify failure**

```bash
cargo test -p primer-pedagogy resume_session_rehydrates
```
Expected: FAIL.

- [ ] **Step 3: Add rehydration to `resume_session`**

After the existing `resume_session` body (which loads session, etc.), add:

```rust
if let Some(store) = self.storage.as_ref() {
    let recent = store.load_recent_assessments(
        session.id,
        self.classifier.identifier(),
        self.classifier_settings.history_depth,
    ).await?;
    if let Some(latest) = recent.last() {
        learner.current_engagement = latest.state;
    }
    learner.recent_assessments = recent;
}
```

- [ ] **Step 4: Run test**

```bash
cargo test -p primer-pedagogy resume_session_rehydrates
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(pedagogy): resume_session rehydrates recent_assessments from store"
```

---

## Phase 6 — CLI integration

### Task 25: New CLI flags

**Files:**
- Modify: `src/crates/primer-cli/Cargo.toml`
- Modify: `src/crates/primer-cli/src/main.rs`

- [ ] **Step 1: Add the dependency**

In `Cargo.toml`:
```toml
primer-classifier.workspace = true
```

- [ ] **Step 2: Add CLI flags**

Find the `Args` struct in `main.rs` (clap derive). Add:

```rust
/// Backend used for the engagement classifier. Defaults to the same
/// backend used for the chat (--backend); `stub` forces deterministic
/// classification regardless of main backend.
#[arg(long)]
classifier_backend: Option<String>,

/// Model used for the engagement classifier. Defaults to the same
/// model used for the chat (--model). Useful for haiku-as-classifier
/// + sonnet-as-chat configurations.
#[arg(long)]
classifier_model: Option<String>,

/// Maximum time to block awaiting the previous turn's classification
/// before the next intent decision. Default 500ms.
#[arg(long, default_value_t = 500)]
classifier_timeout_ms: u64,

/// Print pedagogical decisions (intent chosen, classifier output)
/// alongside the conversation, on stderr.
#[arg(long)]
verbose: bool,
```

- [ ] **Step 3: Verify the CLI parses**

```bash
cargo run --bin primer -- --help
```
Expected: new flags appear in help output.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(cli): add --classifier-backend/-model/-timeout-ms and --verbose flags"
```

---

### Task 26: Classifier construction matrix

**Files:**
- Modify: `src/crates/primer-cli/src/main.rs`

- [ ] **Step 1: Write tests for the construction logic**

If `main.rs` currently has a testable `build_*` helper, mirror it. Otherwise, factor out a `build_classifier` function:

```rust
// near the top of main.rs
fn build_classifier(
    main_backend: Arc<dyn InferenceBackend>,
    main_model: &str,
    args: &Args,
    classifier_settings: ClassifierSettings,
) -> Result<Arc<dyn EngagementClassifier>> {
    match (args.backend.as_str(), args.classifier_backend.as_deref()) {
        (_, Some("stub")) => Ok(Arc::new(StubEngagementClassifier::new())),
        ("stub", None)    => Ok(Arc::new(StubEngagementClassifier::new())),
        (_, Some(other))  => {
            let cls_backend = build_named_backend(other, args)?;
            let model = args.classifier_model.clone().ok_or_else(|| {
                PrimerError::Inference("--classifier-model is required when --classifier-backend is set".into())
            })?;
            Ok(Arc::new(LlmEngagementClassifier::new(cls_backend, model, classifier_settings)))
        }
        (_, None) => {
            let model = args.classifier_model.clone().unwrap_or_else(|| main_model.to_string());
            Ok(Arc::new(LlmEngagementClassifier::new(
                Arc::clone(&main_backend), model, classifier_settings,
            )))
        }
    }
}
```

(`build_named_backend` is a helper that constructs a fresh `InferenceBackend` instance for a given backend name; refactor the existing `--backend` construction logic to be reusable.)

Tests:

```rust
#[cfg(test)]
mod classifier_construction_tests {
    use super::*;

    fn args_with(backend: &str, classifier_backend: Option<&str>, classifier_model: Option<&str>) -> Args {
        Args {
            backend: backend.into(),
            model: "main-model".into(),
            classifier_backend: classifier_backend.map(String::from),
            classifier_model: classifier_model.map(String::from),
            classifier_timeout_ms: 500,
            verbose: false,
            // ... fill in other Args fields with defaults ...
        }
    }

    #[test]
    fn defaults_to_main_backend_and_model() {
        let main = Arc::new(StubBackend::new()) as Arc<dyn InferenceBackend>;
        let args = args_with("cloud", None, None);
        let c = build_classifier(main, "main-model", &args, ClassifierSettings::default()).unwrap();
        assert_eq!(c.identifier(), "llm:main-model");
    }

    #[test]
    fn stub_main_defaults_to_stub_classifier() {
        let main = Arc::new(StubBackend::new()) as Arc<dyn InferenceBackend>;
        let args = args_with("stub", None, None);
        let c = build_classifier(main, "stub", &args, ClassifierSettings::default()).unwrap();
        assert_eq!(c.identifier(), "stub");
    }

    #[test]
    fn explicit_stub_override_works_with_cloud_main() {
        let main = Arc::new(StubBackend::new()) as Arc<dyn InferenceBackend>;
        let args = args_with("cloud", Some("stub"), None);
        let c = build_classifier(main, "main-model", &args, ClassifierSettings::default()).unwrap();
        assert_eq!(c.identifier(), "stub");
    }

    #[test]
    fn classifier_model_override_uses_main_backend() {
        let main = Arc::new(StubBackend::new()) as Arc<dyn InferenceBackend>;
        let args = args_with("cloud", None, Some("haiku-4-5"));
        let c = build_classifier(main, "main-model", &args, ClassifierSettings::default()).unwrap();
        assert_eq!(c.identifier(), "llm:haiku-4-5");
    }
}
```

- [ ] **Step 2: Verify tests fail**

```bash
cargo test -p primer-cli classifier_construction
```
Expected: FAIL.

- [ ] **Step 3: Implement `build_classifier` and `build_named_backend`**

(Implementation above; place in `main.rs`.)

- [ ] **Step 4: Wire `build_classifier` into `main()`**

Replace the existing `DialogueManager::new` construction:

```rust
let classifier_settings = ClassifierSettings {
    blocking_timeout: Duration::from_millis(args.classifier_timeout_ms),
    ..Default::default()
};
let classifier = build_classifier(
    Arc::clone(&backend), &args.model, &args, classifier_settings.clone()
)?;
let mut dm = DialogueManager::new(
    &backend, &knowledge, storage, classifier, classifier_settings,
);
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p primer-cli
cargo test --workspace
```
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(cli): build_classifier matrix — defaults reuse main, overrides per-axis"
```

---

### Task 27: `--verbose` printing

**Files:**
- Modify: `src/crates/primer-cli/src/main.rs`

- [ ] **Step 1: Surface intent + classifier on each turn**

The DialogueManager's `respond_to_streaming` already returns the response. To get the chosen intent and the latest classification, expose two getters:

```rust
// In DialogueManager:
pub fn last_intent(&self) -> Option<PedagogicalIntent> { ... }
pub fn last_assessment(&self) -> Option<&EngagementAssessment> { ... }
```

Or thread an `on_decision` callback. Simpler: expose getters; `main.rs` reads them after each turn when `args.verbose`.

- [ ] **Step 2: Print to stderr after each turn in verbose mode**

In `main.rs`'s REPL loop, after each `respond_to(...)`:

```rust
if args.verbose {
    if let Some(intent) = dm.last_intent() {
        eprintln!("[intent] {:?} -> {:?}", learner.current_engagement, intent);
    }
    if let Some(a) = dm.last_assessment() {
        let r = a.reasoning.as_deref().unwrap_or("");
        eprintln!("[classifier] {:?} conf={:.2} ({})\n             — {r}",
            a.state, a.confidence, dm.classifier_identifier());
    }
}
```

- [ ] **Step 3: Manual smoke test**

```bash
RUST_LOG=info cargo run --bin primer -- --backend stub --classifier-backend stub --verbose --no-persist --name Tester --age 8
```
Type a few inputs. Expected: `[intent]` lines on every turn; `[classifier]` lines from turn 2 onward (turn 1 has no classification yet); stdout shows the conversation.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(cli): --verbose prints intent + classifier debug lines to stderr"
```

---

## Phase 7 — End-to-end + docs

### Task 28: End-to-end integration test

**Files:**
- Modify: `src/crates/primer-pedagogy/src/dialogue_manager.rs` (or a new `tests/integration.rs`)

- [ ] **Step 1: Write the test**

```rust
#[tokio::test]
async fn end_to_end_classifier_routing_across_multi_turn_session() {
    let backend = StubBackend::new();
    let knowledge = StubKnowledge::new();
    let storage = Arc::new(SqliteSessionStore::open_in_memory().await.unwrap());
    // Scripted classifier: turn 1 -> Engaged, turn 2 -> FrustratedTrying,
    // turn 3 -> Disengaging.
    let classifier = Arc::new(StubEngagementClassifier::with_script(vec![
        EngagementAssessment {
            state: EngagementState::Engaged, confidence: 0.9, reasoning: None,
        },
        EngagementAssessment {
            state: EngagementState::FrustratedTrying, confidence: 0.9, reasoning: None,
        },
        EngagementAssessment {
            state: EngagementState::Disengaging, confidence: 0.9, reasoning: None,
        },
    ])) as Arc<dyn EngagementClassifier>;
    let settings = ClassifierSettings {
        blocking_timeout: Duration::from_secs(5),
        ..Default::default()
    };
    let mut dm = DialogueManager::new(&backend, &knowledge, Some(storage.clone()), classifier, settings);

    let mut learner = test_learner();
    learner.preferences.early_disengagement_threshold = Duration::from_secs(60);
    dm.open_session(&mut learner).await.unwrap();
    // Backdate started_at so Disengaging on turn 4 routes to SessionClose.
    dm.session_for_test().started_at = Utc::now() - chrono::Duration::seconds(120);

    // Turn 1
    let _ = dm.respond_to(&mut learner, "i'm curious about gravity").await.unwrap();
    dm.await_pending_classification_for_test(&mut learner).await;
    // After turn 1's classifier: current_engagement = Engaged.
    assert_eq!(learner.current_engagement, EngagementState::Engaged);

    // Turn 2 — intent should reflect Engaged still.
    let _ = dm.respond_to(&mut learner, "I think it's hard to explain").await.unwrap();
    dm.await_pending_classification_for_test(&mut learner).await;
    assert_eq!(learner.current_engagement, EngagementState::FrustratedTrying);

    // Turn 3 — intent decision should now use FrustratedTrying -> Encouragement.
    let _ = dm.respond_to(&mut learner, "I'm not sure but maybe...").await.unwrap();
    dm.await_pending_classification_for_test(&mut learner).await;
    assert_eq!(learner.current_engagement, EngagementState::Disengaging);

    // Turn 4 — Disengaging + elapsed > 60s should yield SessionClose.
    let last_intent_before = dm.last_intent();
    let _ = dm.respond_to(&mut learner, "ok").await.unwrap();
    let intent_now = dm.last_intent().unwrap();
    assert_eq!(intent_now, PedagogicalIntent::SessionClose);

    // All four classifications must be persisted.
    let recent = storage.load_recent_assessments(
        dm.session_id_for_test(), "stub", 10,
    ).await.unwrap();
    assert_eq!(recent.len(), 4);
}
```

- [ ] **Step 2: Run**

```bash
cargo test -p primer-pedagogy end_to_end_classifier_routing
```
Expected: PASS.

- [ ] **Step 3: Run the full test suite + clippy + fmt**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --check
```
Expected: all clean. Final test count target ~170.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "test(pedagogy): end-to-end classifier routing across multi-turn session"
```

---

### Task 29: Docs — ROADMAP, CLAUDE.md, primer_next_session.md

**Files:**
- Modify: `ROADMAP.md`
- Modify: `CLAUDE.md`
- Modify: `primer_next_session.md`

- [ ] **Step 1: Tick the three Phase 0.3 bullets in ROADMAP**

Find the bullets:
- "Make `Encouragement` reachable from `decide_intent`"
- "Detect factual-question patterns in `decide_intent`"
- "Make `Disengaging` session-length-aware"

Change `[ ]` to `✅` for each. Add a one-line note on what landed.

- [ ] **Step 2: Update CLAUDE.md**

Add a section under "Architecture" describing:
- New `primer-classifier` crate
- The classifier flow (post-response, in natural pause, with bounded blocking timeout)
- Schema v3 (engagement_states, classifiers, turn_classifications)
- The `EngagementState::FrustratedStuck/FrustratedTrying` split
- New CLI flags
- Soft-failure policy for classifier errors

Update the dependency-graph diagram to include `primer-classifier`.

Add to the "Conventions and gotchas worth knowing" section:
- "**The engagement classifier is one model behind the chat.** Classification of turn N happens after the Primer's response to turn N is complete; its result is applied at the start of turn N+1's intent decision. This is a deliberate latency tradeoff for memory-constrained-device support and is invisible to children but worth knowing when reasoning about why a misclassified turn doesn't immediately yank the intent."

- [ ] **Step 3: Refresh primer_next_session.md**

Update the "What's now on main" section to describe what the engagement classifier PR landed. List remaining Phase 0.3 work (concept extraction, comprehension assessment, learner-model persistence). Reset the test-count baseline.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "docs: ROADMAP, CLAUDE.md, next-session brief reflect engagement classifier landing"
```

---

## Final verification

- [ ] `cargo build --workspace` clean
- [ ] `cargo test --workspace` — target ~170 passing
- [ ] `cargo clippy --workspace --all-targets` — zero warnings
- [ ] `cargo fmt --check` — clean
- [ ] Manual REPL run (`--backend stub --classifier-backend stub --verbose --no-persist --name Test --age 8`) — verbose lines on stderr, conversation on stdout, intent flips on scripted state changes if you wire a script in
- [ ] Manual REPL run (`--backend cloud --no-persist --name Test --age 8`) — real classification calls work, soft-fail on bad responses doesn't break the conversation

---

## PR submission

After all tasks pass:

```bash
git push -u origin feature/engagement-classifier
gh pr create --title "feat: engagement classifier (Phase 0.3 decide_intent gaps + LLM-based assessment)" --body "..."
```

PR body should reference the spec (`docs/superpowers/specs/2026-05-01-engagement-classifier-design.md`) and link the three Phase 0.3 ROADMAP bullets it closes. Include the test-count delta (119 → ~170) and the schema migration note ("delete existing `~/.primer/*.db` files before pulling").
