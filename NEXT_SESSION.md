# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-03 (after the comprehension-classifier branch landed: new `primer-comprehension` crate, schema v5, dialogue-manager chained spawn, three new CLI flags, verbose extension; 275 → 328 tests).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root** — every cargo command runs from `src/`.
2. Skim [ROADMAP.md](ROADMAP.md). Phase 0.1 streaming + `--model` + conversation persistence + resume + long-term memory + concept extraction + **comprehension classification** are all checked off. Phase 0.3 has two open bullets remaining: per-child vocabulary tracking with spaced repetition; session-time tracking with break suggestions. Phase 0.1 also still has graceful-API-error handling as a leftover.
3. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test`. Should be green: **328 tests** across the workspace (374 with `--features primer-cli/speech`, of which 2 are `#[ignore]`'d hardware tests — count not personally re-verified this session, only the default-feature 328).
4. From `src/`: `~/.cargo/bin/cargo clippy --workspace --all-targets`. Should be 100 % clean. `~/.cargo/bin/cargo fmt --all -- --check` should also pass.
5. **Don't assume nothing changed since this brief was written.** Read the current state of the files you intend to touch first — Horst may have made interim changes.

## Branch status

The work for this session is on branch **`feature/comprehension-classifier`**, currently 14 commits ahead of `main`, not yet pushed or merged. The branch passes all verification gates and a final holistic code review. Remaining choice for Horst:

```bash
# Option A: open a PR
cd /Users/hherb/src/primer
git push -u origin feature/comprehension-classifier
gh pr create --base main --head feature/comprehension-classifier \
  --title "LLM-based comprehension classifier (Phase 0.3)" \
  --body-file docs/handoffs/2026-05-03T1601+0800.md  # or compose afresh

# Option B: merge directly
git checkout main && git merge --ff-only feature/comprehension-classifier
git branch -d feature/comprehension-classifier  # safe — work is on main now
```

Both work. Horst's existing pattern (PR #13, etc.) is Option A; suggested unless the branch is purely local cleanup.

## What's now on the feature branch (so you don't redo it)

### LLM-based comprehension classifier — Phase 0.3 (this session, branch `feature/comprehension-classifier`)

What landed in 14 commits:

- **New `primer-comprehension` crate** (mirrors `primer-classifier` and `primer-extractor` shape exactly). `ComprehensionClassifier` trait with `identifier()` + `classify()` async method. `LlmComprehensionClassifier` wraps `Arc<dyn InferenceBackend>` (no `primer-inference` build dep). `StubComprehensionClassifier` with `with_response`/`with_script` constructors for tests. `ComprehensionSettings` + `consts.rs` with eight tunables (blocking_timeout 1500ms, max_output_chars 4096, max_concepts_per_call 10, confidence_threshold 0.6, generation_max_tokens 512, generation_temperature 0.1, generation_top_p 0.9, evidence_max_chars 240). Soft-fail policy throughout. Identifier format `llm:{backend}:{model}` matches the extractor's (newer) precedent. — commit `9065aef`.
- **Per-concept output.** One LLM call per completed exchange against the deduped union of `child_concepts ∪ primer_concepts` from the extractor (capped at `max_concepts_per_call`). Returns `[{concept, depth, confidence, evidence}]` where `depth` is `Aware|Recall|Comprehension|Application|Analysis` from the existing `UnderstandingDepth` enum. Output parser drops assessments whose concept isn't in the candidate list, whose confidence is out of [0,1], or whose depth name is unknown. Evidence text truncated char-boundary-safe to `evidence_max_chars`.
- **Schema v5.** New `comprehension_classifiers` lookup (lazy) + `turn_comprehensions(session_id, turn_id, concept_id, depth_id, confidence, classifier_id, evidence, created_at)` keyed by `UNIQUE(turn_id, concept_id, classifier_id)`. Idempotent + transaction-wrapped migration; pre-existing v3 tests + v4 user-version assertions updated. — commit `181ef48` (+ post-review polish in `f0b065e`).
- **`SessionStore::save_comprehensions` trait method** + `SqliteSessionStore` impl. Empty-slice no-op; missing-turn `Err`; transaction-wrapped; lazy classifier-id insert; per-call concept-name HashMap cache. Touched `dialogue_manager.rs` test mocks (`CountingStore`, `ConceptCapturingStore`) to provide no-op then capturing stubs for the new trait method. — commit `c271d80`.
- **DialogueManager wiring (the biggest behavioural change).** `DialogueManagerSubsystems` extended with `comprehension: Arc<dyn ComprehensionClassifier>` + `comprehension_settings: ComprehensionSettings`. The existing extractor task is widened into a chained `extractor → comprehension` task; one `JoinHandle` covers both (renamed `extract_task` → `post_response_task`; result type widened to `PostResponseResult { extraction: ExtractionPart, comprehension: ComprehensionResult }`). The chain is: extract → persist via `update_exchange_concepts` → build `candidate_concepts = dedup(child ∪ primer)` capped → classify (or empty if no candidates) → persist via `save_comprehensions`. Combined timeout = `extractor_settings.blocking_timeout + comprehension_settings.blocking_timeout` (default 1500 + 1500 = 3000 ms). Renamed `await_pending_extraction` → `await_pending_post_response`. New apply step `apply_comprehension(learner, &result, &settings) -> bool` — monotonic-max promotion of `concept.depth`, gated on `confidence_threshold`, never inserts concepts (extraction is the only insertion path). New accessors `last_comprehension()` + `comprehension_identifier()` for `--verbose`. `ConceptCapturingStore` mock extended with `comprehensions: Mutex<Vec<...>>` so chain tests can assert call shape. — commit `46c5f2c`.
- **Three new CLI flags** — `--comprehension-backend stub|cloud|ollama` (default: same as `--backend`), `--comprehension-model <name>` (default: same as `--model`), `--comprehension-timeout-ms <ms>` (default: 1500). `build_comprehension` dispatch matrix in `main.rs` is a verbatim parallel of `build_extractor`. Five construction-test cells. — commit `bb083f0`.
- **`--verbose` extension** — now also prints `[comprehension] <concept>=<Depth>(<confidence:.2>) ... (<identifier>)` per turn (or `[comprehension] (none) (<identifier>)` when no assessments), alongside `[intent]`, `[classifier]`, `[extractor]`. Uses the new `UnderstandingDepth::Display` impl.
- **`UnderstandingDepth::name()` + `Display` impl** added to `primer-core::learner` (was previously only in `primer-storage::catalog`; the catalog helper now delegates). Bit-identical lookup-table contents; v4 validate-and-seed pass keeps passing. — commit `a7ae518`.
- **`primer-core::llm_util` shared module** — hosts `extract_first_json_object` and `truncate_to_chars`, which had been duplicated verbatim in `primer-classifier::llm` and `primer-extractor::llm`. Both consumers now import from there; duplicates deleted. The third caller (the new comprehension crate) imports from the same home. — commits `659244a` (add) + `5f8ba01` (drop duplicates).
- **`primer-core::comprehension` types** — `ComprehensionAssessment { concept, depth, confidence, evidence }`, `ComprehensionResult { assessments }` (with `::empty()` factory), `ComprehensionContext<'a> { child_turn, primer_turn, recent_turns, candidate_concepts }`. Newtype-wrapped context for forward compatibility. — commit `3eb0cde`.
- **Docs** — CLAUDE.md dependency diagram + new crate paragraph + dialogue-manager paragraph extension + four new gotcha bullets + corrected `update_learner_model` placeholder bullet + three new flags in the CLI paragraph + `[comprehension]` in `--verbose` description. ROADMAP Phase 0.3 bullet ticked. README crate tree, "What works today" bullet, status header, and "Still ahead" line all reflect the landed state. — commits `6deb1bd` + `9b7dffd`.

Test count delta: **275 → 328 (+53)**. Spread:
- `primer-core` 12 → 27 (+15: 3 comprehension types, 10 llm_util, 2 UnderstandingDepth)
- `primer-classifier` 12 → 12 (no change; helpers moved out, replaced by imports)
- `primer-extractor` 29 → 29 (same)
- `primer-comprehension` 0 → 15 (new crate)
- `primer-storage` 96 → 103 (+7: 2 v5 schema, 5 save_comprehensions)
- `primer-pedagogy` 76 → 82 (+6: 4 apply_comprehension, 2 chain integration)
- `primer-cli` 29 → 34 (+5: comprehension construction)
- everything else unchanged.

