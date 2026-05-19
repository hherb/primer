# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-19T0640+0800 (mid-flight on branch `speech/macos-native-pcm-streaming-issue-114` for #114 — macOS-native TTS PCM streaming. **Stage A complete and green**: trait reshape from `Vec<AudioChunk>`-returning `push_text`/`finalize` to a callback-driven `&mut dyn FnMut(SynthesisEvent)` API, with all five in-tree `SynthesisSession` impls and the state-machine SPEAK consumer adapted. **Stage B + Stage C still remaining**: the actual macOS streaming implementation (Tasks 7-9), dead-code cleanup (Task 10), state-machine TTFA test (Task 11), `--measure-ttfa` smoke flag (Task 12), and final verification + PR (Task 13). Branch pushed to origin as a backup; no PR opened. PR #121 + earlier PRs from the 2026-05-18 session all merged before this branch began.)

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo` (Homebrew rust shadows PATH and silently downgrades to 1.86, breaking silero).
2. Check git state:
   ```bash
   cd /Users/hherb/src/primer
   git status                       # confirm clean state
   git fetch --all --prune
   gh pr list --state open          # no PRs from this branch yet
   git log --oneline main..origin/speech/macos-native-pcm-streaming-issue-114
   ```
   You should see seven commits ahead of main: `f58bbeb` (spec), `38cf197` (plan), `65d54d5` (trait reshape), `8dd8d9e` (Named canary + object-safety pin), `98b5828` (stub adapt), `ed0fcba` (Stage A integration), `e53260c` (consts lift).
3. Check out the branch:
   ```bash
   git checkout speech/macos-native-pcm-streaming-issue-114
   ```
4. Read **the plan** at [docs/superpowers/plans/2026-05-19-macos-tts-pcm-streaming.md](docs/superpowers/plans/2026-05-19-macos-tts-pcm-streaming.md) — Tasks 1-6 are done; resume at **Task 7**. Read the **spec** at [docs/superpowers/specs/2026-05-19-macos-tts-pcm-streaming-design.md](docs/superpowers/specs/2026-05-19-macos-tts-pcm-streaming-design.md) for the architectural context.
5. Verify the branch is green:
   ```bash
   cd src
   ~/.cargo/bin/cargo test --workspace
   # Expected: 858 passed, 0 failed, 3 ignored.
   ~/.cargo/bin/cargo test -p primer-speech --features voice-loop
   # Expected: 85 passed, 0 failed, 2 ignored.
   ~/.cargo/bin/cargo test -p primer-speech --features macos-native
   # Expected: 42 passed, 0 failed, 3 ignored.
   ~/.cargo/bin/cargo fmt --all -- --check
   ~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
   ~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech -- -D warnings
   # All expected clean.
   ```
6. **Don't assume nothing changed since this brief was written.** Read the current state of files you intend to touch first. Always verify open-issue claims and `git log` since this brief's "last updated" timestamp.

## What we shipped this session

### Design + plan (commits `f58bbeb`, `38cf197`)

- **Spec** (`f58bbeb`): [docs/superpowers/specs/2026-05-19-macos-tts-pcm-streaming-design.md](docs/superpowers/specs/2026-05-19-macos-tts-pcm-streaming-design.md). Captures the two-stage TDD approach: Stage A reshapes the trait to a callback API with no observable behaviour change; Stage B replaces the macOS impl's accumulator with a true `mpsc::sync_channel` streaming path so PCM callbacks reach the speaker as `AVSpeechSynthesizer` emits them (cuts per-phrase TTFA from ~hundreds of ms to ~50 ms). Drops the `DispatchSemaphore` machinery from the background path — `SynthesisEvent::PhraseEnd` is the synchronisation primitive now.
- **Plan** (`38cf197`): [docs/superpowers/plans/2026-05-19-macos-tts-pcm-streaming.md](docs/superpowers/plans/2026-05-19-macos-tts-pcm-streaming.md). 13 bite-sized TDD tasks: Tasks 1-6 are Stage A (trait + adapts + atomic Stage-A wrapper commit); Tasks 7-9 are Stage B (consts + failing test + streaming impl, bundled into one commit); Task 10 is the dead-code cleanup sweep; Task 11 is the state-machine TTFA test with `TimedMockTts`; Task 12 extends the `tts_macos_pcm_smoke` example with a `--measure-ttfa` flag; Task 13 is final verification + PR.

### Stage A: trait reshape + all in-tree impls + consumer (commits `65d54d5`, `8dd8d9e`, `98b5828`, `ed0fcba`, `e53260c`)

- **`65d54d5`** core(speech): reshape `SynthesisSession` trait to callback-driven events. New `SynthesisEvent` enum with `Audio(AudioChunk)` and `PhraseEnd` variants. `push_text` and `finalize` now take `&mut dyn FnMut(SynthesisEvent)` and return `Result<()>` — no more `Vec<AudioChunk>` allocations in the hot path. Trait remains object-safe (FnMut is sized). `CannedSynthesisSession` test mock adapted; existing canary test rewritten to drive the callback API; new sibling `synthesis_session_fires_audio_before_phrase_end` pins the explicit `[Audio, PhraseEnd]` order.
- **`8dd8d9e`** test(speech): expand Named canary + add SynthesisSession object-safety pin. Code-review follow-ups on Task 1. Adds `StreamingTextToSpeech` to the `named_super_trait_resolves_via_each_speech_trait` test (pre-existing omission), and `synthesis_session_is_object_safe` — a zero-runtime-cost compile-time canary pinning `Box<dyn SynthesisSession>` as a valid type. Pattern mirrors `loop_observer_is_object_safe` in `primer-speech`.
- **`98b5828`** speech(stub): adapt StubSynthesisSession to callback-driven trait. Two stub tests rewritten with per-event-kind counting (stronger than "len == 2").
- **`ed0fcba`** speech: adapt remaining SynthesisSession impls + consumer to new trait. The big Stage A integration commit: `PiperSession`, `MacosTtsSession` (Stage-A wrapper that still synthesises full-phrase then fires one `Audio` + one `PhraseEnd` — Stage B replaces with true streaming), `MockTtsSession` in the state-machine mocks module, `CapturingSession` in the `llm_error_synthesises_fallback_line` test. The state-machine SPEAK consumer now drives a closure that maps `Audio → on_committed_audio(chunk.samples)` and `PhraseEnd → on_committed_audio(vec![0.0; inter_phrase_silence_samples])` — same observable timing as the pre-trait-reshape behaviour. Two macOS integration tests rewritten to assert `phrase_end_count == 2` instead of `total_chunks == 2`, decoupling the consumer contract (one `PhraseEnd` per phrase) from the producer's chunk granularity (1 today, ≥2 after Stage B). Bonus fix: `examples/tts_hello.rs` also adapted to the new API (the plan missed that the example also called `push_text` the old way).
- **`e53260c`** core(consts): lift INTER_PHRASE_SILENCE_MS to primer_core::consts::speech. Code-review follow-up on `ed0fcba`. CLAUDE.md's "no magic numbers" rule + the codebase's own documentation that 200 ms is user-tunable both point at lifting the inline `const INTER_PHRASE_SILENCE_MS: u32 = 200;` out of the SPEAK consumer body. New `pub const DEFAULT_INTER_PHRASE_SILENCE_MS: u32 = 200;` in `primer_core::consts::speech`. The `SynthesisEvent::PhraseEnd` doc comment and the enum-level doc now reference the constant via rustdoc intra-doc links instead of free-text "~200 ms" approximations.

**Branch:** `speech/macos-native-pcm-streaming-issue-114` (pushed to origin as a backup; no PR opened).
**Tests:** `cargo test --workspace`: **858 passed / 0 failed / 3 ignored** (was 856 at branch start; +2 from Task 1's new primer-core tests — object-safety pin + audio-before-phrase-end ordering). `cargo test -p primer-speech --features voice-loop`: **85 / 0 / 2** (was 84; +1 from the same primer-core lift). `cargo test -p primer-speech --features macos-native`: **42 / 0 / 3**. fmt + clippy `-D warnings` clean on default features and on `--features primer-gui/speech`.

**Process notes for next session:** Used subagent-driven development. Tasks 1, 2 each had a single implementer + spec reviewer + code-quality reviewer. Tasks 3-6 were batched into one subagent dispatch (the plan deliberately staged Tasks 3-5 without committing because the workspace doesn't compile between them; Task 6 atomically committed them along with the consumer change). The batched integration commit went through full spec + code-quality review, which surfaced the magic-number lift. Tasks 7-9 will follow the same batched pattern (the plan also stages 7+8 and commits at Task 9).

## What's next

### Stage B (Tasks 7-9, atomic commit)

The actual #114 fix lives here. The plan's Tasks 7-9 commit as a single Stage B integration:

- **Task 7**: Add two consts to `src/crates/primer-speech/src/macos/tts.rs`:
  - `PCM_EVENT_CHANNEL_CAPACITY: usize = 64` — bounded `mpsc::sync_channel` capacity (~6× the observed 10 callbacks/phrase).
  - `STREAM_DRAIN_POLL_MS: Duration = Duration::from_millis(10)` — background-path `recv_timeout` slice.
- **Task 8**: Add a failing structural test `streaming_emits_multiple_audio_events_before_phrase_end` to `tests/macos_tts.rs` that asserts ≥2 `Audio` events arrive before the first `PhraseEnd`. The Stage-A wrapper produces exactly 1, so the test fails on `ed0fcba`'s code. This is the TDD gate before implementing Stage B.
- **Task 9**: Implement `synthesize_streaming_main_thread` and `synthesize_streaming_background` in `macos/tts.rs`. PCM callback (running on the GCD main queue) sends `SynthesisEvent::Audio(chunk)` into a bounded `mpsc::sync_channel`; the caller thread drains the channel via `try_recv` (main path, interleaved with `runUntilDate(10ms)` slices) or `recv_timeout` (background path). EOS sentinel sends `PhraseEnd`. **Drops the `DispatchSemaphore` machinery from the background path** — `PhraseEnd` is the synchronisation primitive now. Wire `MacosTtsSession::push_text` / `finalize` to call `synthesize_streaming` instead of the Stage-A wrapper.

After Task 9, the failing test from Task 8 must pass. **This is the moment #114 closes** in behavioural terms — `cargo test -p primer-speech --features macos-native --test macos_tts` should report 7 passed / 1 ignored (was 5 + 1; +2 from the test plus an existing one rewritten to use the streaming API).

### Stage B cleanup (Task 10)

After Stage B lands, the following items in `macos/tts.rs` become dead code:
- `fn synthesize_to_chunks` + the `_main_thread` / `_background` pair
- `fn coalesce_phrase`
- `fn chunks_to_audio_buffer`
- `fn pcm_callback`
- `type Accumulator = Arc<Mutex<Vec<AudioChunk>>>`
- `struct DispatchSemaphore` + its FFI declarations (`dispatch_semaphore_create`, `_signal`, `_wait`, `dispatch_release`, `dispatch_time`, `DISPATCH_TIME_NOW`, `TIMEOUT_NS`, `dispatch_semaphore_t` typedef)
- `struct SynthCtx`

Task 10 deletes all of these. The one-shot `TextToSpeech::synthesize` impl is rewritten to drive `synthesize_streaming` with a local `Vec<f32>` accumulator (`synthesize_to_buffer` helper) — same concatenation behaviour as the deleted `chunks_to_audio_buffer`, no separate code path. The dispatch FFI module shrinks to `dispatch_async_f` + `_dispatch_main_q`. Expected: `tts.rs` drops ~130 lines net (from ~734 to ~600 lines — still over the 500-line guideline; a follow-up issue to split into a `tts/` directory module is documented in the plan's Task 13 Step 5).

### Stage C (Tasks 11-12)

- **Task 11**: State-machine TTFA test with `TimedMockTts` in `voice_loop/state_machine.rs::tests`. New mock emits three `Audio` events at 50 ms intervals (real `std::thread::sleep`, no virtualised time — `push_text` is synchronous so plain sleep is correct), then `PhraseEnd`. The new test `streaming_chunks_reach_speaker_before_phrase_completes` records `Instant::now()` per `on_committed_audio` call and asserts the timestamp of the first 0.1-marker precedes the timestamp of the third 0.3-marker by ≥80 ms — pins the consumer's guarantee that PCM events reach the speaker as they arrive, not buffered after `push_text` returns. Expected: voice-loop test count 85 → 86.
- **Task 12**: Extend `examples/tts_macos_pcm_smoke.rs` with a `--measure-ttfa` flag. Prints three explicit grep-friendly summary lines after the per-callback rows: `[smoke] TTFA: <N> ms (writeUtterance → first PCM callback)`, `[smoke] PhraseEnd: <M> ms (writeUtterance → EOS)`, `[smoke] Streaming win: M - N = <K> ms earlier than coalesce`. No assertion — instrumentation only; the smoke is for re-runs after macOS major releases.

### Task 13 — final verification, push, PR

Per-feature test sweep, fmt + clippy across every relevant combination, then push (already done) and `gh pr create`. The plan's Task 13 Step 5 also has a follow-up issue body for the `tts.rs` file split — open that issue at PR close (it's not part of #114's scope).

**Acceptance criteria for #114:**
- Default-features `cargo test --workspace`: 858 → 859 (the new trait-level explicit-ordering test was already added in `65d54d5`; Stage B doesn't add primer-core tests).
- `cargo test -p primer-speech --features voice-loop`: 85 → 86 (TTFA test).
- `cargo test -p primer-speech --features macos-native`: 42 → 43 (new structural test).
- Other feature combinations unchanged.
- fmt + clippy clean across the full matrix.
- Manual: `cargo run --example tts_macos_pcm_smoke -p primer-speech -- --measure-ttfa` prints `[smoke] Streaming win: <N> ms earlier than coalesce` with `N > 100`.
- Manual: `cargo run -p primer-cli --features speech,macos-native --bin primer -- --speech --name Smoke --age 9 --no-persist --verbose` — Primer's spoken response begins playing within ~100 ms of the LLM stream completing (subjective; the prior delay was clearly noticeable).

### Other carried-forward items (unchanged from yesterday's brief)

The full list of carried-forward follow-ups is in the previous brief at [docs/handoffs/2026-05-18T2114+0800-issue-103-pr-121-cancel-retry-transcript-stitch.md](docs/handoffs/2026-05-18T2114+0800-issue-103-pr-121-cancel-retry-transcript-stitch.md). None changed this session. Highlights:

- **#98** — split `tests/common/sweep.rs` into `bm25`/`hybrid` submodules. **Defer until Hindi or another third locale lands.**
- **#46** — Hybrid sweep: explore post-RRF `min_score` as a fifth grid axis.
- **#41, #40, #22, #21, #20** — all explicitly deferred per their issue bodies.
- **Hindi locale follow-ups** — native-speaker review of `prompts/hi.toml`, Hindi children's-vocabulary corpus, real-LLM smoke, flip-to-stable PR.
- **OpenAI-compat real-server smoke testing** — spin up oMLX / LM Studio / vLLM, run `--backend openai-compat --openai-compat-url http://localhost:8000`, confirm SSE streaming + error classification + embedder round-trip.
- **Klexikon corpus expansion** past 66 articles to close the 2 corpus gaps (`gänsehaut` reflex; tides on `mond`).
- **Local llama.cpp inference (Phase 1.1)** — `LlamaCppBackend` stub remains the entry point.
- **Voice-loop hardening** — echo cancellation, ambient-noise robustness. #114 (this branch) is the first step in this area.
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default.
- **Branch-protection-on-main** — repo owner needs to flip a GitHub setting: Settings → Branches → require `cargo test (default features)` check on `main`.

