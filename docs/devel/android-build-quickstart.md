# Android build quickstart (primer-gui)

Builds a debug APK of the Primer GUI for `aarch64-linux-android` via Tauri
mobile. This is the Phase 3 packaging **scaffold** (sub-project 1) — it does
**not** yet include the QNN NPU backend or the model-asset bundle; see
[docs/superpowers/specs/2026-06-11-android-packaging-scaffold-design.md](../superpowers/specs/2026-06-11-android-packaging-scaffold-design.md)
for the roadmap to the first on-device NPU token.

Verified end-to-end on a macOS arm64 host on 2026-06-11: a clean
`app-universal-debug.apk` (~196 MB, debug/unstripped) builds with BM25-only
retrieval (no fastembed/ort). No device is needed to build.

## Prerequisites

- `cargo-tauri` 2.11.1 — `~/.cargo/bin/cargo-tauri --version`
- Android SDK at `~/Library/Android/sdk`
- Android NDK **r29** (`29.0.14206865`) at `/opt/homebrew/share/android-ndk`
- **JDK 21** — Gradle needs exactly 21. On this host it is Android Studio's
  bundled JBR at `/Applications/Android Studio.app/Contents/jbr/Contents/Home`
  (Homebrew's `openjdk` is 25 and will not work; `/usr/libexec/java_home -v 21`
  does not find a standalone 21).
- `rustup target add aarch64-linux-android --toolchain 1.88`

### NDK discovery symlink (one-time)

Tauri 2.11's NDK probe scans `$ANDROID_HOME/ndk/<version>/` and ignores
`NDK_HOME`. Point it at the Homebrew-Cask NDK with a symlink:

```bash
ln -s /opt/homebrew/share/android-ndk "$HOME/Library/Android/sdk/ndk/29.0.14206865"
```

## Environment

```bash
export ANDROID_HOME="$HOME/Library/Android/sdk"
export NDK_HOME=/opt/homebrew/share/android-ndk
export JAVA_HOME="/Applications/Android Studio.app/Contents/jbr/Contents/Home"
```

## One-time scaffold (already committed)

`gen/android` is committed, so you normally skip this. Only re-run when
regenerating from scratch:

```bash
cd src/crates/primer-gui
~/.cargo/bin/cargo-tauri android init
```

> The `primer-gui` crate carries `[lib] crate-type = ["staticlib", "cdylib",
> "rlib"]` in `Cargo.toml`. The `cdylib` is what Android's `loadLibrary` loads;
> `rlib` keeps the desktop binary + unit tests building. `tauri android init`
> does **not** add this for a binary-only crate — it is committed manually.

## Build a debug APK (BM25-only)

```bash
cd src/crates/primer-gui
~/.cargo/bin/cargo-tauri android build --apk --debug --target aarch64 -- --no-default-features
```

The APK lands at
`gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`.

**Note the `--` separator.** `cargo-tauri android build` forwards everything
after `--` verbatim to the inner `cargo build`. `--no-default-features` MUST go
after `--`; placing it before is not accepted. This is what keeps
`fastembed`/`ort` (the device-unverified aarch64-android ONNX-runtime download,
issue #157) out of the Android build. Android retrieval is BM25-only by
guidance. Confirm with a build-log grep — `fastembed`, `ort-sys`, `tokenizers`,
and `hf-hub` should all be absent from the aarch64 compile.

## What's NOT here yet

- **QNN NPU backend** (sub-project 2): enable the `qnn` feature for the Android
  build; bundle `libGenie.so` + QAIRT `.so`s as `jniLibs/arm64-v8a`.
- **Model assets** (sub-project 3): the multi-GB Qwen3-4B bundle.
- **On-device run** (sub-project 4): install → DSP grant → first NPU token. The
  DSP (`/dev/fastrpc-cdsp`) grant only applies to a normally-launched app, which
  is why this packaging path exists. See `[[project_qnn_dsp_needs_app_packaging]]`.

## No device this session

Acceptance for the scaffold is a clean APK build only. Installing and running on
the RedMagic 11 Pro is a later, device-gated session. The on-device runtime also
has known path-resolution gaps (the GUI resolves `~/.primer/` via `$HOME` and the
seed corpus via bundled `resources/`, which differ on Android) — those are
sub-project 2+ work, not addressed by the build.
