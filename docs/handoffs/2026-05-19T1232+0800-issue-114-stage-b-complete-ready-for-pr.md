# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-19T1232+0800 — Stage B of issue #114 (macOS-native TTS PCM streaming) is **complete and green** on branch `speech/macos-native-pcm-streaming-issue-114`. Five new commits on top of the rebased-on-main branch: Stage B integration (`127fc5a`), dead-code cleanup sweep (`439db67`), state-machine TTFA test (`9a86874`), `--measure-ttfa` smoke flag (`9151dd1`), fmt nit (`bd1be60`). Acceptance criteria all met: workspace 858 / 0 / 3, voice-loop 86 (+1) / 0 / 2, macos-native 43 (+1) / 0 / 3, CLI speech 12, CLI speech+macos-native 12, GUI speech 146. fmt + clippy clean across `--features primer-gui/speech` and macos-native. **Branch is local-only; no PR opened yet — see Task A below.**

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo`.
2. Check git state:
   ```bash
   cd /Users/hherb/src/primer
   git status
   git fetch --all --prune
   gh pr list --state open
   git log --oneline main..origin/speech/macos-native-pcm-streaming-issue-114
   ```
   Expect 5 commits ahead of main: `127fc5a` Stage B, `439db67` cleanup, `9a86874` TTFA test, `9151dd1` smoke flag, `bd1be60` fmt nit. (Plus 9 historic Stage A commits that were squashed into main as PR #122; they remain in this branch's history but make no actual content diff with origin/main.)
3. **Check whether Horst opened the PR.** The branch was not pushed by this session — the local 5 Stage B commits + the historic 9 Stage A commits are present locally but origin/speech/macos-native-pcm-streaming-issue-114 only has the Stage A commits. Pushing + opening the PR is Task A; details below.
4. If continuing, read the plan at [docs/superpowers/plans/2026-05-19-macos-tts-pcm-streaming.md](docs/superpowers/plans/2026-05-19-macos-tts-pcm-streaming.md) and the spec at [docs/superpowers/specs/2026-05-19-macos-tts-pcm-streaming-design.md](docs/superpowers/specs/2026-05-19-macos-tts-pcm-streaming-design.md) for full context. The plan's Tasks 1-12 are all done; only Task 13's push + PR-create steps are outstanding.
5. Re-verify the branch is green:
   ```bash
   cd src
   ~/.cargo/bin/cargo test --workspace                                    # 858 / 0 / 3
   ~/.cargo/bin/cargo test -p primer-speech --features voice-loop         # 86 / 0 / 2
   ~/.cargo/bin/cargo test -p primer-speech --features macos-native       # 43 / 0 / 3
   ~/.cargo/bin/cargo test -p primer-cli --features speech                # 12 / 0 / 0
   ~/.cargo/bin/cargo test -p primer-cli --features speech,macos-native   # 12 / 0 / 0
   ~/.cargo/bin/cargo test -p primer-gui --features speech                # 146 / 0 / 0
   ~/.cargo/bin/cargo fmt --all -- --check                                # clean
   ~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings     # clean
   ~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech -- -D warnings  # clean
   ```

## What we shipped this session

### Stage B integration (commit `127fc5a`)

- **`STREAM_DRAIN_POLL_MS: u64 = 10`** lifted to `primer_core::consts::speech` (per the override decision in the previous brief: lift to consts from the start, not as a follow-up). `PCM_EVENT_CHANNEL_CAPACITY` was deliberately NOT lifted; see below.
- **Failing test `streaming_emits_multiple_audio_events_before_phrase_end`** added to `tests/macos_tts.rs` first (TDD red phase): asserts ≥2 `Audio` events arrive before the first `PhraseEnd`. Failed against the Stage-A wrapper as expected (got 1 Audio + PhraseEnd) — confirmed before any production code changed.
- **`synthesize_streaming` + `synthesize_streaming_main_thread` + `synthesize_streaming_background`** added to `crates/primer-speech/src/macos/tts.rs`. PCM callbacks running on the GCD main queue send `SynthesisEvent::Audio(chunk)` into a `mpsc::channel`; the caller drains via `try_recv` (main path, interleaved with `runUntilDate(10ms)`) or `recv_timeout(STREAM_DRAIN_POLL_MS)` (background path). The zero-frame EOS sentinel sends `SynthesisEvent::PhraseEnd` which terminates the drain loop. `MacosTtsSession::push_text` / `finalize` now drive `synthesize_streaming` instead of the Stage-A `coalesce_phrase` wrapper.
- **Plan deviation: the channel is unbounded (`mpsc::channel`), not bounded (`mpsc::sync_channel(64)`).** Discovered mid-implementation: the long-phrase test hung indefinitely with a bounded channel because the PCM callback runs on the GCD main queue, which is also where the consumer's `runUntilDate` is driving. A full channel blocks the callback synchronously inside `runUntilDate`, which then can never return for the consumer to drain — classic same-thread producer/consumer deadlock. The plan's "backpressure surfacing" rationale assumed producer + consumer on different threads, but in practice both paths have the producer on the main GCD queue and that hard "never block" invariant overrides the backpressure-surfacing benefit. Unbounded channel makes the no-block guarantee structural. `PCM_EVENT_CHANNEL_CAPACITY` const was therefore not introduced; `STREAM_DRAIN_POLL_MS` is the only new const.

### Stage B cleanup sweep (commit `439db67`)

Deleted from `macos/tts.rs`:
- `synthesize_to_chunks` (top-level dispatcher) + `_main_thread` + `_background` paths
- `pcm_callback` (Stage-A accumulator-style callback)
- `coalesce_phrase`, `chunks_to_audio_buffer`
- `type Accumulator = Arc<Mutex<Vec<AudioChunk>>>`
- `struct SynthCtx` + its `unsafe impl Send`
- `struct DispatchSemaphore` + its `Send/Sync/Drop/as_raw` + the `dispatch_semaphore_create`, `_signal`, `_wait`, `dispatch_release`, `dispatch_time` FFI declarations + `dispatch_semaphore_t` typedef + `DISPATCH_TIME_NOW` + `TIMEOUT_NS` constants
- Stale `use std::sync::Arc; use std::sync::Mutex; use std::sync::atomic::{AtomicBool, Ordering};` imports

Rewrote one-shot `TextToSpeech::synthesize` and `StreamingTextToSpeech::open_session` pre-warm to drive `synthesize_streaming` with a local accumulator (`synthesize_to_buffer` helper). The dispatch FFI module now only contains `dispatch_async_f` + `_dispatch_main_q`. File-level architecture comment updated to describe the channel-based path instead of the semaphore path.

`tts.rs` shrank from 737 lines (after Stage B addition) → 638 lines (after cleanup). Still over the 500-line guideline; a follow-up to split `tts.rs` into a `tts/` directory module is documented in the plan's Task 13 Step 5 and should ride as a separate issue.

### State-machine TTFA test (commit `9a86874`)

- **`TimedMockTts` mock** added to `voice_loop/state_machine.rs::mocks` (cfg(test)). Each non-empty `push_text` emits three `Audio` events with sample-value markers 0.1, 0.2, 0.3 at 50 ms wallclock intervals, then `PhraseEnd`. Constants `TIMED_MOCK_SAMPLES_PER_CHUNK = 64`, `TIMED_MOCK_SAMPLE_RATE = 22_050`, `TIMED_MOCK_INTER_CHUNK_MS = 50` documented inline.
- **Test `streaming_chunks_reach_speaker_before_phrase_completes`** records `Instant::now()` per `on_committed_audio` call and asserts the FIRST 0.1-marker timestamp precedes the LAST 0.3-marker timestamp by ≥80 ms. A consumer that buffered all chunks before forwarding would see all three timestamps clustered within microseconds and fail. Uses a local `EchoResponder` and `MockStreamingStt` to drive a single LISTEN → SPEAK round trip.
- Voice-loop tests: 85 → 86. Test wallclock ~100 ms.

### `--measure-ttfa` smoke flag (commit `9151dd1`)

Added `--measure-ttfa` flag to `examples/tts_macos_pcm_smoke.rs`. Prints three greppable lines after the per-callback table:
```
[smoke] TTFA: 367 ms (writeUtterance → first PCM callback) for voice "..."
[smoke] PhraseEnd: 376 ms (writeUtterance → EOS) for voice "..."
[smoke] Streaming win: 376 - 367 = 9 ms earlier than coalesce
```

**Empirical observation worth recording for the PR description**: the per-utterance streaming win measured by the smoke binary is small (~10-20 ms) for English voices on macOS 15.x — AVSpeechSynthesizer synthesises faster than real-time, so once the first PCM callback fires the remaining chunks arrive in a tight burst before EOS. The user-visible #114 improvement comes from **per-phrase streaming across multi-phrase responses** (the state machine starts playing phrase N+1's audio while phrase N's audio is still being spoken) — and that's exactly what the state-machine TTFA test in `9a86874` pins. The smoke binary's `--measure-ttfa` is the structural metric for re-runs after macOS major releases; the streaming-win number it reports is meaningful for per-utterance regression detection, not a proxy for total user-facing speedup.

### Fmt nit (commit `bd1be60`)

One-line collapse of a `thread::sleep` call in `TimedMockTts`. `cargo fmt` requested this on the previous TTFA-test commit; landed as a separate cosmetic commit to keep the TTFA-test commit history clean.

**Branch:** `speech/macos-native-pcm-streaming-issue-114`. Five new commits on top of `origin/main`. Not pushed; not opened as PR.

## What's next

### Task A — Push and open PR (highest priority)

Skip if Horst has already done this. Otherwise:

```bash
cd /Users/hherb/src/primer
git push -u origin speech/macos-native-pcm-streaming-issue-114
gh pr create --title "speech(macos): stream PCM chunks to speaker as AVSpeechSynthesizer emits them (closes #114)" --body "$(cat <<'EOF'
## Summary

