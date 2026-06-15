# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-16 — **`main` is clean at `4f97f90`.** Since the last brief, two PRs merged independently: **#244** (living-docs refresh) and **#245 / #98** (split `tests/common/sweep.rs` into bm25/hybrid submodules — was previously listed as "defer until 3rd locale"). This session knocked out the **#242 real-GGUF llamacpp length-recovery smoke** (passed) and recorded it in ROADMAP via open **PR #247** (`2053fbb`, doc-only, not yet merged).

**Context at session start:** clean `main`; the inference layer is feature-complete for Phase 1. What remains is judgement (pedagogy tuning) and device/owner-gated validation. No carried-in code PR.

## What we shipped this session

- **#242 real-GGUF length-recovery smoke — PASSED (no code change).** Verified `RealLlamaEngine`'s `FinishReason::Length` path end-to-end against `LFM2-1.2B-Q4_K_M` on a `--features primer-cli/llamacpp-metal` build. This closes the gap the prior brief flagged ("the `RealLlamaEngine` length logic remains compile-checked only"). Issue **#242 closed** with full evidence.
  - **Test A (no spurious retry):** a one-sentence question produced a complete answer with **no** apology/soft-stop — EOS within the 512-token budget → `Stop`. ✓
  - **Test B (`Length` → apology → retry ladder → soft-stop):** temporarily forcing `max_tokens: 12` in `stream_inference_response` (no CLI override exists; **reverted**, working tree clean) produced the exact expected signature for `MAX_TRUNCATION_RETRIES=2` + the 3-tier Full→NoKnowledge→Minimal ladder: **two** `"Oh — I'm sure up to there…"` apologies + **one** `"Let's pause there for now…"` soft-stop. ✓
- **PR #247 (`2053fbb`) — open, doc-only.** Adds a clause to the ROADMAP #238 line recording the real-GGUF confirmation. **Merge it** (default-features CI is the only gate; it touches no `.rs`).

## What's next (concrete acceptance criteria)

### 0. Merge PR #247 (trivial)
- Doc-only ROADMAP note. Confirm CI green and merge; delete the branch.

