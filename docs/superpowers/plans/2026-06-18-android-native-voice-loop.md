# Android-Native Voice POC — Plan 2 of 2: The Voice Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete the first on-device, offline voice turn inside the Tauri-Android APK — child speaks → Android on-device `SpeechRecognizer` transcribes → `QnnBackend` (NPU) answers → Android `TextToSpeech` speaks it aloud → loop returns to LISTEN — reusing the shared `primer_speech::voice_loop` state machine unchanged.

**Architecture:** Android is the first backend where the OS owns **both** the mic (`SpeechRecognizer` captures + endpoints itself — no cpal, no Silero, no audio thread) **and** the speaker (`TextToSpeech` plays itself — no cpal speaker, no ringbuf drain). The voice loop is fed through two reused seams: a channel-backed `ChannelStt` (the macos-native-26 pattern) plus a derived-VAD `event_rx`, and a `StreamingTextToSpeech` whose `push_text` calls `speak()` and blocks until the engine reports `onDone`. All decision logic (derived VAD, event parsing, offline-voice selection from Plan 1) is pure Rust, host-tested with a `MockBridge`. The real bridge (`JniSpeechBridge`) is `target_os = "android"`-gated and the only device-only code — exactly the `primer-inference::qnn` `GenieLibrary`/`GenieDialog` pattern Plan 1 established.

**Tech Stack:** Rust (`primer-speech`, `primer-gui`), `jni` 0.21, Kotlin (Android `SpeechRecognizer` + `TextToSpeech` + `RecognitionListener` + `UtteranceProgressListener`), Tauri 2.11 mobile, `serde_json`, the existing `voice_loop` state machine.

## Global Constraints