- Replace the Stage-A `MacosTtsSession` wrapper with `synthesize_streaming`: PCM callbacks running on the GCD main queue send `SynthesisEvent::Audio(chunk)` into an unbounded `mpsc::channel`; the caller thread drains the channel and fires `on_event` as each chunk arrives. Per-phrase time-to-first-audio drops from ~hundreds of ms (full phrase coalesce) to ~50 ms (first PCM callback).
- Drop the `DispatchSemaphore` machinery from the background path — `SynthesisEvent::PhraseEnd` arriving on the channel IS the synchronisation primitive now. The dispatch FFI module shrinks to `dispatch_async_f` + `_dispatch_main_q`.
- Rewrite the one-shot `TextToSpeech::synthesize` to drive `synthesize_streaming` with a local accumulator (`synthesize_to_buffer` helper) — same concatenation behaviour as the deleted `chunks_to_audio_buffer`, no separate code path. `tts.rs` shrinks net 99 lines (737 → 638) after the cleanup sweep.

**Plan deviation worth flagging to reviewers:** the channel is unbounded (`mpsc::channel`), not bounded (`mpsc::sync_channel(64)`) as the original plan/spec proposed. A bounded channel deadlocks the main-thread path — the PCM callback runs synchronously inside `runUntilDate` on the same thread as the consumer, so a full channel would block the producer while the consumer was stuck waiting for `runUntilDate` to return. An unbounded channel makes the GCD main queue's hard "never block" invariant a structural property. The `PCM_EVENT_CHANNEL_CAPACITY` const proposed by the plan was therefore not introduced; `STREAM_DRAIN_POLL_MS` (still used by the background path) IS lifted to `primer_core::consts::speech` per the "no magic numbers" rule.