Verified state at branch tip:
- `~/.cargo/bin/cargo build --workspace` → clean.
- `~/.cargo/bin/cargo test --workspace` → **328 passed, 0 failed.**
- `~/.cargo/bin/cargo clippy --workspace --all-targets` → clean.
- `~/.cargo/bin/cargo fmt --all -- --check` → clean.
- `~/.cargo/bin/cargo build --workspace --features primer-cli/speech` → clean.
- Manual smoke against Anthropic `claude-sonnet-4-6` with `--verbose --extractor-timeout-ms 8000 --comprehension-timeout-ms 8000` confirmed: `[comprehension]` lines populate plausibly across mixed-depth exchanges (Comprehension(0.90) for an own-words pinball-analogy explanation; Aware(0.85) for "child only asked"; `(none)` for bare "ok" and topical jumps). DB persistence verified — schema v5, 6 `turn_comprehensions` rows, lazy `comprehension_classifiers` populated.

Plan-of-record committed at `docs/superpowers/plans/2026-05-03-comprehension-classifier.md` (19 tasks, all delivered; bundled into 9 commit boundaries in execution).

Spec-of-record committed at `docs/superpowers/specs/2026-05-03-comprehension-classifier-design.md`.

### Carried over from prior sessions (still relevant)

- LLM-based concept extraction (PR #13): `primer-extractor` crate, schema v4 → `turn_concepts`, two-list output ({child_concepts, primer_concepts}), monotonic upsert into learner.concepts. **The new comprehension chain depends on this.**
- Resume past session + long-term memory (PR #2): `--resume <uuid>`, default `--session-db ~/.primer/<slug>.db`, `--no-persist`, schema v2 with rolling LLM summary + FTS5 retrieval over session turns.
- Engagement classifier (Phase 0.3): `primer-classifier` crate, schema v3 with `turn_classifications`, classifier flow in `DialogueManager` (await-at-start-of-turn pattern that the new comprehension flow's `await_pending_post_response` mirrors).
- Learner-model SQLite persistence (PR #5, schema v4): `LearnerStore` trait, monotonic concept upsert, CLI adoption flow on first-run-after-upgrade.
- Voice round-trip POC (PR #7, merged + smoke fixes in #8/#9/#10/#12): `--speech` mode behind the `speech` Cargo feature; LISTEN→LATENT_THINK→SPEAK state machine; vendored `silero-vad-rust` + `piper-rs`. See CLAUDE.md for the full pile of speech gotchas.

## The next task: pick one

Phase 0.1 is complete except for graceful API-error handling. Phase 0.3 still has two open bullets:

### Option A — Per-child vocabulary tracking with spaced repetition (Phase 0.3)

The infrastructure is now richer than ever: every child turn has extracted concepts AND per-concept depth assessments, all longitudinally persisted. Vocabulary tracking is the natural next layer — tag concepts with `vocab:` namespace, extend `learner.concepts` with a "next due" date, weave the words back at expanding intervals based on demonstrated depth.

Schema implications: `learner_concepts` already has `last_encountered: TIMESTAMP` and `encounter_count: INTEGER`. Adding `next_due: TIMESTAMP NULL` would cover the spaced-repetition state. New CLI flag `--vocab-window <days>` to bound how aggressively the dialogue manager surfaces overdue words. Algorithmic integration in `prompt_builder` (system-prompt-level injection of overdue vocab); no new LLM calls needed.

Files to start in: `src/crates/primer-core/src/learner.rs::ConceptState` (add `next_due`), `src/crates/primer-storage/src/schema.rs` (v6 migration), `src/crates/primer-pedagogy/src/prompt_builder.rs` (vocab-injection). The ROADMAP entry has the full schema constraint about future corpus-contribution.

### Option B — Session-time tracking with break suggestions (Phase 0.3)

Easy and self-contained. `should_suggest_break` already exists in `dialogue_manager` (turn-count heuristic). Refine to use elapsed wall-clock time from `session.started_at`, surface in the CLI loop, hook into the existing engagement-state-driven encouragement intent. Estimated 1–2 hours.

Files to start in: `src/crates/primer-pedagogy/src/dialogue_manager.rs::should_suggest_break`. `LearningPreferences::typical_session_minutes` already exists for per-child personalisation.

### Option C — Graceful API errors (Phase 0.1 leftover)

Mid-stream errors propagate cleanly; rate-limit / network-drop / invalid-key still bubble up as raw `PrimerError::Inference("Generation failed: ...")`. Detect 401/403 (bad key), 429 (rate limit), 5xx (server) at the top of `CloudBackend::generate_stream` and surface a user-friendly message. Optional: retry-with-jittered-backoff for 429.

Files to start in: `src/crates/primer-inference/src/cloud.rs` (top of `generate_stream`), `src/crates/primer-core/src/error.rs` (you'll likely add a new `PrimerError` variant or two).

### Option D — Refactor `dialogue_manager.rs` (now 3356 lines)

The plan deferred this explicitly; the file added ~515 lines this session. Natural axes for a split: lifecycle (open / resume / close), per-turn hot path (`respond_to_streaming`), background analysis (the spawn-and-await machinery), helper free functions (`apply_*`, `merge_concepts_into_turn`, etc.), and the test module (mocks alone are ~700 lines). A meaty independent change. CLAUDE.md should learn about it once it's done; leave a note in the file's module-level doc comment in the meantime ("This file is intentionally large during Phase 0.3 — splitting on subsystem boundaries is a Phase 0.3+ task") was a deferred minor flagged by the final review.

### Option E — Cleanup follow-ups from the comprehension branch

If you'd rather work through the small-things list (queued during this session's reviews; none blocking, all known-locations):

**One-liners / very small:**
- `catalog.rs` doc comment says `pedagogical_intent_name` where actual function is `intent_name`.
- `truncate_to_chars` walks input twice (single-pass form would be O(max_chars) regardless of input length).
- Add UTF-8 safety comment on `extract_first_json_object` in `primer-core::llm_util` (continuation bytes 0x80-0xBF can't collide with ASCII `"`, `\\`, `{`, `}` — subtle but correct property worth inoculating future readers about).
- More edge-case tests for `llm_util` (zero-cap, empty-input, exact-equality boundary).
- v5 migration rollback test (v4 has one as `apply_v4_migrations_rolls_back_on_failure`).
- v5 idempotency test could be stronger (assert `COUNT(*) FROM sqlite_master WHERE name='turn_comprehensions'` == 1 on re-run).
- `save_comprehensions` cache test (HashMap reuse within one call: two assessments same concept → 1 concept row).
- `save_comprehensions` cross-classifier parallel-rows test (UNIQUE-on-triple semantic: same `(turn, concept)` with two different classifier identifiers should produce TWO rows).
- `LlmComprehensionClassifier` per-assessment validation drops are silent; `tracing::debug!` per drop would help field-diagnose a misbehaving classifier model.
- "1 of N preserved" test in comprehension parser (current tests assert "all dropped → empty"; partial-keep behavior is correct by inspection but not directly tested).
- Evidence-truncate block in `LlmComprehensionClassifier` reallocates `to_string()` even when already within cap; trivial early-return saves the allocation.
- `subsystems_with_comprehension` test helper is `#[allow(dead_code)]` — either pick it up in Bundle 9-style work or delete it.
- "Comprehension classify error → chain still returns Some-with-empty-comprehension" test (soft-fail path under partial success not directly exercised).
- Mock `save_comprehensions` empty-record nuance — `ConceptCapturingStore` records every `update_exchange_concepts` call (incl. empty) but never empty-assessments `save_comprehensions` because the spawn-side guard prevents the call. Behaviour fine; flag for any future maintainer who refactors that guard.
- Two `extract_task_*` test names in `dialogue_manager.rs` are now slightly misleading (they exercise the post-response chain as a whole, not just the extractor).
- `comprehension_construction_tests` lacks the stub-vs-name explanatory comment that `extractor_construction_tests` has (lines 1381-1386 of `main.rs` — same comment block, lifted to comprehension's `no_flags_reuses_main_model_id` test).
- "Explicit non-stub backend WITH model" branch lacks a dedicated test in classifier/extractor/comprehension construction tests (gap shared across all three modules; one test added per module would close it together).
- A small `recent_context_turns` minor was dropped from `ComprehensionSettings` in commit `933e871` — settings are now leaner.

**Prompt-engineering improvement worth considering** (surfaced by gemma4:e4b smoke after the timeout bump):

- **Trim or zero `recent_turns` for comprehension specifically.** Smoke testing showed gemma4:e4b correctly produced empty assessments on bare "ok" exchanges only when `recent_turns` was small or absent; with the default 4-turn context, gemma "bled" prior conversation into the assessment and emitted depth ratings for concepts not actually demonstrated in the current turn. Sonnet-4-6 follows the prompt's "only assess THIS turn" instruction more strictly so the issue didn't surface in cloud smoke. Two options for fixing model-prompt-adherence on smaller models: (a) add a separate `ComprehensionSettings::recent_context_turns` (deliberately re-introducing the field that commit `933e871` dropped) and default it to 0 or 1, wiring it through the chained spawn so comprehension gets a smaller / empty `recent_turns` slice while the extractor keeps its 4-turn context for pronoun resolution; or (b) tighten the comprehension prompt itself to emphasize "ignore prior context for the assessment decision; use it only to disambiguate pronouns in the CURRENT exchange". (a) is the more reliable fix; (b) is cheaper but model-dependent.

## Files most relevant to start in

- **Option A (vocab + spaced repetition)**: `src/crates/primer-core/src/learner.rs::ConceptState` for the schema; `src/crates/primer-storage/src/schema.rs` for v6; `src/crates/primer-pedagogy/src/prompt_builder.rs` for the vocab-injection-into-system-prompt site.
- **Option B (break suggestions)**: `src/crates/primer-pedagogy/src/dialogue_manager.rs::should_suggest_break` — refine the heuristic to compare against `session.started_at`.
- **Option C (graceful API errors)**: `src/crates/primer-inference/src/cloud.rs` (top of `generate_stream`), `src/crates/primer-core/src/error.rs`.
- **Option D (dialogue_manager refactor)**: the whole file. Plan the split first; one PR per axis.
- **Option E (cleanup)**: see the bullet list above. Each item names its file path.

## Patterns to reuse, not reinvent

(All inherited from prior sessions; the comprehension work didn't change any of these — just added a new exemplar.)

- **Categorical text → integer FK lookup tables.** Closed Rust enum = source of truth, validate-and-seed on `open()`.
- **Idempotent schema migrations, transaction-wrapped.** v2/v3/v4/v5 in `primer-storage::schema` are templates. Schema currently at v5; next migration is v6.
- **Append-only persistence.** If a Rust-side type is append-only in memory, persistence should mirror that.
- **System-prompt layering for context that isn't part of the chat timeline.** Long-term memory + extracted concepts + (future) overdue vocabulary all live in the system prompt; the `messages` array stays exactly = `recent_turns(window)`.
- **Soft-fail policy for asynchronous side-effects.** Engagement classifier, concept extractor, AND comprehension classifier all return empty/unknown rather than `Err` on any failure; errors flow to `tracing::warn!`. Mirror this for any new async background analysis.
- **Spawn-and-await pattern.** Spawn at end of turn N (after save), await with bounded timeout at start of turn N+1, apply to in-memory state on success, detach on timeout. Used by classifier (parallel) and extractor → comprehension (chained, one task). Use it again for any new background analysis.
- **TDD discipline.** Write tests first; watch them fail; implement to green. The 9-bundle execution this session followed this for every commit.
- **Code-review fixes in-place.** When a review surfaces multiple issues, fix them all in one pass on the same branch rather than deferring.
- **Shared LLM helpers in `primer-core::llm_util`.** Any new structured-output crate imports `extract_first_json_object` and `truncate_to_chars` from there. Don't reintroduce per-crate copies.
- **`.env` and `~/.primer_env`** are auto-loaded.

## Watch-outs (still relevant)

- **`close_session` drain may not capture the very last classifier task on a short conversation.** Empirically, smoke testing with gemma4:e4b and 4 exchanges produced 3 `turn_classifications` rows when 4 were expected (`[classifier]` verbose line printed correctly all 4 times in-session, so the task DID complete; the row just didn't land). Likely cause: `close_session` calls `classify_task.take().await` but if the spawn captured an `Arc<dyn SessionStore>` and writes happen via that Arc *after* the task's outer `await` returns (e.g. via internal background work), the binary may exit before the write flushes. Worth investigating; not blocking. The chained extractor → comprehension path doesn't show this gap because its persistence is part of the awaited future body, not a tail-call.
- **`Role::System → "user"`** mapping in `cloud.rs` versus the leading `system` message in `ollama.rs` are different on purpose. Both are correct for their respective APIs.
- **`update_learner_model` in `dialogue_manager.rs` is still a placeholder for engagement state** (word-count heuristic only). Real engagement assessment ships via the classifier crate; the placeholder is purely the in-line word-count code path that's about to become irrelevant once we trust the classifier output more.
- **The stub backend emits one final chunk by design** — degenerate but valid stream. Don't "fix" it.
- **`top_p` is silently dropped from cloud requests.** Watch `api_request_omits_top_p` test.
- **Streaming-task spawns are fire-and-forget.** Long-lived deployments will want a cancellation token.
- **Save failures from `SqliteSessionStore` are logged, not propagated.** Watch out if a future feature needs to *know* a save succeeded.
- **`save_session` assumes append-only memory.** If you ever introduce mutation/deletion of already-recorded turns, the storage layer's `SELECT COUNT(*)`-based skip will silently drop your changes. Document any such mutation up front.
- **Synchronous summarization on the hot path.** Each summary refresh is one extra inference call per ~K turns, blocking the turn it triggers on. Acceptable at default K=20.
- **`LearnerStore::save_learner` is monotonic on concepts.** A concept dropped from `LearnerModel.concepts` survives on disk. Forgetting must be explicit.
- **Persisted name vs `--name` mismatch fires both `tracing::warn!` and `eprintln!`.** Don't quietly drop the eprintln.
- **`retrieve_session_turns` index-lag invariant.** Callers MUST use `exclude_indices_at_or_after = total - window`.
- **Concept extraction lags one turn for in-memory state.** Same pattern now applies to comprehension assessments — both apply at the start of turn N+1, not within turn N.
- **Comprehension is monotonic-max.** A weak exchange never demotes `concept.depth` in memory. Sub-threshold assessments are persisted to `turn_comprehensions` (full longitudinal record) but don't update in-memory state.
- **`update_turn_concepts` and `save_comprehensions` exist because `save_session` skips already-persisted turns.** Don't move concept persistence or comprehension persistence back into `save_session` — that would break the append-only invariant.
- **`extract_first_json_object` and `truncate_to_chars` live in `primer-core::llm_util`.** All three structured-output crates import from there. Don't reintroduce per-crate copies.
- **`dialogue_manager.rs` is now 3356 lines.** Refactor deferred to a dedicated session — see Option D above.

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it. Read files as they actually are.

## Exact commands needed to resume

```bash
# Resume on this session's branch (if not yet merged):
cd /Users/hherb/src/primer
git checkout feature/comprehension-classifier
cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 328 passed, 0 failed.
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

For the speech build path: `~/.cargo/bin/cargo build --workspace --features primer-cli/speech`.

For real-LLM smoke testing the comprehension classifier (Anthropic):
```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --bin primer -- --backend cloud --name SmokeTester --age 9 --verbose \
  --extractor-timeout-ms 8000 --comprehension-timeout-ms 8000
# Type a few exchanges; verify [comprehension] lines populate after exchange 2+.
```

For real-LLM smoke against local ollama (high latency expected on Horst's box right now):
```bash
~/.cargo/bin/cargo run --bin primer -- --backend ollama --model qwen3.6:35b-a3b-q8_0 \
  --name SmokeTester --age 9 --verbose \
  --extractor-timeout-ms 30000 --comprehension-timeout-ms 30000
```

## Open decisions / risks (carried forward + new)

1. **`close_session` may drop the final classifier task's `turn_classifications` row.** Smoke testing with gemma4:e4b on a 4-exchange session produced 3 of 4 expected rows — the `[classifier]` verbose line printed all four times in-session (so the task itself completed) but the final row didn't land on disk. The chained extractor → comprehension path doesn't show this gap because its persistence is inside the awaited future body. Investigation lead: check whether the engagement classifier's spawn captures the `Arc<dyn SessionStore>` differently from the post-response chain.

2. **Identifier-format divergence across structured-output crates.** Engagement classifier uses `llm:{model}`; extractor + comprehension use `llm:{backend}:{model}`. Documented inside each crate's `llm.rs`, but not in CLAUDE.md. Future structured-output crates (e.g. a tone classifier, a question-quality classifier) will reasonably copy whichever crate they look at first; the divergence will permanently grow without a CLAUDE.md note. One-line gotcha bullet would close it.

3. **`apply_comprehension` insertion policy.** Today's design only updates concepts already in `learner.concepts` (extraction owns insertion). The two corner cases (LLM-emitted concept not in candidates → dropped at parser; re-classification of historical turns with drifted learner state → persisted but not applied) are intentionally tolerated, documented in the spec.

4. **Concepts are monotonic on `learner_concepts`** — saved-once stays saved. Forgetting is undefined. If a future feature needs explicit forgetting (Option A's spaced repetition might), it must be a separate operation, not a side effect of `save_learner`.

5. **No cancellation token on streaming-task spawns** — both classifier and the chained extract/comprehend tasks can outlive the dialogue manager if a `JoinHandle::abort` doesn't gracefully cancel underlying HTTP. The `close_session` drain mostly papers over this.

6. **Shared LLM-with-JSON-output utilities** are now consolidated at `primer-core::llm_util`. Any new structured-output crate imports from there.

7. **`dialogue_manager.rs` is 3356 lines.** Refactor deferred — see Option D.

8. **`recent_context_turns` was dropped from `ComprehensionSettings` (commit `933e871`).** The dialogue manager uses `extractor_settings.recent_context_turns` to size the shared context slice for both subsystems. If a future change wants different context window sizes for extraction vs. comprehension, that field would need to be added back AND wired through.

9. **Background-task spawn order is load-bearing on serialized backends — but only by accident today.** In `respond_to_streaming` the engagement classifier is spawned *before* the post-response chain. On a parallel backend (Anthropic; multi-model Ollama with `OLLAMA_NUM_PARALLEL ≥ 2`), the order is irrelevant. On a serialized backend (single Ollama instance with `OLLAMA_NUM_PARALLEL = 1`, common on resource-constrained devices), the second-spawned task queues server-side while its `blocking_timeout` ticks from spawn time — so the order determines whether the classifier hits its 3 s deadline at all. The current order happens to be right (classifier spawns first → admitted to the model first → completes within budget), but there is no comment locking it in place and no test guarding against accidental reordering. Full formalisation, measurement plan, and Ollama tuning advisory at [docs/TODO/OPTIMIZING_LLM_REQUEST_ORDER.md](docs/TODO/OPTIMIZING_LLM_REQUEST_ORDER.md). **Phase 1 (latency instrumentation) has shipped** — `tracing::info!(target: "primer::latency", ...)` events emit `queued_ms` / `work_ms` / per-call sub-timings at every spawn, visible with `RUST_LOG=primer::latency=info`. A 3-turn smoke against Anthropic `claude-sonnet-4-6` produced first observed numbers (preserved in the doc): `queued_ms = 0` everywhere (cloud parallelizes as expected), comprehension ~30-80 % slower than extraction, chain latency climbing turn-over-turn, and classifier headroom on sonnet at only ~14 % of budget. Recommended pickup: Phase 2 (calibrate) + Phase 3 (codify with comment + lock-in test) + Phase 4 (Ollama setup advisory + docs) in one session when Anthropic and Ollama are both at hand.