## Open decisions / risks

Carried-forward open items (still relevant):

- Smoke #6 (Ollama daemon down) deferred verification.
- `is_request()` heuristic conflates config errors with network errors.
- HTTP-date form `Retry-After` silently dropped (true on both Rust and Python sides).
- Pre-flight auth check at startup deferred.
- Voice-mode bridging hook is data-only.
- `close_session` may drop the final classifier task's `turn_classifications` row on a short conversation.
- Identifier-format divergence across structured-output crates (`llm:{model}` vs `llm:{backend}:{model}`).
- `apply_comprehension` insertion policy (only updates concepts already in `learner.concepts`).
- Concepts are monotonic on `learner_concepts` — saved-once stays saved.
- No cancellation token on streaming-task spawns.
- Background-task spawn order is load-bearing on serialized backends.
- "I just don't want to see this word" escape valve for vocab not yet implemented.

Newly carried into this brief from Stage A:

- **`SynthesisSession` trait change is a workspace-wide compile breakage between Task 1 and Task 6.** The plan was deliberately structured to commit the trait change first (`65d54d5`) and then the adapt sweep as an atomic Task 6 commit, so any single-commit revert lands at a green workspace state. The intermediate state on the branch (between `65d54d5` and `ed0fcba`) does NOT compile — `git bisect` users beware. The code-review on Task 1 flagged this as a "blocking defect" even though the prompt described it as by-design; if a fresh reviewer raises it again at PR time, link them to the plan's "atomic Task 6" rationale.
- **The Stage-A `MacosTtsSession` wrapper is a deliberate intermediate state.** It still synthesises the full phrase via the existing `synthesize_to_chunks` + `coalesce_phrase` path and emits exactly one `Audio` + one `PhraseEnd` per phrase. The `streaming_session_yields_chunks_for_one_phrase` test asserts `audio_count >= 1` (not `== 1`) specifically so Stage B can grow that count without rewriting the test again. The doc comment on the wrapper says "Stage-A wrapper" and references #114.
- **`examples/tts_hello.rs` got a bonus fix** (`ed0fcba`) — the plan didn't enumerate it, but the trait change broke it. The adaptation uses a named `collect` closure re-used across `push_text` and `finalize`; faithful translation of the old `for chunk in ...` pattern.
- **`PCM_EVENT_CHANNEL_CAPACITY = 64` is a magic-number candidate worth flagging.** The plan defines it inline in `macos/tts.rs` rather than in `primer_core::consts::speech`. Per the Stage A code-review's `DEFAULT_INTER_PHRASE_SILENCE_MS` lift, the equivalent for `PCM_EVENT_CHANNEL_CAPACITY` is to put it in `primer_core::consts::speech::PCM_EVENT_CHANNEL_CAPACITY`. **Decision for next session:** do this from the start in Task 7 — lift to consts immediately, not as a follow-up. Same for `STREAM_DRAIN_POLL_MS`. The plan's "inline const" instruction is overridden by the now-established convention.

