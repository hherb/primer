# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-15 — **We have real on-device QNN throughput numbers, and they're excellent: ~25.7 tok/s sustained decode on the Hexagon NPU (1.7× the ≥15 target), TTFT p95 ≈ 2.6 s (< 3 s target), peak ~52 °C.** That's ~2.7× the old chatapp-proxy placeholder (~9.4 tok/s) and matches the owner's "near-instant, feels like a MacBook" impression. Getting the number required two detours that are now fixed: the documented `qnn_bench` path **can't reach the DSP** from a sideloaded process on this ROM (so throughput is now instrumented *inside the APK*), and a **PR #225 boot regression** (a `const`-TDZ that aborted GUI boot before the session picker) had to be fixed first.

**Context at session start:** clean `main` at `5b26cac` (PR #225 responsive-GUI merged). **End state:** all work on branch `qnn-per-turn-throughput-metrics`, opened as **PR #227** (5 commits; CI running — merge when green). Device (RedMagic 11 Pro, `912607710061`) runs the PR #227 APK; cl2048 bundle staged; CmaFree ~631 MB.

## What we shipped this session (PR #227, branch `qnn-per-turn-throughput-metrics`)

1. **`cdb2f80` — feat(qnn): per-turn throughput metrics inside the APK.** The standalone `examples/qnn_bench.rs` can't reach the Hexagon DSP from a sideloaded/Termux process on this ROM (FastRPC node is SELinux-gated to packaged apps — see [[project_qnn_dsp_needs_app_packaging]]), so `QnnBackend::generate_stream` now records a per-turn JSONL line (TTFT, decode tokens, decode ms, tok/s) to `<app_data>/.primer/qnn_metrics.jsonl`, read via `run-as cat`. New `qnn::metrics` module (`MeteredStream` decorator + pure `format_metric_line` + never-panic file sink), a shared host-tested `bench::StreamTimer`/`StreamTiming` (extracted from `measure_prompt`, which now reuses it), and `PRIMER_QNN_METRICS_PATH` set by the GUI startup hook (`primer-gui/src/paths.rs::set_qnn_metrics_path`, mirrors `set_genie_log_path`).
2. **`323c9f4` — fix(gui): safe-area insets.** The Android WebView drew edge-to-edge, so the status bar covered the header and the nav bar covered the composer (input unreachable, keyboard wouldn't open). Added `viewport-fit=cover` + `env(safe-area-inset-*)` padding on the app grid; the fixed-position drawer + picker carry their own insets. 3 new `responsive_layout_contract` tests.
3. **`fe99989` — fix(gui): invoke `main()` after module consts init (BOOT REGRESSION).** PR #225 added `const mobileQuery` partway down app.js but `main()` ran at the top → `setupSidebarToggle()` hit `mobileQuery` in its TDZ → because `main()` is `async`, the throw became an unhandled rejection that aborted boot *after* wiring the sidebar toggle but *before* `showPicker()`. Symptom: app booted straight to a dead chat shell ("Starting session…"), no picker, only ☰ worked. **Affected desktop too** (TDZ is viewport-independent); it shipped undetected because PR #225's static-render verification never exercised the real boot. Fix: call `main()` last. Pinned by `app_js_invokes_main_after_mobilequery_is_initialized`.
4. **`d047347` — fix(qnn): record metric on the `done` chunk, not only stream-end.** The dialogue manager's consume loop breaks on `chunk.done` and never polls again, so `MeteredStream` never saw `Poll::Ready(None)` and the sink never fired (turns ran fine but `qnn_metrics.jsonl` stayed empty). Finalize on the terminal `done` chunk too (idempotent). New `metered_stream_records_when_consumer_breaks_on_done` test drives the stream exactly like turn.rs.
5. **`4ed8f24` — docs:** README + ROADMAP record the measured numbers + the in-APK metrics approach.

**Measured (RedMagic 11 Pro, cl2048, 20 queries, 2026-06-15):** decode **25.7 tok/s mean** (min 25.0, max 26.2); TTFT **p50 ≈ 0.78 s, p95 ≈ 2.6 s, max 2.93 s** (chat turns ~2–2.9 s prefill on the 2048-ctx prompt; subsystem calls ~0.2–0.8 s); peak **~52 °C**. All three targets (≥15 tok/s, <3 s, ≤70 °C) pass.

Host green: `fmt --check`, `clippy --workspace` + `-p primer-inference --features qnn`, full workspace suite, `-p primer-inference --features qnn` (243 tests).

## What's next (concrete acceptance criteria)

### 1. ⭐ Pedagogy / answer-quality + rating tuning on the 4B NPU model (owner-in-the-loop)
The conversation is technically excellent on-device; the open question is pedagogical quality at the compressed 2K-context budget. Owner: *"Quality of answers and ratings will have to be tuned."*
- **Acceptance:** spot-check that the small-context prompt budget (8-turn window, KB top-K 3, ~110-token passage truncation) didn't dull Socratic behaviour (more questions than answers; comprehension-via-transfer); sanity-check `turn_classifications` / `turn_comprehensions` rows on a real device session (pull `explorer.db`, `sqlite3` it). **Define specific tuning targets with the owner first** — this is judgement, not a blind code change.

### 2. Latency-aware routing calibration (now unblocked — we have the TTFT number)
- **Acceptance:** local TTFT p95 ≈ 2.6 s is the calibration anchor for `--primary-ttft-budget-ms` / GUI "Primary TTFT budget (ms)". Decide *whether* hybrid routing is even wanted (local is already fast); if so, set a budget around the measured p95 and verify borderline turns route to cloud while trivial turns stay local. Owner decision.

### 3. (optional) Thermal under sustained load
- The in-APK metrics path records throughput only; peak ~52 °C is a one-shot post-run read. If a sustained-load thermal number is wanted, add a lightweight periodic `/sys/class/thermal` sampler to the APK (the standalone `bench::spawn_thermal_sampler` can't run here). Low priority — 52 °C has huge headroom under 70 °C.

### Carried / owner-or-hardware-gated
- #170 Supertonic Stages E/F; #192 / #166 human-at-mic smokes; #157 Termux ONNX-runtime validation; #98 split `sweep.rs`; #135 glib bump on Tauri 3; #201 llamacpp BOS; llama.cpp device bench (owner-gated).

### Open queue (issues)

| #   | Title | State |
| --- | --- | --- |
| 201 | llamacpp BOS handling across model families | owner-gated (real-model smoke) |
| 192 | Manual smoke: macOS-native STT + injected TTS | needs human at mic |
| 170 | Supertonic 3 voice-mode TTS | Stage D shipped; E/F + manual gates next |
| 166 | Real-audio multi-utterance Whisper smoke | needs human at mic + ggml-small.bin |
| 135 | bump glib → 0.20+ once Tauri 3 ships | waits on Tauri 3 |
| 98  | split tests/common/sweep.rs | defer until 3rd locale |

## Open decisions / risks

- **PR #227 is open, not merged.** Branch protection on `main` requires `cargo test (default features)`. Merge once CI is green (host suite passed locally). 5 commits: metrics + safe-area + boot fix + done-chunk fix + docs.
- **The PR #225 boot regression (TDZ) was on `main`** — every GUI build off `main` between PR #225 and PR #227 booted to a dead shell (desktop AND mobile). PR #227 fixes it. If anyone built the GUI from `main` in that window and it "didn't work," this is why. The contract test now guards it.
- **`qnn_metrics.jsonl` mixes chat turns and subsystem calls** (each message → 1 chat + up to 3 subsystem queries, all through the shared dialog). Decode tok/s is the same for all (~25.7, prompt-independent); TTFT differs (chat turns ~2–2.9 s, subsystem ~0.2–0.8 s). When aggregating, the high-token / high-TTFT lines are the chat turns. The file is **append-only and unbounded** — fine for a dev/eval device; if it ever ships to a child's device, add rotation/a size cap + make recording opt-in (tracked in **#228**) (currently it only writes when `PRIMER_QNN_METRICS_PATH` is set, which only the GUI mobile startup hook does).
- **Metrics are always-on when the env is set** (the GUI sets it unconditionally on mobile). That's a tiny per-turn file append; acceptable for now. If you want it opt-in, gate `set_qnn_metrics_path` behind a config flag (tracked in **#228** alongside the rotation/size-cap work).
- **The cl2048 bundle + V81 libs are git-ignored / off-repo**, staged on-device at `files/qnn-bundle` (`size: 2048`). Re-stage with `~/qnn-export-2048/stage-bundle.sh <SRC>` if needed.
- **Pedagogy on a 4B NPU model is the remaining open quality question** — technically excellent (25.7 tok/s, stable), pedagogically unverified at scale.

## Patterns to reuse, not reinvent

New this session:
- **On-device throughput on this ROM = instrument the APK, NOT `qnn_bench`.** The standalone harness can't reach the DSP from a sideloaded process. The pattern: a `MeteredStream` `TokenStream` decorator (no I/O of its own — fires a sink closure) + a shared host-tested `StreamTimer` + an env-gated append-only JSONL file read via `run-as cat`. Read `<app_data>/.primer/qnn_metrics.jsonl`.
- **A stream decorator must finalize on the terminal `done` chunk, not only on `None`.** The dialogue manager breaks on `chunk.done` without draining to `None`, so any wrap that defers work to stream-end silently no-ops. Finalize on `done` (idempotent via `Option::take`) AND keep the `None`/error arms for backends that close without a sentinel.
- **`const`-TDZ + top-of-file `main()` + `async main()` = a silent boot abort.** A `const` referenced (even transitively, via a setup fn) before its declaration line executes throws; in an `async` entry point that becomes an unhandled rejection that kills boot partway. Invoke the entry point LAST, after all module-level `const`s. Pinned by a source-order contract test.
- **On-device JS debugging when logcat is dead:** add a temporary inline `window.onerror` / `unhandledrejection` handler at the top of `<body>` that renders the error into a fixed on-screen div; the owner reads it off the screen. That's how the TDZ error was caught (`REJECT: Cannot access 'mobileQuery' before initialization`). Remove before merge.
- **Frontend "TDD" here = a Rust `cfg(test)` contract module that `include_str!`s the UI assets** (`responsive_layout_contract.rs` / `modal_dialog_contract.rs`). There's no JS test runner. New tests this session: viewport-fit, safe-area grid inset, drawer top-inset, and the boot-order (`main()` after `mobileQuery`).

Carried (prior QNN/device handoffs, still true): Android scoped storage hides `adb`-pushed `/sdcard/Android/data/<pkg>` — stage app-internal via `adb push /data/local/tmp/<f>` → `run-as <pkg> cp`; this RedMagic ROM has **dead logcat + black screencap** — read app-internal files via `run-as <pkg> cat` (relative paths like `.primer/...` work from the run-as cwd) + owner reads the screen; `/proc/<pid>/environ` shows only *exec-time* env, NOT runtime `set_var` (don't use it to check `PRIMER_*` vars); a reboot leaves `/data/user/0/<pkg>` encrypted until the owner enters the PIN; reboot maximizes CmaFree (~631 MB); QNN APK rebuild = `--no-default-features --features qnn`; cargo from `src/` with `+1.88`; commits touching `.github/workflows` need `gh auth refresh -s workflow`. Android host facts: JDK 21 = Android Studio JBR for `JAVA_HOME`; NDK at `/opt/homebrew/share/android-ndk`; throwaway build script `/tmp/build-qnn-apk.sh` wraps env + `cargo-tauri android build --apk --debug --target aarch64 -- --no-default-features --features qnn`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status        # PR #227 merged? then clean main

# === Host health check (default features = required gate) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 test --workspace
~/.cargo/bin/cargo +1.88 test -p primer-inference --features qnn qnn::      # incl. metrics + StreamTimer
~/.cargo/bin/cargo +1.88 test -p primer-gui responsive_layout_contract      # incl. the boot-order guard

# === Device: read the QNN throughput metrics (the new in-APK path) ===
ADB="$HOME/Library/Android/sdk/platform-tools/adb"
"$ADB" devices                                   # 912607710061; plug in + unlock (PIN) if empty
"$ADB" shell run-as org.theprimer.gui cat .primer/qnn_metrics.jsonl   # per-turn TTFT + decode tok/s
"$ADB" shell run-as org.theprimer.gui cat .primer/genie.log | grep -i 'context limit'   # expect NONE

# === Rebuild + reinstall the QNN APK after changes ===
bash /tmp/build-qnn-apk.sh   # wraps env + cargo-tauri android build --no-default-features --features qnn
"$ADB" install -r src/crates/primer-gui/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
# clear metrics for a fresh capture: "$ADB" shell run-as org.theprimer.gui rm -f .primer/qnn_metrics.jsonl

# === New code work: PR-first (branch protection on main) ===
git checkout main && git pull
git checkout -b <branch> main && git push -u origin <branch> && gh pr create --base main ...
```

## Reporting back

- State plainly what works and what doesn't, by acceptance criterion.
- **This session's headline:** the Primer's own `QnnBackend` decodes at **~25.7 tok/s on the Hexagon NPU** (TTFT p95 ≈ 2.6 s, peak ~52 °C) — all Phase 1.2 acceptance targets met, ~2.7× the old proxy estimate. The number was captured via a new **in-APK per-turn metrics** path (the standalone `qnn_bench` can't reach the DSP on this ROM), which required fixing a **PR #225 GUI boot regression** (const-TDZ) and a stream-finalize bug along the way. Shipped in PR #227 (5 commits). Remaining: pedagogy/answer-quality tuning on the 4B model and (optional) latency-routing calibration — both owner-in-the-loop.
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
