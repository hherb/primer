# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-11 — **Genie log-to-file diagnostic shipped; the `GenieDialog_create` -1 cause is now KNOWN.** Owner-paired session, RedMagic 11 Pro connected. We wired Genie's logging callback to a file (logcat is dead on this ROM), started a QNN session on-device, and read behind the generic -1. **The blocker is a missing per-arch HTP library, not DSP signing.** PR #217 carries the diagnostic + the finding.

**Context at session start:** PR #216 (sub-project 3) was already squash-merged to `origin/main` as `d332ae7`. **End state:** branch `qnn-genie-log-to-file` @ `6eb9edb`, **PR #217 open** (CI re-running on the doc commits; was green on the code commit). Working tree clean on the branch. Branches = `main`, `qnn-genie-log-to-file`, `android-path-resolution` (merged, deletable), `backup/pre-rebase-stageB`.

## ⚠️ First action: check + merge PR #217

```bash
cd /Users/hherb/src/primer && git status
gh pr checks 217                 # required gate: cargo test (default features)
gh pr view 217
```
If green + owner approves: `gh pr merge 217 --squash --delete-branch`. No proprietary blobs, no `.github/workflows` changes. No new deps.

## The headline finding (this overturns a prior assumption)

The Genie log file (`<app_data>/.primer/genie.log`, read via `run-as cat`) captured the DSP-init trace behind `GenieDialog_create` returning **-1** (`GENIE_STATUS_ERROR_GENERAL`):

```
Failed in loading stub: dlopen failed: library "libQnnHtpV81Stub.so" not found
loadRemoteSymbols failed with err 4000
Failed to load skel, error: 4000
Transport layer setup failed: 14001
Failed to parse platform info: 14001
qnn-api initialization failed!
```