- Workspace root is `src/`; every cargo command runs from `src/`. Invoke as `~/.cargo/bin/cargo` (rustup), never Homebrew cargo. Toolchain pin **1.88**, edition 2024.
- Per-crate `Cargo.toml` uses `.workspace = true`; new deps are pinned in `src/Cargo.toml` `[workspace.dependencies]`.
- **No magic numbers** — every numeric goes to a consts module (invariant) or settings (tunable). `[[feedback_no_magic_numbers]]`.
- **Strict offline-first** — never select a TTS voice where `isNetworkConnectionRequired() == true`; never bind a network recognizer. Use `SpeechRecognizer.createOnDeviceSpeechRecognizer()` only. `[[project_strict_offline_first]]`.
- **No barge-in** — the loop is strictly LISTEN → LATENT_THINK → SPEAK → LISTEN; the recognizer is armed only in LISTEN and is stopped during SPEAK, so the Primer never hears itself and never interrupts the child. `[[project_no_barge_in_pedagogy]]`.
- The `android-native` feature is **mutually exclusive** with `macos-native` / `macos-native-26` (the existing `compile_error!` in `primer-speech/src/lib.rs`).
- The real JNI bridge is `#[cfg(target_os = "android")]`; every other target gets a stub returning `PrimerError::Speech(...)` — pure logic stays host-compilable (the `primer-qnn-sys` precedent).
- Android APK build is BM25-only / `--no-default-features --features android-native` (issue #157); the new code must cross-compile clean for `aarch64-linux-android`.
- Desktop `primer-gui` build/test/fmt/clippy must stay byte-identical when `android-native` is off.
- Files under 500 lines where reasonable; pure functions in reusable modules over inline complexity; inline docs + unit tests mandatory.
- Device: RedMagic 11 Pro, `~/Library/Android/sdk/platform-tools/adb -s 912607710061`. **logcat is dead on this ROM** — read diagnostics via `run-as org.theprimer.gui cat files/...`. APK build: `NDK_HOME=/opt/homebrew/share/android-ndk ~/.cargo/bin/cargo-tauri android build --apk --debug --target aarch64 -- --no-default-features --features android-native`.

## What Plan 1 already shipped (merged, PR #249)

- `primer-speech/android-native` feature; `primer_speech::android` module.
- `SpeechCapabilities` / `TtsVoiceInfo` types + serde (`android/capabilities.rs`).
- `select_offline_voice(&[TtsVoiceInfo], bcp47) -> Option<&TtsVoiceInfo>` — the offline-first guard (reused unchanged in Task 6 here).
- `AndroidSpeechBridge` seam trait (`android/bridge.rs`) with `query_capabilities()` — **extended** in Task 2.
- `JniSpeechBridge` (`android/jni_bridge.rs`) over `jni` — currently builds its `JavaVM` from `ndk_context::android_context()`, which **panics under the Tauri-mobile runtime** (the carried open item; fixed in Task 1).
- `PrimerSpeech.kt` Kotlin helper with `init(ctx)` + `queryCapabilities()` — **extended** with `nativeInit`/recognizer/TTS in Tasks 1, 7.
- `speech_capabilities` Tauri command (`primer-gui/src/commands/speech_diag.rs`) — registered but non-functional on-device until Task 1.
- CI `android-native` cross-compile drift-guard.

**Device gate result (handoff 2026-06-18): GO.** `on_device_recognition_available == true`; multiple offline en-US voices present (`en-us-x-tpf-local` etc., `network_required:false`). The architecture is validated; only the Rust→JNI bootstrap (Task 1) and the loop wiring (Tasks 2–10) remain.

## Design decisions locked for this plan (rationale inline; all precedent-driven)

- **D1 — Android does NOT use `LocalBackends`.** That struct (and `MicCapture`/`SpeakerSink`) is `#[cfg(feature = "cpal")]`-gated (voice_loop/mod.rs:26-27,62-63) and Android pulls no cpal. The Android builder returns a small cpal-free bundle `AndroidVoiceBackends { backends: LoopBackends, event_rx, handle }`. The GUI passes its pieces to `run_loop` with a **no-op `on_committed_audio`**, `wait_for_speaker_drain = None`, and `is_speaking = None` — all already-optional in `run_loop`'s signature (state_machine.rs:424-436).
- **D2 — `ChannelStt` is un-gated from cpal.** It is pure `std::sync::mpsc` channel code with zero cpal dependency; it only *lives* in the cpal-gated `backends_common`. Task 4 moves it to a cpal-free module so the Android STT can reuse it verbatim (DRY — no second channel adapter).
- **D3 — Android TTS blocks inside `push_text` until `onDone`.** On Android the synthesis *is* the playback. `AndroidTts::push_text` calls `bridge.speak(text)` which blocks until the Kotlin `UtteranceProgressListener.onDone` fires, emits **no** `SynthesisEvent::Audio` (so `on_committed_audio` stays a no-op), and `wait_for_speaker_drain = None`. Blocking a tokio worker for the speech duration is the documented, accepted cost already used by the macOS-native path (state_machine.rs:847-893). The recognizer is not listening during SPEAK, so the no-barge-in mic-feedback gating (`is_speaking`) is unnecessary.
- **D4 — Kotlin→Rust eventing is a poll model, not an upcall.** The recognizer's `RecognitionListener` callbacks (main-Looper-bound) enqueue events into a thread-safe queue inside `PrimerSpeech`; Rust drains them via a blocking `pollSpeechEvent()` JNI call (Rust→Kotlin only — the single direction Plan 1 de-risked). No Rust function is exported for Kotlin to call except the one-time `nativeInit`. This avoids per-event thread-attach + `GlobalRef` listener ceremony.
- **D5 — Android gets its own pure `DerivedVadStateMachine`** (Task 3), structurally mirroring `macos26/vad.rs` but with its own consts (`primer_core::consts::speech::android`). `SpeechAnalyzer` (continuous partials) and `SpeechRecognizer` (discrete `onEndOfSpeech`/`onResults`) have genuinely different event semantics, so a shared abstraction is premature (YAGNI). A DRY follow-up to hoist a common state machine is noted in Task 3 once both paths are device-tuned.

## File Structure

**New files:**
- `src/crates/primer-speech/src/android/events.rs` — `SpeechEvent` enum (`Partial`/`Final`/`EndOfSpeech`/`SttError`/`TtsDone`/`TtsError`) + serde; the JSON shape `pollSpeechEvent()` emits.
- `src/crates/primer-speech/src/android/vad.rs` — `AndroidDerivedVad` pure state machine (recognizer events → `VadEvent`).
- `src/crates/primer-speech/src/android/stt.rs` — `AndroidStt` (`StreamingSpeechToText` via `ChannelStt`) + the consumer task `run_recognizer_loop`.
- `src/crates/primer-speech/src/android/tts.rs` — `AndroidTts` (`StreamingTextToSpeech`, blocks until `onDone`).
- `src/crates/primer-speech/src/voice_loop/backends_android_native.rs` — `build_android_voice_backends(...) -> AndroidVoiceBackends`.
- `src/crates/primer-speech/src/voice_loop/channel_stt.rs` — `ChannelStt` moved here, cpal-free (Task 4).
- `src/crates/primer-gui/src/voice/backends_android.rs` — GUI Android voice-backend construction.
- `src/crates/primer-gui/src/commands/voice_android.rs` — `start_voice_mode_android` / `stop_voice_mode_android` / `cancel_voice_response_android` Tauri commands.

**Modified files:**
- `src/crates/primer-speech/src/android/bridge.rs` — extend `AndroidSpeechBridge` with the voice-loop methods; extend `MockBridge`.
- `src/crates/primer-speech/src/android/jni_bridge.rs` — `nativeInit` VM cache + the new JNI methods.
- `src/crates/primer-speech/src/android/mod.rs` — wire new modules + `query_capabilities` reads the cached VM.
- `src/crates/primer-speech/src/lib.rs` — `nativeInit` `#[no_mangle]` export (android-gated).
- `src/crates/primer-speech/src/voice_loop/mod.rs` — declare `channel_stt` (cpal-free) + `backends_android_native` (android-gated); move the `ChannelStt` re-export.
- `src/crates/primer-speech/src/voice_loop/backends_common/mod.rs` — remove `ChannelStt` (moved to `channel_stt.rs`); re-export it for back-compat.
- `src/crates/primer-speech/src/voice_loop/selectors.rs` — add `SttBackend::AndroidNative` / `TtsBackend::AndroidNative`.
- `src/crates/primer-core/src/consts.rs` — `speech::android` const block.
- `src/crates/primer-gui/gen/android/app/src/main/java/org/theprimer/gui/PrimerSpeech.kt` — recognizer + TTS + event queue + `nativeInit` declaration.
- `src/crates/primer-gui/gen/android/app/src/main/java/org/theprimer/gui/MainActivity.kt` — call `nativeInit()` + runtime `RECORD_AUDIO` request.
- `src/crates/primer-gui/src/commands/mod.rs` + `src/crates/primer-gui/src/voice/mod.rs` — register Android voice modules/commands under `android-native`.
- `src/crates/primer-gui/ui/voice.js` (or the existing voice controller) — invoke the android commands when on android.
- `.github/workflows/ci.yml` — the existing android-native guard already covers the new lib code; no change unless a new crate target appears.

---

### Task 1: Bridge bootstrap fix — `nativeInit` JavaVM cache (replaces `ndk_context`)

This is the **carried blocker** from Plan 1: `JniSpeechBridge::new()` panics because `ndk_context::android_context()` is not populated for our call path under the Tauri-mobile runtime. The fix (documented in Plan 1 Risks) is a Rust `nativeInit` JNI export, called once from `MainActivity.onCreate`, caching the `JavaVM` in a `OnceLock`.

**Files:**
- Create: `src/crates/primer-speech/src/android/vm.rs`
- Modify: `src/crates/primer-speech/src/android/jni_bridge.rs`
- Modify: `src/crates/primer-speech/src/android/mod.rs`
- Modify: `src/crates/primer-speech/src/lib.rs`
- Modify: `src/crates/primer-gui/gen/android/app/src/main/java/org/theprimer/gui/PrimerSpeech.kt`
- Modify: `src/crates/primer-gui/gen/android/app/src/main/java/org/theprimer/gui/MainActivity.kt`

**Interfaces:**
- Produces: `primer_speech::android::vm::{set_java_vm(JavaVM), java_vm() -> Result<&'static JavaVM>}` (android-only real impl; host has the `java_vm()` error stub for the pure tests). `JniSpeechBridge::new()` consumes `java_vm()`.

- [ ] **Step 1: Write the failing host test for the VM-cache accessor's not-set path**

The cache itself wraps a `JavaVM` (android-only type), but the *contract* — "`java_vm()` before `set_java_vm` is an error, not a panic" — is host-testable via a tiny generic mirror. Create `src/crates/primer-speech/src/android/vm.rs`:

```rust
//! Process-wide `JavaVM` handle, populated once by the `nativeInit` JNI
//! export (called from `MainActivity.onCreate`) and read by every JNI
//! bridge. Replaces `ndk_context::android_context()`, which the
//! Tauri-mobile runtime does not populate for our call path (Plan 1's #1
//! risk, confirmed on-device).
//!
//! The cache is a `OnceLock`: set exactly once at startup, read for the
//! process lifetime. Reading before `nativeInit` ran is a recoverable
//! error (a clear "voice not initialised" message), never a panic — the
//! whole point of moving off `ndk_context`.

#[cfg(test)]
mod tests {
    /// The generic invariant the android `OnceLock<JavaVM>` relies on:
    /// reading an unset `OnceLock` yields `None` (→ our error), and a
    /// second `set` after the first is rejected. Pins the contract
    /// host-side without a real `JavaVM`.
    #[test]
    fn once_lock_is_set_once_and_empty_until_set() {
        let cell: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
        assert!(cell.get().is_none(), "unset cell must read None (→ error path)");
        assert!(cell.set(7).is_ok(), "first set succeeds");
        assert!(cell.set(9).is_err(), "second set is rejected");
        assert_eq!(*cell.get().unwrap(), 7, "value is the first set");
    }
}
```

- [ ] **Step 2: Run it, verify it passes (pins the OnceLock contract)**

Run: `~/.cargo/bin/cargo test -p primer-speech --features android-native android::vm::tests`
Expected: PASS. (This is a guard test for the semantics the android impl depends on; it compiles host-side because it uses no `JavaVM`.)

- [ ] **Step 3: Add the android-only VM cache impl**

Append to `vm.rs`:

```rust
#[cfg(target_os = "android")]
mod imp {
    use jni::JavaVM;
    use primer_core::error::{PrimerError, Result};
    use std::sync::OnceLock;

    static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();

    /// Cache the process `JavaVM`. Called once from the `nativeInit` JNI
    /// export. A second call is ignored (logged) — `OnceLock::set` after
    /// the first returns `Err`, which we swallow because re-init is benign.
    pub fn set_java_vm(vm: JavaVM) {
        if JAVA_VM.set(vm).is_err() {
            tracing::warn!(
                target: "primer::speech::android",
                "nativeInit called more than once; keeping the first JavaVM"
            );
        }
    }

    /// Borrow the cached `JavaVM`. Errors (does not panic) if `nativeInit`
    /// has not run yet.
    pub fn java_vm() -> Result<&'static JavaVM> {
        JAVA_VM.get().ok_or_else(|| {
            PrimerError::Speech(
                "android speech not initialised: nativeInit() has not run \
                 (MainActivity.onCreate must call PrimerSpeech.nativeInit)"
                    .into(),
            )
        })
    }
}

#[cfg(target_os = "android")]
pub use imp::{java_vm, set_java_vm};
```

- [ ] **Step 4: Switch `JniSpeechBridge::new()` to the cached VM**

In `jni_bridge.rs`, replace the `ndk_context` body of `new()`:

```rust
impl JniSpeechBridge {
    pub fn new() -> Result<Self> {
        // The JavaVM is cached by the `nativeInit` JNI export
        // (MainActivity.onCreate → PrimerSpeech.nativeInit). We no longer
        // touch ndk_context — it is not populated for our call path under
        // the Tauri-mobile runtime (Plan 1 gate finding).
        let vm = crate::android::vm::java_vm()?;
        // SAFETY: the cached JavaVM lives for the process lifetime; we copy
        // the underlying pointer into a new JavaVM handle (jni::JavaVM is a
        // thin wrapper over *mut JavaVM and is Copy-of-pointer cheap).
        let vm = unsafe { JavaVM::from_raw(vm.get_java_vm_pointer()) }.map_err(jerr)?;
        Ok(Self { vm })
    }
}
```

Remove the now-unused `ndk-context` dependency from `primer-speech/Cargo.toml` and `src/Cargo.toml` `[workspace.dependencies]` (it was Plan 1's bootstrap path; Task 1 retires it). Drop the `ndk_context` references.

- [ ] **Step 5: Add the `nativeInit` JNI export (android-gated)**

In `src/crates/primer-speech/src/lib.rs`, under the `android-native` feature, add:

```rust
/// JNI entry point cached at app startup. Kotlin declares this as
/// `external fun nativeInit()` on `PrimerSpeech` and calls it from
/// `MainActivity.onCreate`. We capture the `JavaVM` from the provided
/// `JNIEnv` and stash it for every later JNI bridge to reuse. This is the
/// fix for Plan 1's `ndk_context` blocker.
///
/// The symbol name MUST be `Java_<pkg>_<Class>_nativeInit` with `/`/`.`
/// replaced by `_`; here `org.theprimer.gui.PrimerSpeech` → the name
/// below. Mismatch = `UnsatisfiedLinkError` at the Kotlin call site.
#[cfg(all(target_os = "android", feature = "android-native"))]
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_theprimer_gui_PrimerSpeech_nativeInit(
    env: jni::JNIEnv,
    _class: jni::objects::JClass,
) {
    match env.get_java_vm() {
        Ok(vm) => {
            crate::android::vm::set_java_vm(vm);
            tracing::info!(target: "primer::speech::android", "nativeInit: JavaVM cached");
        }
        Err(e) => {
            tracing::error!(
                target: "primer::speech::android",
                "nativeInit: get_java_vm failed: {e}"
            );
        }
    }
}
```

Wire `mod vm;` into `android/mod.rs` (add `pub mod vm;`).

- [ ] **Step 6: Declare + call `nativeInit` from Kotlin**

In `PrimerSpeech.kt`, add the external declaration inside the `object`:

```kotlin
// Implemented in Rust (primer-speech, android-native) as
// Java_org_theprimer_gui_PrimerSpeech_nativeInit. Caches the JavaVM so
// the JNI speech bridge can resolve it without ndk_context.
@JvmStatic external fun nativeInit()
```

In `MainActivity.kt` `onCreate`, after `PrimerSpeech.init(this)` and after the Tauri/Rust native lib is loaded, add:

```kotlin
// Cache the JavaVM for the JNI speech bridge (Plan 2 Task 1). Must run
// after the Rust shared library is loaded (Tauri loads it during super
// .onCreate); the symbol is exported from primer-speech.
PrimerSpeech.nativeInit()
```

(If `System.loadLibrary` for the Tauri app lib is explicit in this project, ensure `nativeInit()` is called *after* it. Confirm against how the existing Tauri-mobile entry loads the Rust lib; match it.)

- [ ] **Step 7: Cross-compile + host build**

Run from `src/`:
```bash
~/.cargo/bin/cargo build -p primer-speech --features android-native
~/.cargo/bin/cargo build -p primer-speech --features android-native --target aarch64-linux-android
~/.cargo/bin/cargo test -p primer-speech --features android-native android::vm
```
Expected: all PASS.

- [ ] **Step 8: Commit**

```bash
git add src/crates/primer-speech/src/android/ src/crates/primer-speech/src/lib.rs \
  src/crates/primer-speech/Cargo.toml src/Cargo.toml \
  src/crates/primer-gui/gen/android/app/src/main/java/org/theprimer/gui/PrimerSpeech.kt \
  src/crates/primer-gui/gen/android/app/src/main/java/org/theprimer/gui/MainActivity.kt
git commit -m "fix(speech): cache JavaVM via nativeInit, retire ndk_context (android voice Plan 2 Task 1)"
```

> **Device check (deferred to Task 10, but unblocked here):** rebuild the APK and re-run `speech_capabilities` — it should now return the real JSON instead of panicking. This is the proof Task 1 worked; do it as the first sub-step of Task 10 since it needs the device.

---

### Task 2: Voice-loop event types + extended bridge trait

**Files:**
- Create: `src/crates/primer-speech/src/android/events.rs`
- Modify: `src/crates/primer-speech/src/android/bridge.rs`
- Modify: `src/crates/primer-speech/src/android/mod.rs`

**Interfaces:**
- Produces: `SpeechEvent` enum (serde-tagged, matching the Kotlin `pollSpeechEvent()` JSON); `AndroidSpeechBridge` extended with `start_listening(bcp47: &str)`, `stop_listening()`, `poll_event(timeout_ms: u32) -> Result<Option<SpeechEvent>>`, `speak(text: &str) -> Result<()>` (blocks until done), `cancel_speech()`. `MockBridge` implements all of them from scripted queues.

- [ ] **Step 1: Write the failing test (event JSON round-trip)**

`src/crates/primer-speech/src/android/events.rs`:

```rust
//! Recognizer / synthesizer events the Kotlin side enqueues and Rust
//! drains via `pollSpeechEvent()`. The serde tag/shape is the exact JSON
//! `PrimerSpeech.pollSpeechEvent()` emits.

use serde::{Deserialize, Serialize};

/// One event from the Android speech engines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SpeechEvent {
    /// A volatile partial transcript (`onPartialResults`).
    Partial { text: String },
    /// The final transcript for the utterance (`onResults`).
    Final { text: String },
    /// The recognizer detected end-of-speech (`onEndOfSpeech`).
    EndOfSpeech,
    /// Recognizer error (`onError`), carrying the Android error code.
    SttError { code: i32 },
    /// TTS finished speaking the current utterance (`onDone`).
    TtsDone,
    /// TTS error (`onError`).
    TtsError { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_partial_and_final_and_end() {
        let p: SpeechEvent =
            serde_json::from_str(r#"{"kind":"partial","text":"how do"}"#).unwrap();
        assert_eq!(p, SpeechEvent::Partial { text: "how do".into() });
        let f: SpeechEvent =
            serde_json::from_str(r#"{"kind":"final","text":"how do birds fly"}"#).unwrap();
        assert_eq!(f, SpeechEvent::Final { text: "how do birds fly".into() });
        let e: SpeechEvent = serde_json::from_str(r#"{"kind":"end_of_speech"}"#).unwrap();
        assert_eq!(e, SpeechEvent::EndOfSpeech);
    }

    #[test]
    fn parses_error_and_tts_events() {
        let s: SpeechEvent = serde_json::from_str(r#"{"kind":"stt_error","code":7}"#).unwrap();
        assert_eq!(s, SpeechEvent::SttError { code: 7 });
        let d: SpeechEvent = serde_json::from_str(r#"{"kind":"tts_done"}"#).unwrap();
        assert_eq!(d, SpeechEvent::TtsDone);
    }
}
```

- [ ] **Step 2: Run, verify PASS**

Run: `~/.cargo/bin/cargo test -p primer-speech --features android-native android::events`
Expected: PASS. Wire `pub mod events;` into `android/mod.rs`.

- [ ] **Step 3: Extend the bridge trait + MockBridge**

In `android/bridge.rs`, replace the trait with:

```rust
use crate::android::events::SpeechEvent;
use crate::android::SpeechCapabilities;
use primer_core::error::Result;

/// Everything the Android voice loop needs from the device. Real impl is
/// JNI (`JniSpeechBridge`, android-only); `MockBridge` (test) drives the
/// host-side logic. All calls are Rust→Kotlin (one direction) — events
/// flow back via `poll_event` (the D4 poll model), not Kotlin upcalls.
pub trait AndroidSpeechBridge: Send + Sync {
    fn query_capabilities(&self) -> Result<SpeechCapabilities>;
    /// Arm the on-device recognizer for one utterance in `bcp47`.
    fn start_listening(&self, bcp47: &str) -> Result<()>;
    /// Stop / cancel the recognizer.
    fn stop_listening(&self) -> Result<()>;
    /// Pull the next queued speech event, waiting up to `timeout_ms`.
    /// `Ok(None)` = no event within the timeout (caller loops).
    fn poll_event(&self, timeout_ms: u32) -> Result<Option<SpeechEvent>>;
    /// Speak `text` and BLOCK until the engine reports done (D3). The
    /// synthesis is the playback on Android.
    fn speak(&self, text: &str) -> Result<()>;
    /// Abort any in-progress speech (GUI Stop / Esc).
    fn cancel_speech(&self) -> Result<()>;
}
```

Update the `#[cfg(test)] mod tests` `MockBridge` to implement the full trait from scripted queues:

```rust
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::android::TtsVoiceInfo;
    use std::sync::Mutex;

    /// Scriptable bridge: `events` is drained by `poll_event`; `spoken`
    /// records every `speak`. Construct via `with_events`.
    pub struct MockBridge {
        pub caps: SpeechCapabilities,
        pub events: Mutex<std::collections::VecDeque<SpeechEvent>>,
        pub spoken: Mutex<Vec<String>>,
    }

    impl MockBridge {
        pub fn with_events(events: Vec<SpeechEvent>) -> Self {
            Self {
                caps: SpeechCapabilities {
                    on_device_recognition_available: true,
                    recognition_locales: vec![],
                    tts_voices: vec![TtsVoiceInfo {
                        name: "offline".into(),
                        locale: "en-US".into(),
                        network_required: false,
                        not_installed: false,
                    }],
                },
                events: Mutex::new(events.into()),
                spoken: Mutex::new(vec![]),
            }
        }
    }

    impl AndroidSpeechBridge for MockBridge {
        fn query_capabilities(&self) -> Result<SpeechCapabilities> {
            Ok(self.caps.clone())
        }
        fn start_listening(&self, _bcp47: &str) -> Result<()> {
            Ok(())
        }
        fn stop_listening(&self) -> Result<()> {
            Ok(())
        }
        fn poll_event(&self, _timeout_ms: u32) -> Result<Option<SpeechEvent>> {
            Ok(self.events.lock().unwrap().pop_front())
        }
        fn speak(&self, text: &str) -> Result<()> {
            self.spoken.lock().unwrap().push(text.to_string());
            Ok(())
        }
        fn cancel_speech(&self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn mock_bridge_polls_scripted_events_in_order() {
        let bridge = MockBridge::with_events(vec![
            SpeechEvent::Partial { text: "how".into() },
            SpeechEvent::Final { text: "how do birds fly".into() },
            SpeechEvent::EndOfSpeech,
        ]);
        assert_eq!(
            bridge.poll_event(0).unwrap(),
            Some(SpeechEvent::Partial { text: "how".into() })
        );
        assert_eq!(
            bridge.poll_event(0).unwrap(),
            Some(SpeechEvent::Final { text: "how do birds fly".into() })
        );
        assert_eq!(bridge.poll_event(0).unwrap(), Some(SpeechEvent::EndOfSpeech));
        assert_eq!(bridge.poll_event(0).unwrap(), None);
    }
}
```

- [ ] **Step 4: Run, verify PASS; cross-compile**

Run:
```bash
~/.cargo/bin/cargo test -p primer-speech --features android-native android::bridge
~/.cargo/bin/cargo build -p primer-speech --features android-native --target aarch64-linux-android
```
Expected: host test PASS; cross-compile PASS (the android `JniSpeechBridge` now fails to compile — it only implements `query_capabilities`. That's expected; Task 7 fills the new methods. To keep the tree green between tasks, add `#[allow(unused)]` stubs returning `Err(jerr("not yet implemented (Task 7)"))` for the five new methods on `JniSpeechBridge` now, replaced in Task 7).

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-speech/src/android/
git commit -m "feat(speech): android voice event types + extended bridge trait (Plan 2 Task 2)"
```

---

### Task 3: Android derived-VAD state machine

**Files:**
- Create: `src/crates/primer-speech/src/android/vad.rs`
- Modify: `src/crates/primer-core/src/consts.rs`
- Modify: `src/crates/primer-speech/src/android/mod.rs`

**Interfaces:**
- Produces: `AndroidDerivedVad` with `on_event(&mut self, &SpeechEvent) -> Option<VadEvent>` and `reset(&mut self)`. SpeechStart on the first non-empty `Partial`/`Final`; SpeechEnd on `EndOfSpeech` or `Final` (whichever the engine emits) while in-speech.

> **DRY follow-up (noted, not done now):** this mirrors `macos26/vad.rs`. The two are kept separate because `SpeechRecognizer` (discrete `onEndOfSpeech`) and `SpeechAnalyzer` (continuous partials + inactivity timer) have different end-of-utterance signals. Once both are device-tuned, hoist a shared pure state machine to `voice_loop/derived_vad.rs` if the logic converges. YAGNI until then.

- [ ] **Step 1: Add the consts**

In `primer-core/src/consts.rs`, in the `speech` module, add:

```rust
/// Android on-device `SpeechRecognizer` derived-VAD tunables. The
/// recognizer endpoints itself (`onEndOfSpeech`), so unlike macos26 we do
/// not need an inactivity timer in the common case — but we keep a guard
/// timeout for engines that emit `onResults` without a preceding
/// `onEndOfSpeech`.
pub mod android {
    use std::time::Duration;

    /// Minimum trimmed characters in a partial to treat as speech onset.
    pub const SPEECH_START_MIN_TEXT_CHARS: usize = 1;

    /// How long the recognizer consumer waits per `poll_event` call before
    /// looping (lets `stop`/`cancel` be observed promptly).
    pub const POLL_TIMEOUT: Duration = Duration::from_millis(100);
}
```

- [ ] **Step 2: Write the failing tests**

`src/crates/primer-speech/src/android/vad.rs`:

```rust
//! Derived VAD for the Android on-device `SpeechRecognizer`. The
//! recognizer owns the mic and endpoints itself; we synthesise the
//! `VadEvent::SpeechStart`/`SpeechEnd` pair the voice-loop state machine
//! expects from the recognizer's discrete callbacks.

use crate::android::events::SpeechEvent;
use primer_core::consts::speech::android::SPEECH_START_MIN_TEXT_CHARS;
use primer_core::speech::VadEvent;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_nonempty_partial_starts_speech() {
        let mut vad = AndroidDerivedVad::new();
        assert_eq!(vad.on_event(&SpeechEvent::Partial { text: "".into() }), None);
        assert_eq!(
            vad.on_event(&SpeechEvent::Partial { text: "how".into() }),
            Some(VadEvent::SpeechStart)
        );
        // Subsequent partials do not re-fire SpeechStart.
        assert_eq!(
            vad.on_event(&SpeechEvent::Partial { text: "how do".into() }),
            None
        );
    }

    #[test]
    fn end_of_speech_ends_when_in_speech() {
        let mut vad = AndroidDerivedVad::new();
        vad.on_event(&SpeechEvent::Partial { text: "hi".into() });
        assert_eq!(vad.on_event(&SpeechEvent::EndOfSpeech), Some(VadEvent::SpeechEnd));
    }

    #[test]
    fn final_without_prior_end_still_ends() {
        let mut vad = AndroidDerivedVad::new();
        vad.on_event(&SpeechEvent::Partial { text: "hi".into() });
        assert_eq!(
            vad.on_event(&SpeechEvent::Final { text: "hi there".into() }),
            Some(VadEvent::SpeechEnd)
        );
    }

    #[test]
    fn end_before_start_is_ignored() {
        let mut vad = AndroidDerivedVad::new();
        assert_eq!(vad.on_event(&SpeechEvent::EndOfSpeech), None);
    }

    #[test]
    fn final_can_also_start_then_end_idempotently() {
        // A Final arriving with no prior partial both starts and ends.
        let mut vad = AndroidDerivedVad::new();
        // First Final: starts speech (non-empty) → SpeechStart, latches end.
        // We model this as SpeechStart on the start edge; the consumer
        // emits SpeechEnd via the same Final by re-querying. To keep the
        // VAD single-edge-per-call, a Final with no prior speech returns
        // SpeechStart and sets a pending-end the consumer drains via
        // `take_pending_end`.
        assert_eq!(
            vad.on_event(&SpeechEvent::Final { text: "yes".into() }),
            Some(VadEvent::SpeechStart)
        );
        assert_eq!(vad.take_pending_end(), Some(VadEvent::SpeechEnd));
        assert_eq!(vad.take_pending_end(), None);
    }
}
```

- [ ] **Step 3: Implement**

```rust
/// Two-state derived VAD. `Idle` until a non-empty transcript arrives
/// (→ `SpeechStart`); `Speaking` until `EndOfSpeech`/`Final` (→
/// `SpeechEnd`). A `Final` arriving from `Idle` both starts and ends: it
/// returns `SpeechStart` and stashes a pending `SpeechEnd` the consumer
/// drains with `take_pending_end` (keeps one edge per `on_event`).
#[derive(Debug, Default)]
pub struct AndroidDerivedVad {
    in_speech: bool,
    pending_end: bool,
}

impl AndroidDerivedVad {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.in_speech = false;
        self.pending_end = false;
    }

    fn is_onset(text: &str) -> bool {
        text.trim().chars().count() >= SPEECH_START_MIN_TEXT_CHARS
    }

    /// Feed a recognizer event; returns at most one `VadEvent` edge.
    pub fn on_event(&mut self, event: &SpeechEvent) -> Option<VadEvent> {
        match event {
            SpeechEvent::Partial { text } if !self.in_speech && Self::is_onset(text) => {
                self.in_speech = true;
                Some(VadEvent::SpeechStart)
            }
            SpeechEvent::Final { text } if !self.in_speech && Self::is_onset(text) => {
                self.in_speech = true;
                self.pending_end = true;
                Some(VadEvent::SpeechStart)
            }
            SpeechEvent::EndOfSpeech | SpeechEvent::Final { .. } if self.in_speech => {
                self.in_speech = false;
                Some(VadEvent::SpeechEnd)
            }
            _ => None,
        }
    }

    /// Drain a stashed end edge (set when a `Final` started speech from
    /// `Idle`). Returns `Some(SpeechEnd)` exactly once after such a Final.
    pub fn take_pending_end(&mut self) -> Option<VadEvent> {
        if self.pending_end {
            self.pending_end = false;
            self.in_speech = false;
            Some(VadEvent::SpeechEnd)
        } else {
            None
        }
    }
}
```

- [ ] **Step 4: Run, verify PASS**

Run: `~/.cargo/bin/cargo test -p primer-speech --features android-native android::vad`
Expected: all 5 PASS. Wire `pub mod vad;` into `android/mod.rs`.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-speech/src/android/vad.rs \
  src/crates/primer-core/src/consts.rs src/crates/primer-speech/src/android/mod.rs
git commit -m "feat(speech): android derived-VAD state machine (Plan 2 Task 3)"
```

---

### Task 4: Un-gate `ChannelStt` from cpal

`ChannelStt` is pure `std::sync::mpsc` channel code (it only forwards `String` transcripts as `TranscriptSegment`s); it has no cpal dependency and only lives in the cpal-gated `backends_common`. Move it so the cpal-free Android STT can reuse it (DRY — D2).

**Files:**
- Create: `src/crates/primer-speech/src/voice_loop/channel_stt.rs`
- Modify: `src/crates/primer-speech/src/voice_loop/backends_common/mod.rs`
- Modify: `src/crates/primer-speech/src/voice_loop/mod.rs`

**Interfaces:**
- Produces: `voice_loop::channel_stt::ChannelStt { rx: Arc<Mutex<std::sync::mpsc::Receiver<String>>> }` (cpal-free) — re-exported as `voice_loop::ChannelStt` (unchanged public path).

- [ ] **Step 1: Move the type (cut from `backends_common/mod.rs`)**

Cut the `ChannelStt` struct + its `Named` + `StreamingSpeechToText` impls + the `ChannelSttSession` impl (backends_common/mod.rs:84-130 region) into a new `channel_stt.rs`:

```rust
//! Channel-backed `StreamingSpeechToText` adapter. Decouples whatever
//! produces final transcripts (a cpal+whisper audio thread, the macOS-26
//! Swift pipeline, or the Android `SpeechRecognizer` consumer) from the
//! voice-loop state machine, which only needs `finalize()` to yield the
//! next utterance. Pure `std::sync::mpsc` — no cpal — so it is reachable
//! from the cpal-free `android-native` build.
//!
//! Moved out of `backends_common` (which is cpal-gated) in Plan 2 Task 4.

use std::sync::{Arc, Mutex};

use primer_core::error::{PrimerError, Result};
use primer_core::speech::{
    Named, StreamingSpeechToText, TranscriptSegment, TranscriptionSession,
};

// ... (verbatim ChannelStt + ChannelSttSession bodies from backends_common) ...
```

(Copy the exact existing bodies — `ChannelStt`, `impl Named`, `impl StreamingSpeechToText`, `struct ChannelSttSession`, `impl TranscriptionSession` including the "transcript receiver mutex poisoned" error string at backends_common/mod.rs:125.)

- [ ] **Step 2: Re-export from both old and new paths**

In `voice_loop/mod.rs`:

```rust
/// Channel-backed STT adapter — cpal-free, reachable from android-native.
pub mod channel_stt;
pub use channel_stt::ChannelStt;
```

Remove the `ChannelStt` from the `#[cfg(feature = "cpal")] pub use backends_common::{ChannelStt, LocalBackends};` line — keep `LocalBackends` there, drop `ChannelStt` (now exported above, unconditionally). In `backends_common/mod.rs`, add `pub use crate::voice_loop::channel_stt::ChannelStt;` so the builders that write `use ...backends_common::ChannelStt` keep compiling unchanged.

- [ ] **Step 3: Verify every cpal build still compiles + tests pass**

Run from `src/`:
```bash
~/.cargo/bin/cargo build -p primer-speech --features silero,whisper,piper,cpal
~/.cargo/bin/cargo test -p primer-speech --features silero,whisper,piper,cpal voice_loop
~/.cargo/bin/cargo build -p primer-speech --features android-native   # channel_stt now reachable, no cpal
```
Expected: all PASS (the macOS builders are not exercised on Linux/CI; the cpal feature combo is). On macOS additionally run the macos-native-26 build to confirm the `ChannelStt` import path still resolves.

- [ ] **Step 4: Commit**

```bash
git add src/crates/primer-speech/src/voice_loop/
git commit -m "refactor(speech): move ChannelStt out of cpal-gated backends_common (Plan 2 Task 4)"
```

---

### Task 5: `AndroidTts` — blocking `StreamingTextToSpeech`

**Files:**
- Create: `src/crates/primer-speech/src/android/tts.rs`
- Modify: `src/crates/primer-speech/src/android/mod.rs`

**Interfaces:**
- Consumes: `Arc<dyn AndroidSpeechBridge>`, `select_offline_voice` (Plan 1).
- Produces: `AndroidTts { bridge, sample_rate }` implementing `StreamingTextToSpeech`; its `SynthesisSession::push_text` calls `bridge.speak(text)` (blocks until `onDone`) and emits **no** `SynthesisEvent::Audio` (D3). `name()` = `"android-tts"`.

- [ ] **Step 1: Write the failing test (push_text routes to bridge.speak, no audio events)**

`src/crates/primer-speech/src/android/tts.rs`:

```rust
//! `StreamingTextToSpeech` over Android `TextToSpeech`. The OS plays the
//! audio itself, so `push_text` calls `bridge.speak` (which blocks until
//! the engine's `onDone`) and emits NO `SynthesisEvent::Audio` — the
//! voice loop's `on_committed_audio` stays a no-op on android (D3).

use std::sync::Arc;

use primer_core::error::Result;
use primer_core::speech::{
    Named, StreamingTextToSpeech, SynthesisEvent, SynthesisSession, VoiceProfile,
};

use crate::android::bridge::AndroidSpeechBridge;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::android::bridge::tests::MockBridge;

    #[test]
    fn push_text_speaks_via_bridge_and_emits_no_audio() {
        let bridge = Arc::new(MockBridge::with_events(vec![]));
        let tts = AndroidTts::new(Arc::clone(&bridge) as Arc<dyn AndroidSpeechBridge>);
        let mut session = tts.open_session(&VoiceProfile::default()).unwrap();
        let mut audio_events = 0u32;
        session
            .push_text("Why do you think birds have feathers?", &mut |e| {
                if let SynthesisEvent::Audio(_) = e {
                    audio_events += 1;
                }
            })
            .unwrap();
        assert_eq!(audio_events, 0, "android TTS plays itself; no PCM events");
        assert_eq!(
            bridge.spoken.lock().unwrap().as_slice(),
            ["Why do you think birds have feathers?"]
        );
    }

    #[test]
    fn empty_text_is_a_noop() {
        let bridge = Arc::new(MockBridge::with_events(vec![]));
        let tts = AndroidTts::new(Arc::clone(&bridge) as Arc<dyn AndroidSpeechBridge>);
        let mut session = tts.open_session(&VoiceProfile::default()).unwrap();
        session.push_text("", &mut |_| {}).unwrap();
        assert!(bridge.spoken.lock().unwrap().is_empty());
    }
}
```

- [ ] **Step 2: Run, verify FAIL** — `AndroidTts` not defined.

Run: `~/.cargo/bin/cargo test -p primer-speech --features android-native android::tts`

- [ ] **Step 3: Implement**

```rust
/// Nominal sample rate reported to the loop. Android plays audio itself,
/// so this never drives a resampler on android; it exists only to satisfy
/// `StreamingTextToSpeech::sample_rate`. 22_050 mirrors the Piper-class
/// default used elsewhere.
const ANDROID_TTS_NOMINAL_RATE: u32 = 22_050;

pub struct AndroidTts {
    bridge: Arc<dyn AndroidSpeechBridge>,
}

impl AndroidTts {
    pub fn new(bridge: Arc<dyn AndroidSpeechBridge>) -> Self {
        Self { bridge }
    }
}

impl Named for AndroidTts {
    fn name(&self) -> &str {
        "android-tts"
    }
}

impl StreamingTextToSpeech for AndroidTts {
    fn sample_rate(&self) -> u32 {
        ANDROID_TTS_NOMINAL_RATE
    }
    fn open_session(&self, _voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>> {
        Ok(Box::new(AndroidTtsSession {
            bridge: Arc::clone(&self.bridge),
        }))
    }
}

struct AndroidTtsSession {
    bridge: Arc<dyn AndroidSpeechBridge>,
}

impl SynthesisSession for AndroidTtsSession {
    fn push_text(
        &mut self,
        text: &str,
        _on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()> {
        if text.trim().is_empty() {
            return Ok(());
        }
        // Blocks until the Android engine reports onDone (D3). No PCM is
        // surfaced — the OS already played it.
        self.bridge.speak(text)
    }

    fn finalize(self: Box<Self>, _on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
        Ok(())
    }
}
```

- [ ] **Step 4: Run, verify PASS**

Run: `~/.cargo/bin/cargo test -p primer-speech --features android-native android::tts`
Expected: both PASS. Wire `pub mod tts;` into `android/mod.rs` and `pub use bridge::tests` visibility — make `MockBridge` reachable from sibling test modules by declaring `bridge::tests` as `pub(crate)` (already done in Task 2 Step 3).

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-speech/src/android/
git commit -m "feat(speech): AndroidTts blocking StreamingTextToSpeech (Plan 2 Task 5)"
```

---

### Task 6: `AndroidStt` + recognizer consumer + `build_android_voice_backends`

**Files:**
- Create: `src/crates/primer-speech/src/android/stt.rs`
- Create: `src/crates/primer-speech/src/voice_loop/backends_android_native.rs`
- Modify: `src/crates/primer-speech/src/android/mod.rs`
- Modify: `src/crates/primer-speech/src/voice_loop/mod.rs`

**Interfaces:**
- Consumes: `AndroidDerivedVad`, `ChannelStt`, `AndroidTts`, `Arc<dyn AndroidSpeechBridge>`, `LoopBackends`.
- Produces: `run_recognizer_loop(bridge, bcp47, event_tx, transcript_tx, stop)` — drains `poll_event`, drives the VAD, forwards `VadEvent`s + final transcripts; `AndroidVoiceBackends { backends: LoopBackends, event_rx, stop }`; `build_android_voice_backends(bridge, locale, voice) -> Result<AndroidVoiceBackends>`.

- [ ] **Step 1: Write the failing test (consumer drives VAD + transcript from scripted events)**

The consumer's pure core is `process_event(event, &mut vad, &event_tx, &transcript_tx)`. Test it directly (sync, no tokio) in `android/stt.rs`:

```rust
//! Android recognizer → voice-loop adapter. `process_event` is the pure
//! per-event step (host-tested); `run_recognizer_loop` is the async
//! driver that polls the bridge and calls it.

use crate::android::bridge::AndroidSpeechBridge;
use crate::android::events::SpeechEvent;
use crate::android::vad::AndroidDerivedVad;
use primer_core::error::Result;
use primer_core::speech::VadEvent;
use std::sync::Arc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_event_emits_start_then_transcript_then_end() {
        let mut vad = AndroidDerivedVad::new();
        let (event_tx, event_rx) = std::sync::mpsc::channel::<VadEvent>();
        let (txt_tx, txt_rx) = std::sync::mpsc::channel::<String>();

        process_event(
            &SpeechEvent::Partial { text: "how".into() },
            &mut vad,
            &event_tx,
            &txt_tx,
        );
        process_event(
            &SpeechEvent::Final { text: "how do birds fly".into() },
            &mut vad,
            &event_tx,
            &txt_tx,
        );
        process_event(&SpeechEvent::EndOfSpeech, &mut vad, &event_tx, &txt_tx);

        // VAD: SpeechStart (from partial), SpeechEnd (from Final).
        assert_eq!(event_rx.recv().unwrap(), VadEvent::SpeechStart);
        assert_eq!(event_rx.recv().unwrap(), VadEvent::SpeechEnd);
        // Transcript: the Final text, forwarded once.
        assert_eq!(txt_rx.recv().unwrap(), "how do birds fly");
        assert!(txt_rx.try_recv().is_err(), "no extra transcripts");
    }

    #[test]
    fn stt_error_does_not_emit_transcript() {
        let mut vad = AndroidDerivedVad::new();
        let (event_tx, _event_rx) = std::sync::mpsc::channel::<VadEvent>();
        let (txt_tx, txt_rx) = std::sync::mpsc::channel::<String>();
        process_event(&SpeechEvent::SttError { code: 7 }, &mut vad, &event_tx, &txt_tx);
        assert!(txt_rx.try_recv().is_err());
    }
}
```

- [ ] **Step 2: Run, verify FAIL** — `process_event` not defined.

- [ ] **Step 3: Implement `process_event` + the async driver + `AndroidStt`**

```rust
/// One recognizer event → (VadEvent edges, final transcript). Pure: takes
/// the channels by ref so it is host-testable without tokio. The final
/// transcript is forwarded on `Final` only — partials are volatile and
/// the voice loop only needs the committed utterance (the macos26
/// ChannelStt-bridge policy).
pub fn process_event(
    event: &SpeechEvent,
    vad: &mut AndroidDerivedVad,
    event_tx: &std::sync::mpsc::Sender<VadEvent>,
    transcript_tx: &std::sync::mpsc::Sender<String>,
) {
    if let SpeechEvent::Final { text } = event {
        // Forward the committed transcript BEFORE the SpeechEnd edge so the
        // loop's ChannelStt has it ready when it transitions out of LISTEN
        // (mirrors the macos26 "text before event" ordering contract).
        let _ = transcript_tx.send(text.clone());
    }
    if let Some(edge) = vad.on_event(event) {
        let _ = event_tx.send(edge);
        if let Some(end) = vad.take_pending_end() {
            let _ = event_tx.send(end);
        }
    }
}
```

Then the async driver (android-reachable; uses only `Arc<dyn AndroidSpeechBridge>` + std channels + tokio for the stop signal):

```rust
/// Poll the bridge for recognizer events, driving the derived VAD and
/// forwarding edges + transcripts, until `stop` fires or the bridge errors.
/// Re-arms `start_listening` after each utterance (the recognizer is
/// one-shot per `startListening`).
pub async fn run_recognizer_loop(
    bridge: Arc<dyn AndroidSpeechBridge>,
    bcp47: String,
    event_tx: std::sync::mpsc::Sender<VadEvent>,
    transcript_tx: std::sync::mpsc::Sender<String>,
    mut stop: tokio::sync::oneshot::Receiver<()>,
) -> Result<()> {
    use primer_core::consts::speech::android::POLL_TIMEOUT;
    let timeout_ms = POLL_TIMEOUT.as_millis() as u32;
    let mut vad = AndroidDerivedVad::new();
    bridge.start_listening(&bcp47)?;
    loop {
        if stop.try_recv().is_ok() {
            let _ = bridge.stop_listening();
            return Ok(());
        }
        // poll_event blocks up to timeout_ms inside Kotlin; wrap in
        // spawn_blocking so the tokio worker is not held for the wait.
        let bridge_poll = Arc::clone(&bridge);
        let polled =
            tokio::task::spawn_blocking(move || bridge_poll.poll_event(timeout_ms))
                .await
                .map_err(|e| primer_core::error::PrimerError::Speech(format!("poll join: {e}")))??;
        let Some(event) = polled else { continue };
        let was_end = matches!(event, SpeechEvent::EndOfSpeech | SpeechEvent::Final { .. });
        process_event(&event, &mut vad, &event_tx, &transcript_tx);
        if was_end {
            // Re-arm for the next utterance (one-shot recognizer).
            vad.reset();
            bridge.start_listening(&bcp47)?;
        }
    }
}
```

`AndroidStt` is simply a `ChannelStt` wrapper — the builder constructs `ChannelStt` directly, so `AndroidStt` need not be a distinct type. (If a `name()` of `"android-stt"` is wanted over `ChannelStt`'s, add a thin newtype; otherwise reuse `ChannelStt`.) Keep it as `ChannelStt` for DRY.

- [ ] **Step 4: Run, verify PASS**

Run: `~/.cargo/bin/cargo test -p primer-speech --features android-native android::stt`
Expected: both PASS. Wire `pub mod stt;` into `android/mod.rs`.

- [ ] **Step 5: Write the builder**

`src/crates/primer-speech/src/voice_loop/backends_android_native.rs`:

```rust
//! Android-native voice backend builder. Unlike the cpal builders, the OS
//! owns the mic (SpeechRecognizer) and the speaker (TextToSpeech), so
//! there is no audio thread, no mic/speaker ringbuf, and no `on_audio` /
//! drain machinery — the GUI passes a no-op `on_committed_audio`,
//! `wait_for_speaker_drain = None`, and `is_speaking = None` to `run_loop`.

