# Dialogue Manager Refactor — Design Spec

**Date:** 2026-05-04
**Phase:** Phase 0.3 (file-size hygiene; deferred from prior sessions)
**Author:** Claude (with Horst Herb)
**Status:** Approved by user, pending implementation plan

## Goal

Split `crates/primer-pedagogy/src/dialogue_manager.rs` (currently 3572 lines, of which ~700 lines are test mocks and ~2320 lines is the test module) into a `dialogue_manager/` directory module with focused per-axis files, none over ~500 lines (with one acknowledged soft exception for the cohesive test_support module). Decompose the single ~382-line `respond_to_streaming` orchestrator method into a short flow plus seven named private helper methods. Preserve all existing behavior, all existing test coverage, and the entire public API.

The CLAUDE.md gotcha currently flags the file as "intentionally large during Phase 0.3 — splitting on subsystem boundaries is a Phase 0.3+ task." This spec executes that split.

After this PR:

- `find crates/primer-pedagogy/src/dialogue_manager -name '*.rs' | xargs wc -l` shows twelve files (nine production + three test), none over 500 lines except `test_support.rs` (~700, intentional).
- `respond_to_streaming` reads as a 30-line orchestrator with named steps. The 230 lines of inline `tokio::spawn` closure bodies become two named methods: `spawn_classification_task` and `spawn_post_response_task`.
- `primer-cli/src/main.rs` and `primer-cli/src/speech_loop.rs` compile untouched — public API surface is identical.
- Test count is preserved at ≥ 434 across the workspace.

## Non-goals

- **No public API changes.** `DialogueManager`, `DialogueManagerStores`, `DialogueManagerSubsystems` keep their current shape exactly. The free functions `apply_assessment` / `apply_extraction` / `apply_comprehension` / `merge_concepts_into_turn` stay `pub(crate)` and keep their current signatures.
- **No behavioral changes.** Order of operations, error semantics, save semantics, classifier-spawn-before-post-response ordering all preserved. The existing test suite is the regression net.
- **No new tests added.** The decomposition exposes new private methods but does not unit-test them — they're implementation detail. The existing 80+ tests already cover the public surface end-to-end. Adding tests for the new private methods would expand scope and test the same behavior twice.
- **No relocation of `apply_*` free functions out of the module.** They could plausibly live in `primer-pedagogy/src/learner_apply.rs` for crate-wide reuse, but no other module needs them today. YAGNI.
- **No further restructuring inside `spawn_post_response_task`.** Its ~150-line body (extract → persist concepts → build candidates → comprehension → persist comprehensions) keeps its labeled-step structure. Splitting inside the spawn would force lifetime gymnastics for marginal readability gain.
- **No changes to `prompt_builder.rs`, `prompt_pack.rs`, or `consts.rs`.** Out of scope.
- **No changes to consumer crates.** `primer-cli/src/main.rs` and `primer-cli/src/speech_loop.rs` compile untouched. If a consumer-side edit is required, the refactor is wrong.
- **No change to test count.** No tests added; no tests removed; tests relocated only.
- **No update to the `update_learner_model` placeholder heuristic.** A separate concern, tracked in NEXT_SESSION.md watch-outs.
- **No cancellation token on streaming-task spawns.** A separate concern, tracked in NEXT_SESSION.md watch-outs.

## Architecture

### Module map

