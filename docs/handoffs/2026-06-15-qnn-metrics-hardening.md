# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-15 — **The in-APK QNN throughput metrics file (PR #227) is now production-hardened: bounded size (1 MiB + single-backup rotation) and opt-in (OFF by default, behind a Settings → Diagnostics toggle).** This closes the two risks PR #227's own handoff flagged twice — an append-only/unbounded file and always-on recording — before the metrics path could ever ship to a child's device. Shipped as **PR #229** (issue #228). No device round-trip was needed: this is a host-only change, fully covered by Rust tests.

**Context at session start:** clean `main` at `cc1e4d5` (PR #227 merged). **End state:** all work on branch `qnn-metrics-hardening`, opened as **PR #229** (1 commit `2c463c9`; CI running — merge when green). No device interaction this session; the RedMagic APK is unchanged from PR #227.

## What we shipped this session (PR #229, branch `qnn-metrics-hardening`, commit `2c463c9`)

**`2c463c9` — feat(qnn): size-cap + opt-in gate for on-device throughput metrics (#228).** Two production-readiness hardenings to the in-APK metrics path:

1. **Bounded file size (rotation).** `primer_inference::qnn::metrics` rotates the live `qnn_metrics.jsonl` to a single `.1` backup once it reaches `primer_core::consts::qnn::METRICS_FILE_MAX_BYTES` (**1 MiB**), so the on-disk footprint is bounded at **~2× the cap**. New pure host-tested helpers `should_rotate(current_len, max_bytes)` (inclusive `>=`) and `rotated_metrics_path(path)`; `append_metric_line_capped(path, line, max_bytes)` exposes the cap for testing; `append_metric_line` is the thin wrapper using the const. Best-effort/never-panic throughout (rotation/open/write failures are `tracing::warn!`'d and swallowed, like `genie::log`).
2. **Opt-in recording.** New `DiagnosticsConfig { qnn_metrics_enabled: bool }` on `GuiConfig` (**default OFF**), threaded through `GuiConfigView`/`GuiConfigUpdate`/`into_config` + a new **Settings → Diagnostics** checkbox (`ui/index.html` + `ui/settings.js` `gather()`/`populate()` + `dom.fields`). `primer-gui/src/paths.rs::init_mobile_state` now calls `set_qnn_metrics_path` **only when the flag is on** — a child's device records nothing by default; a developer ticks the box for an eval capture. The update DTO's `diagnostics` field carries a field-level `#[serde(default)]` so older `settings.js` payloads still deserialize (OFF is the safe privacy direction — unlike the backend fields, a silent revert here can never *enable* telemetry).

New `primer-gui/src/diagnostics_toggle_contract.rs` `include_str!`s the UI assets and pins the HTML id ↔ `settings.js` wiring (no JS test runner in this crate — same frontend-TDD pattern as `modal_dialog_contract` / `responsive_layout_contract`).

**Tests added:** 4 in `qnn::metrics` (rotation boundary, suffix, rotate-on-cap, single-backup-replacement); 5 in `config` (defaults-off, disk round-trip, view pass-through, update with/without diagnostics); 2 contract tests.

Host green: `fmt --check`, `clippy --workspace --all-targets`, `clippy -p primer-inference --features qnn`, primer-core (174), primer-gui (192), primer-inference (165 default / 235 with `qnn`), full workspace suite (exit 0).

## What's next (concrete acceptance criteria)

### 1. Merge PR #229
- **Acceptance:** branch protection requires `cargo test (default features)` — merge once green (host suite passed locally). Then `main` carries the hardened metrics path.

### 2. ⭐ Pedagogy / answer-quality + rating tuning on the 4B NPU model (owner-in-the-loop) — *carried, still the top open question*
The conversation is technically excellent on-device (25.7 tok/s, PR #227); the open question is pedagogical quality at the compressed 2K-context budget. Owner: *"Quality of answers and ratings will have to be tuned."*
- **Acceptance:** spot-check that the small-context prompt budget (8-turn window, KB top-K 3, ~110-token passage truncation) didn't dull Socratic behaviour (more questions than answers; comprehension-via-transfer); sanity-check `turn_classifications` / `turn_comprehensions` rows on a real device session (pull the session DB, `sqlite3` it). **Define specific tuning targets with the owner first** — this is judgement, not a blind code change. *(With #229's opt-in toggle, enabling metrics for a capture run now means ticking Settings → Diagnostics, not editing JSON on-device.)*

### 3. Latency-aware routing calibration (carried; unblocked since PR #227 gave the TTFT number)
- **Acceptance:** local TTFT p95 ≈ 2.6 s is the calibration anchor for `--primary-ttft-budget-ms` / GUI "Primary TTFT budget (ms)". Decide *whether* hybrid routing is even wanted (local is already fast); if so, set a budget around the measured p95. Owner decision.

### 4. (optional) Thermal under sustained load
- The in-APK metrics path records throughput only; peak ~52 °C is a one-shot post-run read. A sustained-load thermal sampler in the APK is low priority — 52 °C has huge headroom under 70 °C.

### Carried / owner-or-hardware-gated
- #226 a11y focus/scroll-lock for the mobile drawer; #224 context-limit mid-sentence truncation; #223 confirm GENIE context-limit enum; #170 Supertonic Stages E/F; #201 llamacpp BOS; #192 / #166 human-at-mic smokes; #157 Termux ONNX-runtime validation; #98 split `sweep.rs`; #135 glib bump on Tauri 3; llama.cpp device bench (owner-gated).

### Open queue (issues)

| #   | Title | State |
| --- | --- | --- |
| 228 | QNN metrics rotation/size-cap + opt-in gate | **DONE — PR #229 (merge when green)** |
| 226 | a11y: focus mgmt + scroll-lock for mobile overlay drawer | open, actionable (frontend) |
| 224 | QNN context-limit graceful completion can truncate mid-sentence | enhancement, rust |
| 223 | Confirm GENIE_STATUS_CONTEXT_LIMIT_EXCEEDED (=4) vs authoritative header | docs (needs QAIRT header) |
| 201 | llamacpp BOS handling across model families | owner-gated (real-model smoke) |
| 192 | Manual smoke: macOS-native STT + injected TTS | needs human at mic |
| 170 | Supertonic 3 voice-mode TTS | Stage D shipped; E/F + manual gates next |
| 166 | Real-audio multi-utterance Whisper smoke | needs human at mic + ggml-small.bin |
| 135 | bump glib → 0.20+ once Tauri 3 ships | waits on Tauri 3 |
| 98  | split tests/common/sweep.rs | defer until 3rd locale |

## Open decisions / risks

- **PR #229 is open, not merged.** Merge once `cargo test (default features)` is green (host suite passed locally). 1 commit.
- **The opt-in flip changes the dev capture workflow.** To capture metrics on-device now, enable Settings → Diagnostics → "Record QNN per-turn throughput metrics" before the run (it persists to `gui-config.json`). The `PRIMER_QNN_METRICS_PATH` env override still wins if set (the startup hook no-ops when the env is already present), so an env-driven capture path remains available. A fresh install records nothing.
- **Rotation is single-backup, append-after-check.** The live file can exceed the cap by at most one record (the one written after the size check); the `.1` backup is replaced, not chained, so the footprint never exceeds ~2× the cap. This is intentional — a child/eval device never needs deep history, only a bounded recent window.
- **Default cap is 1 MiB (~8.7k turns; ~17k retained across the backup).** Lives in `primer_core::consts::qnn::METRICS_FILE_MAX_BYTES` — tune there, not at call sites, if a longer capture window is wanted.
- **Pedagogy on a 4B NPU model remains the headline open quality question** — technically excellent, pedagogically unverified at scale. (Unchanged from PR #227's handoff.)
- **The cl2048 bundle + V81 libs are git-ignored / off-repo**, staged on-device at `files/qnn-bundle`. Re-stage with `~/qnn-export-2048/stage-bundle.sh <SRC>` if needed. (No device work this session, so the device state is whatever PR #227 left.)

## Patterns to reuse, not reinvent

New this session:
- **A bounded best-effort log/metrics sink = pure `should_rotate(len, cap)` + pure `rotated_path(path)` + a best-effort `rotate_if_oversize` that stats-then-renames, all driven by a `*_capped(path, line, max_bytes)` core the production wrapper calls with the const.** Keeps the rotation policy a host-tested pure decision and the I/O a thin never-panic shell. Reuse this shape for any future on-device append-only file (e.g. if `genie.log` ever needs a cap).
- **A new opt-in GUI toggle = a small `#[serde(default)]` `*Config` sub-struct on `GuiConfig` + its View/Update mirror + `into_config` + `settings.js` gather/populate + modal HTML + a `*_contract.rs` test that `include_str!`s the assets to pin the id ↔ JS wiring.** For a brand-new section default OFF, prefer field-level `#[serde(default)]` on the *Update* DTO (forgiving of older payloads) rather than the strict no-default discipline used for `BackendConfigUpdate` — that discipline exists to catch a *dangerous* silent revert, and OFF-by-default telemetry is the safe direction.
- **Frontend "TDD" here = a Rust `cfg(test)` contract module that `include_str!`s the UI assets** (`diagnostics_toggle_contract.rs` joins `modal_dialog_contract.rs` / `responsive_layout_contract.rs`). There's no JS test runner; assert load-bearing ids/strings appear in both `index.html` and `settings.js` so a rename in one place fails `cargo test`.

Carried (prior QNN/device handoffs, still true): the standalone `qnn_bench` example **can't reach the DSP** from a sideloaded/Termux process on this ROM — on-device throughput is instrumented **inside the APK** (read `<app_data>/.primer/qnn_metrics.jsonl` via `run-as cat`). A stream decorator must finalize on the terminal `done` chunk, not only on `None` (the dialogue manager breaks on `chunk.done`). Android scoped storage hides `adb`-pushed `/sdcard/Android/data/<pkg>` — stage app-internal via `adb push /data/local/tmp/<f>` → `run-as <pkg> cp`; this RedMagic ROM has **dead logcat + black screencap** — read app-internal files via `run-as <pkg> cat` + owner reads the screen; a reboot leaves `/data/user/0/<pkg>` encrypted until the owner enters the PIN; reboot maximizes CmaFree (~631 MB); QNN APK rebuild = `--no-default-features --features qnn`; cargo from `src/` with `+1.88`; commits touching `.github/workflows` need `gh auth refresh -s workflow`. Android host facts: JDK 21 = Android Studio JBR for `JAVA_HOME`; NDK at `/opt/homebrew/share/android-ndk`; throwaway build script `/tmp/build-qnn-apk.sh` wraps env + `cargo-tauri android build --apk --debug --target aarch64 -- --no-default-features --features qnn`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git status        # PR #229 merged? then clean main

# === Host health check (default features = required gate) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 test --workspace
~/.cargo/bin/cargo +1.88 test -p primer-inference --features qnn qnn::    # incl. metrics rotation
~/.cargo/bin/cargo +1.88 test -p primer-gui diagnostics                   # config + contract tests

# === Merge PR #229 (when CI green) ===
gh pr checks 229
gh pr merge 229 --squash --delete-branch    # or merge in the GitHub UI

# === Device: read the QNN throughput metrics (now opt-in) ===
# Enable first: Settings → Diagnostics → "Record QNN per-turn throughput metrics"
# (persists to gui-config.json), OR set PRIMER_QNN_METRICS_PATH before launch.
ADB="$HOME/Library/Android/sdk/platform-tools/adb"
"$ADB" shell run-as org.theprimer.gui cat .primer/qnn_metrics.jsonl       # per-turn TTFT + decode tok/s
"$ADB" shell run-as org.theprimer.gui ls -la .primer/qnn_metrics.jsonl*   # live + .1 backup if rotated

# === Rebuild + reinstall the QNN APK after changes ===
bash /tmp/build-qnn-apk.sh
"$ADB" install -r src/crates/primer-gui/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk

# === New code work: PR-first (branch protection on main) ===
git checkout main && git pull
git checkout -b <branch> main && git push -u origin <branch> && gh pr create --base main ...
```

## Reporting back

- State plainly what works and what doesn't, by acceptance criterion.
- **This session's headline:** the in-APK QNN metrics file is now **production-hardened** — bounded at 1 MiB with single-backup rotation, and **opt-in (OFF by default)** behind a Settings → Diagnostics toggle, so a child's device records no telemetry unless a developer enables it (issue #228). Host-only change, fully test-covered (11 new tests), shipped as PR #229 (commit `2c463c9`). No device round-trip was needed. Remaining top item is unchanged: pedagogy/answer-quality tuning on the 4B model (owner-in-the-loop).
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