#![cfg(feature = "android-native")]

use std::sync::Arc;

use primer_core::error::Result;
use primer_core::i18n::Locale;
use primer_core::speech::{VadEvent, VoiceProfile};

use crate::android::bridge::AndroidSpeechBridge;
use crate::android::stt::run_recognizer_loop;
use crate::android::tts::AndroidTts;
use crate::voice_loop::channel_stt::ChannelStt;
use crate::voice_loop::{LoopBackends, VAD_EVENT_CHANNEL_CAPACITY};

/// Cpal-free Android backend bundle. The GUI extracts `backends` +
/// `event_rx` for `run_loop`; `stop` ends the recognizer consumer task
/// when voice mode is turned off.
pub struct AndroidVoiceBackends {
    pub backends: LoopBackends,
    pub event_rx: tokio::sync::mpsc::Receiver<VadEvent>,
    pub stop: tokio::sync::oneshot::Sender<()>,
}

/// Map the loop's BCP-47 for the recognizer + TTS from the active locale.
fn bcp47_for(locale: Locale) -> String {
    // The android POC is en-only (spec scope); en → en-US. de fallback
    // would route to Whisper (deferred). Use pack_id → BCP-47.
    match locale.pack_id() {
        "de" => "de-DE".to_string(),
        _ => "en-US".to_string(),
    }
}

