# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-23T1444+0800 — second sync session of the day. **No code or docs changed this session.** Everything from the prior brief (2026-05-23T1106) was committed as `9a3e293 docs(handoff): catch up doc-debt for macos-native Stage B and macos-native-26 in-flight`. Since then Horst landed two further direct-to-main commits — `ab6d21d` (cargo fmt --all cleanup across 7 files: 3 vendored `supertonic-rs` files imported in PR #127 with upstream formatting, plus the 4 macos26 scaffolding files that bypassed PR CI) and `16b8f7c` (lock-file-only Tauri 2.11.1 → 2.11.2 patch bump, build smoke confirmed clean). Both are mechanical hygiene; neither alters user-facing surface. This session's only output was: (a) `cargo test --workspace` re-verified at **858/0/3 — green**, (b) `cargo fmt --all -- --check` re-verified clean, and (c) this brief + its handoff archive. PR #134 remains DRAFT pending Horst's manual mic round-trip; nothing about that has changed.

## What landed since the previous brief

| SHA       | Title                                                                            | Date              | Notes                                                                                                              |
| --------- | -------------------------------------------------------------------------------- | ----------------- | ------------------------------------------------------------------------------------------------------------------ |
| `9a3e293` | `docs(handoff): catch up doc-debt for macos-native Stage B and macos-native-26 in-flight` | 2026-05-23 ~11:10 | The previous session's brief + the handoff archive + the CLAUDE.md / README.md / ROADMAP.md / NEXT_SESSION.md doc-debt edits, bundled into a single doc-only commit as that brief recommended. |
| `ab6d21d` | `style: cargo fmt --all to clear pre-existing drift on main`                     | 2026-05-23 11:14 | Pure rustfmt cleanup of 7 files: 3 in `src/vendor/supertonic-rs/` (PR #127 vendor import, kept upstream formatting), 4 in `src/crates/primer-speech/src/macos26/` + the supertonic example (came in via Horst's direct-to-main scaffolding commits 9a89ac8..47c004a, which bypassed PR CI). No semantic changes. |
| `16b8f7c` | `chore(deps): bump tauri family 2.11.1 -> 2.11.2`                                | 2026-05-23 11:24 | Lock-file-only across 7 crates in the tauri family. Workspace pin in `src/Cargo.toml` is already `"2"`; `cargo update -p tauri --precise 2.11.2` was sufficient. `cargo build -p primer-gui --features speech` smoke ran clean in ~1m09s on macOS. Does NOT shift the `gtk 0.18.2` → `glib 0.18.5` floor that Dependabot alert #2 needs — that requires Tauri 3+. |

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo`.
2. Check git state:
   ```bash
   cd /Users/hherb/src/primer
   git status                 # expect: clean working tree
   git fetch --all --prune
   gh pr list --state open    # expect: PR #134 (claude/macos-native-26) DRAFT
   git log --oneline -8
   ```
   Expect `16b8f7c` at HEAD, then `ab6d21d`, `9a3e293`, `2f1dc81`, `47c004a`, `2d321a4`, `9a89ac8`, `6591be5`.
3. Read the PR #134 body in full (`gh pr view 134 --json body | jq -r .body`) before touching the macos-native-26 code path — design spec at [docs/superpowers/specs/2026-05-20-macos-native-26-design.md](docs/superpowers/specs/2026-05-20-macos-native-26-design.md), 16-task plan at [docs/superpowers/plans/2026-05-20-macos-native-26.md](docs/superpowers/plans/2026-05-20-macos-native-26.md). PR is **DRAFT pending a single manual mic round-trip** by Horst.

## What we shipped this session

Nothing. This was a status-sync session.

The auto-mode workflow ran through:

1. Read the 2026-05-23T1106 brief — confirmed all its Task C / D edits were on disk and committed as `9a3e293`.
2. `git status` — clean. `git log` — two new commits (`ab6d21d`, `16b8f7c`) since the prior brief, both direct-to-main, both mechanical.
3. `~/.cargo/bin/cargo fmt --all -- --check` from `src/` — clean (validates `ab6d21d` did the job, and that no further drift slipped in).
4. `~/.cargo/bin/cargo test --workspace` from `src/` — **858 passed / 0 failed / 3 ignored**, matching the prior brief's expectation exactly.
5. Decision point: Task A is Horst-driven, Task B depends on Task A, Tasks C / D are done, Task E ("close PR #123 follow-up issues #124, #125, #126") is explicitly marked deferred — "would warrant a separate code PR" — and Task F is conditional on Task A's smoke failing. No Claude-actionable code work remained.
6. README.md / ROADMAP.md scan — neither needs an edit. The two new commits are pure mechanical hygiene with no user-facing surface change.

### Tasks not addressed this session

Same as the previous brief — none of the priority blockers shifted:

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

Unchanged. PR #134 has merge conflicts with the 3 scaffolding commits now on `main` (`9a89ac8`, `2d321a4`, `47c004a`) plus the Cargo.lock sync (`2f1dc81`), AND now the fmt cleanup (`ab6d21d`) and tauri patch bump (`16b8f7c`) layered on top. The PR-branch versions of the 5 originally-conflicted files (`primer-speech/Cargo.toml`, `primer-speech/src/lib.rs`, `macos26/mod.rs`, `macos26/audio_session.rs`, `tests/macos26_smoke.rs`) are strict supersets of what's on main; the resolution rule remains "take PR-branch version" for those five. `Cargo.lock` will need careful merging (regen via `cargo generate-lockfile` after rebase is the cleanest path). The fmt cleanup may have touched files the PR branch also modified — if so, re-running `cargo fmt --all` on the PR branch after rebase will produce the right state.

```bash
git checkout claude/macos-native-26
git fetch origin
git rebase origin/main
# Resolve conflicts: take PR-branch (ours, in rebase context) on the original 5 files;
# regen Cargo.lock; re-run cargo fmt --all to absorb ab6d21d if needed.
git push --force-with-lease origin claude/macos-native-26
gh pr ready 134
```

…or let GitHub's "resolve conflicts" UI handle it post-mic-smoke.

### Task E — Close PR #123 follow-up issues #124, #125, #126 (still deferred)

Unchanged. All three are still open and untouched:

- **#124** — factor shared drain-loop helper between main-thread and background streaming paths in `crates/primer-speech/src/macos/tts.rs`. The two `synthesize_streaming_*` functions duplicate utterance construction, callback closure, the `writeUtterance_toBufferCallback` invocation, and the deadline / PhraseEnd / wait-step branching. The threading-model difference (`try_recv` + `runUntilDate` vs `recv_timeout`) absorbs cleanly into a `wait_step` closure passed to a shared `drive_streaming_drain` helper. ~100 LOC extraction; natural first step toward the `tts.rs` file split (638 lines, over the 500-line guideline).
- **#125** — migrate raw GCD bindings to the `dispatch2` crate. The `dispatch_async_f` + `_dispatch_main_q` FFI block currently lives inline in `tts.rs`. Touches `tts.rs` only; verify the chunk-size assumption post-migration via `examples/tts_macos_pcm_smoke.rs`.
- **#126** — wrap `SynthesisSession::push_text` / `finalize` in `spawn_blocking` at call sites. Currently relies on the `Builder::new_current_thread()` + `NSApplicationMain` invariants documented in CLAUDE.md. (a) wrap in `spawn_blocking` for future-proofing, or (b) document the contract more explicitly and close as "won't fix; documented". (a) is the safer choice.

These are good "single-session code PR" sized — pick the one that lines up with the next bit of speech-stack work you have on the table, do it in isolation, ship a small focused PR. Issue #124 is the natural first one (the helper extraction is what lets the file split in #124 land cleanly).

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
- **Branch-protection-on-main** — repo owner needs to flip a GitHub setting: Settings → Branches → require `cargo test (default features)` check on `main`. **Bumped up sharply now — six direct-to-main commits in the past week** (3 macos26 scaffolding 9a89ac8..47c004a, the Cargo.lock sync `2f1dc81`, the fmt cleanup `ab6d21d`, the tauri bump `16b8f7c`). Two of the three direct-to-main pathologies the previous brief warned about have already played out: PR CI did not run on the scaffolding (caught by `ab6d21d`), and a `--frozen` build regression window existed on `main` between `9a89ac8` and `2f1dc81`. The structural fix is one GitHub setting flip; until that lands every contributor (Horst and Claude alike) needs hand-discipline to route through PRs.

## Open decisions / risks

Newly surfaced this session: none. The two new direct-to-main commits are mechanical hygiene; nothing in the codebase has changed in a way that would invalidate prior decisions.

Carried forward — all still applicable:

- **PR #134 will need a merge conflict resolution** with the 6 commits now on main since the PR was opened. See Task B; resolution rule is uniform on the original 5 PR-changed files (take PR-branch version), with Cargo.lock regenerated and a fresh `cargo fmt --all` to absorb any fmt drift the PR branch introduced.
- **CLAUDE.md may need a parallel `macos-native-26` bullet added** once PR #134 lands — PR #134's `802fa87` commit on the branch already adds one. Be careful not to mention "PCM callback → channel" patterns in a way that conflates the two — `macos-native-26` uses `swift-bridge`'s async iterator (`nextResult`) not a channel, and its synchronisation primitive is a single-flighted Swift Task.
- **Branch-protection-on-main risk is now demonstrated, not theoretical.** Six direct-to-main commits in a week, one PR-CI bypass already manifested as fmt drift on main (cleaned up by `ab6d21d`). Re-prioritise this above the deferred-issue queue.

Carried forward from earlier briefs (all still pending verification):

- The mpsc channel in macos-native is unbounded by design — do not "fix" this to be bounded (deadlock risk on the GCD main queue; see the CLAUDE.md `macos-native speech backend` bullet for the full rationale).
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

- **Status-sync-only sessions are a legitimate sized-down session shape, and the brief should say so explicitly.** Earlier this same day a doc-debt session ran on Tasks C + D; this session ran on… nothing actionable, because all priority tasks were either Horst-driven, dependency-blocked, or explicitly deferred. The right output of such a session is: run the workspace health checks the previous brief recommended (`cargo fmt --check`, `cargo test --workspace`), record their outcomes verbatim in the new brief so the next session has a fresh "last-known-green" anchor, and produce a handoff archive. Don't manufacture work because the session needs to "ship something". The verification-and-record contribution IS the shipping.
- **Direct-to-main commits in the "what landed since" table are useful even when they're mechanical.** The previous brief's table-format pattern surfaced two direct-to-main commits this session that would otherwise have been invisible to the next Claude (`ab6d21d`, `16b8f7c`); having them in the table reminds future-Claude that they need to be reconciled with any in-flight PR branches and that branch-protection is overdue, without anyone having to re-query `gh`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status                       # expect: clean working tree
git fetch --all --prune
gh pr list --state open          # expect: PR #134 DRAFT on claude/macos-native-26

# Opt-in to the local pre-commit hook (one-time per clone):
git config core.hooksPath .githooks

# === Verify default-features build is still green on main ===
cd src
~/.cargo/bin/cargo test --workspace                                    # expect 858/0/3
~/.cargo/bin/cargo fmt --all -- --check                                # expect clean
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings     # expect clean

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
# Resolve conflicts on the 5 PR-changed files: Cargo.toml, lib.rs, macos26/mod.rs,
# macos26/audio_session.rs, tests/macos26_smoke.rs. Resolution rule: take PR-branch version.
# Regen Cargo.lock; re-run cargo fmt --all to absorb ab6d21d drift if it appears.
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
