# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-05 (after a brainstorm-and-plan-only session for the vocabulary spaced-repetition feature on `feature/vocab-spaced-repetition`).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root** — every cargo command runs from `src/`.
2. Read the spec at [docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md](docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md). It is the authoritative design — no ambiguity, no TBDs.
3. Read the implementation plan at [docs/superpowers/plans/2026-05-05-vocabulary-spaced-repetition.md](docs/superpowers/plans/2026-05-05-vocabulary-spaced-repetition.md). 15 bite-sized tasks, every step has actual code, every commit message is pre-written.
4. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test`. Should be green: **434 tests** across the workspace (no code changes shipped this session).
5. **Don't assume nothing changed since this brief was written.** Read the current state of the files you intend to touch first — Horst may have made interim changes.

## Branch status

Working branch: **`feature/vocab-spaced-repetition`**, branched from `main` at `9733a16` (Refactor dialogue_manager into a directory module). Two commits on the branch, both documentation-only:

| Commit | Type | What |
|---|---|---|
| `f657a54` | spec | Design for vocab tracking with Leitner-box spaced repetition (Phase 0.3) |
| `4af1bc4` | plan | 15-task TDD implementation plan, ~35-40 new tests |

No code committed. No production behaviour changed yet.

## What we shipped this session

Brainstorm + design-doc + implementation-plan only. No code.

The feature scope (locked in this session):
- **Per-child vocabulary tracking with spaced repetition (Phase 0.3, Option A from the prior NEXT_SESSION).**
- Algorithm: **Leitner box per concept** — new `learner_concepts.box_level INTEGER` column (schema v7). Boxes [0..4] map to intervals [1d, 3d, 7d, 14d, 30d].
- Box transition rule: `depth ≥ Recall && conf ≥ 0.6` → `box+=1`; `depth=Aware OR conf<0.6` → `box=0`; sub-threshold → no-op (skipped by outer guard, matches existing comprehension semantics).
- Selection: top-K (default 4) by overdue-amount descending, configurable via `--vocab-max-per-prompt N`.
- UX: passive prompt-injection (LLM weaves words in only if topically relevant — no drilling).
- No `PedagogicalIntent::ReviewVocab` (kept passive).
- No new background task (box update runs inside existing `apply_comprehension`).

**Architectural decisions captured in the spec:**
- Three pure functions in a new `primer-core::vocab` module: `apply_box_transition`, `is_due`, `due_concepts`.
- Box transitions run on **every** accepted comprehension assessment (not only when depth strict-increases) — that's the SR mechanism for expanding intervals.
- `MIN_CONF_FOR_BOX_PROMOTION` = 0.6 in `primer-core::consts::vocab` is **conditionally dead code in the default config** (numerically equal to `comprehension_settings.confidence_threshold`); kept as a separate constant so a future researcher can tune box-advancement strictness independently.
- v7 migration is mechanical: one column added with `DEFAULT 0`. Pre-v7 rows upgrade to box 0; no backfill needed (no field-deployed users).

## The next task: implement the plan

The plan in `docs/superpowers/plans/2026-05-05-vocabulary-spaced-repetition.md` is fully self-contained. Each task ships as a single commit with green tests, clippy, and fmt at every step.

**Task ordering (must be sequential — each depends on prior):**

1. Add `consts::vocab` submodule and `box_level` field on `ConceptState` (mechanical; updates 9 struct literals)
2. Pure fn `apply_box_transition` + 8 tests
3. Pure fn `is_due` + 6 tests
4. Pure fn `due_concepts` + 6 tests
5. Schema v7 migration + USER_VERSION bump + 4 tests
6. `save_learner` writes `box_level`
7. `load_learner` reads `box_level` + round-trip test
8. `apply_comprehension` calls `apply_box_transition` per accepted assessment + 4 tests
9. `VocabSettings` + `DialogueManagerSubsystems` plumbing
10. `PromptPack::vocab_review_intro` trait method + en/de TOML + parsing
11. `vocab_section` injection in `build_system_prompt_with_pack_and_vocab` + 3 tests
12. Wire `due_concepts` into `build_turn_prompt`
13. `--vocab-max-per-prompt` CLI flag + 3 parse tests
14. End-to-end integration test (mocked clock via back-dated `last_encountered`)
15. README + ROADMAP updates + final verification gauntlet

**Acceptance criteria for the implementation work** (from the spec, repeated for emphasis):

1. `~/.cargo/bin/cargo test --workspace` passes with new tests + previous 434 (target: ~470-490 passed total).
2. `~/.cargo/bin/cargo clippy --workspace --all-targets` clean.
3. `~/.cargo/bin/cargo fmt --all -- --check` passes.
4. `~/.cargo/bin/cargo build --workspace --features primer-cli/speech` builds clean (silero/whisper/piper feature gating preserved).
5. v7 migration applies in-place to a v6 DB; resume of pre-v7 sessions works.
6. `--vocab-max-per-prompt 8` overrides the default 4 in a runtime check.
7. README + ROADMAP reflect the new feature.

**Recommended execution mode:** subagent-driven (one task per dispatched subagent, two-stage review between tasks). The plan was written for that workflow but works for inline execution too.

## Open decisions / risks

All carried over from the spec — quoted here for visibility:

1. **Pre-v7 data gets `box_level = 0` on upgrade.** Every previously-encountered concept becomes due 1d after its old `last_encountered`. Acceptable for Phase 0.3 with no field-deployed users (Horst confirmed this is a non-issue).
2. **Box state drift across classifier-version upgrades.** New comprehension-classifier versions could change how depth maps to assessments, producing different box transitions. The `comprehension_classifiers` lookup table records which classifier emitted each historical assessment, so retro-derivation from `turn_comprehensions` is possible — but not implemented in v1.
3. **No "I just don't want to see this word" escape valve.** A child genuinely uninterested in a concept the extractor latched onto could see it surface every 30d forever. v2 follow-up.
4. **Wallclock dependency.** `chrono::Utc::now()` is called from `build_turn_prompt`. The pure functions take `now` as a parameter (testable), but the production call site reads the system clock. A future "fast-forward time for testing" mode would override at the call site.
5. **The "stuck at Aware" failure mode.** A concept the child genuinely doesn't grasp will surface every 1d forever. Mitigation in practice = the extractor doesn't usually surface concepts the child doesn't engage with at all; concepts that *do* surface but stay weak are the relevant subset, and 1-day surfacing for them is arguably correct.
6. **`MIN_CONF_FOR_BOX_PROMOTION` and `comprehension_settings.confidence_threshold` are numerically equal at 0.6 today.** Intentional — see the spec section "Subtle but deliberate" for the rationale.

**Carried-forward open items** (not specific to this feature, still relevant):

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
- Background-task spawn order is load-bearing on serialized backends — documented at the orchestrator level in `dialogue_manager/turn.rs` but still without a lock-in test.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and documented in the spec's "Patterns to follow" section.)

- **Pure functions in `primer-core`** for the algorithmic core — tested by injection of `now`, no async, no I/O.
- **`#[serde(default)]` on new fields** for backward-compatible deserialisation.
- **Idempotent, transaction-wrapped schema migrations.** v7 follows the v2/v6 templates exactly.
- **Constants in `consts.rs` submodules.** No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green.
- **File-size hygiene.** Stay under 500 lines per file where reasonable. New `vocab.rs` should land at ~100-150 lines.
- **Locale strings in TOML packs.** Both `en.toml` and `de.toml` get the new `vocab_review_intro`.