/// Build the Android voice backends: a `ChannelStt` fed by the recognizer
/// consumer task, an `AndroidTts`, and the `VadEvent` channel.
pub fn build_android_voice_backends(
    bridge: Arc<dyn AndroidSpeechBridge>,
    locale: Locale,
    voice: VoiceProfile,
) -> Result<AndroidVoiceBackends> {
    let bcp47 = bcp47_for(locale);

    // tokio mpsc: consumer → voice loop (VadEvent).
    let (event_tx_tok, event_rx) =
        tokio::sync::mpsc::channel::<VadEvent>(VAD_EVENT_CHANNEL_CAPACITY);
    // std mpsc: consumer → ChannelStt (final transcripts).
    let (transcript_tx, transcript_rx) = std::sync::mpsc::channel::<String>();
    // The recognizer consumer uses a std Sender for VadEvent; bridge it to
    // the tokio channel the loop reads.
    let (event_tx_std, event_rx_std) = std::sync::mpsc::channel::<VadEvent>();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();

    // Forward std VadEvents → tokio channel (the loop awaits tokio recv).
    tokio::spawn(async move {
        while let Ok(evt) = event_rx_std.recv() {
            if event_tx_tok.send(evt).await.is_err() {
                break;
            }
        }
    });

    // Recognizer consumer task.
    let bridge_consumer = Arc::clone(&bridge);
    tokio::spawn(async move {
        if let Err(e) =
            run_recognizer_loop(bridge_consumer, bcp47, event_tx_std, transcript_tx, stop_rx)
                .await
        {
            tracing::warn!(target: "primer::speech::android", "recognizer loop ended: {e}");
        }
    });

    let stt = Arc::new(ChannelStt {
        rx: Arc::new(std::sync::Mutex::new(transcript_rx)),
    });
    let tts = Arc::new(AndroidTts::new(bridge));
    let backends = LoopBackends::single_locale(stt, tts, voice, locale);
    backends.ensure_active_locale_coverage()?;

    Ok(AndroidVoiceBackends {
        backends,
        event_rx,
        stop: stop_tx,
    })
}
```

> **Note on the std→tokio event bridge:** `run_recognizer_loop` uses a std `Sender<VadEvent>` (so its pure `process_event` core is host-testable without tokio). The extra forwarder task converts those to the tokio `Receiver` `run_loop` awaits. If preferred, change `run_recognizer_loop` to take a `tokio::sync::mpsc::Sender` directly and drop the forwarder — but keep `process_event` taking std channels so the unit tests stay tokio-free. (Decide during implementation; both compile. The forwarder is the lower-risk default.)

Declare the module in `voice_loop/mod.rs`:

```rust
/// Android-native voice backend builder (OS-owned mic + speaker).
#[cfg(feature = "android-native")]
pub mod backends_android_native;
```

- [ ] **Step 6: Build host + cross-compile**

```bash
~/.cargo/bin/cargo build -p primer-speech --features android-native
~/.cargo/bin/cargo build -p primer-speech --features android-native --target aarch64-linux-android
~/.cargo/bin/cargo test -p primer-speech --features android-native
```
Expected: all PASS.

- [ ] **Step 7: Commit**

```bash
git add src/crates/primer-speech/src/android/stt.rs \
  src/crates/primer-speech/src/voice_loop/backends_android_native.rs \
  src/crates/primer-speech/src/android/mod.rs src/crates/primer-speech/src/voice_loop/mod.rs
