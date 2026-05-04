# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-04 (after the graceful-API-errors branch landed: typed `InferenceError`, retry helper, i18n boundary; **Phase 0.1 closed**; 328 → 364 tests).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root** — every cargo command runs from `src/`.
2. Skim [ROADMAP.md](ROADMAP.md). **Phase 0.1 is now fully closed.** Phase 0.3 has two open bullets: per-child vocabulary tracking with spaced repetition; session-time tracking with break suggestions.
3. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test`. Should be green: **364 tests** across the workspace.
4. From `src/`: `~/.cargo/bin/cargo clippy --workspace --all-targets`. Should be 100 % clean. `~/.cargo/bin/cargo fmt --all -- --check` should also pass.
5. **Don't assume nothing changed since this brief was written.** Read the current state of the files you intend to touch first — Horst may have made interim changes.

## Branch status

The work for this session is on branch **`feature/graceful-api-errors`**, currently 13 commits ahead of `main` (12 implementation + 1 docs), not yet pushed or merged. The branch passes all verification gates and has been smoke-tested end-to-end against the Anthropic API. Remaining choice for Horst:

```bash
# Option A: open a PR
cd /Users/hherb/src/primer
git push -u origin feature/graceful-api-errors
gh pr create --base main --head feature/graceful-api-errors \
  --title "Graceful API errors (Phase 0.1)" \
  --body-file docs/handoffs/<the-handoff-file-you-created>.md  # or compose afresh

# Option B: merge directly
git checkout main && git merge --ff-only feature/graceful-api-errors
git branch -d feature/graceful-api-errors
```

Both work. Horst's existing pattern (PR #13, etc.) is Option A; suggested unless the branch is purely local cleanup.

## What's now on the feature branch (so you don't redo it)

### Graceful API errors — Phase 0.1 closure (this session, branch `feature/graceful-api-errors`)

What landed in 13 commits across 6 implementation bundles plus docs:

- **`primer-core::error::InferenceError` enum** (Bundle 1, commits `4acbdd6` + `761c4c7`) — replaces the catch-all `PrimerError::Inference(String)` with a typed enum: `Auth | RateLimited { retry_after: Option<Duration> } | ServiceUnavailable | NetworkUnavailable | ModelNotFound { model } | Other(String)`. `From<&str>` and `From<String>` impls keep existing producers terse — they fall through to `Other`. `is_retryable()` classifies which variants the retry helper attempts to recover. 23 existing call sites migrated mechanically by adding `.into()`.
- **`primer-core::retry::retry_with_backoff`** (Bundle 2, commit `f276550`) — bounded jittered exponential backoff. Honors `Retry-After` ≤ `retry_after_budget` (default 5 s); longer waits surface immediately. Emits `tracing::info!(target: "primer::retry", ...)` so a future voice-mode change can plug in bridging messages between attempts. HTTP-neutral (no `reqwest` dep). Defaults in `primer_core::consts::retry`.
- **`primer-core::i18n::render_inference_error`** (Bundle 3, commits `c2ad856` + `38ca19c`) — single locale-neutral render boundary. `Locale` enum has one variant today (`English`) marked `#[default]`. The match arms produce English messages keyed off variant + structured fields. `Other`'s inner dev string is structurally inaccessible inside its arm — never reaches users. Future i18n PR adds locales as new variants without touching producers.
- **Cloud backend wired** (Bundle 4, commit `9ab45ff`) — three new `pub(crate)` pure helpers in `cloud.rs`: `classify_anthropic_status`, `parse_retry_after`, `classify_reqwest_error`. `CloudBackend::generate_stream` now wraps the request-send + status-check phase in `retry_with_backoff`; the streaming consumer downstream is byte-identical to before. Closure form `|| async { ... }` borrows `&self.X` and `&request` directly — no clones.
- **Ollama backend wired** (Bundle 5, commit `2875988`) — mirrors the cloud bundle. New `classify_ollama_status` (404 with "model" + "not" body → `ModelNotFound`); imports `classify_reqwest_error` from `crate::cloud` (not duplicated).
- **CLI render integration** (Bundle 6, commits `eeeb3a8` + `a11f364`) — single `Locale::default()` plumbing site at startup. The REPL error branch splits into `Err(PrimerError::Inference(inf)) => render_inference_error(...)` and `Err(other) => "Error generating response: {other}"` (catch-all preserves the original wording). Inference errors also emit a `tracing::warn!` with the dev-facing Display.
- **Critical fix surfaced by smoke testing** (commit `c157825`) — the Bundle 1 mechanical `.into()` migration left a `.map_err(|e| PrimerError::Inference(format!("Generation failed: {e}").into()))` site at `dialogue_manager.rs:534`. That re-wrapped the typed `InferenceError` from the backend back into `Other` via the `From<String>` route — destroying the typed dispatch the i18n render layer relies on. Removed via `?` propagation; the "Generation failed:" prefix was a Phase 0.0 string-error artefact and is redundant now that `PrimerError`'s Display already prefixes "Inference error:". **Without this fix, none of the friendly messages would have surfaced.** Tests at `dialogue_manager.rs:1597, 1691` still pass (they assert `is_err()` only).

