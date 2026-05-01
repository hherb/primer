# Engagement Classifier — Design Spec

**Date:** 2026-05-01
**Status:** Approved, ready for implementation plan
**Tracks ROADMAP bullets:** Phase 0.3 — `decide_intent` gap fixes (Frustrated split,
factual-question routing, session-length-aware Disengaging) and the LLM-based
engagement-assessment foundation that closes Gap 1 properly.

## Why

The current `update_learner_model` in `primer-pedagogy::dialogue_manager` is a
deliberate placeholder — it sets `EngagementState` from a word-count heuristic
on the child's last turn. The brief in `primer_next_session.md` flags three
related routing gaps in `decide_intent`:

1. `Encouragement` is unreachable. `Frustrated` always returns `Scaffolding`.
2. Factual-question patterns (`"what is X?"`, `"how does Y work?"`) are not
   detected — `DirectAnswer` and `AnswerThenPivot` are unreachable.
3. `Disengaging` always returns `SessionClose` regardless of session duration —
   too aggressive in the first minute of a session.

Fixing Gap 1 with a keyword heuristic ("I don't know", "this is too hard")
would bake brittle string matching into a function we'd then have to revisit.
The right fix is to upgrade the engagement assessor itself — replace the
word-count placeholder with a real classifier — and then route `decide_intent`
on the richer signal.

## Scope

In scope:

- New `primer-classifier` crate with an `EngagementClassifier` trait, an
  LLM-backed implementation (`LlmEngagementClassifier`), and a stub
  implementation (`StubEngagementClassifier`).
- `EngagementState` enum: split `Frustrated` into `FrustratedStuck` and
  `FrustratedTrying`. The umbrella `Frustrated` variant is removed.
- New `EngagementAssessment` and `EngagementContext` types in `primer-core`.
- Schema v3 in `primer-storage`: `engagement_states` lookup, `classifiers`
  lookup, `turn_classifications` table.
- `DialogueManager` post-response classification flow with bounded blocking
  on the next turn boundary.
- `decide_intent` rewritten on the new variants, with factual-question pattern
  detection and session-length-aware `Disengaging` routing.
- New CLI flags: `--classifier-backend`, `--classifier-model`,
  `--classifier-timeout-ms`, `--verbose`.

Out of scope (explicitly deferred):