git commit -m "feat(speech): android STT consumer + voice-backend builder (Plan 2 Task 6)"
```

---

### Task 7: Real JNI bridge methods + Kotlin recognizer/TTS plugin (device-only)

The host-testable seam is done; this fills the device halves. No host TDD here (JNI/Kotlin are device-verified, like Plan 1's `jni_bridge`); the code is exact, not sketched.

**Files:**
- Modify: `src/crates/primer-speech/src/android/jni_bridge.rs`
- Modify: `src/crates/primer-gui/gen/android/app/src/main/java/org/theprimer/gui/PrimerSpeech.kt`

**Interfaces:**
- Produces: `JniSpeechBridge` implementing all six `AndroidSpeechBridge` methods over `jni`; `PrimerSpeech` Kotlin object owning the recognizer + synthesizer + a thread-safe event queue.

- [ ] **Step 1: Kotlin — recognizer + TTS + event queue**

Extend `PrimerSpeech.kt`. Add a `LinkedBlockingQueue<String>` of event JSON, a persistent `SpeechRecognizer` + `RecognitionListener` (created on the main Looper), a persistent `TextToSpeech` with an `UtteranceProgressListener`, and the methods `startListening(bcp47)`, `stopListening()`, `pollSpeechEvent(timeoutMs)`, `speak(text)` (blocking on a per-utterance `CountDownLatch` released by `onDone`/`onError`), `cancelSpeech()`. Each `RecognitionListener` callback enqueues a `SpeechEvent` JSON string matching Task 2's serde shape (e.g. `onPartialResults` → `{"kind":"partial","text":...}`; `onResults` → `{"kind":"final","text":...}`; `onEndOfSpeech` → `{"kind":"end_of_speech"}`; `onError(code)` → `{"kind":"stt_error","code":code}`). Use `SpeechRecognizer.createOnDeviceSpeechRecognizer(ctx)` (offline-first; never the network factory). Recognizer + synthesizer construction must run on the main Looper (`Handler(Looper.getMainLooper()).post { ... }` with a latch when called from the JNI thread). Build the `RecognizerIntent` with `EXTRA_LANGUAGE = bcp47`, `EXTRA_PREFER_OFFLINE = true`, `EXTRA_PARTIAL_RESULTS = true`. Pick the offline voice via the Plan 1 selection result passed down (or re-query voices and apply `network_required == false`).

(Full Kotlin is written at implementation time matching these exact event JSON shapes; it is device-verified in Task 10. Keep `TTS_INIT_TIMEOUT_SECONDS` and any new bounds as named consts per `[[feedback_no_magic_numbers]]`.)

- [ ] **Step 2: Rust — implement the six bridge methods over JNI**

In `jni_bridge.rs`, replace the Task-2 `not yet implemented` stubs with real `call_static_method` invocations mirroring `query_capabilities`'s pattern (attach thread → `find_class("org/theprimer/gui/PrimerSpeech")` → call). `poll_event` calls `pollSpeechEvent(I)Ljava/lang/String;`, returns `Ok(None)` on an empty string, else `serde_json::from_str`. `start_listening`/`stop_listening`/`speak`/`cancel_speech` call the matching `(Ljava/lang/String;)V` / `()V` methods. `speak` blocks because the Kotlin side blocks until `onDone`.

> **Classloader fallback (Plan 1 risk #2):** if `find_class("org/theprimer/gui/PrimerSpeech")` fails on the attached thread (system classloader can't see app classes), resolve via the cached app `Context.getClassLoader().loadClass("org.theprimer.gui.PrimerSpeech")` instead. The cached `Context` is in `PrimerSpeech.appContext` (Plan 1). Task 10 reveals which path the Tauri runtime needs.

- [ ] **Step 3: Cross-compile**

```bash
~/.cargo/bin/cargo build -p primer-speech --features android-native --target aarch64-linux-android
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/crates/primer-speech/src/android/jni_bridge.rs \
  src/crates/primer-gui/gen/android/app/src/main/java/org/theprimer/gui/PrimerSpeech.kt
