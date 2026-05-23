# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-23T1106+0800 — small doc-debt session. Closed Tasks C and D from the 2026-05-21 brief: README.md + ROADMAP.md now reflect the macos-native (PR #123) + Supertonic vendor (PRs #127, #128) merges and the in-flight macos-native-26 (PR #134), and the CLAUDE.md `macOS-native speech backend` bullet has been rewritten to describe the Stage B channel-based PCM event stream instead of the pre-Stage-B semaphore path. **No code touched this session.** PR #134 is still DRAFT pending the Horst-driven manual mic round-trip; nothing about that has changed. Three small doc edits + this brief + the handoff archive are the only output. The previous session's archive (`docs/handoffs/2026-05-21T1236+0800-...md`) and the NEXT_SESSION.md rewrite from that session were left uncommitted; both are still on disk and reflect the current state of plans — see [Open decisions / risks](#open-decisions--risks) below if you want to bundle them with this session's commits.

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo`.
2. Check git state:
   ```bash
   cd /Users/hherb/src/primer
   git status                 # expect: 4 modified docs + 2 untracked handoff archives from prior sessions
   git fetch --all --prune
   gh pr list --state open    # expect: PR #134 (claude/macos-native-26) DRAFT
   git log --oneline -8
   ```
   Expect `2f1dc81` at HEAD, then `47c004a`, `2d321a4`, `9a89ac8` (the macos-native-26 scaffolding), then the merge sequence ending at `6591be5`. **No new commits landed since the 2026-05-21 brief.**
3. Decide whether to commit the carried-over docs:
   - `CLAUDE.md`, `README.md`, `ROADMAP.md`, `NEXT_SESSION.md` modifications from this session (2026-05-23) + the prior session's modifications to `NEXT_SESSION.md` (2026-05-21) — all interleaved into a single working tree.
   - Two untracked handoff archives: `2026-05-21T1236+0800-pr123-merged-pr134-draft-cargolock-sync.md` and `2026-05-23T1106+0800-doc-debt-sync-tasks-c-and-d.md`.
   - A single doc-only commit bundling all of them is the cleanest path, since none of the changes touch code and the doc-debt is what the 2026-05-21 brief explicitly asked the next session to do.
4. Read the PR #134 body in full (`gh pr view 134 --json body | jq -r .body`) before touching the macos-native-26 code path — design spec at [docs/superpowers/specs/2026-05-20-macos-native-26-design.md](docs/superpowers/specs/2026-05-20-macos-native-26-design.md), 16-task plan at [docs/superpowers/plans/2026-05-20-macos-native-26.md](docs/superpowers/plans/2026-05-20-macos-native-26.md). PR is **DRAFT pending a single manual mic round-trip** by Horst.

## What we shipped this session

### Task C — README + ROADMAP doc-debt sync (uncommitted)

Three doc edits to [README.md](README.md):

- **Status header sentence** (around line 56): the "Still ahead" phrase now mentions "ongoing hardening of the speech loop (macOS-native Apple-platform path ships now; a macOS 26+ SpeechAnalyzer variant is in flight)" alongside llama.cpp and hardware integration.
- **`primer-speech` directory description** (line 68): tail extended with "macos-native: SFSpeechRecognizer + AVSpeechSynthesizer; macos-native-26: SpeechAnalyzer + AVSpeechSynthesizer, in flight".
- **Speech features list** (around lines 142-156): added a new "Apple-platform alternatives sit behind two mutually-exclusive cargo features" block describing `macos-native` (PR #123, en-US + de-DE, Silero stays as VAD) and `macos-native-26` (PR #134 in flight, SpeechAnalyzer-based, ~100× TTFP / ~2× TTFR win per the PR #131 probe), plus a one-line note on the vendored `supertonic` feature (PRs #127, #128) for A/B evaluation.

One doc edit to [ROADMAP.md](ROADMAP.md):

- **New section `### 2.4 — Native Apple speech (macOS / iOS)`** inserted between 2.3 and the Phase 2 exit criteria. Five bullets: `macos-native` (shipped), `supertonic` (shipped, A/B), SpeechAnalyzer streaming-STT spike (PR #130, shipped), A/B latency probe (PR #131, shipped), `macos-native-26` (PR #134, DRAFT).

### Task D — CLAUDE.md `macOS-native speech backend` bullet rewrite (uncommitted)

