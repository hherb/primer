# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-16 — **`main` is clean at `2670904`** (PR #248 merged since the prior brief). This session was a **focused docs-cleanup pass**: it fixed the one known CLAUDE.md staleness (the `ReasoningWithoutAnswer`-vs-`Length` caveat). **No code changed; no new PR.** The autonomous code/CI queue is drained — every remaining item is owner-judgement or device/hardware-gated.

**Context at session start:** the inference layer is feature-complete for Phase 1. What remains is judgement (pedagogy tuning), an owner config decision (latency routing), and device/owner-gated validation. PR #248 (the #246 CI clippy gate) was already merged at the start of this session — it is the current `main` HEAD.

## What we shipped this session

- **CLAUDE.md staleness fix (docs-only).** The reasoning-mode gotcha line claimed unconditionally that an all-reasoning, no-visible-answer reply surfaces `InferenceError::ReasoningWithoutAnswer`. That is true only for a **clean** reply (`finish_reason: FinishReason::Stop`); a **truncated** one (`finish_reason: FinishReason::Length`) takes the context-limit recovery ladder instead (issue #241 — length wins over `ReasoningWithoutAnswer`, gated in `process_filtered_chunk`'s `None` arm). Added the one-clause caveat + a "Do not drop the `Length` gate" warning. Verified against the live code in `primer-inference/src/reasoning_stream.rs` (the `None if chunk.finish_reason == FinishReason::Length` arm).
- **Confirmed README.md and ROADMAP.md need NO change.** Both already document the nuance correctly: README.md line ~113 (*"a clean, non-truncated all-reasoning reply still surfaces `ReasoningWithoutAnswer`, since a retry would not help"*) and ROADMAP.md line ~80 (issue #241 entry). Only CLAUDE.md carried the unqualified sentence. No functional change this session, so no other doc surface moved.
- **Verified host health.** `main` clean; `cargo +1.88 fmt --all -- --check` passes; PR #248's full CI (incl. `cargo test (default features)`) was green at merge.

> ⚠️ **The CLAUDE.md edit is in the working tree but may be uncommitted** — confirm with `git status`. If uncommitted, it's a doc-only change; the established repo pattern (see prior briefs) is to commit doc-only changes directly to `main` (CI `paths-ignore` skips `.md`). Commit it with this brief + its handoff snapshot.

## What's next (concrete acceptance criteria)

> The autonomous code queue is empty. Every item below needs owner judgement or a device/mic. **Pick one with the owner before coding.**

### 1. ⭐ Pedagogy / answer-quality + rating tuning on the 4B NPU model (owner-in-the-loop) — *the top open question*
The conversation is technically excellent on-device (25.7 tok/s, full multi-turn, PR #227); the open question is pedagogical quality at the compressed 2K-context budget. Owner: *"Quality of answers and ratings will have to be tuned."*
- **Acceptance:** spot-check that the small-context prompt budget (8-turn window, KB top-K 3, ~110-token passage truncation) didn't dull Socratic behaviour (more questions than answers; comprehension-via-transfer); sanity-check `turn_classifications` / `turn_comprehensions` rows on a real device session (pull the session DB, `sqlite3` it). **Define specific tuning targets with the owner first** — judgement, not a blind code change. *(Settings → Diagnostics → "Record QNN per-turn throughput metrics" enables on-device metrics, PR #229.)*

### 2. On-device spot-check of #224 (owner/device-gated; deferred from PR #235)
- **Acceptance:** on the RedMagic 11 Pro with the cl2048 bundle, deliberately overflow a turn and confirm the child sees partial → apology → clean retry, with `genie.log` showing the context-limit status and the retry's smaller prompt succeeding. *(With the small-context budget + per-query `GenieDialog_reset`, overflow is now rare — you may need to force it.)* #243 makes a truncated reasoning-only reply also recover — worth confirming on a reasoning model if one is on-device.
- **Note:** the host-side llamacpp (#242) AND Ollama (#239) analogues are both done; QNN is the remaining device-gated leg.

### 3. Latency-aware routing calibration (carried; unblocked since PR #227 gave the TTFT number)
- **Acceptance:** local TTFT p95 ≈ 2.6 s is the calibration anchor for `--primary-ttft-budget-ms` / GUI "Primary TTFT budget (ms)". Decide *whether* hybrid routing is even wanted (local is already fast); if so, set a budget around the measured p95. Owner decision.

### Carried / owner-or-hardware-gated
- #223 confirm GENIE context-limit enum (needs QAIRT header); #170 Supertonic Stages E/F; #201 llamacpp BOS; #192 / #166 human-at-mic smokes; #157 Termux ONNX-runtime validation; #135 glib bump on Tauri 3; llama.cpp device throughput bench (owner-gated); (optional) sustained-load thermal sampler in the APK.

### Open queue (issues)

| #   | Title | State |
| --- | --- | --- |
| 223 | Confirm GENIE_STATUS_CONTEXT_LIMIT_EXCEEDED (=4) vs authoritative header | docs (needs QAIRT header) |
| 201 | llamacpp BOS handling across model families | owner-gated (real-model smoke) |
| 192 | Manual smoke: macOS-native STT + injected TTS | needs human at mic |
| 170 | Supertonic 3 voice-mode TTS | Stage D shipped; E/F + manual gates next |
| 166 | Real-audio multi-utterance Whisper smoke | needs human at mic + ggml-small.bin |
| 135 | bump glib → 0.20+ once Tauri 3 ships | waits on Tauri 3 |

## Open decisions / risks

- **The autonomous queue is genuinely drained.** #246 and #239 closed last session; this session cleared the last known doc-staleness item. The remaining work is all owner-judgement or hardware-gated — do not invent code work to fill the gap; ask the owner for direction.
- **Both real-backend length smokes are settled.** #242 (llamacpp) and #239 (Ollama) are behaviourally confirmed against real models. The only remaining length-recovery leg is **QNN on-device (#224)**, hardware-gated.
- **No CLI/env override exists for `max_tokens`.** A truncation smoke still needs a throwaway edit to the `GenerationParams` literal in `turn.rs::stream_inference_response` (default `max_tokens: 512` → tiny), reverted after. If forcing this becomes routine, consider a hidden `--max-response-tokens` debug flag — but it's only ever needed for these smokes, so throwaway-edit-and-revert is fine.
- **Length-wins precedence (issue #241) is settled and now correctly documented in CLAUDE.md.** A truncated all-reasoning reply triggers recovery; a clean all-reasoning reply still errors. The gate is `chunk.finish_reason == FinishReason::Length` in the shared `process_filtered_chunk` `None` arm. **Do not drop the `Length` gate.**
- **Pedagogy on a 4B NPU model remains the headline open quality question** — technically excellent, pedagogically unverified at scale.

## Patterns to reuse, not reinvent

- **A truncation smoke = build with the backend feature + force `max_tokens` tiny via a throwaway edit (revert after) + drive the REPL over stdin.** For #239 (free, local): `printf 'long-answer question\nquit\n' | ./target/debug/primer --backend ollama --model granite4.1:3b-q8_0 --no-persist --embedder-backend none --classifier/extractor/comprehension-backend stub`. Keep subsystems on `stub` so the smoke isolates the chat-path truncation; `--no-persist` + `--embedder-backend none` skip DB writes and the 570 MB embedder download. The visible signature is N apologies + 1 soft-stop where N = `MAX_TRUNCATION_RETRIES` (= 2 today).
- **Before editing a CLAUDE.md "gotcha", verify it against the live code.** This session's fix was confirmed against the `None if chunk.finish_reason == FinishReason::Length` arm in `reasoning_stream.rs` — the doc had drifted from a behaviour that issue #241 had since refined. README/ROADMAP were already correct; only CLAUDE.md lagged.
- **When two terminal outcomes compete at a stream boundary, gate the precedence on the explicit signal, not the default.** `ReasoningWithoutAnswer` vs context-limit recovery: gate on `finish_reason == Length`.
- **QNN device facts (still true):** the standalone `qnn_bench` example **can't reach the DSP** from a sideloaded/Termux process on this ROM — on-device throughput is instrumented **inside the APK** (read `<app_data>/.primer/qnn_metrics.jsonl` via `run-as cat`; opt-in via Settings → Diagnostics or `PRIMER_QNN_METRICS_PATH`; bounded 1 MiB + single `.1` backup). Android scoped storage hides `adb`-pushed `/sdcard/Android/data/<pkg>` — stage app-internal via `adb push /data/local/tmp/<f>` → `run-as <pkg> cp`; this RedMagic ROM has **dead logcat + black screencap** — read app-internal files via `run-as <pkg> cat` + owner reads the screen; a reboot leaves `/data/user/0/<pkg>` encrypted until the owner enters the PIN; QNN APK rebuild = `--no-default-features --features qnn`; cargo from `src/` with `+1.88`; commits touching `.github/workflows` need `gh auth refresh -s workflow`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status        # expect clean main at/after 2670904 (or the doc commit on top)

# === Host health check (default features = required gate) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 test --workspace
# feature-gated arms still compile / pass:
~/.cargo/bin/cargo +1.88 clippy -p primer-inference --features llamacpp --all-targets
~/.cargo/bin/cargo +1.88 clippy -p primer-kb-load --features fastembed --all-targets   # the #246 gate
~/.cargo/bin/cargo +1.88 test -p primer-inference --features qnn --lib qnn::genie

# === Reproduce the #239 ollama truncation smoke (optional, free) ===
# 1. build:  ~/.cargo/bin/cargo +1.88 build --bin primer
# 2. edit turn.rs::stream_inference_response → add `max_tokens: 8,` to the GenerationParams literal (REVERT after)
# 3. printf 'Tell me everything about how the sun makes light and heat.\nquit\n' | ./target/debug/primer \
#      --backend ollama --model granite4.1:3b-q8_0 --no-persist --embedder-backend none \
#      --classifier-backend stub --extractor-backend stub --comprehension-backend stub --name SmokeKid --age 9
#    expect: 2 apologies + 1 soft-stop

# === New code work: PR-first (branch protection on main) ===
git checkout main && git pull
git checkout -b <branch> main && git push -u origin <branch> && gh pr create --base main ...
```

## Reporting back

- State plainly what works and what doesn't, by acceptance criterion.
- The autonomous code queue is empty — the next session should open by asking the owner which owner/device-gated item to tackle, not by inventing code work.
- The top remaining open question is pedagogy/answer-quality tuning on the 4B NPU model (owner-in-the-loop) — the conversation is technically excellent on-device but pedagogically unverified at scale.
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
