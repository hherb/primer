# Engagement Classifier — Next Session Brief

**Branch:** `feature/engagement-classifier`
**Last commit on branch:** `b35c6f4` (Task 26 test-gap fix)
**Last commit on main:** `e583bdf` (this branch's plan-doc commit)
**Purpose of this doc:** Hand off the engagement-classifier branch from the implementation session to a fresh session that will do the final code review and PR creation.

## Status

All 29 tasks in [docs/superpowers/plans/2026-05-01-engagement-classifier-impl.md](2026-05-01-engagement-classifier-impl.md) are complete. The full design is in [docs/superpowers/specs/2026-05-01-engagement-classifier-design.md](../specs/2026-05-01-engagement-classifier-design.md). Branch is ready for the **final whole-branch code review** and PR creation. Do not rerun any of Tasks 1-29.

### Verification at handoff

From `/Users/hherb/src/primer/src/`:

```bash
cargo build --workspace      # clean
cargo test --workspace       # 182 tests passing, 0 failures
cargo clippy --workspace --all-targets   # zero warnings
cargo fmt --check            # clean
```

Test count delta from baseline: **119 → 182** (+63 new tests; spec target was ~170).

### Branch shape

13 commits on `feature/engagement-classifier` since branching from `main` at `e583bdf`:

| SHA | Subject |
|---|---|
| `79419d7` | Task 1 — `feat(core): add PrimerError::Classification variant` |
| `d1d0ad7` | Task 2 — `refactor: split EngagementState::Frustrated into FrustratedStuck/FrustratedTrying` |
| `1dd19c8` | Task 3 — `feat(core): add EngagementAssessment and EngagementContext types` |
| `f989899` | Task 4 — `feat(core): extend LearningPreferences and LearnerModel for classifier` |
| `377c10e` | Task 5 — `feat(storage): add engagement_states catalog with stable ID mapping` |
| `dc99c34` | Task 6 — `feat(storage): schema v3 — engagement_states/classifiers/turn_classifications tables` |
| `2f00e67` | Task 7 — `feat(storage): validate-and-seed engagement_states against EngagementState::ALL` |
| `b2e3637` | Task 8 — `feat(storage): lazy classifier-row creation helper` |
| `a2ab7a8` | Task 9 — `feat(storage): save_classification and load_recent_assessments` |
| `707988f` | Task 10 — `feat(classifier): create primer-classifier crate skeleton with EngagementClassifier trait` |
| `5f3c11e` | Task 11 — `feat(classifier): ClassifierSettings struct with consts-backed defaults` |
| `b655c97` | Task 12 — `feat(classifier): StubEngagementClassifier with default/with_response/with_script` |
| `e2650b1` | Task 13 — `feat(classifier): LlmEngagementClassifier scaffolding (identifier only)` |
| `df6b3fb` | Task 14 — `feat(classifier): LlmEngagementClassifier with prompt building, parsing, and soft-fail` |
| `1c2c3be` | Task 14 fix — `fix(classifier): address review feedback — settings-backed GenerationParams, backend-error test, char-safe truncation` |
| `ad627d9` | Task 15 — `refactor(pedagogy): extract magic numbers into consts module` |
| `cef4af9` | Task 16 — `feat(pedagogy): is_factual_question helper for Gap 2` |
| `3fae787` | Task 17 — `feat(pedagogy): decide_intent routes FrustratedTrying -> Encouragement (Gap 1)` |
| `18cdf43` | Task 18 — `feat(pedagogy): decide_intent_at session-length-aware Disengaging routing (Gap 3)` |
| `fd76b55` | Task 19 — `feat(pedagogy): decide_intent factual-question pattern routing (Gap 2)` |
| `1db84e5` | Task 20 — `feat(pedagogy): engagement note covers FrustratedStuck and FrustratedTrying` |
| `24b7d28` | Task 21 — `refactor(pedagogy): DialogueManager holds Arc classifier + Arc storage` |
| `4e3d84e` | Task 22 — `feat(pedagogy): apply_assessment and await_pending_classification helpers` |
| `b094420` | Task 23 — `feat(pedagogy): wire classifier into respond_to_streaming flow` |
| `eb176ff` | Task 24 — `feat(pedagogy): resume_session rehydrates recent_assessments from store` |
| `0a22f57` | Task 25 — `feat(cli): add --classifier-backend/-model/-timeout-ms and --verbose flags` |
| `69d051e` | Task 26 — `feat(cli): build_classifier matrix — defaults reuse main, overrides per-axis` |
| `b35c6f4` | Task 26 fix — `fix(cli): add test coverage for non-stub model-override + unknown-backend dispatch arms` |
| `8d71c53` | Task 27 — `feat(cli): --verbose prints intent + classifier debug lines to stderr` |
| `83411e1` | Task 28 — `test(pedagogy): end-to-end classifier routing across multi-turn session` |
| `671424e` | Task 29 — `docs: ROADMAP, CLAUDE.md, next-session brief reflect engagement classifier landing` |

(Some Task 14 / 26 had a follow-up fix commit after code review. The implementation session never amended commits — every fix was a fresh commit on the branch.)

## Architecture decisions worth flagging

These came up during implementation and may warrant explicit confirmation in the final review:

1. **`InferenceBackend.generate(prompt, params)` does not take a model parameter.** The model is owned by each backend instance. So `--classifier-model X` (without `--classifier-backend Y`) constructs a *separate* `InferenceBackend` of the same type with model X — it does not reuse the main backend with a per-call model override. Documented in CLAUDE.md gotchas. Spec was wrong on this point; implementation is correct.

2. **`DialogueManager` holds `Arc<dyn EngagementClassifier>` and `Option<Arc<dyn SessionStore>>` instead of `&'a dyn ...`.** Reason: the post-response classifier task spawned via `tokio::spawn(...)` requires `'static` futures. `backend` and `knowledge` stay as `&'a dyn ...` since they're only used synchronously inside method bodies. Documented in CLAUDE.md (and in [the spec](../specs/2026-05-01-engagement-classifier-design.md) as an explicit divergence from the spec's "held by reference" wording).