The bullet at [CLAUDE.md:168](CLAUDE.md#L168) was describing the pre-Stage-B semaphore + `dispatch_semaphore` machinery, which Stage B (PR #123, merged 2026-05-19) replaced with an unbounded `mpsc::channel` of `SynthesisEvent`s. The producer/consumer mechanism description was rewritten to describe:

- `SynthesisEvent::Audio(chunk)` per PCM-callback fire + `SynthesisEvent::PhraseEnd` on the zero-frame EOS sentinel.
- Main path drains via `try_recv` interleaved with `NSRunLoop::runUntilDate(STREAM_DRAIN_POLL_MS)`; background path uses `recv_timeout(STREAM_DRAIN_POLL_MS)`.
- `STREAM_DRAIN_POLL_MS = 10` lives in `primer_core::consts::speech`.
- Channel **must** stay unbounded — a bounded `sync_channel` that filled up would deadlock the GCD main queue (the AVFoundation contract is "never block" there). Per-phrase memory footprint capped at ~100 events × ~1 KB ≈ ~100 KB.
- Structural invariant pinned by `streaming_emits_multiple_audio_events_before_phrase_end` in `tests/macos_tts.rs`.
- Per-phrase TTFA across multi-phrase responses pinned by `streaming_chunks_reach_speaker_before_phrase_completes` in `voice_loop::state_machine` (`TimedMockTts` emits 3 marker-valued chunks at 50 ms wallclock intervals).

The runtime-on-main-thread rationale paragraph was preserved verbatim — it's the load-bearing part. A trailing sentence was added noting `tts.rs` is 638 lines (>500-line guideline) and that the deferred file-split + drain-helper extraction is tracked in issues #124 / #125 / #126.

### Tasks not addressed this session

- **Task A** — Horst-driven manual mic round-trip smoke for PR #134. Not Claude-actionable.
- **Task B** — PR #134 rebase + flip out of DRAFT. Depends on Task A.
- **Task E** — close PR #123 follow-up issues #124, #125, #126. Deferred; would warrant a separate code PR.
- **Task F** — continue the macos-native-26 plumbing per the 16-task plan. Only relevant if Task A's smoke fails.

## What's next — by priority

### Task A — Run the PR #134 manual mic round-trip smoke (Horst-driven)

Unchanged from the previous brief. The single unchecked box on PR #134's test plan:

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

If the smoke fails, attach log output to PR #134 as a comment. Common gotchas the PR body calls out: the `AsyncStreamBuffer.swift:508` fatal-error fix in commit `69c04df`, the `Library not loaded: @rpath/libswift_Concurrency.dylib` rpath workaround in `primer-speech/build.rs` / `primer-cli/build.rs` / `primer-gui/build.rs`, and silent de-DE asset download on first use.

### Task B — Resolve the PR #134 merge conflicts and flip out of DRAFT

Unchanged. PR #134 has merge conflicts with the 3 scaffolding commits now on `main` (`9a89ac8`, `2d321a4`, `47c004a`) plus the Cargo.lock sync (`2f1dc81`). The PR-branch versions of the 5 conflicted files (`primer-speech/Cargo.toml`, `primer-speech/src/lib.rs`, `macos26/mod.rs`, `macos26/audio_session.rs`, `tests/macos26_smoke.rs`) are strict supersets of what's on main — uniform resolution rule: take PR-branch version. Either rebase locally:

```bash
git checkout claude/macos-native-26
git fetch origin
git rebase origin/main
# Resolve conflicts: take PR-branch (ours, in rebase context) on every file.
git push --force-with-lease origin claude/macos-native-26
gh pr ready 134
```

…or let GitHub's "resolve conflicts" UI handle it post-mic-smoke.

### Task E — Close PR #123 follow-up issues #124, #125, #126 (deferred from previous brief)

Unchanged. All three are still untouched:

- **#124** — factor shared drain-loop helper between main-thread and background streaming paths in `crates/primer-speech/src/macos/tts.rs`. The two `synthesize_streaming_*` functions duplicate the drain loop (one uses `try_recv` + `runUntilDate(10ms)`, the other uses `recv_timeout(STREAM_DRAIN_POLL_MS)`). Likely <100 LOC to extract `drain_pcm_events<F>(rx, on_event, on_yield)`. This is the natural first step toward the `tts.rs` file split (the file is 638 lines, over the 500-line guideline).
- **#125** — migrate raw GCD bindings to the `dispatch2` crate. The `dispatch_async_f` + `_dispatch_main_q` FFI block currently lives inline in `tts.rs`. Touches `tts.rs` only; verify the chunk-size assumption post-migration via `examples/tts_macos_pcm_smoke.rs`.
- **#126** — wrap `SynthesisSession::push_text` / `finalize` in `spawn_blocking` at call sites. Currently relies on the `Builder::new_current_thread()` + `NSApplicationMain` invariants documented in the rewritten CLAUDE.md bullet. (a) wrap in `spawn_blocking` for future-proofing, or (b) document the contract more explicitly and close as "won't fix; documented". (a) is the safer choice.

### Task F — Continue the macos-native-26 plumbing (only if Task A's smoke fails)

Unchanged. Plan at `docs/superpowers/plans/2026-05-20-macos-native-26.md` is the source of truth; tasks 1-16 are all marked done on the branch — only the manual-smoke gate is holding the PR in DRAFT. Post-merge follow-ups (out of scope for #134 per the PR body): partial-streaming visibility through the `swift-bridge` boundary, `speech` umbrella feature slimming under `macos-native-26`, iOS Tauri config + `macos26/` → `apple26/` rename.

### Carried-forward follow-ups (unchanged from previous brief)

The full list is in [`docs/handoffs/2026-05-19T1232+0800-issue-114-stage-b-complete-ready-for-pr.md`](docs/handoffs/2026-05-19T1232+0800-issue-114-stage-b-complete-ready-for-pr.md). Highlights:

- **#98** — split `tests/common/sweep.rs` into `bm25`/`hybrid` submodules. **Defer until Hindi or another third locale lands.**
- **#46** — Hybrid sweep: explore post-RRF `min_score` as a fifth grid axis.
- **#41, #40, #22, #21, #20** — all explicitly deferred per their issue bodies.
- **#129** — wire `--features supertonic` build into CI (opened from PR #127's review).
- **#133** — Whisper streaming re-initialises KV cache on every utterance; the macos-native-26 path sidesteps this entirely by replacing Whisper.
- **Hindi locale follow-ups** — native-speaker review of `prompts/hi.toml`, Hindi children's-vocabulary corpus, real-LLM smoke, flip-to-stable PR.
- **OpenAI-compat real-server smoke testing** — spin up oMLX / LM Studio / vLLM, run `--backend openai-compat --openai-compat-url http://localhost:8000`, confirm SSE streaming + error classification + embedder round-trip.
- **Klexikon corpus expansion** past 66 articles to close the 2 corpus gaps (`gänsehaut` reflex; tides on `mond`).
- **Local llama.cpp inference (Phase 1.1)** — `LlamaCppBackend` stub remains the entry point.
- **Voice-loop hardening** — echo cancellation, ambient-noise robustness. PR #134 is the next iteration of the LISTEN-side hardening.
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default.
- **Branch-protection-on-main** — repo owner needs to flip a GitHub setting: Settings → Branches → require `cargo test (default features)` check on `main`. **Bumped in priority** by the recent direct-to-main commits (3 scaffolding + 1 Cargo.lock sync this past week).

## Open decisions / risks

Newly surfaced this session:

- **Two uncommitted handoff archives + two sessions of NEXT_SESSION.md edits stack in the working tree.** This is fine — neither the 2026-05-21 brief nor this session's brief depends on the other being committed first. The two NEXT_SESSION.md edits compose cleanly because this session's edit replaces the file content wholesale (Write tool semantics), so the prior session's intermediate state isn't lost — it's preserved in `docs/handoffs/2026-05-21T1236+0800-...md`. Recommendation: bundle CLAUDE.md + README.md + ROADMAP.md + NEXT_SESSION.md + both handoff archives into a single `docs(handoff): catch up doc-debt for macos-native Stage B and macos-native-26 in-flight` commit.

Carried forward — all still applicable:

- **`origin/main` `cargo build --frozen` regression window between `9a89ac8` and `2f1dc81`.** Not a runtime issue; the lock file was just briefly behind the Cargo.toml. Bisects across that 4-commit window will fail `--frozen` builds.
- **PR #134 will need a merge conflict resolution** with the 3 scaffolding commits now on main + the Cargo.lock sync. See Task B; resolution rule is uniform (take PR-branch version).
- **CLAUDE.md may need a parallel `macos-native-26` bullet added** once PR #134 lands — PR #134's `802fa87` commit on the branch already adds one. Be careful not to mention "PCM callback → channel" patterns in a way that conflates the two — `macos-native-26` uses `swift-bridge`'s async iterator (`nextResult`) not a channel, and its synchronisation primitive is a single-flighted Swift Task.
- **Direct-to-main commits happened twice the week before this session** — Horst pushed 3 scaffolding commits during the 2026-05-21 session, and that session's `2f1dc81` Cargo.lock fix also went direct-to-main. Branch protection on `main` would force these through PRs + CI build. Re-prioritise the branch-protection item.

Carried forward from earlier briefs (all still pending verification):

- The mpsc channel in macos-native is unbounded by design — do not "fix" this to be bounded (deadlock risk on the GCD main queue; see the rewritten CLAUDE.md bullet for the full rationale).
- The per-utterance streaming win measured by `--measure-ttfa` is small (~10-20 ms); the real user-facing win is per-phrase across multi-phrase responses (pinned by the state-machine TTFA test).
- `tts.rs` is 638 lines (over the 500-line guideline). Task E above + issue #124 are the path here.
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
- `SpeechLoopConfig` shape differs across speech builds (3 fields macos-native / 7 fields else; now 3 shapes with macos-native-26 in flight).
- Open-counter is thread-local, not global.
- `SqliteSessionStore::set_locale` is mutable by reference.

## Patterns to reuse, not reinvent

(All inherited from prior sessions; see [docs/handoffs/2026-05-19T1232+0800-issue-114-stage-b-complete-ready-for-pr.md](docs/handoffs/2026-05-19T1232+0800-issue-114-stage-b-complete-ready-for-pr.md) for the full list.)

New from this session:

- **Doc-debt-only sessions are a legitimate sized-down session shape.** This session shipped exactly three doc edits + a handoff brief, no code touched. The 2026-05-21 brief explicitly flagged Tasks C + D as "small, can ride on the next code PR" but they were also doable as a standalone session; running them on their own keeps them from blocking on a code task that wants to ship. Trade-off: the resulting commit is doc-only, so CI runs nothing meaningful — verification is "the markdown still renders" + "no broken links".
- **When rewriting a CLAUDE.md bullet, cross-reference the test that pins the invariant you describe.** The rewritten macos-native bullet names two tests by their function names (`streaming_emits_multiple_audio_events_before_phrase_end` and `streaming_chunks_reach_speaker_before_phrase_completes`) and which file they live in. A future Claude looking at the bullet can grep those test names and find the live source of truth instead of taking the prose at face value.
- **The 2026-05-21 brief's "what landed since" table is the canonical hand-off shape for cross-session bookkeeping.** Reproducing it (PR # | title | branch | merged date) at the top of the brief is far more useful than prose like "six PRs landed" — future Claude can use it directly without re-querying `gh pr list`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status                       # expect: 4 modified docs + 2 untracked handoff archives
git fetch --all --prune
gh pr list --state open          # expect: PR #134 DRAFT on claude/macos-native-26

# Opt-in to the local pre-commit hook (one-time per clone):
git config core.hooksPath .githooks

# === Verify default-features build is still green on main ===
cd src
~/.cargo/bin/cargo test --workspace                                    # expect 858+/0/3+
~/.cargo/bin/cargo fmt --all -- --check
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings

# === If you want to commit this session's doc-debt + the carried-over handoff ===
cd /Users/hherb/src/primer
git add CLAUDE.md README.md ROADMAP.md NEXT_SESSION.md \
        docs/handoffs/2026-05-21T1236+0800-pr123-merged-pr134-draft-cargolock-sync.md \
        docs/handoffs/2026-05-23T1106+0800-doc-debt-sync-tasks-c-and-d.md
# (use a HEREDOC commit message; see the commit-push skill if invoked.)

# === Manual mic round-trip smoke for PR #134 (Horst-driven; the only unchecked box) ===
git checkout claude/macos-native-26
cd src
~/.cargo/bin/cargo run --features primer-cli/speech,primer-cli/macos-native-26 --bin primer -- \
    --backend stub --speech --language en --name Smoke --age 8 --no-persist
# Speak: "what colour is the sky"
# Expected: streaming partials → stub Primer response → spoken via AVSpeechSynthesizer
# Exit: type/say "bye" or "exit"

# === If smoke passes: rebase PR #134 onto main and push ===
git rebase origin/main
# Resolve conflicts on the 5 files: Cargo.toml, lib.rs, macos26/mod.rs,
# macos26/audio_session.rs, tests/macos26_smoke.rs. Resolution rule: take PR-branch version.
git push --force-with-lease origin claude/macos-native-26
gh pr ready 134

# === Verify post-merge main state is still green (Linux + macOS) ===
git checkout main
cd src
~/.cargo/bin/cargo test --workspace                                    # expect 858+/0/3+
~/.cargo/bin/cargo test -p primer-speech --features voice-loop         # expect 86+/0/2+
~/.cargo/bin/cargo test -p primer-cli --features speech                # expect 12+/0/0
~/.cargo/bin/cargo test -p primer-cli --features speech,macos-native   # expect 12+/0/0 (macOS only)
~/.cargo/bin/cargo test -p primer-gui --features speech                # expect 146+/0/0
```

Carried-forward smokes (unchanged this session):

```bash
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
