# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-05-29T1800+1000 — CI drift-guard follow-up implemented, committed, pushed, opened as **PR #178** (`ci/qnn-android-and-supertonic-clippy`, head `1b386bf`), and **CI is GREEN on the PR** (all four CI jobs + CodeQL passed on the `pull_request` event; both new steps confirmed `success` on the real Linux/Android runners). **PR #178 OPEN, awaiting merge.** Prior session's **PR #177** (per-backend context-window budget, step 1.2.5, `e72b217`) is **merged** to `main` — README/ROADMAP/CLAUDE.md already carry step 1.2.5, nothing further to backfill.

## What we shipped this session

### PR #178 — `ci: cross-compile primer-cli --features qnn + clippy supertonic drift-guards`

- Branch `ci/qnn-android-and-supertonic-clippy`, single code commit `1b386bf` (+ a handoff-docs commit). **One workflow file changed** (`.github/workflows/ci.yml`, +26).
- Closes two carried-forward CI drift-guard gaps from the prior brief:
  1. **`android-cross-compile` job** gains `cargo build --target aarch64-linux-android -p primer-cli --features qnn`. Steps 1.2.2–1.2.5 added the `QnnBackend` impl, the streaming bridge, and the per-backend context budget — all gated on the `qnn` cargo feature. The existing `--bin primer` step builds with the feature **off**, so the entire QnnBackend dep graph (the `Arc<GenieLibrary>` wrapper, the C-ABI token callback, the `spawn_blocking` query path, the `QnnBackend ↔ primer-qnn-sys` FFI seam) was invisible to default Android CI. This step closes that.
  2. **`feature-combos` job** gains `cargo clippy -p primer-speech --features supertonic --all-targets -- -D warnings`. The existing `cargo check --features supertonic` step guards build breaks but **not** lint rot; PR #175 (Supertonic Stage B) shipped a latent clippy warning in `examples/tts_supertonic_hello.rs` that `check` could never catch. **`--all-targets` is load-bearing** — without it, clippy doesn't lint examples, which is exactly where the rot was. (The brief's suggested line omitted `--all-targets`; I added it deliberately so the guard actually covers the example.)
- **Verification — CI on the PR (the authoritative signal):** run `26625201672`, conclusion **success**. Per-step confirmation: `Cross-compile primer-cli --features qnn` → success; `cargo clippy --features supertonic` → success. All four CI jobs (`cargo test`, `cargo check (non-default)`, `cargo clippy (macOS)`, `cargo build (aarch64-linux-android)`) green; CodeQL Analyze (actions/js/python/rust) green.
- **Verification — local pre-push (macOS `aarch64-apple-darwin`):**
  - `cargo build --target aarch64-linux-android -p primer-cli --features qnn` → **Finished in 1m09s**; produced a real `ELF 64-bit LSB pie executable, ARM aarch64` Android binary at `target/aarch64-linux-android/debug/primer`. Used brew NDK **r29** (API-24 clang `aarch64-linux-android24-clang`), with the four `CC_/CXX_/AR_/CARGO_TARGET_..._LINKER` env vars set to exactly match the CI job's `Export NDK toolchain env` step.
  - `cargo clippy -p primer-speech --features supertonic --all-targets -- -D warnings` → **exit 0, 0 warnings**.
- **Local NDK note for the resumer:** NDK already installed via Homebrew at `/opt/homebrew/share/android-ndk` (r29; CI uses r26d — both target API 24, both produced a clean build); `ANDROID_HOME=/Users/hherb/Library/Android/sdk`; `aarch64-linux-android` rust target already added to the pinned 1.88 toolchain. No install needed to re-verify.

### README / ROADMAP / CLAUDE.md

**No changes needed.** This is a CI-internals-only change. README and ROADMAP are product/roadmap docs and carry no CI-coverage claims (`grep` confirmed: no `cross-compile` / `drift-guard` / `features qnn` references in either). CLAUDE.md's android bullet already says the CI job "enforces this as a drift-guard on every push and PR" — accurate (CI does run on PRs; confirmed this session), and concerns the pre-existing job, so left untouched.

## What's next — by priority

**First: merge #178.** CI-only change, green on the PR, low-risk. After merge, optionally confirm the post-merge `main` push run stays green (`gh run list --branch main --limit 1`), though the PR run already validated everything.