- Comprehension classifier (separate Phase 0.3 bullet — its own spec/PR).
- Concept extraction from child input (separate bullet).
- Learner-model SQLite persistence (Option B in `primer_next_session.md`).
- Knowledge-passages-retrieved verbose output (different crate, ROADMAP 0.4).
- `--reclassify-on-resume` (resume rehydrates only what's persisted).
- Embedding-based engagement classifier (premature without a baseline).
- Per-child classifier-model preferences (waits for learner-model persistence).

## Pre-production migration note

Existing `~/.primer/*.db` files from earlier testing should be deleted before
first run after this lands. The v3 migration is otherwise idempotent and would
work in place — but starting with a clean store is the cleanest pre-production
move and avoids any latent assumptions about historical schema.

## Architecture

### New crate: `primer-classifier`

Sits as a peer of `primer-inference`, `primer-knowledge`, `primer-storage`.
Reasons for a separate crate over folding into `primer-inference`:

- Different abstraction. A classifier is not an inference backend; it
  consumes one (when LLM-backed) or none (BERT-backed).
- Future BERT impls will pull a model-loading dep (candle / ort / tract)
  that has no business in `primer-inference`.
- `primer-pedagogy` can depend on `primer-classifier` directly without
  pulling either inference impl through a feature flag.

### Dependency graph

```
primer-cli → primer-pedagogy → primer-core
                    ↓
                    └→ primer-classifier ← primer-core
                    └→ primer-inference  ← primer-core
                    └→ primer-knowledge  ← primer-core
                    └→ primer-storage    ← primer-core
```

`primer-classifier` depends only on `primer-core`. The LLM-backed
implementation takes `Arc<dyn InferenceBackend>` so the wiring crosses crate
boundaries through trait objects, not direct dependencies. `primer-cli`
constructs both backend and classifier and hands them to `DialogueManager`.

### Per-turn flow

```
1. await_pending_classification(timeout)            // from PREVIOUS turn
     - bounded wait on the prior turn's classify task
     - on Ok → push to learner.recent_assessments,
       update learner.current_engagement if confidence ≥ threshold
     - on timeout → abort the task, log, proceed with stale state

2. session.add_turn(child_turn)                     // in-memory
3. intent = decide_intent_at(&learner, &session, now())
4. retrieve knowledge passages   (unchanged)
5. retrieve long-term memory     (unchanged)
6. build prompt + call inference (streaming)
7. session.add_turn(primer_turn)
8. refresh_summary_if_due        (unchanged)
9. save_session(&session)        (unchanged — persists turns, assigns turn_ids)

10. SPAWN classify task for the child turn:
      ctx = EngagementContext {
          recent_child_turns: last N child turns,
          prior_assessments:  learner.recent_assessments,
      }
      tokio::spawn:
        match classifier.classify(ctx).await {
          Ok(a)  => store.save_classification(session_id, idx, &a, classifier.identifier()).await
                    .or_log_warn();
                    Some(a)
          Err(e) => tracing::warn!(?e, classifier = %classifier.identifier(),
                                   "classifier failed"); None
        }
    self.classify_task = Some(handle);
```

The classifier runs sequentially in the natural pause between Primer's
response and the child's next input. On memory-constrained devices the same
model can be reused (one model loaded, used twice in sequence). On bigger
machines a separate small classifier model can run alongside without forcing
parallel execution during response generation.

## Core types (in `primer-core`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngagementState {
    Engaged,
    Reflecting,
    FrustratedStuck,    // No progress, gives up. Routes to Scaffolding.
    FrustratedTrying,   // Articulating an attempt. Routes to Encouragement.
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngagementAssessment {
    pub state: EngagementState,
    pub confidence: f32,           // 0.0–1.0
    pub reasoning: Option<String>, // populated by LLM impl, None for stub/BERT
}

pub struct EngagementContext<'a> {
    pub recent_child_turns: &'a [Turn],
    pub prior_assessments: &'a [EngagementAssessment],
    // Reserved for extension. Adding fields here is non-breaking
    // because the trait method takes EngagementContext by value.
}
```

`LearnerModel` gains a bounded buffer:

```rust
pub struct LearnerModel {
    pub profile: LearnerProfile,
    pub concepts: Vec<ConceptState>,
    pub preferences: LearningPreferences,
    pub current_engagement: EngagementState,
    pub recent_assessments: Vec<EngagementAssessment>, // NEW
}
```

`recent_assessments` is the trajectory buffer fed to the classifier as
`EngagementContext::prior_assessments`. Capacity = `ClassifierSettings::history_depth`
(default 3). Bounded with manual eviction (`remove(0)` on overflow — `Vec`
rather than `VecDeque` because slice access into it is needed for the
trait input, and at the depths in play the FIFO cost is negligible).
Ephemeral in active use; rehydrated from `turn_classifications` on resume.

## Trait + implementations (in `primer-classifier`)

```rust
#[async_trait]
pub trait EngagementClassifier: Send + Sync {
    /// Stable identifier carrying impl + model-version. Stored in the
    /// classifiers lookup table; lets us re-classify the archive with a
    /// different classifier later without overwriting old labels.
    /// Examples: "llm:claude-haiku-4-5", "stub", "bert-engagement-v1".
    fn identifier(&self) -> &str;

