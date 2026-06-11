# QAIRT / Genie native libraries — manual staging required

This directory is where the **QNN-on-Android** build (`--features qnn`) expects
the Qualcomm QAIRT / Genie runtime shared libraries. Android Gradle Plugin
auto-bundles every `*.so` here into `lib/arm64-v8a/` of the APK, and the app
`dlopen`s `libGenie.so` at runtime (`primer-qnn-sys::GenieLibrary::open`).

## These `.so` files are NOT committed — and must not be

`gen/android/app/.gitignore` carries `/src/main/jniLibs/**/*.so`, so every
library staged here is git-ignored. That is deliberate:

- They are **proprietary Qualcomm binaries** gated behind a Qualcomm developer
  account; they cannot be redistributed in this public AGPL repository.
- The app's own `libprimer_gui.so` cdylib (symlinked here by `cargo-tauri
  android build`) is also ignored by the same rule.

So a fresh clone has an *empty* `arm64-v8a/` (just this README). A QNN APK build
will succeed at the cargo+gradle level but the resulting APK will be missing the
9 QAIRT libs — you must stage them first.

## Which libraries (9 files)

Sourced from the QAIRT **2.45** SDK, HTP **V79** skel (Snapdragon 8 Elite /
SM8850-class Hexagon NPU — the dev/demo device is a RedMagic 11 Pro). The
sha256 manifest below pins the exact build validated against the Primer's
`QnnBackend` (PR #213, device-validated FFI):

```
5cebceb9f7866e9f5cd5841c6ed73312afbfba5c165a19e75a66cb4343160667  libGenie.so
4a36bb9fea544751326e7b45db2a83265dfd635575f475548111cca8dd0562b3  libQnnHtp.so
9d2b04e2ffbc244421d24e7570d402b3b7d250dd830c6aee044a7821f5ea1f31  libQnnHtpNetRunExtensions.so
22baed9c84bb817ddfb6adaf09e4602553886fea32eaa137f0baafc614ffc6f2  libQnnHtpPrepare.so
892ee975c0f7a6f5fbc5f17a5876016286cb9b47cb0eb2a295c6c513f5ad4c3e  libQnnHtpV79CalculatorStub.so
747528be359d03c9ce2d80d611117b6c84421646c245354edc0a5adde4c760d7  libQnnHtpV79Skel.so
d6656dca6fdcd58475fcb160b52a7b4aec9b450190ed9da81aec5582a083b085  libQnnHtpV79Stub.so
73027c46810cf228b7310200f02c6fee445622d12dc258139cebd5286f4b184a  libQnnSaver.so
55d920c008337242f183eeca65379a881c8c8190066b38dc1db1427132ce8a8e  libQnnSystem.so
```

Only `libGenie.so` is `dlopen`'d by name from Rust; the rest are its transitive
dependencies (the HTP backend + V79 skel/stub) resolved by the dynamic linker
from this same `lib/arm64-v8a/` directory at load time.

### `libcdsprpc.so` is intentionally absent

FastRPC's client library (`libcdsprpc.so`) is a **system / vendor** library that
lives on the device at `/vendor/lib64/libcdsprpc.so` and is resolved from there
at runtime. It is NOT bundled into the APK — bundling a vendor FastRPC stub can
break the DSP handshake. This is why the Hexagon DSP grant (`/dev/fastrpc-cdsp`)
only works from a normally-launched, packaged app — see
`project_qnn_dsp_needs_app_packaging`.

## How to stage them

### Option A — `adb pull` from the device staging area (used to validate this scaffold)

The PR #213 on-device validation left the libs at `/data/local/tmp/primer-qnn/qairt/`:

```bash
ADB="$HOME/Library/Android/sdk/platform-tools/adb"   # or just `adb` if on PATH
JNI="src/crates/primer-gui/gen/android/app/src/main/jniLibs/arm64-v8a"
"$ADB" pull /data/local/tmp/primer-qnn/qairt/. "$JNI/"
# verify you got the right build (the grep anchors on the 64-hex-digit manifest
# rows so prose mentions of *.so don't leak malformed lines into shasum -c):
( cd "$JNI" && shasum -a 256 -c <(grep -E '^[0-9a-f]{64}  lib' README.md) )
```

### Option B — copy from a QAIRT SDK install

From a `qairt/<version>/lib/aarch64-android/` directory (and the HTP V79 skel
from `lib/hexagon-v79/unsigned/`), copy the 9 files above into this directory.

## Build the QNN APK once staged

```bash
export ANDROID_HOME="$HOME/Library/Android/sdk"
export NDK_HOME=/opt/homebrew/share/android-ndk
export JAVA_HOME="/Applications/Android Studio.app/Contents/jbr/Contents/Home"
cd src/crates/primer-gui
cargo-tauri android build --apk --debug --target aarch64 -- --no-default-features --features qnn
# verify the libs landed in the APK:
unzip -l gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk \
  | grep -E 'lib/arm64-v8a/libGenie|libQnn'
```

See `docs/devel/android-build-quickstart.md` for the full runbook (host env,
NDK symlink, model-asset bundling).
