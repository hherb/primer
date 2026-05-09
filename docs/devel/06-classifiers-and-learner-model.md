# Classifiers and the learner model

This chapter is about the three structured-output LLM tasks that run alongside the conversation — engagement, concept extraction, and per-concept comprehension — and the [LearnerModel](../../src/crates/primer-core/src/learner.rs) they update. They share a single shape: a small trait, an LLM-backed implementation that wraps `Arc<dyn InferenceBackend>`, and a stub for tests. They share a single execution pattern: spawn at the end of turn N, await with a bounded timeout at the start of turn N+1, detach on timeout. They share a single failure mode: never propagate. And they share a single source of truth for the data they produce: the [LearnerModel](../../src/crates/primer-core/src/learner.rs) umbrella struct, where the engagement-state snapshot, the concept-mastery record, and the spaced-repetition box levels all live together.

If you read chapter 3 you already saw the dialogue manager's turn loop spawn these tasks; this chapter is what they actually do.

## The structured-output trio

Three crates, each with the same five files (`lib.rs`, `llm.rs`, `stub.rs`, `settings.rs`, `consts.rs`) and the same export pattern:

| Crate | Trait | LLM impl | Stub | Output |
|---|---|---|---|---|
| [primer-classifier](../../src/crates/primer-classifier/src/lib.rs) | `EngagementClassifier` | `LlmEngagementClassifier` | `StubEngagementClassifier` | `EngagementAssessment` |
| [primer-extractor](../../src/crates/primer-extractor/src/lib.rs) | `ConceptExtractor` | `LlmConceptExtractor` | `StubConceptExtractor` | `ConceptExtraction` |
| [primer-comprehension](../../src/crates/primer-comprehension/src/lib.rs) | `ComprehensionClassifier` | `LlmComprehensionClassifier` | `StubComprehensionClassifier` | `ComprehensionResult` |

Each LLM impl wraps `Arc<dyn InferenceBackend>` rather than depending on `primer-inference` directly — that's what keeps these crates' build graphs free of the concrete backends, and what lets a memory-constrained device reuse the chat model while a bigger machine swaps in something cheaper. The wrapping is `Arc<dyn ...>` not `&dyn ...` because the dialogue manager spawns these calls into `tokio::spawn`, which requires the captured backend to be `'static`.

Each stub provides two test-ergonomic constructors: `with_response(canned)` returns the same value forever, and `with_script(vec)` walks a queue and falls back to a default once exhausted. The pattern is consistent across the trio so test fixtures translate cleanly between them. See for example [stub.rs in primer-classifier](../../src/crates/primer-classifier/src/stub.rs) — the comprehension and extractor stubs follow the identical shape.

Every LLM impl carries a stable string `identifier()` that gets persisted alongside its output. The classifier writes into `turn_classifications.classifier_id`, the comprehension classifier into `turn_comprehensions.classifier_id`. The identifier embeds the backend name and the model name, so re-classifying an old session DB with a different model later produces parallel rows rather than overwriting history. Don't mint identifiers ad hoc — the existing constructors generate them for you (`format!("llm:{}:{}", backend.name(), model)` in the extractor; `format!("llm:{model}")` in the classifier).

## The spawn / await / detach-on-timeout pattern

All three classifiers run on the same cadence. At the end of turn N — after the Primer's response has streamed and the session has been saved — the dialogue manager spawns two background tasks: the engagement classifier on its own, and the extractor → comprehension chain together. At the start of turn N+1, both are awaited concurrently via `tokio::join!` with a bounded timeout. On timeout the task is dropped from the await, but the underlying `tokio::spawn` future continues to run to completion in the background — its database persistence still lands.

