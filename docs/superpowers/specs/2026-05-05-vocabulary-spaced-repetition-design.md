# Per-Child Vocabulary Tracking with Spaced Repetition — Design Spec

**Date:** 2026-05-05
**Phase:** Phase 0.3
**Author:** Claude (with Horst Herb)
**Status:** Approved by user, pending implementation plan

## Goal

Surface previously-encountered concepts back into the system prompt at expanding intervals based on demonstrated mastery, so the Primer naturally revisits words and ideas the child is at risk of forgetting — without nagging, without quizzing, and without the Primer steering the topic. The mechanism reuses the per-concept depth assessments the comprehension classifier already produces, layering an orthogonal Leitner-box "review cadence" signal alongside the existing depth signal.

After this work:

- Each `ConceptState` carries a `box_level: u8` recording how many successful reviews have spaced out its review interval.
- After every comprehension assessment, a strong reading (depth ≥ Recall AND confidence ≥ 0.6) bumps the box up; a weak reading (depth = Aware OR confidence < 0.6) resets it to 0; sub-threshold assessments are ignored, mirroring existing comprehension semantics.
- At every turn, `due_concepts(&learner, now, max_per_prompt)` selects the K most-overdue concepts and injects them as a new system-prompt section that the LLM weaves into its response **only if topically relevant**.
- A new `--vocab-max-per-prompt <N>` CLI flag (default 4) bounds prompt bloat.
- Schema v7 persists `box_level` per learner-concept; v6 → v7 upgrade is mechanical (one column add, default 0).
- The Primer's "follow the child's lead" Socratic stance is preserved — the LLM is given a hint list, not a script.

## Non-goals

- **No new `PedagogicalIntent` variant.** No `ReviewVocab` intent. The mechanism is passive prompt-injection only.
- **No active "drilling" or quizzing flow.** The LLM is told to weave words in only if topically relevant; the prompt explicitly says "don't drill or quiz".
- **No manual "push back this word" or "I forgot" signals from CLI/REPL.** The comprehension classifier is the only thing that moves box state.
- **No sub-threshold-driven decay.** A low-confidence assessment is still a no-op for in-memory learner state, matching the existing `apply_comprehension` skip rule. Decay-from-low-confidence is a v2 concern that would also touch comprehension.
- **No per-concept review history beyond `box_level` and `last_encountered`.** The `turn_comprehensions` log already has full historical assessments if retro-derivation ever becomes useful.
- **No cross-locale concept linking.** Concept "gravity" (en) and "Schwerkraft" (de) remain separate concepts per the v6 `concepts.concept_language_tag` design.
- **No locale-aware "4 days ago" rendering** in the vocab list. English integer-day text inside the system prompt for v1 (the LLM consumes it; the child never sees this text).
- **No `--vocab-window <days>` CLI flag** (mentioned speculatively in NEXT_SESSION.md). No use case justifies it.
- **No catastrophically-overdue handling.** A concept overdue by >2× its scheduled interval is still surfaced normally; the "reset box on long gaps" forgetting model is deferred.
- **No backfill of `box_level` from historical comprehension assessments on v6 → v7 upgrade.** All pre-existing `learner_concepts` rows get `box_level = 0`. Acceptable for Phase 0.3 with no field-deployed users.
- **No changes to `apply_extraction`.** Brand-new concepts get `box_level: 0` from the `Default`/literal initialiser; only `apply_comprehension` updates the box.
- **No changes to `update_learner_model`** placeholder. Box state is comprehension's domain only.
- **No changes to existing CLAUDE.md gotchas about extraction and comprehension lagging one turn.** Box transitions inherit the same one-turn lag; the prompt for turn N reflects box state from end-of-turn-(N-1).

## Pedagogical principles preserved

These are the load-bearing constraints from CLAUDE.md and `MEMORY.md`:

- **The Primer never tries to maximise engagement.** The vocab section is a hint list with explicit "don't drill or quiz" instruction; it doesn't push the LLM to surface words the child isn't already opening up to.
- **The Primer asks more questions than it answers.** Vocab review happens through Socratic re-engagement, not flashcard-style drilling.
- **All learner data is local.** `box_level` is per-DB; never leaves the device.
- **Comprehension is verified through transfer questions, application, and contradiction probing — not assumed.** Box advancement requires a comprehension classifier reading at depth ≥ Recall and confidence ≥ 0.6 — not just "child mentioned the word again".

## Architecture

The feature layers onto the existing concept-tracking pipeline without disturbing it. Three new pure functions in `primer-core::vocab`, one new schema column, one new prompt-pack method, one new system-prompt section, one new CLI flag.

### Data flow

```
                    ┌─────────────────────────────────────────────────┐
                    │                EXISTING (unchanged)             │
                    │                                                 │
       child turn ──▶ extractor ──▶ comprehension ──▶ apply_*_outcome │
                    │  (concepts)    (depth+conf)         │           │
                    │                                     ▼           │
                    │              learner.concepts: Vec<ConceptState>│
                    └─────────────────────────────────────────────────┘
                                                          │
                    ┌─────────────────────────────────────┴───────────┐
                    │                  NEW                            │
                    │                                                 │
                    │   apply_box_transition(box, depth, conf) -> u8  │
                    │      (called inside apply_comprehension loop)   │
                    │                                                 │
                    │   due_concepts(&learner, now, max_k)            │
                    │      (called inside build_turn_prompt)          │
                    │                                                 │
                    │   PromptPack::vocab_review_intro()              │
                    │   build_system_prompt: vocab_section            │
                    └─────────────────────────────────────────────────┘
```

### Six surface points

1. **`primer-core/src/learner.rs::ConceptState`** — add `box_level: u8` field with `#[serde(default)]` for backward compatibility with old serialised profiles.
2. **`primer-core/src/vocab.rs`** (new file) — three pure functions: `apply_box_transition`, `is_due`, `due_concepts`. Plus interval table and constants in `primer-core::consts::vocab`.
3. **`primer-storage/src/schema.rs`** — `apply_v7_migrations` adds `learner_concepts.box_level INTEGER NOT NULL DEFAULT 0`. `USER_VERSION` bumped to 7. `save_learner` upsert and `load_learner` SELECT extended by one column.
4. **`primer-pedagogy/src/dialogue_manager/apply.rs::apply_comprehension`** — call `apply_box_transition` for each accepted assessment alongside the existing depth-promotion call.
5. **`primer-pedagogy/src/prompt_builder.rs`** — new `vocab_section` injected into the system prompt between `retrieved_section` and `knowledge_section`. New `PromptPack::vocab_review_intro()` method, implemented in both `en` and `de` packs.
6. **`primer-cli/src/main.rs`** — new `--vocab-max-per-prompt <N>` flag (default 4, must be ≥ 1).

## Data model

### Rust types

`ConceptState` gains one field:

```rust
pub struct ConceptState {
    pub concept_id: String,
    pub depth: UnderstandingDepth,
    pub confidence: f32,
    pub encounter_count: u32,
    pub last_encountered: Option<DateTime<Utc>>,
    pub notes: Vec<String>,
    /// Spaced-repetition box level. 0 = freshly encountered or recently
    /// failed; up to MAX_BOX_LEVEL = 4 (interval = 30 days). The interval
    /// `now - last_encountered` must exceed `BOX_INTERVALS_DAYS[box_level]`
    /// for the concept to be "due".
    #[serde(default)]
    pub box_level: u8,
}
```

`#[serde(default)]` keeps old persisted profiles deserialising cleanly with `box_level = 0` — same pattern used for `LearnerProfile::locale`.

### Constants

In `primer-core/src/consts.rs` as a new `pub mod vocab` submodule, mirroring the existing `pub mod retry` pattern:

```rust
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
/// Numerically equal to the comprehension classifier's confidence_threshold
/// (also 0.6) but kept independent so a future researcher can tune
/// box-advancement strictness without affecting depth promotion.
pub const MIN_CONF_FOR_BOX_PROMOTION: f32 = 0.6;

/// Default cap on overdue concepts injected into the system prompt
/// per turn. Configurable via VocabSettings.max_per_prompt and the
/// `--vocab-max-per-prompt` CLI flag.
pub const DEFAULT_VOCAB_MAX_PER_PROMPT: usize = 4;

}  // pub mod vocab
```

### Settings struct

New `VocabSettings` in `primer-pedagogy`, mirroring the existing `ClassifierSettings` / `ExtractorSettings` / `ComprehensionSettings`:

```rust
pub struct VocabSettings {
    pub max_per_prompt: usize,
}

impl Default for VocabSettings {
    fn default() -> Self {
        Self { max_per_prompt: DEFAULT_VOCAB_MAX_PER_PROMPT }
    }
}
```

Threaded into `DialogueManagerSubsystems` as a new field. CLI flag overrides via the constructor.

### Schema v7 migration

```rust
pub const USER_VERSION: i64 = 7;

pub(crate) fn apply_v7_migrations(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()
        .map_err(|e| PrimerError::Storage(format!("v7 migration: failed to begin tx: {e}")))?;

    if !column_exists(&tx, "learner_concepts", "box_level")? {
        tx.execute_batch(
            "ALTER TABLE learner_concepts \
             ADD COLUMN box_level INTEGER NOT NULL DEFAULT 0;"
        ).map_err(|e| PrimerError::Storage(
            format!("v7 migration: ALTER learner_concepts ADD box_level: {e}")
        ))?;
    }

    tx.commit().map_err(|e| PrimerError::Storage(format!("v7 migration: commit: {e}")))?;
    Ok(())
}
```

Idempotent and transaction-wrapped, matching the v2 / v6 pattern. Pre-v7 rows get `box_level = 0` automatically via the column default.

`save_learner`'s `learner_concepts` UPSERT extends from 7 columns → 8 columns (adds `box_level`). `load_learner`'s SELECT extends correspondingly. Both are mechanical edits inside existing functions.

## Pure functions in `primer-core::vocab`

### `apply_box_transition`

```rust
/// Box-level transition driven by a comprehension assessment.
///
/// Rules (three-zone policy):
/// - confidence < MIN_CONF_FOR_BOX_PROMOTION → box = 0 (reset)
/// - depth = Aware → box = 0 (reset, regardless of confidence)
/// - depth ≥ Recall AND confidence ≥ MIN_CONF_FOR_BOX_PROMOTION → box += 1 (capped)
///
/// Pure — returns the new box level. The caller decides whether to write
/// it back to ConceptState.
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
```

### `is_due`

```rust
/// True if a concept is due for review now.
///
/// Returns false for concepts never encountered (last_encountered = None).
/// Returns true for concepts with last_encountered older than the box's
/// interval. Out-of-range box_level (defensive) clamps to MAX_BOX_LEVEL.
pub fn is_due(concept: &ConceptState, now: DateTime<Utc>) -> bool {
    let Some(last) = concept.last_encountered else {
        return false;
    };
    let box_idx = (concept.box_level as usize).min(BOX_INTERVALS_DAYS.len() - 1);
    let interval = chrono::Duration::days(BOX_INTERVALS_DAYS[box_idx] as i64);
    now - last > interval
}
```

### `due_concepts`

