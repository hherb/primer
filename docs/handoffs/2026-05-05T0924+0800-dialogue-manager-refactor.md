# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-05 (after the dialogue_manager refactor branch landed: 3572-line file split across 9 production + 3 test files, respond_to_streaming decomposed into 7 named helpers; 434 tests still green).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root** — every cargo command runs from `src/`. The `dialogue_manager` is now a directory module; see the gotcha that explains the layout.
2. Skim [ROADMAP.md](ROADMAP.md). Phase 0.1 is fully closed. Phase 0.3 has two open bullets: per-child vocabulary tracking with spaced repetition; session-time tracking with break suggestions. Plus a long tail of i18n / locale follow-ups merged across May 4–5.
3. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test`. Should be green: **434 tests** across the workspace.
4. From `src/`: `~/.cargo/bin/cargo clippy --workspace --all-targets`. Should be 100 % clean. `~/.cargo/bin/cargo fmt --all -- --check` should also pass.
5. **Don't assume nothing changed since this brief was written.** Read the current state of the files you intend to touch first — Horst may have made interim changes.

## Branch status

The work for this session is on branch **`feature/dialogue-manager-refactor`**, currently 10 commits ahead of `main` (9 implementation + 1 spec). Not yet pushed. The branch passes all verification gates. Remaining choice for Horst:

```bash
# Option A: open a PR (matches PR #15/#17/#23/etc. pattern)
cd /Users/hherb/src/primer
git push -u origin feature/dialogue-manager-refactor
gh pr create --base main --head feature/dialogue-manager-refactor \
  --title "Refactor dialogue_manager into a directory module" \
  --body-file docs/handoffs/2026-05-05T0924+0800-dialogue-manager-refactor.md  # or compose afresh

# Option B: merge directly (formatting/refactor; low blast radius)
git checkout main && git merge --ff-only feature/dialogue-manager-refactor
git branch -d feature/dialogue-manager-refactor
```

Option A is the established pattern. Option B is defensible since this is purely structural.

## What's now on the feature branch (so you don't redo it)

### dialogue_manager refactor (this session, branch `feature/dialogue-manager-refactor`)

The 3572-line `crates/primer-pedagogy/src/dialogue_manager.rs` was split into a directory module. Spec at [docs/superpowers/specs/2026-05-04-dialogue-manager-refactor-design.md](docs/superpowers/specs/2026-05-04-dialogue-manager-refactor-design.md).

**Final layout:**

```
crates/primer-pedagogy/src/dialogue_manager/
├── mod.rs                  212  struct + type aliases + module declarations
├── lifecycle.rs            245  new / open_session / resume_session / close_session
│                                 + small accessors (last_*, *_identifier,
│                                 should_suggest_break)
├── turn.rs                 491  respond_to (thin wrapper) + respond_to_streaming
│                                 (~30-line orchestrator) + 7 private helpers
│                                 (record_child_turn, build_turn_prompt,
│                                  stream_inference_response, record_primer_turn,
│                                  persist_turn, spawn_classification_task,
│                                  spawn_post_response_task)
├── background.rs           178  await_pending_* / drain_* / apply_*_outcome
├── retrieval.rs             57  retrieve_knowledge, retrieve_long_term_memory
├── summary.rs               72  refresh_summary_if_due / _if_stale +
│                                 regenerate_summary_through
├── learner_update.rs        53  update_learner_model (placeholder heuristic)
├── apply.rs                427  pure free fns + their 8 unit tests
├── test_support.rs         503  #[cfg(test)] shared mocks (ScriptedBackend,
│                                 EmptyKnowledge, CountingStore,
│                                 CountingLearnerStore, RepeatingBackend,
│                                 ConceptCapturingStore) + builders
└── tests/
    ├── lifecycle_tests.rs  412  12 tests (open/resume/close + close-related)
    ├── turn_tests.rs       478  15 tests (respond_to_streaming + per-turn save
    │                                 + summary + FailingLearnerStore mock)
    └── background_tests.rs 761  14 tests (classifier spawn-await +
                                          post-response chain)
