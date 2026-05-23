# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-23T1516+0800 — third session of the day. Shipped one Claude-actionable code change: closed issue #124 by extracting the shared drain-loop helper between the two macOS-native streaming TTS paths into a new `primer-speech/src/macos/stream_drain.rs` submodule. PR #136 is open against `main`. The prior session's brief + handoff archive (the 2026-05-23T1444 status-sync output) had been left uncommitted on disk; this session committed them as `f2a14f9` before branching for the code work. PR #134 (macos-native-26) remains DRAFT pending Horst's manual mic round-trip — unchanged.

## What landed since the previous brief

| SHA       | Title                                                                                          | Date              | Notes                                                                                                                                                                                          |
| --------- | ---------------------------------------------------------------------------------------------- | ----------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `f2a14f9` | `docs(handoff): record 2026-05-23T1444 status-sync session`                                    | 2026-05-23 ~15:01 | The 2026-05-23T1444 brief + handoff archive that the prior session left uncommitted. Pure doc commit — verbatim copy of what was on disk; no edits.                                            |
| `6accf18` | `speech(macos): extract shared drain-loop helper (#124)` (on branch `claude/issue-124-drain-helper`, **PR #136**) | 2026-05-23 ~15:11 | New `stream_drain.rs` submodule housing `drive_streaming_drain<F>` + 4 structural `#[cfg(test)]` tests. `tts.rs` shrunk 678 → 671 lines. CLAUDE.md macOS-native bullet updated to point at the new helper. All test gates green (see PR body for the matrix). Closes #124. |

## What we shipped this session

**Closed issue #124** — factor shared drain-loop helper between the two `synthesize_streaming_*` paths in [`primer-speech/src/macos/tts.rs`](src/crates/primer-speech/src/macos/tts.rs). The deadline check + channel-drain + `PhraseEnd`-terminates loop is now expressed once, in [`primer-speech/src/macos/stream_drain.rs::drive_streaming_drain`](src/crates/primer-speech/src/macos/stream_drain.rs); the threading-model difference (main-thread `try_recv` + `NSRunLoop::runUntilDate` vs background `recv_timeout(STREAM_DRAIN_POLL_MS)`) is absorbed by a `next_event: F` closure passed in by each caller.

The shape closely matches the proposal in issue #124's body, with one variation: instead of a `wait_step` closure passed alongside the receiver, the closure directly returns `Result<Option<SynthesisEvent>>`. This lets the helper stay receiver-agnostic — the same helper works for both `mpsc::Receiver<SynthesisEvent>` and any future channel type that might appear (e.g. if `dispatch2` migration in #125 ends up changing the producer side).

Behaviour is functionally equivalent. One observable change: the deadline check now fires per-event rather than once per outer loop iteration — strictly safer, no real-world impact. The timeout error string dropped the (load-bearing-only-for-grep) `NSRunLoop` qualifier on the main-thread path; no tests or external code referenced it.

Structural coverage for both branch shapes ships as `#[cfg(test)] mod tests` inside `stream_drain.rs`:

- `drives_recv_timeout_shape_until_phrase_end` — background-path closure shape (`Ok` / `Timeout` / `Disconnected`)
- `drives_try_recv_shape_until_phrase_end` — main-thread-path closure shape (poll + no-op-wait)
- `returns_timeout_error_when_deadline_has_already_passed` — deadline check
- `propagates_inner_error_from_next_event_closure` — error propagation

These tests are platform-neutral (std::sync::mpsc + std::time only — no ObjC dep) so they run cleanly inside the `#[cfg(all(target_os = "macos", feature = "macos-native"))]`-gated module without needing the `harness = false` machinery that `tests/macos_tts.rs` uses for real-AVFoundation runs.

### Verification matrix (all green this session)

| Command                                                                                | Expected         | Got              |
| -------------------------------------------------------------------------------------- | ---------------- | ---------------- |
| `cargo test --workspace` (default features)                                            | 858/0/3 baseline | **858/0/3**      |
| `cargo test -p primer-speech --features macos-native`                                  | 43/0/3 + 4 new   | **47/0/3**       |
| `cargo test -p primer-cli --features speech,macos-native` (macOS only)                 | 12/0/0           | **12/0/0**       |
| `cargo test -p primer-cli --features speech`                                           | 12/0/0           | **12/0/0**       |
| `cargo fmt --all -- --check`                                                           | clean            | **clean**        |
| `cargo clippy --workspace --all-targets -- -D warnings`                                | clean            | **clean**        |
| `cargo clippy -p primer-speech --features macos-native --all-targets -- -D warnings`   | clean            | **clean**        |
| `cargo build -p primer-gui --features speech`                                          | clean            | **clean**        |

Real-AVFoundation harness tests (`tests/macos_tts.rs`, including `streaming_emits_multiple_audio_events_before_phrase_end`) still pass against the refactored code, confirming the helper drives the actual synth path equivalently.

### Tasks not addressed this session

- **Task A** — Horst-driven manual mic round-trip smoke for PR #134. Still not Claude-actionable.
- **Task B** — PR #134 rebase + flip out of DRAFT. Still depends on Task A. **Note**: PR #134 now has an additional in-flight conflict with this session's PR #136 — but only on `CLAUDE.md`, and only on the macOS-native bullet's tail; cleanly resolvable by taking the PR #136 version (which mentions stream_drain.rs and the closed #124) and re-adding the macos-native-26 paragraph from PR #134 after it.
- **Task E.2** — issue #125 (dispatch2 migration). Still deferred; natural next follow-up after #124.
- **Task E.3** — issue #126 (`spawn_blocking` wrap). Still deferred.
- **Task F** — macos-native-26 plumbing. Still gated on Task A failing.

## What's next — by priority

### Task A — Run the PR #134 manual mic round-trip smoke (Horst-driven)

Unchanged. The single unchecked box on PR #134's test plan:

> - [ ] **Manual mic round-trip** — speak "what colour is the sky" into the mic, verify streaming partials arrive, verify the stub Primer responds, verify quit/bye exits cleanly. **Pending the human reviewer**; that's why this PR is a draft.

```bash
cd /Users/hherb/src/primer
git checkout claude/macos-native-26
cd src
~/.cargo/bin/cargo run --features primer-cli/speech,primer-cli/macos-native-26 --bin primer -- \
    --backend stub --speech --language en --name Smoke --age 8 --no-persist
# Speak: "what colour is the sky"
# Expected: streaming partials → stub Primer response → spoken via AVSpeechSynthesizer
# Exit: type/say "bye" or "exit"
```

If the smoke fails, attach log output to PR #134 as a comment.

### Task B — Resolve the PR #134 merge conflicts and flip out of DRAFT

Resolution rule on the original 5 PR-changed files (`primer-speech/Cargo.toml`, `primer-speech/src/lib.rs`, `macos26/mod.rs`, `macos26/audio_session.rs`, `tests/macos26_smoke.rs`) is still "take PR-branch version". After this session's PR #136 lands, there's also a small new conflict on `CLAUDE.md` (the macOS-native bullet's tail mentions the now-closed #124 and the new `stream_drain.rs` path); resolution is to keep both — preserve PR #136's drain-helper sentence AND keep PR #134's separate macos-native-26 bullet. `Cargo.lock` regen + `cargo fmt --all` after rebase is the cleanest path.

```bash
git checkout claude/macos-native-26
git fetch origin
git rebase origin/main
# Resolve conflicts per the rule above.
git push --force-with-lease origin claude/macos-native-26
gh pr ready 134
```

### Task C — PR #136 review & merge

New this session. PR #136 (issue #124 drain-helper extraction) is open against `main`. Acceptance criteria from the issue body are all met:

> - [x] Both `synthesize_streaming_main_thread` and `synthesize_streaming_background` share the drain-loop helper.
> - [x] All existing tests pass (workspace 858, macos-native +4).
> - [x] New regression test (or extension of `streaming_chunks_reach_speaker_before_phrase_completes`) covers both branches structurally.

Local verification matrix above. Next Claude can either merge PR #136 first (no conflicts on `main` today) or pick a different deferred issue to layer on — but **issue #125 (dispatch2 migration) becomes much cleaner to do on top of PR #136**, since the GCD bindings are the only remaining low-level dispatch primitive after the drain loop is factored.

### Task E.2 / Task E.3 — Close PR #123 follow-up issues #125, #126

Unchanged from prior brief (one item dropped: #124 is closed by PR #136 above):

- **#125** — migrate raw GCD bindings to the `dispatch2` crate. Cleaner now that drain-loop helper is out — the `mod dispatch { extern "C" { ... } }` block in `tts.rs` is the only remaining raw-FFI surface. Touches `tts.rs` only; verify chunk-size assumption post-migration via `examples/tts_macos_pcm_smoke.rs`.
- **#126** — wrap `SynthesisSession::push_text` / `finalize` in `spawn_blocking` at call sites. Currently relies on the `Builder::new_current_thread()` + `NSApplicationMain` invariants. (a) wrap in `spawn_blocking` for future-proofing, or (b) document the contract more explicitly and close as "won't fix; documented". (a) is the safer choice.

Both remain good "single-session code PR" sized.

### Task F — Continue the macos-native-26 plumbing (only if Task A's smoke fails)

Unchanged. Plan at `docs/superpowers/plans/2026-05-20-macos-native-26.md` is the source of truth.

### Carried-forward follow-ups (unchanged this session)

Full list in [`docs/handoffs/2026-05-19T1232+0800-issue-114-stage-b-complete-ready-for-pr.md`](docs/handoffs/2026-05-19T1232+0800-issue-114-stage-b-complete-ready-for-pr.md). Highlights:

- **#98** — split `tests/common/sweep.rs` into `bm25`/`hybrid` submodules. **Defer until Hindi or another third locale lands.**
- **#46** — Hybrid sweep: explore post-RRF `min_score` as a fifth grid axis.
- **#41, #40, #22, #21, #20** — all explicitly deferred per their issue bodies.
- **#129** — wire `--features supertonic` build into CI.
- **#133** — Whisper streaming KV cache re-init per utterance; macos-native-26 sidesteps this entirely.
- **Hindi locale follow-ups** — native-speaker review of `prompts/hi.toml`, Hindi children's-vocabulary corpus, real-LLM smoke, flip-to-stable PR.
- **OpenAI-compat real-server smoke testing**.
- **Klexikon corpus expansion** past 66 articles.
- **Local llama.cpp inference (Phase 1.1)**.
- **Voice-loop hardening** — echo cancellation, ambient-noise robustness.
- **CI validation of `cdn.pyke.io` ort-runtime download** — then flip default features so hybrid retrieval is on by default.
- **Branch-protection-on-main** — repo owner needs to flip a GitHub setting. **Still overdue**; six direct-to-main commits in the past week. The pre-commit hook (`.githooks/pre-commit`) catches fmt drift locally but only fires when devs opt in via `git config core.hooksPath .githooks`. The structural fix is still one GitHub setting flip.

## Open decisions / risks

Newly surfaced this session:

- **PR #136's per-event deadline check is a deliberate semantics change from the original main-thread path** (which checked the deadline once per outer-loop iteration, after draining all available events). The new behaviour is strictly safer for pathological producers; normal-case synthesis is unchanged (30 s budget; real phrases complete in single-digit seconds). Pinned by the description in the PR body; if a future reader sees a "deadline fires while channel still has PhraseEnd queued" report, the fix is to reorder the helper to drain pending events before each deadline check (keeping the helper's other guarantees). No such report observed in 858+47 tests + harness-based macos_tts.rs runs.
- **CLAUDE.md macos-native bullet now mentions a `stream_drain.rs` file that won't exist on the `main` branch until PR #136 merges.** If a future session reads the CLAUDE.md instructions BEFORE PR #136 merges, the file reference will point at a branch-only location. Mitigation: PR #136 is small + green; merging quickly avoids the gap.

Carried forward — still applicable:

- **PR #134 will need a merge conflict resolution** with everything on main since the PR was opened (now includes PR #136 too — see Task B above for the additional CLAUDE.md note).
- **Branch-protection-on-main risk is now demonstrated, not theoretical** — six direct-to-main commits this past week; one PR-CI bypass already manifested as fmt drift. Re-prioritise above deferred-issue queue.

Carried forward from earlier briefs (all still pending verification):

- The mpsc channel in macos-native is unbounded by design — do not "fix" this to be bounded.
- The per-utterance streaming win measured by `--measure-ttfa` is small (~10-20 ms); the real win is per-phrase across multi-phrase responses.
- `tts.rs` is now 671 lines (was 638 in earlier briefs, now 671 because earlier brief was tracking a stale figure; today's count is post-#124). Still over the 500-line guideline. File-split is the next refactor.
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
- CSP regression test reads `tauri.conf.json` via `include_str!` from a hardcoded relative path.
- `SpeechLoopConfig` shape differs across speech builds (3 fields macos-native / 7 fields else; macos-native-26 adds a third shape).
- Open-counter is thread-local, not global.
- `SqliteSessionStore::set_locale` is mutable by reference.

## Patterns to reuse, not reinvent

New from this session:

- **Two-streaming-paths-share-a-loop is a shape that appears in other places in the codebase** (e.g., the voice loop's LISTEN/THINK/SPEAK state machine, the on_audio push loop in `primer-speech`). When the threading-model difference is small and the loop body is the same, extracting a closure-parameterised helper into a sibling submodule (here: `macos/stream_drain.rs`) is cleaner than either a trait-objected strategy pattern OR inline duplication. The closure parameter must return `Result<Option<T>>` so the helper can distinguish "got an event" / "wait-step elapsed, retry" / "structural failure, bail" without leaking the underlying primitive (channel, runloop, semaphore) into the helper's signature.
- **Platform-neutral structural tests for ObjC-adjacent code** can live in `#[cfg(test)] mod tests` inside an `#[cfg(all(target_os = "macos", ...))]`-gated module. The tests compile only on macOS, but they can mock the ObjC-specific dispatch via std::sync::mpsc + std::thread without needing a custom `harness = false` test binary (which the real-AVFoundation tests in `tests/macos_tts.rs` need). Use the custom harness only when you actually need the OS main thread; use a standard `#[cfg(test)]` block when the helper's logic is platform-neutral.

Carried forward (all inherited from prior sessions; see [docs/handoffs/2026-05-19T1232+0800-issue-114-stage-b-complete-ready-for-pr.md](docs/handoffs/2026-05-19T1232+0800-issue-114-stage-b-complete-ready-for-pr.md) for the full list).

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status                       # expect: clean working tree
git fetch --all --prune
gh pr list --state open          # expect: PR #134 DRAFT + PR #136 OPEN
git log --oneline -8             # expect 6accf18 (PR #136), f2a14f9 at top of main once #136 merges

# Opt-in to the local pre-commit hook (one-time per clone):
git config core.hooksPath .githooks

# === Verify default-features build is still green on main ===
cd src
~/.cargo/bin/cargo test --workspace                                    # expect 858/0/3
~/.cargo/bin/cargo fmt --all -- --check                                # expect clean
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings     # expect clean

# === Verify the macOS-native gate (post-#136) ===
~/.cargo/bin/cargo test -p primer-speech --features macos-native       # expect 47/0/3
~/.cargo/bin/cargo test -p primer-cli --features speech,macos-native   # expect 12/0/0
~/.cargo/bin/cargo clippy -p primer-speech --features macos-native --all-targets -- -D warnings   # expect clean

# === If picking up issue #125 (dispatch2 migration) next ===
git checkout main
git pull
git checkout -b claude/issue-125-dispatch2
# Edit src/crates/primer-speech/src/macos/tts.rs: replace `mod dispatch { ... }` block
# with `use dispatch2::...;`. Add `dispatch2 = { version = "...", ... }` to
# primer-speech/Cargo.toml under [target.'cfg(target_os = "macos")'.dependencies].
# Run the verification matrix above. Verify chunk-size assumption via
# `cargo run -p primer-speech --features macos-native --example tts_macos_pcm_smoke`.
```

Carried-forward smokes (unchanged this session):

```bash
# Manual mic round-trip smoke for PR #134 (Horst-driven; the only unchecked box):
cd /Users/hherb/src/primer
git checkout claude/macos-native-26
cd src
~/.cargo/bin/cargo run --features primer-cli/speech,primer-cli/macos-native-26 --bin primer -- \
    --backend stub --speech --language en --name Smoke --age 8 --no-persist

# Hindi preview locale (developer-only):
cd /Users/hherb/src/primer/src
RUST_LOG=primer::prompt_pack=warn,info ~/.cargo/bin/cargo run --bin primer -- \
    --backend stub --name Aarav --age 8 --language hi --no-persist

# OpenAI-compat smoke (spin up a local server first):
llama-server --port 8000 --model /path/to/some.gguf
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --bin primer -- \
    --backend openai-compat --openai-compat-url http://localhost:8000 \
    --model <model-id-from-server> --name SmokeTester --age 9 --no-persist --verbose

# German retrieval-quality regression benchmarks:
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_de
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_hybrid_de
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid_de

# Python ingestion pipeline tests (uv-only — never invoke pip directly):
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If Horst's mic-round-trip smoke for PR #134 passed or failed, capture the outcome in the PR comments (not just locally).
