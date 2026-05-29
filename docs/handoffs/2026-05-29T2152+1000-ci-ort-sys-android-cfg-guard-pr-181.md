# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-29 — Implemented issue **#180** (CI cfg-regression guard for the vendored `ort-sys` android `cache_dir` patch). Added `scripts/check-ort-sys-android-cfg.sh` + a CI step in the `android-cross-compile` job, committed on branch `ci/ort-sys-android-cfg-guard-180` (commit `81bfa17`), pushed, opened as **PR #181** (`Closes #180`). **CI status at handoff: running — confirm green before merging** (`gh pr checks 181`). The prior session's **PR #179** (vendor ort-sys rc.10 android cfg fix) is **merged** to `main` (`5bb838f`); issue **#157** is fixed at the cfg level — the only remaining #157 item is on-device Termux validation of the ONNX-runtime download (developer-side, needs the phone).

## What we shipped this session

### PR #181 — `ci: guard the ort-sys android cache_dir cfg patch (#180)`

- Branch `ci/ort-sys-android-cfg-guard-180`, single commit `81bfa17`. Closes issue #180.
- **The problem:** PR #179's android `cache_dir` cfg patch fixes an E0432 in `ort-sys`'s `build.rs`. Build scripts compile for the **host**, so the bug only manifests when building **natively on an Android host** (Termux). Every current CI job (`android-cross-compile`, `cargo check --features fastembed`) runs on a **Linux host**, so the android arm is never exercised — a regression of the patch would keep CI green while native Termux builds break again.
- **The guard:** `scripts/check-ort-sys-android-cfg.sh` codifies PR #179's manual probe at the **cfg-resolution level** (no Android host, no NDK linker — only `rustc --target aarch64-linux-android --emit=metadata` on a small `build.rs`-mimicking consumer):
  1. **GREEN** — the vendored (patched) `dirs.rs` must compile `cache_dir` for the android target. Failure ⇒ patch regressed.
  2. **TEETH** — a cfg-reverted counterfactual must **fail with E0432**. Proves the guard exercises the patched arm (a guard that can't fail on the unpatched code is worthless).
  - Literal matching (`grep -F` / `awk` index-based), **not** `sed` regex — the patched cfg line contains `[](){}"`. Asserts the patch is still a single line (count == 1), so a rewording fails loudly with "update PATCHED_CFG" rather than silently losing teeth. Honest std-installed precondition (trivial-lib probe, not `--print sysroot` which succeeds even when std is absent).
- **CI wiring:** new step in the existing `android-cross-compile` job, placed **before** the NDK download so a regression fails fast. Runs from `src/` so `rustc` resolves to the workspace-pinned 1.88 toolchain.
- **Verification (local):**
  - Passes on the patched file from `src/` exactly as CI invokes it (`rustc 1.88.0`).
  - Temporarily reverting the **real** `dirs.rs` makes the guard FAIL with E0432, then restores cleanly (empty `git diff`) — proves the GREEN check has teeth too.
  - `shellcheck` clean; `bash -n` clean; `ci.yml` YAML parses.

### Docs

- **ROADMAP.md:** added the new guard to the Linux-CI enumeration in the "Set up CI" bullet.
- **README.md / CLAUDE.md: no change needed.** README doesn't enumerate CI steps; the CLAUDE.md `ort-sys` vendor note already says "keep byte-identical to rc.10 except the one cfg arm" — the guard enforces exactly that, no doc drift.

## What's next — by priority

**First: confirm PR #181 CI is green, then merge.** `gh pr checks 181`. The relevant check is `cargo build (aarch64-linux-android)` — its new "Guard ort-sys android cache_dir cfg patch" step must pass. Docs-only-ish (one script + one CI step + one ROADMAP line); low-risk.

```bash
gh pr merge 181 --squash --delete-branch
git checkout main && git pull
```

Then the standing priority order resumes. The brief's **highest-value follow-up** remains **on-device Termux validation of #157** — does `ort-sys`'s build.rs actually download a prebuilt ONNX Runtime for `aarch64-linux-android` from cdn.pyke.io? Acceptance: on the RedMagic, `cargo build -p primer-cli --features embedding` succeeds natively (Termux), and `--embedder-backend fastembed` runs a hybrid query. Developer-side (needs the phone). Until then the Android default stays `--embedder-backend none`.

### Concrete actionable candidates

- **Step 1.2.6 — Benchmark + thermal harness. Top remaining host-draftable candidate.** Plan §1.2.6: `data/bench/socratic_prompts.jsonl` (30 continuation prompts drawn from `tests/common/en.rs::QUERIES`) + `primer-inference/examples/qnn_bench.rs` (`--bundle-dir`, `--qairt-lib-dir`, `--prompts`, `--duration-secs`, `--thermal-out`). Background tokio task samples `/sys/class/thermal/thermal_zone*/temp` every 2 s → CSV. Report p50/p95 TTFT, mean/min decode tok/s, peak thermal. Pass/fail vs targets (15 tok/s decode, <3 s TTFT, <70 °C). Most of the harness + prompts JSONL + CLI-parse sanity tests draft and test on host; only the measured run needs the phone.
- **Step 1.2.0 — QAIRT SDK install + chatapp_android device validation (developer-side; highest-value QNN pending).** Runbook at `docs/devel/qnn-validation-runbook.md`. Acceptance: handoff doc at `docs/handoffs/2026-MM-DD-qnn-validation-chatapp.md` with decode/prefill tok/s, TTFT, peak thermal. **If decode < 8 tok/s on Qwen3-4B, stop-and-reassess.**
- **#170 Stage A.5 spike (developer-side, unblocks Supertonic Stage C).** Run the one-hour spike on a network-permitted host: `git clone https://huggingface.co/Supertone/supertonic-3` (~400 MB), then `cargo run --example tts_supertonic_hello --features supertonic -- --onnx-dir <v3> --voice-style <v3>/voice_styles/F1.json --lang hi --text "नमस्ते…" --out hi.wav` for hi/de/en. Confirm v2-fork accepts v3 assets unchanged; capture per-language model-load / TTFA / RTF. If it passes, proceed to Stage C.
- **#166 — real-audio multi-utterance Whisper smoke** (PR #164 follow-up). Needs a quiet room + `ggml-small.bin`. Developer-side.
- **Branch protection on `main`** — require the `cargo test (default features)` check before merging. CI runs on PRs (confirmed). Revisit PR #169's `paths-ignore` at the same time. One-time, repo-owner setting; carried-forward as overdue.

### Open queue (issues)

| #   | Title                                                                                                            | State                                        |
| --- | ---------------------------------------------------------------------------------------------------------------- | -------------------------------------------- |
| 180 | CI cannot catch the ort-sys android cache_dir cfg regression                                                     | **FIXED via #181 (pending CI green + merge)** |
| 170 | Stage B: wire Supertonic 3 as a voice-mode TTS backend (Hindi unlock)                                            | Stage B trait impl merged (#175); A.5 spike next |
| 166 | Add real-audio multi-utterance smoke test for WhisperStream cache reuse (PR #164 follow-up)                       | needs human at mic + ggml-small.bin          |
| 163 | split backends_common.rs once the test module grows (post-#149)                                                  | explicit deferral; revisit when a test grows |
| 135 | deps: bump glib 0.18.5 → 0.20+ once Tauri 3 ships (RUSTSEC-2024-0429)                                            | waits on Tauri 3                             |
| 98  | refactor(tests): split `tests/common/sweep.rs` into bm25/hybrid submodules                                        | defer until 3rd locale lands                 |
| 41  | data/ingest: consider scoping disambiguation regex to lead-sentence patterns                                      | self-deferred                                |
| 40  | data/ingest: aggregate per-source attribution for the Wikipedia layer                                             | self-deferred                                |
| 22  | primer-knowledge: cache prepared statements for the corpus-bootstrap path (Phase 0.2)                              | self-deferred                                |
| 21  | CLI: separate `--languages` preference list from bound `--language` locale                                        | self-deferred                                |
| 20  | i18n: placeholder validator can false-fail on translator narrative text                                           | self-deferred                                |

### Carried-forward queue (not in any open issue)

- **#157 on-device validation** (the remaining unknown after #179: ONNX-runtime android-binary download from cdn.pyke.io). Developer-side; highest-value follow-up.
- Step 1.2.0 QAIRT install + on-device validation (highest-value QNN pending; runbook merged via #176).
- Step 1.2.6 benchmark + thermal harness (top automatable; host-drafts).
- Swap `primer-qnn-sys` hand-rolled bindings to `bindgen` over vendored headers once QAIRT licence-redistribution is signed off.
- GUI QNN bundle picker — populate `BackendParams::qnn_bundle_dir` from a settings-page picker in `primer-gui/src/wiring.rs`.
- Supertonic Stages C–F (#170): voice-loop wiring + CLI/GUI flags (C, gated on A.5 spike), asset auto-download + consent (D), A/B latency/quality numbers (E), Hindi preview→stable promotion (F).
- Hindi locale follow-ups — native-speaker review of `prompts/hi.toml`, Hindi children's corpus, real-LLM smoke, flip-to-stable PR.
- OpenAI-compat real-server smoke testing.
- Klexikon corpus expansion past 66 articles.
- Local llama.cpp inference (Phase 1.1) — big feature work.
- Voice-loop hardening — echo cancellation, ambient-noise robustness.
- **CI validation of `cdn.pyke.io` ort-runtime download — then flip default features so hybrid retrieval is on by default.** (Now also relevant to Android via #179, pending the on-device ONNX-download check.)
- **Branch-protection-on-main** — repo owner needs to flip a GitHub setting; **still overdue**. When done, ALSO revisit the workflow-level `paths-ignore` from PR #169.
- CodeQL workflow docs-only skip — repo-owner-only (GitHub-default Analyze setup).
- Swift-side XCTest harness for the sidecar at `crates/primer-speech/swift-sources/Macos26PipelineImpl.swift`.
- Loosen the `voice_loop` parent gate — one-line follow-up from PR #152.
- Continue trimming `backends_common.rs` toward < 500 lines (issue #163).
- macOS-native-26 in CI — needs a hosted `macos-26` runner.
- Full `cargo test` on macOS in CI — runner-billing increase; defer.
- `primer-cli --features speech` on Linux in CI — needs `libasound2-dev`; defer.
- **Local-branch graveyard** — ~35+ stale local branches (mostly merged PRs). Run `commit-commands:clean_gone` to prune the `[gone]` ones when convenient.

## Open decisions / risks

Carried forward — still applicable:

- **#157 is fixed at the cfg level only.** The next layer — `ort-sys` build.rs downloading a prebuilt ONNX Runtime for `aarch64-linux-android` — is genuinely untested and could be the next blocker. Do not advertise "hybrid retrieval works on Android" until the on-device build succeeds end-to-end. Conservative default stays `--embedder-backend none`. **PR #181's guard protects the cfg fix but says nothing about the download** — green CI ≠ Android hybrid works.
- **pykeio/ort rejects AI-assisted PRs (CONTRIBUTING.md).** This is why #157 went the local-vendor route, not upstream. Any upstream contribution of the android `cache_dir` arm must be **hand-authored by a human**. (Upstream main already has unconditional `cache_dir`, so the only upstreamable improvement is marginal.)
- **The vendored `ort-sys` is a maintenance liability tied to the rc.10 pin** — a 5th vendored ort-ecosystem crate (after silero/whisper/piper/supertonic). All five drop together when the workspace can move off `ort = "=2.0.0-rc.10"`. Keep the vendored `ort-sys` byte-identical to rc.10 except the one cfg arm so a re-pin is mechanical. **PR #181's guard now enforces that the cfg arm is present and single-line — if you re-pin or reword the patch, the guard will tell you to update `PATCHED_CFG`.**
- **Supertonic Stage A.5 spike still unverified in practice** (#175 proves compile + unit-tests; v2/v3 ONNX compat needs real assets). Don't wire Stage C until A.5 passes.
- **The 12-turn / 3-passage small-context budget values (step 1.2.5) are untested against a real 4K tokeniser.** Step 1.2.0 / 1.2.6 would confirm. If it overflows, lower `DEFAULT_CONTEXT_WINDOW_TURNS_SMALL_CONTEXT`.
- **The QNN ABI smoke check is still unverified against a real `libGenie.so`** — step 1.2.0's chatapp_android validation is what would empirically prove it.
- **Branch-protection ↔ paths-ignore interaction** from PR #169 — revisit together when the required-check lands.
- **The CodeQL Analyze jobs are NOT skipped on docs-only changes.**
- (… plus all carried-forward decisions/risks from prior briefs — see prior handoffs under `docs/handoffs/`.)

## Patterns to reuse, not reinvent

New from this session:

- **"A guard for a host-specific cfg bug is a `rustc --target` probe, not a `cargo` step."** `cargo build --target X` compiles build scripts for the *host*, so it can never exercise a "fails natively on X" build.rs bug. Compile a tiny `build.rs`-mimicking consumer with `rustc --target X --emit=metadata` (no NDK linker) — that's the only thing that resolves the import under the target's cfg.
- **"A regression guard MUST prove it has teeth."** The script doesn't just assert the patched file compiles — it constructs a cfg-reverted counterfactual and asserts it fails with the *exact* error (E0432). A guard you've never seen fail is a guard you can't trust. (Also re-verified end-to-end by reverting the real `dirs.rs` and watching the guard fail.)
- **"Literal match, not regex, when the needle is code."** The patched cfg line `#[cfg(any(target_os = "linux", target_os = "android"))]` contains `[](){}"`. A `sed s|…|…|` silently mis-matched (the `[` opened a character class) and reported 0 substitutions. `grep -F` + `awk index()` are the right literal tools. (The first script revision shipped the sed bug; caught on first run.)
- **"Self-validate the assumption your guard rests on."** The script asserts the patch is still exactly one line (`grep -Fc … == 1`). If a future edit rewords it, the guard fails loudly with "update PATCHED_CFG" instead of silently degrading into a no-op.
- **"Honest preconditions over reassuring ones."** `rustc --target X --print sysroot` succeeds even when the target std is absent — a false "OK". A trivial-lib probe compile actually detects a missing target and reports it as such, instead of mislabelling it a patch regression.

Carried forward (from prior sessions; see prior handoff trail): read-CONTRIBUTING-before-upstream-PR, build-script-E0432-is-host-cfg-verify-with-rustc-target, `| tail`-masks-exit-code, vendor-verbatim-patch-minimally-document-drop-condition, make-the-CI-guard-cover-the-failure-mode, verify-cross-compile-with-exact-env, don't-conclude-CI-didn't-run-from-an-early-poll, put-cross-crate-naming-const-in-shared-layer, name-config-knob-by-constraint-not-backend, route-every-reader-through-one-effective-value-method, genuine-TDD-red-even-for-trivial-pure-functions, behaviour-preserving-const-tidy-rides-inline, rescue-orphaned-branch-via-clean-cherry-pick, verify-in-isolated-worktree-before-PR, cargo-check-is-not-a-clippy-drift-guard, fix-latent-lint-inline-and-say-so, convert-pip/venv→uv-even-in-docs, feature-gate-the-dispatch-arm-not-the-struct-shape, distinct-error-messages-for-build-vs-runtime, sync-fire-async-drain, post-construction-model-name-rebind, trait-abstraction-at-the-FFI-seam, stack-scoped-guard-for-paired-create/destroy, structured-error-variants-over-Other, mod-with-tests-sibling file-size pattern, capacity-1-channel-as-sync-lever, pure-helper extraction, gate-narrowing, RFC-2229 whole-struct capture, public-re-export protecting call sites from renames, generic-single-slot-cache, `blocking_lock` inside `spawn_blocking`, smoke-check-through-the-streaming-path, `Script` enum-driven mock.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status
git fetch --all --prune
GH_PAGER=cat gh pr view 181 && GH_PAGER=cat gh pr checks 181      # CI-only guard; confirm green before merge

# === Merge #181 (owner's call) once CI is green ===
gh pr merge 181 --squash --delete-branch
git checkout main && git pull

# === Re-run the #180 guard locally (no device, no external assets) ===
cd /Users/hherb/src/primer/src
../scripts/check-ort-sys-android-cfg.sh        # expect: 2× PASS + "OK" (rustc resolves to pinned 1.88 from src/)
# Prove it still has teeth: revert the real dirs.rs and watch the guard fail with E0432
cp src/vendor/ort-sys/src/internal/dirs.rs /tmp/dirs.bak   # (run from repo root)
# …revert the cfg arm, run the guard, expect FAIL exit 1, then restore from /tmp/dirs.bak

# === #157 on-device follow-up (developer-side; the remaining unknown) ===
# On the RedMagic / Termux:  cargo build -p primer-cli --features embedding   # does ort-sys build.rs fetch an android ONNX runtime?
# If yes:  run --backend stub --embedder-backend fastembed  and confirm a hybrid query.
# If the ONNX download has no android arm, document the next blocker in #157 + docs/devel/redmagic-termux-quickstart.md.

# === If picking up step 1.2.6 (benchmark + thermal harness; top host-draftable) ===
# Create data/bench/socratic_prompts.jsonl (30 prompts from tests/common/en.rs::QUERIES)
# + primer-inference/examples/qnn_bench.rs (--bundle-dir --qairt-lib-dir --prompts --duration-secs --thermal-out)
# Host-side: CLI-parse sanity tests + the prompts JSONL. Measured run is the device test.
~/.cargo/bin/cargo run --release --example qnn_bench --features qnn -- --bundle-dir <dir> --duration-secs 900 --thermal-out <csv>

# === If picking up step 1.2.0 (developer-side; follow the merged runbook) ===
# docs/devel/qnn-validation-runbook.md ; decision gate: decode < 8 tok/s on Qwen3-4B -> stop.

# Opt-in to the local pre-commit hook (one-time per clone):
git config core.hooksPath .githooks
```

Carried-forward smokes (unchanged this session):

```bash
# Hindi preview locale (developer-only):
cd /Users/hherb/src/primer/src
RUST_LOG=primer::prompt_pack=warn,info ~/.cargo/bin/cargo run --bin primer -- \
    --backend stub --name Aarav --age 8 --language hi --no-persist

# German retrieval-quality regression benchmarks:
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_de
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid_de

# Python ingestion pipeline tests (uv-only — never invoke pip directly):
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- **If #181 merged:** the `ort-sys` android cfg patch now has a CI regression guard (`scripts/check-ort-sys-android-cfg.sh`, run by the `android-cross-compile` job). The on-device ONNX-runtime-download question (#157) is still the remaining open item — keep `--embedder-backend none` as the Android default until it's validated on the phone.
- Remember the `pykeio/ort` anti-AI-PR policy if any upstream contribution is ever reconsidered.
