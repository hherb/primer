# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-21T1236+0800 — handoff-cleanup session with one small drive-by commit. Confirmed that the previous brief's **Task A is complete**: PR #123 ("speech(macos): stream PCM chunks to speaker as AVSpeechSynthesizer emits them") merged on 2026-05-19. Six PRs landed on `main` since the previous brief (#123 Stage B, #127 Supertonic vendor, #128 Supertonic build fix, #130 macOS 26 SpeechAnalyzer streaming-STT spike, #131 macOS 26 vs Whisper A/B latency probe, #132 openssl dep bump). One new PR is open as **DRAFT — PR #134 "speech(macos): macos-native-26 — SpeechAnalyzer STT/VAD + Swift sidecar"** on branch `claude/macos-native-26`. Horst (working in parallel during this session) **directly committed three macos-native-26 scaffolding commits to `main`** — `9a89ac8`, `2d321a4`, `47c004a`. These were pushed mid-session; they are legitimate main-branch history, not stale. This session's drive-by commit `2f1dc81` (chore(deps): sync Cargo.lock with macos-native-26 swift-bridge dep) reconciles the Cargo.lock that the 3 scaffolding commits had left out of sync. Three small follow-up issues from PR #123's review (#124, #125, #126) are still untouched, plus the new #129 (Supertonic CI) and #133 (Whisper KV-cache re-init).

## First moves when you start