3. **`apply_assessment` always pushes to `recent_assessments` regardless of confidence**, but only updates `current_engagement` when `confidence >= settings.confidence_threshold`. Low-confidence assessments contribute to trajectory history but don't yank the intent decision. Per the spec's design discussion. See `dialogue_manager.rs::apply_assessment` and the `_keeps_current_engagement_when_low_confidence` test.

4. **`await_pending_classification` aborts the task on timeout** rather than letting it run to completion. Trade: one lost classification per timeout, vs. preventing pile-up of runaway tasks (slow classifier + fast typist would otherwise spawn an extra background task each turn). Per the spec.

5. **Schema v3 migration is idempotent and transaction-wrapped**, mirroring `apply_v2_migrations`. See `primer-storage/src/schema.rs::apply_v3_migrations`.

6. **Pre-production migration note in CLAUDE.md**: existing `~/.primer/*.db` files from earlier testing should be deleted before first run after this lands. The migration *would* work in place — but starting fresh is cleanest.

## What the final code review should focus on

Per Horst's policy ("periodic code reviews are a backup system, not a replacement for per-task reviews"): every task already had spec compliance review, and substantive tasks had code-quality review. The final review's job is **whole-branch perspective** — looking for things that are only visible across the diff, not within a single task.

Suggested foci for the final reviewer:

1. **Cross-task consistency.** Naming, error wrapping, soft-fail policy: all consistent across the new code in `primer-classifier`, `primer-storage` (the new methods), `primer-pedagogy::dialogue_manager`?

2. **The Arc rationale.** Is the partial Arc adoption (classifier+storage as Arc, backend+knowledge as `&dyn`) clearly motivated and consistently applied? Any place where `&dyn` would silently work today but break on a future spawn requirement?

