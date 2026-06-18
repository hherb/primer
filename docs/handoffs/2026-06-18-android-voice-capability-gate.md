# Android-Native Voice POC — Plan 1 capability gate (on-device result)

**Date:** 2026-06-18
**Device:** RedMagic 11 Pro (NX809J, NubiaOS, Android 16 / API 36), adb serial `912607710061`
**Spec:** [docs/superpowers/specs/2026-06-18-android-native-voice-poc-design.md](../superpowers/specs/2026-06-18-android-native-voice-poc-design.md)
**Plan:** [docs/superpowers/plans/2026-06-18-android-native-voice-bridge-diagnostic.md](../superpowers/plans/2026-06-18-android-native-voice-bridge-diagnostic.md)
**Branch:** `android-native-voice-poc`

## Verdict: GO ✅

On-device offline STT + an offline en-US TTS voice both exist on the real device.
The OS-native voice architecture (Plan 1 §1) is validated; **Plan 2 (the voice loop) is unblocked.**

## Evidence

Read off the device from `files/speech_caps.json` (the diagnostic writes a JSON
mirror to app-internal storage because **logcat is dead on this ROM** — same
finding as the QNN work; read via `run-as org.theprimer.gui cat files/...`).

- **`on_device_recognition_available: true`** — `SpeechRecognizer.isOnDeviceRecognitionAvailable(ctx)`
  returns true on the device, confirming the ASI/SODA on-device recognizer at
  runtime (not just the circumstantial `pm`-recon from the design phase).
- **Offline en-US TTS voices present** (`network_required:false, not_installed:false`):
  `en-us-x-sfg-local`, `en-us-x-tpf-local`, `en-us-x-iol-local`, `en-us-x-iom-local`,
  `en-us-x-iog-local`, `en-us-x-tpc-local`, `en-us-x-tpd-local`, `en-us-x-iob-local`,
  `en-US-language`, plus several en-AU local voices. `select_offline_voice(voices,"en-US")`
  returns a real installed offline voice.
- **The `not_installed` filter is load-bearing:** the engine advertises ~370 voices,
  the overwhelming majority `not_installed:true` (downloadable, absent). Only the
  en-US / en-AU set is installed. Without the `not_installed` guard, selection would
  have returned an uninstalled voice. The guard works as designed.

## Open item carried to Plan 2: the Rust→JNI bridge round-trip

The capability answer above came from the **direct Kotlin path** (`MainActivity` →
`PrimerSpeech.queryCapabilities()` → file), which is pure Android APIs and needs no
Rust/JNI. The **Rust→JNI→Kotlin round-trip** (the `speech_capabilities` Tauri command →
`ndk_context` → `JavaVM` → `find_class` → static call) **fails at runtime** — diagnosed
via an instrumented scaffold writing INVOKED / ERROR:<msg> / JSON to
`files/speech_caps_via_rust.json`:

- The file was written as **`INVOKED: calling query_capabilities`** and then **never
  overwritten** — neither the `Ok(caps)` (JSON) nor the `Err(e)` (ERROR:…) arm ran.
- So the `app.js` probe DID fire and the Tauri command WAS invoked, but
  `primer_speech::android::query_capabilities()` **never returned** — it panicked before
  reaching either match arm.
- The panic is almost certainly `ndk_context::android_context()` in `JniSpeechBridge::new()`:
  `ndk_context` **panics if the global context was never initialized**, and the
  Tauri-mobile runtime does not populate it for our call path. (The identical Kotlin
  `queryCapabilities` succeeds when called directly from `MainActivity`, so the failure
  is upstream of the Kotlin call, in the Rust→JNI bootstrap.)

This is exactly the plan's **documented #1 risk** (`ndk_context` population under the
Tauri-mobile runtime). The capability gate does NOT depend on it (the OS-native APIs are
reachable from Kotlin directly), but Plan 2's loop wiring does. **Resolution path
(documented in the plan's Risks), now confirmed necessary — Plan 2 Task 1:** add a
`#[no_mangle] extern "C"` `nativeInit(JNIEnv, JObject)` exported from Rust and called
from `MainActivity.onCreate`, caching the `JavaVM` (via `env.get_java_vm()`) in a
`OnceLock`, and have `JniSpeechBridge::new()` read that cached VM instead of
`ndk_context`. The Kotlin `PrimerSpeech.init` already caches the Context.

## Reproduce

```bash
ADB="$HOME/Library/Android/sdk/platform-tools/adb"; D=912607710061
# build (NDK r29 @ /opt/homebrew/share/android-ndk, JDK 21):
cd src/crates/primer-gui
NDK_HOME=/opt/homebrew/share/android-ndk ANDROID_HOME=~/Library/Android/sdk \
  ~/.cargo/bin/cargo-tauri android build --apk --debug --target aarch64 -- \
  --no-default-features --features android-native
"$ADB" -s $D install -r gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
"$ADB" -s $D shell am start -n org.theprimer.gui/.MainActivity
# logcat is DEAD on this ROM — read the result file instead:
"$ADB" -s $D shell run-as org.theprimer.gui cat files/speech_caps.json
```

## Note on gate scaffolding

The file-mirror diagnostic (`PrimerSpeech.kt` file write, `MainActivity` direct probe,
`speech_diag.rs` round-trip recorder, the `app.js` probe) is **temporary `[GATE-SCAFFOLD]`
code**, to be reverted before the branch merges — Plan 1's permanent surface is the
`speech_capabilities` Tauri command (a real Settings→Diagnostics trigger lands in Plan 2).