```
crates/primer-pedagogy/src/
├── lib.rs                           (unchanged: re-exports)
├── consts.rs                        (unchanged)
├── prompt_builder.rs                (unchanged)
├── prompt_pack.rs                   (unchanged)
└── dialogue_manager/                [NEW directory]
    ├── mod.rs                ~250   struct DialogueManager + DialogueManagerStores
    │                                + DialogueManagerSubsystems + PostResponseResult
    │                                + ExtractionPart + type aliases
    │                                + module-organization guide doc comment
    │                                + #[cfg(test)] mod test_support; mod tests { ... }
    ├── lifecycle.rs          ~280   impl block: new, open_session, resume_session,
    │                                close_session, accessors (last_intent, last_*,
    │                                *_identifier), should_suggest_break
    ├── turn.rs               ~430   impl block: respond_to (thin wrapper),
    │                                respond_to_streaming (~30-line orchestrator),
    │                                7 private decomposition methods (Section 2)
    ├── background.rs         ~280   impl block: await_pending_classification,
    │                                await_pending_background, drain_classification,
    │                                drain_post_response, apply_classification_outcome,
    │                                apply_post_response_outcome
    ├── retrieval.rs          ~80    impl block: retrieve_knowledge,
    │                                retrieve_long_term_memory
    ├── summary.rs            ~120   impl block: refresh_summary_if_due,
    │                                refresh_summary_if_stale,
    │                                regenerate_summary_through
    ├── learner_update.rs     ~50    impl block: update_learner_model
    │                                (the placeholder word-count heuristic)
    ├── apply.rs              ~400   pub(super) free fns + their inline unit tests:
    │                                apply_assessment, apply_extraction,
    │                                apply_comprehension, merge_concepts_into_turn
    │                                (~150 lines fns + ~250 lines tests, 8 tests total)
    ├── test_support.rs       ~700   #[cfg(test)] mocks + builders:
    │                                ScriptedBackend, EmptyKnowledge,
    │                                RecordingStorage, RecordingLearnerStore,
    │                                stub_classifier / stub_extractor / stub_comprehension,
    │                                default_subsystems / subsystems_with_*,
    │                                test_learner / chunk / make_test_session_with_turns
    └── tests/
        ├── lifecycle_tests.rs ~500  open_session, close_session, all resume_session_*
        │                            (incl. summary-on-resume tests),
        │                            close_session_drains_extractor_task,
        │                            close_session_always_saves_learner_regardless_of_dirty
        ├── turn_tests.rs      ~480  All respond_to_streaming_* + respond_to_thin_wrapper +
        │                            per-turn save tests (per_turn_save_*, dirty_cleared_*,
        │                            save_learner_failure_does_not_propagate_*) +
        │                            summary_does_not_refresh_when_below_threshold (the lone
        │                            non-resume summary test belongs here)
        └── background_tests.rs ~400 respond_to_streaming_spawns_classify_task_and_persists,
                                     await_pending_classification_aborts_*,
                                     end_to_end_classifier_routing_*,
                                     end_to_end_save_learner_after_open_*,
                                     divergence_bug_closed_*, extract_task_*,
                                     pending_extraction_applied_*, post_response_chain_*
```

The `tests/` subdirectory here is a sub-module of `dialogue_manager/`, not Cargo's integration `tests/` directory. Inside `mod.rs`:

```rust
#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    mod lifecycle_tests;
    mod turn_tests;
    mod background_tests;
}
```

Tests reach mocks via `super::test_support::*` and production items via `super::super::*`.

### Why a directory module (not sibling files)

Rust idiom for a complex single-concept module: see `tokio::sync::mutex` internals. The struct is one logical thing — `DialogueManager` — split across multiple `impl<'a> DialogueManager<'a> { ... }` blocks for readability. The directory contains everything related; sibling files at the crate root would force prefix-naming (`dialogue_apply.rs`, `dialogue_background.rs`, …) for no encapsulation gain.

### Apply tests stay inline with apply.rs

The 8 `apply_*` tests are pure-function unit tests on `pub(super)` items. Moving them to `tests/apply_tests.rs` would force them through `super::super::apply::*` for no readability gain. They stay alongside the functions they test, in the same file.

### test_support visibility

`#[cfg(test)] mod test_support` is a private sibling of the test modules. Items in it are `pub(super)` so all three test files plus the inline `apply.rs` tests can reach them via `super::test_support::*` (or `super::super::test_support::*` from inside `tests/*`).

## Decomposition of `respond_to_streaming`

The current 382-line method has 7 numbered phases (0–6) in its body comments. Those map cleanly to private methods. The two `tokio::spawn` closures (steps 6a and 6b) are the largest gains — they pull ~230 lines of inline closure body into focused methods.

### New orchestrator (~30 lines, pure flow)

```rust
pub async fn respond_to_streaming<F: FnMut(&str)>(
    &mut self,
    child_input: &str,
    on_chunk: F,
) -> Result<String> {
    self.await_pending_background().await;                              // 0
    self.record_child_turn(child_input);                                 // 1

    let intent = prompt_builder::decide_intent_with_pack(
        &*self.prompt_pack, &self.learner, &self.session,
    );
    let prompt = self.build_turn_prompt(child_input, intent).await;      // 2
    let result = self.stream_inference_response(&prompt, on_chunk).await; // 3

    if let Ok(text) = &result {
        self.record_primer_turn(text, intent);                           // 4
        self.update_learner_model(child_input, &intent);
        self.refresh_summary_if_due().await;
    }

    self.persist_turn().await;                                           // 5

    if result.is_ok() {
        self.spawn_classification_task();                                // 6a
        self.spawn_post_response_task();                                 // 6b
    }

    result
}
```