### 1. ⭐ Pedagogy / answer-quality + rating tuning on the 4B NPU model (owner-in-the-loop) — *the top open question*
The conversation is technically excellent on-device (25.7 tok/s, full multi-turn, PR #227); the open question is pedagogical quality at the compressed 2K-context budget. Owner: *"Quality of answers and ratings will have to be tuned."*
- **Acceptance:** spot-check that the small-context prompt budget (8-turn window, KB top-K 3, ~110-token passage truncation) didn't dull Socratic behaviour (more questions than answers; comprehension-via-transfer); sanity-check `turn_classifications` / `turn_comprehensions` rows on a real device session (pull the session DB, `sqlite3` it). **Define specific tuning targets with the owner first** — judgement, not a blind code change. *(Settings → Diagnostics → "Record QNN per-turn throughput metrics" enables on-device metrics, PR #229.)*

### 2. On-device spot-check of #224 (owner/device-gated; deferred from PR #235)
- **Acceptance:** on the RedMagic 11 Pro with the cl2048 bundle, deliberately overflow a turn and confirm the child sees partial → apology → clean retry, with `genie.log` showing the context-limit status and the retry's smaller prompt succeeding. *(With the small-context budget + per-query `GenieDialog_reset`, overflow is now rare — you may need to force it.)* #243 makes a truncated reasoning-only reply also recover — worth confirming on a reasoning model if one is on-device.
- **Note:** the host-side llamacpp analogue of this is now done (#242 above); QNN is the remaining device-gated leg.

### 3. Optional real-provider smoke for the length-recovery path (cheap, deferred)
- **#239** (cloud/Ollama/openai-compat): force `max_tokens=8` on a real turn and confirm the apology+retry fires end-to-end. **Ollama is reachable on this machine (free, no API burn)** — the cheapest way to close this; same temporary-`max_tokens`-edit technique as #242 (no CLI override). The parse-layer wiring is the only new logic and is fully host-covered.

### 4. Latency-aware routing calibration (carried; unblocked since PR #227 gave the TTFT number)
- **Acceptance:** local TTFT p95 ≈ 2.6 s is the calibration anchor for `--primary-ttft-budget-ms` / GUI "Primary TTFT budget (ms)". Decide *whether* hybrid routing is even wanted (local is already fast); if so, set a budget around the measured p95. Owner decision.

### Carried / owner-or-hardware-gated
- #223 confirm GENIE context-limit enum (needs QAIRT header); #170 Supertonic Stages E/F; #201 llamacpp BOS; #192 / #166 human-at-mic smokes; #157 Termux ONNX-runtime validation; #135 glib bump on Tauri 3; llama.cpp device throughput bench (owner-gated); (optional) sustained-load thermal sampler in the APK.

### Open queue (issues)

| #   | Title | State |
| --- | --- | --- |
| 239 | Real-provider smoke: confirm max_tokens length-recovery fires (cloud/Ollama/openai-compat) | optional smoke — **ollama path is free here** |
| 223 | Confirm GENIE_STATUS_CONTEXT_LIMIT_EXCEEDED (=4) vs authoritative header | docs (needs QAIRT header) |
| 201 | llamacpp BOS handling across model families | owner-gated (real-model smoke) |
| 192 | Manual smoke: macOS-native STT + injected TTS | needs human at mic |
| 170 | Supertonic 3 voice-mode TTS | Stage D shipped; E/F + manual gates next |
| 166 | Real-audio multi-utterance Whisper smoke | needs human at mic + ggml-small.bin |
| 135 | bump glib → 0.20+ once Tauri 3 ships | waits on Tauri 3 |

## Open decisions / risks

- **#242 is settled and closed** — the `RealLlamaEngine` `Length` path is now behaviourally confirmed (not just compile-checked). The only remaining real-backend length smoke is **#239** (cheaply doable via local ollama).
- **No CLI/env override exists for `max_tokens`.** Both #242 and #239 require a throwaway edit to `GenerationParams { .. }` in `turn.rs::stream_inference_response` (default `max_tokens: 512`). If forcing this becomes routine, consider a hidden `--max-response-tokens` debug flag — but it's only ever needed for these truncation smokes, so the throwaway-edit-and-revert pattern is fine.
- **In the real llamacpp engine, `FinishReason::Length` comes ONLY from hitting `max_tokens`** — an `n_ctx` overflow errors out via `ctx.decode` (a hard error), it does *not* truncate. So a forced tiny `max_tokens` truncates every tier identically and the ladder always ends at soft-stop (it never reaches a *successful* retry `Stop` via this mechanism). That's expected and still fully exercises the apology+retry path; do not read "always soft-stops under forced `max_tokens`" as a bug.
- **Length-wins precedence (issue #241) is settled.** A truncated all-reasoning reply triggers recovery; a clean all-reasoning reply still errors. The gate is `chunk.finish_reason == FinishReason::Length` in the shared `process_filtered_chunk` `None` arm. **Do not drop the `Length` gate.**
- **Minor CLAUDE.md staleness (unchanged from last brief):** the gotcha line "If a model reasons but emits NO visible answer, the backend sends `InferenceError::ReasoningWithoutAnswer`…" holds only for a *clean* (`Stop`) reply; a *truncated* (`Length`) one recovers instead. A future `/revise-claude-md` pass should add the one-clause caveat.
- **Pedagogy on a 4B NPU model remains the headline open quality question** — technically excellent, pedagogically unverified at scale.

## Patterns to reuse, not reinvent

- **A truncation smoke = build with the backend feature + force `max_tokens` tiny via a throwaway edit (revert after) + drive the REPL over stdin.** For #242: `printf 'long-answer question\nquit\n' | ./target/debug/primer --backend llamacpp --model <gguf> --no-persist --embedder-backend none --classifier/extractor/comprehension-backend stub`. Keep subsystems on `stub` so the smoke isolates the chat-path truncation; `--no-persist` + `--embedder-backend none` skip DB writes and the 570 MB embedder download. The visible signature of the recovery loop firing is N apologies + 1 soft-stop where N = `MAX_TRUNCATION_RETRIES` (= 2 today).
- **When two terminal outcomes compete at a stream boundary, gate the precedence on the explicit signal, not the default.** `ReasoningWithoutAnswer` vs context-limit recovery: gate on `finish_reason == Length`.
- **A shared streaming helper is the right place for a cross-backend UX fix.** `process_filtered_chunk` is the single byte-stream reasoning step ollama/openai-compat/llamacpp all route through.
- **QNN device facts (still true):** the standalone `qnn_bench` example **can't reach the DSP** from a sideloaded/Termux process on this ROM — on-device throughput is instrumented **inside the APK** (read `<app_data>/.primer/qnn_metrics.jsonl` via `run-as cat`; opt-in via Settings → Diagnostics or `PRIMER_QNN_METRICS_PATH`; bounded 1 MiB + single `.1` backup). Android scoped storage hides `adb`-pushed `/sdcard/Android/data/<pkg>` — stage app-internal via `adb push /data/local/tmp/<f>` → `run-as <pkg> cp`; this RedMagic ROM has **dead logcat + black screencap** — read app-internal files via `run-as <pkg> cat` + owner reads the screen; a reboot leaves `/data/user/0/<pkg>` encrypted until the owner enters the PIN; QNN APK rebuild = `--no-default-features --features qnn`; cargo from `src/` with `+1.88`; commits touching `.github/workflows` need `gh auth refresh -s workflow`. Android host facts: JDK 21 = Android Studio JBR for `JAVA_HOME`; NDK at `/opt/homebrew/share/android-ndk`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status        # expect clean main at/after 4f97f90
gh pr view 247                                   # doc-only ROADMAP note — merge if CI green

# === Host health check (default features = required gate) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 test --workspace
# feature-gated arms still compile / pass:
~/.cargo/bin/cargo +1.88 clippy -p primer-inference --features llamacpp --all-targets
~/.cargo/bin/cargo +1.88 test -p primer-inference --features qnn --lib qnn::genie

# === Reproduce the #242 llamacpp truncation smoke (optional) ===
# 1. build:  ~/.cargo/bin/cargo +1.88 build --bin primer --features primer-cli/llamacpp-metal
# 2. edit turn.rs::stream_inference_response → add `max_tokens: 12,` to the GenerationParams literal (REVERT after)
# 3. GGUF=/Users/hherb/Library/Caches/llama.cpp/LiquidAI_LFM2-1.2B-GGUF_LFM2-1.2B-Q4_K_M.gguf
#    printf 'Tell me everything about how the sun makes light.\nquit\n' | ./target/debug/primer \
#      --backend llamacpp --model "$GGUF" --no-persist --embedder-backend none \
#      --classifier-backend stub --extractor-backend stub --comprehension-backend stub --name SmokeKid --age 9

# === New code work: PR-first (branch protection on main) ===
git checkout main && git pull
git checkout -b <branch> main && git push -u origin <branch> && gh pr create --base main ...
```

## Reporting back

- State plainly what works and what doesn't, by acceptance criterion.
- The top remaining open question is pedagogy/answer-quality tuning on the 4B NPU model (owner-in-the-loop) — the conversation is technically excellent on-device but pedagogically unverified at scale.
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
