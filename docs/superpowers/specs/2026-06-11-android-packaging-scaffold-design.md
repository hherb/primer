# Android Packaging Scaffold — Design

**Date:** 2026-06-11
**Status:** approved (owner-paired brainstorming)
**Scope:** Sub-project 1 of the Android-packaging effort — scaffold + baseline debug APK only.
**Phase:** unblocks the Phase 1.2 finish line (first on-device NPU token) by starting Phase 3 packaging.

## Context

The Primer's `QnnBackend` is device-validated to the HTP device-creation boundary (PR #213,
`a03ff69`). The remaining gap to a real on-device NPU token is **deployment, not code**: the
Hexagon DSP node (`/dev/fastrpc-cdsp`) is SELinux/DAC-gated to properly-packaged apps. A
sideloaded `shell`-uid binary and a Termux app-uid process are both denied even a read-open
(`Failed to create device: 14001`); the reference chatapp reaches the DSP only because it runs
as a normally-launched app. See `[[project_qnn_dsp_needs_app_packaging]]`.

The fix is to run the Primer inside a properly-packaged Android app. `primer-gui` is already a
fully working Tauri 2.x desktop app (launch picker, streaming chat, settings, persistence, voice
mode). Packaging it for Android via Tauri mobile reuses the entire working GUI and IPC surface
and — crucially — gives us the app-uid DSP grant for free.

This effort is multi-session. It decomposes into four sub-projects, of which **only sub-project
1 is in scope here**:

1. **Scaffold + baseline build** (THIS SPEC) — `tauri android init`, mobile entry point,
   BM25-only Android feature set, a debug APK of the existing GUI building host-side. No device.
2. **QNN-on-Android** — enable the `qnn` cargo feature for the Android build; bundle
   `libGenie.so` + QAIRT runtime `.so`s as Android `jniLibs`.
3. **Model-asset bundling** — strategy for the multi-GB Qwen3-4B (w4a16, 4096 ctx) bundle.
4. **On-device first token** — install → DSP grant → first NPU token (device-gated).

## Environment (verified 2026-06-11, this macOS host)

- `cargo-tauri` 2.11.1 (`~/.cargo/bin/cargo-tauri`)
- Android NDK **r29** (`29.0.14206865`) at `/opt/homebrew/share/android-ndk`
- Android SDK at `~/Library/Android/sdk` (`ANDROID_HOME` set; `adb` lives under
  `platform-tools/`, not on `PATH`)
- JDK 21 (OpenJDK 21.0.8)
- rustup target `aarch64-linux-android` installed for the pinned 1.88 toolchain
- Workspace `tauri = { version = "2", default-features = false }`; `primer-gui` re-enables `wry`
- No `package.json`; `tauri.conf.json` uses `frontendDist: "ui"` with no `beforeBuildCommand`
  → no npm/node in the build loop; the pure-JS frontend ships as static assets
- **No device connected** → on-device verification (install/run/token) is out of scope today

## Chosen approach

**A — `cargo tauri android init` (full Tauri mobile).** Reuses the working GUI, IPC commands,
streaming, settings, and persistence unchanged. The `QnnBackend` (sub-project 2) will run inside
the normally-launched app process, which is exactly what supplies the DSP grant the brief
identified as the blocker.

Rejected alternatives:
- **B — minimal native-activity APK.** Discards the working GUI + IPC plumbing; forces rebuilding
  a UI later. The brief's stated fallback, worth revisiting only if Tauri mobile fights us badly.
- **C — wrap the CLI binary in an APK shell.** Android apps aren't CLIs; no real UX. Rejected.

## Design — sub-project 1 (in scope)

### 1. Mobile entry point

Tauri mobile requires a library entry point annotated for the `mobile` cfg. `primer-gui` already
has the right shape: `lib.rs` exposes `run()` and `main.rs` is a thin desktop shim.

Add a `mobile`-gated entry point in `lib.rs` that drives the same Tauri builder the desktop
`run()` uses:

```rust
#[cfg(mobile)]
#[tauri::mobile_entry_point]
fn mobile_entry_point() {
    // Drive the same builder as desktop `run()`. Startup-time tracing
    // init failure stays non-fatal (mirrors desktop posture).
    if let Err(e) = run() {
        eprintln!("primer-gui (mobile) exited with error: {e}");
    }
}
```

Refactor `run()` only as far as needed so the shared Tauri `Builder` construction is callable
from both the desktop `main.rs` shim and the mobile entry point. The desktop path stays
behaviourally identical.