```rust
/// Top `max_count` due concepts from the learner model, sorted by
/// overdue-ness descending (most overdue first).
///
/// Pure — borrows from the learner. Returns at most `max_count` references;
/// fewer if the learner has fewer due concepts.
pub fn due_concepts<'a>(
    learner: &'a LearnerModel,
    now: DateTime<Utc>,
    max_count: usize,
) -> Vec<&'a ConceptState> {
    if max_count == 0 {
        return Vec::new();
    }
    let mut due: Vec<&ConceptState> = learner.concepts
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

fn overdue_amount(concept: &ConceptState, now: DateTime<Utc>) -> chrono::Duration {
    let Some(last) = concept.last_encountered else {
        return chrono::Duration::zero();
    };
    let box_idx = (concept.box_level as usize).min(BOX_INTERVALS_DAYS.len() - 1);
    let interval = chrono::Duration::days(BOX_INTERVALS_DAYS[box_idx] as i64);
    (now - last - interval).max(chrono::Duration::zero())
}
```

## Integration

### Apply path — `dialogue_manager/apply.rs::apply_comprehension`

Inside the existing per-assessment loop in `apply_comprehension`, alongside the existing depth promotion. **Important asymmetry**: depth promotion runs only on strict increase (`a.depth > c.depth`); box advancement runs on **every** accepted assessment regardless of depth movement. This is deliberate — the spaced-repetition mechanic *requires* that a successful re-confirmation at the same depth still advance the box. A concept stuck at `Comprehension` for a year should still see its review interval expand if every re-encounter confirms `Comprehension` confidently.

```rust
for a in &result.assessments {
    if a.confidence < settings.confidence_threshold {
        continue;  // existing — sub-threshold skipped entirely
    }
    if let Some(c) = learner.concepts.iter_mut()
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
        // at the same depth is exactly what advances the box (the SR
        // mechanism — expanding intervals on successful review).
        let new_box = primer_core::vocab::apply_box_transition(
            c.box_level, a.depth, a.confidence,
        );
        if new_box != c.box_level {
            c.box_level = new_box;
            changed = true;
        }
    }
}
```

`last_encountered` is refreshed by `apply_extraction` in the same await sequence (which runs before `apply_comprehension` per the existing chain — see the doc comment on `apply_comprehension`). So every box advancement is paired with a fresh `last_encountered` from the same turn, keeping the "due" math consistent.

**Subtle but deliberate:** `apply_box_transition` runs *inside* the outer `confidence_threshold` guard, but its own logic *also* checks confidence. Today these thresholds coincide (both 0.6) which makes the inner `confidence < MIN_CONF_FOR_BOX_PROMOTION` reset branch **conditionally dead code in the default config** — anything reaching `apply_box_transition` already passed the outer 0.6 gate. The branch is retained because the constants are deliberately namespaced apart:

- `confidence_threshold` lives in `comprehension_settings` and gates "should we touch in-memory learner state at all?"
- `MIN_CONF_FOR_BOX_PROMOTION` lives in `primer-core::consts::vocab` and gates "should this assessment promote the box?"

A future researcher who tightens box-advancement strictness (e.g., requires confidence ≥ 0.75 for promotion while still letting confidence ≥ 0.6 update depth) only changes the latter. Or who loosens `comprehension_settings.confidence_threshold` to 0.4 (capturing weaker comprehension signals into in-memory state) gets the inner reset branch becoming live without any code change. Conflating them now would obscure that policy split and hide the natural divergence point.

**Sub-threshold note:** under default config, assessments below 0.6 confidence are skipped entirely by the outer guard, so they don't trigger `box = 0` reset. Decay-from-low-confidence is a v2 concern that would either lower `comprehension_settings.confidence_threshold` or restructure the apply path so sub-threshold assessments still reach `apply_box_transition`.

### Apply path — places where box stays untouched (or only initialises)

- **`apply_extraction`** — adds `box_level: 0` to the `ConceptState` struct-literal for brand-new concepts (mechanical one-line edit). Does **not** modify `box_level` for already-known concepts — that's `apply_comprehension`'s job. No special "first-encounter is box 1" boost.
- **`update_learner_model`** — engagement heuristic; not box's domain.
- **`apply_assessment`** — engagement classifier; not box's domain.