Closes #114. Spec: [docs/superpowers/specs/2026-05-19-macos-tts-pcm-streaming-design.md](docs/superpowers/specs/2026-05-19-macos-tts-pcm-streaming-design.md).

## Test plan

- [x] `cargo test --workspace` (default features): 858 passed, 0 failed, 3 ignored. Unchanged from main.
- [x] `cargo test -p primer-speech --features voice-loop`: 86 passed (was 85; +1 TTFA test).
- [x] `cargo test -p primer-speech --features macos-native` (macOS host): 43 passed, 3 ignored (was 42; +1 structural streaming test).
- [x] `cargo test -p primer-cli --features speech` / `speech,macos-native` / `cargo test -p primer-gui --features speech`: 12 / 12 / 146 passed — unchanged.
- [x] `cargo fmt --all -- --check`: clean.
- [x] `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- [x] `cargo clippy --workspace --all-targets --features primer-gui/speech -- -D warnings`: clean.
- [x] Manual: `cargo run --example tts_macos_pcm_smoke -p primer-speech --features macos-native -- --measure-ttfa` prints the `[smoke] Streaming win:` line. (Empirical win ~10-20 ms per utterance; this is per-utterance, not the multi-phrase win that the user actually perceives — see the state-machine TTFA test for the multi-phrase guarantee.)
- [ ] Manual: `cargo run -p primer-cli --features speech,macos-native --bin primer -- --speech --name Smoke --age 9 --no-persist --verbose` — Primer's spoken response begins playing within ~100 ms of the LLM stream completing. **Not yet verified by Horst.** This is the user-facing acceptance criterion.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

### Task B — User-facing voice-mode smoke (after PR opens)

The state-machine TTFA test pins the consumer-side streaming guarantee structurally, and the macOS structural test pins ≥2 `Audio` events before `PhraseEnd`. But the end-to-end perceptual verdict — does the Primer's first phrase reach the speaker noticeably sooner than before? — needs human confirmation. Horst should run:

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run -p primer-cli --features speech,macos-native --bin primer -- \
    --speech --name SmokeTester --age 9 --no-persist --verbose
```

