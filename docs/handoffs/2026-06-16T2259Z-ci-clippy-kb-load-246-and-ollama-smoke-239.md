# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-16 — **`main` is clean at `4b6edb3`** (PR #247 merged since the last brief). This session shipped the **#246 CI clippy gate** (open **PR #248**, `8bd0b2b`) and closed the **#239 real-Ollama length-recovery smoke** (passed; ROADMAP note pending in the docs commit below). No carried-in code PR remains.

**Context at session start:** the inference layer is feature-complete for Phase 1. What remains is judgement (pedagogy tuning) and device/owner-gated validation. The two autonomously-actionable issues in the queue (#246, #239) are now both handled.

## What we shipped this session

- **#246 CI clippy gate — PR #248 (`8bd0b2b`), open, CI green.** Adds `cargo clippy -p primer-kb-load --features fastembed --all-targets -- -D warnings` to the Linux `feature-combos` job (where the ort-runtime download is already warm; clippy doesn't run tests so no BGE-M3 download). The gate surfaced **two pre-existing latent `uninlined_format_args` warnings** in the fastembed-gated hybrid retrieval-quality tests (`retrieval_quality_hybrid.rs`, `retrieval_quality_hybrid_de.rs`) — fixed inline so the gate is green on landing. CI checks all pass except the unrelated `cargo test` (was still running at handoff; non-blocking — this PR touches only CI yaml + two test assert strings). **Merge it** once `cargo test` goes green.
- **#239 real-provider length-recovery smoke — PASSED (Ollama leg), issue closed.** Forced `max_tokens: 8` (throwaway edit in `turn.rs::stream_inference_response`, **reverted — working tree clean**) against `granite4.1:3b-q8_0` over local Ollama (free, no API burn). Produced the exact `MAX_TRUNCATION_RETRIES=2` + 3-tier ladder signature: **two** `"Oh — I'm sure up to there…"` apologies + **one** `"Let's pause there for now…"` soft-stop. Confirms the Ollama `done_reason: "length"` → `FinishReason::Length` path drives the shared `process_filtered_chunk` recovery ladder end-to-end on a real backend (mirrors the #242 llamacpp confirmation). Recorded in ROADMAP (line ~78). The cloud / openai-compat legs share the same recovery path; only their status-mapping differs and that is host-covered — so #239 is closed as the representative real-backend confirmation.
- **Docs commit (this session):** ROADMAP #239 note + this NEXT_SESSION.md + its handoff snapshot, committed directly to `main` (doc-only; CI `paths-ignore` skips it).

## What's next (concrete acceptance criteria)

### 0. Merge PR #248 (trivial, once `cargo test` is green)
- CI clippy gate. Confirm `cargo test (default features)` passes (the other jobs already passed at handoff), merge, delete the branch.

### 1. ⭐ Pedagogy / answer-quality + rating tuning on the 4B NPU model (owner-in-the-loop) — *the top open question*
The conversation is technically excellent on-device (25.7 tok/s, full multi-turn, PR #227); the open question is pedagogical quality at the compressed 2K-context budget. Owner: *"Quality of answers and ratings will have to be tuned."*
- **Acceptance:** spot-check that the small-context prompt budget (8-turn window, KB top-K 3, ~110-token passage truncation) didn't dull Socratic behaviour (more questions than answers; comprehension-via-transfer); sanity-check `turn_classifications` / `turn_comprehensions` rows on a real device session (pull the session DB, `sqlite3` it). **Define specific tuning targets with the owner first** — judgement, not a blind code change. *(Settings → Diagnostics → "Record QNN per-turn throughput metrics" enables on-device metrics, PR #229.)*

### 2. On-device spot-check of #224 (owner/device-gated; deferred from PR #235)
- **Acceptance:** on the RedMagic 11 Pro with the cl2048 bundle, deliberately overflow a turn and confirm the child sees partial → apology → clean retry, with `genie.log` showing the context-limit status and the retry's smaller prompt succeeding. *(With the small-context budget + per-query `GenieDialog_reset`, overflow is now rare — you may need to force it.)* #243 makes a truncated reasoning-only reply also recover — worth confirming on a reasoning model if one is on-device.
- **Note:** the host-side llamacpp (#242) AND Ollama (#239) analogues of this are now both done; QNN is the remaining device-gated leg.

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

*(#239 and #246 closed this session; #239's PR-equivalent was the doc note, #246's is PR #248.)*

## Open decisions / risks

- **Both real-backend length smokes are now settled.** #242 (llamacpp) and #239 (Ollama) are both behaviourally confirmed against real models. The only remaining length-recovery leg is **QNN on-device (#224)**, which is hardware-gated.
- **No CLI/env override exists for `max_tokens`.** Both #242 and #239 required a throwaway edit to the `GenerationParams` literal in `turn.rs::stream_inference_response` (default `max_tokens: 512` → tiny). Both reverted; tree clean. If forcing this becomes routine, consider a hidden `--max-response-tokens` debug flag — but it's only ever needed for these truncation smokes, so the throwaway-edit-and-revert pattern is fine.
- **#246's gate has teeth and already caught real lints.** Two `uninlined_format_args` warnings had been sitting in the fastembed-gated hybrid tests since they were written, invisible to every CI gate. Expect the gate to occasionally fail a future PR that adds fastembed-gated kb-load test code — that's the point.
- **Length-wins precedence (issue #241) is settled.** A truncated all-reasoning reply triggers recovery; a clean all-reasoning reply still errors. The gate is `chunk.finish_reason == FinishReason::Length` in the shared `process_filtered_chunk` `None` arm. **Do not drop the `Length` gate.**
- **Minor CLAUDE.md staleness (unchanged):** the gotcha line "If a model reasons but emits NO visible answer, the backend sends `InferenceError::ReasoningWithoutAnswer`…" holds only for a *clean* (`Stop`) reply; a *truncated* (`Length`) one recovers instead. A future `/revise-claude-md` pass should add the one-clause caveat.
- **Pedagogy on a 4B NPU model remains the headline open quality question** — technically excellent, pedagogically unverified at scale.

## Patterns to reuse, not reinvent

- **A truncation smoke = build with the backend feature + force `max_tokens` tiny via a throwaway edit (revert after) + drive the REPL over stdin.** For #239 (free, local): `printf 'long-answer question\nquit\n' | ./target/debug/primer --backend ollama --model granite4.1:3b-q8_0 --no-persist --embedder-backend none --classifier/extractor/comprehension-backend stub`. Keep subsystems on `stub` so the smoke isolates the chat-path truncation; `--no-persist` + `--embedder-backend none` skip DB writes and the 570 MB embedder download. The visible signature is N apologies + 1 soft-stop where N = `MAX_TRUNCATION_RETRIES` (= 2 today).
- **When adding a CI feature-gate, run the exact gate command locally first** — it routinely surfaces pre-existing latent lints (the gate is worthless red-on-landing). Fix those inline in the same PR rather than deferring (per the inline-quick-fixes principle).
- **When two terminal outcomes compete at a stream boundary, gate the precedence on the explicit signal, not the default.** `ReasoningWithoutAnswer` vs context-limit recovery: gate on `finish_reason == Length`.
- **A shared streaming helper is the right place for a cross-backend UX fix.** `process_filtered_chunk` is the single byte-stream reasoning step ollama/openai-compat/llamacpp all route through.
- **QNN device facts (still true):** the standalone `qnn_bench` example **can't reach the DSP** from a sideloaded/Termux process on this ROM — on-device throughput is instrumented **inside the APK** (read `<app_data>/.primer/qnn_metrics.jsonl` via `run-as cat`; opt-in via Settings → Diagnostics or `PRIMER_QNN_METRICS_PATH`; bounded 1 MiB + single `.1` backup). Android scoped storage hides `adb`-pushed `/sdcard/Android/data/<pkg>` — stage app-internal via `adb push /data/local/tmp/<f>` → `run-as <pkg> cp`; this RedMagic ROM has **dead logcat + black screencap** — read app-internal files via `run-as <pkg> cat` + owner reads the screen; a reboot leaves `/data/user/0/<pkg>` encrypted until the owner enters the PIN; QNN APK rebuild = `--no-default-features --features qnn`; cargo from `src/` with `+1.88`; commits touching `.github/workflows` need `gh auth refresh -s workflow` (it was already present this session).

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status        # expect clean main at/after 4b6edb3
gh pr checks 248                                 # CI clippy gate — merge once `cargo test` is green

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
- The top remaining open question is pedagogy/answer-quality tuning on the 4B NPU model (owner-in-the-loop) — the conversation is technically excellent on-device but pedagogically unverified at scale.
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