### New private methods on `DialogueManager`

| Method | Lines | Purpose |
|---|---|---|
| `record_child_turn(&str)` | ~10 | Construct + push the Child `Turn` (no side effects beyond `session.add_turn`). |
| `build_turn_prompt(&str, intent) -> String` | ~25 | retrieve_knowledge + retrieve_long_term_memory + build_prompt_with_pack — pure assembly. |
| `stream_inference_response(&str, F) -> Result<String>` | ~25 | The inference call + token loop. The `F: FnMut(&str)` callback flows through. |
| `record_primer_turn(&str, intent)` | ~15 | Compute active_concepts via `prompt_builder::extract_active_concepts`, push the Primer `Turn`. |
| `persist_turn()` | ~25 | Save session + conditionally save learner (the `learner_dirty` gate, lifecycle-event saves are unaffected). |
| `spawn_classification_task()` | ~70 | Owned-context capture + `tokio::spawn` block + latency tracing. Stashes `JoinHandle` in `self.classify_task`. |
| `spawn_post_response_task()` | ~150 | Chained extractor → comprehension spawn block. Stashes `JoinHandle` in `self.post_response_task`. Internal labeled steps (extract / persist concepts / candidates / comprehension / persist comprehensions) preserved as block comments. |

### Behavior preservation contract

The decomposition must preserve, asserted by existing tests:

1. **Order of operations** — `await_pending_background` → record child → decide intent → retrieve → build prompt → stream → record primer → update learner → refresh summary → persist → spawn classification → spawn post-response. Existing ordering tests stay green.
2. **Error semantics** — mid-stream error drops the partial Primer turn but keeps the child turn. Test `respond_to_streaming_does_not_record_primer_turn_on_stream_error` is the guard.
3. **Save semantics** — `persist_turn` runs on both Ok and Err paths; the `learner_dirty` gate stays per-turn (lifecycle events save unconditionally). Tests `respond_to_streaming_fires_engine_save_on_success` / `_on_stream_error` and `per_turn_save_*` are the guards.
4. **Spawn order** — classifier task spawns before post-response task. Load-bearing per gotcha #14 in NEXT_SESSION.md (queueing behavior on serialized backends like single-instance Ollama with `OLLAMA_NUM_PARALLEL=1`). Even though no test asserts this directly, the order is preserved by inspection of the orchestrator.

### Why `spawn_post_response_task` stays at ~150 lines

The body has 5 labeled internal steps. Most of the length is owned-data preparation before the `tokio::spawn` (so the closure satisfies `'static`) plus the spawn body itself. Splitting *inside* the spawn would require either:

- Threading lifetime parameters through helper closures (lifetime gymnastics for marginal gain), or
- Moving helpers off the closure into free fns that take owned data — readable but generates ~5 more named items for ~30 lines each.

Acceptable as a single named method.

## Commit plan

Branch `feature/dialogue-manager-refactor` off main. One PR with 9 commits.

```
1. Create dialogue_manager/ directory; move dialogue_manager.rs to dialogue_manager/mod.rs
   (pure relocation — verifies mod-as-directory shape compiles)

2. Extract apply.rs
   Move the 4 pure free fns + their inline tests.
   ~150 lines code + ~250 lines tests out of mod.rs.

3. Extract test_support.rs
   Move mocks + helper builders. ~700 lines out of mod.rs.

4. Extract small impl modules: retrieval.rs + summary.rs + learner_update.rs
   ~250 lines of impl methods relocated. Batched because each is small
   and they're all "pure mechanical move."

5. Extract background.rs
   await_pending_*, drain_*, apply_*_outcome. ~280 lines. Mechanical move.

6. Extract lifecycle.rs
   new, open_session, resume_session, close_session, accessors,
   should_suggest_break. ~280 lines. Mechanical move.

7. Decompose respond_to_streaming into private methods
   The real refactor commit — body restructuring without relocation.
   Still in mod.rs at end of commit. Existing tests are the regression net.

8. Extract turn.rs
   Move the decomposed orchestrator + 7 private helpers. ~430 lines.
   Mechanical move post-decomposition.

9. Split tests into tests/{lifecycle,turn,background}_tests.rs
   Mechanical relocation; preserves test count.
```

