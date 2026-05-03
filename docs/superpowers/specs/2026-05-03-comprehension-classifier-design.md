# Comprehension Classifier — Design Spec

**Date:** 2026-05-03
**Phase:** 0.3 (Pedagogical depth — third bullet)
**Author:** Claude (with Horst Herb)
**Status:** Approved by user, pending implementation plan

## Goal

Add per-concept comprehension assessment to the Primer's per-turn analysis stack. After each completed exchange (child turn → Primer response), classify the depth of understanding the child has demonstrated for each concept they engaged with — mapping observed evidence onto the existing `UnderstandingDepth` enum (`Aware | Recall | Comprehension | Application | Analysis`).

The result feeds two consumers:

1. **In-memory `LearnerModel.concepts`** — `concept.depth` is updated via monotonic max (never demoted by a single weak exchange). This shapes the next exchange's system prompt, which already conveys what the learner has been exposed to.
2. **`turn_comprehensions` table (new, schema v5)** — a longitudinal record of every assessment, ready for offline depth-trajectory analysis without involving the LLM. Includes evidence text and confidence so a future spaced-repetition / forgetting-detection feature can reason over the timeline.

Comprehension is the third LLM-backed structured-output analysis on the per-turn path, after engagement classification (`primer-classifier`) and concept extraction (`primer-extractor`, PR #13). It mirrors their architecture exactly — same trait shape, same `Arc<dyn InferenceBackend>` indirection, same soft-fail policy, same spawn-and-await background pattern.

## Non-goals

- **Forgetting / depth demotion.** The in-memory depth is monotonic (max). Demoting on weak evidence is deferred to a future spaced-repetition feature that operates over the longitudinal `turn_comprehensions` log algorithmically — not via another LLM call.
- **Real-time depth display.** The result is consumed by the system prompt for turn N+1, never surfaced inline to the child.
- **Per-domain classifiers.** One classifier covers all concepts. Per-domain specialization (e.g. a math-specific classifier) is out of scope.
- **Multi-turn comprehension reasoning.** The classifier looks at one exchange (with a small `recent_turns` context) — it does not synthesize across the full session. The `recent_context_turns` setting bounds what it sees.
- **File refactor of `dialogue_manager.rs`** (already 2821 lines, will reach ~2950 after this work). Scheduled for a dedicated session.

## Architecture

### New crate: `primer-comprehension`

Sibling to `primer-classifier` and `primer-extractor`, dependency-equivalent:

```
primer-comprehension/
└── src/
    ├── lib.rs        — ComprehensionClassifier trait + re-exports
    ├── llm.rs        — LlmComprehensionClassifier (Arc<dyn InferenceBackend>)
    ├── stub.rs       — StubComprehensionClassifier (with_response, with_script)
    ├── settings.rs   — ComprehensionSettings
    └── consts.rs     — defaults backing settings (no magic numbers)
```

Cargo deps: `primer-core`, `async-trait`, `serde`, `serde_json`, `tracing`, `tokio` (test only). No dep on `primer-inference` — `Arc<dyn InferenceBackend>` is the only inference touchpoint, identical to `primer-classifier` / `primer-extractor`.

Total workspace crate count goes from 9 → 10.

### New module: `primer-core::llm_util`

Houses the two helpers currently duplicated in `primer-classifier::llm` and `primer-extractor::llm`:

```rust
pub fn extract_first_json_object(raw: &str) -> Option<&str>;
pub fn truncate_to_chars(s: &str, max_chars: usize) -> &str;
```

Both helpers move from their current homes to `primer-core::llm_util`, are re-exported as needed from `primer-classifier::llm` and `primer-extractor::llm` callsites, and the duplicate copies are deleted. Existing tests for these helpers move into `llm_util`'s test module.

`primer-classifier` and `primer-extractor` now both depend on `primer-core::llm_util` — both already depend on `primer-core` so there's no new edge in the dependency graph.

### Small additions to `primer-core::learner`

Add a `UnderstandingDepth::name(self) -> &'static str` method, mirroring the existing `EngagementState::name()` exactly:

```rust
impl UnderstandingDepth {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Aware => "Aware",
            Self::Recall => "Recall",
            Self::Comprehension => "Comprehension",
            Self::Application => "Application",
            Self::Analysis => "Analysis",
        }
    }
}
```

Plus a `Display` impl that delegates to `name()`. The existing `primer-storage::catalog::understanding_depth_name()` helper is rewritten as a one-liner delegator (`UnderstandingDepth::name(depth)`) so the on-disk `understanding_depths` lookup contents stay bit-identical — the v4 validate-and-seed pass keeps passing without any data migration. `primer-comprehension::llm` uses `depth.name()` directly when building the prompt and parsing JSON output, eliminating any need for a `primer-comprehension → primer-storage` dependency.

### New types in `primer-core::comprehension`

```rust
/// One comprehension assessment for one concept on one turn.
pub struct ComprehensionAssessment {
    pub concept: String,
    pub depth: UnderstandingDepth,
    pub confidence: f32,
    pub evidence: Option<String>,
}

/// One classifier output: zero-or-more per-concept assessments.
/// Empty when extraction yielded no concepts, or when the LLM saw
/// no evidence for any candidate concept.
pub struct ComprehensionResult {
    pub assessments: Vec<ComprehensionAssessment>,
}

impl ComprehensionResult {
    pub fn empty() -> Self { Self { assessments: vec![] } }
}

/// Input to the comprehension classifier.
pub struct ComprehensionContext<'a> {
    pub child_turn: &'a Turn,
    pub primer_turn: &'a Turn,
    /// Small slice of preceding turns for context, oldest-first.
    pub recent_turns: &'a [Turn],
    /// Concepts to assess: the union of child_concepts ∪ primer_concepts
    /// from the just-completed exchange's extraction. Already deduplicated
    /// and capped to ComprehensionSettings::max_concepts_per_call.
    pub candidate_concepts: &'a [String],
}
```

`UnderstandingDepth` is the existing enum from `primer-core::learner` (no new variants).

### Trait

```rust
#[async_trait]
pub trait ComprehensionClassifier: Send + Sync {
    /// Stable identifier carrying impl + model. Persisted in the
    /// comprehension_classifiers lookup table; mirrors the format used
    /// by EngagementClassifier / ConceptExtractor.
    fn identifier(&self) -> &str;

    async fn classify(&self, ctx: ComprehensionContext<'_>) -> Result<ComprehensionResult>;
}
```

**Identifier format**: `llm:{backend_name}:{model}` — matches the format adopted by `LlmConceptExtractor` (the most recent precedent). The engagement classifier uses `llm:{model}` (older format); we don't change it now to avoid disturbing existing data.

### Settings

```rust
pub struct ComprehensionSettings {
    pub blocking_timeout: Duration,           // default 1500ms — same as extractor
    pub recent_context_turns: usize,          // default 4   — same as extractor
    pub max_output_chars: usize,              // default 2048 — larger JSON than extractor
    pub max_concepts_per_call: usize,         // default 10  — bounds prompt size
    pub confidence_threshold: f32,            // default 0.6 — below this, persist but don't apply
    pub generation_max_tokens: u32,           // default 512  — larger than extractor's 384
    pub generation_temperature: f32,          // default 0.1  — same as siblings (reproducibility)
    pub generation_top_p: f32,                // default 0.9  — same as siblings
    pub evidence_max_chars: usize,            // default 240  — per-assessment evidence cap
}
```

All values backed by named constants in `consts.rs` per project rule "no magic numbers." Slightly larger output/token defaults reflect the per-concept JSON shape (10 entries × ~60 chars structured content ≈ ~600 chars before envelope).

### Prompt

System message: structured-output instruction asking for one JSON object of the form

```json
{
  "assessments": [
    {"concept": "<from candidates>", "depth": "<one of: Aware, Recall, Comprehension, Application, Analysis>", "confidence": 0.0..=1.0, "evidence": "<one short sentence>"}
  ]
}
```

with explicit instructions:

- Only emit assessments for concepts where the **child's turn** demonstrates depth. Concepts the Primer introduced that the child did not engage with should be **omitted** (not assessed at the lowest level — we want silence, not a noise floor).
- A child's bare acknowledgement ("yeah" / "ok" / "I see") is not evidence of depth; omit those concepts too.
- `Aware` is reserved for "child shows the concept registered" (named it back, asked about it). Higher levels need active demonstration: `Recall` (states a fact), `Comprehension` (own words), `Application` (transfers to a new context), `Analysis` (compares, contrasts, reasons).
- One sentence of evidence per assessment, quoting or paraphrasing the child's words.

User message: prior context (oldest-first), the exchange to analyse, the candidate concept list. Format mirrors `LlmConceptExtractor::build_extraction_prompt`.

### Stub

`StubComprehensionClassifier` with `with_response(ComprehensionResult)` and `with_script(Vec<ComprehensionResult>)` constructors, plus a default that returns `empty()`. Mirrors `StubConceptExtractor`. Used by every test that doesn't specifically test the LLM path.

### Soft-fail policy

Errors NEVER propagate — implementations log via `tracing::warn!` and return `ComprehensionResult::empty()`. Every error path is tested:

- Backend error → empty result
- Unparseable JSON → empty result
- Parsed JSON with concept name not in candidate list → that assessment dropped (others kept)
- Parsed JSON with unknown depth name → that assessment dropped
- Parsed JSON with confidence outside [0.0, 1.0] → that assessment dropped
- Parsed JSON with `evidence` longer than `evidence_max_chars` → truncated (char-boundary-safe)

## DialogueManager wiring

### `DialogueManagerSubsystems` gains two fields

```rust
pub struct DialogueManagerSubsystems {
    pub classifier: Arc<dyn EngagementClassifier>,
    pub classifier_settings: ClassifierSettings,
    pub extractor: Arc<dyn ConceptExtractor>,
    pub extractor_settings: ExtractorSettings,
    pub comprehension: Arc<dyn ComprehensionClassifier>,    // NEW
    pub comprehension_settings: ComprehensionSettings,      // NEW
}
```

Constructor signature unchanged at the outside (still `new(learner, inference, knowledge, stores, subsystems, config)`); the `Subsystems` bundle absorbs the additions cleanly. No `#[allow(clippy::too_many_arguments)]` introduced.

### `DialogueManager` internals

- The existing `extract_task: Option<JoinHandle<Option<ExtractionResult>>>` is **renamed** to `post_response_task: Option<JoinHandle<Option<PostResponseResult>>>`.
- Its result type widens: `PostResponseResult { extraction: ExtractionResult, comprehension: ComprehensionResult }`.
- New field `last_comprehension: Option<ComprehensionResult>` mirrors `last_extraction` for `--verbose`.
- New fields `comprehension: Arc<dyn ComprehensionClassifier>` and `comprehension_settings: ComprehensionSettings`.
- New accessor: `comprehension_identifier(&self) -> &str` (used by `--verbose`).
- New accessor: `last_comprehension(&self) -> Option<&ComprehensionResult>` (used by `--verbose`).

### Chained spawn (replaces today's standalone extractor spawn at `dialogue_manager.rs:603-638`)

The same conditions still gate the spawn (response succeeded, last two turns are child→primer in order). Inside the spawned task body:

```
1. Run extractor.extract(ctx) — soft-fail returns empty().
2. Persist via store.update_exchange_concepts(...). Same as today.
3. Build candidate_concepts = dedup(child ∪ primer), capped to
   comprehension_settings.max_concepts_per_call (preserve order).
4. If candidate_concepts is non-empty:
     Run comprehension.classify(ComprehensionContext { ..., candidate_concepts }).
     Soft-fail returns empty().
   Else: skip comprehension (set comprehension = empty()).
5. If comprehension is non-empty:
     Persist via store.save_comprehensions(session_id, primer_turn_index, &assessments, classifier.identifier()).
6. Return PostResponseResult { extraction, comprehension }.
```

The spawn captures `Arc::clone(&self.comprehension)` and clones `comprehension_settings`. No new spawn handle — the chain lives inside one `tokio::spawn`.

### Renamed `await_pending_extraction` → `await_pending_post_response`

Bounded await with `timeout = extractor_settings.blocking_timeout + comprehension_settings.blocking_timeout` (default 1500 + 1500 = 3000 ms). On the success path:

1. Apply `extraction` to in-memory state via existing `apply_extraction(...)` (sets new concepts to `Aware`, increments encounter counts). Sets `learner_dirty`.
2. Apply `comprehension` to in-memory state via new `apply_comprehension(learner, &result, &settings)` free function:

```rust
pub(crate) fn apply_comprehension(
    learner: &mut LearnerModel,
    result: &ComprehensionResult,
    settings: &ComprehensionSettings,
) -> bool {
    let mut changed = false;
    for a in &result.assessments {
        if a.confidence < settings.confidence_threshold { continue; }
        if let Some(c) = learner.concepts.iter_mut().find(|c| c.concept_id == a.concept) {
            if a.depth > c.depth {
                c.depth = a.depth;
                c.confidence = a.confidence;
                changed = true;
            }
        }
        // If concept missing from learner.concepts (apply_extraction
        // didn't create it because it was empty before extraction),
        // we don't insert here — extraction is the only insertion path.
    }
    changed
}
```

Setting `learner_dirty = true` on any change ensures the per-turn save flushes the new depth.

3. Update `last_comprehension = Some(result)` for `--verbose`.

On timeout: detach (don't abort), so DB persistence still completes. Same policy as today's extractor await.

### `close_session`

Drains both classifier task and post-response task before final save (today drains classifier task and extract_task — same shape, just renamed handle).

## Storage / schema v5

### Migration

Idempotent column-add pattern, transaction-wrapped. New `apply_v5_migrations` function added to `primer-storage::schema`, called after `apply_v4_migrations`.

```sql
-- Lookup for comprehension-classifier identifiers (lazily populated).
CREATE TABLE IF NOT EXISTS comprehension_classifiers (
    id      INTEGER PRIMARY KEY,
    name    TEXT NOT NULL UNIQUE
);

-- Per-turn-per-concept comprehension assessments.
CREATE TABLE IF NOT EXISTS turn_comprehensions (
    id              INTEGER PRIMARY KEY,
    session_id      BLOB NOT NULL REFERENCES sessions(id),
    turn_id         INTEGER NOT NULL REFERENCES turns(id),
    concept_id      INTEGER NOT NULL REFERENCES concepts(id),
    depth_id        INTEGER NOT NULL REFERENCES understanding_depths(id),
    confidence      REAL NOT NULL,
    classifier_id   INTEGER NOT NULL REFERENCES comprehension_classifiers(id),
    evidence        TEXT,
    created_at      TEXT NOT NULL,
    UNIQUE(turn_id, concept_id, classifier_id)
);

CREATE INDEX IF NOT EXISTS idx_turn_comprehensions_turn ON turn_comprehensions(turn_id);
CREATE INDEX IF NOT EXISTS idx_turn_comprehensions_concept ON turn_comprehensions(concept_id);
```

`turn_id` references the **Primer** turn (the one that completes the exchange). The `UNIQUE(turn_id, concept_id, classifier_id)` constraint makes re-running the same classifier on the same turn idempotent (`INSERT OR IGNORE`); a different `classifier_id` (future model upgrade) lands as a parallel row.

`understanding_depths` lookup already exists from v4. `concepts` lookup already exists (lazy from extractor work). Both reused without modification.

`USER_VERSION` advances from 4 to 5. Reject `existing_version > 5`; advance silently otherwise.

Pre-production: no production data exists yet, so accidental wipe paths are not a concern. Migrations remain idempotent + transaction-wrapped for the discipline.

### `SessionStore` trait gains one method

```rust
async fn save_comprehensions(
    &self,
    session_id: SessionId,
    primer_turn_index: usize,
    assessments: &[ComprehensionAssessment],
    classifier_identifier: &str,
) -> Result<()>;
```

Implementation pattern, mirroring `save_classification`:
1. Resolve `(session_id, primer_turn_index)` → `turn_id`. Missing turn returns `Err`.
2. Resolve or insert `classifier_identifier` into `comprehension_classifiers` lookup → `classifier_id`.
3. For each assessment:
   a. Resolve or insert `concept` into `concepts` lookup → `concept_id`.
   b. Resolve `depth` to `depth_id` from the existing `understanding_depths` lookup.
   c. `INSERT OR IGNORE INTO turn_comprehensions(...)`.
4. Whole call wrapped in a single `conn.unchecked_transaction()`.

Empty `assessments` slice is a no-op (no transaction needed).

## CLI surface

### New flags in `primer-cli` (mirror extractor pattern verbatim)

| Flag | Default | Notes |
|---|---|---|
| `--comprehension-backend stub\|cloud\|ollama` | same as `--backend` | parallels `--classifier-backend` / `--extractor-backend` |
| `--comprehension-model <name>` | same as `--model` | required for ollama when `--backend` ≠ ollama |
| `--comprehension-timeout-ms <ms>` | 1500 | parallels `--extractor-timeout-ms` |

`build_comprehension` dispatch matrix in `main.rs` is a verbatim parallel of `build_classifier` and `build_extractor`. Five construction-test cells: stub/stub, cloud/cloud, ollama/ollama, mixed (`--backend cloud --comprehension-backend ollama`), mixed (`--backend ollama --comprehension-backend cloud`).

### `--verbose` extension

After `[intent]` / `[classifier]` / `[extractor]`, a new line per turn:

```
[comprehension] photosynthesis=Comprehension(0.85) chlorophyll=Aware(0.6) (llm:cloud-anthropic:claude-sonnet-4-6)
```

Format: `<concept>=<Depth>(<confidence:.2>)` space-separated, then `(<classifier_identifier>)`. Empty assessments line: `[comprehension] (none) (llm:...)`.

Depth names come from a new `UnderstandingDepth::name()` method (added in `primer-core::learner`, mirroring the existing `EngagementState::name()` exactly). The existing `primer-storage::catalog::understanding_depth_name()` helper becomes a thin delegator to it, so the on-disk lookup-table contents (`understanding_depths`) remain bit-identical and the v4 lookup-validate-and-seed pass keeps passing without a data migration. This name function is also what the LLM prompt and JSON parser use, so depth-name-string consistency across the prompt, the parser, the verbose line, and the schema lookup is structurally guaranteed.

## Testing strategy

TDD throughout (project rule + brief precedent).

### Per-crate test surface

- **`primer-core::comprehension`** — types compile; `ComprehensionResult::empty()` returns empty assessments; `ComprehensionAssessment` round-trips through serde.
- **`primer-core::llm_util`** — both helpers tested (existing test cases moved verbatim from the two existing duplicate locations).
- **`primer-comprehension::settings`** — defaults equal the consts.
- **`primer-comprehension::stub`** — `with_response` and `with_script` behave deterministically; default returns empty.
- **`primer-comprehension::llm`** — well-formed JSON parses; wrapper-text tolerated; backend error → empty; unparseable → empty; concept name not in candidates → assessment dropped; unknown depth name → dropped; out-of-range confidence → dropped; evidence over `evidence_max_chars` → truncated.
- **`primer-storage`** — v5 migration idempotent; `save_comprehensions` round-trips; UNIQUE constraint enforced (re-save no-op); empty slice no-op; missing turn returns `Err`; `comprehension_classifiers` lookup populated lazily.
- **`primer-pedagogy`** — chained spawn persists both extraction + comprehension; chain skips comprehension on extraction failure (extractor returned empty); chain skips comprehension on empty concept union (no concepts to assess); next-turn await applies extraction THEN comprehension; comprehension below `confidence_threshold` is persisted but not applied to in-memory state; `apply_comprehension` is monotonic-max; `close_session` drains the renamed task; `last_comprehension` accessor returns the latest applied result.
- **`primer-cli`** — five `comprehension_construction_tests` cells matching the build_* dispatch matrix.

Estimated test delta: **~30 new tests**, plus ~6 moved tests when consolidating helpers. Test-count target: 275 → ~305.

### Smoke verification

Manual verification via `--verbose`. Acceptable backends:

- **Anthropic** (default `--backend cloud`, key in `ANTHROPIC_API_KEY` or `~/.primer_env`).
- **Local ollama** (`--backend ollama --model qwen3.6:35b-a3b-q8_0`). Latency may be high on the user's box right now (model under heavy load with another workload); not a concern at this stage.

Pass criterion: across a handful of exchanges with mixed-depth child responses, the `[comprehension]` line populates with plausible depth assignments. No assertion of "correct" depth — this is a gut-check, not an evaluation suite. (A real eval suite is a downstream task.)

## File budget

`dialogue_manager.rs` is 2821 lines today. The chain-and-await changes add ~80–120 lines (rename, new field, `apply_comprehension` free function, chain inside one spawn body, comprehension await + apply step). New total: ~2900–2950 lines.

Per CLAUDE.md guidance ("try to keep code files under 500 lines where reasonably possible, refactor when files grow too big where reasonable"), this file is well over budget already. **Refactoring `dialogue_manager.rs` is deferred to a dedicated session.** The natural axes for the split: lifecycle (open / resume / close), per-turn hot path (`respond_to_streaming`), background analysis (the spawn-and-await machinery), and helper functions (`apply_*`, `merge_concepts_into_turn`, etc). Flagged in NEXT_SESSION.md.

All other files stay well under 500 lines. New `primer-comprehension` files target ≤300 lines each.

## Documentation updates

### CLAUDE.md

- Update the dependency-graph diagram to include `primer-comprehension`.
- Add a paragraph on `primer-comprehension` (mirror the `primer-extractor` paragraph: trait shape, soft-fail policy, settings).
- Update the `primer-pedagogy` paragraph: the post-response background path is now `(classifier task) || (extractor → comprehension chain)`; the await applies extraction first then comprehension; `apply_comprehension` is monotonic-max-with-confidence-threshold.
- New gotcha bullet: "Chained spawn for extraction → comprehension." The combined timeout is ~3s; on timeout the task detaches so DB writes still complete.
- New gotcha bullet: "`apply_comprehension` is monotonic-max." A weak exchange never demotes; explicit forgetting is a Phase 0.3+ concern handled algorithmically over `turn_comprehensions`, not via another LLM call.
- New gotcha bullet: "Per-concept comprehension assessments are persisted even when sub-threshold." `confidence_threshold` only gates the in-memory apply; the longitudinal record gets every assessment.
- CLI flags paragraph updated with the three new flags + `[comprehension]` in `--verbose`.

### ROADMAP.md

Tick the "LLM-based comprehension classifier" bullet in Phase 0.3.

### README.md

Bump the development-stage description: third LLM-side analysis (engagement, concept extraction, comprehension) now in place. Test count update.

### NEXT_SESSION.md

Updated at end-of-session per the `/nextsession` protocol with: shipped artifacts (PR + commit SHAs), next-task options (vocabulary tracking, break suggestions, graceful API errors, `dialogue_manager.rs` split), open decisions, exact resume commands.

## Verification gates (before declaring done)

Per `verification-before-completion` discipline:

- `~/.cargo/bin/cargo build --workspace` clean
- `~/.cargo/bin/cargo test --workspace` passes (~305 tests)
- `~/.cargo/bin/cargo clippy --workspace --all-targets` 100% clean (no warnings)
- `~/.cargo/bin/cargo fmt --all -- --check` clean
- `~/.cargo/bin/cargo build --workspace --features primer-cli/speech` clean
- Manual smoke (Anthropic or local ollama): `--verbose` shows plausible `[comprehension]` lines across multiple exchanges

## Open questions / risks

1. **Identifier-format inconsistency.** `LlmEngagementClassifier::identifier()` uses `llm:{model}` (older pattern); `LlmConceptExtractor` uses `llm:{backend}:{model}` (newer pattern). Comprehension follows the newer pattern. This is a minor inconsistency in the existing code; not addressed here. If ever harmonized, migrate `engagement_classifiers.name` values forward in a separate one-time data-fix migration.

2. **`apply_comprehension` insertion policy.** `apply_comprehension` only promotes depths on concepts that are already present in `learner.concepts` — it never inserts. The chain ensures `apply_extraction` runs first (which inserts every candidate at `Aware`), so under normal operation every assessment finds a home. Two corner cases are intentionally tolerated rather than defended against: (a) a misbehaving LLM that emits an assessment for a concept not in `candidate_concepts` — dropped at the parser layer, never reaches `apply_comprehension`; (b) future re-classification of historical turns where `learner.concepts` has since drifted — those assessments persist in `turn_comprehensions` but find no in-memory target, which is correct (we don't want a re-classification to retroactively yank present-day depths). Documented in code comments on `apply_comprehension`.

3. **Combined timeout budget.** `extractor_timeout + comprehension_timeout` (default 3s) eats further into the inter-turn pause. If the child responds within 3s of the Primer's last word, the in-memory state for turn N+1 may not yet reflect this turn's analysis — the previous turn's data still drives intent selection and the system prompt. This is the same lag pattern as today (extractor lags one turn into in-memory state); now both extraction and comprehension lag together. Documented in CLAUDE.md gotchas.

4. **No retry / circuit-breaker.** A persistently failing comprehension backend just generates `tracing::warn!` per turn forever. Acceptable for v1; addressed by the broader graceful-API-errors task (Option B in NEXT_SESSION.md).

5. **`evidence` text retention.** Stored as plain `TEXT` in `turn_comprehensions`, capped to `evidence_max_chars` at the LLM-output layer. Not normalized into a separate table. If evidence text becomes a bulk-storage concern later, a pruning policy can be added without a schema change.

## Acceptance criteria

A successful PR:

1. Builds clean across all four cargo invocations listed in "Verification gates."
2. Passes ~305 tests (delta of ~30, mostly new + ~6 moved).
3. Adds `primer-comprehension` crate per the architecture section.
4. Adds `primer-core::llm_util` and removes the two duplicates.
5. Adds schema v5 migration and `SessionStore::save_comprehensions`.
6. Wires comprehension into the dialogue manager via the chained spawn pattern.
7. Adds the three CLI flags + `[comprehension]` `--verbose` line.
8. Updates CLAUDE.md, ROADMAP.md, README.md per the docs-update section.
9. Manual smoke against either Anthropic or local ollama produces plausible `[comprehension]` output.
10. No regression in any existing test or `--verbose` behaviour.