## Watch-outs

(All in the spec, but worth flagging at session-resume time.)

- **Box transitions run on EVERY accepted assessment, not only on depth strict-increase.** This is the SR-critical detail — see Task 8 in the plan. A concept already at `Comprehension` re-confirmed at `Comprehension` confidently must still advance the box. Otherwise expanding intervals never expand.
- **The existing `apply_comprehension` only mutates `concept.depth` on `a.depth > c.depth`.** Don't conflate "depth changed" with "box changed" when reading existing code.
- **`apply_extraction` adds a one-line edit** (`box_level: 0,` in the struct literal at apply.rs:83-90). Easy to forget.
- **9 existing struct literals need `box_level: 0,`** added (Task 1 of the plan). Listed by line in the plan.

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it.

## Exact commands needed to resume

```bash
# Resume on this session's branch (the spec + plan are committed but no code yet):
cd /Users/hherb/src/primer
git checkout feature/vocab-spaced-repetition
git log --oneline main..HEAD     # should show f657a54 (spec) + 4af1bc4 (plan)

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 434 passed, 0 failed.

~/.cargo/bin/cargo clippy --workspace --all-targets
~/.cargo/bin/cargo fmt --all -- --check
```

For real-LLM smoke testing once the implementation lands (Anthropic):

```bash
cd /Users/hherb/src/primer/src
RUST_LOG=debug ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name SmokeTester --age 9 --no-persist \
    --vocab-max-per-prompt 4 --verbose 2>&1 | tee /tmp/vocab-smoke.log
```

For manual back-dating to verify a vocab section appears (post-implementation):

```bash
sqlite3 ~/.primer/<learner-slug>.db \
    "UPDATE learner_concepts SET last_encountered = datetime('now', '-10 days')
     WHERE concept_id IN (SELECT id FROM concepts WHERE name = 'physics:gravity');"
~/.cargo/bin/cargo run --bin primer -- --resume <session-id> --verbose
# Look for the vocab section in the captured system prompt.
```

For the speech build path: `~/.cargo/bin/cargo build --workspace --features primer-cli/speech`.

## Alternative paths if you don't want to start with vocab implementation

If something else is more pressing, the spec + plan stay valid indefinitely on this branch and can be picked up later. Other Phase 0.3 work that's still queued:

- **Session-time tracking with break suggestions** — `should_suggest_break` already lives in `dialogue_manager/lifecycle.rs`. Estimated 1-2 hours. Files: `lifecycle.rs::should_suggest_break`, `prompt_builder.rs` (potential new `PedagogicalIntent::SuggestBreak`?), `primer-cli/src/main.rs`.

- **Cleanup follow-ups from comprehension branch** — small one-liners listed in the prior NEXT_SESSION (most still relevant).

But the recommended path is to execute the vocab plan top-to-bottom in a focused fresh session.
