# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-05 (after a full implementation session for the session-time-based break-suggestion feature on `feature/session-break-suggestion`).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. From `src/`: `~/.cargo/bin/cargo build && ~/.cargo/bin/cargo test --workspace`. Should be green: **506 tests** across the workspace (was 474 before this session — +32 new for break-suggestion).
3. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first — Horst may have made interim changes.

## Branch status

Working branch: **`feature/session-break-suggestion`**, branched from `main` at `b13b83f`. **Implementation complete and ready for review / merge.** 14 commits since the branch point (3 planning + 10 implementation + 1 docs):

| Commit | Type | What |
|---|---|---|
| `81dde54` | spec | Design for session-time-based break suggestion (Phase 0.3 close-out) |
| `ce88e30` | spec | Bring locale-aware `{minutes}` interpolation in scope (multilingual is first-class) |
| `f3c99fc` | plan | 12-task TDD implementation plan |
| `edeb564` | feat | core: add `consts::break_suggest::DEFAULT_INTERVAL_MINUTES` |
| `71049ed` | feat | core: add `session_timing` module — `should_suggest_break_now` + `BreakGate` (8 tests) |
| `6ff5c67` | feat | add `PedagogicalIntent::SuggestBreak` variant (id=9) + storage catalog + round-trip test |
| `7085293` | refactor | rename `max_session_minutes` → `break_suggest_after_minutes`; delete dead `should_suggest_break()` accessor + hardcoded `println!` banner |
| `6ff102d` | feat | prompt-pack: `break_suggestion_intro(minutes)` trait method + en/de locale templates (4 tests) |
| `bb47efe` | feat | decide_intent: wire `BreakGate` into post-engagement override (6 tests) |
| `f483c84` | feat | prompt-builder: inject `break_suggestion_intro` section on `SuggestBreak` (2 tests) |
| `30ef76f` | feat | dialogue-manager: in-memory `last_break_suggested_at` + `#[cfg(test)]` clock seam (2 tests) |
| `b6114b4` | feat | dialogue-manager: real `BreakGate` in `respond_to_streaming` + record on fire (2 end-to-end tests) |
| `c005b93` | feat | cli: `--session-break-after-mins N` flag (default 30, min 1) (3 tests) |
| `7f3d174` | docs | README + ROADMAP + CLAUDE.md for break-suggestion (Phase 0.3 close) |

## What we shipped this session

**Session-time-based break suggestions (Phase 0.3 close-out).** Spec `81dde54` and plan `f3c99fc` implemented end-to-end across 11 commits. All 12 plan tasks completed.

The shipped feature:

- **Pure scheduling function** in `primer-core::session_timing`: `should_suggest_break_now(now, started_at, last_suggested_at, interval_minutes) -> bool` — deterministic, takes `now` as a parameter, no async, no I/O.
- **`BreakGate` carrier struct** — `{ interval_minutes: u32, last_suggested_at: Option<DateTime<Utc>> }` with `disabled()` no-op constructor used by the pre-existing characterization tests.
- **New `PedagogicalIntent::SuggestBreak`** (lookup-table id=9) — auto-seeded into existing DBs on next open via `validate_and_seed_lookup`. No schema migration needed.
- **`decide_intent_at_with_pack` leading override**: after engagement-state arms (frustration wins), before turn analysis. A frustrated child past 30 minutes still gets `Scaffolding`, not `SuggestBreak` — fix the frustration first.
- **Locale-aware `{minutes}` template** in `prompt_pack::break_suggestion_intro(minutes) -> String`. Each locale's TOML template owns its own unit word (`minutes` / `Minuten`). Adding a new locale is purely additive; no shared Rust formatter, no `chrono::format` localization dependency.
- **System-prompt section injection** when `intent == SuggestBreak`, placed right after `engagement_note` (groups with the "what should you do" guidance at the top of the prompt body).
- **In-memory `DialogueManager.last_break_suggested_at`** field — reset on `new()` and `resume_session()`. Not persisted across `--resume` (a resumed session might cause one extra suggestion if timing aligns badly; intentional per the spec's non-goals).
- **`#[cfg(test)] clock_override`** test seam with zero production cost. End-to-end tests fast-forward past the 30-min threshold without sleeping via `dm.set_clock_for_test(...)`.
- **CLI flag `--session-break-after-mins N`** (default `consts::break_suggest::DEFAULT_INTERVAL_MINUTES = 30`, must be ≥1; clap's `value_parser!(u32).range(1..)` rejects 0 at parse time).
- **Field rename** `PedagogyConfig.max_session_minutes` → `break_suggest_after_minutes`. The semantic shifted when we adopted intent-based suggestion; the old name implied a hard ceiling. Dead `should_suggest_break()` accessor and the hardcoded `println!` banner that called it were both deleted.
- **32 new tests** across the workspace (8 pure-function in primer-core, 1 variant-count update in primer-core, 4 prompt-pack template, 6 decide-intent break-gate, 2 system-prompt section, 2 dialogue-manager lifecycle, 2 dialogue-manager turn end-to-end, 2 storage round-trip, 3 CLI parse, +2 ripple updates). **Pre-feature baseline 474 → now 506 tests**, all green.

**Architectural decisions honoured:**

- Pure function in `primer-core` for the algorithmic core (no async, no I/O, takes `now` as a parameter).
- No new schema version; lookup-table seeding alone propagates the new variant.
- `#[cfg(test)]` test seam — zero production cost, no runtime config switch needed.
- Engagement-state overrides win over the timer.
- Locale-keyed `{minutes}` substitution per locale; no shared formatter.
- Constants in `consts::break_suggest`; no magic numbers.
- File-size hygiene: `session_timing.rs` is ~120 lines; modifications kept under 500-line ceilings.
- One commit per task with TDD discipline: tests first, watch them fail, implement to green, commit.

## Final verification gauntlet (all green)

- `~/.cargo/bin/cargo test --workspace` → **506 passed**, 0 failed (was 474; +32 new).
- `~/.cargo/bin/cargo clippy --workspace --all-targets` → clean.
- `~/.cargo/bin/cargo fmt --all -- --check` → clean.
- `~/.cargo/bin/cargo build --workspace --features primer-cli/speech` → clean.
- `cargo run --bin primer -- --backend stub --name SmokeTester --age 9 --no-persist --session-break-after-mins 15 --verbose` → REPL starts, flag accepted, no panics.
- Independent end-of-branch reviewer ran final spec-compliance + quality audit: **APPROVE FOR MERGE**, no anti-patterns, no remaining TODOs, all non-goals respected.

## What's next

Two reasonable directions. **Recommended:** open a PR for the break-suggestion work and merge it; that closes Phase 0.3 entirely and the next session can pick up Phase 0.2 (knowledge-base bootstrapping) on a fresh branch.

### Option A — open a PR for the break-suggestion work, then merge

Concrete acceptance criteria:

1. Push the branch: `git push -u origin feature/session-break-suggestion`
2. Open a GitHub PR with title summarising the feature; reference the spec at `docs/superpowers/specs/2026-05-05-session-break-suggestion-design.md`.
3. CI green (once CI is set up — currently no CI).
4. Real-LLM smoke test (post-PR or pre-merge) per the command sequence below.
5. Merge to `main`, delete `feature/session-break-suggestion`. With this, Phase 0.3 is fully complete.

### Option B — Phase 0.2: knowledge-base bootstrapping (fresh feature branch)

The remaining open Phase 0 work. The infrastructure is there (`SqliteKnowledgeBase`, FTS5, locale-keyed tables) but the corpus is empty by default. Phase 0.2 is about:

1. Picking a small starter corpus (children's encyclopedia style, or curated science/curiosity passages).
2. A bootstrap CLI command or script that ingests passages into `~/.primer/knowledge.db` (or a similar default path).
3. Per-locale support (English first, German second).
4. Basic age-gating in the corpus (passages tagged with min/max age band).

Concrete acceptance criteria:

1. New CLI subcommand or standalone binary (`primer-bootstrap` or `primer ingest`) that takes a directory of source files (markdown? JSONL?) and populates `passages_<pack_id>` tables.
2. ~50-100 curated passages for the first locale to validate the retrieval flow.
3. A test that confirms `KnowledgeBase::retrieve` returns sensible passages for a few sample queries.
4. Documentation in CLAUDE.md and ROADMAP.md.

This is a meaty multi-session piece. Estimate 3-5 sessions depending on corpus curation effort.

## Open decisions / risks

Carried forward from the spec and not blocking merge:

1. **`last_break_suggested_at` is in-memory.** A resumed session might cause one extra suggestion if timing aligns badly. Intentional; if real users complain, we can add a persisted `sessions.last_break_suggested_at` column behind a schema v8 migration.
2. **The cadence is purely wallclock.** A child who says "no thanks" to a break still gets nudged again `interval_minutes` later. If field testing shows this is too pushy, a future change can layer "back off N turns on decline" using the comprehension classifier's read of the child's response.
3. **The break-suggestion may interrupt a coherent topic.** The prompt-pack guidance explicitly tells the LLM "you can finish a thought naturally first if you're mid-explanation" — phrasing is the LLM's call.
4. **No anti-spam if the child asks "what?".** A `ComprehensionCheck` follow-up after `SuggestBreak` is fine; cadence is cooldown-only on `SuggestBreak` itself.
5. **CLI flag rejects 0 but the `BreakGate` itself supports a disabled state** (`interval_minutes == 0`). The disabled path is reachable only via direct `PedagogyConfig` construction (e.g. tests). This is by design; the CLI rejects 0 so a typo doesn't accidentally disable break suggestions.
6. **English template uses "~{minutes} minutes"; switching to "hours" past 60 min** is deferred — if intervals routinely exceed 60 minutes, revisit per-locale.

**Carried-forward open items** (not specific to this feature, still relevant from prior sessions):

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
- Background-task spawn order is load-bearing on serialized backends.
- "I just don't want to see this word" escape valve for vocab not yet implemented.

## Patterns to reuse, not reinvent

(All inherited from prior sessions and confirmed by this session's work.)

- **Pure functions in `primer-core`** for algorithmic cores — tested by injection of `now`, no async, no I/O. Same shape as `vocab::apply_box_transition` from the prior session.
- **`#[cfg(test)]` test seams with `pub` accessor methods** — zero production cost, no runtime branches. Pattern used here for `clock_override` and the `last_break_suggested_at_for_test` accessors.
- **Locale-keyed templates in TOML packs** with `{placeholder}` substitution at trait method level. Pattern set by `vocab_review_intro` (no placeholder) and now extended by `break_suggestion_intro(minutes)` (with placeholder). Each locale's template owns its own unit word.
- **Carrier structs (`BreakGate`) for parameter bundling** with a `disabled()` no-op constructor for the wrappers that don't need it. Lets new parameters land without disrupting pre-existing characterization tests.
- **Lookup-table seeding for new closed-enum variants** — no schema migration needed; `validate_and_seed_lookup` auto-INSERTs new rows on next open.
- **Constants in `consts.rs` submodules.** No magic numbers anywhere.
- **TDD discipline.** Tests first; watch them fail; implement to green. (This session's dialogue-manager end-to-end tests benefited especially from this — without them, the time-machine test seam might have been missed.)
- **File-size hygiene.** New files came in well under 500 lines.

## Exact commands needed to resume

```bash
# Resume on this session's branch (implementation complete):
cd /Users/hherb/src/primer
git checkout feature/session-break-suggestion
git log --oneline main..HEAD     # should show 14 commits

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 506 passed, 0 failed.

~/.cargo/bin/cargo clippy --workspace --all-targets
~/.cargo/bin/cargo fmt --all -- --check
```

For real-LLM smoke testing (Anthropic):

```bash
cd /Users/hherb/src/primer/src
RUST_LOG=debug ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name SmokeTester --age 9 --no-persist \
    --session-break-after-mins 5 --verbose 2>&1 | tee /tmp/break-smoke.log
# Then converse for ~5 minutes; the next turn should fire SuggestBreak.
# (Setting --session-break-after-mins 5 makes the smoke testable in a real session.)
```

For the speech build path: `~/.cargo/bin/cargo build --workspace --features primer-cli/speech`.

To open a PR (Option A above):

```bash
git push -u origin feature/session-break-suggestion
gh pr create --title "Phase 0.3 close: session-time-based break suggestion" --body "$(cat <<'EOF'
## Summary

- New `PedagogicalIntent::SuggestBreak` (id=9) driven by a wallclock gate; cadence resets each fire.
- Locale-aware `{minutes}` interpolation in en.toml + de.toml; pattern reusable for future i18n durations.
- `--session-break-after-mins N` parental tunable (default 30, ≥1).
- Field rename `max_session_minutes` → `break_suggest_after_minutes` to reflect actual semantics.
- Spec at `docs/superpowers/specs/2026-05-05-session-break-suggestion-design.md`.

## Test plan

- [x] Workspace tests green: 506 passed (was 474; +32 new).
- [x] Clippy clean.
- [x] Fmt clean.
- [x] Speech feature build clean.
- [ ] Manual smoke test with `--session-break-after-mins 5` against the cloud backend.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it.
