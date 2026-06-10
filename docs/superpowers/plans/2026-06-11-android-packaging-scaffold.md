# Android Packaging Scaffold Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Produce a host-side debug APK of the existing `primer-gui` for `aarch64-linux-android` via Tauri mobile, scaffolding the path to the first on-device NPU token without touching QNN or model assets yet.

**Architecture:** Full Tauri mobile (`cargo tauri android init`). Add a `mobile`-gated entry point to `primer-gui/src/lib.rs` that drives the same Tauri builder as the desktop `run()`. Build the Android target with `--no-default-features` (BM25-only, mirroring the CLI's #157 Android posture). Commit the generated `gen/android` Gradle project with a `.gitignore` for build outputs. Verification is build-only — no device this session.

**Tech Stack:** Tauri 2.11 (`cargo-tauri` 2.11.1), Android NDK r29, Android SDK, JDK 21, Rust 1.88, `aarch64-linux-android` target.

**Spec:** `docs/superpowers/specs/2026-06-11-android-packaging-scaffold-design.md`

**Note on TDD:** Scaffold/codegen tasks (Tasks 2–3) cannot be guarded by unit tests — you cannot write a failing test for "`init` generated `gen/android`". Their verification gate is **build success** plus the desktop-regression sweep. Task 1 (the entry point) and Task 5 (final sweep) carry the real regression discipline.

**Working directory:** all `cargo` commands run from `/Users/hherb/src/primer/src`; all `cargo-tauri` commands run from `/Users/hherb/src/primer/src/crates/primer-gui`. The repo root is `/Users/hherb/src/primer`. Branch `android-packaging-scaffold` is already checked out with the spec committed (`fa87296`).

**Required env for every Android build command:**
```bash
export ANDROID_HOME="$HOME/Library/Android/sdk"
export NDK_HOME=/opt/homebrew/share/android-ndk
export JAVA_HOME="$(/usr/libexec/java_home -v 21)"
```

---

### Task 1: Add the `mobile` entry point to `lib.rs`

**Files:**
- Modify: `src/crates/primer-gui/src/lib.rs` (append after `run()`, around line 79)

- [ ] **Step 1: Capture the desktop-regression baseline**

Run (from `src/`):
```bash
~/.cargo/bin/cargo +1.88 build -p primer-gui 2>&1 | tail -3
~/.cargo/bin/cargo +1.88 test -p primer-gui 2>&1 | grep "test result:"
```
Expected: build `Finished`; every test line `0 failed`. This is the baseline the entry point must not disturb.

- [ ] **Step 2: Add the mobile entry point**

Append to `src/crates/primer-gui/src/lib.rs` immediately after the closing brace of `run()` (line 79):

```rust
/// Tauri mobile (Android/iOS) entry point.
///
/// The `tauri::mobile_entry_point` macro generates the `extern "C"`
/// symbol the generated Android `MainActivity` (and a future iOS host)
/// calls via JNI/FFI. It drives the same [`run`] builder the desktop
/// `main.rs` shim uses, so mobile and desktop share one app-construction
/// path. A startup error is logged rather than propagated — there is no
/// caller to return a `Result` to on the FFI boundary.
///
/// Compiled only under the `mobile` cfg (set by `tauri-build` for Android
/// and iOS targets), so the desktop build is byte-identical to before.
///
/// Known gap (tracked for sub-project 2+): `run()` resolves `~/.primer/`
/// via `$HOME` and the seed corpus via bundled `resources/`, which differ
/// on Android (app-specific dirs via Tauri's path API). This entry point
/// only needs to *compile and link* for the scaffold; correct on-device
/// path resolution is deferred.
#[cfg(mobile)]
#[tauri::mobile_entry_point]
fn mobile_entry_point() {
    if let Err(e) = run() {
        eprintln!("primer-gui (mobile) exited with error: {e}");
    }
}
```

- [ ] **Step 3: Verify the desktop build is unaffected**

The `#[cfg(mobile)]` gate means this code is configured out on desktop. Confirm nothing regressed (from `src/`):
```bash
~/.cargo/bin/cargo +1.88 build -p primer-gui 2>&1 | tail -3
~/.cargo/bin/cargo +1.88 fmt -p primer-gui -- --check && echo FMT_OK
~/.cargo/bin/cargo +1.88 clippy -p primer-gui --all-targets -- -D warnings 2>&1 | tail -3
```
Expected: build `Finished`; `FMT_OK`; clippy `Finished` with no warnings. (The new fn is cfg'd out on desktop, so it is not yet compile-checked — Task 3's Android build is its first real check.)

- [ ] **Step 4: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-gui/src/lib.rs
git commit -m "feat(gui): add mobile entry point for Tauri Android

cfg(mobile)-gated tauri::mobile_entry_point that drives the same run()
builder as the desktop shim. Configured out on desktop, so the desktop
build is unchanged. First real compile check is the Android build.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Run `tauri android init` and commit `gen/android`

**Files:**
- Create (generated): `src/crates/primer-gui/gen/android/**`
- Create: `src/crates/primer-gui/gen/android/.gitignore`
- Possibly modified by init: `src/crates/primer-gui/Cargo.toml` (`[lib] crate-type`), `src/crates/primer-gui/tauri.conf.json` (android bundle section)

- [ ] **Step 1: Run `tauri android init`**

```bash
cd /Users/hherb/src/primer/src/crates/primer-gui
export ANDROID_HOME="$HOME/Library/Android/sdk"
export NDK_HOME=/opt/homebrew/share/android-ndk
export JAVA_HOME="$(/usr/libexec/java_home -v 21)"
~/.cargo/bin/cargo-tauri android init 2>&1 | tail -30
```
Expected: success message; a new `gen/android/` directory exists. If it errors on a missing `[lib] crate-type` or on the crate being binary-only, read the error — `cargo-tauri` 2.11 normally adds the needed lib target and config automatically; if it asks you to, apply exactly what it instructs and re-run.

- [ ] **Step 2: Inspect what init changed**

```bash
cd /Users/hherb/src/primer
git status
git diff -- src/crates/primer-gui/Cargo.toml src/crates/primer-gui/tauri.conf.json
ls src/crates/primer-gui/gen/android
```
Expected: `gen/android/` is untracked; `Cargo.toml` may have gained `[lib] crate-type = [...]`; `tauri.conf.json` may have gained android-bundle fields. Confirm the changes are init-generated and reasonable (no deletion of existing desktop config).

- [ ] **Step 3: Verify the desktop build STILL works after init's mutations**

This is the critical regression gate — `init` can add a `cdylib`/`staticlib` lib target that must not break the desktop binary (from `src/`):
```bash
~/.cargo/bin/cargo +1.88 build -p primer-gui 2>&1 | tail -3
~/.cargo/bin/cargo +1.88 test -p primer-gui 2>&1 | grep "test result:"
```
Expected: build `Finished`; tests `0 failed`. If the desktop build broke (e.g. the lib `crate-type` change), adjust `Cargo.toml` so it lists `"rlib"` alongside any `"cdylib"`/`"staticlib"` init added (the desktop binary + unit tests need `rlib`), then re-verify.

- [ ] **Step 4: Add the `gen/android` build-output `.gitignore`**

Create `src/crates/primer-gui/gen/android/.gitignore`:
```
# Gradle / NDK build outputs — regenerated on every build.
.gradle/
build/
app/build/
app/.cxx/
# Machine-specific SDK path written by Gradle.
local.properties
```

- [ ] **Step 5: Commit the scaffold**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-gui/gen/android src/crates/primer-gui/Cargo.toml src/crates/primer-gui/tauri.conf.json
git status   # confirm .gradle/ build/ local.properties are NOT staged
git commit -m "build(gui): tauri android init — generate gen/android scaffold

Generated Gradle project for the Android target, committed per Tauri
practice for reproducible/CI-buildable APKs. .gitignore excludes Gradle
build outputs and the machine-specific local.properties. Desktop build
verified unaffected by the [lib]/tauri.conf.json mutations.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Build the BM25-only debug APK

**Files:**
- Possibly modify: `src/crates/primer-gui/gen/android/app/build.gradle.kts` or `src/crates/primer-gui/tauri.conf.json` (to pass `--no-default-features` to the Android cargo build)

- [ ] **Step 1: Locate the Android cargo-feature knob**

`cargo-tauri` 2.11 drives the Rust build through Gradle. Find where build args / features are configured:
```bash
cd /Users/hherb/src/primer/src/crates/primer-gui
grep -rn "features\|no-default\|cargo" gen/android/app/build.gradle.kts gen/android/build.gradle.kts 2>/dev/null
~/.cargo/bin/cargo-tauri android build --help 2>&1 | grep -iA1 "feature\|no-default"
```
Expected: identify either a `cargo-tauri` CLI flag (`--no-default-features` / `-f`/`--features`) or a `build.gradle.kts` Rust-plugin block (`tauri { ... }` or a cargo args list). Prefer the CLI flag if `cargo-tauri android build` accepts it.

- [ ] **Step 2: Build the debug APK with default features disabled**

Using the CLI flag if available (preferred):
```bash
cd /Users/hherb/src/primer/src/crates/primer-gui
export ANDROID_HOME="$HOME/Library/Android/sdk"
export NDK_HOME=/opt/homebrew/share/android-ndk
export JAVA_HOME="$(/usr/libexec/java_home -v 21)"
~/.cargo/bin/cargo-tauri android build --apk --debug --target aarch64 --no-default-features 2>&1 | tail -40
```
If `cargo-tauri` rejects `--no-default-features`, instead set it in `gen/android/app/build.gradle.kts` (in the Rust-build/cargo args block identified in Step 1) and run the same command without the flag. Expected: Gradle assembles a debug APK; build ends `BUILD SUCCESSFUL` / Tauri prints the APK path.

- [ ] **Step 3: Verify the APK artifact exists**

```bash
find /Users/hherb/src/primer/src/crates/primer-gui/gen/android -name "*.apk" -path "*debug*" 2>/dev/null
```
Expected: at least one `*-debug.apk` under `gen/android/app/build/outputs/apk/`. This is the session's primary acceptance artifact.

- [ ] **Step 4: Confirm the native lib was built WITHOUT `embedding` (BM25-only)**

The Android build must not pull `fastembed`/`ort` (#157). Confirm by checking the build invocation captured Step 2 output, or inspect the staged cargo features:
```bash
# The build log from Step 2 should show the cargo invocation; grep it.
# Re-run with a verbose tail if needed:
cd /Users/hherb/src/primer/src/crates/primer-gui
~/.cargo/bin/cargo-tauri android build --apk --debug --target aarch64 --no-default-features 2>&1 | grep -i "fastembed\|ort-sys\|--no-default" | head
```
Expected: NO `fastembed`/`ort-sys` compile lines; the `--no-default-features` (or the gradle-configured equivalent) is in effect. If `ort-sys` compiles, the feature knob from Step 1 is not wired — fix it before proceeding.

- [ ] **Step 5: Commit any build-config edits**

Only if Step 2 required a `build.gradle.kts`/`tauri.conf.json` edit:
```bash
cd /Users/hherb/src/primer
git add src/crates/primer-gui/gen/android/app/build.gradle.kts src/crates/primer-gui/tauri.conf.json
git commit -m "build(gui): Android build uses --no-default-features (BM25-only)

Mirrors the CLI's #157 Android posture — keeps the device-unverified
aarch64-linux-android ONNX-runtime download (fastembed/ort) out of the
Android build. Retrieval is BM25-only on Android.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Write the Android build runbook

**Files:**
- Create: `docs/devel/android-build-quickstart.md`

- [ ] **Step 1: Write the runbook**

Create `docs/devel/android-build-quickstart.md`:
```markdown
# Android build quickstart (primer-gui)

Builds a debug APK of the Primer GUI for `aarch64-linux-android` via Tauri
mobile. This is the Phase 3 packaging scaffold (sub-project 1) — it does
**not** yet include the QNN NPU backend or the model-asset bundle; see
`docs/superpowers/specs/2026-06-11-android-packaging-scaffold-design.md`
for the roadmap to the first on-device NPU token.

## Prerequisites (verified on this macOS host, 2026-06-11)

- `cargo-tauri` 2.11.1 — `~/.cargo/bin/cargo-tauri --version`
- Android SDK at `~/Library/Android/sdk`
- Android NDK r29 at `/opt/homebrew/share/android-ndk`
- JDK 21
- `rustup target add aarch64-linux-android --toolchain 1.88`

## Environment

```bash
export ANDROID_HOME="$HOME/Library/Android/sdk"
export NDK_HOME=/opt/homebrew/share/android-ndk
export JAVA_HOME="$(/usr/libexec/java_home -v 21)"
```

## One-time scaffold (already committed)

`gen/android` is committed. Only re-run if regenerating from scratch:

```bash
cd src/crates/primer-gui
cargo-tauri android init
```

## Build a debug APK (BM25-only)

```bash
cd src/crates/primer-gui
cargo-tauri android build --apk --debug --target aarch64 --no-default-features
```

The APK lands under `gen/android/app/build/outputs/apk/`.

`--no-default-features` keeps `fastembed`/`ort` (the device-unverified
aarch64-android ONNX-runtime download, issue #157) out of the Android
build. Android retrieval is BM25-only by guidance.

## What's NOT here yet

- **QNN NPU backend** (sub-project 2): enable the `qnn` feature for the
  Android build; bundle `libGenie.so` + QAIRT `.so`s as `jniLibs`.
- **Model assets** (sub-project 3): the multi-GB Qwen3-4B bundle.
- **On-device run** (sub-project 4): install → DSP grant → first NPU
  token. The DSP (`/dev/fastrpc-cdsp`) grant only applies to a
  normally-launched app, which is why this packaging path exists.
  See `[[project_qnn_dsp_needs_app_packaging]]`.

## No device this session

Acceptance for the scaffold is a clean APK build only. Installing and
running on the RedMagic 11 Pro is a later, device-gated session.
```

- [ ] **Step 2: Commit**

```bash
cd /Users/hherb/src/primer
git add docs/devel/android-build-quickstart.md
git commit -m "docs: Android build quickstart runbook

Documents the host-side debug-APK build, the BM25-only feature posture,
and the deferred QNN/asset/on-device path.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Final regression sweep

**Files:** none (verification only)

- [ ] **Step 1: Full desktop sweep**

From `src/`:
```bash
~/.cargo/bin/cargo +1.88 fmt --all -- --check && echo FMT_OK
~/.cargo/bin/cargo +1.88 clippy -p primer-gui --all-targets -- -D warnings 2>&1 | tail -3
~/.cargo/bin/cargo +1.88 test -p primer-gui 2>&1 | grep "test result:"
~/.cargo/bin/cargo +1.88 build -p primer-gui 2>&1 | tail -3
```
Expected: `FMT_OK`; clippy clean; tests `0 failed`; build `Finished`. The Android scaffold must leave the desktop GUI fully green.

- [ ] **Step 2: Confirm the working tree is clean and branch is ready for PR**

```bash
cd /Users/hherb/src/primer
git status   # clean
git log --oneline origin/main..HEAD   # spec + 3-4 scaffold commits
```
Expected: clean tree; the branch carries the spec commit plus the Task 1–4 commits.

---

## Self-review notes

- **Spec coverage:** §1 mobile entry point → Task 1. §2 `tauri android init` + commit policy → Task 2. §3 BM25-only → Task 3. §4 NDK/env → documented in the per-task env exports + Task 4 runbook. §5 host-side verification → Tasks 3 & 5. §6 CI deferred → not a task (correctly out of scope, recorded in spec). Out-of-scope sub-projects 2–4 → Task 4 runbook "What's NOT here yet" section.
- **No device:** every acceptance is build-only, consistent with the spec.
- **Regression discipline:** Tasks 1, 2 (Step 3), and 5 all re-verify the desktop build/tests — the scaffold's biggest risk is `init` mutating `Cargo.toml`/`tauri.conf.json` in a way that breaks desktop.
```