Test count delta: **328 → 364 (+36)**. Spread:
- `primer-core` 27 → 48 (+21: 4 InferenceError, 8 retry, 9 i18n)
- `primer-inference` 12 → 27 (+15: 10 cloud classify, 5 ollama classify)
- everything else unchanged.

Verified state at branch tip (with the docs commit landed):
- `~/.cargo/bin/cargo build --workspace` → clean.
- `~/.cargo/bin/cargo test --workspace` → **364 passed, 0 failed.**
- `~/.cargo/bin/cargo clippy --workspace --all-targets` → clean.
- `~/.cargo/bin/cargo fmt --all -- --check` → clean.
- `~/.cargo/bin/cargo build --workspace --features primer-cli/speech` → clean.
- **Manual smoke tested end-to-end with Anthropic:**
  - Bad API key → "Authentication failed. Please check your ANTHROPIC_API_KEY in .env or ~/.primer_env." ✅
  - Working key → normal streaming response ✅
  - Missing Ollama model → "Model 'X' is not available. For Ollama, run `ollama pull X`." ✅
  - Ollama daemon-down → deferred (daemon was in use by a long-running experiment); transport-error path is unit-tested and uses the same `classify_reqwest_error` helper as the cloud-side smoke that did pass.

Plan-of-record committed at `docs/superpowers/plans/2026-05-04-graceful-api-errors.md`. Spec at `docs/superpowers/specs/2026-05-04-graceful-api-errors-design.md`.

### Carried over from prior sessions (still relevant)