git commit -m "feat(speech): real android JNI recognizer/TTS bridge + Kotlin plugin (Plan 2 Task 7)"
```

---

### Task 8: GUI Android voice commands + mic permission

**Files:**
- Create: `src/crates/primer-gui/src/voice/backends_android.rs`
- Create: `src/crates/primer-gui/src/commands/voice_android.rs`
- Modify: `src/crates/primer-gui/src/voice/mod.rs`, `src/crates/primer-gui/src/commands/mod.rs`
- Modify: `src/crates/primer-gui/gen/android/app/src/main/java/org/theprimer/gui/MainActivity.kt` (runtime `RECORD_AUDIO` request)
- Modify: `src/crates/primer-gui/ui/voice.js` (invoke android commands on android)

**Interfaces:**
- Consumes: `build_android_voice_backends`, the GUI's existing `run_loop` + observer (`TauriEventObserver`) + responder wiring from `commands/voice.rs`.
- Produces: `start_voice_mode_android` / `stop_voice_mode_android` / `cancel_voice_response_android` Tauri commands (registered under `feature = "android-native"`), emitting the existing `primer://voice/*` events.

- [ ] **Step 1: GUI backend constructor**

`voice/backends_android.rs`: build `Arc<dyn AndroidSpeechBridge>` (the real `JniSpeechBridge` via `primer_speech::android::...`), resolve the active locale + a `VoiceProfile` (model_id can be the selected offline voice name or `VoiceProfile::default()` — AndroidTts ignores it beyond locale), call `primer_speech::voice_loop::backends_android_native::build_android_voice_backends`, and return its `AndroidVoiceBackends`.