### Inject path — `dialogue_manager/turn.rs::build_turn_prompt`

```rust
let due_vocab = primer_core::vocab::due_concepts(
    &self.learner,
    chrono::Utc::now(),
    self.subsystems.vocab_settings.max_per_prompt,
);
// Threaded through into build_prompt alongside knowledge_context, summary, retrieved_older.
```

### Inject path — `prompt_builder.rs::build_system_prompt_with_pack`

A new section sits between `retrieved_section` and `knowledge_section`:

```rust
let vocab_section = if due_vocab.is_empty() {
    String::new()
} else {
    let intro = pack.vocab_review_intro();
    let lines: String = due_vocab.iter()
        .map(|c| format!(
            "- {} (depth: {}, last seen {} days ago)",
            c.concept_id,
            c.depth,
            days_since(c.last_encountered.unwrap(), now),
        ))
        .collect::<Vec<_>>()
        .join("\n");
    format!("\n\n{intro}\n\n{lines}")
};

format!(
    "{base}\n\n{intent_instruction}{engagement_note}\
     {summary_section}{retrieved_section}{vocab_section}{knowledge_section}"
)
```

`days_since` is a small helper rendering the integer-day delta as English text. No locale variation in v1 — the LLM consumes the prompt; the child never sees it.

**Order rationale:** vocab between retrieved-older and knowledge. The LLM reads sections top-to-bottom; vocab is "what the child already knows and may benefit from revisiting" — a recent-history concern rather than the static knowledge corpus.

### `PromptPack::vocab_review_intro`

```rust
trait PromptPack {
    // ... existing ...
    fn vocab_review_intro(&self) -> &str;
}
```

English (in `prompt-packs/en.toml`):

> Concepts {child} has previously encountered. Weave them in **only if topically relevant** to what they bring up — don't drill or quiz. The list is a hint, not a script.

German (in `prompt-packs/de.toml`):

> Konzepte, die {child} bereits kennengelernt hat. Greife sie **nur dann auf, wenn sie thematisch passen** — keine Abfrage, kein Drillen. Die Liste ist ein Hinweis, kein Skript.

The German rendering is a faithful translation for v1; a native speaker can polish later.

### Resume path

`resume_session` in `dialogue_manager/lifecycle.rs` loads the learner model from `learner_concepts` (with `box_level` via the new column). Nothing else changes. The next turn's `build_turn_prompt` runs `due_concepts` over the freshly-loaded learner just like a continuing turn would.

### Non-persisted (`--no-persist`) path

Learner is in-memory only; `box_level` updates happen normally; on session close everything is discarded. No special handling.

## CLI flag

```rust
/// Maximum number of overdue/due concepts to inject into the system prompt
/// per turn. Higher = more review pressure, more prompt bloat. Default 4.
#[arg(long, value_name = "N")]
vocab_max_per_prompt: Option<usize>,
```

Validation: must be ≥ 1. Reject `0` at parse time with a clear error.

## Tests

### Test order (TDD)

Tests are written first per task, fail, then turn green:

#### 1. `primer-core::vocab` pure-function tests (~18 tests, inline in `vocab.rs#mod tests`)

- `apply_box_transition`:
  - Promotes from 0 → 1 on `(Recall, 0.7)`.
  - Promotes from 0 → 1 on `(Comprehension, 0.9)`.
  - Promotes from 2 → 3 on `(Application, 0.95)`.
  - Caps at `MAX_BOX_LEVEL` for `(box=4, Analysis, 1.0)`.
  - Resets to 0 on `(box=3, Aware, 0.9)`.
  - Resets to 0 on `(box=3, Comprehension, 0.4)`.
  - Resets to 0 on `(box=4, Analysis, 0.59)`.
  - Boundary: `(box=0, Recall, 0.6)` promotes to 1 (≥ inclusive).