Then the standing priority order resumes:

### Concrete actionable candidates

- **Step 1.2.6 — Benchmark + thermal harness. The top remaining automatable candidate (host-side drafting).** Plan §1.2.6: `data/bench/socratic_prompts.jsonl` (30 continuation prompts drawn from `tests/common/en.rs::QUERIES` + seeded "child→Primer→child" continuations) + `primer-inference/examples/qnn_bench.rs` (`--bundle-dir`, `--qairt-lib-dir`, `--prompts`, `--duration-secs`, `--thermal-out`). Background tokio task samples `/sys/class/thermal/thermal_zone*/temp` every 2 s → CSV. Report p50/p95 TTFT, mean/min decode tok/s, peak thermal. Pass/fail vs targets (15 tok/s decode, <3 s TTFT, <70 °C). **Most of the harness + the prompts JSONL + CLI-parse sanity tests draft and test on host; only the actual measured run needs the phone.** Also update `docs/devel/redmagic-termux-quickstart.md` (QAIRT section, `--backend qnn` row, "Run the benchmark" pointer).
- **Step 1.2.0 — QAIRT SDK install + chatapp_android device validation (developer-side; highest-value pending).** Cannot be automated. **Runbook exists** at `docs/devel/qnn-validation-runbook.md` (merged via #176) — follow it. Acceptance: handoff doc at `docs/handoffs/2026-MM-DD-qnn-validation-chatapp.md` with decode/prefill tok/s, TTFT, peak thermal across a 5-min session. **If decode < 8 tok/s on Qwen3-4B, stop-and-reassess.** Every subsequent device-side step depends on it.
- **#170 Stage A.5 spike (developer-side, unblocks Supertonic Stage C).** #175 merged the trait impl; before Stage C wiring, run the one-hour spike on a network-permitted host: `git clone https://huggingface.co/Supertone/supertonic-3` (~400 MB), then `cargo run --example tts_supertonic_hello --features supertonic -- --onnx-dir <v3> --voice-style <v3>/voice_styles/F1.json --lang hi --text "नमस्ते…" --out hi.wav` for hi/de/en. Confirm v2-fork accepts v3 assets unchanged (the load-bearing assumption), capture per-language model-load / TTFA / RTF. If it passes, proceed to Stage C.
- **#157 — upstream PR to `pyke/ort`** adding a `target_os = "android"` arm in `internal::dirs::cache_dir`. Unblocks hybrid retrieval AND Supertonic on Android Termux. ~1 hr + upstream review.
- **#166 — real-audio multi-utterance Whisper smoke** (PR #164 follow-up). Needs a quiet room + `ggml-small.bin`. Developer-side.
- **Branch protection on `main`** — require the `cargo test (default features)` check before merging. CI runs on PRs (confirmed this session — run `26625201672` was a `pull_request` event), so a required PR check is viable. Revisit PR #169's `paths-ignore` at the same time (a docs-only PR would skip the workflow entirely, so a required-but-never-run check could wedge it — see the workflow-level rationale comment in `ci.yml` lines 20–30 for the two documented mitigations). One-time, repo-owner setting; carried-forward as overdue.

### Open queue (issues)

| #   | Title                                                                                                            | State                                        |
| --- | ---------------------------------------------------------------------------------------------------------------- | -------------------------------------------- |
| 170 | Stage B: wire Supertonic 3 as a voice-mode TTS backend (Hindi unlock)                                            | Stage B trait impl merged (#175); A.5 spike next |
| 166 | Add real-audio multi-utterance smoke test for WhisperStream cache reuse (PR #164 follow-up)                       | needs human at mic + ggml-small.bin          |
| 163 | split backends_common.rs once the test module grows (post-#149)                                                  | explicit deferral; revisit when a test grows |
| 157 | fastembed/ort-sys cannot build for `aarch64-linux-android` (no Android arm in ort-sys cache_dir cfg)             | upstream — needs PR to `pyke/ort`            |
| 135 | deps: bump glib 0.18.5 → 0.20+ once Tauri 3 ships (RUSTSEC-2024-0429)                                            | waits on Tauri 3                             |
| 98  | refactor(tests): split `tests/common/sweep.rs` into bm25/hybrid submodules                                        | defer until 3rd locale lands                 |
| 41  | data/ingest: consider scoping disambiguation regex to lead-sentence patterns                                      | self-deferred                                |
| 40  | data/ingest: aggregate per-source attribution for the Wikipedia layer                                             | self-deferred                                |
| 22  | primer-knowledge: cache prepared statements for the corpus-bootstrap path (Phase 0.2)                              | self-deferred                                |
| 21  | CLI: separate `--languages` preference list from bound `--language` locale                                        | self-deferred                                |
| 20  | i18n: placeholder validator can false-fail on translator narrative text                                           | self-deferred                                |

### Carried-forward queue (not in any open issue)

- **CI follow-up DONE this session** (the qnn cross-compile + supertonic clippy lines landed in #178 and are green on the PR). Item retired.
- Step 1.2.0 QAIRT install + on-device validation (highest-value pending; developer-side; runbook merged via #176).
- Step 1.2.6 benchmark + thermal harness (top automatable; host-drafts).
- Swap `primer-qnn-sys` hand-rolled bindings to `bindgen` over vendored headers once QAIRT licence-redistribution is signed off.
- GUI QNN bundle picker — populate `BackendParams::qnn_bundle_dir` from a settings-page picker in `primer-gui/src/wiring.rs`. (The GUI already inherits the 1.2.5 budget automatically — it builds `PedagogyConfig` via `..default()`, so once a qnn backend is wired the 12/3 budget applies with no further change.)
- Supertonic Stages C–F (#170): voice-loop wiring + CLI/GUI flags (C, gated on A.5 spike), asset auto-download + consent (D), A/B latency/quality numbers (E), Hindi preview→stable promotion (F).
- Hindi locale follow-ups — native-speaker review of `prompts/hi.toml`, Hindi children's corpus, real-LLM smoke, flip-to-stable PR.
- OpenAI-compat real-server smoke testing.
- Klexikon corpus expansion past 66 articles.
- Local llama.cpp inference (Phase 1.1) — big feature work.
- Voice-loop hardening — echo cancellation, ambient-noise robustness.
- CI validation of `cdn.pyke.io` ort-runtime download — then flip default features so hybrid retrieval is on by default.
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

Newly surfaced this session:

- **`--all-targets` deviation from the brief's suggested clippy line was intentional.** The brief said `cargo clippy -p primer-speech --features supertonic -- -D warnings`. That form does NOT lint examples — but the rot the guard exists to catch was *in an example*. I added `--all-targets`. Verified clean both locally and on the PR's Linux `feature-combos` runner. If a future maintainer wonders why the CI line differs from the handoff suggestion, this is why.
- **CI is confirmed to run on `pull_request` events in this repo.** (Noting explicitly because an earlier draft of this brief wrongly suspected otherwise from premature polling — corrected after observing run `26625201672`, a `pull_request`-triggered run, go fully green. Don't repeat that mistake: `gh pr checks <n>` and the run's `event` field are the truth; give the run ~20–30 s to be created before concluding it doesn't exist.)

Carried forward — still applicable:

- **Supertonic Stage A.5 spike still unverified in practice.** #175 proves the feature compiles + unit-tests on host; the v2/v3 ONNX compatibility check needs real Supertonic 3 assets + a listen test. Don't wire Stage C until A.5 passes.
- **The 12-turn / 3-passage small-context budget values (step 1.2.5) are untested against a real 4K tokeniser.** Principled estimates, not measured. Step 1.2.0's on-device validation (or 1.2.6's bench) is what would confirm the prompt fits 4K with headroom. If it overflows, lower `DEFAULT_CONTEXT_WINDOW_TURNS_SMALL_CONTEXT` (a single const). LTM top-K (`LTM_FINAL_TOP_K`=3) was deliberately NOT shrunk for small-context backends — it's the next knob if 4K still overflows.
- **The QNN ABI smoke check is still unverified against a real `libGenie.so`** — step 1.2.0's chatapp_android validation is what would empirically prove it.
- **Branch-protection ↔ paths-ignore interaction** from PR #169 — revisit together when the required-check lands.
- **The CodeQL Analyze jobs are NOT skipped on docs-only changes.**
- (… plus all carried-forward decisions/risks from prior briefs — see the prior handoff trail under `docs/handoffs/`.)

## Patterns to reuse, not reinvent

New from this session:

- **"Make the CI guard actually cover the failure mode it exists for."** The brief's suggested supertonic clippy line lacked `--all-targets`, which would have made the guard a no-op for the *example* rot it was created to catch. Always check the guard's scope covers the failure mode, not just the happy-path lib build.
- **"Verify a cross-compile CI step locally with the EXACT env the job sets."** Before adding the Android qnn line, I exported the same four `CC_/CXX_/AR_/CARGO_TARGET_..._LINKER` vars the `Export NDK toolchain env` step writes, then built. Confirmed a real aarch64 ELF, not just a no-op. Cheap insurance that the CI line will work, not just parse.
- **"Don't conclude CI 'didn't run' from a too-early poll."** A `pull_request` run takes ~20–30 s to be created; polling at +5 s and seeing nothing is not evidence of absence. Check the run's `event` field and `gh pr checks` after a short wait, and prefer the `Monitor` tool's until-loop over manual sleep chains (which the harness blocks anyway).

Carried forward (from prior sessions; see prior handoff trail): put-cross-crate-naming-const-in-shared-layer, name-config-knob-by-constraint-not-backend, route-every-reader-through-one-effective-value-method, genuine-TDD-red-even-for-trivial-pure-functions, behaviour-preserving-const-tidy-rides-inline, rescue-orphaned-branch-via-clean-cherry-pick, verify-in-isolated-worktree-before-PR, cargo-check-is-not-a-clippy-drift-guard, fix-latent-lint-inline-and-say-so, convert-pip/venv→uv-even-in-docs, feature-gate-the-dispatch-arm-not-the-struct-shape, distinct-error-messages-for-build-vs-runtime, sync-fire-async-drain for Send-only FFI traits, post-construction-model-name-rebind, `required_if_eq`+`env` clap composition, trait abstraction at the FFI seam, stack-scoped guard for paired C-API create/destroy, structured error variants over `Other(String)` across i18n, `#[serde(default)]` + tolerated unknown fields, mod-with-tests-sibling file-size pattern, verify-locally-first-then-CI, two-layer drain, capacity-1-channel-as-sync-lever, pure-helper extraction, gate-narrowing, RFC-2229 whole-struct capture, public-re-export protecting call sites from renames, generic-single-slot-cache + mechanical-caller-wiring, `blocking_lock` inside `spawn_blocking`, smoke-check-through-the-streaming-path, `Script` enum-driven mock.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status
git fetch --all --prune
gh pr view 178 && gh pr checks 178      # CI-only change; green on the PR at handoff time

# === Merge #178 (owner's call), then optionally confirm the post-merge main run ===
gh pr merge 178 --squash --delete-branch
git checkout main && git pull
GH_PAGER=cat gh run list --branch main --limit 1 --json databaseId,status,conclusion

# === Re-verify #178's two steps locally (NDK already installed: brew r29) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo clippy -p primer-speech --features supertonic --all-targets -- -D warnings   # exit 0, 0 warnings
NDK_BIN=/opt/homebrew/share/android-ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin
export CC_aarch64_linux_android="$NDK_BIN/aarch64-linux-android24-clang"
export CXX_aarch64_linux_android="$NDK_BIN/aarch64-linux-android24-clang++"
export AR_aarch64_linux_android="$NDK_BIN/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$NDK_BIN/aarch64-linux-android24-clang"
~/.cargo/bin/cargo build --target aarch64-linux-android -p primer-cli --features qnn   # ~1m; produces aarch64 ELF
file target/aarch64-linux-android/debug/primer   # expect: ELF 64-bit … ARM aarch64

# Opt-in to the local pre-commit hook (one-time per clone):
git config core.hooksPath .githooks

# === Re-verify step 1.2.5 (merged in #177) locally (no external assets needed) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test --workspace                                      # expect 884 / 0
~/.cargo/bin/cargo test --workspace --features primer-cli/qnn            # expect 932 / 0
~/.cargo/bin/cargo fmt --all -- --check                                  # clean
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings       # clean
~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-cli/qnn -- -D warnings  # clean

# === PR #167 regression suite (macOS feature-combos drift-guard) ===
~/.cargo/bin/cargo clippy -p primer-speech --features macos-native --all-targets -- -D warnings
~/.cargo/bin/cargo clippy -p primer-speech --features voice-loop,macos-native --all-targets -- -D warnings
~/.cargo/bin/cargo clippy -p primer-cli --features speech,macos-native --all-targets -- -D warnings

# === If picking up step 1.2.6 (benchmark + thermal harness) — top automatable candidate ===
# Goal: feat(qnn): benchmark example and thermal capture — see plan §1.2.6
# Create:
#   - data/bench/socratic_prompts.jsonl   (30 continuation prompts; draw from tests/common/en.rs::QUERIES)
#   - primer-inference/examples/qnn_bench.rs  (--bundle-dir --qairt-lib-dir --prompts --duration-secs --thermal-out)
#       * per prompt: measure TTFT + decode tok/s; loop until --duration-secs elapsed
#       * background tokio task: sample /sys/class/thermal/thermal_zone*/temp every 2s -> CSV
#       * final report p50/p95 TTFT, mean/min decode tok/s, peak thermal; pass/fail vs 15 tok/s / <3s / <70°C
#   - update docs/devel/redmagic-termux-quickstart.md (QAIRT section, --backend qnn row, "Run the benchmark")
# Host-side: CLI-parse sanity tests + the prompts JSONL. The measured run is the device test.
~/.cargo/bin/cargo run --release --example qnn_bench --features qnn -- --bundle-dir <dir> --duration-secs 900 --thermal-out <csv>

# === If picking up step 1.2.0 (developer-side; follow the merged runbook) ===
# See docs/devel/qnn-validation-runbook.md for the full step-by-step.
# Decision gate: if decode < 8 tok/s on Qwen3-4B, stop and reassess.
# Write up to docs/handoffs/2026-MM-DD-qnn-validation-chatapp.md.

# === If picking up #170 Stage A.5 spike (gates Supertonic Stage C; needs network + ~400 MB) ===
git clone https://huggingface.co/Supertone/supertonic-3 /tmp/supertonic-3
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --example tts_supertonic_hello --features supertonic -- \
    --onnx-dir /tmp/supertonic-3 \
    --voice-style /tmp/supertonic-3/voice_styles/F1.json \
    --lang hi --text "नमस्ते, आज आप क्या सीखना चाहते हैं?" --out /tmp/hi.wav
# Repeat for --lang de and --lang en. Capture model-load / TTFA / RTF per language.
# Confirm the v2-fork accepts v3 assets unchanged (the load-bearing assumption).
```

Carried-forward smokes (unchanged this session):

```bash
# Hindi preview locale (developer-only):
cd /Users/hherb/src/primer/src
RUST_LOG=primer::prompt_pack=warn,info ~/.cargo/bin/cargo run --bin primer -- \
    --backend stub --name Aarav --age 8 --language hi --no-persist

# OpenAI-compat smoke (spin up a local server first):
llama-server --port 8000 --model /path/to/some.gguf
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo run --bin primer -- \
    --backend openai-compat --openai-compat-url http://localhost:8000 \
    --model <model-id-from-server> --name SmokeTester --age 9 --no-persist --verbose

# German retrieval-quality regression benchmarks:
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_de
~/.cargo/bin/cargo test -p primer-kb-load --test retrieval_quality_hybrid_de
~/.cargo/bin/cargo test -p primer-kb-load --features fastembed --test retrieval_quality_hybrid_de

# Python ingestion pipeline tests (uv-only — never invoke pip directly):
cd /Users/hherb/src/primer/data/ingest
.venv/bin/pytest tests/
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- If you exposed bugs in existing behaviour, flag them separately from the assigned task.
- **If #178 merged:** nothing further to update — README/ROADMAP/CLAUDE.md need no CI-coverage edits, and the steps already passed on the PR run.
- If picking up step 1.2.6: the QnnBackend construction path is fully wired (PR #174); the bench just needs `--bundle-dir`/`--qairt-lib-dir` and a real bundle. Draft + test the harness on host, gate the measured numbers behind device availability, and `log()` any coverage you had to skip (no silent caps).
- If picking up #170 Stage A.5/C: the trait impl is merged (#175); don't wire Stage C until the A.5 v2/v3 ONNX compat spike passes on a real asset set.