The classifier's timeout is `ClassifierSettings::blocking_timeout` (default 3000ms). The extractor → comprehension chain shares a combined budget of `ExtractorSettings::blocking_timeout + ComprehensionSettings::blocking_timeout` (default 5000 + 5000 = 10000ms). At the workspace defaults this caps the worst-case wallclock at `max(3000, 10000) = 10s` rather than the naive sum of 13s — see [background.rs](../../src/crates/primer-pedagogy/src/dialogue_manager/background.rs) for the `tokio::join!` site.

These numbers are deliberately generous. Smoke testing against small local models (gemma4:e4b, qwen3.5:4b) showed that a 1500ms extractor budget would have meant most chains never finished within the wait, so the in-memory state would lag two turns instead of one. The natural inter-turn pause for a child reading the previous response and thinking about a reply absorbs a 10-second budget without any UX impact — the timeout exists to catch a stuck backend, not to bound a healthy one.

> **Why:** The classifier and chain awaits run with `tokio::join!` rather than sequential awaits because the two tasks write to disjoint fields of the learner. Joining them halves the worst-case wallclock and lets the next turn start sooner if either task finishes early.

## Soft-fail discipline

The three classifiers are advisory — they sharpen the Primer's behaviour but they are not on the critical path of producing a response. A failed classification is recoverable; a propagated error that aborts a child's turn is not. So the rule across all three crates is absolute: **errors never propagate**.

In practice this looks like the body of `LlmEngagementClassifier::classify` in [llm.rs](../../src/crates/primer-classifier/src/llm.rs):

```rust
// src/crates/primer-classifier/src/llm.rs
let raw = match self.backend.generate(&prompt, &params).await {
    Ok(r) => r,
    Err(e) => {
        warn!(classifier = %self.identifier, error = ?e, "classifier backend call failed");
        return Ok(EngagementAssessment::unknown_low_confidence(format!(
            "backend error: {e}"
        )));
    }
};
```

A backend error becomes a `tracing::warn!` line plus an `Ok(EngagementAssessment::unknown_low_confidence(...))`. Same shape in the extractor (`Ok(ConceptExtraction::empty())`) and the comprehension classifier (`Ok(ComprehensionResult::empty())`). Parser failures do the same. The function signature still says `Result<...>`, but the `Err` arm is structurally unreachable on the hot path — we keep it because the return type is shared with future non-LLM implementations that may legitimately want to surface a hard failure.

> **Gotcha:** Classifier, extractor, and comprehension errors NEVER propagate. If you find yourself adding `?` on one of their results, you have a bug — log via `tracing::warn!` and return the empty/unknown result instead.

## The chain inside the dialogue manager

The post-response work fans out into two independent branches: the classifier task on one side, and the extractor → comprehension chain on the other. The chain is a chain because comprehension needs the candidate concepts that extraction surfaced — there is no point asking "how deeply does the child understand X" when X hasn't been mentioned. The two branches join at the start of the next turn, where [`apply_post_response_outcome`](../../src/crates/primer-pedagogy/src/dialogue_manager/background.rs) applies extraction first (so any new concepts land in `learner.concepts` at depth `Aware`) and then comprehension (so [`apply_comprehension`](../../src/crates/primer-pedagogy/src/dialogue_manager/apply.rs) can promote depths and advance Leitner-box levels for those concepts).

DB persistence runs inside the spawned tasks themselves, not in the apply step. The extractor calls `SessionStore::update_exchange_concepts` from inside its `tokio::spawn`; the comprehension classifier calls `save_comprehensions` the same way. So the per-turn rows in `turn_concepts` and `turn_comprehensions` land as soon as the LLM returns — even if the dialogue manager has already moved on and the await fires on a stale future.

> **Gotcha:** Concepts and comprehension lag one turn for in-memory state but NOT for DB persistence. The system prompt for turn N does not include concepts surfaced in turn N — only those from earlier turns. Per-turn rows in `turn_concepts` and `turn_comprehensions` are written immediately by the spawned tasks; only the `LearnerModel.concepts` field updates at the start of the next turn.