    async fn classify(
        &self,
        ctx: EngagementContext<'_>,
    ) -> Result<EngagementAssessment>;
}
```

### `LlmEngagementClassifier`

Wraps `Arc<dyn InferenceBackend>` and a model name independently of the chat
model. Builds a tight classification prompt:

```text
System: You classify a child's engagement state in a Socratic learning
conversation. Output ONLY valid JSON of the form:
  {"state": "<one of: Engaged, Reflecting, FrustratedStuck, FrustratedTrying,
  Disengaging, Unknown>", "confidence": 0.0–1.0, "reasoning": "<one short sentence>"}

User: Recent trajectory (oldest first):
{prior_assessments rendered as one line each:
  "1 turn ago: FrustratedTrying (0.8) — child articulating but stuck on the analogy"}

Most recent child responses:
{recent_child_turns rendered as plain text, oldest first}

Classify the CURRENT engagement state.
```

Output parsing tolerates wrapper text by extracting the first balanced JSON
object. Soft-fail on any of: parse error, unknown state name, confidence
out of [0.0, 1.0], missing required field. Each soft failure goes through
`tracing::warn!` with structured fields (`classifier_id`, raw output snippet
truncated to ~200 chars, error kind) and returns
`EngagementAssessment { state: Unknown, confidence: 0.0, reasoning: Some("...") }`.

### `StubEngagementClassifier`

Three constructors:

```rust
StubEngagementClassifier::new()
    // Always Engaged, confidence 1.0, no reasoning.

StubEngagementClassifier::with_response(assessment)
    // Always returns this assessment.

StubEngagementClassifier::with_script(vec![...])
    // Returns each in turn, falls back to Engaged once exhausted.
```

`with_script` covers trajectory tests cleanly. Internal state behind
`Mutex<Option<VecDeque<EngagementAssessment>>>` since the trait method
is `&self`. Identifier is always `"stub"`.

### Settings

```rust
// primer-classifier/src/consts.rs — defaults live here per the
// no-magic-numbers principle.
pub const DEFAULT_HISTORY_DEPTH: usize = 3;
pub const DEFAULT_BLOCKING_TIMEOUT_MS: u64 = 500;
pub const DEFAULT_CONFIDENCE_THRESHOLD: f32 = 0.6;
pub const DEFAULT_RECENT_CHILD_TURNS_FOR_CLASSIFICATION: usize = 3;
pub const DEFAULT_MAX_CLASSIFIER_OUTPUT_CHARS: usize = 512;

pub struct ClassifierSettings {
    pub history_depth: usize,
    pub blocking_timeout: Duration,
    pub confidence_threshold: f32,
    pub recent_child_turns: usize,
    pub max_output_chars: usize,
}
impl Default for ClassifierSettings { /* uses the consts above */ }
```

## Schema v3 (in `primer-storage`)

```sql
-- Closed-enum FK lookup, validated and seeded against EngagementState::ALL
-- on open(). Mandatory per the categorical-text-normalize convention.
CREATE TABLE IF NOT EXISTS engagement_states (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

-- Open-vocabulary lookup. Lazily populated on first use, like `concepts`.
-- The string identifier carries impl + model-version so different versions
-- get distinct rows. Examples:
--   "llm:claude-haiku-4-5", "llm:llama3.2:1b", "stub", "bert-engagement-v1"
CREATE TABLE IF NOT EXISTS classifiers (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    identifier TEXT NOT NULL UNIQUE
);

-- One row per (turn, classifier) pair. The UNIQUE constraint supports
-- re-classification by a *different* classifier in future without
-- duplicating same-classifier rows.
CREATE TABLE IF NOT EXISTS turn_classifications (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    turn_id             INTEGER NOT NULL REFERENCES turns(id),
    engagement_state_id INTEGER NOT NULL REFERENCES engagement_states(id),
    classifier_id       INTEGER NOT NULL REFERENCES classifiers(id),
    confidence          REAL NOT NULL,
    reasoning           TEXT,             -- nullable; LLM impl populates
    classified_at       TIMESTAMP NOT NULL,
    UNIQUE(turn_id, classifier_id)
);
CREATE INDEX IF NOT EXISTS idx_turn_classifications_turn_id
    ON turn_classifications(turn_id);
```

Migration is idempotent: `CREATE … IF NOT EXISTS`, all wrapped in
`conn.unchecked_transaction()`. Mirrors the v1→v2 template
(`apply_v2_migrations`). Insertion of new classification rows uses plain
`INSERT` (not `OR REPLACE`) — a UNIQUE conflict means the same classifier ran
twice on the same turn, which is a logic bug worth surfacing.

### New `SessionStore` methods

```rust
async fn save_classification(
    &self,
    session_id: SessionId,
    turn_index: usize,                      // 0-based within session
    assessment: &EngagementAssessment,
    classifier_identifier: &str,
) -> Result<()>;

async fn load_recent_assessments(
    &self,
    session_id: SessionId,
    classifier_identifier: &str,
    k: usize,
) -> Result<Vec<EngagementAssessment>>;
```

`save_classification` resolves `(session_id, turn_index) → turn_id` via a
lookup, lazily creates the `classifiers` row if the identifier is new (per the
existing concepts pattern), then inserts.

`load_recent_assessments` is filtered by classifier identifier so resuming
with classifier X does not pollute the trajectory with rows from classifier Y.
Useful for switching classifier models between sessions.

## Dialogue manager flow

### Struct changes

`DialogueManager` gains two fields in addition to its existing
`&dyn InferenceBackend` and `&dyn KnowledgeBase` references:

```rust
pub struct DialogueManager<'a> {
    // ... existing fields ...
    classifier: &'a dyn EngagementClassifier,
    classifier_settings: ClassifierSettings,
    classify_task: Option<JoinHandle<Option<EngagementAssessment>>>,
}
```

Held by reference, like the other backend deps, so swapping classifier
implementations stays a CLI/config decision and tests inject stubs.

### `await_pending_classification`

```rust
async fn await_pending_classification(&mut self, learner: &mut LearnerModel) {
    let Some(task) = self.classify_task.take() else { return };
    let abort = task.abort_handle();
    let timeout = self.classifier_settings.blocking_timeout;

    match tokio::time::timeout(timeout, task).await {
        Ok(Ok(Some(assessment))) => apply_assessment(learner, assessment, &self.classifier_settings),
        Ok(Ok(None))             => { /* soft failure; nothing to apply */ }
        Ok(Err(panic))           => tracing::warn!(?panic, "classifier task panicked"),
        Err(_)                   => {
            abort.abort();
            tracing::debug!("classifier exceeded blocking timeout — proceeding with stale engagement state");
        }
    }
}

fn apply_assessment(
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
    // Low-confidence assessments are still recorded in history (signal for
    // trajectory) but current_engagement stays unchanged.
}
```

Aborting on timeout prevents pile-up of runaway classifier tasks (slow
classifier + fast typist would otherwise spawn an extra background task each
turn). The cost is one lost classification per timeout. Logging the timeout
makes empirical retuning of `blocking_timeout` straightforward.

### Resume rehydration

`DialogueManager::resume_session(loaded)` extends to:

```rust
let recent = store.load_recent_assessments(
    loaded.id,
    self.classifier.identifier(),
    self.classifier_settings.history_depth,
).await?;
learner.current_engagement = recent
    .last()
    .map(|a| a.state)
    .unwrap_or(EngagementState::Engaged);
learner.recent_assessments = recent;
```

If the session has no classifications for this classifier (e.g. the user
switched classifier models between sessions, or the classifier was
unavailable on previous runs), the trajectory starts fresh and
`current_engagement` defaults to `Engaged`.

## CLI surface (in `primer-cli`)

New flags:

```
--classifier-backend <stub|cloud|ollama>     Default: same as --backend
--classifier-model <model-name>              Default: same as --model
--classifier-timeout-ms <ms>                 Default: 500
--verbose                                    Print classifier+intent debug to stderr
```

Other classifier settings (`history_depth`, `confidence_threshold`,
`recent_child_turns`, `max_output_chars`) stay at constants-module defaults
in this PR. They live in `ClassifierSettings` ready to be wired up as flags
or config-file fields when empirical retuning is needed.

### Construction logic

```rust
let backend: Arc<dyn InferenceBackend> = build_main_backend(&args)?;

let classifier_settings = ClassifierSettings {
    blocking_timeout: Duration::from_millis(args.classifier_timeout_ms),
    ..Default::default()
};

let classifier: Box<dyn EngagementClassifier> =
    match (args.backend.as_str(), args.classifier_backend.as_deref()) {
        // Explicit stub override.
        (_, Some("stub")) => Box::new(StubEngagementClassifier::new()),

        // Main backend is stub → classifier defaults to stub too.
        // (LlmEngagementClassifier wrapping a stub would parse-fail every time.)
        ("stub", None) => Box::new(StubEngagementClassifier::new()),

        // Explicit different backend (e.g. main=cloud, classifier=ollama).
        (_, Some(other)) => {
            let cls_backend = build_classifier_backend(other, &args)?;
            let model = args.classifier_model.clone()
                .ok_or_else(/* required for ollama */)?;
            Box::new(LlmEngagementClassifier::new(cls_backend, model, classifier_settings))
        }

        // Default: reuse main backend, optionally with a different model.
        (_, None) => {
            let model = args.classifier_model.unwrap_or_else(|| args.model.clone());
            Box::new(LlmEngagementClassifier::new(Arc::clone(&backend), model, classifier_settings))
        }
    };
```

The `(_, None)` reuse path is the truly-one-model-loaded case for
memory-constrained devices: same `Arc`, same model name → sequential reuse
during the post-response classification.

### `--verbose` output

When `--verbose` is set, after each Primer response the CLI prints debug
lines to stderr (stdout stays clean for the conversation):

```
[intent] FrustratedTrying → Encouragement
[classifier] FrustratedTrying conf=0.82 (llm:claude-haiku-4-5)
             — child articulating but stuck on the analogy
```

The `[intent]` line shows engagement state used + intent chosen. The
`[classifier]` line shows the classifier output with reasoning when present.
On turn 1 only `[intent]` shows — no classification yet.

ROADMAP 0.4's "knowledge passages retrieved" verbose feature is out of
scope — different crate, different concern.

## `decide_intent` updates (Gaps 1, 2, 3)

```rust
// Existing public signature preserved. Tests use the _at variant
// with synthetic `now` values.
pub fn decide_intent(learner: &LearnerModel, session: &Session) -> PedagogicalIntent {
    decide_intent_at(learner, session, Utc::now())
}

pub fn decide_intent_at(
    learner: &LearnerModel,
    session: &Session,
    now: DateTime<Utc>,
) -> PedagogicalIntent {
    // ─── Gap 1: engagement-state gates (FrustratedStuck/Trying split) ───
    match learner.current_engagement {
        EngagementState::FrustratedStuck => return PedagogicalIntent::Scaffolding,
        EngagementState::FrustratedTrying => return PedagogicalIntent::Encouragement,
        // ─── Gap 3: session-length-aware Disengaging ───
        EngagementState::Disengaging => {
            let elapsed = now - session.started_at;
            return if elapsed < learner.preferences.early_disengagement_threshold {
                PedagogicalIntent::Encouragement
            } else {
                PedagogicalIntent::SessionClose
            };
        }
        EngagementState::Engaged
        | EngagementState::Reflecting
        | EngagementState::Unknown => { /* fall through */ }
    }

    if let Some(last) = session.turns.last() {
        if last.speaker == Speaker::Child {
            // ─── Gap 2: factual-question pattern routing ───
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

            if last.text.split_whitespace().count() < consts::SHORT_TURN_WORD_BOUNDARY {
                return PedagogicalIntent::ComprehensionCheck;
            }

            // Existing concept-depth → Extension (unchanged).
        }
    }

    PedagogicalIntent::SocraticQuestion
}

fn is_factual_question(text: &str) -> bool {
    const FACTUAL_PREFIXES: &[&str] = &[
        "what is ", "what are ", "what's ", "what does ",
        "how does ", "how do ",  "how is ",  "how are ",
    ];
    let lowered = text.trim().to_lowercase();
    FACTUAL_PREFIXES.iter().any(|p| lowered.starts_with(p))
}
```

### Constants and settings

```rust
// primer-pedagogy/src/consts.rs
pub const SHORT_TURN_WORD_BOUNDARY: usize = 10;          // was inline
pub const ACTIVE_CONCEPT_LOOKBACK: usize = 4;            // was inline
pub const DEFAULT_EARLY_DISENGAGEMENT_SECS: u64 = 5 * 60;
```

`early_disengagement_threshold` is per-child tunable — some children
disengage faster than others. The home for this kind of value is
`LearningPreferences` (already in `primer-core::learner`), so adding it
there now is the right shape:

```rust
pub struct LearningPreferences {
    // ... existing fields ...
    pub early_disengagement_threshold: Duration,
}

impl Default for LearningPreferences {
    fn default() -> Self {
        Self {
            // ...
            early_disengagement_threshold:
                Duration::from_secs(consts::DEFAULT_EARLY_DISENGAGEMENT_SECS),
        }
    }
}
```

`decide_intent_at` reads `learner.preferences.early_disengagement_threshold`.
No extra function parameter, value already in scope. When learner-model
persistence (Option B) lands, the field auto-persists with the rest of
preferences. CLI flag override deferred to a later PR — defaults work for
the initial landing.

`Duration` does not serialize cleanly via stock serde; the field stores as
`Duration` in memory but persistence (when Option B lands) will round-trip
via a `u64` seconds representation. Not a v3 schema concern — preferences
aren't persisted in v3.

### `prompt_builder` engagement-note section

```rust
let engagement_note = match learner.current_engagement {
    EngagementState::FrustratedStuck | EngagementState::FrustratedTrying => {
        "\n\nIMPORTANT: The child appears frustrated. Be especially gentle. \
         Offer to approach the topic differently or switch topics entirely."
    }
    EngagementState::Disengaging => { /* unchanged */ }
    _ => "",
};
```

Same engagement note for both Frustrated* variants — the routing-specific
guidance lives in the `intent_instruction` block (Scaffolding vs Encouragement),
which already encodes the right tone for each.

## Testing strategy

TDD discipline per the existing pattern: write the characterization test
(positive case for new behaviour, regression guard for old), watch it fail,
implement, watch pass.

### `primer-classifier`

- Stub: default-returns-Engaged, with_response, with_script (in-order, fall-back when exhausted), identifier
- LLM impl: well-formed JSON parse, JSON-with-wrapper-text extraction, unparseable → `Unknown(0.0)` + warn, invalid state name → soft-fail, out-of-range confidence → soft-fail, identifier includes model name, prompt includes prior_assessments and recent_child_turns

### `primer-storage`

- Schema v2→v3 migration: creates tables + seeds engagement_states, idempotent re-open, rolls back on failure (mirror `apply_v2_migrations_rolls_back_on_failure`)
- Validate-and-seed: rejects unknown id / name mismatch on engagement_states (parallel to existing speaker / pedagogical_intent tests)
- `save_classification`: persists, lazy-creates classifier row, surfaces UNIQUE conflict on duplicate, handles multiple classifier_ids per turn, persists/null reasoning
- `load_recent_assessments`: ordered, classifier-filtered, k-bounded, empty when no rows

### `primer-pedagogy`

`decide_intent`:

| Existing test | Fate |
|---|---|
| `frustrated_returns_scaffolding` | Renamed → `frustrated_stuck_returns_scaffolding` |
| `frustrated_overrides_short_child_turn_branch` | Renamed → `frustrated_stuck_overrides_short_child_turn` |
| `disengaging_returns_session_close` | Renamed → `disengaging_late_returns_session_close`; uses `decide_intent_at` with backdated `started_at` |
| `disengaging_overrides_short_child_turn_branch` | Same; uses backdated `started_at` |
| `frustrated_does_not_currently_return_encouragement` | Removed — replaced by positive case below |
| `factual_question_pattern_does_not_currently_return_direct_answer` | Removed — replaced by positive cases below |

New tests:

- `frustrated_trying_returns_encouragement`
- `frustrated_trying_overrides_short_child_turn`
- `factual_question_what_is_returns_direct_answer`
- `factual_question_how_does_returns_direct_answer`
- `factual_question_after_direct_answer_returns_answer_then_pivot`
- `factual_question_with_frustrated_state_still_routes_via_engagement` (regression guard)
- `non_factual_short_turn_still_returns_comprehension_check` (regression guard)
- `factual_question_doesnt_match_partial_words` (e.g. `"whatever"` doesn't trigger `"what is"`)
- `disengaging_early_returns_encouragement` (`started_at` 1 minute ago)
- `disengaging_at_threshold_returns_session_close` (`started_at` exactly 5 minutes ago — boundary)
- `disengaging_just_after_threshold_returns_session_close`

`prompt_builder`: engagement-note coverage for both `FrustratedStuck` and `FrustratedTrying`.

`DialogueManager`:

- Classifies child turn after response
- Awaits pending classification on next turn
- Respects `blocking_timeout`
- Aborts on timeout
- Applies-when-confident / no-`current_engagement`-update-when-not-confident (always pushes to history)
- Buffer respects `history_depth`
- Soft-fail handling
- Persists classification
- Resume rehydration filtered by classifier_id

### `primer-cli`

- Classifier construction matrix: defaults-to-main, separate-backend override, stub-when-main-is-stub, model-only override
- `--verbose` writes to stderr; absence keeps stderr clean

### Integration

End-to-end test with `--backend stub --classifier-backend stub`, scripted
classifier outputs via `with_script`, asserts intent flips correctly across
a multi-turn session including a Disengaging-late state forcing SessionClose.

### Test count target

Total estimated new tests: ~50, on top of the existing 119. Target after the
spec lands: ~170 tests across the workspace.

## Soft-failure policy summary

The classifier is a secondary signal. Errors must never propagate up and
abort the conversation:

- Network / API errors → log via `tracing::warn!`, return `Unknown(0.0)`
- JSON parse failures → log raw output snippet, return `Unknown(0.0)`
- Invalid state names → log, return `Unknown(0.0)`
- Out-of-range confidence → log, return `Unknown(0.0)`
- Task panics → caught by JoinHandle, logged
- Timeouts → abort task, log at debug level

Absence of a `turn_classifications` row for a child turn is the "this turn
wasn't classified" signal; logs explain why. If absences become problematic
in practice, a separate `classification_failures` table could be added —
deferred until evidence shows it's needed.

## Open implementation questions

These are deferred to the implementation plan, not the design:

- Exact JSON-extraction approach for wrapped LLM output (regex vs. balanced-brace scan).
- Whether `LlmEngagementClassifier::classify` calls `generate` (non-streaming) directly or uses a smaller wrapper. `generate` is fine — classifier output is short.
- Tokio task handle storage: `Option<JoinHandle<Option<EngagementAssessment>>>` directly on `DialogueManager`, or a richer struct. Direct field is simpler.

## Closes

- `primer_next_session.md` Option A, all three gaps
- ROADMAP 0.3 bullets:
  - "Make `Encouragement` reachable from `decide_intent`"
  - "Detect factual-question patterns in `decide_intent`"
  - "Make `Disengaging` session-length-aware"
  - Foundation work for "Add LLM-based comprehension assessment" (the architecture extends naturally for comprehension, but comprehension itself is a separate spec).

Does NOT close (orthogonal):

- Concept extraction from child input
- Learner-model SQLite persistence (Option B)
- Vocabulary tracking with spaced repetition