**Known runtime gap (NOT fixed here):** `run()` resolves `~/.primer/` paths via `$HOME` and the
seed corpus via bundled `resources/`. On Android those resolve differently (app-specific dirs via
Tauri's path API). For this session the build must only *compile and link* — correct on-device
path resolution is a sub-project-2+ concern, tracked as a follow-up.

### 2. `tauri android init`

Run `cargo tauri android init` from `crates/primer-gui/`. Generates
`crates/primer-gui/gen/android/` — a Gradle project (app module, `build.gradle.kts`,
`AndroidManifest.xml`, Rust-plugin wiring).

**Commit policy:** commit `gen/android` (standard Tauri practice — keeps the APK reproducible and
CI-buildable without re-running `init`), with a `.gitignore` for build outputs:

```
gen/android/.gradle/
gen/android/build/
gen/android/app/build/
gen/android/app/.cxx/
gen/android/local.properties
```

`local.properties` carries the machine-specific SDK path and must not be committed. Debug builds
use Android's default debug keystore (not committed). Release signing is a Phase 3 concern, out of
scope.

### 3. Android feature set = BM25-only

The default `primer-gui` feature set includes `embedding` (fastembed/ort). The CLI's
`android-cross-compile` CI job deliberately builds `--no-default-features` to avoid dragging the
device-unverified `aarch64-linux-android` ONNX-runtime download into the Android path (issue
#157 — "Android stays BM25-only by guidance"). The Android GUI build mirrors this: compile with
`--no-default-features` (BM25-only retrieval; speech is already off by default, so no
cpal/whisper/piper/swift on Android).

Mechanism: configure the feature set for the Android cargo invocation. Tauri mobile drives cargo
through Gradle; the feature flags live in the Tauri Android config / the generated
`build.gradle.kts` Rust-build args (whichever `cargo-tauri` 2.11 exposes). The acceptance test is
that the produced APK's native lib was compiled without `embedding`.

### 4. NDK / env wiring

The build needs:
- `NDK_HOME=/opt/homebrew/share/android-ndk` (or `ANDROID_NDK_HOME`; currently unset)
- `ANDROID_HOME=~/Library/Android/sdk` (already set)
- `JAVA_HOME` → JDK 21 (Gradle requirement)

These are documented in a short runbook (`docs/devel/android-build-quickstart.md` or appended to
the existing redmagic quickstart), **not** committed as repo env.

### 5. Verification (host-side, this session)

```bash
cd src/crates/primer-gui
NDK_HOME=/opt/homebrew/share/android-ndk \
  ~/.cargo/bin/cargo-tauri android build --apk --debug --target aarch64
```

**Acceptance for this session:** the command produces a debug APK for `aarch64-linux-android`
with a clean build. No install, no run, no token (no device).

Also required green (regression guard — the scaffold must not break the desktop build):
- `~/.cargo/bin/cargo +1.88 build -p primer-gui` (desktop, default features) still builds
- `~/.cargo/bin/cargo +1.88 test -p primer-gui` still passes
- `~/.cargo/bin/cargo +1.88 fmt --all -- --check` clean
- `~/.cargo/bin/cargo +1.88 clippy -p primer-gui --all-targets -- -D warnings` clean

### 6. CI

**Deferred to a documented follow-up.** A full `tauri android build` in CI needs JDK + SDK + NDK
+ Gradle on the runner — heavier than the existing `cargo build --target aarch64-linux-android`
CLI guard. The spec records a cheap interim candidate — a lib-only drift guard
(`cargo build -p primer-gui --no-default-features --target aarch64-linux-android`, no Gradle) —
but CI wiring is **not** done this session. Deliverable is a documented, locally-reproducible
build + runbook.

## Out of scope (deferred sub-projects — context, not built here)

- **Sub-project 2 — QNN-on-Android.** Enable the `qnn` cargo feature for the Android GUI build;
  bundle `libGenie.so` + the QAIRT runtime `.so`s (incl. `libQnnHtpNetRunExtensions.so`,
  `libcdsprpc.so`) as Android `jniLibs/arm64-v8a`. Wire the GUI's existing qnn bundle-dir /
  QAIRT-lib-dir path config to Android app-private storage.
- **Sub-project 3 — model-asset bundling.** The multi-GB Qwen3-4B bundle. Leaning toward
  **adb-push to app-private external storage** for the dev/demo device (matches the device
  staging already in place from PR #213; zero hosting; not a shippable consumer story but the
  fastest path to the first token). APK-asset bundling and first-run download are the consumer
  alternatives, evaluated later.
- **Sub-project 4 — on-device first token.** Install → DSP grant verification (the `14001`
  boundary should clear once app-uid) → first Socratic NPU token. Device-gated; this is the real
  Phase 1.2 finish line.

## Risks

- **`tauri android init` may expect a `lib.rs` crate-type / Tauri config shape** the current
  binary-only crate doesn't have. Mitigation: `cargo-tauri` 2.11 typically adds the needed
  `[lib] crate-type = ["staticlib", "cdylib", "rlib"]` and config; verify the desktop build still
  works after init, and revert/adjust if it mutates the desktop target.
- **Feature-flag plumbing through Gradle.** Getting `--no-default-features` to the Android cargo
  invocation may require editing the generated `build.gradle.kts` or the Tauri Android config;
  the exact knob in `cargo-tauri` 2.11 must be confirmed during implementation.
- **The webkit2gtk concern is a non-issue on Android** (wry uses the Android System WebView), but
  confirm no Linux-only dep sneaks in via a default feature.
- **No device to validate the APK runs** — acceptance is build-only. A built APK that crashes on
  launch (e.g. HOME/path resolution) would only surface in sub-project 2+. This is an accepted
  limitation of the no-device session.