This is acceptable for v1. If it ever surprises a child ("I just told you about photosynthesis and you're acting like I didn't"), the fix is to apply extraction synchronously before the next prompt build — the storage path already works.

## The LearnerModel umbrella

`LearnerModel` is composition, not a leaf. It pulls four pieces together:

```rust
// src/crates/primer-core/src/learner.rs
pub struct LearnerModel {
    pub profile: LearnerProfile,
    pub concepts: Vec<ConceptState>,
    pub preferences: LearningPreferences,
    pub current_engagement: EngagementState,
    #[serde(default)]
    pub recent_assessments: Vec<EngagementAssessment>,
}
```

`LearnerProfile` carries identity (name, age, ISO 639-1 language list, bound `Locale`). `ConceptState` is one row per concept with its `depth: UnderstandingDepth`, `confidence`, `encounter_count`, `last_encountered`, free-form `notes`, and `box_level: u8` for the spaced-repetition scheduler. `LearningPreferences` is the open-vocabulary side — narrative-vs-Socratic-vs-visual-vs-kinesthetic preference scores plus a list of high-engagement topics — and `current_engagement` plus `recent_assessments` is the rolling output of the engagement classifier.

When you extend the learner, edit the leaf type that owns the field, not the umbrella. Adding a new style preference goes in `LearningPreferences`. A new per-concept signal goes in `ConceptState`. The umbrella is just composition.

The split between open-vocabulary and closed-enum data is deliberate. `LearningPreferences` serialises as JSON because children's interests don't fit a closed enum — "loves dragons" needs to coexist with "fascinated by tide pools" without a schema migration. `UnderstandingDepth`, by contrast, is a closed Rust enum (Unknown / Aware / Recall / Comprehension / Application / Analysis) backed by a normalised lookup table in [primer-storage](../../src/crates/primer-storage/src/store/mod.rs), because depth has a fixed semantic ladder and we want the classifier's output validated against it.

> **Why:** `LearningPreferences` is JSON because children's interests don't fit a closed enum; `UnderstandingDepth` is a closed enum with a lookup table because depth has a fixed semantic ladder and we want compile-time enforcement of which values can appear in a row.

## apply_comprehension is monotonic-max

`apply_comprehension` in [apply.rs](../../src/crates/primer-pedagogy/src/dialogue_manager/apply.rs) is the function that turns one `ComprehensionResult` into in-memory updates to the learner. It is monotonic-max in two senses. First, depth never demotes: if the learner is recorded at `Comprehension` and a weak exchange comes in with `Aware`, the depth is left untouched. Second, the comparison is by enum order — the function only writes back when `assessment.depth > current.depth`. A weak read can never claw back ground the child has already covered.

Explicit forgetting is a deliberately-deferred concern. When it arrives it will be an algorithm running offline over the longitudinal `turn_comprehensions` record, not another in-line LLM call — the longitudinal record is already there waiting for it.

Sub-threshold assessments (those with `confidence < ComprehensionSettings::confidence_threshold`, default 0.6) are NOT applied to in-memory state, but they ARE persisted to `turn_comprehensions` in full. The threshold gates the in-memory step only; the longitudinal record gets every assessment, with confidence scores intact, so an offline analyser can apply different filters later without re-running the LLM.

The box-level transition runs on every accepted assessment, even when depth doesn't move. A confident re-confirmation at the same depth is the spaced-repetition mechanism for expanding intervals — without that, a concept stuck at `Comprehension` would never advance from box 1 to box 4 even after years of re-engagement. See the long comment in [apply.rs](../../src/crates/primer-pedagogy/src/dialogue_manager/apply.rs) on `apply_comprehension` for why the inner confidence guard inside `apply_box_transition` is load-bearing despite looking redundant at the default settings.

## Vocabulary spaced repetition

