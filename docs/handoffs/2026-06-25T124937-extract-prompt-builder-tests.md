# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-25. Branch `refactor/extract-prompt-builder-tests` (pushed; **PR #271 open** against `main`, awaiting owner review/merge). `main` is at `8e7765b` (PR #270, the `prompt_pack.rs` test extraction, is merged).

**The previous handoff is fully discharged:** PR #270 (`prompt_pack.rs` test extraction) merged as `8e7765b`. The host-actionable *feature* backlog remains empty — every open issue is gated on a microphone, the RedMagic phone, a gated SDK, or upstream Tauri 3. The owner again picked a **low-risk, test-guarded refactor** slice: oversized-file reduction of `primer-pedagogy/src/prompt_builder.rs` (the fourth test-module extraction in a row, after #268/#269/#270).

## What we shipped this session

- **`4de5a5a` — refactor(pedagogy): extract prompt_builder tests to sibling file** (branch `refactor/extract-prompt-builder-tests`, **PR #271**). Pure verbatim relocation, **no behaviour change, no incidental fixes**.
  - `primer-pedagogy/src/prompt_builder.rs` was **2042 lines**, dominated by a ~1390-line `#[cfg(test)] mod tests` (lines 651–2042) — characterization tests pinning `decide_intent`'s Socratic-intent heuristic plus the prompt-building tests. Moved that module verbatim into a new sibling `prompt_builder/tests.rs` (1385 lines after `cargo fmt`), using the idiomatic Rust-2018 `prompt_builder.rs` + `prompt_builder/tests.rs` subdirectory form — same pattern as PRs #268/#269/#270.
  - **prompt_builder.rs drops 2042 → 652 lines (−1390).** It now holds only the production code (intent/prompt helpers, `decide_intent*` family, factual-question helpers) plus the `#[cfg(test)] fn is_factual_question` test helper and a 2-line `#[cfg(test)] mod tests;` declaration.
  - **Zero visibility churn.** As a child module, the tests still reach every private parent item via `use super::*`. The `#[cfg(test)] fn is_factual_question` helper (a parent-level cfg(test) free fn used only by tests) **stays in the parent** — descendant modules read ancestor items regardless of visibility, so no signature changed.
  - **Verbatim discipline:** extracted lines 653–2041 with `sed -n`, trimmed the parent with `head -n 650`, appended `#[cfg(test)]\nmod tests;`, then `cargo fmt` to dedent the body one level. The only hand-edit was removing the single leading blank line `cargo fmt --check` flagged before the file-level `//!` doc comment.

**What this session deliberately did NOT do:** no feature, no behaviour change, no incidental production-code fix (100% test-only — even cleaner than #269). README/ROADMAP needed no change: the only README mention of `prompt_builder` (line 150) is a conceptual description of *what the module does*, unaffected by where its test code lives; ROADMAP has no reference. prompt_builder.rs is still 652 lines (over the ~500 guideline) — a further *production-code* cut would need `pub(super)` churn and is scoped out (see "What's next").

### Verification — all gates green
- `cargo test -p primer-pedagogy --lib prompt_builder`: **62 passed** (62 before = 62 after — the regression guard; "existing tests ARE the regression guard").
- `cargo test --workspace` (default features): **0 failures** (51 "test result: ok", same as last session).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo fmt --all -- --check`: clean.
- No `cfg(feature …)` in the moved span, so the default run fully covers this crate — no per-feature matrix needed (simpler than the cfg-heavy `primer-cli`).

## What's next (concrete acceptance criteria)

### 0. ✅ Push + open the PR — DONE this session
- Pushed; **PR #271** is open against `main` (refactor; no issue to close). Only owner review/merge remains.

### Further oversized-file refactor candidates (host-completable, same pattern)
Non-test files still over the ~500-line guideline (line counts as of this session):
- `primer-gui/src/config.rs` (**1985**) — likely many serde DTOs + defaults; test module is lines ~900–1985 (~1085 lines). **Has 2 `cfg(feature)` gates** — check whether either falls in the moved span; if so, run the per-feature clippy+test matrix to verify, not just the default run. Otherwise the cleanest next cut.
- `primer-pedagogy/src/prompt_builder.rs` (**now 652**, down from 2042 — still over 500). Residue is all production code (`decide_intent*` family + prompt/intent helpers). A further cut would move a helper cluster down to a `prompt_builder/<x>.rs`, but the parent calls those helpers, so each would need `pub(super)` — the inverse of this session's no-churn test move. Higher surface; scope deliberately if picked.
- `primer-pedagogy/src/prompt_pack.rs` (**894**) — production residue after PR #270; a further cut moves the validation/intent/template helper cluster (~lines 613–892) to `prompt_pack/parse.rs`, each needing `pub(super)` (parent `from_toml` calls them). Higher surface.
- `primer-cli/src/main.rs` (**1357**) — residue is the `Cli` struct (~450 lines of per-field doc comments, indivisible) + `async_main`'s linear setup. Moving `Cli` down costs ~40 `pub(crate)` field edits. Scope deliberately.

**The low-churn play every time:** move the *test module* (and/or standalone helpers the parent doesn't call) DOWN into a child; keep the *struct + parent-called helpers* in the parent. Moving a struct or a parent-called helper down forces `pub(super)`/`pub(crate)` on it — only do that when it's the explicit goal. **Four PRs in a row (#268/#269/#270/#271) used exactly the test-module move.**

### Carried / owner-or-hardware-gated (none host-completable autonomously)
- **#260** — Android-voice on-device acceptance: 10 consecutive clean voice turns, sustained ~10-min session, mid-session airplane toggle, no-barge-in, leading-word-clip check. Needs the RedMagic 11 Pro + mic + quiet room.
- **#192** — manual macOS-native STT + injected non-AVSpeech TTS (Piper/Supertonic) audio path. Needs a mic + macOS build.
- **#170 Stage B / E / F** — Supertonic voice-mode TTS backend + in-loop A/B TTFA/RTF numbers + Hindi preview→stable. Stage E needs a mic + audio bench (overlaps #192). Stage F is gated on (a) native-speaker review of `prompts/hi.toml` (grep `# REVIEW:`); (b) a Hindi corpus (none exists); (c) `tests/common/hi.rs` benchmark + retrieval/sweep tests mirroring EN/DE; (d) real-LLM smoke. Licence sub-gate done (PR #263). **Promotion is one commit when those clear:** add `Self::Hindi` to `Locale::ALL` (`primer-core/src/i18n.rs:59`); `status = "stable"` in `hi.toml`; bump `locale_all_excludes_hindi_until_translation_reviewed` (2→3); remove/invert `list_locales_excludes_preview_hindi` (`primer-gui/src/commands/settings.rs`); flip the Hindi README header PREVIEW→STABLE. **OpenRAIL-M clause (e) — an express "AI-generated voice" disclosure — must ship before any default Supertonic flip.**
- **#166 item #1** — owner-run the WhisperStream reuse-invariance smoke on a real model + two 16 kHz mono WAVs (commands at the bottom). Owner-gated.
- **#135** — glib 0.18.5 → 0.20+ (RUSTSEC-2024-0429); blocked on Tauri 3 shipping.
- QNN stable-token-across-reboots gate; NPU pedagogy/answer-quality tuning; latency-routing calibration.

### CI follow-up still open (raised two sessions ago, not yet actioned — owner call)
- **`cargo clippy -p primer-cli --features speech` is NOT in the required gate.** That gap let the `AndroidNative` non-exhaustive match (fixed in PR #269) slip in. A small CI step (`primer-cli --features speech` clippy) would make the next platform-native `TtsBackend` variant fail CI rather than a local run. Out of scope for a pure-test refactor; flagged for the owner.

## Open decisions / risks

- **PR #271 open, awaiting owner review/merge** (item #0, done) — refactor only, no runtime behaviour change, no incidental fixes.
- **`prompt_builder/tests.rs` is 1385 lines** — over the ~500 guideline, but it's a single cohesive test module (same latitude as the 1182-line `prompt_pack/tests.rs` from #270). Splitting it further (e.g. decide_intent tests vs prompt-building tests) is possible but low-value churn; not recommended unless the owner wants test-file granularity.
- **The host-actionable feature backlog remains genuinely empty.** Every open issue needs a microphone, the RedMagic phone, a gated SDK, or upstream Tauri 3. When a session opens like this one, the highest-value host slices are (in order): clean bookkeeping (close resolved-but-open issues — there are none right now), then a small test-guarded refactor of an oversized file, then docs/maintenance. Survey `gh issue list` + the "Carried" section and **confirm direction with the owner before picking** (the owner picked prompt_builder.rs this session).

## Patterns to reuse, not reinvent

- **Test-module extraction is the lowest-risk, zero-churn oversized-file cut.** Move `#[cfg(test)] mod tests { … }` into a sibling `<module>/tests.rs` and replace the inline block with `#[cfg(test)]\nmod tests;`. The test module is a *descendant* of the parent, so `use super::*` reaches all parent privates (incl. cfg(test) helpers) with no visibility edits. Four PRs in a row (#268/#269/#270/#271) used exactly this. Prefer it over moving production code down (which costs `pub(super)`/`pub(crate)` churn).
- **Relocate blocks with `sed` line ranges, never hand-transcription.** A ~1390-line verbatim move is exactly where Edit/Write silently corrupts a line. `head -N` to trim the parent, `sed -n 'A,Bp' > child.rs` to extract the module *body* (inner lines only — drop the `mod tests {` wrapper and closing brace), then `cargo fmt` to dedent — reliable and reviewable.
- **`cargo fmt` may leave one blank line above the file-level `//!` doc that `--check` then rejects.** After moving a module body whose first lines are `//!` inner docs, run `cargo fmt --all -- --check` and delete the leading blank line if flagged (`sed -i '' '1{/^$/d;}'`). One-line fix; don't be surprised by it.
- **For a refactor, the test COUNT is the regression guard.** Capture a pre-move baseline (here: `cargo test -p primer-pedagogy --lib prompt_builder` = 62) and confirm the identical count passes post-move under the new module path. No new tests authored, none needed.
- **Check `cfg(feature …)` in the moved span before trusting a default `cargo test`.** This crate had none, so the default run was sufficient. A cfg-gated span (like `primer-gui/src/config.rs`'s 2 gates, or `primer-cli`) needs the per-feature clippy+test matrix.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git fetch && git log --oneline -4
git checkout refactor/extract-prompt-builder-tests       # this session's branch (4de5a5a)
# Item #0 — push + PR: DONE (PR #271 open against main)

# === Standard workspace gate (run from src/ if you touch .rs) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test --workspace
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo fmt --all -- --check

# === This session's crate (no feature gates — default run is sufficient) ===
~/.cargo/bin/cargo test -p primer-pedagogy --lib prompt_builder       # 62 tests

# === Carried: owner-run the #166 reuse smoke (needs a model + two 16 kHz mono WAVs) ===
PRIMER_WHISPER_MODEL=/path/to/ggml-small.en.bin \
PRIMER_WHISPER_AUDIO_A=/path/to/utterance_a.wav \
PRIMER_WHISPER_AUDIO_B=/path/to/utterance_b.wav \
  ~/.cargo/bin/cargo test -p primer-speech --features whisper \
  --test whisper_stream_reuse -- --ignored --nocapture
```

## Reporting back

- Extracted `primer-pedagogy/src/prompt_builder.rs`'s `#[cfg(test)] mod tests` into a sibling `prompt_builder/tests.rs` (1385 lines). prompt_builder.rs dropped **2042 → 652 lines (−1390)**. The `decide_intent*` family, prompt/intent helpers, and the `#[cfg(test)] fn is_factual_question` test helper all stayed in the parent with visibilities unchanged (descendant-module visibility). Commit `4de5a5a`; **PR #271** open against `main`.
- Pure test-only relocation — no behaviour change, no incidental fixes. Default tests pass identically (62 prompt_builder tests; 0 workspace failures). README/ROADMAP needed no change.
- The GUI is a full app, not a scaffold.