- [ ] **Step 2: The command** mirrors `start_voice_mode` (commands/voice.rs:134-280) but: builds `AndroidVoiceBackends` instead of `LocalBackends`; calls `run_loop(backends, event_rx, responder, /* no-op */ Box::new(|_| {}), /* drain */ None, verbose, /* is_speaking */ None, observer)`; stores `stop` for `stop_voice_mode_android`. Reuse the existing `TauriEventObserver` + `DialogueResponder` so the `primer://voice/*` events and chat journaling are identical to desktop.

- [ ] **Step 3: Mic runtime permission** — in `MainActivity.onCreate`, request `RECORD_AUDIO` via `ActivityCompat.requestPermissions` if not granted (the manifest permission was added in Plan 1 Task 5). The recognizer needs it; request before the first `start_voice_mode_android`.

- [ ] **Step 4: Register the commands** under `#[cfg(feature = "android-native")]` in `commands/mod.rs`, matching the established `generate_handler!` cfg idiom (the same place `speech_capabilities` is registered).

- [ ] **Step 5: Frontend** — in `ui/voice.js`, when running on android (detect via a capability/`window.__TAURI_OS_PLUGIN__` or a build flag exposed by a command), call `start_voice_mode_android` instead of `start_voice_mode`. The state-change / transcript / chunk event handling is unchanged (same `primer://voice/*` events).