Carried over from PR #121's brief, still pending:

- **The append separator is a single space, not a punctuation cue.** Whisper transcripts typically include surrounding punctuation in the segment text but the trim+single-space concatenation in `state_machine.rs` preserves it correctly.
- **`ScriptedStreamingStt` is intentionally minimal.** Drains a FIFO of finalize texts; doesn't model partial pushes, errors, or latency.
- **The `if !new_text.is_empty()` gate around the append matters.** Without it, an iteration whose `finalize()` returned an empty segment list would append a phantom trailing space. Pinned by `pin_empty_finalize_gate_*` regression tests.

Carried over from PR #120's brief, still pending:

- Backdrop-click-closes is preserved for settings, intentionally NOT added for voice-consent.
- Escape on the voice-consent modal now routes through `onCancel` (newly enabled by `<dialog>`'s native cancel event).
- The HTML-tag walker `tag_for_id` is intentionally simple.
- `dialog.modal[open] { display: flex; ... }` is the load-bearing CSS rule.
- The `cancelListener` removal in voice.js cleanup() is load-bearing.

Carried over from earlier briefs, still pending:

- CSP regression test reads `tauri.conf.json` via `include_str!` from a hardcoded relative path.
- `SpeechLoopConfig` shape differs between speech builds (3 fields macos-native / 7 fields else).
- Open-counter is thread-local, not global.
- `SqliteSessionStore::set_locale` is mutable by reference.

## Patterns to reuse, not reinvent

(All inherited from prior sessions; see [docs/handoffs/2026-05-18T2114+0800-issue-103-pr-121-cancel-retry-transcript-stitch.md](docs/handoffs/2026-05-18T2114+0800-issue-103-pr-121-cancel-retry-transcript-stitch.md) for the full list.)

New from this session:

- **Two-stage TDD with explicit intermediate state.** When a trait reshape is the cleanest end-state but lands in a workspace that has 5+ in-tree implementors, structure the plan so the trait change commits first as a known-broken intermediate, then ONE atomic commit fixes everything else. This keeps each commit reviewable (small, focused diff) while preserving the property that every "green" SHA in the branch history actually compiles. The intermediate broken state must be flagged in the plan and in the PR description so reviewers know not to flag it as a defect.
- **Stage-A wrapper preserves observable behaviour while changing the trait surface.** When the real change (Stage B's true streaming) is risky, ship a Stage-A wrapper first that adapts the OLD impl to the NEW trait with zero observable difference. The Stage-A wrapper is throwaway code that exists for one commit, but it gives reviewers a safe shape to land before the substantive change arrives. Tests must be written against the consumer contract (here: `phrase_end_count`), not the producer's chunk granularity, so they don't break between Stage A and Stage B.
- **Batch "stage but don't commit" tasks into one subagent dispatch.** When a plan deliberately stages changes across multiple tasks without committing (because intermediate states don't compile), don't dispatch each task to a separate fresh subagent — the staged state doesn't transfer across context boundaries cleanly. Batch them all into one dispatch with the atomic commit as the final step. Full subagent review of the batched output is still appropriate.
- **Magic-number lift convention is enforced by code review.** CLAUDE.md says "no magic numbers — never inline." Plans that leave constants inline (even with `const` declarations inside function bodies) WILL be caught and rejected by the code-quality reviewer. Lift to `primer_core::consts::<module>` from the start, not as a follow-up. The doc comment on the consumer enum should reference the constant via rustdoc intra-doc link, not free-text approximation.
- **The Named super-trait canary is the canonical place to add a new speech trait.** When adding any trait that inherits `Named`, the SAME PR must add a corresponding assertion to `named_super_trait_resolves_via_each_speech_trait` and update the doc comment's "fifth/sixth/Nth leaf trait" maintenance hint. Object-safety canaries (`fn _accepts_boxed(_s: Box<dyn YourTrait>) {}` inside a `#[test]`) are also free-cost compile-time guarantees worth adding for any trait that's used through `Box<dyn>`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status                       # confirm clean state
git fetch --all --prune

# Check for any new PRs or issues opened since this brief.
gh pr list --state open
gh issue list --state open --limit 30

# Switch to the in-flight branch.
git checkout speech/macos-native-pcm-streaming-issue-114
git log --oneline -8             # e53260c on top, 7 commits ahead of main

# Opt-in to the local pre-commit hook (one-time per clone; from PR #109):
git config core.hooksPath .githooks

cd src
~/.cargo/bin/cargo build --workspace && ~/.cargo/bin/cargo test --workspace
# Expected: 858 passed, 0 failed, 3 ignored (default features).

~/.cargo/bin/cargo test -p primer-speech --features voice-loop
# Expected: 85 passed, 0 failed, 2 ignored.

~/.cargo/bin/cargo test -p primer-speech --features macos-native
# Expected: 42 passed, 0 failed, 3 ignored.

~/.cargo/bin/cargo fmt --all -- --check
# Expected: clean.

~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
# Expected: clean exit 0.

~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech -- -D warnings
# Expected: clean exit 0.

# Resume work at Task 7 of the plan. The plan's Tasks 7-9 commit as a single
# Stage B integration commit (the plan stages Tasks 7 + 8 without committing
# because the workspace breaks between them). Recommended workflow:
#   1. Read the spec at docs/superpowers/specs/2026-05-19-macos-tts-pcm-streaming-design.md
#   2. Read the plan at docs/superpowers/plans/2026-05-19-macos-tts-pcm-streaming.md
#      Resume at Task 7 (line ~750 of the plan).
#   3. NOTE the open decision in this brief: lift PCM_EVENT_CHANNEL_CAPACITY
#      and STREAM_DRAIN_POLL_MS to primer_core::consts::speech immediately
#      rather than as a follow-up. Update the plan's Task 7 instructions
#      accordingly.
#   4. Continue with Tasks 7-13. Tasks 7-9 batch into one subagent dispatch
#      mirroring Tasks 3-6's pattern this session.
```

To do a manual voice-mode smoke verifying Stage B (recommended after Task 9 lands and the structural test passes):

```bash
cd /Users/hherb/src/primer/src

# Macos-native CLI build:
~/.cargo/bin/cargo run --features primer-cli/speech,primer-cli/macos-native --bin primer -- \
    --speech --name SmokeTester --age 9 --no-persist --verbose
# Speak a 2-3 sentence response prompt. Expected: Primer's spoken response
# begins playing within ~100 ms of the LLM stream completing. Pre-Stage-B
# (current HEAD), the delay is ~hundreds of ms per phrase and noticeable.

# Manual TTFA smoke (after Task 12 lands):
~/.cargo/bin/cargo run --example tts_macos_pcm_smoke -p primer-speech -- --measure-ttfa
# Expected: prints "[smoke] Streaming win: <N> ms earlier than coalesce"
# with N > 100.
```

To exercise the Hindi preview locale manually (carried-forward — no changes this session):

```bash
cd /Users/hherb/src/primer/src
RUST_LOG=primer::prompt_pack=warn,info ~/.cargo/bin/cargo run --bin primer -- \
    --backend stub --name Aarav --age 8 --language hi --no-persist
```

For a real-LLM Hindi smoke (carried-forward):

```bash
cd /Users/hherb/src/primer/src
ANTHROPIC_API_KEY=... RUST_LOG=info ~/.cargo/bin/cargo run --bin primer -- \
    --backend cloud --name Aarav --age 9 --language hi --no-persist --verbose 2>&1 | tee /tmp/smoke_hi.log
```

For an OpenAI-compat smoke (carried-forward — spin up a local server first):

```bash
llama-server --port 8000 --model /path/to/some.gguf

cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --bin primer -- \
    --backend openai-compat --openai-compat-url http://localhost:8000 \
    --model <model-id-from-server> \
    --name SmokeTester --age 9 --no-persist --verbose
```

To re-run the German regression benchmarks (carried-forward; unchanged this session):

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_de
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_hybrid_de
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid_de
```

For the Python ingestion pipeline tests (uv-only — never invoke pip directly):

```bash
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If you discover that Horst did interim work that changes the plan, flag it. Always verify open-issue claims and `git log` since this brief's "last updated" timestamp before starting.
- The #114 acceptance criteria are at the top of the "Task 13 — final verification, push, PR" section; check them off as you land each.