3. **Soft-fail policy uniformity.** Every classifier failure path returns `EngagementAssessment::unknown_low_confidence(...)` and logs `tracing::warn!`. Verify: does this hold across `LlmEngagementClassifier::classify`, the spawned task in `respond_to_streaming`, and resume rehydration? Anywhere a classifier error could currently propagate up?

4. **The `recent_assessments` lifecycle.** Pushed in `apply_assessment` (Task 22), rehydrated in `resume_session` (Task 24), used as `prior_assessments` in the spawned classify task (Task 23). Is there any path where the buffer can grow unbounded or contain inconsistent state across resume?

5. **Schema v3 + validate-and-seed integrity.** `engagement_states` is a closed-enum FK lookup (Task 5/7); `classifiers` is open-vocabulary lazy-create (Task 8). Any inconsistency in how these are used? E.g., does `save_classification` correctly resolve both?

6. **CLI flag handling.** `--classifier-model` without `--classifier-backend` constructs a fresh backend of the same type (case 5 in the matrix). Now tested in commit `b35c6f4`. Any other dispatch case worth verifying?

7. **Test count plausibility.** 119 → 182 (+63). Phase 0.3 was supposed to add ~50 tests. The extra ~13 came from defensive tests during implementation. Acceptable or over-tested?

8. **Documentation accuracy.** [CLAUDE.md](../../../CLAUDE.md) and [primer_next_session.md](../../../primer_next_session.md) updated in commit `671424e`. Verify the new gotchas accurately describe runtime behavior (especially the "engagement classifier is one model behind the chat" note and the per-instance model ownership note).

9. **No accidentally-uncommitted work.** Commit `671424e` (docs) picked up some rustfmt formatting changes in 4 code files that should arguably have been part of earlier task commits. Trace: these were reformatted by `cargo fmt` during Task 28's verification step but not committed until Task 29. Cosmetic only — no functional change. Verify by reading the diff for `671424e`.

## Recommended next steps for the next session

### Step 1 — Run the final whole-branch code review

Spawn a `superpowers:code-reviewer` subagent with:
- BASE_SHA = `e583bdf` (where the branch diverged from main)
- HEAD_SHA = `b35c6f4` (current tip)
- The 9 review foci listed above as guidance

Be prepared to dispatch implementer fixes for any issues the review surfaces, then re-review until clean.

### Step 2 — Use `superpowers:finishing-a-development-branch` skill

This decides PR-vs-merge-vs-cleanup. The user (Horst) will likely want a PR; `gh pr create` with a body that:
- Summarizes the engagement classifier work
- References the spec at `docs/superpowers/specs/2026-05-01-engagement-classifier-design.md`
- Notes the schema migration (existing test DBs should be deleted before first run)
- Lists the 3 ROADMAP 0.3 bullets closed
- Test count delta 119 → 182

### Step 3 — Stop after the PR is created

Do NOT merge. The user reviews the PR and merges manually.

## Things explicitly out of scope for the next session

These are deferred to future sessions per the spec:
- Comprehension classifier (separate Phase 0.3 bullet)
- Concept extraction from child input
- Learner-model SQLite persistence (Option B in the original `primer_next_session.md`)
- Knowledge-passages-retrieved verbose output (different crate, ROADMAP 0.4)
- `--reclassify-on-resume` (resume rehydrates only what's persisted)
- Per-child classifier-model preferences in `LearningPreferences`

## Memory check

If the next session is fresh, it should also:
- Read [CLAUDE.md](../../../CLAUDE.md) for project conventions
- Read [primer_next_session.md](../../../primer_next_session.md) for the project-level brief (it was updated in commit `671424e` to reflect this branch's work)
- Skim the [spec](../specs/2026-05-01-engagement-classifier-design.md) for context on architectural decisions

The auto-memory at `/Users/hherb/.claude/projects/-Users-hherb-src-primer/memory/` has two relevant feedback memories:
- `feedback_no_magic_numbers.md` — invariant numerics → consts; tunable → settings
- `feedback_schema_normalize_categorical.md` — categorical text → lookup-table FK

Both were applied throughout this branch.