- [ ] **Step 6: Build checks**

```bash
~/.cargo/bin/cargo build -p primer-gui                              # desktop default — unchanged
~/.cargo/bin/cargo build -p primer-gui --no-default-features --features android-native --target aarch64-linux-android
```
Expected: both PASS.

- [ ] **Step 7: Commit**

```bash
git add src/crates/primer-gui/src/ src/crates/primer-gui/gen/android/ src/crates/primer-gui/ui/voice.js
git commit -m "feat(gui): android voice-mode commands + mic permission (Plan 2 Task 8)"
```

---

### Task 9: `SttBackend`/`TtsBackend` android variants + CI guard confirm

**Files:**
- Modify: `src/crates/primer-speech/src/voice_loop/selectors.rs`
- Modify: `.github/workflows/ci.yml` (only if a new build target is needed)

**Interfaces:**
- Produces: `SttBackend::AndroidNative`, `TtsBackend::AndroidNative` (kebab-case serde) so config/diagnostics can name the android path; `build_tts`/`build_voice_backends` gain android arms or are bypassed (the GUI android command calls `build_android_voice_backends` directly, so the selector arms are optional — add them only if the CLI/selectors path is wired for android, which the POC does not require).

- [ ] **Step 1:** Add the enum variants behind `#[cfg(feature = "android-native")]` (or unconditionally with a "android-only" doc note — match how `MacosNative` is gated). Add tests pinning their serde strings (`"android-native"`).

- [ ] **Step 2:** Confirm the existing `.github/workflows/ci.yml` `android-native` cross-compile guard covers all new lib code (it compiles `primer-gui --features android-native` for aarch64, which pulls `primer-speech/android-native`). No new step needed unless a new crate/target appears.

- [ ] **Step 3:** `~/.cargo/bin/cargo test -p primer-speech --features android-native selectors` → PASS. Commit.

```bash
git add src/crates/primer-speech/src/voice_loop/selectors.rs
git commit -m "feat(speech): android-native Stt/Tts backend enum variants (Plan 2 Task 9)"
```

---

### Task 10: On-device acceptance — THE GATE (airplane-mode voice turn)

No host test proves this; the deliverable is observed device behaviour. The Plan 1 capability gate (handoff 2026-06-18) already returned GO; Task 10 proves the full loop.

**Files:** none (verification + handoff note).

- [ ] **Step 1: Re-prove Task 1 (the `nativeInit` fix).** Build the APK (Global Constraints command), install, launch, invoke `speech_capabilities`, read `run-as org.theprimer.gui cat files/...` (or the command's return). It must now return the real `SpeechCapabilities` JSON — not panic. This confirms the JavaVM cache works under the Tauri-mobile runtime. If it still fails, apply the classloader fallback (Task 7 Step 2 note) and rebuild.

- [ ] **Step 2: One voice turn, online first.** From the app, enable voice mode (android command). Speak an English question. Confirm via the on-screen transcript + chat bubble + audible TTS that: transcript appears (on-device STT), the NPU answers (`QnnBackend`), and the answer is spoken (`TextToSpeech`), then the loop returns to LISTEN. Capture the `primer://voice/*` sequence if the webview console is reachable; else rely on the on-screen state widget.

- [ ] **Step 3: Airplane-mode turn (the spec's §5 acceptance).** Enable airplane mode, repeat Step 2. A complete voice turn with no network proves fully-offline STT + NPU + TTS. This is simultaneously the Phase 2 and Phase 3 exit demo.

- [ ] **Step 4: No-barge-in spot check.** While the Primer is speaking, talk over it — confirm the Primer is not interrupted and the child's speech during SPEAK is not transcribed as the next turn (the recognizer is stopped during SPEAK).

- [ ] **Step 5: Write + commit the handoff note** `docs/handoffs/<ts>-android-voice-loop.md` recording: the captured behaviour per step, the chosen classloader path (system vs app), any recognizer quirks (auto-timeout, re-arm latency), and a GO/PARTIAL verdict. If German is attempted, note whether on-device `de-DE` STT exists (likely not → Whisper fallback, deferred).

```bash
git add docs/handoffs/
git commit -m "docs: android voice loop on-device acceptance (Plan 2 Task 10)"
```

---

## Self-Review

**Spec coverage (against `2026-06-18-android-native-voice-poc-design.md`):**
- §1 STT (`SpeechRecognizer` on-device, derived VAD) → Tasks 3, 6, 7. ✅
- §1 TTS (offline voice, plays itself, `onDone` → LISTEN) → Tasks 5, 7; offline-voice guard reused from Plan 1 Task 3. ✅
- §1 "satisfies the existing speech traits so `voice_loop` works unchanged" → `ChannelStt` (Task 4) + `AndroidTts` (Task 5) + `LoopBackends` (Task 6); `run_loop` untouched. ✅
- §2 Kotlin owns Looper-bound work + Rust adapter → Tasks 7, 8; poll model (D4). ✅
- §4 `android-native` feature, BM25-only android build, desktop-unchanged → all tasks build-checked; Task 8 Step 6. ✅
- §5 acceptance (airplane-mode turn) → Task 10. ✅
- Strict-offline-first (createOnDeviceSpeechRecognizer, offline voice) → Tasks 7; Global Constraints. ✅
- No-barge-in (recognizer armed only in LISTEN) → Task 6 re-arm logic + Task 10 Step 4. ✅
- The carried `ndk_context` blocker → Task 1 (`nativeInit`). ✅
- CI drift-guard → Task 9 (existing guard confirmed sufficient). ✅

**Placeholder scan:** The only deliberate non-TDD code is the device-only JNI/Kotlin in Task 7 (and the Kotlin recognizer in Task 8 Step 1/3) — exact in shape and event-JSON contract, device-verified in Task 10, exactly as Plan 1 treated `jni_bridge`. Task 9's selector arms are explicitly optional-for-POC with the reason given. No "TBD"/"add error handling"/"similar to Task N".

**Type consistency:** `SpeechEvent` serde shape is identical across Task 2 (Rust), Task 7 (Kotlin JSON), and Task 6 (`process_event` match). `AndroidSpeechBridge`'s six methods match between the trait (Task 2), `MockBridge` (Task 2), `AndroidTts`/consumer use (Tasks 5, 6), and `JniSpeechBridge` impl (Task 7). `ChannelStt { rx: Arc<Mutex<Receiver<String>>> }` field name matches between Task 4 (moved type) and Task 6 (builder construction). `build_android_voice_backends`/`AndroidVoiceBackends` names match between Task 6 (def) and Task 8 (GUI use). `VadEvent::SpeechStart/SpeechEnd` are the existing `primer_core::speech` variants.

**Risks (carried + new):**
- **`nativeInit` symbol resolution / load order.** The `Java_org_theprimer_gui_PrimerSpeech_nativeInit` symbol must be in the loaded Tauri app `.so`. If Kotlin's `external fun nativeInit()` throws `UnsatisfiedLinkError`, the symbol name or the lib-load order is wrong — verify the exact JNI name and that `nativeInit()` is called after the Rust lib loads (Task 1 Step 6). This is the single biggest unknown; Task 10 Step 1 is its gate.
- **`find_class` on an attached thread** (Plan 1 risk #2) — fallback to the cached `Context` classloader (Task 7 Step 2 note).
- **Recognizer one-shot / re-arm latency.** `SpeechRecognizer` finalises per utterance; the consumer re-arms (`start_listening`) after each `EndOfSpeech`/`Final` (Task 6). If re-arm is too slow for back-to-back turns, batch the re-arm into LISTEN entry — tune in Task 10.
- **`speak` blocking model.** If the Android engine never fires `onDone` (rare engine bug), `push_text` would block forever. Add a generous `TTS_SPEAK_TIMEOUT` latch bound in Kotlin (named const) returning a `TtsError` so the loop recovers — fold into Task 7 if Task 10 shows it bites.
- **DRY debt:** `AndroidDerivedVad` duplicates the *shape* of `macos26/vad.rs`; hoist a shared pure state machine only if the two converge after device tuning (Task 3 note).

## Execution Handoff

This plan is intentionally front-loaded with **host-testable, TDD'd pure logic** (Tasks 1–6, 9) so the bulk lands and is verified on CI before the **device-only integration** (Tasks 7, 8, 10). Tasks 1–6 + 9 can be implemented and merged from a desktop; Tasks 7, 8, 10 need the RedMagic.