```

**Two acknowledged soft-cap violations** (documented in the spec's Risk register and commit messages):
- `test_support.rs` at 503 lines (under the spec's 700 estimate; cohesion of mocks)
- `background_tests.rs` at 761 lines (splitting classifier-tests from post-response-tests would be artificial)

All other files are under 500.

**The 9 commits in order:**

| # | Commit | What landed |
|---|---|---|
| 1 | `b821230` | Move `dialogue_manager.rs` → `dialogue_manager/mod.rs` (pure relocation, zero diff). |
| 2 | `c5a86e9` | Extract `apply.rs` with the 4 pure free fns + 8 inline unit tests. Visibility tightened from `pub(crate)` to `pub(super)`. |
| 3 | `e847e78` | Extract `test_support.rs` with universally-shared mocks + builders. |
| 4 | `58c4476` | Extract `retrieval.rs` + `summary.rs` + `learner_update.rs` (small impl modules, batched). |
| 5 | `a08507f` | Extract `background.rs` (await/drain/apply_*_outcome machinery). |
| 6 | `49a1ed8` | Extract `lifecycle.rs` (new/open/resume/close + accessors). |
| 7 | `1440929` | **Decompose `respond_to_streaming` into 7 named private methods** (still in `mod.rs`). The only behavioral-restructuring commit. |
| 8 | `789bc8f` | Extract `turn.rs` (move the decomposed orchestrator + helpers). |
| 9 | `65829c7` | Split tests into `tests/{lifecycle,turn,background}_tests.rs`. Promoted `RepeatingBackend`, `ConceptCapturingStore`, `dirty_flag_test_setup` to `test_support.rs` since they're used across files. |

Plus `d58ec82` for the spec doc itself.

**Test count delta:** 434 → 434 (no test changes; the spec explicitly committed to "no new tests, no removed tests").

**Verified state at branch tip:**
- `~/.cargo/bin/cargo build --workspace` → clean.
- `~/.cargo/bin/cargo test --workspace` → **434 passed, 0 failed.**
- `~/.cargo/bin/cargo clippy --workspace --all-targets` → clean.
- `~/.cargo/bin/cargo fmt --all -- --check` → clean.
- `~/.cargo/bin/cargo build --workspace --features primer-cli/speech` → clean.

**Public API verified unchanged:** `primer-cli/src/main.rs` and `primer-cli/src/speech_loop.rs` compile without modification — they consume `DialogueManager`, `DialogueManagerStores`, `DialogueManagerSubsystems` exactly as before.

### Carried over from prior sessions (still relevant)

- i18n PRs 1–5 (PR #17 + PR #31): prompt-pack architecture, `Locale` enum, schema v6 with `learners.locale_id`, `--language` CLI flag, locale-keyed TTS dispatch, locale/learner mismatch detection on `--resume`, voice-pack pre-flight check.
- Knowledge-base `open_for_locale` migration (PR #23 + PR #30): `SqliteKnowledgeBase::open()` deprecated; storage callers migrated.
- Graceful API errors (PR #15 + PR #16): typed `InferenceError`, retry helper, i18n boundary, mid-stream typed-error dispatch.
- LLM-based comprehension classifier (PR #14): per-concept depth assessments persisted to `turn_comprehensions`.
- LLM-based concept extraction (PR #13): two-list output, monotonic upsert into learner.concepts.
- Resume past session + long-term memory (PR #2).
- Engagement classifier (PR #5/#6).
- Voice round-trip POC (PR #7 + smoke fixes).

## The next task: pick one

### Option A — Per-child vocabulary tracking with spaced repetition (Phase 0.3)

The infrastructure is now richer than ever: every child turn has extracted concepts AND per-concept depth assessments, all longitudinally persisted. Vocabulary tracking is the natural next layer — tag concepts with `vocab:` namespace, extend `learner.concepts` with a "next due" date, weave the words back at expanding intervals based on demonstrated depth.

Schema implications: `learner_concepts` already has `last_encountered: TIMESTAMP` and `encounter_count: INTEGER`. Adding `next_due: TIMESTAMP NULL` would cover the spaced-repetition state. New CLI flag `--vocab-window <days>` to bound how aggressively the dialogue manager surfaces overdue words. Algorithmic integration in `prompt_builder` (system-prompt-level injection of overdue vocab); no new LLM calls needed.

Files to start in: `src/crates/primer-core/src/learner.rs::ConceptState` (add `next_due`), `src/crates/primer-storage/src/schema.rs` (v7 migration), `src/crates/primer-pedagogy/src/prompt_builder.rs` (vocab-injection). Consider also extending `dialogue_manager/lifecycle.rs` to surface overdue-vocab state at session start, and `dialogue_manager/learner_update.rs` to track due-date changes.

### Option B — Session-time tracking with break suggestions (Phase 0.3)

`should_suggest_break` lives in `dialogue_manager/lifecycle.rs` now and is a turn-count-cum-elapsed-minutes heuristic against `LearningPreferences::typical_session_minutes`. Refine to surface the suggestion in the CLI loop and hook into the existing engagement-state-driven encouragement intent. Estimated 1–2 hours.

Files to start in: `src/crates/primer-pedagogy/src/dialogue_manager/lifecycle.rs::should_suggest_break` (the existing implementation), `src/crates/primer-pedagogy/src/prompt_builder.rs` (to surface the break hint via a new `PedagogicalIntent::SuggestBreak`?), `src/crates/primer-cli/src/main.rs` (REPL loop to surface the suggestion).

### Option C — Cleanup follow-ups from the dialogue_manager refactor (low priority)

If you want to pick up small things from this session's work:

- **`background_tests.rs` at 761 lines could split** into `classifier_tests.rs` + `post_response_tests.rs` if a future change makes that distinction more meaningful. Today they share too much setup to be worth splitting.
- **`apply.rs` tests live alongside the production code** as `mod tests` rather than under `tests/`. If you ever need to share apply-test mocks with other test files, move them to `tests/apply_tests.rs` for symmetry.
- **`spawn_post_response_task` in `turn.rs` is still ~150 lines.** Splitting its 5 internal labeled steps into private helpers was deferred (lifetime gymnastics for marginal gain). Worth revisiting if a 6th step is ever added.

### Option D — Cleanup follow-ups from the comprehension branch (still relevant)

(Carried forward from the prior NEXT_SESSION.md. None blocking, all known-locations.)

**One-liners / very small:**
- `catalog.rs` doc comment says `pedagogical_intent_name` where actual function is `intent_name`.
- `truncate_to_chars` walks input twice (single-pass form would be O(max_chars) regardless of input length).
- Add UTF-8 safety comment on `extract_first_json_object` in `primer-core::llm_util`.
- More edge-case tests for `llm_util` (zero-cap, empty-input, exact-equality boundary).
- v5 migration rollback test (v4 has one as `apply_v4_migrations_rolls_back_on_failure`).
- v5 idempotency test could be stronger.
- `save_comprehensions` cache test (HashMap reuse within one call).
- `save_comprehensions` cross-classifier parallel-rows test.
- `LlmComprehensionClassifier` per-assessment validation drops are silent; `tracing::debug!` per drop would help field-diagnose.
- "1 of N preserved" test in comprehension parser.
- Evidence-truncate block reallocates `to_string()` even when within cap; trivial early-return saves the allocation.
- "Comprehension classify error → chain still returns Some-with-empty-comprehension" test.
- Two `extract_task_*` test names in `dialogue_manager/tests/background_tests.rs` are now slightly misleading (they exercise the post-response chain as a whole, not just the extractor).

**Prompt-engineering improvement worth considering** (surfaced by gemma4:e4b smoke after the timeout bump):
- **Trim or zero `recent_turns` for comprehension specifically.** Smoke testing showed gemma4:e4b correctly produced empty assessments on bare "ok" exchanges only when `recent_turns` was small or absent; with the default 4-turn context, gemma "bled" prior conversation into the assessment.

## Files most relevant to start in

- **Option A (vocab + spaced repetition)**: `src/crates/primer-core/src/learner.rs::ConceptState` for the schema; `src/crates/primer-storage/src/schema.rs` for v7; `src/crates/primer-pedagogy/src/prompt_builder.rs` for the vocab-injection-into-system-prompt site; consider `src/crates/primer-pedagogy/src/dialogue_manager/lifecycle.rs` for surfacing overdue-vocab state.
- **Option B (break suggestions)**: `src/crates/primer-pedagogy/src/dialogue_manager/lifecycle.rs::should_suggest_break` — extend the heuristic.
- **Option C (refactor cleanup)**: see the bullet list above. Each item names its file path.
- **Option D (comprehension-branch cleanup)**: see the bullet list above. Each item names its file path.

## Patterns to reuse, not reinvent

(All inherited from prior sessions; this session formalised the dialogue-manager directory-module idiom.)

- **Categorical text → integer FK lookup tables.** Closed Rust enum = source of truth, validate-and-seed on `open_for_locale()`.
- **Idempotent schema migrations, transaction-wrapped.** v2/v3/v4/v5/v6 in `primer-storage::schema` are templates. Schema currently at v6; next migration is v7.
- **Append-only persistence.** If a Rust-side type is append-only in memory, persistence should mirror that.
- **System-prompt layering for context that isn't part of the chat timeline.** Long-term memory + extracted concepts + (future) overdue vocabulary all live in the system prompt.
- **Soft-fail policy for asynchronous side-effects.** Engagement classifier, concept extractor, AND comprehension classifier all return empty/unknown rather than `Err` on any failure; errors flow to `tracing::warn!`. Mirror this for any new async background analysis.
- **Spawn-and-await pattern.** Spawn at end of turn N (after save), await with bounded timeout at start of turn N+1, apply to in-memory state on success, detach on timeout. Used by classifier (parallel) and extractor → comprehension (chained, one task). Use it again for any new background analysis.
- **TDD discipline.** Write tests first; watch them fail; implement to green. (For pure refactors like this one, existing tests are the regression net — no new tests added.)
- **Code-review fixes in-place.** When a review surfaces multiple issues, fix them all in one pass on the same branch rather than deferring.
- **Shared LLM helpers in `primer-core::llm_util`.** Any new structured-output crate imports `extract_first_json_object` and `truncate_to_chars` from there.
- **Typed errors + i18n boundary + retry helper in `primer-core`.** Errors are HTTP-/transport-neutral data structures; user-facing rendering happens at a single boundary; transient failures retry via a domain-agnostic helper.
- **Directory modules for complex single-concept components.** `dialogue_manager/` is the canonical example. `mod.rs` = struct + glue; per-axis files = impl blocks; `test_support.rs` = shared mocks; `tests/<axis>_tests.rs` = per-axis test files. Use `pub(super)` for cross-module method visibility.
- **`.env` and `~/.primer_env`** are auto-loaded.

## Watch-outs (still relevant)

- **The `.map_err` removal at the inference call site is load-bearing.** Now in `dialogue_manager/turn.rs::stream_inference_response`. If a future change reintroduces `.map_err(|e| PrimerError::Inference(format!(...).into()))`, it will silently re-erase the typed dispatch and friendly messages will revert to "Something unexpected went wrong." Just propagate via `?`.
- **`Other(String)` is dev-facing only — never render to users.** The i18n contract is documented in `primer-core::i18n::render_inference_error`'s module doc.
- **Spawn order in `respond_to_streaming` is load-bearing.** Classifier spawn → post-response spawn. The orchestrator in `dialogue_manager/turn.rs` documents this with a step-6 comment block. Don't re-order without re-reading [docs/TODO/OPTIMIZING_LLM_REQUEST_ORDER.md](docs/TODO/OPTIMIZING_LLM_REQUEST_ORDER.md).
- **`close_session` drain may not capture the very last classifier task on a short conversation.** Carried forward from comprehension branch — see prior NEXT_SESSION.md for details.
- **The retry helper does not surface intermediate state to the streaming consumer.** During retry, the consumer's `Stream` hasn't been created yet.
- **`Retry-After` parsing is integer-seconds only.** HTTP-date form silently dropped.
- **`is_request()` from reqwest is mapped to `NetworkUnavailable`** — conflates "config error" with "no network."
- **`Role::System → "user"`** mapping in `cloud.rs` versus the leading `system` message in `ollama.rs` are different on purpose.
- **The stub backend emits one final chunk by design** — degenerate but valid stream.
- **`top_p` is silently dropped from cloud requests.**
- **Streaming-task spawns are fire-and-forget.** Long-lived deployments will want a cancellation token.
- **Save failures from `SqliteSessionStore` are logged, not propagated.**
- **`save_session` assumes append-only memory.**
- **Synchronous summarization on the hot path.** Each summary refresh is one extra inference call per ~K turns.
- **`LearnerStore::save_learner` is monotonic on concepts.** Forgetting must be explicit.
- **`retrieve_session_turns` index-lag invariant.** Callers MUST use `exclude_indices_at_or_after = total - window`.
- **Concept extraction lags one turn for in-memory state.** Same pattern applies to comprehension.
- **Comprehension is monotonic-max.** Sub-threshold assessments are persisted but don't update in-memory state.
- **`update_turn_concepts` and `save_comprehensions` exist because `save_session` skips already-persisted turns.**
- **`extract_first_json_object` and `truncate_to_chars` live in `primer-core::llm_util`.**
- **`background_tests.rs` is 761 lines** (over the 500 soft cap); see Option C above. Splitting would be artificial today.

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it.

## Exact commands needed to resume

```bash
# Resume on this session's branch (if not yet merged):
cd /Users/hherb/src/primer
git checkout feature/dialogue-manager-refactor
cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 434 passed, 0 failed.
~/.cargo/bin/cargo clippy --workspace --all-targets
~/.cargo/bin/cargo fmt --all -- --check

