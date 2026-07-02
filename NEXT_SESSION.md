# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-07-02 13:31 UTC. On branch `refactor/split-dialogue-manager-turn` at `bfafd78`. **One new PR open against `main`: #320 (dialogue_manager turn.rs split).** `main` is at `70a1261` — both of the prior session's PRs merged between sessions (#317 → `ce5aa04`, #318 → `70a1261`).

This session ran the standard sweep FIRST: the oversized-file sweep found the expected list, and the inline-test detector was **clean for the first time in six sessions** (no `#[cfg(test)] mod … {` inline module in any >500-line file). So the session went straight to the owner-approved production-split lane and shipped the recommended pick.

## What we shipped this session

- **PR #320 (branch `refactor/split-dialogue-manager-turn`), one commit `bfafd78`** — `refactor(pedagogy): split dialogue_manager turn.rs into responsibility submodules`. The 746-line per-turn hot path became a directory module, all files < 500 lines:
  - `turn/mod.rs` (138) — module doc (submodule map) + the `respond_to` / `respond_to_streaming` orchestrator. Uses `use super::DialogueManager` (same depth as the old flat file).
  - `turn/prompt.rs` (132) — `record_child_turn`, `build_turn_prompt`, `record_primer_turn`. **Visibility wrinkle (new this session): `build_turn_prompt` is called directly by `dialogue_manager/tests/turn_tests.rs`**, so plain `pub(super)` (which would now resolve to `turn`) is not enough — it became `pub(in crate::dialogue_manager)`, the same effective reach the old `pub(super)` had from the flat file. Everything only the orchestrator calls stayed `pub(super)`.
  - `turn/stream.rs` (137) — `StreamOutcome` (kept `pub(super)`), `stream_inference_response` (narrowed to file-private — its only caller `run_recovery_loop` moved with it), `run_recovery_loop` (`pub(super)`).
  - `turn/persist.rs` (98) — `persist_turn` (`pub(super)`), `spawn_embedding_task` (private).
  - `turn/spawn_tasks.rs` (284) — `spawn_classification_task`, `spawn_post_response_task` (both `pub(super)`).
  - Deeper submodules import via absolute `crate::dialogue_manager::…` (recipe step 5); zero churn in the 1178-line `turn_tests.rs`.
  - CLAUDE.md: `dialogue_manager` module-layout bullet rewritten for the `turn/` sub-split + two stale `turn.rs` path references fixed (the i18n note's `stream_inference_response` site, the small-context budget call-site list). README/ROADMAP untouched (internal refactor; grep-verified no references to `turn.rs` / `respond_to_streaming` / `run_recovery_loop` in either).

### Verification
- Baseline BEFORE the split: `cargo test -p primer-pedagogy` → **216 passed / 0 failed**; identical after the split. fmt made zero changes (no re-wraps this time — the moved code was already at final indent depth).
- Pub-surface diff (incl. `type` in the regex) empty: `{pub async fn respond_to, pub async fn respond_to_streaming}` before and after.
- `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean.
- Full `cargo test --workspace` → **51 × `test result: ok`, 0 FAILED/error** (matches prior session's count).
- No cargo features on `primer-pedagogy` — the default test run is the full matrix; no feature-combos clippy needed.

## What's next (concrete acceptance criteria)

### 0. PR #320 — owner review/merge
**CI fully green at session close (all 11 checks pass, including the full Rust matrix).** Acceptance: owner merges.

### CHEAP TEST EXTRACTIONS: detector was CLEAN this session — but re-run it EVERY session
The sweep + detector are cheap and between-sessions PRs have pushed files over 500 lines five times before. Commands in the resume block. Watch-list: `primer-classifier/src/llm.rs` (~460) and `primer-extractor/src/llm.rs` (~470) both carry inline test modules and sit near the threshold — the next feature PR touching them likely trips the detector.

### Production-code splits — the open, owner-approved lane (pick the next one, lowest-risk first)
Remaining oversized **production** (non-test) files after #320 (post-split sweep):
- **`primer-pedagogy/src/prompt_builder.rs` (708)** — **recommended next pick.** Contains `decide_intent()` (the Socratic brain) + heavy characterization tests; sensitive but the 40 intent-routing tests are the guard, and the likely first move is the cheap shape: extract the inline test module to a sibling (`prompt_builder/tests.rs` after converting to a directory module, or leave production intact and only split tests) — measure how much is tests vs production before deciding.
- **`primer-speech/src/macos/tts.rs` (668)** — the #126 `spawn_blocking` + file-split is tracked together (macos-native-gated → dual-verify; needs macOS host for the feature build).
- **`primer-storage/src/schema.rs` (623)** — migration chain; `cargo test -p primer-storage` guard.
- **`primer-gui/src/wiring.rs` (591)** / **`primer-inference/src/qnn/genie/real.rs` (566, qnn-gated)** / **`primer-core/src/consts.rs` (562)** / **`primer-gui/src/commands/voice.rs` (559, speech-gated)** / **`primer-gui/src/config/types.rs` (539)** / **`primer-speech/src/voice_loop/state_machine/inner.rs` (506)** / **`primer-speech/src/macos/stt.rs` (504, macos-native-gated)** — medium/small, several feature-gated (dual-verify).
- Hardest: `primer-cli/src/main.rs` (1357, heavily `cfg(feature)`-gated — needs the per-feature clippy+test matrix).
- `dialogue_manager/turn.rs` — **NO LONGER on the list** (fixed by #320).

The >500 test-support files (`store/tests/session_tests.rs` 2442, `state_machine/mocks.rs` 1381, `dialogue_manager/tests/turn_tests.rs` 1178, `dialogue_manager/tests/background_tests.rs` 777, `kb-load/tests/common/mod.rs` 677, `test_support.rs` 655, `store/tests/learner_tests.rs` 607) remain lower-value than the production files.

**No re-ask needed — the production-split lane is owner-approved.** Only re-confirm if changing lanes back to docs/maintenance.

### The proven split recipe (third clean run this session)
1. Baseline: `cargo test -p <crate>` (with the right `--features` if tests are gated) green BEFORE touching anything; record the pass count.
2. Read the whole file; map natural responsibility boundaries.
3. Convert `foo.rs` → `foo/mod.rs` + siblings. `mod.rs` keeps anything callers outside the new dir reach via unchanged paths (or re-exports it).
4. **Visibility across the new module boundary — three cases seen so far:**
   - Private helpers/consts a *test child of the parent* reaches (`FALLBACK_LINE`, #318): keep them IN `mod.rs`.
   - A `pub(super)` method that *parent-level tests* call directly (`build_turn_prompt`, this session): re-declare as `pub(in crate::<parent-path>)` — same effective reach, test files stay untouched.
   - A helper whose only caller moves with it (`stream_inference_response`): narrow to private.
5. Fix moved relative paths: prefer absolute `crate::…` imports in the new submodules over `super::super::…` chains. (`mod.rs` itself keeps plain `super::…` — its depth is unchanged.)
6. Verify: pub-surface diff empty (`grep -oE 'pub (struct|enum|fn|const|async fn|trait|type) [A-Za-z_]+' | sort -u`, old vs new — **include `type`**), crate suite count matches baseline, feature-combos clippy if cfg arms exist, full-workspace `cargo test`, clippy `-D warnings`, fmt `--check` (run fmt AFTER the split).

### Carried / owner-or-hardware-gated (none host-completable autonomously)
- **#260** — Android-voice on-device acceptance (RedMagic 11 Pro + mic + quiet room).
- **#192** — manual macOS-native STT + injected non-AVSpeech TTS audio path (mic + macOS build).
- **#170 Stage B / E / F** — Supertonic voice-mode TTS + in-loop A/B numbers + Hindi preview→stable (OpenRAIL-M clause (e) disclosure must ship before any default Supertonic flip).
- **#166 item #1** — owner-run WhisperStream reuse smoke (model + two 16 kHz WAVs; command at the bottom).
- **#135** — glib 0.18.5 → 0.20+ (blocked on Tauri 3).
- **Branch protection** — wire `cargo test (default features)` as a required status check on `main` (owner GitHub-settings call; still outstanding).
- QNN stable-token-across-reboots gate; NPU pedagogy/answer-quality tuning; latency-routing calibration.

## Open decisions / risks

- **PR #320 open, awaiting owner review/merge.** Pure refactor, no runtime behaviour change; CI fully green.
- **The production-split lane stays open and owner-approved.** Recommended next: `primer-pedagogy/src/prompt_builder.rs` (708) — no feature gates, but it holds `decide_intent()`; treat the 40 intent-routing characterization tests as the hard guard and prefer the test-extraction shape first if the file is mostly tests.
- **The inline-test detector was clean this session (first time in six).** Between-sessions PRs can still push near-threshold files (`primer-classifier/src/llm.rs` ~460, `primer-extractor/src/llm.rs` ~470) over 500. The sweep + detector are cheap — run both before picking work.
- **Machine load / build times:** deps were warm — `cargo test -p primer-pedagogy` seconds, workspace clippy ~2 min, full workspace test ~7 min. Cold-start budget remains ~35 min for the first cargo pass. Run ONE cargo pass at a time; don't run fmt (source-modifying) while clippy is mid-flight on the same crate.

## Patterns to reuse, not reinvent

- **The 6-step split recipe above** — new wrinkle this session: `pub(in crate::<parent>)` for a method that parent-level test files call directly (keeps the test files 100% untouched while preserving effective visibility after the module moves one level deeper).
- **Pub-surface diff must include `pub type`**. Full regex: `pub (struct|enum|fn|const|async fn|trait|type) [A-Za-z_]+`. Note it deliberately does NOT match `pub(super)`/`pub(crate)` — those are internal.
- **Doc-drift hunt recipe:** after a split, grep the moved file's name AND its key symbols across README/ROADMAP/CLAUDE (this session caught two stale `turn.rs` path references in CLAUDE.md beyond the layout bullet).
- **Use ABSOLUTE paths for shell tools** — Bash cwd resets between calls. `git show main:<path>` wants the repo-root-relative path (`src/crates/…`).
- **A grep final pipe stage returns exit 1 on zero matches** — check the `test result:` / `Finished` lines, not the pipeline exit code.
- **Branch each PR off `main`, not off the previous branch.**
- **Run long cargo passes with `run_in_background: true`**; the harness notifies on completion — no polling needed.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git fetch && git log --oneline -4 && gh pr list --state open
# One PR open at close: #320 (turn.rs split, CI fully green). After it merges, start fresh off main.

# === Standard workspace gate (run from src/ if you touch .rs) — one cargo pass at a time, grep twice ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test --workspace 2>&1 | tee /tmp/ws.log | tail -3
grep -cE 'test result: ok' /tmp/ws.log            # expect 51
grep -E 'test result: FAILED|^error' /tmp/ws.log  # expect empty
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check

# === Oversized-file sweep + inline-test detector (re-verify EVERY session) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done
# Empty inline output = no cheap pick. Next host-actionable work = next production split (owner-approved lane).

# === Recommended next split: primer-pedagogy/src/prompt_builder.rs (708). Baseline FIRST: ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-pedagogy 2>&1 | grep 'test result: ok'   # record pass count BEFORE splitting (216 today)
# Check how much of the 708 is the inline test module — if most, the cheap test-extraction shape may suffice.
# ... apply the 6-step recipe ... then re-verify same count + pub-surface diff + clippy + fmt + workspace.

# === Behaviour-preserving pub-surface diff (repo-root-relative git path; note `type` in the regex) ===
git show main:src/crates/<path>.rs | grep -oE 'pub (struct|enum|fn|const|async fn|trait|type) [A-Za-z_]+' | sort -u > /tmp/old-pub.txt
cat <new submodule files> | grep -oE 'pub (struct|enum|fn|const|async fn|trait|type) [A-Za-z_]+' | sort -u > /tmp/new-pub.txt
diff /tmp/old-pub.txt /tmp/new-pub.txt   # empty = identical external pub surface

# === Carried: owner-run the #166 reuse smoke (needs a model + two 16 kHz mono WAVs) ===
PRIMER_WHISPER_MODEL=/path/to/ggml-small.en.bin \
PRIMER_WHISPER_AUDIO_A=/path/to/utterance_a.wav \
PRIMER_WHISPER_AUDIO_B=/path/to/utterance_b.wav \
  ~/.cargo/bin/cargo test -p primer-speech --features whisper \
  --test whisper_stream_reuse -- --ignored --nocapture
```

## Reporting back

- **PR #320 (dialogue_manager turn.rs split):** the recommended pick from the prior brief. 746 → `{mod 138, prompt 132, stream 137, persist 98, spawn_tasks 284}` — 2-symbol external pub surface byte-identical, 216/216 crate tests (baseline match), zero churn in the 1178-line `turn_tests.rs` (the `pub(in crate::dialogue_manager)` wrinkle), workspace clippy/fmt clean, workspace suite green (51 ok).
- **Prior session's PRs #317 and #318 merged between sessions** — the inline-test detector came up clean for the first time in six sessions.
- **The production-split lane is open and owner-approved** — next pick `primer-pedagogy/src/prompt_builder.rs` (708; check test-vs-production ratio first) without re-asking.
- The GUI is a full app, not a scaffold.
