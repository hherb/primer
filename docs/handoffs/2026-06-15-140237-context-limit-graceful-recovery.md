# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-15 — **Context-limit graceful recovery shipped** (issue #224 + the owner's notify-and-retry extension), opened as **PR #235** on branch `qnn-context-limit-recovery`. When an inference backend fills its context window mid-reply, the Primer now streams a **locale-aware apology to the child** ("…something just happened to my memory — let me try again") and **auto-retries with a progressively smaller prompt** (`Full → drop-knowledge → drop-LTM + shrink window`, up to 2 retries, then a soft-stop cue), recording only the final clean answer. Brainstormed → spec'd → planned → executed via subagent-driven development with two-stage review on the logic-heavy tasks + a final holistic review (2 findings fixed). Host suite green; on-device spot-check deferred. (Prior session's PR #234 merged as `03236a3`.)

**Context at session start:** clean `main` at `03236a3` (PR #234 — the prior brief's headline — already merged before this session began). Of the open queue, #224 was the only fully host-actionable code item; the top open questions (pedagogy tuning, latency routing) remain owner-in-the-loop. The owner chose to ship #224 and extended it: *"if trimmed/incomplete, the Primer has to notify the child … and trigger a second try with a modified prompt."* **End state:** all work on branch `qnn-context-limit-recovery`, opened as **PR #235** (13 commits; CI running — merge when green). No device interaction this session.

## What we shipped this session (PR #235, branch `qnn-context-limit-recovery`)

The feature crosses three layers. Design decisions (settled with the owner in brainstorming):
1. **UX:** partial + apology + clean retry (streamed text can't be un-displayed; the apology + retry is the closure — and the visible self-correction is *itself* pedagogical: it models that no answer source is invariably right).
2. **Persist:** only the final clean answer (keeps `turn_comprehensions` / summary / learner-model coherent).
3. **Retry:** progressive shrink, up to 2 retries (≤ 3 inference calls), then soft-stop.
4. **Scope:** a general `FinishReason` signal; QNN is the first producer.

Commits (oldest first):
- **`3716ab6`** docs: design spec.
- **`d85e612`** docs: implementation plan.
- **`a3d2082`** feat(core): add `FinishReason { Stop, Length }` + `finish_reason` field on `TokenChunk` (both derive `Default`).
- **`5aea3c3`** refactor: thread `..Default::default()` through ~30 `TokenChunk` literal sites (all stay `Stop` — byte-identical behaviour).
- **`6133686`** feat(qnn): `emit_query_outcome` maps `ContextLimit → FinishReason::Length` (host-tested, no NPU needed).
- **`c77ecad`** feat(pedagogy): `PromptBudgetTier { Full, NoKnowledge, Minimal }` pure ladder (`next_tighter`/`includes_knowledge`/`includes_long_term_memory`/`context_window_turns`).
- **`3e8f189`** feat(pedagogy): locale-aware `memory_limit_retry` / `memory_limit_soft_stop` pack strings (en/de/hi).
- **`91ef7a9`** feat(pedagogy): tier-aware `build_turn_prompt` (gates KB / LTM / window on the tier; production passes `Full`).
- **`73b4326`** feat(pedagogy): the recovery retry loop — `run_recovery_loop` + `StreamOutcome`; `stream_inference_response` returns the finish reason and borrows the callback.
- **`87ab5f9`** test(pedagogy): harden retry tests (`SequenceBackend::remaining()==0`; symmetric exhaustion assertions). *(code-review nits)*
- **`8ac9ed7`** style: rustfmt the widened literals.
- **`a6c6f75`** docs: README + ROADMAP note.
- **`091d1a0`** fix(pedagogy): **fail-loud on empty context-limit pack strings** (`validate_non_empty`, drop `#[serde(default)]`, negative test, non-empty fixtures) + **carry `finish_reason` through the reasoning filter** so a future ollama/openai-compat `Length` mapping isn't swallowed. *(final-review findings)*

**Tests (all host-runnable, no NPU):** core `FinishReason`/`TokenChunk` defaults; QNN `Length`/`Stop` emission; `PromptBudgetTier` ladder (4); tier-gated `build_turn_prompt` (Full incl. KB marker, NoKnowledge omits it); retry loop (`Length→Length→Stop` records only "A3" + exactly 2 apologies + all 3 attempts ran; `Length×3` soft-stop + partial accepted; clean-first-try no apology); pack non-empty (en/de) + empty-fails-loud. Final state: `fmt --all --check` clean, `clippy --workspace --all-targets` clean, `cargo test --workspace` exit 0, `cargo test -p primer-inference --features qnn` green.

## What's next (concrete acceptance criteria)

### 1. Merge PR #235
- **Acceptance:** branch protection requires `cargo test (default features)` — merge once green (host suite passed locally). Then `main` carries the graceful-recovery behaviour.

### 2. On-device spot-check of #224 (owner/device-gated; the only deferred piece of this PR)
- **Acceptance:** on the RedMagic 11 Pro with the cl2048 bundle, deliberately overflow a turn (e.g. a very long child question) and confirm the child sees: partial → apology → clean retry, with the `genie.log` showing the context-limit status and the retry's smaller prompt succeeding. The host path is fully covered; this only validates the real `GENIE_STATUS_CONTEXT_LIMIT_EXCEEDED` firing end-to-end. *(With the small-context budget + per-query `GenieDialog_reset`, overflow is now rare — you may need to force it.)*

### 3. ⭐ Pedagogy / answer-quality + rating tuning on the 4B NPU model (owner-in-the-loop) — *carried, still the top open question*
The conversation is technically excellent on-device (25.7 tok/s, PR #227); the open question is pedagogical quality at the compressed 2K-context budget. Owner: *"Quality of answers and ratings will have to be tuned."*
- **Acceptance:** spot-check that the small-context prompt budget (8-turn window, KB top-K 3, ~110-token passage truncation) didn't dull Socratic behaviour (more questions than answers; comprehension-via-transfer); sanity-check `turn_classifications` / `turn_comprehensions` rows on a real device session (pull the session DB, `sqlite3` it). **Define specific tuning targets with the owner first** — this is judgement, not a blind code change. *(Settings → Diagnostics → "Record QNN per-turn throughput metrics" enables on-device metrics, PR #229.)*

### 4. Latency-aware routing calibration (carried; unblocked since PR #227 gave the TTFT number)
- **Acceptance:** local TTFT p95 ≈ 2.6 s is the calibration anchor for `--primary-ttft-budget-ms` / GUI "Primary TTFT budget (ms)". Decide *whether* hybrid routing is even wanted (local is already fast); if so, set a budget around the measured p95. Owner decision.

### Follow-up unlocked by this PR (out of scope here, easy next step)
- **Map cloud/ollama/openai-compat native length finish-reasons to `FinishReason::Length`.** The signal + recovery loop are now general; only QNN produces `Length` today. Cloud's `stop_reason: "max_tokens"`, Ollama's `done_reason: "length"`, and openai-compat's `finish_reason: "length"` could each set `Length` on their terminal chunk so the same notify-and-retry fires for them. The reasoning filter already carries `finish_reason` through (commit `091d1a0`), so the plumbing is ready.

### Carried / owner-or-hardware-gated
- #223 confirm GENIE context-limit enum (needs QAIRT header); #170 Supertonic Stages E/F; #201 llamacpp BOS; #192 / #166 human-at-mic smokes; #157 Termux ONNX-runtime validation; #98 split `sweep.rs`; #135 glib bump on Tauri 3; llama.cpp device bench (owner-gated); (optional) sustained-load thermal sampler in the APK.

### Open queue (issues)

| #   | Title | State |
| --- | --- | --- |
| 224 | QNN context-limit graceful completion can truncate mid-sentence | **DONE — PR #235 (merge when green); on-device spot-check deferred** |
| 223 | Confirm GENIE_STATUS_CONTEXT_LIMIT_EXCEEDED (=4) vs authoritative header | docs (needs QAIRT header) |
| 201 | llamacpp BOS handling across model families | owner-gated (real-model smoke) |
| 192 | Manual smoke: macOS-native STT + injected TTS | needs human at mic |
| 170 | Supertonic 3 voice-mode TTS | Stage D shipped; E/F + manual gates next |
| 166 | Real-audio multi-utterance Whisper smoke | needs human at mic + ggml-small.bin |
| 135 | bump glib → 0.20+ once Tauri 3 ships | waits on Tauri 3 |
| 98  | split tests/common/sweep.rs | defer until 3rd locale |

## Open decisions / risks

- **PR #235 is open, not merged.** Merge once `cargo test (default features)` is green (host suite passed locally). 13 commits.
- **#224's literal "trim to last sentence" was deliberately reframed** to "closure via apology + clean retry" (streamed text can't be retroactively un-displayed; the `truncate_to_tokens` sentence helper is unused in this path). The owner approved this as an *improvement* — the visible self-correction is pedagogically valuable. Don't "add back" a retroactive trim without owner sign-off.
- **Only the final answer is persisted; partials + apology are stream-only.** The child turn is recorded once; intent is decided once and reused across retries (no mid-turn re-decide). Don't move apology/partial text into the recorded Primer turn — it would pollute `turn_comprehensions` / the summary / the learner model.
- **`memory_limit_*` pack strings are now required + non-empty at load** (`validate_non_empty`, like `voice_state`) because they're streamed *unconditionally* on the truncation path — unlike `summary_intro`/`vocab_review_intro` which are omitted-when-empty. A new locale pack MUST provide both keys or it fails to load.
- **The retry bound is structural** (`PromptBudgetTier::next_tighter()` returns `None` at `Minimal`), pinned to `MAX_TRUNCATION_RETRIES` by `ladder_length_matches_retry_budget`. Adding a tier means bumping the const too (the test enforces it).
- **On-device validation of #224 is the only unverified piece** — host coverage is complete, but the real `GENIE_STATUS_CONTEXT_LIMIT_EXCEEDED` path hasn't been exercised on hardware.
- **Pedagogy on a 4B NPU model remains the headline open quality question** — technically excellent, pedagogically unverified at scale.
- **The cl2048 bundle + V81 libs are git-ignored / off-repo**, staged on-device at `files/qnn-bundle`. Re-stage with `~/qnn-export-2048/stage-bundle.sh <SRC>` if needed. (No device work this session.)

## Patterns to reuse, not reinvent

New this session:
- **A backend→engine streaming signal = a field on the terminal `TokenChunk` (`FinishReason`), defaulted so the ~30-site migration is a mechanical `..Default::default()` append.** The dialogue manager reads `chunk.done && chunk.finish_reason` to react; every backend that doesn't care stays at the default. The reasoning filter must *carry* the field through, not rebuild with the default (commit `091d1a0`) — a default-rebuild silently swallows the signal.
- **Progressive prompt-budget recovery = a pure tier ladder (`PromptBudgetTier`) + `next_tighter()` driving a bounded loop.** Each tier's predicates (`includes_knowledge`/`includes_long_term_memory`/`context_window_turns`) are pure and unit-tested; `build_turn_prompt` takes the tier and gates retrieval/window on it; the loop in `respond_to_streaming` is the only stateful part. The loop bound is the ladder length (pinned to a `MAX_*` const by a test), never a manual counter.
- **An unconditionally-streamed pack string needs `validate_non_empty` at load (the `voice_state` discipline), NOT the `summary_intro` serde-default-empty pattern.** The test to mirror is `empty_voice_state_field_returns_err` (mutate a synthetic pack body, assert the load `Err` names the field + "must not be empty").
- **Multi-attempt test backend:** `SequenceBackend` returns a different scripted stream per `generate_stream` call (pop a `VecDeque`), with a `remaining()==0` assertion proving the expected number of attempts ran; `finished(FinishReason)` builds a terminal chunk with a chosen reason (the existing `chunk(text,done)` helper is always `Stop`).

Carried (still true): the strict mobile-modal focus trap = inert the non-toggle background controls (#232); in-dialog modal close = sticky control inside the `aria-modal` subtree reusing `.modal-close` (#234); frontend a11y "TDD" = extend `responsive_layout_contract.rs` with `include_str!` shape-assertions.

Carried (prior QNN/device handoffs, still true): the standalone `qnn_bench` example **can't reach the DSP** from a sideloaded/Termux process on this ROM — on-device throughput is instrumented **inside the APK** (read `<app_data>/.primer/qnn_metrics.jsonl` via `run-as cat`; opt-in via Settings → Diagnostics or `PRIMER_QNN_METRICS_PATH`); the metrics file is bounded (1 MiB + single `.1` backup, PR #229). A stream decorator must finalize on the terminal `done` chunk, not only on `None`. Android scoped storage hides `adb`-pushed `/sdcard/Android/data/<pkg>` — stage app-internal via `adb push /data/local/tmp/<f>` → `run-as <pkg> cp`; this RedMagic ROM has **dead logcat + black screencap** — read app-internal files via `run-as <pkg> cat` + owner reads the screen; a reboot leaves `/data/user/0/<pkg>` encrypted until the owner enters the PIN; QNN APK rebuild = `--no-default-features --features qnn`; cargo from `src/` with `+1.88`; commits touching `.github/workflows` need `gh auth refresh -s workflow`. Android host facts: JDK 21 = Android Studio JBR for `JAVA_HOME`; NDK at `/opt/homebrew/share/android-ndk`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status        # PR #235 merged? then clean main

# === Host health check (default features = required gate) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 test --workspace
~/.cargo/bin/cargo +1.88 test -p primer-pedagogy -- truncated_turn_retries exhausted_retries clean_first_try   # the recovery loop
~/.cargo/bin/cargo +1.88 test -p primer-inference --features qnn -- emits_terminal                            # the Length producer

# === Merge PR #235 (when CI green) ===
gh pr checks 235
gh pr merge 235 --squash --delete-branch    # or merge in the GitHub UI

# === New code work: PR-first (branch protection on main) ===
git checkout main && git pull
git checkout -b <branch> main && git push -u origin <branch> && gh pr create --base main ...
```

## Reporting back

- State plainly what works and what doesn't, by acceptance criterion.
- **This session's headline:** context-limit graceful recovery shipped (issue #224 + the owner's notify-and-retry extension) as **PR #235**. A general `FinishReason::Length` signal on the terminal `TokenChunk` (QNN is the first producer) drives a dialogue-manager recovery loop: stream a **locale-aware apology** to the child, then **auto-retry with a progressively smaller prompt** (`PromptBudgetTier` ladder: drop knowledge → drop long-term memory + shrink window, up to 2 retries), then soft-stop. **Only the final clean answer is persisted.** Brainstormed → spec'd → planned → executed via subagent-driven development (two-stage review on the logic tasks + a final holistic review whose 2 findings — fail-loud on empty pack strings, carry `finish_reason` through the reasoning filter — were fixed). Fully host-tested; the on-device spot-check against the cl2048 bundle is the one deferred piece. Commits `3716ab6`→`091d1a0`. Top remaining open question is unchanged: pedagogy/answer-quality tuning on the 4B model (owner-in-the-loop).
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