- `is_due`:
  - `last_encountered = None` → not due.
  - Box 0, gap = 12h → not due.
  - Box 0, gap = 25h → due.
  - Box 4, gap = 29d → not due.
  - Box 4, gap = 31d → due.
  - Out-of-range `box_level = 99` clamps to box 4.
- `due_concepts`:
  - `max_count = 0` → empty Vec.
  - Empty `learner.concepts` → empty Vec.
  - All concepts not due → empty Vec.
  - Mix of 5 due + 3 not-due, `max_count = 4` → 4 most-overdue, in order.
  - Concept with `last_encountered = None` → not selected.
  - Out-of-range `box_level` clamping verified.

#### 2. `primer-storage::schema` v7 migration tests

- `apply_v7_migrations` adds the `box_level` column.
- Idempotent on re-run.
- Rolls back on failure (fault-inject via colliding object name, matching the v4 test pattern).
- `USER_VERSION` constant is 7.
- Pre-existing `learner_concepts` rows default to `box_level = 0`.

#### 3. `primer-storage` round-trip tests

- Save a learner with `box_level = 3` → load → `box_level = 3`.
- Save with mixed box levels across multiple concepts → all preserved.

#### 4. `primer-pedagogy::dialogue_manager::apply` extension tests

- `apply_comprehension` advances `box_level` on a strong assessment that promotes depth (`box_level: 0 → 1` AND `depth: Aware → Recall`).
- `apply_comprehension` advances `box_level` on a strong re-confirmation at the **same** depth (concept already at `Comprehension`; new assessment also `Comprehension` with high confidence → `box_level: 1 → 2`, depth unchanged). **This is the SR-critical case** — proves expanding intervals work without depth movement.
- `apply_comprehension` resets `box_level` to 0 when the assessment is `Aware` (concept previously at higher depth — depth itself is monotonic so doesn't drop, but box drops to 0 reflecting "needs re-review soon").
- Sub-threshold assessment (below `comprehension_settings.confidence_threshold`) leaves both `depth` AND `box_level` unchanged (matches existing in-memory-skip rule).
- Returns `changed = true` correctly when only `box_level` moved (not `depth`), so the per-turn save flushes.

#### 5. `primer-pedagogy::prompt_builder` injection tests

- `build_system_prompt` includes vocab section when due-concepts non-empty.
- Omits the section when empty (`due_vocab = []`).
- Omits the section when `learner.concepts` is empty.
- Section appears in the right order (after retrieved-older, before knowledge).
- Uses the locale-keyed `vocab_review_intro()` (verified for both `en` and `de` packs).

#### 6. `primer-pedagogy::dialogue_manager` integration tests

- End-to-end with scripted backend: a concept hits `box_level = 1` after one strong comprehension, becomes due after a 4-day mocked clock advance, is included in the next prompt's vocab section.
- `--vocab-max-per-prompt` override propagates correctly to the prompt.

#### 7. `primer-cli` parse tests

- `--vocab-max-per-prompt 6` parses, threads into settings.
- `--vocab-max-per-prompt 0` rejected at parse with a clear error.

**Estimated total new tests: ~35-40.** Existing 434 stay green.

### Existing test guarantees retained

- All depth-promotion tests in `apply_comprehension` continue to pass — the new `apply_box_transition` call is additive.
- All save/load round-trip tests in `primer-storage` continue to pass — the new column has a default value.
- All prompt-builder section-order tests continue to pass — vocab is inserted between two existing sections without re-ordering them.

## Acceptance criteria

The feature is "done" when:

1. `~/.cargo/bin/cargo test --workspace` passes with new tests + previous 434.
2. `~/.cargo/bin/cargo clippy --workspace --all-targets` is clean.
3. `~/.cargo/bin/cargo fmt --all -- --check` passes.
4. A scripted REPL exchange (`--backend stub --no-persist`) demonstrably injects a vocab section after the second turn for a concept that was strong-comprehended on turn 1, where the test mock advances the clock by 25h.
5. With `--backend cloud` (or ollama) and `RUST_LOG=debug`, a real session shows the vocab section appearing in the system prompt log when concepts age past their interval.
6. v7 migration applies in-place to a v6 DB; previously-saved sessions resume without error and pick up `box_level = 0` for all existing concepts.
7. `--vocab-max-per-prompt 8` overrides the default 4 in a runtime check.
8. README and ROADMAP updated.

## Open risks

1. **Pre-v7 data gets `box_level = 0` on upgrade**, which means every previously-encountered concept becomes due 1 day after its old `last_encountered`. For Phase 0.3 with no field-deployed users, acceptable. If this ever ships to a real user before v7 lands, a backfill that estimates initial box from depth is the right path.
2. **Box state drift across classifier-version upgrades.** New comprehension-classifier versions could change how depth maps to assessments, producing different box transitions than historical data would suggest. The `comprehension_classifiers` lookup table records which classifier emitted each historical assessment, so retro-derivation from `turn_comprehensions` is possible — but not implemented in v1.
3. **No "I just don't want to see this word" escape valve.** A child genuinely uninterested in a concept the extractor latched onto could see it surface every 30d forever. v2 follow-up.
4. **Wallclock dependency.** `chrono::Utc::now()` is called from `build_turn_prompt`. The pure functions take `now` as a parameter (testable), but the production call site reads the system clock. A future "fast-forward time for testing" mode would override at the call site.
5. **The "stuck at Aware" failure mode is shared with the depth-driven design we rejected.** A concept the child genuinely doesn't grasp will surface every 1d forever. Mitigation in practice = the extractor doesn't usually surface concepts the child doesn't engage with at all; concepts that *do* surface but stay weak are the relevant subset, and 1-day surfacing for them is arguably correct (they need re-exposure). If this is too aggressive in real testing, we can tune box-0's interval up without schema impact.
6. **`MIN_CONF_FOR_BOX_PROMOTION` and `comprehension_settings.confidence_threshold` are numerically equal at 0.6 today.** This is intentional but means a casual reader may mistake them for duplicates. The comment in `apply_comprehension` explains.

## Patterns to follow

All inherited from CLAUDE.md and prior session work:

- **Pure functions in `primer-core`** for the algorithmic core. No async, no I/O, no logging.
- **`#[serde(default)]` on new fields** for backward-compatible deserialisation of existing state.
- **Idempotent, transaction-wrapped schema migrations** matching `apply_v2_migrations` through `apply_v6_migrations`.
- **Constants in `consts.rs`, settings struct for tunables.** No magic numbers.
- **Single-locale rendering at the i18n boundary.** `vocab_review_intro` lives in the prompt pack; per-locale TOML provides the translation.
- **TDD discipline.** Tests first, watch them fail, implement to green.
- **File-size hygiene.** Stay under 500 lines per file where reasonable. New `vocab.rs` should land at ~100-150 lines including tests.
- **No new background task.** Box updates run synchronously inside the existing `apply_comprehension` path.
- **Soft-fail policy preserved.** No new failure paths; box state is always consistent or reset to 0.

## Resume / smoke commands

```bash
cd /Users/hherb/src/primer
git checkout -b feature/vocab-spaced-repetition
cd src
~/.cargo/bin/cargo build --workspace
~/.cargo/bin/cargo test --workspace
~/.cargo/bin/cargo clippy --workspace --all-targets
~/.cargo/bin/cargo fmt --all -- --check
```

For end-to-end smoke (after implementation):

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name SmokeTester --age 9 --no-persist \
    --vocab-max-per-prompt 4
# Speak about 4-5 concepts across the session, RUST_LOG=debug to see
# system prompts; verify vocab section appears in turn N+1 after concepts
# age past their interval.
```
