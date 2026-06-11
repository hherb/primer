# QAIRT / Genie native libraries — manual staging required

> ℹ️ **On-device status (2026-06-11, RedMagic 11 Pro / SM8850 / "canoe").** The
> Genie log-to-file diagnostic (PR #217) read behind `GenieDialog_create`'s
> generic **status -1** and found the concrete cause — **a missing per-arch HTP
> host stub**, not DSP signing and not `ADSP_LIBRARY_PATH`:
>
> ```
> Failed in loading stub: dlopen failed: library "libQnnHtpV81Stub.so" not found
> loadRemoteSymbols failed with err 4000 → Transport layer setup failed: 14001
> ```
>
> The QAIRT **2.45** runtime correctly identifies the SM8850 (8 Elite Gen 5) as
> **V81** and `dlopen`s the host-side `libQnnHtpV81Stub.so` — which earlier runs
> never staged (only the **V79** HTP libs were). This **overturns** two prior
> hypotheses recorded here: the "V79 runs on this V81 part" note (true only for an
> older QNN that didn't recognise V81; our 2.45 does) and the `ADSP_LIBRARY_PATH`
> suspect (the trace shows unsigned-PD as the default and gets *past* skel
> resolution — the host stub is the wall). **The HTP arch is the blocker.**
>
> **Fix staged (2026-06-11):** a coherent **`2.45.0.260326`** V81 set now lives
> here — host `libQnnHtpV81Stub.so` + `libQnnHtpV81CalculatorStub.so`, DSP
> `libQnnHtpV81Skel.so`, and the matching `libGenie`/`libQnnHtp`/… host libs from
> the *same* build (so the version-sensitive stub↔QnnHtp↔skel triple is one
> build, no skew). Next: rebuild APK → `adb install -r` → start a session →
> `run-as cat .primer/genie.log` to confirm the first on-device NPU token.

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

Sourced from the QAIRT **2.45.0.260326** SDK (AISW build `v2.45.0.260326154327`),
HTP **V81** for the SM8850. All nine `.so`s come from the **one** SDK zip so the
stub↔QnnHtp↔skel triple is version-coherent (the missing-`libQnnHtpV81Stub.so`
finding above is what made coherence load-bearing). The sha256 manifest:

```
e9a7608c323ae62997f37981490c3ff42e20632ec34d3865aaf4cf3f4001d567  libGenie.so
b5b36b2d5e0cc352b5e5f468f6f668b1585d515854fc1c5890b1365047a36be1  libQnnHtp.so
0d29d3dac7d82d8eb5bdc47a0649a7d6a5f5b4a2fbc52b2f975b6ae2c16b6213  libQnnHtpNetRunExtensions.so
d0001d629c6d7ded14f2c850e93983b87589cd5748724bb9aaf56f7adb2b338a  libQnnHtpPrepare.so
53c464f8833cabc65baf16d0292aa0fe293909bf9333327092283261f713e603  libQnnHtpV81CalculatorStub.so
57330ed3f7846b94181b25b7a87bc3fe5dc71033766f2d5630520a1400477c97  libQnnHtpV81Skel.so
e97898d6772288e202e9cf988b80b39b8336f3dca8b99ee3075d234093582e3c  libQnnHtpV81Stub.so
61376cb2d732dc680466740366d1c719b058370f4adfa43dae1ff4514a781f1b  libQnnSaver.so
f08e9ddc18a40719839cce8bbfb3afd3aef06f5ad486b21934a1c4568c9a6a48  libQnnSystem.so
```

`libQnnHtpV81Skel.so` is the **DSP-side** (Hexagon DSP6 ELF) lib from the SDK's
`lib/hexagon-v81/unsigned/`; the other eight are host **aarch64-android** libs.
Only `libGenie.so` is `dlopen`'d by name from Rust; the rest are its transitive
dependencies (the HTP backend + V81 skel/stub) resolved by the dynamic linker
from this same `lib/arm64-v8a/` directory at load time. There is **no** host-side
`libQnnHtpV81.so` (that name exists only under `hexagon-v81/` as a DSP lib) — the
host per-arch files are the stub + calculator-stub, mirroring the V79 set.

### `libcdsprpc.so` is intentionally absent

FastRPC's client library (`libcdsprpc.so`) is a **system / vendor** library that
lives on the device at `/vendor/lib64/libcdsprpc.so` and is resolved from there
at runtime. It is NOT bundled into the APK — bundling a vendor FastRPC stub can
break the DSP handshake. This is why the Hexagon DSP grant (`/dev/fastrpc-cdsp`)
only works from a normally-launched, packaged app — see
`project_qnn_dsp_needs_app_packaging`.

## How to stage them

### Option A — direct download from the open Software Center endpoint (no QPM, no login)

The Qualcomm Software Center exposes an **unauthenticated** direct-download API
(QPM3 — Linux/Windows only — is *not* required, which matters on macOS). The zip
is ~1.66 GB but you only need ~110 MB of libs, so fetch just those entries over
HTTP range requests with `remotezip` (no full download):

```bash
URL="https://softwarecenter.qualcomm.com/api/download/software/sdks/Qualcomm_AI_Runtime_Community/All/2.45.0.260326/v2.45.0.260326.zip"
B="qairt/2.45.0.260326/lib"
uvx --from remotezip remotezip "$URL" \
  "$B/aarch64-android/libGenie.so" "$B/aarch64-android/libQnnHtp.so" \
  "$B/aarch64-android/libQnnHtpNetRunExtensions.so" "$B/aarch64-android/libQnnHtpPrepare.so" \
  "$B/aarch64-android/libQnnSaver.so" "$B/aarch64-android/libQnnSystem.so" \
  "$B/aarch64-android/libQnnHtpV81Stub.so" "$B/aarch64-android/libQnnHtpV81CalculatorStub.so" \
  "$B/hexagon-v81/unsigned/libQnnHtpV81Skel.so"
JNI="src/crates/primer-gui/gen/android/app/src/main/jniLibs/arm64-v8a"
find qairt -name '*.so' -exec cp -p {} "$JNI/" \;
# verify (the grep anchors on the 64-hex-digit manifest rows):
( cd "$JNI" && shasum -a 256 -c <(grep -E '^[0-9a-f]{64}  lib' README.md) )
```

The package version string is `2.45.0.260326` (`MAJOR.MINOR.0.YYMMDD`); other
versions are listed at <https://softwarecenter.qualcomm.com/catalog/item/Qualcomm_AI_Runtime_Community>.
The lib's *internal* `AISW_VERSION` (`2.45.41…`) is **not** the package version —
do not put it in the URL (it 404s as `NoSuchKey`).

### Option B — copy from a QAIRT SDK install

From a `qairt/<version>/lib/aarch64-android/` directory (and the HTP V81 skel
from `lib/hexagon-v81/unsigned/`), copy the 9 files above into this directory.

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