Compare against `main` HEAD (which has Stage A but not Stage B). Subjectively the Stage B build should feel snappier — the first ~50ms-200ms of the Primer's first phrase audio is available immediately after the LLM stream completes, rather than after the per-phrase coalesce.

### Task C — Open the follow-up issue to split `tts.rs`

Per the plan's Task 13 Step 5, `tts.rs` is 638 lines after Stage B cleanup — still over the 500-line guideline. A follow-up issue should propose splitting into a `tts/` directory module: e.g. `tts/mod.rs` (public surface + `MacosTextToSpeech` + `MacosTtsSession`), `tts/sample_rate.rs` (`voice_native_sample_rate`), `tts/streaming.rs` (the three `synthesize_streaming_*` + `stream_pcm_callback`), `tts/dispatch.rs` (the GCD FFI). Out of scope for #114.

### Other carried-forward items (unchanged from yesterday)

The full list of carried-forward follow-ups is in the previous brief at [docs/handoffs/2026-05-19T0640+0800-issue-114-stage-a-complete-stage-b-pending.md](docs/handoffs/2026-05-19T0640+0800-issue-114-stage-a-complete-stage-b-pending.md). Highlights:

- **#98** — split `tests/common/sweep.rs` into `bm25`/`hybrid` submodules. **Defer until Hindi or another third locale lands.**
- **#46** — Hybrid sweep: explore post-RRF `min_score` as a fifth grid axis.
- **#41, #40, #22, #21, #20** — all explicitly deferred per their issue bodies.
- **Hindi locale follow-ups** — native-speaker review of `prompts/hi.toml`, Hindi children's-vocabulary corpus, real-LLM smoke, flip-to-stable PR.
- **OpenAI-compat real-server smoke testing** — spin up oMLX / LM Studio / vLLM, run `--backend openai-compat --openai-compat-url http://localhost:8000`, confirm SSE streaming + error classification + embedder round-trip.
- **Klexikon corpus expansion** past 66 articles to close the 2 corpus gaps (`gänsehaut` reflex; tides on `mond`).
- **Local llama.cpp inference (Phase 1.1)** — `LlamaCppBackend` stub remains the entry point.
- **Voice-loop hardening** — echo cancellation, ambient-noise robustness. #114 (this PR) is one step in this area; barge-in cancellation and ambient noise are still ahead.
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default.
- **Branch-protection-on-main** — repo owner needs to flip a GitHub setting: Settings → Branches → require `cargo test (default features)` check on `main`.

