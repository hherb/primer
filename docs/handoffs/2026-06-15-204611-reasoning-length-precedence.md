# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-15 — **`FinishReason::Length` recovery now also rescues truncated reasoning-only replies**, opened as **PR #243** on branch `reasoning-length-precedence` (closes issue #241). The prior session's PR #240 (llamacpp length-recovery parity, the fifth producer) had already merged into `main` (`0cafc83`) before this session began. PR #243 is a follow-up bug-fix from the PR #240 code review: when a reply is truncated by the token budget *and* everything emitted stayed inside a `<think>` block, the shared reasoning filter used to mask the `Length` flag behind `ReasoningWithoutAnswer`. Now `Length` wins (owner decision) so the dialogue manager's notify-and-retry recovery fires. Host suite green; no device interaction.

**Context at session start:** clean `main` at `0cafc83` (PR #240 already merged — the brief's headline action #1 was done; README/ROADMAP already documented the five-producer length recovery). The brief's remaining items were all owner-in-the-loop or device-gated. Owner chose "pick a host-runnable issue"; **#241** (a real cross-backend bug in the shared `process_filtered_chunk` filter, surfaced by the PR #240 review, fully host-testable) was the natural pick. The issue flagged one decision as the owner's — Length-wins vs ReasoningWithoutAnswer-wins — owner chose **Length wins (retry)**. **End state:** all work on branch `reasoning-length-precedence`, opened as **PR #243** (1 commit; CI running — merge when green).

## What we shipped this session (PR #243, branch `reasoning-length-precedence`, closes #241)

A precedence refinement inside the shared reasoning filter. **No dialogue-manager change** — the recovery loop (`turn.rs`) was already backend-agnostic; the fix lets a truncated reasoning-only reply reach it instead of erroring out first.

**Problem.** In [`reasoning_stream.rs::process_filtered_chunk`](src/crates/primer-inference/src/reasoning_stream.rs) the terminal (`chunk.done`) branch routed on `finalize_visible` first. With `had_visible == false` and suppression seen, the `None` arm returned `InferenceError::ReasoningWithoutAnswer` and silently discarded `chunk.finish_reason` (`Length`). The child got a generic "thinking problem, try again" and the turn dropped — no recovery retry.

**Fix.** The `None` arm now checks `chunk.finish_reason == FinishReason::Length` first:
- **`Length` (truncated)** → forward the terminal chunk (`Final(Ok)` carrying `Length`, empty text). `run_recovery_loop` sees `(Length, Some(next))` and fires the apology + tighter-tier retry, which may let the model finish its reasoning and produce a visible answer.
- **`Stop` (clean) all-reasoning reply** → still `ReasoningWithoutAnswer`. A retry would not help — the model genuinely said nothing.

Cross-backend: `OllamaBackend`, `OpenAiCompatBackend`, and `LlamaCppBackend` all route their terminal chunk through this one shared helper, so all three benefit. QNN does not use this filter today.

Commit:
- **`c5a8ea2`** feat(inference): truncated reasoning-only reply recovers via `FinishReason::Length` (#241).

**Tests (host-runnable on default `cargo test`, TDD — red then green):**
- `truncated_reasoning_only_reply_carries_length_not_error` — `done` + `Length` + no visible + suppression ⇒ `Final(Ok)` carrying `Length` (was the failing red test).
- `clean_reasoning_only_reply_still_errors` — same but `Stop` ⇒ `Final(Err)` (the gating guarantee).

Final state: `fmt --all --check` clean, `clippy --workspace --all-targets` clean, `clippy -p primer-inference --features llamacpp --all-targets` clean, `cargo test --workspace` exit 0. README + ROADMAP updated with a tight note (the existing "a truncated reply from any backend triggers recovery" claim is now strictly more accurate).

## What's next (concrete acceptance criteria)

### 1. Merge PR #243
- **Acceptance:** branch protection requires `cargo test (default features)` — merge once green (host suite passed locally). 1 commit.

### 2. ⭐ Pedagogy / answer-quality + rating tuning on the 4B NPU model (owner-in-the-loop) — *carried, still the top open question*
The conversation is technically excellent on-device (25.7 tok/s, PR #227); the open question is pedagogical quality at the compressed 2K-context budget. Owner: *"Quality of answers and ratings will have to be tuned."*
- **Acceptance:** spot-check that the small-context prompt budget (8-turn window, KB top-K 3, ~110-token passage truncation) didn't dull Socratic behaviour (more questions than answers; comprehension-via-transfer); sanity-check `turn_classifications` / `turn_comprehensions` rows on a real device session (pull the session DB, `sqlite3` it). **Define specific tuning targets with the owner first** — judgement, not a blind code change. *(Settings → Diagnostics → "Record QNN per-turn throughput metrics" enables on-device metrics, PR #229.)*

### 3. On-device spot-check of #224 (owner/device-gated; deferred from PR #235)
- **Acceptance:** on the RedMagic 11 Pro with the cl2048 bundle, deliberately overflow a turn and confirm the child sees partial → apology → clean retry, with `genie.log` showing the context-limit status and the retry's smaller prompt succeeding. *(With the small-context budget + per-query `GenieDialog_reset`, overflow is now rare — you may need to force it.)* **#243 makes a truncated reasoning-only reply also recover** — worth confirming on a reasoning model if one is on-device.

### 4. Optional real-provider / real-GGUF smokes for the length-recovery path (cheap, deferred)
- **#239** (cloud/Ollama/openai-compat): force `max_tokens=8` on a real cloud turn and confirm the apology+retry fires end-to-end. Burns a little API; the parse-layer wiring is the only new logic and is fully host-covered.
- **#242** (llamacpp): load a real GGUF, force a tiny `max_tokens`, confirm `Length` → apology → retry, and that a naturally short reply reports `Stop` (no spurious retry). The `RealLlamaEngine` arm is compile-checked only (feature-gated, not host-run); behavioural coverage is the mock. Owner/GGUF-gated.

### 5. Latency-aware routing calibration (carried; unblocked since PR #227 gave the TTFT number)
- **Acceptance:** local TTFT p95 ≈ 2.6 s is the calibration anchor for `--primary-ttft-budget-ms` / GUI "Primary TTFT budget (ms)". Decide *whether* hybrid routing is even wanted (local is already fast); if so, set a budget around the measured p95. Owner decision.

### Carried / owner-or-hardware-gated
- #223 confirm GENIE context-limit enum (needs QAIRT header); #170 Supertonic Stages E/F; #201 llamacpp BOS; #192 / #166 human-at-mic smokes; #157 Termux ONNX-runtime validation; #98 split `sweep.rs` (defer until 3rd locale); #135 glib bump on Tauri 3; llama.cpp device bench (owner-gated); (optional) sustained-load thermal sampler in the APK.

### Open queue (issues)

| #   | Title | State |
| --- | --- | --- |
| 242 | Real-GGUF smoke: confirm llamacpp max_tokens length-recovery fires end-to-end | owner/GGUF-gated smoke |
| 241 | truncated reasoning-only reply masks Length with ReasoningWithoutAnswer | **PR #243 open** |
| 239 | Real-provider smoke: confirm max_tokens length-recovery fires (cloud/Ollama/openai-compat) | optional smoke (burns API) |
| 223 | Confirm GENIE_STATUS_CONTEXT_LIMIT_EXCEEDED (=4) vs authoritative header | docs (needs QAIRT header) |
| 201 | llamacpp BOS handling across model families | owner-gated (real-model smoke) |
| 192 | Manual smoke: macOS-native STT + injected TTS | needs human at mic |
| 170 | Supertonic 3 voice-mode TTS | Stage D shipped; E/F + manual gates next |
| 166 | Real-audio multi-utterance Whisper smoke | needs human at mic + ggml-small.bin |
| 135 | bump glib → 0.20+ once Tauri 3 ships | waits on Tauri 3 |
| 98  | split tests/common/sweep.rs | defer until 3rd locale |

## Open decisions / risks

- **PR #243 is open, not merged.** Merge once `cargo test (default features)` is green (host suite passed locally). 1 commit.
- **Owner decided Length-wins precedence (issue #241).** A truncated all-reasoning reply now triggers recovery; a clean all-reasoning reply still errors. The gate is `chunk.finish_reason == FinishReason::Length` in the shared `process_filtered_chunk` `None` arm. **Do not drop the `Length` gate** — without it, a model that legitimately reasons-then-says-nothing would loop the recovery retry pointlessly.
- **Minor CLAUDE.md staleness (not fixed this session — out of /nextsession scope):** the gotcha line "If a model reasons but emits NO visible answer, the backend sends `InferenceError::ReasoningWithoutAnswer`…" is now nuanced — that holds only for a *clean* (`Stop`) reply; a *truncated* (`Length`) one recovers instead. A future `/revise-claude-md` pass should add the one-clause caveat.
- **The `RealLlamaEngine` length logic remains compile-checked only** (carried from #238/#240): behavioural coverage is the `MockLlamaEngine`; the real-GGUF smoke (#242) is the only thing that confirms llama.cpp's actual stop behaviour. Cheap optional follow-up.
- **`max_tokens` truncation on the network/local backends is rarer than on QNN** (generous default budget + 8K+ windows) — a robustness/parity/UX fix, not a hot path. Still correct.
- **Pedagogy on a 4B NPU model remains the headline open quality question** — technically excellent, pedagogically unverified at scale.

## Patterns to reuse, not reinvent

New/refined this session:
- **When two terminal outcomes compete at a stream boundary, gate the precedence on the explicit signal, not the default.** Here `ReasoningWithoutAnswer` (no-visible-answer) vs context-limit recovery: gate on `finish_reason == Length` so only a *truncated* reasoning-only reply jumps the queue; a clean one keeps the old behaviour. The signal already rode the terminal chunk (PR #235's defaulted-field pattern) — the fix is one match arm, no new plumbing.
- **A shared streaming helper is the right place for a cross-backend UX fix.** `process_filtered_chunk` is the single byte-stream reasoning step ollama/openai-compat/llamacpp all route through, so one branch fixes all three; the host unit tests in that module cover it on the default `cargo test` with no backend dep.

Carried (still true from PR #240): when a backend's streaming seam carries no native stop-reason (token callback + unit-returning `infer`), change the seam to return the `FinishReason` rather than inventing a side channel; update both the real impl and the test mock (`MockLlamaEngine::truncated()`). The synthetic terminal/flush chunk is where the finish reason lands — build it with the engine-reported reason and rely on `process_filtered_chunk` to carry `finish_reason` through the reasoning filter.

Carried (PR #237): a backend's terminal finish-reason maps via a small pure `map_*_finish_reason(native: &str) -> FinishReason` helper for HTTP backends; when a backend splits the reason from the end-of-stream marker across two events (Anthropic), use a small stateful translator struct.

Carried (PR #235): a backend→engine streaming signal = a defaulted field on the terminal `TokenChunk` (`FinishReason`), so adding a producer is local to one backend and needs no DM change; the reasoning filter must *carry* the field through, not rebuild with the default.

Carried (prior QNN/device handoffs, still true): the standalone `qnn_bench` example **can't reach the DSP** from a sideloaded/Termux process on this ROM — on-device throughput is instrumented **inside the APK** (read `<app_data>/.primer/qnn_metrics.jsonl` via `run-as cat`; opt-in via Settings → Diagnostics or `PRIMER_QNN_METRICS_PATH`); the metrics file is bounded (1 MiB + single `.1` backup, PR #229). Android scoped storage hides `adb`-pushed `/sdcard/Android/data/<pkg>` — stage app-internal via `adb push /data/local/tmp/<f>` → `run-as <pkg> cp`; this RedMagic ROM has **dead logcat + black screencap** — read app-internal files via `run-as <pkg> cat` + owner reads the screen; a reboot leaves `/data/user/0/<pkg>` encrypted until the owner enters the PIN; QNN APK rebuild = `--no-default-features --features qnn`; cargo from `src/` with `+1.88`; commits touching `.github/workflows` need `gh auth refresh -s workflow`. Android host facts: JDK 21 = Android Studio JBR for `JAVA_HOME`; NDK at `/opt/homebrew/share/android-ndk`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status        # PR #243 merged? then clean main

# === Host health check (default features = required gate) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 test --workspace
# the #241 precedence tests:
~/.cargo/bin/cargo +1.88 test -p primer-inference --lib reasoning_stream
# the RealLlamaEngine arm still compiles:
~/.cargo/bin/cargo +1.88 clippy -p primer-inference --features llamacpp --all-targets
# the QNN producer still green:
~/.cargo/bin/cargo +1.88 test -p primer-inference --features qnn --lib qnn::genie

# === Merge PR #243 (when CI green) ===
gh pr checks 243
gh pr merge 243 --squash --delete-branch    # or merge in the GitHub UI

# === New code work: PR-first (branch protection on main) ===
git checkout main && git pull
git checkout -b <branch> main && git push -u origin <branch> && gh pr create --base main ...
```

## Reporting back

- State plainly what works and what doesn't, by acceptance criterion.
- **This session's headline:** extended the `FinishReason::Length` context-limit recovery to **truncated reasoning-only replies** as **PR #243** (closes #241). The shared `process_filtered_chunk` filter now lets a terminal `Length` win over `ReasoningWithoutAnswer` when no visible answer was produced, so a `<think>`-only truncation reaches the dialogue manager's notify-and-retry recovery instead of dropping the turn with a generic "thinking problem". Gated on `Length` (a clean all-reasoning reply still errors). One match arm in the cross-backend filter — ollama/openai-compat/llamacpp all benefit; no dialogue-manager change. TDD, fully host-tested. Commit `c5a8ea2`. Top remaining open question is unchanged: pedagogy/answer-quality tuning on the 4B model (owner-in-the-loop).
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
