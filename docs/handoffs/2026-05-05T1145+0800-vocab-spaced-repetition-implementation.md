# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-05 (after a full implementation session for the vocabulary spaced-repetition feature on `feature/vocab-spaced-repetition`).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root** — every cargo command runs from `src/`.
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test`. Should be green: **474 tests** across the workspace (was 434 before this session — +40 new for vocab SR).
3. **Don't assume nothing changed since this brief was written.** Read the current state of the files you intend to touch first — Horst may have made interim changes.

## Branch status

Working branch: **`feature/vocab-spaced-repetition`**, branched from `main` at `9733a16`. **Implementation complete and ready for review / merge.** 15 commits since the branch point (3 planning + 12 implementation):

| Commit | Type | What |
|---|---|---|
| `f657a54` | spec | Design for vocab tracking with Leitner-box spaced repetition (Phase 0.3) |
| `4af1bc4` | plan | 15-task TDD implementation plan |
| `f7008f5` | docs | session handoff: vocab SR spec + plan committed; implementation queued |
| `19b8985` | feat | vocab: add consts::vocab submodule and box_level field on ConceptState |
| `59ef97a` | feat | vocab: add primer-core::vocab module with three pure functions + 20 tests |
| `99f5659` | feat | storage: schema v7 — add learner_concepts.box_level (Phase 0.3 vocab SR) |
| `888fe55` | feat | storage: save_learner / load_learner round-trip box_level (v7 wiring) |
| `f95c087` | feat | vocab: apply_comprehension drives box transitions per assessment |
| `ae35bbd` | feat | vocab: add VocabSettings + thread through DialogueManagerSubsystems |
| `4b0fdcc` | feat | prompt-pack: add vocab_review_intro trait method + en/de strings |
| `8688263` | feat | prompt_builder: add vocab section to system prompt |
| `29e6db3` | feat | vocab: wire due_concepts into build_turn_prompt |
| `fff20f5` | feat | cli: --vocab-max-per-prompt flag (default 4, min 1) |
| `c604da0` | test | vocab: end-to-end integration tests for due-concept injection |
| `434d85c` | docs | docs + cleanup: README + ROADMAP for vocab SR; clippy + fmt fixes |

## What we shipped this session

**Per-child vocabulary tracking with Leitner-box spaced repetition (Phase 0.3).** Spec `f657a54` and plan `4af1bc4` implemented end-to-end across 12 commits. All 15 plan tasks completed.

The shipped feature:

- **Three pure scheduling functions** in `primer-core::vocab` (`apply_box_transition`, `is_due`, `due_concepts`) — all take `now` as a parameter so they're deterministic and pure.
- **Box-level state per concept**: `learner_concepts.box_level INTEGER NOT NULL DEFAULT 0` (schema v7). Boxes [0..4] map to intervals [1d, 3d, 7d, 14d, 30d].
- **Box transitions on every accepted comprehension assessment**, not only on depth strict-increase — that's the SR mechanism for expanding intervals on stable-mastery concepts.
- **System-prompt injection**: `build_turn_prompt` calls `due_concepts` once per turn and injects the top-K most-overdue concepts as a passive hint section between retrieved-older and knowledge. Format: `- {concept_id} (depth: {depth}, last seen N days ago)`.
- **Locale-keyed intro string** in both `en.toml` and `de.toml`: tells the LLM to weave words in only if topically relevant — no drilling, no quizzing.
- **CLI flag `--vocab-max-per-prompt N`** (default 4, must be ≥ 1; clap rejects 0 at parse).
- **40 new tests** across the workspace (20 pure-function in primer-core, 4 schema-migration in primer-storage, 2 round-trip in primer-storage, 4 apply_comprehension box-transition in primer-pedagogy, 2 prompt-pack intro, 3 prompt_builder section ordering, 2 end-to-end via `build_turn_prompt`, 3 CLI parse).

**Architectural decisions honoured:**

- Pure functions in `primer-core` for the algorithmic core (no async, no I/O).
- `#[serde(default)]` on `box_level` for backward-compat deserialisation.
- Schema migration is idempotent + transaction-wrapped, matching the v2/v6 pattern.
- Constants live in `primer-core::consts::vocab` (no magic numbers).
- `VocabSettings` carries the only runtime tunable (max_per_prompt), default in consts.
- No new background task — box updates run synchronously inside `apply_comprehension` in the existing post-response chain.
- `MIN_CONF_FOR_BOX_PROMOTION` (0.6) lives independently from `comprehension_settings.confidence_threshold` (also 0.6) so a future researcher can tune box-advancement strictness without affecting depth promotion. Today the inner reset branch is conditionally dead code in the default config — by design, with a comment in `apply_comprehension` explaining the policy split.

## Final verification gauntlet (all green)

- `~/.cargo/bin/cargo test --workspace` → **474 passed**, 0 failed (was 434; +40 new).
- `~/.cargo/bin/cargo clippy --workspace --all-targets` → clean.
- `~/.cargo/bin/cargo fmt --all -- --check` → clean.
- `~/.cargo/bin/cargo build --workspace --features primer-cli/speech` → clean.

## What's next

Three reasonable directions. **Recommended:** open a PR for the vocab SR work, then pick up break-suggestion work on a fresh branch.

### Option A — open a PR for the vocab SR work, then merge

Concrete acceptance criteria:

1. PR title and body summarising the feature; reference the spec at `docs/superpowers/specs/2026-05-05-vocabulary-spaced-repetition-design.md`.
2. CI green (once CI is set up — currently no CI).
3. Manual smoke test (post-PR or pre-merge) per the command sequence below.
4. Merge to `main`, delete `feature/vocab-spaced-repetition`.

### Option B — session-time-based break suggestions (rest of Phase 0.3)

The remaining Phase 0.3 carry-over from before this session. `should_suggest_break` already lives in `dialogue_manager/lifecycle.rs` but isn't wired into the dialogue flow. Estimated 1-2 hours.

Concrete acceptance criteria:

1. New `PedagogicalIntent::SuggestBreak` (or use the existing `SessionClose` with a bridging mode — design call).
2. After 25-30 minutes of session time (configurable via `--session-break-after-secs N`), the next intent decision routes to `SuggestBreak` instead of the natural choice.
3. Once the child responds, the timer resets so the prompt isn't repeated every turn.
4. Tests: 4-6 covering the timer threshold, response after suggestion, and that pre-threshold sessions are unaffected.

Files: [src/crates/primer-pedagogy/src/dialogue_manager/lifecycle.rs](src/crates/primer-pedagogy/src/dialogue_manager/lifecycle.rs), `prompt_builder.rs` (potential new intent variant), `primer-cli/src/main.rs` (flag).

### Option C — vocab SR follow-ups (deferred from spec)

Smaller cleanup items that the spec explicitly punted:

- **"I just don't want to see this word" escape valve** — a child who never engages with a concept the extractor latched onto could see it surface every 30d forever. Add a CLI subcommand or REPL slash to suppress per-concept.
- **Backfill `box_level` from historical comprehension assessments** — currently pre-v7 rows get `box_level = 0` on upgrade; a smarter migration could derive initial box from existing `learner_concepts.depth + last_encountered`.
- **Locale-aware "N days ago" rendering in the vocab section** — currently English-only inside the prompt. Low priority since the LLM consumes it; the child never sees it.
- **Wallclock-injection for "fast-forward time for testing"** — `build_turn_prompt` reads `chrono::Utc::now()` directly. The pure functions accept `now` as a parameter, so the refactor is mechanical.

## Open decisions / risks

Carried forward from the spec:

1. **Pre-v7 data gets `box_level = 0` on upgrade.** Every previously-encountered concept becomes due 1d after its old `last_encountered`. Acceptable for Phase 0.3 with no field-deployed users (Horst confirmed).
2. **Box state drift across classifier-version upgrades.** The `comprehension_classifiers` lookup table records which classifier emitted each historical assessment, so retro-derivation from `turn_comprehensions` is possible — but not implemented in v1.
3. **Wallclock dependency.** `chrono::Utc::now()` is called from `build_turn_prompt`. The pure functions take `now` as a parameter (testable), but the production call site reads the system clock.
4. **The "stuck at Aware" failure mode.** A concept the child genuinely doesn't grasp will surface every 1d forever. If this is too aggressive in real testing, we can tune box-0's interval up without schema impact.
5. **`MIN_CONF_FOR_BOX_PROMOTION` and `comprehension_settings.confidence_threshold` are numerically equal at 0.6 today.** Intentional — the inner reset branch is conditionally dead code. The constants are namespaced apart so a future researcher can tune them independently.

**Carried-forward open items** (not specific to this feature, still relevant from the prior session):

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

(All inherited from prior sessions and confirmed by this session's work.)

- **Pure functions in `primer-core`** for algorithmic cores — tested by injection of `now`, no async, no I/O.
- **`#[serde(default)]` on new fields** for backward-compat deserialisation.
- **Idempotent, transaction-wrapped schema migrations.** v7 followed the v2/v6 templates exactly.
- **Constants in `consts.rs` submodules.** No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green. (This session combined some red/green steps where the test and implementation lived in the same small file, but the discipline holds.)
- **File-size hygiene.** New files came in well under 500 lines.
- **Locale strings in TOML packs.** Both `en.toml` and `de.toml` got the new `vocab_review_intro`.

## Exact commands needed to resume

```bash
# Resume on this session's branch (implementation complete):
cd /Users/hherb/src/primer
git checkout feature/vocab-spaced-repetition
git log --oneline main..HEAD     # should show 15 commits

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 474 passed, 0 failed.

~/.cargo/bin/cargo clippy --workspace --all-targets
~/.cargo/bin/cargo fmt --all -- --check
```

For real-LLM smoke testing (Anthropic):

```bash
cd /Users/hherb/src/primer/src
RUST_LOG=debug ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name SmokeTester --age 9 --no-persist \
    --vocab-max-per-prompt 4 --verbose 2>&1 | tee /tmp/vocab-smoke.log
```

For manual back-dating to verify a vocab section appears in a real session:

```bash
sqlite3 ~/.primer/<learner-slug>.db \
    "UPDATE learner_concepts SET last_encountered = datetime('now', '-10 days')
     WHERE concept_id IN (SELECT id FROM concepts WHERE name = 'physics:gravity');"
~/.cargo/bin/cargo run --bin primer -- --resume <session-id> --verbose
# Look for the vocab section in the captured system prompt (RUST_LOG=debug).
```

For the speech build path: `~/.cargo/bin/cargo build --workspace --features primer-cli/speech`.

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it.