There is no separate "vocabulary" subsystem. Vocabulary entries are `ConceptState` rows — the same rows that the comprehension classifier promotes — with their `box_level` field tracking the Leitner-box stage. Keeping the schema flat is a deliberate choice: the longitudinal record stays a single table, which preserves the future anonymisation and contribution path the roadmap calls out.

The scheduler is three pure functions in [vocab.rs](../../src/crates/primer-core/src/vocab.rs):

- `apply_box_transition(current_box, depth, confidence) -> u8` decides the new box. Three zones: low confidence resets to box 0; `depth = Aware` resets to box 0 regardless of confidence; otherwise advance one box, capped at `MAX_BOX_LEVEL = 4`. Called from `apply_comprehension`.
- `is_due(concept, now) -> bool` checks whether a concept's review interval has elapsed against the box-keyed table `[1d, 3d, 7d, 14d, 30d]`.
- `due_concepts(learner, now, max_n) -> Vec<&ConceptState>` returns the top-N most-overdue concepts, sorted by overdue-amount descending with `concept_id` as a stable tiebreaker for reproducibility across resumes.

All three take `now` as a parameter so tests drive them deterministically; production passes `chrono::Utc::now()`. The dialogue manager calls `due_concepts` once per turn in [`build_turn_prompt`](../../src/crates/primer-pedagogy/src/dialogue_manager/turn.rs), capped at `VocabSettings::max_per_prompt` (default 4, configurable via `--vocab-max-per-prompt`), and the prompt builder injects them as a passive review hint. The system prompt instructs the LLM to weave words in only if topically relevant — never drill, never quiz.

Driving everything off the comprehension classifier means **no extra LLM call**. The same assessment that promotes `depth` also advances `box_level`. Spaced repetition is a side effect of the existing chain.

## Shared utilities

Two helpers shared across all three crates live in [primer_core::llm_util](../../src/crates/primer-core/src/llm_util.rs):

- `extract_first_json_object(raw: &str) -> Option<&str>` walks the input bytes once, tracking string and escape state, and returns the first balanced JSON object — including outer braces, tolerating wrapper text before and after, and correctly ignoring braces inside string literals.
- `truncate_to_chars(s: &str, max_chars: usize) -> &str` returns a prefix bounded by Unicode scalar values rather than bytes, so it never panics on a multi-byte boundary.

Both helpers used to live duplicated in each crate; they were pulled into `primer-core` so a fix to either applies everywhere. Don't reintroduce per-crate copies — the duplication that motivated the move is exactly what the move solved.

---

### Recipe — Add a structured-output classifier

Worked example: a hypothetical "tone classifier" that scores child turns on a `cheerful / neutral / anxious` ladder. The pattern below mirrors the engagement classifier; if you'd rather follow the per-concept shape, [primer-comprehension](../../src/crates/primer-comprehension/src/) is the closer template.

**1. Define the trait in a new crate `primer-tone`** following the pattern at [primer-classifier/src/lib.rs](../../src/crates/primer-classifier/src/lib.rs). Declare the public types in `primer-core` (output struct + context struct), and expose the trait + a stable `identifier()` from the new crate. The output type is something like `ToneAssessment { tone: ToneState, confidence: f32, reasoning: Option<String> }` — with a `ToneAssessment::unknown_low_confidence(reason: String)` constructor for the soft-fail path.

**2. Implement `LlmToneClassifier`** wrapping `Arc<dyn InferenceBackend>`. Use [`extract_first_json_object`](../../src/crates/primer-core/src/llm_util.rs) from `primer_core::llm_util` to parse the model's response and [`truncate_to_chars`](../../src/crates/primer-core/src/llm_util.rs) to bound the input. Mirror the soft-fail idiom from [primer-classifier/src/llm.rs](../../src/crates/primer-classifier/src/llm.rs):

