# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-11 — **Android path-resolution shipped + QNN-on-device bring-up to the NPU boundary.** Owner-paired session, RedMagic 11 Pro connected. The QNN APK now **installs, boots, and renders** on-device, and a QNN session drives the full stack onto the Hexagon NPU up to `GenieDialog_create`, which returns **status -1** (the DSP model-load/init boundary). **PR #216** carries the path-resolution fix + the on-device bring-up infrastructure.

**Context at session start:** PR #215 (sub-project 2) was already squash-merged to `origin/main` as `60b6d00`. **End state:** branch `android-path-resolution` @ `0d7fb5c`, **PR #216 open** (CI running). Working tree clean. Branches = `main`, `android-path-resolution`, `backup/pre-rebase-stageB`.

## ⚠️ First action: check PR #216

```bash
cd /Users/hherb/src/primer && git status
gh pr checks 216                 # required gate: cargo test (default features)
gh pr view 216
```
If green + owner approves: `gh pr merge 216 --squash --delete-branch`. No proprietary blobs, no `.github/workflows` changes (no `workflow` scope needed). One new android-only dep (`paranoid-android`, MIT) — see Open decisions.

## What we shipped this session (PR #216, branch `android-path-resolution`)

Four commits:
- **`2a65618`** `feat(android): per-platform path resolution + QNN basename dlopen` — the core fix.
- **`4fa00d1`** `feat(gui/android): route tracing to logcat via paranoid-android`.
- **`f849c18`** `feat(gui/android): set ADSP_LIBRARY_PATH so the DSP finds the bundled QNN skel`.
- **`0d7fb5c`** `docs: Android on-device QNN bring-up status + V79/-1 findings`.

### Path resolution (the assigned task — DONE, host-verified, works on-device)
`home` is the single base-dir knob (config, session DB, voice cache all derive from it via params; `primer-engine` never reads `$HOME`). So the fix is per-platform resolution of that one value:
- **`primer-gui`** `run()` cfg-splits: desktop manages `AppState` before the builder (`$HOME`, byte-identical); **mobile defers into a `.setup()` hook** (`paths::init_mobile_state`) where `app.path().app_data_dir()` exists.
- **Seed corpus**: `resource_dir()` is `asset://localhost/` (not `std::fs`-readable) → `paths::mobile_seed_dir` resolves `<app_data>/seed`; absent ⇒ graceful empty KB.
- **`primer-qnn-sys::genie_load_target`**: Android loads `libGenie.so` by **basename** (linker finds it + deps in `nativeLibraryDir`); `primer-engine::resolve_qairt_lib_dir` returns empty on Android.
- **Logcat layer** (`paranoid-android`) + **`ADSP_LIBRARY_PATH`** wiring (from `/proc/self/maps`).
- All new logic pure + host-tested; `mobile`-cfg path cross-compiles (`--no-default-features --features qnn`).

### On-device results (RedMagic 11 Pro / SM8850 / "canoe")
- ✅ APK **installs, boots, renders** (owner-confirmed) — path resolution works.
- ✅ QNN session reaches `GenieDialog_create`: basename `dlopen libGenie.so` → Genie config parse + absolutize → NPU model-load call.
- ⚠️ `GenieDialog_create` returns **-1**. Persists after the `ADSP_LIBRARY_PATH` fix → cause is deeper than skel discovery.

