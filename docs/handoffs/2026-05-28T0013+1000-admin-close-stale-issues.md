# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-28T0013+1000 — short admin session. Between briefs: PR #159 (issue #141) merged as `72a05f7`; PR #161 (batch follow-ups for #46/#129/#138/#142/#160) merged as `5c99c75`. This session closed the four issues that PR #161 fixed but whose GitHub issues weren't auto-closed (the `Closes #N` keywords landed in the squash commit body only, not the open PR's body).

## What landed since the previous brief

| SHA       | Title                                                                                                                | Date              | Notes                                                                                                                                                                                                          |
| --------- | -------------------------------------------------------------------------------------------------------------------- | ----------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `72a05f7` | `build(macos26): friendlier panic when xcrun/xcode-select/swiftc fail (#141) (#159)` | 2026-05-27 ~23:50 | PR #159 merged. Closed issue #141 — routes Xcode/swiftc probe failures through a pure-helper module (`crates/primer-speech/src/macos26/build_hints.rs`) so cargo emits `cargo:warning=…` lines with an actionable install hint before the panic. |
| `5c99c75` | `chore: batch fix five tracking-issue follow-ups (#161)` | 2026-05-28 ~00:03 | PR #161 merged. Bundled fixes for #46 (Hybrid sweep `min_score` 5th axis), #129 (CI feature-combos job), #138 (VoiceQuality reorder doc note), #142 (drop `Macos26TranscriptionSession` shim), #160 (README/ROADMAP DRAFT→landed flip). |

## What we shipped this session

**Admin only — no code changes.** Closed 4 issues whose work had already merged in PR #161 but which GitHub failed to auto-close:

- **#138** — VoiceQuality reorder doc note. CLAUDE.md anchors the `Default < Enhanced < Premium` rationale; `select_voice` docstring fixed from the stale "Enhanced over Premium over Default" claim.
- **#142** — `Macos26TranscriptionSession` trait shim dropped. `Macos26Stt` no longer implements `StreamingSpeechToText`; locale-validating constructor + `Named` impl remain. Smoke tests unchanged.
- **#129** — `--features supertonic` build wired into CI. New `feature-combos` job runs `cargo check -p primer-speech --features supertonic` and `cargo check -p primer-embedding --features fastembed` on Ubuntu.
- **#160** — README + ROADMAP flipped from "in flight (PR #134, DRAFT)" to "landed (PR #134, 2026-05-23)". Carry-over now resolved after 4 successive session handoffs.

Each issue got a closing comment pointing to the merge commit `5c99c75` and naming the file(s) that contain the fix.

### Why this happened (worth knowing for next time)

PR #161 squash-merged with `Closes #46, #129, #138, #142, #160` *in the commit body* but not in the open PR's *body* on GitHub. GitHub's auto-close keywords have to be in the **PR description** (or in pushed commit messages) at PR-close time — once squash-merge collapses everything to one commit, the parsing only sees what's on the PR. The open PR's body had a section *describing* the five issues but didn't include the literal `Closes #N` form for each. **For future batch PRs, put `Closes #A, #B, #C, …` as a stand-alone line in the PR body** — not just in the commit body and not just as descriptive prose.

### Why README.md and ROADMAP.md were not touched

