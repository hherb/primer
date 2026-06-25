# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-25. Branch `refactor/extract-cli-support-from-main` (pushed; **PR #269 open** against `main`, awaiting owner review/merge). `main` is at `5caa50f` (PR #268, the state_machine/mocks split, is merged).

**The previous handoff is fully discharged:** PR #268 (state_machine mocks extraction) merged as `5caa50f`. As before, the host-actionable *feature* backlog is empty — every open issue is gated on a microphone, the RedMagic phone, a gated SDK, or upstream Tauri 3. The owner picked the next **low-risk refactor** slice: oversized-file reduction of `primer-cli/src/main.rs`.

## What we shipped this session

- **`c15a0d0` — refactor(cli): extract CLI parse/validation helpers + tests from main.rs** (branch `refactor/extract-cli-support-from-main`, **PR #269**). Near-verbatim relocation, **one incidental compile fix** (see below).
  - `primer-cli/src/main.rs` was **2196 lines**. Moved the argument parse/validation helpers (`parse_mic_silence_ms`, `TtsChoice` + `From`, `validate_speech_assets`, `warn_on_npu_serialisation`, `npu_serialisation_warning`, `pair_reasoning_markers`) into a new `cli_support.rs` (273 lines), and the four `#[cfg(test)]` modules into `cli_support/tests.rs` (620 lines).
  - **main.rs drops 2196 → 1357 lines (−839).** It now holds only the runtime wiring (`main` / `run_tokio_on_main` / `async_main` / `probe_espeak_ng_data` / `DEFAULT_LOG_FILTER`) plus the `Cli` struct.
  - **Key low-risk move:** the `Cli` struct **stays in the crate root with every field visibility unchanged**. The moved helpers reach `Cli`'s *private* fields via **descendant-module visibility** (a child module may read an ancestor module's private items), so **zero `pub(crate)` field churn** was needed. Only the six moved fns/enum got `pub(crate)` (for the parent `use`); `warn_on_npu_serialisation(cli: &crate::Cli)` is fully-qualified to avoid re-exporting `Cli` through the glob.
  - Used the idiomatic Rust-2018 `cli_support.rs` + `cli_support/tests.rs` subdirectory form (`#[cfg(test)] mod tests;`), mirroring last session's `state_machine.rs` + `state_machine/mocks.rs`.

- **Incidental fix (rode with the refactor per [[feedback_inline_quick_fixes]]):** `validate_speech_assets`'s `match` only covered `Piper`/`Supertonic`/`MacosNative`, but `TtsBackend::AndroidNative` (added in the recent android-native work, #253–#260) made the portable `--features speech` build **fail to compile** with E0004 — a pre-existing latent gap **not in CI's required gate** (confirmed: `git show HEAD:src/.../main.rs` has the same 3-arm match). Collapsed the two unreachable platform-native arms into `MacosNative | AndroidNative`. **Zero behaviour change** — the CLI's `TtsChoice` only produces `Piper`/`Supertonic`; the native arms are unreachable on this build.

**What this session deliberately did NOT do:** no feature, no behaviour change to the runtime. README/ROADMAP needed no change (status/features/phases/flags unaffected by an internal file split; the compile fix is invisible to users).