- Our bundle is **QAIRT 2.45** (`libQnnHtp.so` = `v2.45.41.260507231357`) but was staged with only the **V79** per-arch HTP libs.
- The 2.45 runtime correctly identifies the SM8850 (8 Elite Gen 5) as **V81** and `dlopen`s the host-side `libQnnHtpV81Stub.so` — which was never staged.
- **The HTP arch IS the blocker.** This overturns the sub-project-3 "v79 runs on this V81 part" note (true only for an older QNN that didn't recognise V81; our 2.45 does).
- **`unsigned PD` is the default in the trace**, so signing / unsigned-PD denial was *NOT* the cause (the prior leading hypothesis — wrong).
- The full saved log is at `/private/tmp/primer-qnn/genie-2026-06-11.log` (504 lines; not committed — proprietary `QnnDsp` strings).

## What we shipped this session (PR #217, branch `qnn-genie-log-to-file`)

Three commits:
- **`bf88531`** `feat(qnn): route Genie logging callback to a file …` — the diagnostic.
- **`051da4b`** `docs(qnn): on-device finding — -1 is the missing V81 host stub …` — runbook.
- **`6eb9edb`** `docs: README + ROADMAP — sub-project 4 finding …`.

### The diagnostic (assigned task — DONE, host-verified, worked on-device)
- **`primer-qnn-sys`** — 3 new QAIRT 2.45 logging symbols (`GenieLog_create`, `GenieLog_free`, `GenieDialogConfig_bindLogger`) + `GenieLog_Callback_t` typedef (with its `va_list` arg) + `GenieLog_Level_t` consts, matched to the staged 2.45 headers (`/private/tmp/primer-qnn/genie-headers/`). Resolved **best-effort** (`Option` fields) so a libGenie.so lacking them never regresses the working path.
- **`primer-inference/src/qnn/genie/log.rs`** (new) — process-global `Mutex<Option<File>>` sink (the Genie log callback has **no userData param**, so a global is the only routing option), the C-ABI callback, pure host-tested helpers (`level_label`, `format_log_line`). The `va_list`→`vsnprintf` bridge is AArch64-gated (a `va_list` decays to a pointer as a callback arg, forwarded verbatim to `vsnprintf`); host builds record the bare `fmt` so the module compiles + pure helpers test.
- **`real.rs`** — `open_dialog` creates the logger → binds to the config before `GenieDialog_create` → owns the log handle in `RealGenieDialog` (freed last in `Drop`). Extracted the pure `absolutize_genie_config` helpers into new `qnn/genie/config.rs` to keep `real.rs` ≈ the 500-line guide.
- **`primer-gui`** — `init_mobile_state` sets `PRIMER_GENIE_LOG_PATH` → `<app_data>/.primer/genie.log` (env-driven opt-in, like `ADSP_LIBRARY_PATH`); absent ⇒ logging off, desktop byte-identical.
- Host: fmt + clippy clean; `primer-qnn-sys` 6, `primer-inference --features qnn` 219, `primer-gui` 168 green; `aarch64-linux-android` cross-compile clean (the `vsnprintf` bridge compiles).

### On-device (device-paired)
- APK rebuilt (`--no-default-features --features qnn`), `adb install -r`, app launched, owner started a QNN session. `genie.log` was written and read — see the finding above. The bundle (13 files incl. 4 model parts ~2.9 GB) + `kind=qnn` config are confirmed staged in app-internal storage (`/data/user/0/org.theprimer.gui/files/qnn-bundle`).

## What's next — by priority

### 1. ⭐ Sub-project 5 — stage the V81 HTP libs → first on-device NPU token (THE Phase 1.2 finish line)
Stage the **V81** per-arch HTP libs from the *same QAIRT 2.45 SDK* the v79 libs came from, into `src/crates/primer-gui/gen/android/app/src/main/jniLibs/arm64-v8a/` alongside the v79 set:
- **host-side:** `libQnnHtpV81.so`, `libQnnHtpV81Stub.so` (from `lib/aarch64-android/`).
- **DSP-side:** `libQnnHtpV81Skel.so` (from `lib/hexagon-v81/unsigned/`) — also reachable via the `/vendor/dsp/cdsp` `ADSP_LIBRARY_PATH` fallback, but bundling our own keeps it self-contained. (Optionally `libQnnHtpV81CalculatorStub.so` to mirror the v79 set.)
- **⚠️ Source:** these come from the **QAIRT 2.45 SDK** (Qualcomm developer-portal login — the SDK is NOT on this dev box; only the 2.45 headers + the v79 libs were ever staged). The device firmware *has* V81 libs in `/vendor/lib64/` + `/vendor/dsp/cdsp/`, but **non-root `adb pull` of `/vendor/lib64` is permission-denied**, and a firmware lib may be a different QAIRT version than our 2.45 set (version-skew risk). Owner action needed to obtain the SDK V81 libs.
- Update the jniLibs `README.md` manifest (sha256) for the new libs.
- Rebuild APK → `adb install -r` → owner starts a session → `run-as cat .primer/genie.log`. If the V81 stub now loads, the next error (if any) is in the log; iterate.
- **Acceptance:** ≥1 coherent token from `QnnBackend` on the Hexagon NPU, confirmed (via `genie.log` since logcat is dead).

### Carried, owner/hardware-gated (unchanged)
- Real `qnn_bench` numbers (gated on the first on-device token).
- Latency-aware routing calibration (`--primary-ttft-budget-ms`) — gated on bench numbers.
- llama.cpp bench on real hardware — owner-gated.
- Full `tauri android build` in CI — deferred (needs JDK+SDK+NDK+Gradle on the runner).
- #170 Supertonic Stages E/F; #192 / #166 human-at-mic smokes; #157 Termux ONNX-runtime validation; #98 split `sweep.rs`; #135 glib bump on Tauri 3; #201 llamacpp BOS.

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

- **V81 libs must come from the QAIRT 2.45 SDK** (portal-gated; not on this box). The firmware V81 libs are version-skew risk and permission-blocked from `adb pull`. This is the single gating dependency for the next token.
- **Version match matters:** stage V81 libs from the *same 2.45 SDK* as the bundled `libGenie.so`/`libQnnHtp.so` (`v2.45.41.260507231357`), not a firmware copy of unknown version.
- **The Genie log file is the loop's eyes** — keep using `run-as cat .primer/genie.log` after every retry (logcat is dead, screencap is black on this ROM). The log auto-appends per session (a `=== genie log session start ===` marker separates runs).
- **The QAIRT `.so`s are NOT in the repo** (git-ignored). A fresh clone has an empty `jniLibs/arm64-v8a/` (just the README). The v79 set sits staged in the working tree on this machine (ignored).
- **Branch protection ACTIVE on `main`** (required `cargo test (default features)`, strict). Docs-only PRs are CI-path-ignored (#168), but PR #217 has code so CI runs.
- `backup/pre-rebase-stageB` still KEPT (intentional snapshot); `android-path-resolution` is merged (PR #216) and deletable. Carried: `--languages` (#21) seeds a fresh learner only; Supertonic OpenRAIL-M licence read before any Stage E/F default flip.

## Patterns to reuse, not reinvent

New from this session:
- **Read behind a generic Genie status with the log-to-file path.** Any future Genie -1/-N is now legible: the GUI sets `PRIMER_GENIE_LOG_PATH` automatically on Android; `adb shell run-as org.theprimer.gui cat .primer/genie.log` shows the `QnnDsp <E>` lines. The mechanism is `primer-qnn-sys` (3 optional logging symbols, best-effort) + `primer-inference/src/qnn/genie/log.rs` (global file sink + AArch64 `va_list`→`vsnprintf` bridge, host-gated to record bare `fmt`).
- **QAIRT 2.45 is per-SoC-arch-strict.** `libQnnHtp.so` auto-detects the SoC and `dlopen`s `libQnnHtpV<N>Stub.so` for that exact arch (SM8850 → V81). Staging the wrong arch's HTP libs fails at `GenieDialog_create` with -1, surfaced as `dlopen failed: library "libQnnHtpV<N>Stub.so" not found`. Always stage the arch matching the target SoC, from the matching SDK version.
- **Optional FFI symbols degrade gracefully:** resolve best-effort into `Option<fn>` (`resolve_optional_symbol` in `primer-qnn-sys`) so a new symbol that an older `.so` lacks never regresses the existing path — mirror this for any future additive Genie API.

Carried forward (prior handoffs): `home` is the single base-dir knob in `primer-gui` (per-platform resolution fixes config+DB+cache; `primer-engine` never reads `$HOME`); mobile Tauri setup defers `app.path()` work into `.setup()`; Android scoped storage hides `adb`-written `/sdcard/Android/data/<pkg>` from the app — stage bulk assets app-internal via `shell cat <src> | run-as <pkg> sh -c 'cat > files/...'`; **this RedMagic ROM has dead logcat AND black screencap** — observe via `run-as cat` of app-internal files + the owner reading the screen; APK rebuild is `--no-default-features --features qnn`; a new inference behavior is a decorator over `Arc<dyn InferenceBackend>` built by `build_main_backend`; run cargo from `src/` with `+1.88`; docs-only PRs are CI-path-ignored (#168). Android host facts: JDK 21 = Android Studio's JBR for `JAVA_HOME`; the `$ANDROID_HOME/ndk/29.0.14206865 → /opt/homebrew/share/android-ndk` symlink Tauri 2.11 needs; commits touching `.github/workflows` need `gh auth refresh -s workflow -h github.com`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status                                    # clean
gh pr checks 217 ; gh pr view 217             # merge if green + approved: gh pr merge 217 --squash --delete-branch

# === Health check ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 clippy -p primer-qnn-sys -p primer-inference --features primer-inference/qnn -p primer-gui -p primer-engine --all-targets -- -D warnings
~/.cargo/bin/cargo +1.88 test --workspace          # default features, the required gate

# === Sub-project 5: stage V81 HTP libs, rebuild, retry ===
# 1) Obtain libQnnHtpV81.so + libQnnHtpV81Stub.so (host) + libQnnHtpV81Skel.so (DSP)
#    from the QAIRT 2.45 SDK (portal). Copy into:
JNI=src/crates/primer-gui/gen/android/app/src/main/jniLibs/arm64-v8a
#    (alongside the existing v79 set; update the jniLibs README manifest + sha256)

# 2) Rebuild + reinstall the QNN APK (device connected)
export ANDROID_HOME="$HOME/Library/Android/sdk"
export NDK_HOME=/opt/homebrew/share/android-ndk
export JAVA_HOME="/Applications/Android Studio.app/Contents/jbr/Contents/Home"
export PATH="$NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin:$JAVA_HOME/bin:$HOME/.cargo/bin:$PATH"
cd src/crates/primer-gui
~/.cargo/bin/cargo-tauri android build --apk --debug --target aarch64 -- --no-default-features --features qnn
ADB="$ANDROID_HOME/platform-tools/adb"
"$ADB" install -r gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
# bundle + qnn config already staged on device (internal). Owner starts a new session in the app.

# 3) Read behind any remaining -1 (logcat is DEAD on this ROM)
"$ADB" shell run-as org.theprimer.gui cat .primer/genie.log
# device bundle internal copy at /data/user/0/org.theprimer.gui/files/qnn-bundle (13 files, ~2.9 GB)
# adb serial 912607710061

# === New work: PR-first (branch protection is on) ===
git checkout -b <branch> main
git push -u origin <branch> && gh pr create --base main ...
# NB: commits touching .github/workflows need `gh auth refresh -s workflow -h github.com` first.
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- Flag any bugs you exposed in existing behaviour separately from the assigned task.
- **This session's headline:** the Genie log-to-file diagnostic shipped (PR #217) and **worked on-device** — it read behind `GenieDialog_create`'s generic -1 and found the concrete blocker: the QAIRT **2.45** runtime detects the SM8850 as **V81** and needs `libQnnHtpV81Stub.so`, but only **V79** HTP libs were staged. This **overturns** two prior hypotheses (signing/unsigned-PD is NOT the cause; "v79 runs on this V81 part" is wrong for our 2.45 bundle). The first on-device token is now one well-defined step away: stage the V81 HTP libs from the QAIRT 2.45 SDK and retry.
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