- LLM-based comprehension classifier (PR #14): `primer-comprehension` crate, schema v5 → `turn_comprehensions`, per-concept `{depth, confidence, evidence}` against the extractor's candidate set; chained extractor → comprehension spawn; monotonic-max promotion of `LearnerModel.concepts.depth`.
- LLM-based concept extraction (PR #13): `primer-extractor` crate, schema v4 → `turn_concepts`, two-list output ({child_concepts, primer_concepts}), monotonic upsert into learner.concepts.
- Resume past session + long-term memory (PR #2): `--resume <uuid>`, default `--session-db ~/.primer/<slug>.db`, `--no-persist`, schema v2 with rolling LLM summary + FTS5 retrieval over session turns.
- Engagement classifier (Phase 0.3): `primer-classifier` crate, schema v3 with `turn_classifications`, classifier flow in `DialogueManager` (await-at-start-of-turn pattern).
- Learner-model SQLite persistence (PR #5, schema v4): `LearnerStore` trait, monotonic concept upsert, CLI adoption flow on first-run-after-upgrade.
- Voice round-trip POC (PR #7, merged + smoke fixes in #8/#9/#10/#12): `--speech` mode behind the `speech` Cargo feature; LISTEN→LATENT_THINK→SPEAK state machine; vendored `silero-vad-rust` + `piper-rs`. See CLAUDE.md for the full pile of speech gotchas.

## The next task: pick one

Phase 0.1 is **complete**. Phase 0.3 has two open bullets:

### Option A — Per-child vocabulary tracking with spaced repetition (Phase 0.3)

The infrastructure is now richer than ever: every child turn has extracted concepts AND per-concept depth assessments, all longitudinally persisted. Vocabulary tracking is the natural next layer — tag concepts with `vocab:` namespace, extend `learner.concepts` with a "next due" date, weave the words back at expanding intervals based on demonstrated depth.

Schema implications: `learner_concepts` already has `last_encountered: TIMESTAMP` and `encounter_count: INTEGER`. Adding `next_due: TIMESTAMP NULL` would cover the spaced-repetition state. New CLI flag `--vocab-window <days>` to bound how aggressively the dialogue manager surfaces overdue words. Algorithmic integration in `prompt_builder` (system-prompt-level injection of overdue vocab); no new LLM calls needed.

Files to start in: `src/crates/primer-core/src/learner.rs::ConceptState` (add `next_due`), `src/crates/primer-storage/src/schema.rs` (v6 migration), `src/crates/primer-pedagogy/src/prompt_builder.rs` (vocab-injection). The ROADMAP entry has the full schema constraint about future corpus-contribution.

### Option B — Session-time tracking with break suggestions (Phase 0.3)

Easy and self-contained. `should_suggest_break` already exists in `dialogue_manager` (turn-count heuristic). Refine to use elapsed wall-clock time from `session.started_at`, surface in the CLI loop, hook into the existing engagement-state-driven encouragement intent. Estimated 1–2 hours.

Files to start in: `src/crates/primer-pedagogy/src/dialogue_manager.rs::should_suggest_break`. `LearningPreferences::typical_session_minutes` already exists for per-child personalisation.

### Option C — Refactor `dialogue_manager.rs` (now ~3360 lines)

The plan deferred this explicitly; the file got minor edits in this branch (the `.map_err` removal at line 534) but the refactor is still deferred. Natural axes for a split: lifecycle (open / resume / close), per-turn hot path (`respond_to_streaming`), background analysis (the spawn-and-await machinery), helper free functions (`apply_*`, `merge_concepts_into_turn`, etc.), and the test module (mocks alone are ~700 lines). A meaty independent change. CLAUDE.md should learn about it once it's done; leave a note in the file's module-level doc comment in the meantime ("This file is intentionally large during Phase 0.3 — splitting on subsystem boundaries is a Phase 0.3+ task") was a deferred minor flagged by the final review of the comprehension branch and still applies.

### Option D — Cleanup follow-ups from the graceful-API-errors branch

If you'd rather work through small things from this session's reviews (queued during the bundle reviews; none blocking):

**Doc-comment polish:**
- `consts.rs` module-level doc reads aspirationally ("every numeric used by primer-core helpers is defined here") even though only retry uses it today. Acceptable; ages well.
- Consider extracting `1_103_515_245` (the LCG multiplier in `jitter_seed_for`) to a named const — currently inline with a comment. Either form is fine.
- `compute_delay` could mention that `backoff_factor = 0` collapses to no growth. One-liner.
- `classify_reqwest_error` doc could explicitly call out that `is_request()` covers builder errors (config, not network) — the conflation is deliberate.
- `classify_ollama_status` body-match heuristic ("model" + "not") could note the `"model not loaded"` false-positive — benign in practice (the suggested remedy "ollama pull" works for both).
- The `ModelNotFound` message embeds backticks for shell command formatting — won't render well in TTS (voice mode). Worth a future voice-mode parallel render path.

**Test coverage gaps (all non-blocking):**
- 5xx-with-Retry-After Anthropic test: assert that `classify_anthropic_status(503, headers_with_retry_after, ...)` does NOT carry the retry_after value (since only 429 reads it).
- Smoke test #6 (Ollama daemon stopped) is the only acceptance criterion not directly verified end-to-end. The path is symmetric with #4 (cloud bad-API-key path that did pass) and uses the same `classify_reqwest_error` helper that's unit-tested. Worth a one-shot end-to-end verification when the daemon is free.
- An elapsed-time integration test for `retry_with_backoff` (asserting `tokio::time::Instant::now()` advanced by ~base_delay between op invocations) would catch a regression where someone swapped `compute_delay` for a no-op.

**Minor wording follow-ups:**
- README "Still ahead" line — graceful API error handling has been removed (this docs commit). Cross-check on any other doc that may have referenced it.

### Option E — Cleanup follow-ups from the comprehension branch (still relevant)

(Carried forward from the prior NEXT_SESSION.md — none blocking, all known-locations.)

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
- "Comprehension classify error → chain still returns Some-with-empty-comprehension" test (soft-fail path under partial success not directly exercised).
- Two `extract_task_*` test names in `dialogue_manager.rs` are now slightly misleading (they exercise the post-response chain as a whole, not just the extractor).

**Prompt-engineering improvement worth considering** (surfaced by gemma4:e4b smoke after the timeout bump):

- **Trim or zero `recent_turns` for comprehension specifically.** Smoke testing showed gemma4:e4b correctly produced empty assessments on bare "ok" exchanges only when `recent_turns` was small or absent; with the default 4-turn context, gemma "bled" prior conversation into the assessment and emitted depth ratings for concepts not actually demonstrated in the current turn. Two options for fixing model-prompt-adherence on smaller models: (a) re-introduce a separate `ComprehensionSettings::recent_context_turns` (the field that commit `933e871` dropped) and default it to 0 or 1, wiring it through the chained spawn so comprehension gets a smaller / empty `recent_turns` slice while the extractor keeps its 4-turn context for pronoun resolution; or (b) tighten the comprehension prompt itself to emphasize "ignore prior context for the assessment decision; use it only to disambiguate pronouns in the CURRENT exchange". (a) is the more reliable fix; (b) is cheaper but model-dependent.

## Files most relevant to start in

- **Option A (vocab + spaced repetition)**: `src/crates/primer-core/src/learner.rs::ConceptState` for the schema; `src/crates/primer-storage/src/schema.rs` for v6; `src/crates/primer-pedagogy/src/prompt_builder.rs` for the vocab-injection-into-system-prompt site.
- **Option B (break suggestions)**: `src/crates/primer-pedagogy/src/dialogue_manager.rs::should_suggest_break` — refine the heuristic to compare against `session.started_at`.
- **Option C (dialogue_manager refactor)**: the whole file. Plan the split first; one PR per axis.
- **Option D (graceful-api-errors cleanup)**: see the bullet list above. Each item names its file path.
- **Option E (comprehension-branch cleanup)**: see the bullet list above. Each item names its file path.

## Patterns to reuse, not reinvent

(All inherited from prior sessions; this session added the typed-error + retry + i18n trio as new exemplars.)

- **Categorical text → integer FK lookup tables.** Closed Rust enum = source of truth, validate-and-seed on `open()`.
- **Idempotent schema migrations, transaction-wrapped.** v2/v3/v4/v5 in `primer-storage::schema` are templates. Schema currently at v5; next migration is v6.
- **Append-only persistence.** If a Rust-side type is append-only in memory, persistence should mirror that.
- **System-prompt layering for context that isn't part of the chat timeline.** Long-term memory + extracted concepts + (future) overdue vocabulary all live in the system prompt; the `messages` array stays exactly = `recent_turns(window)`.
- **Soft-fail policy for asynchronous side-effects.** Engagement classifier, concept extractor, AND comprehension classifier all return empty/unknown rather than `Err` on any failure; errors flow to `tracing::warn!`. Mirror this for any new async background analysis.
- **Spawn-and-await pattern.** Spawn at end of turn N (after save), await with bounded timeout at start of turn N+1, apply to in-memory state on success, detach on timeout. Used by classifier (parallel) and extractor → comprehension (chained, one task). Use it again for any new background analysis.
- **TDD discipline.** Write tests first; watch them fail; implement to green. Both the comprehension session and this graceful-API-errors session followed this for every commit.
- **Code-review fixes in-place.** When a review surfaces multiple issues, fix them all in one pass on the same branch rather than deferring.
- **Shared LLM helpers in `primer-core::llm_util`.** Any new structured-output crate imports `extract_first_json_object` and `truncate_to_chars` from there. Don't reintroduce per-crate copies.
- **Typed errors + i18n boundary + retry helper in `primer-core`.** Errors are HTTP-/transport-neutral data structures; user-facing rendering happens at a single boundary; transient failures retry via a domain-agnostic helper. Adding a new backend (llama.cpp, QNN) → classify its errors locally into `InferenceError` variants and wrap pre-stream calls with `retry_with_backoff`. Don't add per-backend retry logic or per-backend i18n strings.
- **`.env` and `~/.primer_env`** are auto-loaded.

## Watch-outs (still relevant)

- **The `dialogue_manager.rs:534` `.map_err` removal is load-bearing.** If a future change reintroduces a `.map_err(|e| PrimerError::Inference(format!(...).into()))` wrap on an inference call, it will silently re-erase the typed dispatch and friendly messages will revert to "Something unexpected went wrong." The fix commit `c157825` documents this. Just propagate via `?`.
- **`Other(String)` is dev-facing only — never render to users.** The i18n contract is documented in `primer-core::i18n::render_inference_error`'s module doc; the test `other_does_not_leak_inner_dev_string` is the regression-prevention guard.
- **The retry helper does not surface intermediate state to the streaming consumer.** During the retry phase, the consumer's `Stream` hasn't been created yet. Any future change that overlaps retry with stream consumption needs to re-think this.
- **`Retry-After` parsing is integer-seconds only.** The HTTP-date form is silently dropped (returns `None`). Anthropic uses integer seconds in practice.
- **`is_request()` from reqwest is mapped to `NetworkUnavailable`** — this conflates "config error" (malformed URL, invalid header) with "no network." The user-facing message is generic enough to cover both; the `tracing::warn!` carries the underlying `reqwest::Error` for diagnostics.
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
- **`dialogue_manager.rs` is now ~3360 lines.** Refactor deferred to a dedicated session — see Option C above.

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it. Read files as they actually are.

## Exact commands needed to resume

```bash
# Resume on this session's branch (if not yet merged):
cd /Users/hherb/src/primer
git checkout feature/graceful-api-errors
cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 364 passed, 0 failed.
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

For real-LLM smoke testing the comprehension classifier (Anthropic):
```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --bin primer -- --backend cloud --name SmokeTester --age 9 --verbose \
  --extractor-timeout-ms 8000 --comprehension-timeout-ms 8000
# Type a few exchanges; verify [comprehension] lines populate after exchange 2+.
```

## Open decisions / risks (carried forward + new)

1. **Smoke #6 (Ollama daemon down) deferred verification.** End-to-end smoke testing of the daemon-down path was deferred because the local Ollama instance was running an experiment. The path is symmetric with the cloud bad-API-key path that DID smoke-pass; same `classify_reqwest_error` helper, unit-tested. Risk profile is low; one-shot smoke when the daemon is free closes it.

2. **`is_request()` heuristic conflates config errors with network errors.** Acceptable for Phase 0.1: the user message ("Cannot reach the service. Please check your network connection.") is generic enough; the `tracing::warn!` carries the underlying `reqwest::Error` for diagnostics. Future enhancement: differentiate by host (`localhost` → daemon-down hint).

3. **HTTP-date form `Retry-After` silently dropped.** If a future cloud API ever returns this form in practice (Anthropic doesn't), parsing it is a small follow-up — would cost a `chrono` or `httpdate` dep that wasn't worth Phase 0.1.

4. **Pre-flight auth check at startup deferred.** A first-turn auth failure with the friendly message is acceptable UX; adding a startup ping costs a network call on every launch and would conflict with strict-offline-first when the user is on `--backend stub`.

5. **Voice-mode bridging hook is data-only.** The `tracing::info!(target: "primer::retry", ...)` events carry attempt + kind + delay_ms. A future voice-mode change subscribes via `tracing_subscriber` and plays a "give me a moment" canned phrase. Bundle 7's data is in place; the subscriber + audio plumbing is a separate task.

6. **`close_session` may drop the final classifier task's `turn_classifications` row.** Carried forward from the comprehension branch — smoke testing with gemma4:e4b on a 4-exchange session produced 3 of 4 expected rows. The chained extractor → comprehension path doesn't show this gap because its persistence is inside the awaited future body. Investigation lead: check whether the engagement classifier's spawn captures the `Arc<dyn SessionStore>` differently from the post-response chain.

7. **Identifier-format divergence across structured-output crates.** Engagement classifier uses `llm:{model}`; extractor + comprehension use `llm:{backend}:{model}`. Documented inside each crate's `llm.rs`, but not in CLAUDE.md. Future structured-output crates (e.g. a tone classifier, a question-quality classifier) will reasonably copy whichever crate they look at first; the divergence will permanently grow without a CLAUDE.md note. One-line gotcha bullet would close it.

8. **`apply_comprehension` insertion policy.** Today's design only updates concepts already in `learner.concepts` (extraction owns insertion). The two corner cases (LLM-emitted concept not in candidates → dropped at parser; re-classification of historical turns with drifted learner state → persisted but not applied) are intentionally tolerated, documented in the spec.

9. **Concepts are monotonic on `learner_concepts`** — saved-once stays saved. Forgetting is undefined. If a future feature needs explicit forgetting (Option A's spaced repetition might), it must be a separate operation, not a side effect of `save_learner`.

10. **No cancellation token on streaming-task spawns** — both classifier and the chained extract/comprehend tasks can outlive the dialogue manager if a `JoinHandle::abort` doesn't gracefully cancel underlying HTTP. The `close_session` drain mostly papers over this.

11. **Shared LLM-with-JSON-output utilities** are now consolidated at `primer-core::llm_util`. Any new structured-output crate imports from there.

12. **`dialogue_manager.rs` is ~3360 lines.** Refactor deferred — see Option C.

13. **`recent_context_turns` was dropped from `ComprehensionSettings` (commit `933e871`).** The dialogue manager uses `extractor_settings.recent_context_turns` to size the shared context slice for both subsystems. If a future change wants different context window sizes for extraction vs. comprehension, that field would need to be added back AND wired through.

14. **Background-task spawn order is load-bearing on serialized backends — but only by accident today.** In `respond_to_streaming` the engagement classifier is spawned *before* the post-response chain. On a parallel backend (Anthropic; multi-model Ollama with `OLLAMA_NUM_PARALLEL ≥ 2`), the order is irrelevant. On a serialized backend (single Ollama instance with `OLLAMA_NUM_PARALLEL = 1`, common on resource-constrained devices), the second-spawned task queues server-side while its `blocking_timeout` ticks from spawn time — so the order determines whether the classifier hits its 3 s deadline at all. The current order happens to be right (classifier spawns first → admitted to the model first → completes within budget), but there is no comment locking it in place and no test guarding against accidental reordering. Full formalisation, measurement plan, and Ollama tuning advisory at [docs/TODO/OPTIMIZING_LLM_REQUEST_ORDER.md](docs/TODO/OPTIMIZING_LLM_REQUEST_ORDER.md). **Phase 1 (latency instrumentation) has shipped** — `tracing::info!(target: "primer::latency", ...)` events emit `queued_ms` / `work_ms` / per-call sub-timings at every spawn, visible with `RUST_LOG=primer::latency=info`. Recommended pickup: Phase 2 (calibrate) + Phase 3 (codify with comment + lock-in test) + Phase 4 (Ollama setup advisory + docs) in one session when Anthropic and Ollama are both at hand.
