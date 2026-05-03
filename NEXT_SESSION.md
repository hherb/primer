# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-03 (after concept-extraction landing: `primer-extractor` crate + DialogueManager wiring + CLI flags + verbose extension; 275 tests, was 249).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root** — every cargo command runs from `src/`.
2. Skim [ROADMAP.md](ROADMAP.md). Phase 0.1 streaming + `--model` + conversation persistence + resume + long-term memory + concept extraction are all checked off. Phase 0.3 has three open bullets remaining (LLM-based comprehension classifier, per-child vocabulary tracking with spaced repetition, session-time tracking with break suggestions). Phase 0.1 also still has graceful-API-error handling as a leftover.
3. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test`. Should be green: **275 tests** across the workspace (321 with `--features primer-cli/speech`, of which 2 are `#[ignore]`'d hardware tests).
4. From `src/`: `~/.cargo/bin/cargo clippy --workspace --all-targets`. Should be 100 % clean (no warnings). `~/.cargo/bin/cargo fmt --all -- --check` should also pass.
5. **Don't assume nothing changed since this brief was written.** Read the current state of the files you intend to touch first — Horst may have made interim changes.

## What's now on main (so you don't redo it)

### LLM-based concept extraction — Phase 0.3 (PR #13, just shipped)

What landed in PR #13:
- **New `primer-extractor` crate** (mirrors `primer-classifier` shape exactly). `ConceptExtractor` trait with `identifier()` + `extract()` async method; `LlmConceptExtractor` wraps `Arc<dyn InferenceBackend>` (separate dep so the crate stays free of `primer-inference` build dep); `StubConceptExtractor` with `with_response`/`with_script` constructors for tests. `ExtractorSettings` + `consts.rs` with seven tunables (blocking_timeout 1500ms, recent_context_turns 4, max_output_chars 1024, max_concepts_per_speaker 8, generation_max_tokens 384, generation_temperature 0.1, generation_top_p 0.9). Soft-fail policy throughout — extractor errors return `ConceptExtraction::empty()` and `tracing::warn!`, never propagate.
- **One LLM call per completed exchange.** The prompt asks for `{ child_concepts: [...], primer_concepts: [...] }` separately so each turn's `concepts` field is populated correctly. Speaker attribution is explicit in the prompt ("A topic the Primer asks about counts as Primer-introduced even if the child only acknowledged it"). Output is JSON-parsed (with first-balanced-brace extraction tolerating wrapper text), normalised (trim + lowercase + dedupe + 64-char-cap), and per-speaker-truncated to `max_concepts_per_speaker`.
- **`SessionStore::update_turn_concepts(session_id, turn_index, concepts: &[String])`** — new trait method + idempotent `SqliteSessionStore` impl. Resolves `(session_id, turn_index)` → `turn_id`, then `INSERT OR IGNORE INTO concepts` + `INSERT OR IGNORE INTO turn_concepts`. Empty slice is a no-op; missing turn returns `Err`. Wrapped in a single transaction. Exists alongside `save_session` because the latter is append-only and skips already-persisted turns by index — this method is the explicit backfill path.
- **DialogueManager wiring.** Three new fields (`extractor: Arc<dyn ConceptExtractor>`, `extractor_settings: ExtractorSettings`, `extract_task: Option<JoinHandle<Option<ConceptExtraction>>>`). Constructor expanded to 9 args with `#[allow(clippy::too_many_arguments)]`. After each successful response, step 7 of `respond_to_streaming` spawns `tokio::spawn(extractor.extract(...))`; the task self-persists via `update_turn_concepts` for both turns of the just-completed exchange. JoinHandle is consumed at start of next turn by `await_pending_extraction` (bounded 1500ms timeout, **detach** — not abort — on timeout so DB persistence completes anyway). Successful results are applied to `LearnerModel.concepts` via the new `apply_extraction` free function: new concepts get `depth=Aware, confidence=0.5, encounter_count=1`; existing concepts get `encounter_count.saturating_add(1)` and `last_encountered = now()`. `learner_dirty` flag toggles for the next save. `close_session` drains both classifier and extractor tasks before saving final state.
- **CLI flags** — `--extractor-backend stub|cloud|ollama` (default: same as `--backend`), `--extractor-model <name>` (default: same as `--model`), `--extractor-timeout-ms <ms>` (default: 1500). `build_extractor` dispatch matrix in `main.rs` is a verbatim parallel of `build_classifier`. The 5 construction-test cells mirror exactly.
- **`--verbose` extension** — now also prints `[extractor] child=[...] primer=[...] (id)` after each turn, alongside `[intent]` and `[classifier]`.
- **Docs** — CLAUDE.md architecture diagram + new crate paragraph + extractor-flow paragraph + two new gotcha bullets (concept-extraction-lag, `update_turn_concepts`-rationale) + corrected the stale "concepts always empty" placeholder line. ROADMAP Phase 0.3 bullet ticked. CLI-flags paragraph in CLAUDE.md and `--verbose` arg help text both mention the new flags and `[extractor]`.

