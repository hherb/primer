# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-25. Branch `refactor/voice-loop-extract-pure-helpers` (pushed; **PR #266 open** against `main`, awaiting owner review/merge). `main` is at `8f4edb0` (PR #265, the prior living-docs refresh, is merged).

**The previous handoff is fully discharged:** PR #265 (living-docs refresh) merged as `8f4edb0`. As before, the host-actionable *feature* backlog is empty — every open issue is gated on a microphone, the RedMagic phone, a gated SDK, or upstream Tauri 3. The owner chose a **bookkeeping + small refactor** slice for this session.

## What we shipped this session

- **Closed issue #259** (Android voice silent-dead-recognizer wedge) as **resolved**. The liveness-watchdog fix (`should_force_rearm` + `RECOGNIZER_WATCHDOG_TIMEOUT = 12 s` at `primer-core/src/consts.rs:412`, wired into `run_recognizer_loop` in `primer-speech/src/android/stt.rs`) shipped in **PR #261 (`12f82ec`)** and was device-confirmed recovering on the RedMagic 11 Pro (2026-06-24, per #260). The remaining 10-turn / sustained-session / stress acceptance stays tracked in **#260**.

- **`aeebed5` — refactor(speech): extract pure helpers from `state_machine.rs`** (branch `refactor/voice-loop-extract-pure-helpers`, **PR #266**). Pure relocation, **no behaviour change**. `voice_loop/state_machine.rs` was 2520 lines (well over the ~500 guideline); the two clusters of pure helpers the loop merely *calls* on the commit boundary are now leaf modules:
  - `voice_loop/tts_markdown.rs` — `strip_markdown_for_tts` (+ private `consume_digit_times`, `find_paired_marker`) + 9 tests.
  - `voice_loop/quit_detect.rs` — `is_quit_phrase` (+ private `quit_phrases_for`, `normalise_for_match`) + 6 tests.
  - Each exposes only its single entry point via `pub(super)`; `state_machine.rs` now `use super::{quit_detect::is_quit_phrase, tts_markdown::strip_markdown_for_tts}` and drops to **2202 lines**. Follows the established `voice_loop/` "one concern per file" pattern (cf. the `backends_*` modules).

**What this session deliberately did NOT do:** no behaviour change, no new feature. The refactor moves identical function bodies and identical test assertions. README/ROADMAP needed no change (status/features/phases are unaffected by an internal module split).

### Verification — all gates green
- `cargo test --workspace` (default features): **0 failures**.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo fmt --all -- --check`: clean.
- `cargo test -p primer-speech --features voice-loop --lib`: **101 passed** (all 15 relocated tests confirmed running under `voice_loop::{quit_detect,tts_markdown}::tests::*`).
- `cargo clippy -p primer-speech --features voice-loop --all-targets -- -D warnings`: clean.

## What's next (concrete acceptance criteria)

### 0. ✅ Push + open the PR — DONE this session
- Pushed; **PR #266** is open against `main` (refactor; no issue to close). Only owner review/merge remains.

### Further `voice_loop` refactor candidates (host-completable, same pattern)
If another low-risk refactor slice is wanted, `state_machine.rs` (still 2202 lines) is dominated by a large inline `#[cfg(test)] mod mocks { … }` (~1400 lines) plus the loop body. Two clean follow-ups:
- **Move `mod mocks` to a sibling file** (`voice_loop/state_machine_mocks.rs` via `#[cfg(test)] mod mocks;` or `#[path]`), which would drop `state_machine.rs` to ~800 lines (the actual loop). Lowest-risk; test-only code.
- Other oversized non-test files worth a future split: `primer-cli/src/main.rs` (2196), `primer-pedagogy/src/prompt_pack.rs` (2072), `prompt_builder.rs` (2042), `primer-gui/src/config.rs` (1985). Each is bigger-surface; scope carefully and keep it test-guarded.

### Carried / owner-or-hardware-gated (none host-completable autonomously)
- **#260** — Android-voice on-device acceptance: 10 consecutive clean voice turns, sustained ~10-min session, mid-session airplane toggle, no-barge-in, leading-word-clip check. Needs the RedMagic 11 Pro + mic + quiet room.
- **#192** — manual macOS-native STT + injected non-AVSpeech TTS (Piper/Supertonic) audio path. Needs a mic + macOS build.
- **#170 Stage E** — in-loop A/B TTFA/RTF numbers for Supertonic vs Piper/macOS-native *inside* the voice loop. Needs a mic + audio bench. Overlaps #192.
- **#170 Stage F — Hindi preview→stable:** gated on (a) native-speaker review of `prompts/hi.toml` (grep `# REVIEW:`); (b) a Hindi corpus (none exists); (c) `tests/common/hi.rs` benchmark + retrieval/sweep tests mirroring EN/DE; (d) real-LLM smoke. Licence sub-gate done (PR #263). **Promotion is one commit when those clear:** add `Self::Hindi` to `Locale::ALL` (`primer-core/src/i18n.rs:59`); `status = "stable"` in `hi.toml`; bump `locale_all_excludes_hindi_until_translation_reviewed` (2→3); remove/invert `list_locales_excludes_preview_hindi` (`primer-gui/src/commands/settings.rs`); flip the Hindi README header PREVIEW→STABLE. **OpenRAIL-M clause (e) — an express "AI-generated voice" disclosure — must ship before any default Supertonic flip.**
- **#166 item #1** — owner-run the WhisperStream reuse-invariance smoke on a real model + two 16 kHz mono WAVs (commands at the bottom). Owner-gated.
- **#135** — glib 0.18.5 → 0.20+ (RUSTSEC-2024-0429); blocked on Tauri 3 shipping.
- QNN stable-token-across-reboots gate; NPU pedagogy/answer-quality tuning; latency-routing calibration.

## Open decisions / risks

- **PR #266 open, awaiting owner review/merge** (item #0, done) — refactor-only, no behaviour change.
- **The host-actionable feature backlog remains genuinely empty.** Every open issue needs a microphone, the RedMagic phone, a gated SDK, or upstream Tauri 3. When a session opens like this one, the highest-value host slices are (in order): clean bookkeeping (close resolved-but-open issues), then a small test-guarded refactor of an oversized file, then docs/maintenance. Survey `gh issue list` + the "Carried" section and confirm direction with the owner before picking.

## Patterns to reuse, not reinvent

- **Close issues whose *fix* shipped even if the *acceptance* is gated.** #259's watchdog landed in #261 and was device-confirmed; the residual 10-turn acceptance is a separate issue (#260). Closing #259 with a comment pointing at #260 keeps the tracker honest without waiting on hardware.
- **For a refactor, the existing tests ARE the regression guard.** This session moved pure helpers + their characterisation tests verbatim and relied on `cargo test` to prove byte-identical behaviour — no new tests authored, none needed. Verify the relocated tests actually run under their new module path (grep the test output for `voice_loop::tts_markdown::tests::…`) — a silently-uncompiled test module is the failure mode.
- **Match the crate's own module convention.** `voice_loop/` already splits one concern per file (`backends_*`, `selectors`, `channel_stt`); the new `tts_markdown` / `quit_detect` modules slot into that pattern rather than inventing a new layout.
- **Date the last refactor/doc baseline by the commit, not file mtime.** `git log --grep` finds the true baseline; `git log <baseline>..main` scopes the real gap.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git fetch && git log --oneline -4
git checkout refactor/voice-loop-extract-pure-helpers       # this session's branch (aeebed5)
# Item #0 — push + PR: DONE (PR #266 open against main)

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

- Closed #259 (watchdog shipped + device-confirmed via #261/#260). Refactored `voice_loop/state_machine.rs` (2520 → 2202 lines) by extracting two pure-helper leaf modules (`tts_markdown.rs`, `quit_detect.rs`) with their 15 tests. Commit `aeebed5`; PR #266 open against `main`.
- No behaviour changed; README/ROADMAP needed no change.
- The GUI is a full app, not a scaffold.
