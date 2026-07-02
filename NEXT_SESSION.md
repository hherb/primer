# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-07-02 (UTC). On branch `refactor/split-voice-loop-state-machine` at `901b1a7`. **Two new PRs open against `main`: #317 (comprehension llm test extraction) and #318 (voice_loop state_machine production split).** `main` is at `84db198` (prior session's PR #305 merged, plus an owner-driven PR #306 — prompt optimisation for small local models — that landed between sessions).

This session ran the standard sweep FIRST and the inline-test detector **fired for the fifth time** (PR #306 pushed `primer-comprehension/src/llm.rs` to 717 lines with an inline test module), so a cheap test extraction preceded the planned production split. Both shipped on separate branches off `main`.

## What we shipped this session

- **PR #317 (branch `refactor/extract-comprehension-llm-tests`), one commit `ebcccf4`** — `refactor(comprehension): extract llm tests to sibling file`. Moves the inline `#[cfg(test)] mod tests` out of `primer-comprehension/src/llm.rs` into a sibling `llm/tests.rs` (non-root module → one dir deeper, mkdir), following the `macos26/analyzer` precedent (#295): 717 → 317 (production) + 399 (tests). Verbatim relocation plus the rustfmt re-wraps triggered by the one-level dedent; the test module stays a child of `llm`, so `use super::*` still reaches the private `EXAMPLE_OUTPUT` with no visibility edits. **20 lib tests identical pre/post**; clippy `-D warnings` + fmt clean; pub-surface diff empty; no cargo features on the crate. **CI: all 11 checks green.**
- **PR #318 (branch `refactor/split-voice-loop-state-machine`), one commit `901b1a7`** — `refactor(speech): split voice_loop state_machine.rs into responsibility submodules`. The 786-line `state_machine.rs` became a directory-style module (the dir already existed for `mocks.rs`). All files < 500 lines:
  - `state_machine/mod.rs` — module doc, flat `pub use` façade (9-symbol external surface byte-identical: `DrainHook, LoopBackends, LoopConfig, LoopHandle, Responder, VAD_EVENT_CHANNEL_CAPACITY, VoiceLoopError, run_loop, run_loop_borrowed`), and the private `FALLBACK_LINE` — kept there because `mocks.rs` (the test module, a child of this module) asserts against it via `super::FALLBACK_LINE`.
  - `state_machine/types.rs` (189) — `LoopConfig`, `LoopBackends` (+ impl), `Responder`, `LoopHandle`, `DrainHook`, `VoiceLoopError`, `VAD_EVENT_CHANNEL_CAPACITY` (verbatim; the `[run_loop]` intra-doc links qualified to `[super::run_loop]` post-review so `cargo doc` stays warning-free).
  - `state_machine/entry.rs` (90) — `run_loop` + `run_loop_borrowed` (verbatim).
  - `state_machine/inner.rs` — `run_loop_inner`, now `pub(super)`, plus the private `handle_llm_err` helper (moved next to its only call sites in the post-review pass). Imports — observer/quit_detect/tts_markdown via `crate::voice_loop::…` paths (the old flat file used parent-relative `super::…`, which would have become `super::super::…`).
  - `state_machine/mocks.rs` — one-line import churn from the review pass: the observer types (`ExitReason`/`LoopObserver`/`TurnCompletePayload`/`VoiceState`) are now imported directly from `crate::voice_loop::observer` instead of resolving through a `#[cfg(test)]`-gated re-import in `mod.rs` (imports live in the file that uses them).
  - CLAUDE.md got a module-layout note appended to the `primer-speech` bullet (mirrors the #304/#305 precedent). README/ROADMAP untouched (internal refactors; verified no drift — neither file mentions `state_machine`/`run_loop`).

### Verification
- **PR #317:** `cargo test -p primer-comprehension` → 20 passed / 0 failed, identical to pre-change baseline; clippy default `-D warnings` clean; fmt clean; pub-surface diff empty.
- **PR #318:** baseline BEFORE the split with `cargo test -p primer-speech --features voice-loop` → **101 passed / 0 failed / 2 ignored (lib) + 1 + 10 (integration)**; identical after the split AND after the fmt pass. Pub-surface diff empty. cfg-arm dual-verify `cargo clippy -p primer-speech --features voice-loop,silero,whisper,piper --all-targets -- -D warnings` clean; default clippy clean; fmt clean. **Full-workspace `cargo test --workspace` → 51 `test result: ok`, 0 FAILED/error** (matches prior session's count).
- Doc-drift check on the between-sessions PR #306: it shipped its own CLAUDE.md additions; no README/ROADMAP drift found.

## What's next (concrete acceptance criteria)

### 0. PRs #317 and #318 — owner review/merge
#317 CI is fully green. #318 CI was pending at session close — watch `gh pr checks 318` (the full Rust matrix runs; touches `.rs`).

### CHEAP TEST EXTRACTIONS: detector fired AGAIN this session (5th time) — always re-run it
`primer-comprehension/src/llm.rs` crossed 500 lines via the between-sessions PR #306 and the detector caught it. After #317 + #318 merge, the detector should be clean — **but re-run the sweep + detector fresh every session; the "exhausted" claim has now been wrong FIVE times.** Commands in the resume block. Note the sibling crates `primer-classifier/src/llm.rs` (~460 after #306) and `primer-extractor/src/llm.rs` (~470) are close to the 500 line and have inline test modules — the next feature PR touching them likely trips the detector again.

### Production-code splits — the open, owner-approved lane (pick the next one, lowest-risk first)
Remaining oversized **production** (non-test) files after this session's two PRs (post-split sweep):
- **`primer-pedagogy/src/dialogue_manager/turn.rs` (746)** — **recommended next pick.** Already a directory module (`dialogue_manager/`), so add sibling files; single impl block → `pub(super)` churn. Guard: `cargo test -p primer-pedagogy`. No cargo features on the crate (default test run is the full matrix).
- **`primer-pedagogy/src/prompt_builder.rs` (708)** — contains `decide_intent()` (the Socratic brain) + heavy characterization tests; high-value but sensitive. The 40 intent-routing tests are the guard.
- **`primer-comprehension/src/llm.rs` — NO LONGER on the list** (fixed by #317); `voice_loop/state_machine.rs` — **NO LONGER on the list** (fixed by #318).
- **`primer-speech/src/macos/tts.rs` (668)** — the #126 `spawn_blocking` + file-split is tracked together (macos-native-gated → dual-verify; needs macOS host for the feature build).
- **`primer-storage/src/schema.rs` (623)** — migration chain; `cargo test -p primer-storage` guard.
- **`primer-gui/src/wiring.rs` (591)** / **`primer-inference/src/qnn/genie/real.rs` (566, qnn-gated)** / **`primer-core/src/consts.rs` (562)** / **`primer-gui/src/commands/voice.rs` (559, speech-gated)** / **`primer-gui/src/config/types.rs` (539)** / **`primer-speech/src/macos/stt.rs` (504, macos-native-gated)** — medium/small, several feature-gated (dual-verify).
- Hardest: `primer-cli/src/main.rs` (1357, heavily `cfg(feature)`-gated — needs the per-feature clippy+test matrix).

The >500 test-support files (`store/tests/session_tests.rs` 2442, `state_machine/mocks.rs` 1380, `dialogue_manager/tests/*`, `kb-load/tests/common/mod.rs` 677, `test_support.rs` 655, `store/tests/learner_tests.rs` 607) remain lower-value than the production files.

**No re-ask needed — the production-split lane is owner-approved.** Only re-confirm if changing lanes back to docs/maintenance.

### The proven split recipe (two more clean runs this session)
1. Baseline: `cargo test -p <crate>` (with the right `--features` if tests are gated) green BEFORE touching anything; record the pass count.
2. Read the whole file; map natural responsibility boundaries.
3. Convert `foo.rs` → `foo/mod.rs` + siblings (or for an inline test module, `#[cfg(test)] mod tests;` + `foo/tests.rs` — the `macos26/analyzer` shape). `mod.rs` does `pub use submodule::{…}` so external paths are unchanged.
4. **The `use super::*` / `super::X` private-import trap, two-part fix (both parts exercised again this session):**
   - Keep private helpers/consts the test module reaches (`FALLBACK_LINE` this session) IN `mod.rs`.
   - For parent-level `use` aliases that test children resolve via `super::X` but production no longer needs, use a `#[cfg(test)] use …;` in `mod.rs` (new wrinkle this session — avoids unused-import clippy failure on non-test builds while keeping the test file 100% untouched).
5. Fix moved relative paths: prefer absolute `crate::…` imports in the new submodules over `super::super::…` chains.
6. Verify: pub-surface diff empty (`grep -oE 'pub (struct|enum|fn|const|async fn|trait|type) [A-Za-z_]+' | sort -u`, old vs new — **include `type`**, `DrainHook` is a `pub type`), crate suite count matches baseline, feature-combos clippy if cfg arms exist, full-workspace `cargo test`, clippy `-D warnings`, fmt `--check` (run fmt AFTER the split — dedents/moves trigger re-wraps).

### Carried / owner-or-hardware-gated (none host-completable autonomously)
- **#260** — Android-voice on-device acceptance (RedMagic 11 Pro + mic + quiet room).
- **#192** — manual macOS-native STT + injected non-AVSpeech TTS audio path (mic + macOS build).
- **#170 Stage B / E / F** — Supertonic voice-mode TTS + in-loop A/B numbers + Hindi preview→stable (see prior briefs for the promotion one-commit checklist; OpenRAIL-M clause (e) disclosure must ship before any default Supertonic flip).
- **#166 item #1** — owner-run WhisperStream reuse smoke (model + two 16 kHz WAVs; command at the bottom).
- **#135** — glib 0.18.5 → 0.20+ (blocked on Tauri 3).
- **Branch protection** — wire `cargo test (default features)` as a required status check on `main` (owner GitHub-settings call; still outstanding).
- QNN stable-token-across-reboots gate; NPU pedagogy/answer-quality tuning; latency-routing calibration.

## Open decisions / risks

- **PRs #317 + #318 open, awaiting owner review/merge.** Both pure refactors, no runtime behaviour change. #317 CI fully green; #318 CI pending at close. They touch different crates — no merge conflict either order.
- **The production-split lane stays open and owner-approved.** Recommended next: `primer-pedagogy/src/dialogue_manager/turn.rs` (746) — no feature gates, plain `cargo test -p primer-pedagogy` guard.
- **The inline-test detector has now fired 5 sessions total.** Between-sessions PRs (like #306) can push previously-fine files over 500. The sweep + detector are cheap — run both before picking work.
- **Machine load / build times:** deps were warm this session — `cargo test -p primer-comprehension` seconds, `cargo test -p primer-speech --features voice-loop` ~3 min, feature-combos clippy ~2 min, full workspace test ~10 min. Cold-start budget remains ~35 min for the first cargo pass (see prior brief). Run ONE cargo pass at a time; `cargo fmt` doesn't take the target lock — but **don't run fmt (source-modifying) while clippy is mid-flight on the same crate**; re-run the check after (this session did, cheaply).

## Patterns to reuse, not reinvent

- **The 6-step split recipe above** — new wrinkle this session: `#[cfg(test)] use …` in `mod.rs` for aliases only the test child needs (keeps clippy clean on non-test builds AND the test file untouched).
- **Pub-surface diff must include `pub type`** (this session's `DrainHook`). Full regex: `pub (struct|enum|fn|const|async fn|trait|type) [A-Za-z_]+`.
- **Test-extraction shape for a non-root flat module:** `#[cfg(test)] mod tests;` at the bottom of `foo.rs` + body in `foo/tests.rs` (mkdir), dedent one level, expect rustfmt re-wraps — apply `cargo fmt` and re-verify the count.
- **Doc-drift hunt recipe:** after a feature PR merges, grep its public symbols across README/ROADMAP/CLAUDE.
- **Use ABSOLUTE paths for shell tools** — Bash cwd resets between calls. `git show HEAD:<path>` wants the repo-root-relative path (`src/crates/…`).
- **A grep final pipe stage returns exit 1 on zero matches** — check the `test result:` / `Finished` lines, not the pipeline exit code.
- **Branch each PR off `main`, not off the previous branch.**
- **Run long cargo passes with `run_in_background: true`**; the harness notifies on completion — no polling needed.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git fetch && git log --oneline -4 && gh pr list --state open
# Two PRs open at close: #317 (comprehension test extraction, CI green), #318 (state_machine split, CI was pending).
# After they merge, start fresh off main.

# === Standard workspace gate (run from src/ if you touch .rs) — one cargo pass at a time, grep twice ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test --workspace 2>&1 | tee /tmp/ws.log | tail -3
grep -cE 'test result: ok' /tmp/ws.log            # expect 51
grep -E 'test result: FAILED|^error' /tmp/ws.log  # expect empty
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check

# === Oversized-file sweep + inline-test detector (re-verify EVERY session; wrong 5× now) ===
cd /Users/hherb/src/primer/src
find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"' | sort -rn
for f in $(find crates -name '*.rs' -not -path '*/vendor/*' -not -name 'tests.rs' | xargs wc -l | awk '$1>500 && $2!="total"{print $2}'); do \
  awk '/#\[cfg\(test\)\]/{ln=NR} ln && NR==ln+1 && /^[[:space:]]*mod .*\{/{print FILENAME" inline mod @"NR}' "$f"; done
# Empty inline output = no cheap pick. Next host-actionable work = next production split (owner-approved lane).

# === Recommended next split: primer-pedagogy/src/dialogue_manager/turn.rs (746). Baseline FIRST: ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-pedagogy 2>&1 | grep 'test result: ok'   # record pass count BEFORE splitting
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

- **PR #317 (comprehension llm test extraction):** the inline-test detector fired for the 5th time — PR #306 (merged between sessions) pushed `primer-comprehension/src/llm.rs` to 717 lines. Extracted to `llm/tests.rs` per the #295 precedent; 20/20 tests identical; CI fully green.
- **PR #318 (voice_loop state_machine split):** the recommended pick from the prior brief. 786 → `{mod 74, types 189, entry 90, inner 470}` behind a flat façade — 9-symbol external surface byte-identical, 101/0/2 lib tests (baseline match), zero churn in the 1380-line `mocks.rs` (the `#[cfg(test)] use` wrinkle), feature-combos + default clippy clean, workspace green (51 ok).
- **The production-split lane is open and owner-approved** — next pick `primer-pedagogy/src/dialogue_manager/turn.rs` (746, no feature gates) without re-asking.
- The GUI is a full app, not a scaffold.