Test count delta: **249 → 275** (+26). New tests: 4 in `primer-storage` (`update_turn_concepts` round-trip, idempotency, empty-slice no-op, missing-turn error), 12 in `primer-extractor` (1 settings, 4 stub, 7 llm including normalise/dedupe/cap), 5 in `primer-pedagogy` (spawn-persists-both-turns, no-spawn-on-error, next-turn-apply, close-drain, plus a `ConceptCapturingStore` test mock), 5 in `primer-cli` (`extractor_construction_tests`).

Verified state at PR open:
- `~/.cargo/bin/cargo build --workspace` → clean.
- `~/.cargo/bin/cargo test --workspace` → **275 passed, 0 failed.**
- `~/.cargo/bin/cargo clippy --workspace --all-targets` → clean.
- `~/.cargo/bin/cargo fmt --all -- --check` → clean.
- `~/.cargo/bin/cargo build --workspace --features primer-cli/speech` → clean.
- Manual smoke against an Anthropic API key with `--verbose` confirmed by Horst (concept extraction visibly populates the `[extractor]` lines on the second exchange).

Plan-of-record committed at `docs/superpowers/plans/2026-05-03-concept-extraction.md` (14 tasks, all ticked).

### Carried over from prior sessions (still on main, still relevant)

- Resume past session + long-term memory (PR #2): `--resume <uuid>`, default `--session-db ~/.primer/<slug>.db`, `--no-persist`, schema v2 with rolling LLM summary + FTS5 retrieval over session turns.
- Engagement classifier (Phase 0.3): `primer-classifier` crate, schema v3 with `turn_classifications`, classifier flow in `DialogueManager` (await-at-start-of-turn pattern that the new extractor flow mirrors).
- Learner-model SQLite persistence (PR #5, schema v4): `LearnerStore` trait, monotonic concept upsert, CLI adoption flow on first-run-after-upgrade.
- Voice round-trip POC (PR #7, merged + smoke fixes in #8/#9/#10/#12): `--speech` mode behind the `speech` Cargo feature; LISTEN→LATENT_THINK→SPEAK state machine; vendored `silero-vad-rust` + `piper-rs`. See CLAUDE.md for the full pile of speech gotchas.

## The next task: pick one

Phase 0.1 is complete except for graceful API-error handling. Phase 0.3 still has three open bullets:

### Option A — LLM-based comprehension classifier (Phase 0.3, naturally next given the new extractor infrastructure)

This is the natural follow-on to concept extraction. The infrastructure is now there (concepts being extracted per turn, learner.concepts being populated). What's missing is *assessing* the child's grasp — does a response demonstrate genuine understanding, parroting, or confusion?

Architecturally this is another `primer-classifier`-shaped crate (or a sibling within `primer-classifier`): a `ComprehensionClassifier` trait + `LlmComprehensionClassifier` impl + `StubComprehensionClassifier`. Output: a `ComprehensionAssessment { level: Recall|Comprehension|Application|Analysis, confidence: f32, evidence: Option<String> }` — mapping onto the existing `UnderstandingDepth` enum.

Wiring: same async-spawn pattern. After an exchange, classify the depth of understanding the child just demonstrated for the concepts the extractor surfaced. Apply to `learner.concepts[concept].depth` (taking max, since depth is monotonic — never demote, that's an explicit "forgetting" event).

The reviewer of PR #13 noted that `extract_first_json_object` and `truncate_to_chars` are now duplicated verbatim between `primer-classifier/src/llm.rs` and `primer-extractor/src/llm.rs`. If a third LLM-backed-with-JSON-output crate lands (this one would be), now's the time to extract those into a shared `primer-llm-util` crate (or just into `primer-core::llm_util`).

### Option B — Graceful API errors (Phase 0.1 leftover)

Mid-stream errors propagate cleanly; rate-limit / network-drop / invalid-key still bubble up as raw `PrimerError::Inference("Generation failed: ...")`. Detect 401/403 (bad key), 429 (rate limit), 5xx (server) at the top of `CloudBackend::generate_stream` and surface a user-friendly message. Optional: retry-with-jittered-backoff for 429.

### Option C — Per-child vocabulary tracking with spaced repetition (Phase 0.3)

Reuses `learner_concepts` with `concept_id = "vocab:..."`. Plumbing for "remember this word and weave it back at expanding intervals" lives in the dialogue manager. ROADMAP has the full schema constraint about future corpus-contribution.

### Option D — Session-time tracking with break suggestions (Phase 0.3)

Easy. `should_suggest_break` already exists in `dialogue_manager` (turn-count heuristic); refine to use elapsed wall-clock time from `session.started_at`, surface in CLI loop.

### Option E — Cleanup follow-ups from PR #13

If you'd rather just close the small loops:
1. **Per-call concept cache in `update_turn_concepts`** for consistency with `save_session` / `save_learner` (3-line addition; reviewer flagged as Minor).
2. **Bundle `(extractor, extractor_settings)` and `(classifier, classifier_settings)`** into a `DialogueManagerExtras` struct mirroring `DialogueManagerStores` to drop the `#[allow(clippy::too_many_arguments)]`. **NOTE: a scheduled remote agent (`trig_017tBZxEtDPmyPTfbP5KxcP8`) will fire on 2026-05-17 to revisit this exact decision.** If you want to act sooner, do it; if not, the agent will.

## Files most relevant to start in

- **Option A (comprehension classifier)**: clone the shape of `primer-classifier` exactly. The `ConceptExtraction` (in `primer-core::extractor`) and `EngagementAssessment` (in `primer-core::classifier`) types are the architectural precedent. Spawn site: alongside the existing two spawns at end of `respond_to_streaming` step 6 / 7.
- **Option B (graceful API errors)**: `src/crates/primer-inference/src/cloud.rs` (top of `generate_stream`), `src/crates/primer-core/src/error.rs` (you'll likely add a new `PrimerError` variant or two).
- **Option C (vocabulary tracking)**: `src/crates/primer-pedagogy/src/dialogue_manager.rs` for the surface; `src/crates/primer-core/src/learner.rs` for the `ConceptState` shape (already supports the prefix-as-namespace convention via `concept_id: String`).
- **Option D (break suggestions)**: `src/crates/primer-pedagogy/src/dialogue_manager.rs::should_suggest_break`.
- **Option E.1 (concept cache)**: `src/crates/primer-storage/src/lib.rs::update_turn_concepts` — mirror the cache pattern from `save_session`.
- **Option E.2 (bundling)**: `src/crates/primer-pedagogy/src/dialogue_manager.rs` (`DialogueManager::new` and the `DialogueManagerStores` precedent).

## Patterns to reuse, not reinvent

(All inherited from prior sessions; the new extractor work didn't change any of these — just added new exemplars.)

- **Categorical text → integer FK lookup tables.** Closed Rust enum = source of truth, validate-and-seed on `open()`.
- **Idempotent schema migrations, transaction-wrapped.** v2/v3/v4 in `primer-storage::schema` are templates. Schema currently at v4; next migration is v5.
- **Append-only persistence.** If a Rust-side type is append-only in memory, persistence should mirror that. The `update_turn_concepts` method exists *because* `save_session` is append-only — don't merge them.
- **System-prompt layering for context that isn't part of the chat timeline.** Long-term memory + extracted concepts both live in the system prompt; the `messages` array stays exactly = `recent_turns(window)`.
- **Soft-fail policy for asynchronous side-effects.** Both the engagement classifier and the concept extractor return `Ok(empty)` rather than `Err` on any failure; errors flow to `tracing::warn!`. Mirror this for any new async background analysis (comprehension, vocabulary, etc.).
- **Spawn-and-await pattern.** Spawn at end of turn N (after save), await with bounded timeout at start of turn N+1, apply to in-memory state on success, detach on timeout. Used by classifier and extractor — use it again for any new background analysis.
- **TDD discipline.** Write tests first; watch them fail; implement to green. The plan executed for this PR followed this for every task.
- **Code-review fixes in-place.** When a review surfaces multiple issues, fix them all in one pass on the same branch rather than deferring.
- **`.env` and `~/.primer_env`** are auto-loaded.

## Watch-outs (still relevant)

- **`Role::System → "user"`** mapping in `cloud.rs` versus the leading `system` message in `ollama.rs` are different on purpose. Both are correct for their respective APIs.
- **`update_learner_model` in `dialogue_manager.rs` is still a placeholder** for engagement (word-count heuristic only). Comprehension assessment is Option A above.
- **The stub backend emits one final chunk by design** — degenerate but valid stream. Don't "fix" it.
- **`top_p` is silently dropped from cloud requests.** Watch `api_request_omits_top_p` test.
- **The streaming-task spawns are fire-and-forget.** Long-lived deployments will want a cancellation token.
- **Save failures from `SqliteSessionStore` are logged, not propagated.** Watch out if a future feature needs to *know* a save succeeded.
- **`save_session` assumes append-only memory.** If you ever introduce mutation/deletion of already-recorded turns, the storage layer's `SELECT COUNT(*)`-based skip will silently drop your changes. Document any such mutation up front.
- **Synchronous summarization on the hot path.** Each summary refresh is one extra inference call per ~K turns, blocking the turn it triggers on. Acceptable at default K=20.
- **`LearnerStore::save_learner` is monotonic on concepts.** A concept dropped from `LearnerModel.concepts` survives on disk. Forgetting must be explicit.
- **Persisted name vs `--name` mismatch fires both `tracing::warn!` and `eprintln!`.** Don't quietly drop the eprintln.
- **`retrieve_session_turns` index-lag invariant.** Callers MUST use `exclude_indices_at_or_after = total - window`.
- **Concept extraction lags one turn for in-memory state.** Extractor task at end of turn N writes to `turn_concepts` directly; `LearnerModel.concepts` is only updated at the start of turn N+1. The system prompt for turn N does not include concepts surfaced *in* turn N.
- **`update_turn_concepts` exists because `save_session` skips already-persisted turns.** Don't move concept persistence back into `save_session` — that would break the append-only invariant.
- **`extract_first_json_object` and `truncate_to_chars` are duplicated** between `primer-classifier/src/llm.rs` and `primer-extractor/src/llm.rs`. If a third LLM-with-JSON-output crate lands (e.g. comprehension classifier), now's the time to extract them.

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it. Read files as they actually are.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git checkout main && git pull
cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 275 passed, 0 failed.
~/.cargo/bin/cargo clippy --workspace --all-targets
~/.cargo/bin/cargo fmt --all -- --check
```

For the speech build path: `~/.cargo/bin/cargo build --workspace --features primer-cli/speech`.

Open feature work happens on a fresh branch off `main`: `git checkout -b feature/<your-task-name>`.

## Open decisions / risks (carried forward)

1. **Bundling `DialogueManager::new` extras** — `#[allow(clippy::too_many_arguments)]` is on the constructor with a justification comment. Scheduled agent `trig_017tBZxEtDPmyPTfbP5KxcP8` will revisit on 2026-05-17. If a third settings/handle pair lands first (e.g. comprehension classifier), bundle then.

2. **Shared LLM-with-JSON-output utilities** — `extract_first_json_object` and `truncate_to_chars` are duplicated. A `primer-llm-util` crate (or `primer-core::llm_util` module) is the natural home if a third caller lands.

3. **Concepts are monotonic on `learner_concepts`** — saved-once stays saved. Forgetting is undefined. If a future feature needs explicit forgetting, it must be a separate operation, not a side effect of `save_learner`.

4. **No cancellation token on streaming-task spawns** — both classifier and extractor tasks can outlive the dialogue manager if a `JoinHandle::abort` doesn't gracefully cancel underlying HTTP. The `close_session` drain mostly papers over this.
