# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-05 — **Landed PR #200 (LlamaCppBackend), set up branch protection on `main`, and shipped PR #202 (llamacpp GPU-feature-chain bugfix + 2 review follow-ups).** This was a `/nextsession` continuation: the prior session's PR #200 was open with CI pending; this session confirmed CI green, merged it, then took the owner-chosen "both: branch protection + the minor code follow-ups" path. While doing the follow-ups it discovered (and fixed) a real user-facing bug: GPU-variant llamacpp builds silently fell back to the stub backend.

**Context at session start:** `main` @ `ae7342e` (PR #200 already squash-merged at 02:51 UTC — the prior brief was written before the merge completed; CI had in fact passed). Working tree clean. Branches: `main`, `backup/pre-rebase-stageB` (unchanged — owner decision still pending), plus a now-deleted `feat/llamacpp-backend` (PR #200's branch). **End state:** `main` @ `87c1a63`, clean; branches `main` + `backup/pre-rebase-stageB` only.

## What we shipped this session

### 1. Landed PR #200 (Phase 1.1 bullet a — LlamaCppBackend)
- Confirmed all 9 CI checks green; squash-merged as **`ae7342e`**; deleted the local + remote `feat/llamacpp-backend` branch and pruned the stale tracking ref.

### 2. Branch protection on `main` (the long-overdue infra item)
- Applied via the GitHub API. **Required status check: `cargo test (default features)`** (strict = branch must be up to date before merge). Force-pushes blocked, deletions blocked. `enforce_admins: false` (owner can still admin-override a wedged check). Verified live — PR #202 could only merge after the check passed.
- This is the structural fix the CLAUDE.md "Branch protection is the structural fix for `main`" note (issue #96) called for. **No CronCreate/schedule artifact** — it's a one-time config, done.

### 3. PR #202 — `fix(inference): chain llamacpp GPU variants to base feature; llamacpp follow-ups` → squash-merged as **`87c1a63`**
**The bug (found while doing the review follow-ups):** the `llamacpp-metal`/`-cuda`/`-vulkan` cargo features in **primer-engine** (and primer-cli/primer-gui) did NOT enable the base `llamacpp` feature, but `build_llamacpp_backend`'s real arm is `#[cfg(feature = "llamacpp")]`-gated in primer-engine. So a GPU-only build compiled llama.cpp fully (via `primer-inference/llamacpp-metal` → base) yet left **primer-engine's** base `llamacpp` inactive → compiled the `not(feature)` **stub** → returned the "rebuild with --features llamacpp" hint at runtime. The prior brief's own owner-gated REPL command (`cargo run --features llamacpp-metal -- --backend llamacpp …`) would have failed despite a correct build.

**Fix:** every crate's GPU variant now chains to its base `llamacpp` feature, mirroring `primer-inference`'s own `llamacpp-metal = ["llamacpp", …]`. Proven deterministically (no compile) via `cargo tree` — primer-engine's active feature set now includes `llamacpp` for metal/cuda/vulkan (was missing before).

**Two minor review follow-ups folded in (from PR #200's review thread):**
- **engine.rs:** guard empty `tokens` before `tokens.len() - 1` (would underflow → panic/UB if a model had no BOS + empty prompt; `AddBos::Always` makes it practically unreachable, but the guard turns it into a clean `InferenceError`).
- **main.rs:** `#[cfg(feature = "llamacpp")]`-gate the `--llamacpp-gpu-layers` / `--llamacpp-n-ctx` flag declarations + their `BackendParams` reads, so `--help` stays clean on the default REPL build (mirrors qnn). The base-feature chaining keeps them visible on GPU builds.
- The **third** review item (stop-sequence trim leaking the matched marker) was **already fixed in the merged PR #200** via `visible_prefix_before_stop` (well-tested in `params.rs`) — no-op this session.

**Docs:** CLAUDE.md's llamacpp bullet gained the GPU-variant feature-chaining gotcha + the `cargo tree` verification recipe. README/ROADMAP already described GPU offload as landed — this fix makes that claim actually functional, so no text change was needed there.

**Verification (all from `src/`, `+1.88`, green):** `fmt --check` clean; `clippy --workspace --all-targets -D warnings` clean; `test --workspace` green; `clippy -p primer-cli --features llamacpp --all-targets -D warnings` clean (compiled llama.cpp CPU + the now-active real `build_llamacpp_backend` arm + gated flags + the engine guard). PR #202 CI: all 8 checks green incl. `cargo check (non-default features)` (proves the cfg-gating compiles out cleanly) and the required `cargo test (default features)`.

## What's next — by priority

### Owner-gated Phase 1.1 follow-ups (NOT done — need a human + hardware)
These are unchanged from the prior brief and remain the highest-value real validation:
1. **Real-model smoke** (the ONLY end-to-end generation validation no autonomous session can do — no GGUF is downloaded). Download a small GGUF (1–3B Q4_K_M) and run:
   `cd src && PRIMER_LLAMACPP_TEST_GGUF=/path/to/model.gguf ~/.cargo/bin/cargo +1.88 test -p primer-inference --features llamacpp-metal --test llamacpp_smoke -- --ignored --nocapture`
   Acceptance: prints a non-empty sentence; no panic.
2. **GPU REPL end-to-end** (now actually reaches the real backend after this session's fix — previously it returned the stub hint):
   `cd src && ~/.cargo/bin/cargo +1.88 run --bin primer --features llamacpp-metal -- --backend llamacpp --model /path/to/model.gguf --name Binti --age 8`
   Acceptance: streamed Socratic reply, not the "rebuild with --features llamacpp" message.
3. **GUI click-through.** `cd src && ~/.cargo/bin/cargo run --bin primer-gui --features llamacpp-metal`. Settings → Inference backend → llamacpp; set a GGUF path; Save & start a session; confirm a streamed reply.

### Phase 1.1 bullets still open (deferred by owner in the PR #200 session)
- **Bullet (b) — benchmarking.** Qwen3 7B Q4_K_M on MacBook (Metal), DGX (CUDA), RedMagic (Vulkan): tok/s + TTFT. Needs models + the three machines. Mirror the macos-native-26-vs-Whisper probe (PR #131) for the harness shape.
- **Bullet (c) — automatic local→cloud fallback + 3B fallback path.** Overlaps Phase 1.3 (inference router); design them together. If `llamacpp:`-named small models warrant the constrained pedagogy budget, revisit `is_small_context_backend` (today llamacpp is NOT small-context).

### Other standing items (carried, unchanged)
- **#170 Supertonic Stages D (GUI click-through + real-audio, owner/hardware-gated), E (in-loop A/B numbers), F (Hindi preview→stable).**
- **#192 / #166** — human-at-mic smokes (macOS-native STT; Whisper multi-utterance).
- **Step 1.2.0** — QAIRT install + `qnn_bench` device validation (runbook `docs/devel/qnn-validation-runbook.md`).
- **#157** — on-device Termux ONNX-runtime validation (Android stays BM25-only until proven).
- **#163** (split `backends_common.rs`) and **#98** (split `tests/common/sweep.rs`) — both still trigger-gated, NOT ready.
- **#135** — bump glib once Tauri 3 ships.

### Open queue (issues)

| #   | Title | State |
| --- | --- | --- |
| 170 | Supertonic 3 voice-mode TTS | Stage D shipped; E/F + manual gates next |
| 192 | Manual smoke: macOS-native STT + injected TTS | needs human at mic |
| 166 | Real-audio multi-utterance Whisper smoke | needs human at mic + ggml-small.bin |
| 163 | split backends_common.rs once test module grows | NOT triggered |
| 135 | bump glib → 0.20+ once Tauri 3 ships (RUSTSEC-2024-0429) | waits on Tauri 3 |
| 98  | split tests/common/sweep.rs | defer until 3rd locale |

## Open decisions / risks

- **Branch protection is now ACTIVE on `main`** (required check: `cargo test (default features)`, strict; force-push/delete blocked; `enforce_admins: false`). It does its core job: blocking untested **code** from `main`.
- **⚠️ Docs-only PRs are DEADLOCKED through the normal flow — admin-merge them.** This is the documented caveat in `.github/workflows/ci.yml` (issue #168): the CI workflow has `paths-ignore: ['**/*.md', 'docs/**', …]` to save macOS runner minutes, so a docs-only change never triggers the Rust workflow → `cargo test (default features)` never runs → the required check stays pending → `mergeStateStatus: BLOCKED` forever. **This session hit it on the docs PR #203 and resolved it with `gh pr merge 203 --squash --admin`.** Because `enforce_admins: false`, the admin owner can also just `git push origin main` docs directly (the old handoff-to-main flow still works for the admin). **The ci.yml comment lists the proper zero-friction fix** if the owner ever wants docs PRs to merge without `--admin`: a "skipped-but-required" stub — a second workflow (or per-job `paths` allowlist) that emits a passing `cargo test (default features)` status on docs-only paths. It's fiddly (duplicate-check-name / mutually-exclusive-paths sharp edges) and was deliberately NOT rushed this session; flag for the owner as an optional infra follow-up. **Net rule for the next session: code PRs go through CI (enforced); docs-only PRs use `--admin` or an admin direct-push.**
- **The real llama.cpp model has STILL never been run by any autonomous session** — no GGUF was downloaded. The backend's decode loop / GGUF load / chat-template / Metal offload are verified to COMPILE, the orchestration is host-tested via the mock, and (new this session) the GPU-variant feature resolution + real-arm compilation are proven. But the actual generation round-trip remains owner-gated (the `#[ignore]` smoke + the GPU REPL command above). Treat "it generates real text on GPU" as unverified until one of those runs.
- **`llama-cpp-2` 0.1.146 is a 0.1.x crate (patch releases can break).** Pinned `"0.1.146"` in `[workspace.dependencies]`. A `cargo update` could pull a breaking 0.1.x patch; the signature reconciliations (u32 `with_n_gpu_layers`; `token_to_piece` + `encoding_rs`) live only in the feature-gated `real` module of `crates/primer-inference/src/llamacpp/engine.rs`.
- **`backup/pre-rebase-stageB` still KEPT** (unchanged). Tip `6378316`, not in `main` — intentional pre-rebase Stage B snapshot. Owner decision: `git branch -D` it if the Stage B work is fully merged and the snapshot is no longer wanted.
- **Carried (still true):** `--languages` (#21) seeds a *fresh* learner only. Supertonic uses **OpenRAIL-M** weights — licence read required before any Stage E/F default-path flip.

## Patterns to reuse, not reinvent

New from this session:
- **A `#[cfg(feature = "X")]`-gated build arm only works if every downstream crate's *variant* features chain back to the base `X`.** The trap: `primer-inference/llamacpp-metal` chained to `llamacpp`, but `primer-engine/llamacpp-metal` did not — so the engine's `#[cfg(feature = "llamacpp")]` arm silently configured out under a GPU build. **Verify feature resolution with `cargo tree -p <crate> --features <variant> -f '{p} -> {f}' | grep <dep>`** — it resolves features WITHOUT compiling, so it's a near-instant proof for build-system changes (no need to compile llama.cpp to check the cfg logic). When you add a "base + accelerator-variant" feature matrix, make the variants chain to the base in *every* crate, top to bottom.
- **A discovered bug adjacent to the assigned work is worth a separate, clearly-labelled fix in the same PR.** The owner asked for 3 cosmetic follow-ups; one was already fixed, and chasing the third (the CLI flag cfg-gate) surfaced the real feature-chain bug. Bundling them as one cohesive "llamacpp follow-ups" PR with the bug called out first is cleaner than splitting hairs.
- **Guard an established-invariant panic path with a cheap explicit error rather than relying on the invariant.** `tokens.len() - 1` is safe under `AddBos::Always`, but a 4-line `is_empty()` guard converts a would-be underflow panic/UB into a clean `InferenceError` for free — do this whenever an upstream invariant feeds an unchecked subtraction/index.
- **PR-first even for a one-line-feel change, now that branch protection is on.** Direct pushes to `main` are gated; the verify loop → push branch → open PR → watch CI → squash-merge cadence is the path. A `Monitor` poll-loop over `gh pr checks <n>` (emit each terminal state, break when none pending) is the right way to wait for CI without burning cache.

Carried forward (prior handoffs): for a new in-process inference backend, the QNN/llamacpp trait-seam + mock + `spawn_blocking` + two-cfg `build_*_backend` free-fn is the template (reuse wholesale for RKNN); push the feature gate as deep as possible (gate only the real engine body so the bridge + reasoning strip + pure helpers stay CI-covered via a mock); put cross-cutting transforms in the always-compiled bridge, not the gated engine; a process-wide `OnceLock` init must read-back the cell on `Err`, not propagate; surface an issue-author's enumerated options to the owner before designing; nullable self-FK for parent/umbrella; `#[serde(default)] Option<NestedStruct>` for backward-compatible JSONL; regenerate goldens from the deterministic emitter; `Connection::prepare_cached(&stable_sql)` bulk-write idiom; a `backup/*` branch whose tip isn't in `main` is deliberate — surface, don't delete; `git mv` to split an over-500-line module; clap `required_if_eq` is blind to defaults (ArgGroup + runtime validator); a hidden GUI form field must still be sent by `gather()` if its DTO field is mandatory; `--features X` clippy is invisible to `--workspace` clippy — run it explicitly; run cargo from `src/` with `+1.88` so the pin is honored.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status                                    # clean
git branch                                    # main, backup/pre-rebase-stageB
git log --oneline -3                          # 87c1a63 (#202), ae7342e (#200), 699f053
gh api repos/hherb/primer/branches/main/protection --jq '.required_status_checks.checks[].context'
                                              # -> "cargo test (default features)"  (protection active)

# === Rust verify loop (ALWAYS from src/, with +1.88) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo +1.88 test --workspace --no-fail-fast
# Feature-gated clippy is NOT covered by --workspace — run explicitly when touching llamacpp/voice:
~/.cargo/bin/cargo +1.88 clippy -p primer-cli --features llamacpp --all-targets -- -D warnings
# Verify a feature-matrix change without compiling llama.cpp:
~/.cargo/bin/cargo +1.88 tree -p primer-cli --features llamacpp-metal -f '{p} -> {f}' | grep primer-engine
#   -> must include "llamacpp" in the active feature list

# === Owner-gated: real-model llama.cpp smoke (download a small GGUF first) ===
cd /Users/hherb/src/primer/src
PRIMER_LLAMACPP_TEST_GGUF=/path/to/model.gguf \
  ~/.cargo/bin/cargo +1.88 test -p primer-inference --features llamacpp-metal \
  --test llamacpp_smoke -- --ignored --nocapture
# Or run the REPL against a local GGUF on GPU (now reaches the real backend after PR #202):
~/.cargo/bin/cargo +1.88 run --bin primer --features llamacpp-metal -- \
  --backend llamacpp --model /path/to/model.gguf --name Binti --age 8

# === New work: PR-first (branch protection is on) ===
git checkout -b <branch> main
# ... edit, verify loop above ...
git push -u origin <branch> && gh pr create --base main ...
gh pr checks <n>                              # wait green, then:
gh pr merge <n> --squash --delete-branch
# DOCS-ONLY PRs: CI is path-ignored (#168) so the required check never runs →
# the PR is BLOCKED. Admin-merge instead (or admin direct-push docs to main):
gh pr merge <n> --squash --delete-branch --admin
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task. (This session's assigned task was 3 cosmetic follow-ups; chasing them exposed a real user-facing bug — GPU-variant llamacpp builds fell back to the stub — which was fixed in PR #202 and called out as the headline. Item 1 was already fixed in the merged code; the review process from PR #200 catching latent issues, then this session closing the loop, is the intended cadence.)
- **This session's open caveat:** the real-model generation round-trip is STILL owner-gated (no GGUF downloaded). Both the `#[ignore]` smoke and the GPU REPL command remain unverified end-to-end. Phase 1.1 bullets (b) benchmarking and (c) local→cloud fallback are still deferred by owner choice.
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