1. Read [CLAUDE.md](CLAUDE.md) — repo conventions, gotchas, build commands. **Workspace root is `src/`, not the repo root.** Every cargo command runs from `src/`. Always invoke as `~/.cargo/bin/cargo`.
2. Check git state:
   ```bash
   cd /Users/hherb/src/primer
   git status                 # expect: clean tree, on main, up-to-date with origin/main
   git fetch --all --prune
   gh pr list --state open    # expect: PR #134 (claude/macos-native-26) DRAFT
   git log --oneline -8
   ```
   Expect `2f1dc81` at HEAD (the Cargo.lock sync from this session) then `47c004a`, `2d321a4`, `9a89ac8` (Horst's macos-native-26 scaffolding pushed during this session) then the PR-merge sequence ending at `6591be5`.
3. Verify the in-flight PR branch is green on macOS:
   ```bash
   git checkout claude/macos-native-26
   cd src
   ~/.cargo/bin/cargo build --features primer-speech/macos-native-26                # macOS only
   ~/.cargo/bin/cargo test -p primer-speech --features macos-native-26 --lib        # vad + locale unit tests
   ~/.cargo/bin/cargo test -p primer-speech --features macos-native-26 --test macos26_smoke -- --ignored
   ~/.cargo/bin/cargo build --features primer-cli/speech                            # no macos-native-26
   ~/.cargo/bin/cargo build --features primer-cli/speech,primer-cli/macos-native    # the merged path
   ~/.cargo/bin/cargo build --features primer-cli/speech,primer-cli/macos-native-26 # the in-flight path
   ```
4. Read the PR #134 body in full (`gh pr view 134 --json body | jq -r .body`) before touching the macos-native-26 code path — the design spec at [docs/superpowers/specs/2026-05-20-macos-native-26-design.md](docs/superpowers/specs/2026-05-20-macos-native-26-design.md), the 16-task plan at [docs/superpowers/plans/2026-05-20-macos-native-26.md](docs/superpowers/plans/2026-05-20-macos-native-26.md), and the recent CLAUDE.md note (commit `802fa87`) are the source of truth. The PR is **DRAFT pending a single manual mic round-trip** by Horst — the rest of the test plan is green. Horst stated this session that they will run the smoke themselves in a follow-up session.

## What we shipped this session

### Drive-by Cargo.lock sync (commit `2f1dc81`, pushed to `origin/main`)

The three macos-native-26 scaffolding commits Horst pushed during this session (`9a89ac8`, `2d321a4`, `47c004a`) added `swift-bridge` as an optional dependency on `primer-speech` via the new `macos-native-26` cargo feature, but did not update `src/Cargo.lock`. Cargo lock-files include the full resolved dependency graph (optional features included), so the pre-fix state would have failed `cargo build --frozen` for anyone cloning fresh. The 48-line lock-file addition adds `swift-bridge`, `swift-bridge-build`, `swift-bridge-ir`, `swift-bridge-macro`, and their transitive deps to the graph.

This was the entire scope of the session's code changes. No other files touched.

### Session-meta deliverable

This `NEXT_SESSION.md` rewrite + archive. The first draft of this brief (in the same session, ~09:56+0800) incorrectly framed the 3 scaffolding commits as "stale duplicates of PR #134 work" and proposed a `git reset --hard origin/main` to drop them. **That framing was wrong** — at the moment that draft was written, the commits were local-only and ahead of `origin/main`, but Horst pushed them to `origin/main` during the session via a separate terminal, making them part of official main history. The revised brief here reflects that correctly. The wrong first-draft archive at `docs/handoffs/2026-05-21T0956+0800-...-cleanup-pending.md` was deleted before this revised brief was archived.

## What landed between the previous brief and now (verified via `gh pr list --state all`)

| PR  | Title                                                                        | Branch                                          | Merged           |
|-----|------------------------------------------------------------------------------|-------------------------------------------------|------------------|
| 123 | speech(macos): stream PCM chunks to speaker (closes #114)                    | `speech/macos-native-pcm-streaming-issue-114`   | 2026-05-19       |
| 127 | Vendor Supertonic TTS with ort rc.10 migration & smoke test                  | `claude/evaluate-supertonic-tts-fOKNd`          | 2026-05-19       |
| 128 | speech: fix supertonic vendor crate build (follow-up to #127)                | `claude/fix-supertonic-vendor-build`            | 2026-05-19       |
| 130 | speech(macos): add macOS 26 SpeechAnalyzer streaming-STT spike               | `claude/wizardly-shtern-bb8824`                 | 2026-05-20       |
| 131 | speech(macos): A/B latency probe — macOS 26 vs Whisper                      | `claude/macos-speech-ab-probe`                  | 2026-05-20       |
| 132 | chore(deps): bump openssl 0.10.79 → 0.10.80 (dependabot)                    | `dependabot/cargo/src/cargo-b5bfc02d2b`         | 2026-05-20       |
| **134** | **speech(macos): macos-native-26 — SpeechAnalyzer STT/VAD + Swift sidecar** | `claude/macos-native-26`                    | **DRAFT — open** |

Plus three direct-to-main scaffolding commits pushed by Horst during this session:

| Commit  | Subject                                                            |
|---------|--------------------------------------------------------------------|
| 9a89ac8 | speech(macos26): scaffold macos-native-26 feature + mutex gate     |
| 2d321a4 | speech(macos26): audio_session cfg-split (macOS no-op, iOS stub)   |
| 47c004a | speech(macos26): ignored integration smoke tests                   |
| 2f1dc81 | **chore(deps): sync Cargo.lock with macos-native-26 swift-bridge dep** (this session) |

These three scaffolding commits are partial state — Cargo feature, mutual-exclusion `compile_error!`, the empty `macos26/` module dir, `audio_session.rs` (cfg-split no-op for macOS, stub for iOS), and the `tests/macos26_smoke.rs` smoke harness. They build cleanly under any combination of `macos-native` and `macos-native-26` features (the smoke binary is `#[ignore]`'d so it doesn't fail in default CI without macOS 26.5 + mic). The remaining macos-native-26 implementation (Swift sidecar, swift-bridge module, DerivedVadStateMachine, consumer loop, STT shim, builder, CLI/GUI feature propagation, rpath fix) lives on the PR #134 branch and is 22 commits ahead of `main`.

**This means PR #134 will have merge conflicts** with the now-on-main scaffolding, because both branches independently introduce:

- `src/crates/primer-speech/Cargo.toml` `macos-native-26` feature
- `src/crates/primer-speech/src/lib.rs` mutual-exclusion `compile_error!`
- `src/crates/primer-speech/src/macos26/mod.rs`
- `src/crates/primer-speech/src/macos26/audio_session.rs`
- `src/crates/primer-speech/tests/macos26_smoke.rs`

The PR-branch versions are strict supersets of the on-main versions, so conflict resolution is straightforward — keep the PR-branch version everywhere. Recommended approach: rebase `claude/macos-native-26` onto `main` (resolving conflicts in favour of the PR branch) before flipping out of DRAFT. Alternative: GitHub's "resolve conflicts" UI also works since the conflict shape is uniformly "take PR-branch version."

PR #131's A/B probe established the empirical premise behind PR #134: **SpeechAnalyzer is ~100× faster to first partial (~30 ms vs ~3.8 s) and ~2× faster to final (~800 ms vs ~1.8 s) than Whisper `ggml-small.en`** on macOS 26.5.

PR #127 + #128 vendor Supertonic TTS behind a `supertonic` feature — not yet on a primary code path, but available as an A/B option alongside Piper and AVSpeechSynthesizer.

## What's next — by priority

### Task A — Run the PR #134 manual mic round-trip smoke (Horst-driven)

Horst stated this session: "I will run the mic roundtrip smoke test in a follow up session myself." This is the only unchecked box on PR #134's test plan:

> - [ ] **Manual mic round-trip** — speak "what colour is the sky" into the mic, verify streaming partials arrive, verify the stub Primer responds, verify quit/bye exits cleanly. **Pending the human reviewer**; that's why this PR is a draft.

```bash
cd /Users/hherb/src/primer
git checkout claude/macos-native-26
# Optional: rebase onto main to surface the 3 scaffolding-commit conflicts now
# instead of at merge time. The PR-branch version is a strict superset of the
# on-main version everywhere, so conflict resolution is uniform: take PR-branch.
# git rebase origin/main
cd src
~/.cargo/bin/cargo build --features primer-cli/speech,primer-cli/macos-native-26 --bin primer
~/.cargo/bin/cargo run --features primer-cli/speech,primer-cli/macos-native-26 --bin primer -- \
    --backend stub --speech --language en --name Smoke --age 8 --no-persist
# Expected: Terminal mic TCC prompt on first run (one-time per binary identity).
# Speak: "what colour is the sky"
# Expected: streaming partials print on stderr, stub Primer responds with a canned line,
# spoken via AVSpeechSynthesizer. Quit with "bye" or "exit".
```

Common gotchas the PR body calls out:

- **`AsyncStreamBuffer.swift:508` fatal error** — was the bug that the C2 fix in commit `69c04df` closed. Should NOT recur post-fix; if it does, the inner `defer` in `Macos26PipelineImpl.swift::nextResult` is the focal point.
- **`Library not loaded: @rpath/libswift_Concurrency.dylib`** — was the C1 bug. Both `primer-speech/build.rs`, `primer-cli/build.rs`, and `primer-gui/build.rs` now emit `-Wl,-rpath,/usr/lib/swift`. If you see this error from a brand-new binary crate, mirror that build.rs change there.
- **de-DE asset download** — silent on first use (Apple's OS-managed flow). en-US ships with macOS 26 (no download).

If the smoke fails, attach log output to PR #134 as a comment.

### Task B — Resolve the PR #134 merge conflicts and flip out of DRAFT

After the smoke passes, PR #134 needs to either (a) be rebased onto `main` so the merge is a fast-forward, or (b) the merge done through GitHub's UI which will surface the 5 conflicted files. In either case the resolution rule is uniform: take the PR-branch version for every conflict, since the PR-branch versions of `Cargo.toml`, `lib.rs`, `macos26/mod.rs`, `macos26/audio_session.rs`, and `tests/macos26_smoke.rs` are all strict supersets of what's on main.

```bash
cd /Users/hherb/src/primer
git checkout claude/macos-native-26
git fetch origin
git rebase origin/main
# Resolve conflicts: for each file, accept the PR-branch (ours, in rebase context) version.
# Then push:
# git push --force-with-lease origin claude/macos-native-26
# Mark Ready for Review on GitHub, request reviewers, merge.
```

`--force-with-lease` (not `--force`) is what we want — protects against accidentally clobbering changes someone else pushed since the local rebase.

### Task C — README + ROADMAP doc-debt sync (small, can ride on the next code PR)

[README.md](README.md) hasn't been updated for the merges from 2026-05-19 onward. Specifically:

- **Speech features list** (around lines 142-151): mentions Silero, Whisper, Piper. Should add a one-line note that an experimental `macos-native` Apple-platform alternative ships now (PR #123 finalised the PCM-streaming path), and call out that PR #134 is in flight to add a `macos-native-26` Apple-platform alternative that uses SpeechAnalyzer on macOS 26+. Supertonic vendoring (PR #127/#128) is available as an opt-in `supertonic` feature; mention or omit at your discretion since it isn't on a default path.
- **`primer-speech` directory description** (line 68): currently says "VAD + STT + TTS backends (Silero, Whisper, Piper, cpal)". Add "macos-native (SFSpeechRecognizer + AVSpeechSynthesizer), macos-native-26 (SpeechAnalyzer + AVSpeechSynthesizer, in flight)" to that one-liner.
- **Status header** (line 56, "Phase 0.2 and Phase 0.3 are both complete"): the "Still ahead" sentence lists "local llama.cpp inference, hardening of the speech loop, hardware integration." Phase 1 (llama.cpp) and Phase 3 (hardware) status hasn't changed; the speech-loop hardening line should now mention macos-native and the in-flight macos-native-26 as part of the hardening pass.

[ROADMAP.md](ROADMAP.md) Phase 2 section (around lines 98-122) should add bullets under 2.2 (TTS) or a new 2.4 (Native Apple speech) for the macos-native and macos-native-26 work. Suggested phrasing:

> - ✅ **macOS-native speech backend (`--features macos-native`)** — landed (2026-05-15 STT + 2026-05-19 streaming TTS; PRs #95, #112, #122, #123). `MacosSpeechToText` via `SFSpeechRecognizer` with on-device enforcement; `MacosTextToSpeech` via `AVSpeechSynthesizer.writeUtterance:toBufferCallback:` with phrase-by-phrase PCM streaming (Stage B, PR #123). Silero stays as VAD on this path because macOS-26-only `SpeechDetector` would break the macOS 13 floor. en-US + de-DE only — Hindi is deferred until SpeechAnalyzer ships hi-IN on-device.
> - 🟡 **macOS 26 SpeechAnalyzer backend (`--features macos-native-26`)** — in flight (PR #134, 2026-05-20; spec at `docs/superpowers/specs/2026-05-20-macos-native-26-design.md`, plan at `docs/superpowers/plans/2026-05-20-macos-native-26.md`). Replaces Whisper + Silero + ONNX runtime with SpeechAnalyzer + SpeechTranscriber + SpeechDetector via a Swift sidecar bridged through `swift-bridge`. Motivated by the A/B latency probe in PR #131: SpeechAnalyzer is ~100× faster to first partial (~30 ms vs ~3.8 s) and ~2× faster to final (~800 ms vs ~1.8 s) than Whisper `ggml-small.en` on macOS 26.5. Mutually exclusive with `macos-native` at compile time. en-US + de-DE only on macOS 26.5; Hindi `hi-IN` errors loudly at construction (not on-device for SpeechTranscriber yet). **Pending a manual mic round-trip smoke** before flipping out of DRAFT.

Optionally Phase 2.2 can add a "Supertonic TTS vendor — landed (PR #127, #128, 2026-05-19); available behind the `supertonic` feature for A/B evaluation; not wired into a default code path." bullet, but it isn't load-bearing.

### Task D — CLAUDE.md macos-native bullet still describes the pre-Stage-B semaphore path

Line 168 in [CLAUDE.md](CLAUDE.md) (the `**macOS-native speech backend**` bullet) describes the production TTS path branching at runtime using a `dispatch_semaphore` on the background path. **That description is from Stage A; Stage B (PR #123) replaced the semaphore machinery with an `mpsc::channel` and a `synthesize_streaming` driver.** The bullet should be rewritten to describe:

- Channel-based PCM event stream (`SynthesisEvent::Audio` + `SynthesisEvent::PhraseEnd`)
- Unbounded `mpsc::channel` to avoid same-thread producer/consumer deadlock on the GCD main queue
- The `STREAM_DRAIN_POLL_MS = 10` const in `primer_core::consts::speech`
- The structural test pinning ≥2 `Audio` events before `PhraseEnd`
- The state-machine TTFA test pinning per-phrase streaming across multi-phrase responses

The runtime-on-main-thread rationale paragraph (about `Builder::new_current_thread()` on the OS main thread for the CLI) is still correct and load-bearing — keep it. Only the producer/consumer mechanism description changes.

PR #134's `802fa87` commit added a separate `**macos-native-26 speech backend**` bullet immediately after the macos-native one, so the two coexist post-merge. Be careful not to mention "PCM callback → channel" patterns in a way that conflates the two — macos-native-26 uses `swift-bridge`'s async iterator (`nextResult`) not a channel, and its synchronisation primitive is single-flighted Swift Task.

### Task E — Close PR #123 follow-up issues #124, #125, #126

All three were opened by reviewers on PR #123 and are still untouched:

- **#124 — factor shared drain-loop helper between main-thread and background streaming paths.** The two `synthesize_streaming_*` functions in [`crates/primer-speech/src/macos/tts.rs`](src/crates/primer-speech/src/macos/tts.rs) have a duplicated drain loop — one uses `try_recv` interleaved with `runUntilDate(10ms)`, the other uses `recv_timeout(STREAM_DRAIN_POLL_MS)`. Likely <100 LOC to extract a `drain_pcm_events<F>(rx, on_event, on_yield)` helper.
- **#125 — migrate raw GCD bindings to `dispatch2` crate.** The `dispatch_async_f` + `_dispatch_main_q` FFI block currently lives inline in `tts.rs`. The `dispatch2` crate supersedes hand-rolled bindings. Touches `tts.rs` only; verify the chunk-size assumption still holds post-migration via `examples/tts_macos_pcm_smoke.rs`.
- **#126 — wrap `SynthesisSession::push_text`/`finalize` in `spawn_blocking` at call sites.** Currently the synchronous push runs on the tokio task that owns the voice loop. On the CLI this is fine because of the `Builder::new_current_thread()` workaround; on the GUI it's fine because Tauri runs `NSApplicationMain`. The issue documents the contract-fragility — touching either of those two assumptions would surface a regression. Resolution is either (a) wrap in `spawn_blocking` so future moves to a multi-threaded runtime don't break, or (b) document the contract more explicitly in `SynthesisSession`'s doc comment and close as "won't fix; documented." (a) is the safer choice.

These dovetail with the `tts.rs` 638-line split that PR #123's body deferred — extracting the drain helper (#124) is a natural first step toward the file split.

### Task F — Continue the macos-native-26 plumbing per the 16-task plan (if Task A's smoke fails)

The plan at `docs/superpowers/plans/2026-05-20-macos-native-26.md` is the source of truth. Tasks 1-16 are all marked done on the branch — the manual-smoke gate is what holds the PR in DRAFT. Expected follow-ups once green:

- **Partial-streaming visibility** — today only `is_final` SpeechTranscriber results cross the bridge. Volatile partials are dropped at the `TextMessage → String` boundary. Plumbing partials through is a separate refactor (out of scope for #134 per the PR body).
- **`speech` feature slimming under `macos-native-26`** — the `speech` umbrella feature still pulls silero / whisper / piper at build time even when macos-native-26 is the only consumer. Bloats build times for macos-native-26 users.
- **iOS host application** — Apple-platform portability is wired (`cfg(target_vendor = "apple")` where the API is platform-uniform; iOS divergence in `audio_session.rs`), but no iOS Tauri config exists. When iOS lands, rename `macos26/` → `apple26/` per the spec.

### Carried-forward follow-ups (unchanged from previous brief)

The full list is in [`docs/handoffs/2026-05-19T1232+0800-issue-114-stage-b-complete-ready-for-pr.md`](docs/handoffs/2026-05-19T1232+0800-issue-114-stage-b-complete-ready-for-pr.md). Highlights:

- **#98** — split `tests/common/sweep.rs` into `bm25`/`hybrid` submodules. **Defer until Hindi or another third locale lands.**
- **#46** — Hybrid sweep: explore post-RRF `min_score` as a fifth grid axis.
- **#41, #40, #22, #21, #20** — all explicitly deferred per their issue bodies.
- **#129** — wire `--features supertonic` build into CI (opened from PR #127's review).
- **#133** — Whisper streaming re-initialises KV cache on every utterance (opened 2026-05-20; performance hit on long voice sessions; the macos-native-26 path sidesteps this entirely by replacing Whisper).
- **Hindi locale follow-ups** — native-speaker review of `prompts/hi.toml`, Hindi children's-vocabulary corpus, real-LLM smoke, flip-to-stable PR.
- **OpenAI-compat real-server smoke testing** — spin up oMLX / LM Studio / vLLM, run `--backend openai-compat --openai-compat-url http://localhost:8000`, confirm SSE streaming + error classification + embedder round-trip.
- **Klexikon corpus expansion** past 66 articles to close the 2 corpus gaps (`gänsehaut` reflex; tides on `mond`).
- **Local llama.cpp inference (Phase 1.1)** — `LlamaCppBackend` stub remains the entry point.
- **Voice-loop hardening** — echo cancellation, ambient-noise robustness. PR #123 (Stage B) cut per-phrase TTS latency; barge-in cancellation and ambient noise are still ahead, plus PR #134 is the next iteration of the LISTEN-side hardening.
- **CI validation of `cdn.pyke.io` ort-runtime download** — once green, flip the default features so hybrid retrieval is on by default.
- **Branch-protection-on-main** — repo owner needs to flip a GitHub setting: Settings → Branches → require `cargo test (default features)` check on `main`. **Bumped in priority** by this session's events: the 3 macos-native-26 scaffolding commits + this session's Cargo.lock-sync commit all bypassed PR review by going direct-to-main. Branch protection on main would have caught the missing Cargo.lock update at push time via CI's `cargo build --frozen` check (assuming that's wired into the test job — verify before relying on this).

## Open decisions / risks

Newly surfaced this session:

- **`origin/main` is currently committed in a state with a `cargo build --frozen` regression in the pre-`2f1dc81` window.** The 3 scaffolding commits (`9a89ac8` through `47c004a`) added `swift-bridge` to `primer-speech/Cargo.toml` but did not update `Cargo.lock`. Commit `2f1dc81` (this session) closes that gap. **If you bisect main across that 4-commit window**, expect `--frozen` builds to fail between `9a89ac8` and `2f1dc81`. Not a runtime issue — the lock file was just behind the Cargo.toml — but worth knowing if a future CI failure or bisect lands in that window.
- **PR #134 will need a merge conflict resolution** with the 3 scaffolding commits now on main. See Task B above; the resolution rule is uniform (take PR-branch version).
- **CLAUDE.md's macos-native bullet still describes the pre-Stage-B semaphore path.** Stale since PR #123 merged. See Task D above.
- **Direct-to-main commits during interactive multi-session work happened twice this week** — Horst pushed the 3 scaffolding commits during this session, and this session's `2f1dc81` Cargo.lock fix also went direct-to-main. Both with reasonable justification (small mechanical changes that would gate downstream work), but the pattern is worth flagging — branch protection on `main` would force these through PRs and a CI build. Re-prioritise the branch-protection item.

Carried forward from previous brief — all still applicable to the macos-native (Stage B) path that's now on `main`:

- **The mpsc channel in macos-native is unbounded by design — do not "fix" this to be bounded.** Two paths share the same producer thread (GCD main queue): the main-thread path's producer + consumer are literally the same thread, and even the background path's producer is on the GCD main queue. A bounded sync_channel that filled up would deadlock the GCD main queue — a hard "never block" surface in the AVFoundation contract. The unbounded channel makes this a structural property; per-phrase memory footprint is at most ~100 events × ~1 KB = ~100 KB.
- **The per-utterance streaming win measured by `--measure-ttfa` is small (~10-20 ms).** AVSpeechSynthesizer's `writeUtterance:toBufferCallback:` synthesises faster than real-time on macOS 15.x — once the first chunk fires, the remaining ~100 chunks for a 2.5-second phrase arrive in a tight ~17ms burst before EOS. The real user-facing win is **per-phrase across multi-phrase responses**, pinned by the state-machine TTFA test in commit `9a86874` (now on `main` via PR #123).
- **`tts.rs` is 638 lines (over the 500-line guideline).** Task E above + issue #124 are the path here.

Carried forward, still pending verification:

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
- `SpeechLoopConfig` shape differs between speech builds (3 fields macos-native / 7 fields else). **Now 3 shapes** with macos-native-26 in flight — the cfg-gates in `primer-cli/src/speech_loop/mod.rs` and `main.rs` use `any(feature = "macos-native", feature = "macos-native-26")` to widen the gate for both Apple-native paths (PR #134 commit `a43b219`).
- Open-counter is thread-local, not global.
- `SqliteSessionStore::set_locale` is mutable by reference.

## Patterns to reuse, not reinvent

(All inherited from prior sessions; see [docs/handoffs/2026-05-19T1232+0800-issue-114-stage-b-complete-ready-for-pr.md](docs/handoffs/2026-05-19T1232+0800-issue-114-stage-b-complete-ready-for-pr.md) for the full list.)

New from this session:

- **Verify "stale" claims with `git reflog show <remote-ref>` before recommending a destructive operation.** The first draft of this brief proposed `git reset --hard origin/main` to drop 3 commits I claimed were "stale duplicates of PR #134 work". Halfway through preparing the handoff, the user (correctly) asked me to verify the no-data-loss claim. Re-running the comparison surfaced that `origin/main` had moved during the session — those commits were no longer ahead of origin; they had been pushed to origin/main by a parallel terminal session. The lesson: when a multi-session collaboration involves a human who might be working in another shell, `origin/<branch>` can change underneath you in mid-session, and any claim about "ahead-of-origin" is stale the moment you make it unless you re-fetch. The reflog message `update by push` is the unambiguous signal that the remote ref moved by a push action (vs. `fetch: fast-forward` which just means "we learned the remote moved").
- **A handoff session can have a small drive-by commit.** This session's `2f1dc81` (Cargo.lock sync) was not part of any planned task — it surfaced organically while diagnosing why the local working tree had Cargo.lock modifications, and the right resolution was a 1-line commit message + push. Keep an eye out for these during handoff sessions; small mechanical fixes that unblock downstream work are exactly the kind of "ride-along on whatever is open" task that fits a handoff session's scope.
- **`gh pr list --state all --limit N`** is the most reliable single-call source-of-truth for "what merged since the brief". `git log origin/main` shows the merge commits but lots of useful context (PR body, draft status, follow-up issues, smoke checks) lives only in the PR JSON.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status                       # expect: clean tree, up-to-date with origin/main
git fetch --all --prune
gh pr list --state open          # expect: PR #134 DRAFT on claude/macos-native-26

# Opt-in to the local pre-commit hook (one-time per clone):
git config core.hooksPath .githooks

# === Verify PR #134 branch is still green (macOS host only) ===
git checkout claude/macos-native-26
cd src
~/.cargo/bin/cargo build --features primer-cli/speech
~/.cargo/bin/cargo build --features primer-cli/speech,primer-cli/macos-native
~/.cargo/bin/cargo build --features primer-cli/speech,primer-cli/macos-native-26
~/.cargo/bin/cargo test -p primer-speech --features macos-native-26 --lib
~/.cargo/bin/cargo test -p primer-speech --features macos-native-26 --test macos26_smoke -- --ignored
~/.cargo/bin/cargo fmt --all -- --check
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings

# === Manual mic round-trip smoke for PR #134 (Horst-driven; the only unchecked box) ===
~/.cargo/bin/cargo run --features primer-cli/speech,primer-cli/macos-native-26 --bin primer -- \
    --backend stub --speech --language en --name Smoke --age 8 --no-persist
# Speak: "what colour is the sky"
# Expected: streaming partials → stub Primer response → spoken via AVSpeechSynthesizer
# Exit: type/say "bye" or "exit"

# === If smoke passes: rebase PR #134 onto main and push ===
git rebase origin/main
# Resolve conflicts on the 5 files: Cargo.toml, lib.rs, macos26/mod.rs,
# macos26/audio_session.rs, tests/macos26_smoke.rs. Resolution rule: take PR-branch version.
# Then:
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
- The doc-debt items (Tasks C + D) can ride on whatever next code-PR is in flight — they don't need their own PR.
