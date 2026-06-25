# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-25. Branch `docs/living-docs-refresh-2026-06-25` (pushed; **PR #265 open** against `main`, awaiting owner review/merge). `main` is at `81bc6f3`.

**The previous handoff is fully discharged:** its headline (push the `qnn-genie-status-header-confirm-223` branch + open a PR for #223) had already merged as **PR #264 (`81bc6f3`)** before this session opened. With no open PRs and every open issue owner/hardware/SDK-gated, the owner chose a **living-docs refresh** as this session's host-completable slice.

## What we shipped this session (branch `docs/living-docs-refresh-2026-06-25`)

- **`36d537b` — docs: refresh living docs for post-#244 merges.** Docs-only, **no behaviour change**. The last living-docs refresh was PR #244 (`cfac415`, 2026-06-15); this brings the developer manual + `CLAUDE.md` + `README.md` current with everything that merged since. Six files, +40/−4 lines.
  - **Android-native voice subsystem (#249–254, #261) — the biggest gap, previously undocumented in both the manual and CLAUDE.md.** Added a full **"Android-native speech backend"** section to [docs/devel/07-speech-and-voice-loop.md](docs/devel/07-speech-and-voice-loop.md) (module map table for `android/{bridge,capabilities,events,vad,stt,tts,jni_bridge,vm}.rs`, the cpal-free builder, GUI/Kotlin wiring, and 5 gotchas: classloader `GlobalRef` cache, main-Looper + poll model, `onEndOfSpeech`-not-enqueued ordering, recreate-per-arm + 12 s silent-dead-recognizer watchdog, permission-denied UX). Added a matching `CLAUDE.md` bullet + an `android-native` row to the ch07 feature-gate table.
  - **llamacpp per-model BOS (#201):** ch03 gotcha for `should_prepend_bos` / `BosDecision` (no double-BOS on Gemma/Llama-3 templates; the pure helpers are host-tested, only the real-model assertion is owner-gated).
  - **sweep harness split (#98/#245):** ch04 + ch08 linked the now-removed `tests/common/sweep.rs`; repointed both to the `sweep/{mod,bm25,hybrid}.rs` split.
  - **WhisperStream real-audio reuse smoke (#262):** `CLAUDE.md` said the #166 smoke was "tracked as a follow-up"; corrected — it landed as the `#[ignore]`'d `reused_stream_does_not_bleed_prior_utterance` (owner-gated to run; needs `PRIMER_WHISPER_MODEL` + two WAVs).
  - **README Android-voice status:** recreate-per-arm (#254) + watchdog (#259) are now device-confirmed; sustained 10-turn acceptance still pending (#260).

**What this session deliberately did NOT do:** touch any code. Every change is prose in `.md` files. ROADMAP sub-project 6 was reviewed and found already current (it covers #253/#254/#259/#260 through 2026-06-24) → no ROADMAP change.

### Verification — all asserted facts checked against source
- All linked file paths exist (`sweep/{mod,bm25,hybrid}.rs`, the 8 `android/` modules, `voice_android.rs`, `PrimerSpeech.kt`).
- All symbol names confirmed by grep: `should_rearm` / `should_force_rearm` / `arm_action` / `process_event`, `RECOGNIZER_WATCHDOG_TIMEOUT = 12 s` (`consts.rs:412`), `StartVoiceModeError::PermissionDenied`, `has_record_audio_permission`, `android_voice_available()`, `build_android_voice_backends`, `select_offline_voice`, `open_app_settings`, `should_prepend_bos` / `bos_decision` / `BosDecision`.
- **No host gate run** — docs-only, no `.rs` touched, so the fmt pre-commit hook fast-skips and `cargo test`/`clippy` are not implicated. (If you change any `.rs`, the standard workspace gate at the bottom applies.)

## What's next (concrete acceptance criteria)

### 0. ✅ Push + open the PR — DONE this session
- Pushed; **PR #265** is open against `main` (docs-only; no issue to close — a maintenance refresh). Only owner review/merge remains.

### 1. ✅ Stale branch cleanup — DONE this session
- The **`docs/refresh-living-docs-2026-06`** branch (local + `origin`) was deleted. A whole-tree diff proved it was byte-identical to `cfac415` — i.e. it **was** the dev branch already merged as PR #244, so deletion lost nothing. (The earlier "would delete code" worry was an artifact of it being 19 commits behind `main`, not deletions it authored.) No action remains.

### Carried / owner-or-hardware-gated (unchanged from prior brief — none host-completable autonomously)
- **#192** — manual macOS-native STT + injected non-AVSpeech TTS (Piper/Supertonic) audio path. Needs a mic + macOS build.
- **#170 Stage E** — in-loop A/B TTFA/RTF numbers for Supertonic vs Piper/macOS-native *inside* the voice loop. Needs a mic + audio bench. Overlaps #192.
- **#170 Stage F — Hindi preview→stable:** gated on (a) native-speaker review of `prompts/hi.toml` (grep `# REVIEW:`); (b) a Hindi corpus (none exists); (c) `tests/common/hi.rs` benchmark + retrieval/sweep tests mirroring EN/DE; (d) real-LLM smoke. The OpenRAIL-M **licence** sub-gate is done (PR #263). **Promotion is one commit when those clear:** add `Self::Hindi` to `Locale::ALL` (`primer-core/src/i18n.rs:59`); `status = "stable"` in `hi.toml`; bump `locale_all_excludes_hindi_until_translation_reviewed` (2→3); remove/invert `list_locales_excludes_preview_hindi` (`primer-gui/src/commands/settings.rs`); flip the Hindi README header PREVIEW→STABLE. **OpenRAIL-M clause (e) — an express "AI-generated voice" disclosure — must ship before any default Supertonic flip.**
- **#166 item #1** — owner-run the WhisperStream reuse-invariance smoke on a real model + two 16 kHz mono WAVs (commands at the bottom). Owner-gated.
- **#260 / #259** — Android-voice on-device acceptance: 10 consecutive clean voice turns, sustained ~10-min session, mid-session airplane toggle, no-barge-in. Needs the RedMagic 11 Pro + mic.
- **#135** — glib 0.18.5 → 0.20+ (RUSTSEC-2024-0429); blocked on Tauri 3 shipping.
- QNN stable-token-across-reboots gate; NPU pedagogy/answer-quality tuning; latency-routing calibration.

## Open decisions / risks

- **PR #265 open, awaiting owner review/merge** (item #0, done) — docs-only.
- **The host-actionable backlog is genuinely empty of feature work.** Every open issue needs a microphone, the RedMagic phone, a gated SDK, or upstream Tauri 3. When a session opens like this one (headline already merged, backlog gated), the highest-value host slice is documentation/maintenance — survey `gh issue list` + the "Carried" section, confirm with the owner, and pick the genuinely host-completable slice.
- ~~**The dead `docs/refresh-living-docs-2026-06` branch**~~ — deleted this session (item #1, done); it was PR #244's already-merged dev branch.

## Patterns to reuse, not reinvent

- **Date the last doc refresh by the commit, not the file mtime.** `git log --oneline --grep 'refresh living docs'` found `cfac415` (PR #244) as the baseline; the true doc gaps are exactly `git log cfac415..main`. This scopes a "refresh the docs" task to a tight, verifiable set instead of re-auditing all nine chapters.
- **Verify every symbol/path before writing it into docs.** This session grepped each asserted function name, const, and file path against source (one `Bash` batch) and caught `should_re_arm` → `should_rearm` before it shipped. Docs that cite wrong symbol names are worse than no docs.
- **Dispatch an Explore agent for a large undocumented subsystem.** The Android voice subsystem (9 Rust modules + Kotlin bridge + GUI wiring) was mapped by one Explore agent returning exact file:line + gotchas, which fed the docs directly — far faster than reading ~15 files inline.
- **CLAUDE.md is authoritative for agent-facing conventions; the devel manual is authoritative for narrative.** When one covers something the other doesn't (here: neither covered Android voice), update *both* and keep them consistent — that's the contract in `docs/devel/index.md`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git fetch && git log --oneline -4
git checkout docs/living-docs-refresh-2026-06-25            # this session's branch (36d537b)
# Item #0 — push + PR: git push -u origin docs/living-docs-refresh-2026-06-25 && gh pr create
# Item #1 — done this session (dead branch docs/refresh-living-docs-2026-06 deleted)

# === Standard workspace gate (only if you touch .rs) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check

# === Carried: owner-run the #166 reuse smoke (needs a model + two 16 kHz mono WAVs) ===
PRIMER_WHISPER_MODEL=/path/to/ggml-small.en.bin \
PRIMER_WHISPER_AUDIO_A=/path/to/utterance_a.wav \
PRIMER_WHISPER_AUDIO_B=/path/to/utterance_b.wav \
  ~/.cargo/bin/cargo test -p primer-speech --features whisper \
  --test whisper_stream_reuse -- --ignored --nocapture
```

## Reporting back

- Living docs are now current with all post-#244 merges. The Android-native voice subsystem (#249–254, #261) is documented for the first time (ch07 + CLAUDE.md); llamacpp per-model BOS (#201), the sweep-harness split (#98), the WhisperStream reuse smoke (#262), and the README Android-voice status were all refreshed. Commit `36d537b`; push + PR is owner-authorised item #0.
- No behaviour changed; no code touched; ROADMAP needed no change.
- The GUI is a full app, not a scaffold.
