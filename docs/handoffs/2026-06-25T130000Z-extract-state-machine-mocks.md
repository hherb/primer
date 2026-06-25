# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-25. Branch `refactor/extract-state-machine-mocks` (pushed; **PR #268 open** against `main`, awaiting owner review/merge). `main` is at `1c7e576` (PRs #266 + the #267 doc follow-up are both merged).

**The previous handoff is fully discharged:** PR #266 (pure-helper extraction) merged as `c345285`; its doc follow-up #267 merged as `1c7e576`. As before, the host-actionable *feature* backlog is empty — every open issue is gated on a microphone, the RedMagic phone, a gated SDK, or upstream Tauri 3. The owner chose the next **low-risk refactor** slice (the one the prior handoff flagged as the top follow-up).

## What we shipped this session

- **`96991e2` — refactor(speech): extract state_machine test mocks+tests to sibling file** (branch `refactor/extract-state-machine-mocks`, **PR #268**). Pure relocation, **no behaviour change**. `voice_loop/state_machine.rs` was 2202 lines, dominated by a single ~1418-line `#[cfg(test)] mod mocks { … }` block (mock STT/TTS backends + every `#[tokio::test]` for the loop). That block moved verbatim into a new sibling file `voice_loop/state_machine/mocks.rs`, declared in `state_machine.rs` as `#[cfg(test)] mod mocks;`.
  - The module stays `voice_loop::state_machine::mocks`, so `super::` still resolves to the `state_machine` module and every test reaches the loop's public types (`LoopBackends`, `VoiceState`, `run_loop_borrowed`, …) unchanged.
  - `state_machine.rs` drops **2202 → 786 lines** (the actual loop body + public types). Follows the established `voice_loop/` "one concern per file" pattern.
  - Used the idiomatic Rust-2018 `state_machine.rs` + `state_machine/mocks.rs` subdirectory form (no `#[path]` attribute needed).

**What this session deliberately did NOT do:** no behaviour change, no new feature. The refactor moves an identical module body + identical test assertions. README/ROADMAP needed no change (status/features/phases are unaffected by an internal test-file split).

### Verification — all gates green
- `cargo test -p primer-speech --features voice-loop --lib`: **101 passed, 2 ignored** — identical to baseline; all relocated tests confirmed running under `voice_loop::state_machine::mocks::*`.
- `cargo test --workspace` (default features): **0 failures**.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo clippy -p primer-speech --features voice-loop --all-targets -- -D warnings`: clean.
- `cargo fmt --all -- --check`: clean.

## What's next (concrete acceptance criteria)

### 0. ✅ Push + open the PR — DONE this session
- Pushed; **PR #268** is open against `main` (refactor; no issue to close). Only owner review/merge remains.

### Further oversized-file refactor candidates (host-completable, same pattern)
If another low-risk refactor slice is wanted, these non-test files are still over the ~500-line guideline (line counts as of this session):
- `primer-cli/src/main.rs` (2196)
- `primer-pedagogy/src/prompt_pack.rs` (2072)
- `primer-pedagogy/src/prompt_builder.rs` (2042)
- `primer-gui/src/config.rs` (1985)

Each is bigger-surface (production code, not test-only) — scope carefully, keep it test-guarded, and look for a self-contained cluster (a `mod`, an impl block, a related group of pure helpers) that moves verbatim. The two test-only easy wins (`state_machine.rs` mocks, and #266's pure helpers) are now done; what remains needs more thought than a verbatim relocation.

### Carried / owner-or-hardware-gated (none host-completable autonomously)
- **#260** — Android-voice on-device acceptance: 10 consecutive clean voice turns, sustained ~10-min session, mid-session airplane toggle, no-barge-in, leading-word-clip check. Needs the RedMagic 11 Pro + mic + quiet room.
- **#192** — manual macOS-native STT + injected non-AVSpeech TTS (Piper/Supertonic) audio path. Needs a mic + macOS build.
- **#170 Stage B / E / F** — Supertonic voice-mode TTS backend + in-loop A/B TTFA/RTF numbers + Hindi preview→stable. Stage E needs a mic + audio bench (overlaps #192). Stage F is gated on (a) native-speaker review of `prompts/hi.toml` (grep `# REVIEW:`); (b) a Hindi corpus (none exists); (c) `tests/common/hi.rs` benchmark + retrieval/sweep tests mirroring EN/DE; (d) real-LLM smoke. Licence sub-gate done (PR #263). **Promotion is one commit when those clear:** add `Self::Hindi` to `Locale::ALL` (`primer-core/src/i18n.rs:59`); `status = "stable"` in `hi.toml`; bump `locale_all_excludes_hindi_until_translation_reviewed` (2→3); remove/invert `list_locales_excludes_preview_hindi` (`primer-gui/src/commands/settings.rs`); flip the Hindi README header PREVIEW→STABLE. **OpenRAIL-M clause (e) — an express "AI-generated voice" disclosure — must ship before any default Supertonic flip.**
- **#166 item #1** — owner-run the WhisperStream reuse-invariance smoke on a real model + two 16 kHz mono WAVs (commands at the bottom). Owner-gated.
- **#135** — glib 0.18.5 → 0.20+ (RUSTSEC-2024-0429); blocked on Tauri 3 shipping.
- QNN stable-token-across-reboots gate; NPU pedagogy/answer-quality tuning; latency-routing calibration.

## Open decisions / risks

- **PR #268 open, awaiting owner review/merge** (item #0, done) — refactor-only, no behaviour change.
- **The host-actionable feature backlog remains genuinely empty.** Every open issue needs a microphone, the RedMagic phone, a gated SDK, or upstream Tauri 3. When a session opens like this one, the highest-value host slices are (in order): clean bookkeeping (close resolved-but-open issues — there are none right now), then a small test-guarded refactor of an oversized file, then docs/maintenance. Survey `gh issue list` + the "Carried" section and **confirm direction with the owner before picking** (the owner picked the refactor this session).

## Patterns to reuse, not reinvent

- **For a refactor, the existing tests ARE the regression guard.** This session moved a whole test module + its mocks verbatim and relied on `cargo test` to prove byte-identical behaviour — no new tests authored, none needed. **Verify the relocated tests actually run under their new module path** (grep the test output for `voice_loop::state_machine::mocks::…`, and confirm the pass count is identical to a pre-move baseline) — a silently-uncompiled test module is the failure mode.
- **Keep submodule paths stable when extracting tests.** The `mocks` module reaches the production code via `super::`; relocating it as a *submodule of the same parent* (`state_machine/mocks.rs` declared `#[cfg(test)] mod mocks;`) keeps `super::` resolving correctly with zero edits to the moved code. Moving it *up* to a sibling of `state_machine` would have broken every `super::` reference — don't.
- **Prefer the idiomatic `foo.rs` + `foo/` subdirectory over `#[path]`.** `state_machine.rs` + `state_machine/mocks.rs` with a plain `mod mocks;` needs no `#[path]` attribute and matches Rust-2018 conventions.
- **Match the crate's own module convention.** `voice_loop/` already splits one concern per file (`backends_*`, `selectors`, `channel_stt`, and now `state_machine/mocks`).
- **Close issues whose *fix* shipped even if the *acceptance* is gated.** (No such issue this session — all four open issues are genuinely hardware/owner/upstream-gated, not resolved-but-open.)

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git fetch && git log --oneline -4
git checkout refactor/extract-state-machine-mocks          # this session's branch (96991e2)
# Item #0 — push + PR: DONE (PR #268 open against main)

# === Standard workspace gate (run from src/ if you touch .rs) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test --workspace
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check
# voice_loop is feature-gated, so ALSO exercise it explicitly when touching it:
~/.cargo/bin/cargo test -p primer-speech --features voice-loop --lib
~/.cargo/bin/cargo clippy -p primer-speech --features voice-loop --all-targets -- -D warnings

# === Carried: owner-run the #166 reuse smoke (needs a model + two 16 kHz mono WAVs) ===
PRIMER_WHISPER_MODEL=/path/to/ggml-small.en.bin \
PRIMER_WHISPER_AUDIO_A=/path/to/utterance_a.wav \
PRIMER_WHISPER_AUDIO_B=/path/to/utterance_b.wav \
  ~/.cargo/bin/cargo test -p primer-speech --features whisper \
  --test whisper_stream_reuse -- --ignored --nocapture
```

## Reporting back

- Refactored `voice_loop/state_machine.rs` (2202 → 786 lines) by extracting its single ~1418-line `#[cfg(test)] mod mocks` (mocks + all `#[tokio::test]`s) into the new sibling `voice_loop/state_machine/mocks.rs`, declared `#[cfg(test)] mod mocks;`. Commit `96991e2`; PR #268 open against `main`.
- No behaviour changed; tests pass identically (101); README/ROADMAP needed no change.
- The GUI is a full app, not a scaffold.