# Or after merge to main:
cd /Users/hherb/src/primer
git checkout main && git pull
cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace

# Open feature work happens on a fresh branch off main:
git checkout -b feature/<your-task-name>
```

For real-LLM smoke testing the typed-error surface (Anthropic):
```bash
cd /Users/hherb/src/primer/src
ANTHROPIC_API_KEY=sk-bogus ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name SmokeTester --age 9 --no-persist
# Expect: "Authentication failed. Please check your ANTHROPIC_API_KEY..."
```

For the speech build path: `~/.cargo/bin/cargo build --workspace --features primer-cli/speech`.

## Open decisions / risks

1. **`background_tests.rs` over the 500-line soft cap.** Splitting classifier-tests from post-response-tests would be artificial today since both exercise the same spawn-and-await pattern. Worth revisiting only if the file grows past ~1000 lines or the two subsystems diverge structurally.

2. **`apply.rs` could move further out.** The 4 pure helper functions could plausibly live in `primer-pedagogy/src/learner_apply.rs` (crate-wide reuse) or even `primer-core::learner` (cross-crate reuse). Not needed today since only `dialogue_manager` calls them. YAGNI.

3. **`spawn_post_response_task` body could split further.** Its 5 labeled internal steps (extract / persist concepts / candidates / comprehension / persist comprehensions) could become 5 helper closures, but the lifetime story for closures-inside-spawn is ugly. Acceptable as one named method.

4. **Carried-forward open items** (still relevant):
   - Smoke #6 (Ollama daemon down) deferred verification.
   - `is_request()` heuristic conflates config errors with network errors.
   - HTTP-date form `Retry-After` silently dropped.
   - Pre-flight auth check at startup deferred.
   - Voice-mode bridging hook is data-only.
   - `close_session` may drop the final classifier task's `turn_classifications` row on a short conversation.
   - Identifier-format divergence across structured-output crates (`llm:{model}` vs `llm:{backend}:{model}`).
   - `apply_comprehension` insertion policy (only updates concepts already in `learner.concepts`).
   - Concepts are monotonic on `learner_concepts` — saved-once stays saved.
   - No cancellation token on streaming-task spawns.
   - Background-task spawn order is load-bearing on serialized backends — now documented at the orchestrator level in `dialogue_manager/turn.rs` but still without a lock-in test.