This session shipped no code. (README and ROADMAP were already updated in PR #161 to flip the `macos-native-26` DRAFT→landed status.)

### Verification matrix (lightweight — admin session)

| Command                                                  | Expected      | Got           |
| -------------------------------------------------------- | ------------- | ------------- |
| `git status` (on `main`, after `git fetch`)              | clean         | **clean**     |
| `git log --oneline -3`                                   | `5c99c75` top | **`5c99c75`** |
| `gh issue list --state open` after closures              | 11 items      | **11**        |

No `cargo` invocations this session — no code touched.

### Tasks not addressed this session

- **No code work.** The session was an admin/housekeeping pass that took ~10 min total.
- **`feature-combos` CI job hasn't run on a real PR yet.** PR #161 merged via squash, so the first green run of the new job validates only the YAML, not the bug class it's guarding against. The next PR that touches `primer-speech` or `primer-embedding` will be the real test.

## What's next — by priority

With #138/#142/#129/#160 closed, the open queue is **11 issues, all either upstream-blocked or self-deferred**. Pick from the live list below — there is no single dominant follow-up.

### Open-queue snapshot (`gh issue list --state open` after this session)

| #   | Title                                                                                                            | State                                            |
| --- | ---------------------------------------------------------------------------------------------------------------- | ------------------------------------------------ |
| 157 | fastembed/ort-sys cannot build for `aarch64-linux-android` (no Android arm in ort-sys cache_dir cfg)             | upstream — needs PR to `pyke/ort`                |
| 151 | CI: build each documented feature combination, not just default                                                  | macOS-runner blocker on the macOS combos          |
| 149 | speech(voice-loop): split `backends_macos.rs` (744 lines) + DRY shared builder tail                              | mechanical refactor; medium scope                |
| 135 | deps: bump glib 0.18.5 → 0.20+ once Tauri 3 ships (RUSTSEC-2024-0429)                                            | waits on Tauri 3                                 |
| 133 | speech: Whisper streaming re-initialises KV cache on every utterance                                             | sidestepped by `macos-native-26`; non-trivial    |
| 98  | refactor(tests): split `tests/common/sweep.rs` into bm25/hybrid submodules                                       | defer until 3rd locale lands                     |
| 41  | data/ingest: consider scoping disambiguation regex to lead-sentence patterns                                     | self-deferred                                    |
| 40  | data/ingest: aggregate per-source attribution for the Wikipedia layer                                            | self-deferred                                    |
| 22  | primer-knowledge: cache prepared statements for the corpus-bootstrap path (Phase 0.2)                             | self-deferred                                    |
| 21  | CLI: separate `--languages` preference list from bound `--language` locale                                       | self-deferred                                    |
| 20  | i18n: placeholder validator can false-fail on translator narrative text                                          | self-deferred                                    |

### Concrete actionable candidates

In rough order of bang-per-buck:

- **#149** — `backends_macos.rs` (744 lines) split + DRY pass into shared `build_audio_thread` helper. Mechanical refactor; touches the voice-loop builder boundary; medium scope. Largest concrete code task with no upstream dependency.
- **#157** — upstream PR to `pyke/ort` adding a `target_os = "android"` arm in `internal::dirs::cache_dir`. Cross-repo work; unblocks hybrid retrieval on Android Termux. Likely a tiny patch on the upstream side (XDG_CACHE_HOME fallback from the Linux arm would probably work as-is).
- **#133** — Whisper streaming KV-cache re-init per utterance. Sidestepped on macos-native-26 paths (SpeechAnalyzer doesn't have this shape), but still applies to the Whisper-backed paths (Linux, non-Apple). Non-trivial; needs careful TDD against the `voice_loop` state machine.

### Carried-forward queue (not in any open issue)

- Hindi locale follow-ups — native-speaker review of `prompts/hi.toml`, Hindi children's-vocabulary corpus, real-LLM smoke, flip-to-stable PR.
- OpenAI-compat real-server smoke testing.
- Klexikon corpus expansion past 66 articles.
- Local llama.cpp inference (Phase 1.1) — big feature work, would span multiple sessions.
- Voice-loop hardening — echo cancellation, ambient-noise robustness.
- CI validation of `cdn.pyke.io` ort-runtime download — then flip default features so hybrid retrieval is on by default.
- Branch-protection-on-main — repo owner needs to flip a GitHub setting. **Still overdue.**
- Swift-side XCTest harness for the sidecar at `crates/primer-speech/swift-sources/Macos26PipelineImpl.swift` — currently zero direct test coverage. Defer until at least one more Swift-side regression makes the case obvious.
- Loosen the `voice_loop` parent gate — one-line follow-up from PR #152; see [docs/handoffs/2026-05-25T2018+1000-issue-139-pr-152-open.md](docs/handoffs/2026-05-25T2018+1000-issue-139-pr-152-open.md).

## Open decisions / risks

Newly surfaced this session:

- **`Closes #N` keywords must be in the PR body, not just the squash commit body, to auto-close issues on merge.** PR #161 demonstrated the failure mode: the squash commit message had `Closes #46, #129, #138, #142, #160` but the open PR's body only described the issues by number in prose. GitHub parses the PR body (and incoming commit messages while the PR is open) — once squash-merge happens, the synthesized commit is too late. **For future batch PRs, put a stand-alone `Closes #A, #B, #C, …` line in the PR body**, or accept that closing has to be a manual step after merge.

Carried forward — still applicable:

- **`samples.as_ptr()` validity is bounded by ARC, not by anything mechanical** — from the PR #155 brief; still applies.
- **The Layer-1 → Layer-2 → Cold-path branch order in `nextResult()` is load-bearing** — from PR #154; still applies.
- **`PipelineSource`'s `Pin<Box<dyn Future + Send>>` return style was chosen over RPITIT for stability, not aesthetics.** Per-call boxed-future allocation at ~20-100 ms cadence — negligible.
- **`ConsumerConfig` doubles as a testability seam AND a "future-tunable knob".** Resist plumbing it through CLI flags until there's a concrete tuning use case.
- **The `voice-loop` feature is the only way to reach the `PendingMicBuffer` tests from #139.**
- **Unit-test coverage is structural; real-audio (and real-missing-Xcode) coverage still requires a person.**
- **The macos-native-26 clippy gate had never been run before until PR #150** — three lints rotted unnoticed. There may be other "this configuration has never been clippy'd" surfaces.
- **`backends_macos.rs` at 744 lines exceeds the 500-line CLAUDE.md guideline.** Tracked as #149.
- **`#![cfg(...)]` inner attribute pattern is brittle under clippy.**
- **The mpsc channel in macos-native is unbounded by design — do not "fix" this to be bounded.**
- **The per-utterance streaming win measured by `--measure-ttfa` is small (~10-20 ms); the real win is per-phrase across multi-phrase responses.**
- **The `let owned = ctx;` identity-rebind in the `exec_async` closure is load-bearing but not enforced by anything** (RFC 2229 whole-struct capture trap).
- **`tts.rs` is 636 lines, over the 500-line guideline.** File-split deferred.
- **`dispatch2 0.3.1` is the latest line as of 2026-05;** workspace pins `dispatch2 = "0.3"`, a future 0.4 won't auto-upgrade.
- **Option-(a)-vs-(b) on issue #126 is a documented design call** — see the 45-line block at `state_machine.rs:847`.
- **The `#[path]` + `#[cfg(test)] mod` two-load pattern from PR #159** is the only way to get TDD coverage on build-script logic without code duplication. The `#![allow(dead_code)]` at the top of `build_hints.rs` is needed because the helpers are only *used* via `#[path]` from build.rs, not by anything in the lib — without the allow, `cargo clippy -- -D warnings` would fire on every helper item in test mode.
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

- **Always put `Closes #A, #B, #C, …` as a stand-alone line in the PR body for batch-fix PRs.** The squash commit body alone is not enough — GitHub's issue-auto-close only parses what's in the PR description (or in pushed commit messages while the PR is open), and once squash-merge collapses everything, that window has closed. Test: after merging, run `gh issue list --state open` and grep for the issue numbers — if any expected-closed issue is still open, the PR body lacked the magic line. Quick fix: close them by hand with comments pointing to the merge commit, then write a CLAUDE.md note (this brief) so the next PR author doesn't repeat the mistake.

Carried forward (from prior sessions; see prior handoff trail for two-layer drain, capacity-1-channel-as-sync-lever, pure-helper extraction, `#[path]` + `#[cfg(test)] mod` two-load pattern for build-script TDD, gate-narrowing, RFC-2229 whole-struct capture, and trait-directive refusal documentation patterns).

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status                       # expect: clean working tree on main
git fetch --all --prune
gh pr list --state open          # expect: no open PRs from this work
git log --oneline -3             # expect 5c99c75 on top
gh issue list --state open       # expect 11 open: #157 #151 #149 #135 #133 #98 #41 #40 #22 #21 #20

# Opt-in to the local pre-commit hook (one-time per clone):
git config core.hooksPath .githooks

# === Verify default-features build is still green ===
cd src
~/.cargo/bin/cargo test --workspace                                    # expect 858/0/3
~/.cargo/bin/cargo fmt --all -- --check                                # expect clean
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings     # expect clean

# === If picking up a follow-up next ===
gh issue view 149   # backends_macos.rs file-split (744 → ~3 files under 500). Largest concrete refactor available.
gh issue view 157   # ort-sys Android cache_dir cfg gap — needs upstream PR to pyke/ort
gh issue view 133   # Whisper KV-cache re-init per utterance — non-Apple paths only
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

# macOS-native speech gates (skip on non-macOS hosts):
~/.cargo/bin/cargo test -p primer-speech --features macos-native            # expect 49/0/3
~/.cargo/bin/cargo test -p primer-speech --features macos-native-26         # expect 71/0/3

# Python ingestion pipeline tests (uv-only — never invoke pip directly):
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- If picking up #149 (backends_macos.rs split), check the existing tests in `voice_loop::backends` *first* and run them green before any move — the file is the seam between the macOS-native and macOS-native-26 audio paths and a regression here is hard to spot from default-features CI alone.