### Verification — all gates green
- `cargo test --workspace` (default features): **0 failures**.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo fmt --all -- --check`: clean.
- Per-feature (the cfg-heavy paths the split touches), clippy `-D warnings` + tests all green:
  - default: 23 primer-cli tests (relocated under `cli_support::tests::*`).
  - `--features speech`: clippy clean, 33 tests.
  - `--features speech,macos-native`: clippy clean, 27 tests (incl. the macos-native-only parse test).
  - `--features qnn`: clippy clean, 27 tests (incl. the qnn-gated parse tests).

## What's next (concrete acceptance criteria)

### 0. ✅ Push + open the PR — DONE this session
- Pushed; **PR #269** is open against `main` (refactor; no issue to close). Only owner review/merge remains.

### Further oversized-file refactor candidates (host-completable, same pattern)
If another low-risk refactor slice is wanted, these non-test files are still over the ~500-line guideline (line counts as of this session):
- `primer-cli/src/main.rs` (**now 1357**, down from 2196 — still over; the residue is the `Cli` struct [~450 lines, mostly per-field doc comments — inherently indivisible] + `async_main`'s linear setup. A further cut would mean moving the `Cli` struct itself to `cli_support.rs`, which requires `pub(crate)` on ~40 fields because `async_main` (the parent) would then be reading a *child's* private fields — the inverse of this session's no-churn trick. Higher surface; scope deliberately.)
- `primer-pedagogy/src/prompt_pack.rs` (2072)
- `primer-pedagogy/src/prompt_builder.rs` (2042)
- `primer-gui/src/config.rs` (1985)

Each is bigger-surface (production code). Look for a self-contained cluster (a `mod`, an impl block, a related group of pure helpers) that moves verbatim. The descendant-visibility trick (move *helpers/tests* down, keep the *struct* in the parent) is the low-churn play; moving a struct *down* costs field-visibility edits.

### Carried / owner-or-hardware-gated (none host-completable autonomously)
- **#260** — Android-voice on-device acceptance: 10 consecutive clean voice turns, sustained ~10-min session, mid-session airplane toggle, no-barge-in, leading-word-clip check. Needs the RedMagic 11 Pro + mic + quiet room.
- **#192** — manual macOS-native STT + injected non-AVSpeech TTS (Piper/Supertonic) audio path. Needs a mic + macOS build.
- **#170 Stage B / E / F** — Supertonic voice-mode TTS backend + in-loop A/B TTFA/RTF numbers + Hindi preview→stable. Stage E needs a mic + audio bench (overlaps #192). Stage F is gated on (a) native-speaker review of `prompts/hi.toml` (grep `# REVIEW:`); (b) a Hindi corpus (none exists); (c) `tests/common/hi.rs` benchmark + retrieval/sweep tests mirroring EN/DE; (d) real-LLM smoke. Licence sub-gate done (PR #263). **Promotion is one commit when those clear:** add `Self::Hindi` to `Locale::ALL` (`primer-core/src/i18n.rs:59`); `status = "stable"` in `hi.toml`; bump `locale_all_excludes_hindi_until_translation_reviewed` (2→3); remove/invert `list_locales_excludes_preview_hindi` (`primer-gui/src/commands/settings.rs`); flip the Hindi README header PREVIEW→STABLE. **OpenRAIL-M clause (e) — an express "AI-generated voice" disclosure — must ship before any default Supertonic flip.**
- **#166 item #1** — owner-run the WhisperStream reuse-invariance smoke on a real model + two 16 kHz mono WAVs (commands at the bottom). Owner-gated.
- **#135** — glib 0.18.5 → 0.20+ (RUSTSEC-2024-0429); blocked on Tauri 3 shipping.
- QNN stable-token-across-reboots gate; NPU pedagogy/answer-quality tuning; latency-routing calibration.

## Open decisions / risks

