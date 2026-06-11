# Android build quickstart (primer-gui)

Builds a debug APK of the Primer GUI for `aarch64-linux-android` via Tauri
mobile. Two build flavours are covered:

- **BM25-only** (sub-project 1 — the packaging scaffold): the GUI, no NPU.
- **QNN-on-Android** (sub-project 2): the same APK with the `qnn` feature on and
  the QAIRT / Genie runtime `.so`s bundled into `jniLibs/arm64-v8a`.

Still ahead: the multi-GB model-asset bundle (sub-project 3) and the first
on-device NPU token (sub-project 4, device-gated). See
[docs/superpowers/specs/2026-06-11-android-packaging-scaffold-design.md](../superpowers/specs/2026-06-11-android-packaging-scaffold-design.md)
for the roadmap to the first on-device NPU token.

Verified end-to-end on a macOS arm64 host on 2026-06-11: a clean BM25-only
`app-universal-debug.apk` (~196 MB, debug/unstripped) builds with no
fastembed/ort, and a QNN-feature APK (~406 MB, carrying the 9 QAIRT `.so`s)
builds with the libs staged from the device. No device is needed to *build*
either flavour (the QNN libs just have to be staged into `jniLibs` first).

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

## Build the QNN APK (sub-project 2)

The QNN-on-Android APK adds two things to the BM25-only build: the `qnn` cargo
feature (compiled in via `--features qnn`, chaining to `primer-engine/qnn`), and
the 9 proprietary QAIRT / Genie `.so`s bundled into the APK's `lib/arm64-v8a/`.

**The `.so`s are git-ignored and must be staged manually first** (Qualcomm
licence + public AGPL repo). See
[the jniLibs staging README](../../src/crates/primer-gui/gen/android/app/src/main/jniLibs/arm64-v8a/README.md)
for the 9 files, their sha256 manifest, and the two staging routes (`adb pull`
from the device staging area, or copy from a QAIRT SDK install). With a device
connected:

```bash
ADB="$HOME/Library/Android/sdk/platform-tools/adb"
JNI="src/crates/primer-gui/gen/android/app/src/main/jniLibs/arm64-v8a"
"$ADB" pull /data/local/tmp/primer-qnn/qairt/. "$JNI/"
```

Then build with the `qnn` feature on (note `--features` is **additive** to
defaults, so `--no-default-features` is still required to stay BM25-only —
issue #157):

```bash
cd src/crates/primer-gui
~/.cargo/bin/cargo-tauri android build --apk --debug --target aarch64 -- --no-default-features --features qnn
```

Verify the libs landed in the APK:

```bash
unzip -l gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk \
  | grep -E 'lib/arm64-v8a/(libGenie|libQnn)'
# expect 9 QAIRT/Genie .so + the app's own libprimer_gui.so
```

`libcdsprpc.so` is intentionally **not** bundled — it is a device system/vendor
library (`/vendor/lib64/`) resolved at runtime; see the jniLibs README.

## What's NOT here yet

- **Model assets** (sub-project 3): the multi-GB Qwen3-4B (w4a16, v79) bundle.
  The on-device staging dir already holds it at
  `/data/local/tmp/primer-qnn/bundle/`; wiring the GUI's `qnn_bundle_dir` to an
  app-readable path is the next step.
- **On-device run** (sub-project 4): install → DSP grant → first NPU token. The
  DSP (`/dev/fastrpc-cdsp`) grant only applies to a normally-launched app, which
  is why this packaging path exists. See `[[project_qnn_dsp_needs_app_packaging]]`.

## On-device gaps still open

The QNN APK *builds and carries the libs*, but installing and running on the
RedMagic 11 Pro is still a later, device-gated step. The on-device runtime has
known path-resolution gaps (the GUI resolves `~/.primer/` via `$HOME` and the
seed corpus via bundled `resources/`, which differ on Android) — those are
sub-project 3/4 work, not addressed by this build.