**Commit risk profile:**

- Commits 1, 4, 5, 6, 8, 9: pure relocation. Lowest risk.
- Commits 2, 3: extraction + relocation. Slight risk from import-graph changes.
- Commit 7: the only behavioral-restructuring commit. Highest single-commit risk; isolated for review focus.

## Acceptance gates

Every commit must pass:

1. `cd src && ~/.cargo/bin/cargo build --workspace` — clean
2. `~/.cargo/bin/cargo test --workspace` — ≥ 434 passed, 0 failed (no test count regressions, no new tests)
3. `~/.cargo/bin/cargo clippy --workspace --all-targets` — clean
4. `~/.cargo/bin/cargo fmt --all -- --check` — clean

Branch-tip gates (final commit only):

5. `~/.cargo/bin/cargo build --workspace --features primer-cli/speech` — clean (catches any change that breaks gated speech code)
6. Every `.rs` file in `dialogue_manager/` ≤ 500 lines, **except `test_support.rs`** (documented as ~700, intentional cohesion of mocks).
7. Public API surface unchanged — verified by `primer-cli/src/main.rs` and `primer-cli/src/speech_loop.rs` compiling untouched.
8. **Optional smoke test** post-merge: one real exchange against Anthropic confirms end-to-end behavior. Not required for merge.

## Risk register

| Risk | Mitigation |
|---|---|
| Commit 7 (decomposition) silently changes ordering or error semantics. | The 9 `respond_to_streaming_*` tests cover ordering, error paths, and save semantics; they run on every commit. |
| `tokio::spawn` capture-lifetime quirks when extracting `spawn_classification_task` / `spawn_post_response_task` into methods. | The spawns capture `Arc<…>` clones, not `&self`, so they're already `'static`-clean inside the closure body. Pulling the closure out of an `&mut self` method is mechanical. The `&mut self` borrow is released before `tokio::spawn` is called (no captured `self` references). |
| `test_support` module's `pub(super)` visibility creates surprising re-export issues. | Tests-only — verifiable by `cargo test` at each commit. Visibility errors fail loud at compile time. |
| Splitting a single `impl<'a> DialogueManager<'a> { ... }` block across files breaks rustdoc grouping. | Rust merges all `impl` blocks for a single type in rustdoc output — the docs render identically. Verified by `cargo doc --no-deps -p primer-pedagogy` post-refactor (not a gate; visual check during implementation). |
| `mod.rs` becomes a passive glue file with no actual logic. | Acceptable. `mod.rs` hosts the struct definition + type aliases + the test-module wiring + a module-organization guide doc comment for future readers. ~250 lines is right-sized for that role. |
| Future PRs that touch the dialogue manager have to learn the new layout. | Mitigated by the module-organization doc comment in `mod.rs`. NEXT_SESSION.md reference to "the file" is updated to "the module." CLAUDE.md gotcha about "the 3360-line file" is removed. |

## Documentation updates

The following documentation files need touching as part of this PR:

- **CLAUDE.md** — Remove or update the gotcha "`dialogue_manager.rs` is now ~3360 lines. Refactor deferred to a dedicated session". Add a brief module-organization pointer if useful.
- **NEXT_SESSION.md** — Remove Option C (dialogue_manager refactor) from the open-options list. Add the refactor to the "what we shipped this session" section.
- **README.md** — No changes expected (no public API or feature change).
- **ROADMAP.md** — No changes expected (this is hygiene, not a Phase 0.x feature bullet).

## Out-of-scope follow-ups (noted, not actioned)

These are real follow-ups but explicitly deferred to keep this PR focused:

- The `apply_*` free functions could move to `primer-pedagogy/src/learner_apply.rs` if a future module in this crate needs them. Not needed today.
- `spawn_post_response_task`'s body could split into `extract_step`, `persist_concepts_step`, `build_candidates`, `comprehension_step`, `persist_comprehensions_step` if the lifetime story can be made clean. A separate session.
- Module-level docs in `mod.rs` could be expanded into a richer "guide to this module's organization" if future readers find the per-file doc comments insufficient. Defer until evidence of confusion.
