# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-15 — **`main` is clean at `6da74c5`.** The `FinishReason::Length` context-limit recovery is complete across all five backends (QNN, cloud, Ollama, openai-compat, llamacpp), including the precedence fix for truncated reasoning-only replies (PR #243, closes #241 — merged). The remaining work is owner-in-the-loop tuning or device/hardware-gated. There is no open PR carried into this session.

**Context at session start:** clean `main`; host suite green. The inference layer is feature-complete for Phase 1; what's left is judgement (pedagogy tuning) and device/owner-gated validation.

## What's next (concrete acceptance criteria)

### 1. ⭐ Pedagogy / answer-quality + rating tuning on the 4B NPU model (owner-in-the-loop) — *the top open question*
The conversation is technically excellent on-device (25.7 tok/s, full multi-turn conversation, PR #227); the open question is pedagogical quality at the compressed 2K-context budget. Owner: *"Quality of answers and ratings will have to be tuned."*
- **Acceptance:** spot-check that the small-context prompt budget (8-turn window, KB top-K 3, ~110-token passage truncation) didn't dull Socratic behaviour (more questions than answers; comprehension-via-transfer); sanity-check `turn_classifications` / `turn_comprehensions` rows on a real device session (pull the session DB, `sqlite3` it). **Define specific tuning targets with the owner first** — judgement, not a blind code change. *(Settings → Diagnostics → "Record QNN per-turn throughput metrics" enables on-device metrics, PR #229.)*

### 2. On-device spot-check of #224 (owner/device-gated; deferred from PR #235)
- **Acceptance:** on the RedMagic 11 Pro with the cl2048 bundle, deliberately overflow a turn and confirm the child sees partial → apology → clean retry, with `genie.log` showing the context-limit status and the retry's smaller prompt succeeding. *(With the small-context budget + per-query `GenieDialog_reset`, overflow is now rare — you may need to force it.)* #243 makes a truncated reasoning-only reply also recover — worth confirming on a reasoning model if one is on-device.

### 3. Optional real-provider / real-GGUF smokes for the length-recovery path (cheap, deferred)
- **#239** (cloud/Ollama/openai-compat): force `max_tokens=8` on a real cloud turn and confirm the apology+retry fires end-to-end. Burns a little API; the parse-layer wiring is the only new logic and is fully host-covered.
- **#242** (llamacpp): load a real GGUF, force a tiny `max_tokens`, confirm `Length` → apology → retry, and that a naturally short reply reports `Stop` (no spurious retry). The `RealLlamaEngine` arm is compile-checked only (feature-gated, not host-run); behavioural coverage is the mock. Owner/GGUF-gated.

### 4. Latency-aware routing calibration (carried; unblocked since PR #227 gave the TTFT number)
- **Acceptance:** local TTFT p95 ≈ 2.6 s is the calibration anchor for `--primary-ttft-budget-ms` / GUI "Primary TTFT budget (ms)". Decide *whether* hybrid routing is even wanted (local is already fast); if so, set a budget around the measured p95. Owner decision.

### Carried / owner-or-hardware-gated
- #223 confirm GENIE context-limit enum (needs QAIRT header); #170 Supertonic Stages E/F; #201 llamacpp BOS; #192 / #166 human-at-mic smokes; #157 Termux ONNX-runtime validation; #98 split `sweep.rs` (defer until 3rd locale); #135 glib bump on Tauri 3; llama.cpp device bench (owner-gated); (optional) sustained-load thermal sampler in the APK.

### Open queue (issues)

| #   | Title | State |
| --- | --- | --- |
| 242 | Real-GGUF smoke: confirm llamacpp max_tokens length-recovery fires end-to-end | owner/GGUF-gated smoke |
| 239 | Real-provider smoke: confirm max_tokens length-recovery fires (cloud/Ollama/openai-compat) | optional smoke (burns API) |
| 223 | Confirm GENIE_STATUS_CONTEXT_LIMIT_EXCEEDED (=4) vs authoritative header | docs (needs QAIRT header) |
| 201 | llamacpp BOS handling across model families | owner-gated (real-model smoke) |
| 192 | Manual smoke: macOS-native STT + injected TTS | needs human at mic |
| 170 | Supertonic 3 voice-mode TTS | Stage D shipped; E/F + manual gates next |
| 166 | Real-audio multi-utterance Whisper smoke | needs human at mic + ggml-small.bin |
| 135 | bump glib → 0.20+ once Tauri 3 ships | waits on Tauri 3 |
| 98  | split tests/common/sweep.rs | defer until 3rd locale |

## Open decisions / risks

- **Length-wins precedence (issue #241) is settled.** A truncated all-reasoning reply triggers recovery; a clean all-reasoning reply still errors. The gate is `chunk.finish_reason == FinishReason::Length` in the shared `process_filtered_chunk` `None` arm. **Do not drop the `Length` gate** — without it, a model that legitimately reasons-then-says-nothing would loop the recovery retry pointlessly.
- **Minor CLAUDE.md staleness:** the gotcha line "If a model reasons but emits NO visible answer, the backend sends `InferenceError::ReasoningWithoutAnswer`…" is now nuanced — that holds only for a *clean* (`Stop`) reply; a *truncated* (`Length`) one recovers instead. A future `/revise-claude-md` pass should add the one-clause caveat.
- **The `RealLlamaEngine` length logic remains compile-checked only:** behavioural coverage is the `MockLlamaEngine`; the real-GGUF smoke (#242) is the only thing that confirms llama.cpp's actual stop behaviour. Cheap optional follow-up.
- **`max_tokens` truncation on the network/local backends is rarer than on QNN** (generous default budget + 8K+ windows) — a robustness/parity/UX fix, not a hot path.
- **Pedagogy on a 4B NPU model remains the headline open quality question** — technically excellent, pedagogically unverified at scale.

## Patterns to reuse, not reinvent

- **When two terminal outcomes compete at a stream boundary, gate the precedence on the explicit signal, not the default.** `ReasoningWithoutAnswer` vs context-limit recovery: gate on `finish_reason == Length` so only a *truncated* reasoning-only reply jumps the queue; a clean one keeps the old behaviour.
- **A shared streaming helper is the right place for a cross-backend UX fix.** `process_filtered_chunk` is the single byte-stream reasoning step ollama/openai-compat/llamacpp all route through, so one branch fixes all three; the host unit tests cover it on the default `cargo test`.
- **A backend→engine streaming signal = a defaulted field on the terminal `TokenChunk` (`FinishReason`),** so adding a producer is local to one backend and needs no DM change; the reasoning filter must *carry* the field through, not rebuild with the default. HTTP backends map via a small pure `map_*_finish_reason(native) -> FinishReason` helper; when a backend splits the reason from the end-of-stream marker across two events (Anthropic), use a small stateful translator struct. When a backend's seam carries no native stop-reason (token callback + unit-returning `infer`), change the seam to return the `FinishReason` rather than inventing a side channel (and update the test mock).
- **QNN device facts (still true):** the standalone `qnn_bench` example **can't reach the DSP** from a sideloaded/Termux process on this ROM — on-device throughput is instrumented **inside the APK** (read `<app_data>/.primer/qnn_metrics.jsonl` via `run-as cat`; opt-in via Settings → Diagnostics or `PRIMER_QNN_METRICS_PATH`); the metrics file is bounded (1 MiB + single `.1` backup). Android scoped storage hides `adb`-pushed `/sdcard/Android/data/<pkg>` — stage app-internal via `adb push /data/local/tmp/<f>` → `run-as <pkg> cp`; this RedMagic ROM has **dead logcat + black screencap** — read app-internal files via `run-as <pkg> cat` + owner reads the screen; a reboot leaves `/data/user/0/<pkg>` encrypted until the owner enters the PIN; QNN APK rebuild = `--no-default-features --features qnn`; cargo from `src/` with `+1.88`; commits touching `.github/workflows` need `gh auth refresh -s workflow`. Android host facts: JDK 21 = Android Studio JBR for `JAVA_HOME`; NDK at `/opt/homebrew/share/android-ndk`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status        # expect clean main at/after 6da74c5

# === Host health check (default features = required gate) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 test --workspace
# feature-gated arms still compile / pass:
~/.cargo/bin/cargo +1.88 clippy -p primer-inference --features llamacpp --all-targets
~/.cargo/bin/cargo +1.88 test -p primer-inference --features qnn --lib qnn::genie

# === New code work: PR-first (branch protection on main) ===
git checkout main && git pull
git checkout -b <branch> main && git push -u origin <branch> && gh pr create --base main ...
```

## Reporting back

- State plainly what works and what doesn't, by acceptance criterion.
- The top remaining open question is pedagogy/answer-quality tuning on the 4B NPU model (owner-in-the-loop) — the conversation is technically excellent on-device but pedagogically unverified at scale.
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