```rust
// src/crates/primer-tone/src/llm.rs (sketch)
let raw = match self.backend.generate(&prompt, &params).await {
    Ok(r) => r,
    Err(e) => {
        warn!(classifier = %self.identifier, error = ?e, "tone backend call failed");
        return Ok(ToneAssessment::unknown_low_confidence(format!(
            "backend error: {e}"
        )));
    }
};
let truncated = truncate_to_chars(&raw, self.settings.max_output_chars);
let assessment = match extract_first_json_object(truncated) {
    Some(s) => match serde_json::from_str::<ToneOutput>(s) {
        Ok(out) => out.into(),
        Err(e) => {
            tracing::warn!(target: "primer::tone", "parse error: {e}");
            ToneAssessment::unknown_low_confidence(format!("parse error: {e}"))
        }
    },
    None => {
        tracing::warn!(target: "primer::tone", "no JSON object in LLM output");
        ToneAssessment::unknown_low_confidence("no JSON object".into())
    }
};
Ok(assessment)
```

**3. Implement `StubToneClassifier`** with `with_response(ToneAssessment)` and `with_script(Vec<ToneAssessment>)` constructors. Used for tests; the script falls back to a default once exhausted. Pattern is verbatim from [primer-classifier/src/stub.rs](../../src/crates/primer-classifier/src/stub.rs).

**4. Add `ToneSettings`** with every numeric (`blocking_timeout`, `recent_context_turns`, `max_output_chars`, `confidence_threshold`, `generation_max_tokens`, `generation_temperature`, `generation_top_p`) backed by constants in [primer-core/src/consts.rs](../../src/crates/primer-core/src/consts.rs) (or a per-crate `consts.rs` if scoped narrowly). Add a `defaults_match_consts` test mirroring the one in each existing settings file. Never inline a numeric.

**5. Add a schema migration** if you need to persist (cross-link to chapter 5). For tone you'd add `turn_tones` UNIQUE on `(turn_id, classifier_id)`, plus a `tone_classifiers` registry, plus a `tones` lookup table seeded from the `ToneState` enum. Bump `USER_VERSION`, write `apply_v9_migrations`, wire it into the open path, add round-trip + idempotency tests. Chapter 5's recipe is the template.

**6. Wire the spawn/await pattern** into the dialogue manager in [background.rs](../../src/crates/primer-pedagogy/src/dialogue_manager/background.rs) following the classifier model. Hold the classifier as `Arc<dyn ToneClassifier>` because `tokio::spawn` requires `'static`. Either add a third branch to `await_pending_background` or join it onto the existing classifier branch — depends on whether tone needs to influence intent (parallel branch) or just track longitudinally (fire-and-forget).

**7. Add CLI flags** following the `--classifier-*` template in [primer-cli/src/main.rs](../../src/crates/primer-cli/src/main.rs): `--tone-backend`, `--tone-model`, `--tone-timeout-ms`. Default to mirroring the main `--backend` / `--model` so the simple case requires zero new flags. Update the flags reference in [CLAUDE.md](../../CLAUDE.md).

**8. Soft-fail tracing.** Confirm every error path inside `LlmToneClassifier` produces a `tracing::warn!(classifier = %self.identifier, ...)` line and returns `ToneAssessment::unknown_low_confidence(...)`. Never return `Result::Err` from the public API on the hot path. Follow the test pattern in [primer-classifier/src/llm.rs](../../src/crates/primer-classifier/src/llm.rs) — the `classify_returns_unknown_on_*` tests pin every error branch.

**9. Tests.** Stub-driven, following [test_support.rs](../../src/crates/primer-pedagogy/src/dialogue_manager/test_support.rs): feed a script of canned `ToneAssessment`s, drive a mock dialogue, assert the persistence row at the end of turn N and the in-memory `LearnerModel` state at the start of turn N+1. Add a timeout test that constructs a deliberately-slow stub and asserts that detach-on-timeout leaves prior state intact (no panic, no propagated error, log line emitted). Add a parser-failure test that returns malformed JSON and asserts the `unknown_low_confidence` fallback.