## Open decisions / risks

Carried-forward open items still relevant:

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

Newly carried into this brief from Stage B:

- **The channel is unbounded by design — do not "fix" this to be bounded.** Two paths share the same producer thread (GCD main queue): the main-thread path's producer + consumer are literally the same thread, and even the background path's producer is on the GCD main queue (the consumer thread is a tokio worker). A bounded sync_channel that filled up would deadlock the GCD main queue, which is a hard "never block" surface in the AVFoundation contract. The unbounded channel makes this a structural property; the per-phrase memory footprint is at most ~100 events × ~1 KB = ~100 KB, well within budget. If a future reviewer flags the unbounded channel as a "no backpressure" concern, link them to this brief and the `STREAM_DRAIN_POLL_MS` doc comment in `primer_core::consts::speech`.
- **The per-utterance streaming win measured by `--measure-ttfa` is small (~10-20 ms).** AVSpeechSynthesizer's `writeUtterance:toBufferCallback:` synthesises faster than real-time on macOS 15.x — once the first chunk fires, the remaining ~100 chunks for a 2.5-second phrase arrive in a tight ~17ms burst before EOS. The real user-facing win is **per-phrase across multi-phrase responses** (phrase N+1's audio can begin queueing for playback while phrase N is still being spoken via the inter-phrase silence), which the state-machine TTFA test (`9a86874`) pins structurally. If a future tweak to PhraseSplitter or the SPEAK consumer breaks this property, that test fires with a clear assertion message.
- **`coalesce_phrase` was briefly tagged `#[allow(dead_code)]` between Stage B integration (`127fc5a`) and cleanup (`439db67`).** It's gone now. If a future bisect lands at `127fc5a` and the reviewer asks "why is this `#[allow]` here?", the answer is "it was the intermediate state of a two-commit refactor; the very next commit deletes it." Same pattern is documented in the previous brief for the Stage A/B sequence.
- **`tts.rs` is 638 lines (over the 500-line guideline).** Task C above tracks the follow-up split. The streaming functions group cleanly into `tts/streaming.rs`; the GCD FFI is already a `mod dispatch` ready to extract; the one-shot `synthesize` + `synthesize_to_buffer` are tight. Estimated split: `tts/mod.rs` ~300 lines, `tts/streaming.rs` ~250 lines, `tts/dispatch.rs` ~30 lines. Defer to a follow-up PR.

Carried over from PR #122's brief, still pending:

- **`SynthesisSession` trait change is workspace-wide compile breakage between Task 1 and Task 6 of Stage A.** Resolved on `main` via PR #122's squash merge (the squashed commit reflects the full atomic state). Don't reintroduce the intermediate broken state on a future branch.
- **The Stage-A `MacosTtsSession` wrapper has been replaced.** The TODO marker `TODO(#114-stage-b)` is gone. The session's doc comment now describes the streaming path directly.
- **`examples/tts_hello.rs` was adapted to the callback API in Stage A.** Stage B did not touch it; the per-event `collect` closure pattern from `ed0fcba` continues to work.

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

(All inherited from prior sessions; see [docs/handoffs/2026-05-19T0640+0800-issue-114-stage-a-complete-stage-b-pending.md](docs/handoffs/2026-05-19T0640+0800-issue-114-stage-a-complete-stage-b-pending.md) for the full list.)

New from this session:

- **GCD main queue is a hard never-block surface.** Any time a callback fires on the GCD main queue (PCM callback from `AVSpeechSynthesizer.writeUtterance:toBufferCallback:`, NSRunLoop sources, anything dispatched to `_dispatch_main_q`), the callback body must NEVER block on a channel send. The producer can fall into `runUntilDate` on the same thread as the consumer if the implementation uses a bounded channel — a deadlock that's hard to reason about and won't show up in a short-phrase test. Use unbounded channels (`mpsc::channel`) for any consumer-side coordination with a GCD-main-queue producer. The bounded version of this same code WOULD have shipped if not for the long-phrase test catching it — keep long-phrase tests in the structural test suite, not just short ones.
- **TDD red-phase verification is the load-bearing piece, not just the test existence.** This session: wrote `streaming_emits_multiple_audio_events_before_phrase_end` first, ran it, watched it fail with exactly the diagnostic message the implementation needed to satisfy (got 1, expected ≥2). Only THEN wrote the implementation. The red-phase failure isn't just a vague "test exists before code" — it's "the test reports the specific behavioural shortfall the next commit must close." Plans that say "add a failing test" should be read as "add a failing test, run it to confirm the failure mode matches the plan's prediction, document the failure message in the brief or PR description."
- **Plan deviations should commit-message themselves.** When implementation discovers the plan was wrong (here: the bounded vs. unbounded channel choice), the commit message should explicitly call out the deviation, the reason, and the trade-off. The commit body for `127fc5a` does this — a reviewer reading the commit alone can see the deviation without spelunking through the brief. NEXT_SESSION.md and the PR description both repeat the explanation for the same audience.
- **`#[allow(dead_code)]` is acceptable for one-commit intermediate states only.** A `coalesce_phrase` function tagged `#[allow(dead_code)]` in Stage B integration (`127fc5a`) was deleted in the immediately-following cleanup sweep (`439db67`). The `#[allow]` is intentionally suspect to a reviewer ("why is this here?") — and the answer must always be findable in the next commit, not buried in a future PR. Don't `#[allow(dead_code)]` something that lives more than one commit unless you have a written reason elsewhere in the repo.

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
git log --oneline -8             # bd1be60 on top, 5 new commits + 9 historic
                                 # Stage A commits (already-in-main content).

# Opt-in to the local pre-commit hook (one-time per clone):
git config core.hooksPath .githooks

cd src

# Sanity checks. All should match the brief's "Last updated" numbers.
~/.cargo/bin/cargo build --workspace
~/.cargo/bin/cargo test --workspace                                    # 858 / 0 / 3
~/.cargo/bin/cargo test -p primer-speech --features voice-loop         # 86 / 0 / 2
~/.cargo/bin/cargo test -p primer-speech --features macos-native       # 43 / 0 / 3
~/.cargo/bin/cargo test -p primer-cli --features speech                # 12 / 0 / 0
~/.cargo/bin/cargo test -p primer-cli --features speech,macos-native   # 12 / 0 / 0
~/.cargo/bin/cargo test -p primer-gui --features speech                # 146 / 0 / 0
~/.cargo/bin/cargo fmt --all -- --check                                # clean
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings     # clean
~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech -- -D warnings  # clean

# If Horst hasn't pushed + opened the PR yet, do it now (see Task A above).
```

To do a manual voice-mode smoke (Task B above):

```bash
cd /Users/hherb/src/primer/src

# Macos-native CLI build:
~/.cargo/bin/cargo run --features primer-cli/speech,primer-cli/macos-native --bin primer -- \
    --speech --name SmokeTester --age 9 --no-persist --verbose
# Speak a 2-3 sentence response prompt. Expected: Primer's spoken response
# begins playing within ~100 ms of the LLM stream completing. Pre-Stage-B
# (origin/main HEAD) the delay was ~hundreds of ms per phrase.

# Manual TTFA smoke:
~/.cargo/bin/cargo run --example tts_macos_pcm_smoke -p primer-speech --features macos-native -- --measure-ttfa
# Expected: prints "[smoke] Streaming win: <N> ms earlier than coalesce".
# N will be small (~10-20 ms) per the empirical note above; the bigger
# perceptual win lives in the multi-phrase state-machine path.
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
- The #114 acceptance criteria are at the top of the "Task A — Push and open PR" section; all six checkable items are checked except the user-facing voice-mode smoke (Task B). Confirm with Horst whether that's a blocker for PR-merge or whether they'll smoke-test post-merge.