### Key device facts learned this session
- **Bundle must be in app INTERNAL storage** `/data/user/0/org.theprimer.gui/files/qnn-bundle` (all 12 files, ~2.9 GB, staged this session). Android scoped storage **hides `adb`-written `/sdcard/Android/data/<pkg>` files from the app** (the first -1 was "bundle does not exist" on the `/sdcard` path). Stage via on-device pipe: `adb shell "cat /data/local/tmp/primer-qnn/bundle/<f> | run-as org.theprimer.gui sh -c 'cat > files/qnn-bundle/<f>'"` (shell reads `/data/local/tmp`, run-as writes app-internal). The source bundle is still at `/data/local/tmp/primer-qnn/bundle/`.
- **`logcat` is DEAD on this RedMagic ROM** (`adb logcat -d` = 0 lines; runbook correction #7). And **`adb screencap` returns black** (hardware-composited webview). So on-device observation = owner reads the physical screen + `run-as cat` of files.
- **v79 runs on this V81 part** (step 1.2.0 chatapp = ~9.4 tok/s with v79). The device firmware ships only `libQnnHtpV81Skel.so`, but the bundled v79 skel is what Genie uses. **HTP arch is NOT the -1 blocker.**
- App id `org.theprimer.gui`; `app_data_dir()` = `/data/user/0/org.theprimer.gui`; config at `<app_data>/.primer/gui-config.json` (seeded this session: `backend.kind=qnn`, internal `qnn_bundle_dir`, `embedder.kind=none`).
- adb at `~/Library/Android/sdk/platform-tools/adb`; serial `912607710061`.

## What's next — by priority

### 1. ⭐ Sub-project 4 — first on-device NPU token (THE Phase 1.2 finish line)
`GenieDialog_create` returns -1 after the whole software stack succeeds. To get behind the generic -1 (logcat is dead):
- **Wire a Genie log callback to a FILE.** QAIRT 2.45 exposes a `GenieLog_*` API / log-handler; route it to `<app_data>/.primer/genie.log` (read via `run-as cat`). This is the unblocker — it surfaces the FastRPC/HTP error code behind -1 (e.g. unsigned-PD denial `14001`, skel-not-found, signature failure). New `primer-qnn-sys` FFI (the existing 5 symbols don't include logging) + plumbing.
- **Then act on what the log says.** Leading hypotheses for -1: (a) DSP **unsigned-PD** not permitted for this app (signed PD or a device fastrpc property may be needed); (b) a genuine skel-load failure despite `ADSP_LIBRARY_PATH` (verify the env actually took — the `/proc/self/maps` anchor on `libprimer_gui.so` is unverified at runtime); (c) a native **V81 export** is needed after all (throughput-only per step 1.2.0, but worth ruling in/out once the log is readable).
- **Acceptance:** ≥1 coherent token from `QnnBackend` on the Hexagon NPU, confirmed (via the new Genie log file, since logcat is dead).
- Quick re-test loop (bundle + config already staged on device): rebuild APK (cmd below) → `adb install -r` → owner starts a new session → `run-as cat .primer/genie.log`.

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

- **New dependency `paranoid-android = "0.2"` (MIT), android-target-only** (`[target.'cfg(target_os = "android")'.dependencies]` in `primer-gui`). Routes `tracing` → logcat. Desktop builds never pull it. Owner may want to confirm the dep is acceptable. (Note: on the RedMagic ROM logcat is dead, so this is mainly useful on other devices; the Genie-log-to-file path in sub-project 4 is what's actually needed here.)
- **`GenieDialog_create` -1 is the genuine DSP boundary** and may be a Qualcomm-policy issue (unsigned PD) not fixable purely in code. The Genie-log-to-file step will tell.
- **`ADSP_LIBRARY_PATH` set is unverified at runtime** — the `/proc/self/maps` anchor (`libprimer_gui.so`) is host-tested but I couldn't read the running app's env/maps (run-as can't read another pid's `/proc`; logcat dead). If the Genie log shows skel-not-found, double-check the anchor matches the actual loaded cdylib name.
- **The QAIRT `.so`s are NOT in the repo** (git-ignored). A fresh clone has an empty `jniLibs/arm64-v8a/` (just the README). Stage the 9 v79 libs first (README documents `adb pull` + sha256). They sit staged in the working tree on this machine (ignored).
- **Branch protection ACTIVE on `main`** (required `cargo test (default features)`, strict). PR #216 has code → CI runs. Docs-only PRs are CI-path-ignored (#168).
- `backup/pre-rebase-stageB` still KEPT (intentional snapshot). Carried: `--languages` (#21) seeds a fresh learner only; Supertonic OpenRAIL-M licence read before any Stage E/F default flip.

## Patterns to reuse, not reinvent

New from this session:
- **`home` is the single base-dir knob.** Anything path-related in `primer-gui` derives from `AppState.home`; per-platform resolution of that one value (desktop `$HOME` / mobile `app_data_dir()`) fixes config + session DB + cache at once. `primer-engine` path fns already take `home` as a param — never reintroduce a `$HOME` read there.
- **Mobile Tauri setup ordering:** `app.path()` needs the constructed `App`, so mobile defers config-load + `manage` + env-setup into the `.setup()` hook; desktop keeps the pre-builder path. cfg-split with `#[cfg(mobile)]` / `#[cfg(not(mobile))]` keeps desktop byte-identical.
- **Android storage reality:** the app **cannot** read `adb`-written `/sdcard/Android/data/<pkg>` files (scoped storage). Stage bulk assets into app-internal `/data/user/0/<pkg>/files` via the on-device pipe `shell cat <world-readable src> | run-as <pkg> sh -c 'cat > files/...'` (relative path from run-as cwd = data dir; absolute paths + complex `sh -c` quoting through `adb shell` get mangled — keep it simple, one statement, stdin redirect).
- **This RedMagic ROM has dead logcat AND black screencap.** On-device observation = owner reads the physical screen + `run-as cat` of app-internal files. Plan diagnostics around file output, not logcat.
- **APK rebuild is `--no-default-features --features qnn`** (additive to defaults; `--no-default-features` keeps it BM25-only per #157). Env: `ANDROID_HOME`, `NDK_HOME=/opt/homebrew/share/android-ndk`, `JAVA_HOME=Android Studio JBR`, NDK clang on PATH for cross-compile (`CC_aarch64_linux_android` etc.).

Carried forward (prior handoffs): a new inference-routing/resilience behavior is a decorator over `Arc<dyn InferenceBackend>` built by `build_main_backend`, `name()` → primary, fall over ONLY pre-stream; routing POLICY pure in `primer-core`, MECHANISM in `primer-inference`; over-500-lines-only-because-of-tests → `foo/{mod,tests}.rs`; mirroring a CLI feature into the GUI is a 3-struct (`Config`/`View`/`Update`) + wiring + frontend pattern (the `Update` DTO has no `#[serde(default)]`); run cargo from `src/` with `+1.88`; docs-only PRs are CI-path-ignored (#168). Android host facts: JDK 21 = Android Studio's JBR for `JAVA_HOME`; the `$ANDROID_HOME/ndk/29.0.14206865 → /opt/homebrew/share/android-ndk` symlink Tauri 2.11 needs. Commits touching `.github/workflows` need `gh auth refresh -s workflow -h github.com`.

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer
git status                                    # clean
gh pr checks 216 ; gh pr view 216             # merge if green + approved: gh pr merge 216 --squash --delete-branch

# === Health check ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 clippy -p primer-gui -p primer-engine -p primer-qnn-sys --all-targets -- -D warnings
~/.cargo/bin/cargo +1.88 test --workspace          # default features, the required gate

# === Rebuild + reinstall the QNN APK (device connected) ===
export ANDROID_HOME="$HOME/Library/Android/sdk"
export NDK_HOME=/opt/homebrew/share/android-ndk
export JAVA_HOME="/Applications/Android Studio.app/Contents/jbr/Contents/Home"
export PATH="$NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin:$JAVA_HOME/bin:$HOME/.cargo/bin:$PATH"
# stage v79 QAIRT libs if jniLibs is empty (git-ignored): adb pull /data/local/tmp/primer-qnn/qairt/. <jniLibs/arm64-v8a>
cd src/crates/primer-gui
~/.cargo/bin/cargo-tauri android build --apk --debug --target aarch64 -- --no-default-features --features qnn
ADB="$ANDROID_HOME/platform-tools/adb"
"$ADB" install -r gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
# bundle + qnn config already staged on device (internal). Owner starts a new session in the app.

# === Sub-project 4 next: read behind the -1 (logcat is DEAD on this ROM) ===
# After wiring a Genie log callback to a file, read it:
"$ADB" shell run-as org.theprimer.gui cat .primer/genie.log
# device bundle still at /data/local/tmp/primer-qnn/bundle/ ; internal copy at
# /data/user/0/org.theprimer.gui/files/qnn-bundle (12 files, ~2.9 GB).

# === New work: PR-first (branch protection is on) ===
git checkout -b <branch> main
git push -u origin <branch> && gh pr create --base main ...
# NB: commits touching .github/workflows need `gh auth refresh -s workflow -h github.com` first.
```

## Reporting back

When you finish or hit a blocker:
- State plainly what you got working and what you didn't, by acceptance criterion.
- Flag any bugs you exposed in existing behaviour separately from the assigned task.
- **This session's headline:** the Android path-resolution fix shipped (PR #216) and the QNN APK now **boots, renders, and drives the stack onto the NPU** on the RedMagic — reaching `GenieDialog_create`, which returns **-1** (the DSP init boundary). The remaining blocker is reading behind that -1: **logcat is dead on this ROM**, so sub-project 4 must wire a **Genie log callback to a file** to see the FastRPC/HTP error and then clear it (likely unsigned-PD or a deeper DSP-grant issue). The HTP arch is NOT the blocker (v79 runs on this V81 part). Bundle staging requires app-internal storage (scoped storage).
- The GUI is a full app — when a future brief calls it a scaffold, it's wrong; trust the code.