- **PR #269 open, awaiting owner review/merge** (item #0, done) — refactor + one incidental compile fix, no runtime behaviour change.
- **The incidental `AndroidNative` match fix touches non-test production code.** It is unreachable on the portable `--features speech` build (the CLI's `TtsChoice` has no native arm), so it is pure compile-hygiene; but flag it for the reviewer since it rode in with a refactor PR. If the owner prefers refactor-only PRs, the fix is trivially separable.
- **CI gap surfaced:** `cargo clippy -p primer-cli --features speech` was NOT in the required gate, which is why the `AndroidNative` non-exhaustive match slipped in. Worth a CI follow-up (add a `primer-cli --features speech` clippy step) so the next platform-native `TtsBackend` variant fails CI, not a local clippy run. Not done this session — out of scope for the refactor; raise with the owner.
- **The host-actionable feature backlog remains genuinely empty.** Every open issue needs a microphone, the RedMagic phone, a gated SDK, or upstream Tauri 3. When a session opens like this one, the highest-value host slices are (in order): clean bookkeeping (close resolved-but-open issues — there are none right now), then a small test-guarded refactor of an oversized file, then docs/maintenance. Survey `gh issue list` + the "Carried" section and **confirm direction with the owner before picking** (the owner picked the main.rs refactor this session).

## Patterns to reuse, not reinvent

- **Descendant-module visibility is the low-churn extraction lever.** To shrink a big module that owns a struct + helpers + tests: move the *helpers and tests* into a child module and keep the *struct* in the parent. Child modules can read the parent's private items, so the struct's fields stay private and untouched. Moving the *struct* down instead forces `pub(crate)` on every field the parent still reads — avoid unless that's the explicit goal.
- **For a refactor, the existing tests ARE the regression guard.** This session relied on a pre-move baseline (`cargo test -p primer-cli --bins` = 23 passing) and confirmed the identical 23 pass post-move under their new `cli_support::tests::*` paths. No new tests authored, none needed.
- **Extract big blocks with `sed` line-range moves, not hand-transcription.** A 600-line verbatim relocation is exactly where Edit/Write transcription silently corrupts a line. `sed -n 'A,Bp' >> newfile` then patch the few import/visibility lines is reliable and reviewable.
- **`clippy::module_inception` bites the `foo.rs` + `foo/tests.rs` form** if the inner module is also named `tests` (you get `cli_support::tests::tests`). Rename the inner module (here `cli_parse_tests`).
- **Exercise every cfg branch a split touches.** The cfg-gated imports (`#[cfg(speech)] use …`, `#[cfg(qnn)] use std::path::Path`) are invisible to a default `cargo test`. Run clippy `-D warnings` + tests for default / speech / speech+macos-native / qnn — an unused-import-under-one-feature-combo is the classic silent failure, and CI's feature-combo jobs will catch what a default run misses.
- **Close issues whose *fix* shipped even if the *acceptance* is gated.** (No such issue this session — all four open issues are genuinely hardware/owner/upstream-gated.)

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git fetch && git log --oneline -4
git checkout refactor/extract-cli-support-from-main          # this session's branch (c15a0d0)
# Item #0 — push + PR: DONE (PR #269 open against main)

# === Standard workspace gate (run from src/ if you touch .rs) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test --workspace
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check

# === primer-cli is cfg-heavy — exercise the feature branches the split touches ===
~/.cargo/bin/cargo test  -p primer-cli --bins                              # default (23 tests)
~/.cargo/bin/cargo clippy -p primer-cli --features speech --all-targets -- -D warnings
~/.cargo/bin/cargo test  -p primer-cli --features speech --bins            # 33 tests
~/.cargo/bin/cargo clippy -p primer-cli --features speech,macos-native --all-targets -- -D warnings   # macOS only
~/.cargo/bin/cargo clippy -p primer-cli --features qnn --all-targets -- -D warnings

# === Carried: owner-run the #166 reuse smoke (needs a model + two 16 kHz mono WAVs) ===
PRIMER_WHISPER_MODEL=/path/to/ggml-small.en.bin \
PRIMER_WHISPER_AUDIO_A=/path/to/utterance_a.wav \
PRIMER_WHISPER_AUDIO_B=/path/to/utterance_b.wav \
  ~/.cargo/bin/cargo test -p primer-speech --features whisper \
  --test whisper_stream_reuse -- --ignored --nocapture
```

## Reporting back

- Extracted `primer-cli/src/main.rs`'s CLI parse/validation helpers + 4 test modules into `cli_support.rs` (273) + `cli_support/tests.rs` (620). main.rs dropped **2196 → 1357 lines (−839)**. The `Cli` struct stayed in the crate root with field visibilities unchanged (descendant-module visibility). Commit `c15a0d0`; **PR #269** open against `main`.
- One incidental compile fix: added the missing `TtsBackend::AndroidNative` arm to `validate_speech_assets` (the portable `--features speech` build was pre-existingly broken; unreachable arm, zero behaviour change).
- No runtime behaviour changed; default tests pass identically (23). README/ROADMAP needed no change.
- The GUI is a full app, not a scaffold.
