# Android-Native Voice POC — Plan 1 of 2: Bridge + Capability Diagnostic

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove a Rust↔Kotlin speech bridge inside the existing Tauri-Android APK and answer — on the real RedMagic — whether on-device offline STT (`isOnDeviceRecognitionAvailable`) and an offline en-US TTS voice (`isNetworkConnectionRequired == false`) actually exist. This is the **go/no-go gate** for the whole voice POC.

**Architecture:** A trait-abstracted bridge seam (`AndroidSpeechBridge`) — exactly the pattern `primer-inference::qnn` uses for `GenieLibrary`. All decision logic (capability parsing, offline-voice selection) is pure Rust, host-tested with a mock bridge. The real bridge (`JniSpeechBridge`) is `target_os = "android"`-gated, calls a Kotlin helper (`PrimerSpeech.kt`) over `jni`, and is the only device-only code. A non-Android stub returns `PlatformUnsupported`, so the seam + pure logic compile and test on macOS/Linux CI — just like `primer-qnn-sys`.

**Tech Stack:** Rust (`primer-speech`, `primer-gui`), `jni` crate, Kotlin (Android `SpeechRecognizer` + `TextToSpeech` system APIs), Tauri 2.11 mobile, `serde_json`.

## Global Constraints

- Workspace root is `src/`; every cargo command runs from `src/`. Invoke as `~/.cargo/bin/cargo` (rustup), never Homebrew cargo. Toolchain pin **1.88**, edition 2024.
- Per-crate `Cargo.toml` uses `.workspace = true`; new deps are pinned in `src/Cargo.toml` `[workspace.dependencies]`.
- No magic numbers — every numeric goes to a consts module (invariant) or settings (tunable). `[[feedback_no_magic_numbers]]`.
- Strict offline-first: never select a TTS voice where `isNetworkConnectionRequired() == true`; never fall back to a network recognizer. `[[project_strict_offline_first]]`.
- The `android-native` feature is **mutually exclusive** with `macos-native` / `macos-native-26` (a `compile_error!`), mirroring the existing macOS XOR in `primer-speech/src/lib.rs`.
- The real JNI bridge is `#[cfg(target_os = "android")]`; every other target gets a stub returning `PrimerError::Speech(PlatformUnsupported)` — pure logic stays host-compilable (the `primer-qnn-sys` precedent).
- Android APK build is BM25-only / `--no-default-features` (issue #157); the new feature must cross-compile clean for `aarch64-linux-android`.
- Desktop `primer-gui` build/test/fmt/clippy must stay byte-identical when `android-native` is off.
- adb for the device: `~/Library/Android/sdk/platform-tools/adb -s 912607710061`. `/tmp` is not writable under the Android app sandbox — read diagnostics via `run-as` / logcat.

## File Structure

**New files:**
- `src/crates/primer-speech/src/android/mod.rs` — module root, feature XOR guard, re-exports, `query_capabilities()` entry point + non-Android stub.
- `src/crates/primer-speech/src/android/capabilities.rs` — `SpeechCapabilities`, `TtsVoiceInfo` types + serde + `select_offline_voice`.
- `src/crates/primer-speech/src/android/bridge.rs` — `AndroidSpeechBridge` seam trait + `MockBridge` (test-only).
- `src/crates/primer-speech/src/android/jni_bridge.rs` — `JniSpeechBridge` (real, `#[cfg(target_os = "android")]`).
- `src/crates/primer-gui/gen/android/app/src/main/java/org/theprimer/gui/PrimerSpeech.kt` — Kotlin helper (`queryCapabilities`, `nativeInit`).
- `src/crates/primer-gui/src/commands/speech_diag.rs` — the `speech_capabilities` Tauri command.

**Modified files:**
- `src/Cargo.toml` — add `jni`, `ndk-context` to `[workspace.dependencies]`.
- `src/crates/primer-speech/Cargo.toml` — `android-native` feature + deps.
- `src/crates/primer-speech/src/lib.rs` — `pub mod android;` (feature-gated) + extend the XOR `compile_error!`.
- `src/crates/primer-gui/Cargo.toml` — `android-native` feature.
- `src/crates/primer-gui/src/commands/mod.rs` — register `speech_capabilities` when feature on.
- `src/crates/primer-gui/gen/android/app/src/main/AndroidManifest.xml` — `RECORD_AUDIO` permission.
- `src/crates/primer-gui/gen/android/app/src/main/java/org/theprimer/gui/MainActivity.kt` — call `PrimerSpeech.nativeInit(this)`.
- `.github/workflows/ci.yml` — `android-native` cross-compile drift-guard.

---

### Task 1: `android-native` feature scaffold + module skeleton

**Files:**
- Modify: `src/Cargo.toml` (`[workspace.dependencies]`)
- Modify: `src/crates/primer-speech/Cargo.toml`
- Create: `src/crates/primer-speech/src/android/mod.rs`
- Modify: `src/crates/primer-speech/src/lib.rs`

**Interfaces:**
- Produces: cargo feature `primer-speech/android-native`; module `primer_speech::android`; the XOR guard.

- [ ] **Step 1: Add workspace deps**

In `src/Cargo.toml` under `[workspace.dependencies]`:

```toml
jni = "0.21"
ndk-context = "0.1"
```

- [ ] **Step 2: Add the feature + deps to primer-speech**

In `src/crates/primer-speech/Cargo.toml` `[dependencies]`:

```toml
jni = { workspace = true, optional = true }
ndk-context = { workspace = true, optional = true }
```

(`serde_json` is already an optional dep — reuse it.) In `[features]`:

```toml
# Android-native speech (SpeechRecognizer + TextToSpeech via a Kotlin
# helper called over JNI). Mutually exclusive with the macOS-native
# features. The pure logic compiles on every host; only the JNI bridge is
# target_os="android"-gated, so host CI tests the decision logic.
android-native = ["dep:jni", "dep:ndk-context", "dep:serde_json"]
```

- [ ] **Step 3: Create the module skeleton with the platform stub**

`src/crates/primer-speech/src/android/mod.rs`:

```rust
//! Android-native speech: on-device `SpeechRecognizer` STT + `TextToSpeech`
//! TTS via a Kotlin helper called over JNI. Plan 1 ships only the capability
//! diagnostic; the voice loop lands in Plan 2.

mod capabilities;
pub mod bridge;

pub use capabilities::{select_offline_voice, SpeechCapabilities, TtsVoiceInfo};

#[cfg(target_os = "android")]
mod jni_bridge;

use primer_core::error::Result;

/// Query the device's speech capabilities. On Android this drives the real
/// JNI bridge; on every other target it is a platform stub so the crate and
/// its tests still build host-side.
#[cfg(target_os = "android")]
pub fn query_capabilities() -> Result<SpeechCapabilities> {
    jni_bridge::JniSpeechBridge::new()?.query_capabilities()
}

#[cfg(not(target_os = "android"))]
pub fn query_capabilities() -> Result<SpeechCapabilities> {
    Err(primer_core::error::PrimerError::Speech(
        "android speech capabilities are only available on android targets".into(),
    ))
}
```

- [ ] **Step 4: Wire the module + XOR guard in lib.rs**

In `src/crates/primer-speech/src/lib.rs`, add near the existing macOS `compile_error!`:

```rust
#[cfg(all(
    feature = "android-native",
    any(feature = "macos-native", feature = "macos-native-26")
))]
compile_error!(
    "android-native is mutually exclusive with macos-native / macos-native-26; \
     pick one via --features"
);

#[cfg(feature = "android-native")]
pub mod android;
```

- [ ] **Step 5: Verify it builds on host and cross-compiles**

Run from `src/`:
```bash
~/.cargo/bin/cargo build -p primer-speech --features android-native
~/.cargo/bin/cargo build -p primer-speech --features android-native --target aarch64-linux-android
```
Expected: both PASS (host uses the stub; android target compiles the `jni_bridge` module which is empty so far — Task 4 fills it; until then `jni_bridge` is created as an empty file so the `mod` resolves). Create `src/crates/primer-speech/src/android/jni_bridge.rs` with just `//! placeholder, filled in Task 4` for now.

- [ ] **Step 6: Commit**

```bash
git add src/Cargo.toml src/crates/primer-speech/Cargo.toml src/crates/primer-speech/src/lib.rs src/crates/primer-speech/src/android/
git commit -m "feat(speech): android-native feature scaffold + module skeleton"
```

---

### Task 2: Capability types + serde

**Files:**
- Create: `src/crates/primer-speech/src/android/capabilities.rs`
- Test: same file (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `SpeechCapabilities { on_device_recognition_available: bool, recognition_locales: Vec<String>, tts_voices: Vec<TtsVoiceInfo> }`; `TtsVoiceInfo { name: String, locale: String, network_required: bool, not_installed: bool }`. Both `Serialize + Deserialize`. These are the exact JSON shape the Kotlin `queryCapabilities` emits and the `jni_bridge` parses.

- [ ] **Step 1: Write the failing test (serde round-trip from the Kotlin JSON shape)**

In `src/crates/primer-speech/src/android/capabilities.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kotlin_capabilities_json() {
        // Exactly the shape PrimerSpeech.queryCapabilities() emits.
        let json = r#"{
            "on_device_recognition_available": true,
            "recognition_locales": [],
            "tts_voices": [
                {"name":"en-us-x-sfg#female_1-local","locale":"en-US","network_required":false,"not_installed":false},
                {"name":"en-us-x-iol-network","locale":"en-US","network_required":true,"not_installed":false}
            ]
        }"#;
        let caps: SpeechCapabilities = serde_json::from_str(json).unwrap();
        assert!(caps.on_device_recognition_available);
        assert_eq!(caps.tts_voices.len(), 2);
        assert!(!caps.tts_voices[0].network_required);
        assert!(caps.tts_voices[1].network_required);
    }
}
```

- [ ] **Step 2: Run it, verify it fails**

Run: `~/.cargo/bin/cargo test -p primer-speech --features android-native capabilities::tests::parses_kotlin -- --nocapture`
Expected: FAIL — `SpeechCapabilities` / `TtsVoiceInfo` not defined.

- [ ] **Step 3: Add the types**

At the top of the same file:

```rust
//! Pure capability types + offline-voice selection. No JNI here — this is
//! the host-testable decision layer.

use serde::{Deserialize, Serialize};

/// A snapshot of the device's speech capabilities, mirroring the JSON
/// emitted by `PrimerSpeech.queryCapabilities()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechCapabilities {
    /// `SpeechRecognizer.isOnDeviceRecognitionAvailable(context)`.
    pub on_device_recognition_available: bool,
    /// Best-effort on-device recognition locales (BCP-47). May be empty —
    /// Android exposes no synchronous accessor; populated only if a later
    /// async probe lands. Empty is not a failure.
    pub recognition_locales: Vec<String>,
    /// Every installed `TextToSpeech` voice with its offline flags.
    pub tts_voices: Vec<TtsVoiceInfo>,
}

/// One `android.speech.tts.Voice`, reduced to the fields the offline-first
/// guard needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TtsVoiceInfo {
    pub name: String,
    /// BCP-47 from `Voice.getLocale().toLanguageTag()`, e.g. `en-US`.
    pub locale: String,
    /// `Voice.isNetworkConnectionRequired()`.
    pub network_required: bool,
    /// `Voice.getFeatures()` contains `KEY_FEATURE_NOT_INSTALLED`.
    pub not_installed: bool,
}
```

- [ ] **Step 4: Run it, verify it passes**

Run: `~/.cargo/bin/cargo test -p primer-speech --features android-native capabilities::tests::parses_kotlin`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-speech/src/android/capabilities.rs
git commit -m "feat(speech): android capability types + serde"
```

---

### Task 3: Offline-voice selection (the strict-offline-first guard)

**Files:**
- Modify: `src/crates/primer-speech/src/android/capabilities.rs`
- Test: same file

**Interfaces:**
- Produces: `pub fn select_offline_voice<'a>(voices: &'a [TtsVoiceInfo], bcp47: &str) -> Option<&'a TtsVoiceInfo>` — returns the first installed, non-network voice whose locale matches `bcp47` exactly, else a language-prefix match (`en` ⊂ `en-US`), else `None`. This is the guard the TTS backend (Plan 2) calls; hard-erroring on `None` is the caller's job.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests`:

```rust
fn v(name: &str, locale: &str, net: bool, missing: bool) -> TtsVoiceInfo {
    TtsVoiceInfo { name: name.into(), locale: locale.into(), network_required: net, not_installed: missing }
}

#[test]
fn picks_offline_exact_locale_and_rejects_network() {
    let voices = vec![
        v("net", "en-US", true, false),
        v("offline", "en-US", false, false),
    ];
    assert_eq!(select_offline_voice(&voices, "en-US").unwrap().name, "offline");
}

#[test]
fn rejects_not_installed_even_if_offline() {
    let voices = vec![v("ghost", "en-US", false, true)];
    assert!(select_offline_voice(&voices, "en-US").is_none());
}

#[test]
fn falls_back_to_language_prefix() {
    let voices = vec![v("gb", "en-GB", false, false)];
    assert_eq!(select_offline_voice(&voices, "en").unwrap().name, "gb");
}

#[test]
fn none_when_only_network_voices() {
    let voices = vec![v("net", "en-US", true, false)];
    assert!(select_offline_voice(&voices, "en-US").is_none());
}
```

- [ ] **Step 2: Run, verify FAIL**

Run: `~/.cargo/bin/cargo test -p primer-speech --features android-native capabilities::tests`
Expected: FAIL — `select_offline_voice` not defined.

- [ ] **Step 3: Implement**

```rust
/// Pick the best fully-offline, installed TTS voice for `bcp47`, or `None`.
/// Exact-locale match wins; otherwise a language-prefix match (`en` matches
/// `en-US`). Network-required or not-installed voices are never returned —
/// `[[project_strict_offline_first]]`.
pub fn select_offline_voice<'a>(
    voices: &'a [TtsVoiceInfo],
    bcp47: &str,
) -> Option<&'a TtsVoiceInfo> {
    let usable = |v: &&TtsVoiceInfo| !v.network_required && !v.not_installed;
    voices
        .iter()
        .find(|v| usable(v) && v.locale.eq_ignore_ascii_case(bcp47))
        .or_else(|| {
            let lang = bcp47.split('-').next().unwrap_or(bcp47);
            voices.iter().find(|v| {
                usable(v) && v.locale.split('-').next().unwrap_or("").eq_ignore_ascii_case(lang)
            })
        })
}
```

- [ ] **Step 4: Run, verify PASS**

Run: `~/.cargo/bin/cargo test -p primer-speech --features android-native capabilities::tests`
Expected: PASS (all 4 new + the round-trip).

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-speech/src/android/capabilities.rs
git commit -m "feat(speech): offline TTS voice selection guard"
```

---

### Task 4: Bridge seam + the real JNI bridge + Kotlin helper

This is the **integration spike** the spec flags as the primary risk. The pure seam is host-testable; the JNI + Kotlin halves are device-verified. Do the seam first (host, TDD), then the device halves.

**Files:**
- Create: `src/crates/primer-speech/src/android/bridge.rs`
- Modify: `src/crates/primer-speech/src/android/jni_bridge.rs` (was a placeholder from Task 1)
- Create: `src/crates/primer-gui/gen/android/app/src/main/java/org/theprimer/gui/PrimerSpeech.kt`
- Modify: `src/crates/primer-gui/gen/android/app/src/main/java/org/theprimer/gui/MainActivity.kt`

**Interfaces:**
- Produces: `pub trait AndroidSpeechBridge { fn query_capabilities(&self) -> Result<SpeechCapabilities>; }`; `JniSpeechBridge` implementing it on android; a `MockBridge` for host tests.

- [ ] **Step 1: Write the seam test with a mock (host)**

`src/crates/primer-speech/src/android/bridge.rs`:

```rust
//! The bridge seam. `query_capabilities()` on the module routes through an
//! `AndroidSpeechBridge`; the real impl is JNI (android-only), the mock makes
//! the surrounding logic host-testable — the `primer-inference::qnn`
//! `GenieLibrary`/`GenieDialog` pattern.

use crate::android::SpeechCapabilities;
use primer_core::error::Result;

/// Everything Plan 1 needs from the device. Plan 2 extends this trait with
/// `start_listening` / `speak` / `cancel` / `poll_event`.
pub trait AndroidSpeechBridge: Send {
    fn query_capabilities(&self) -> Result<SpeechCapabilities>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::android::TtsVoiceInfo;

    struct MockBridge(SpeechCapabilities);
    impl AndroidSpeechBridge for MockBridge {
        fn query_capabilities(&self) -> Result<SpeechCapabilities> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn bridge_returns_capabilities() {
        let caps = SpeechCapabilities {
            on_device_recognition_available: true,
            recognition_locales: vec![],
            tts_voices: vec![TtsVoiceInfo {
                name: "offline".into(),
                locale: "en-US".into(),
                network_required: false,
                not_installed: false,
            }],
        };
        let bridge = MockBridge(caps.clone());
        assert_eq!(bridge.query_capabilities().unwrap(), caps);
    }
}
```

- [ ] **Step 2: Run, verify PASS (seam compiles, mock test green)**

Run: `~/.cargo/bin/cargo test -p primer-speech --features android-native bridge::tests`
Expected: PASS.

- [ ] **Step 3: Write the Kotlin helper**

`.../org/theprimer/gui/PrimerSpeech.kt`:

```kotlin
package org.theprimer.gui

import android.content.Context
import android.speech.SpeechRecognizer
import android.speech.tts.TextToSpeech
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Looper-bound Android speech work, called from Rust over JNI. */
object PrimerSpeech {
    // Cached by nativeInit so JNI calls on attached threads can resolve the
    // app classloader + a real Context (the system classloader on an attached
    // thread cannot see app classes — the canonical JNI-on-Android gotcha).
    @Volatile @JvmStatic var appContext: Context? = null

    @JvmStatic
    fun init(ctx: Context) { appContext = ctx.applicationContext }

    /** Returns the SpeechCapabilities JSON the Rust side parses with serde. */
    @JvmStatic
    fun queryCapabilities(): String {
        val ctx = appContext ?: return """{"error":"no context"}"""
        val obj = JSONObject()
        obj.put("on_device_recognition_available",
            SpeechRecognizer.isOnDeviceRecognitionAvailable(ctx))
        obj.put("recognition_locales", JSONArray())

        val voicesJson = JSONArray()
        val latch = CountDownLatch(1)
        var tts: TextToSpeech? = null
        tts = TextToSpeech(ctx) { status ->
            if (status == TextToSpeech.SUCCESS) {
                runCatching {
                    for (vo in tts?.voices ?: emptySet()) {
                        voicesJson.put(JSONObject().apply {
                            put("name", vo.name)
                            put("locale", vo.locale.toLanguageTag())
                            put("network_required", vo.isNetworkConnectionRequired)
                            put("not_installed",
                                vo.features?.contains(
                                    TextToSpeech.Engine.KEY_FEATURE_NOT_INSTALLED) == true)
                        })
                    }
                }
            }
            latch.countDown()
        }
        latch.await(5, TimeUnit.SECONDS)
        tts?.shutdown()
        obj.put("tts_voices", voicesJson)
        return obj.toString()
    }
}
```

- [ ] **Step 4: Call `init` from the activity**

In `MainActivity.kt`, inside `onCreate` after `super.onCreate(...)`:

```kotlin
PrimerSpeech.init(this)
```

- [ ] **Step 5: Write the real JNI bridge (android-gated)**

`src/crates/primer-speech/src/android/jni_bridge.rs`:

```rust
//! Real `AndroidSpeechBridge` over `jni`. Device-only.

use crate::android::bridge::AndroidSpeechBridge;
use crate::android::SpeechCapabilities;
use jni::objects::{JObject, JString};
use jni::JavaVM;
use primer_core::error::{PrimerError, Result};

pub struct JniSpeechBridge {
    vm: JavaVM,
}

fn jerr(e: impl std::fmt::Display) -> PrimerError {
    PrimerError::Speech(format!("android speech JNI: {e}"))
}

impl JniSpeechBridge {
    pub fn new() -> Result<Self> {
        // ndk_context is populated by the Tauri-mobile runtime. If Task 4's
        // on-device smoke shows it is NOT, the fallback is to cache the
        // JavaVM in a `nativeInit` JNI export called from MainActivity —
        // see the plan's Risks section.
        let ctx = ndk_context::android_context();
        let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.map_err(jerr)?;
        Ok(Self { vm })
    }
}

impl AndroidSpeechBridge for JniSpeechBridge {
    fn query_capabilities(&self) -> Result<SpeechCapabilities> {
        let mut env = self.vm.attach_current_thread().map_err(jerr)?;
        // Resolve PrimerSpeech via the app context's classloader, not the
        // system one (attached-thread gotcha).
        let class = env
            .find_class("org/theprimer/gui/PrimerSpeech")
            .map_err(jerr)?;
        let json: JString = env
            .call_static_method(class, "queryCapabilities", "()Ljava/lang/String;", &[])
            .and_then(|v| v.l())
            .map_err(jerr)?
            .into();
        let s: String = env.get_string(&json).map_err(jerr)?.into();
        serde_json::from_str(&s).map_err(jerr)
    }
}
```

- [ ] **Step 6: Cross-compile the android target**

Run from `src/`:
```bash
~/.cargo/bin/cargo build -p primer-speech --features android-native --target aarch64-linux-android
```
Expected: PASS (the JNI bridge compiles for android; host build still uses the stub).

- [ ] **Step 7: Commit**

```bash
git add src/crates/primer-speech/src/android/ \
  src/crates/primer-gui/gen/android/app/src/main/java/org/theprimer/gui/PrimerSpeech.kt \
  src/crates/primer-gui/gen/android/app/src/main/java/org/theprimer/gui/MainActivity.kt
git commit -m "feat(speech): android JNI speech bridge + Kotlin capability helper"
```

---

### Task 5: GUI `android-native` feature + `speech_capabilities` command + manifest permission

**Files:**
- Modify: `src/crates/primer-gui/Cargo.toml`
- Create: `src/crates/primer-gui/src/commands/speech_diag.rs`
- Modify: `src/crates/primer-gui/src/commands/mod.rs`
- Modify: `src/crates/primer-gui/gen/android/app/src/main/AndroidManifest.xml`

**Interfaces:**
- Consumes: `primer_speech::android::query_capabilities() -> Result<SpeechCapabilities>`.
- Produces: Tauri command `speech_capabilities() -> Result<SpeechCapabilities, String>`, registered only under `feature = "android-native"`.

- [ ] **Step 1: Add the GUI feature**

In `src/crates/primer-gui/Cargo.toml` `[features]`:

```toml
# Android-native speech diagnostic + (Plan 2) voice loop. Forwards to
# primer-speech/android-native. Independent of `speech` (no cpal/whisper/piper).
android-native = ["dep:primer-speech", "primer-speech/android-native"]
```

- [ ] **Step 2: Write the command**

`src/crates/primer-gui/src/commands/speech_diag.rs`:

```rust
//! Android speech-capability diagnostic (Plan 1 go/no-go gate). Surfaced in
//! Settings → Diagnostics; read back over adb/logcat on the device.

use primer_speech::android::SpeechCapabilities;

#[tauri::command]
pub async fn speech_capabilities() -> Result<SpeechCapabilities, String> {
    let caps = primer_speech::android::query_capabilities().map_err(|e| e.to_string())?;
    // Mirror to logcat so it is readable without a UI round-trip.
    tracing::info!(target: "primer::speech::diag", ?caps, "speech capabilities");
    Ok(caps)
}
```

- [ ] **Step 3: Register it (feature-gated)**

In `src/crates/primer-gui/src/commands/mod.rs`, add `#[cfg(feature = "android-native")] pub mod speech_diag;` and, in the `register` builder, gate the handler:

```rust
#[cfg(feature = "android-native")]
let builder = builder.invoke_handler(/* ...existing handlers... speech_diag::speech_capabilities */);
```

Follow the existing `commands::register` pattern exactly — if it uses one `generate_handler!`, add `speech_diag::speech_capabilities` to it behind `#[cfg(feature = "android-native")]` using the codebase's established cfg-in-handler-list idiom (check how `qnn`/`speech` handlers are conditionally listed and match it).

- [ ] **Step 4: Add the manifest permission**

In `AndroidManifest.xml`, alongside the existing `<uses-permission android:name="android.permission.INTERNET" />`:

```xml
<uses-permission android:name="android.permission.RECORD_AUDIO" />
```

(Not exercised by the diagnostic — `isOnDeviceRecognitionAvailable` and TTS enumeration need no mic — but staged here for Plan 2; the runtime grant request is a Plan 2 step.)

- [ ] **Step 5: Verify host build (feature off) is unchanged + feature-on compiles**

Run from `src/`:
```bash
~/.cargo/bin/cargo build -p primer-gui                         # desktop, default — unchanged
~/.cargo/bin/cargo build -p primer-gui --features android-native --target aarch64-linux-android --no-default-features
```
Expected: both PASS.

- [ ] **Step 6: Commit**

```bash
git add src/crates/primer-gui/Cargo.toml src/crates/primer-gui/src/commands/ \
  src/crates/primer-gui/gen/android/app/src/main/AndroidManifest.xml
git commit -m "feat(gui): speech_capabilities diagnostic command (android-native)"
```

---

### Task 6: CI drift-guard

**Files:**
- Modify: `.github/workflows/ci.yml` (the `android-cross-compile` job)

**Interfaces:**
- Produces: a CI step that fails if `primer-gui --features android-native` stops cross-compiling for android.

- [ ] **Step 1: Add the cross-compile step**

In the `android-cross-compile` job, after the existing `--features qnn` GUI step, add:

```yaml
- name: Cross-compile primer-gui --features android-native (Tauri mobile)
  run: cargo build --target aarch64-linux-android --no-default-features -p primer-gui --features android-native
  # Drift-guard for the Android-native voice POC (Plan 1): compiles the
  # JNI speech bridge + diagnostic command into the android-mobile dep
  # graph. Lib-only (no Gradle, no APK), mirroring the qnn guard above.
```

- [ ] **Step 2: Validate the YAML locally**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('ok')"`
Expected: `ok`.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: android-native cross-compile drift-guard"
```

---

### Task 7: On-device validation — THE GATE

No host test can answer this; the deliverable is observed device output. Build the APK, install on the RedMagic, invoke the diagnostic, read the result.

**Files:** none (verification only).

**Interfaces:**
- Consumes: everything above. Produces: a recorded `SpeechCapabilities` from the real device and a go/no-go decision.

- [ ] **Step 1: Build the debug APK with android-native**

From `src/crates/primer-gui`:
```bash
NDK_HOME=/opt/homebrew/share/android-ndk \
  ~/.cargo/bin/cargo-tauri android build --apk --debug --target aarch64 \
  -- --no-default-features --features android-native
```
Expected: a debug APK at `gen/android/app/build/outputs/apk/.../app-*-debug.apk`. (Confirm the exact feature-passthrough syntax `cargo-tauri` 2.11 accepts; if `--` passthrough differs, set the features in the Tauri Android config per the existing qnn build — match how the qnn APK selects features.)

- [ ] **Step 2: Install on the device**

```bash
ADB="$HOME/Library/Android/sdk/platform-tools/adb"
"$ADB" -s 912607710061 install -r gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
```
Expected: `Success`. (Adjust the APK path to the actual output.)

- [ ] **Step 3: Launch + drive logcat**

```bash
"$ADB" -s 912607710061 logcat -c
"$ADB" -s 912607710061 shell monkey -p org.theprimer.gui -c android.intent.category.LAUNCHER 1
"$ADB" -s 912607710061 logcat -s primer:* | grep -i "speech capabilities"
```

- [ ] **Step 4: Invoke the diagnostic and read the result**

Trigger `speech_capabilities` from Settings → Diagnostics in the app (Plan-1 minimal: a button that `invoke("speech_capabilities")` and logs; if no UI hook is wired, call it from the devtools console via `window.__TAURI__.core.invoke("speech_capabilities")`). Read the `primer::speech::diag` logcat line.

Record the JSON. **The gate:**
- `on_device_recognition_available == true` AND
- at least one `tts_voices[]` entry with `locale` starting `en` and `network_required == false` and `not_installed == false`.

- [ ] **Step 5: Decide**

- **Both true → GO.** Write a short handoff note (`docs/handoffs/<ts>-android-voice-gate.md`) recording the captured `SpeechCapabilities`, and proceed to brainstorm/write **Plan 2** (the voice loop: extend `AndroidSpeechBridge` with `start_listening`/`speak`/`poll_event`, the recognizer→VadEvent state machine, the cpal-free `LoopBackends` wiring into `start_voice_mode`, mic runtime-permission, airplane-mode acceptance).
- **Either false → STOP, re-scope.** If on-device recognition is false, fall back to Whisper STT (the spec's option B). If no offline en voice, the fix is a one-time voice install (`adb shell am start -a com.android.settings.TTS_SETTINGS` to install, then re-probe). Record the finding either way.

- [ ] **Step 6: Commit the handoff note**

```bash
git add docs/handoffs/
git commit -m "docs: android voice capability gate result (on-device)"
```

---

## Self-Review

**Spec coverage (against `2026-06-18-android-native-voice-poc-design.md`):**
- §3 capability diagnostic → Tasks 2–7. ✅
- Strict-offline-first TTS guard → Task 3. ✅
- Kotlin-owns-native-work + Rust adapter bridge → Task 4 (refined from "Tauri plugin" to a Kotlin helper called over `jni`; the spec's principle — Kotlin owns the Looper-bound speech work, Rust adapts — is preserved; rationale in Risks). ✅
- `android-native` feature + BM25-only android build + desktop-unchanged → Tasks 1, 5. ✅
- CI drift-guard → Task 6. ✅
- Go/no-go gate with re-scope branch → Task 7. ✅
- §1 STT/TTS/VAD voice loop, §5 acceptance (airplane-mode turn) → **deliberately Plan 2** (written after the gate; planning it now would build on an unvalidated bridge). Noted in Task 7 Step 5.

**Placeholder scan:** the only intentional deferrals are the `jni_bridge.rs` empty file in Task 1 (filled in Task 4) and the Plan-2 hand-off — both explicit, not vague. No "TBD"/"add error handling"/"similar to". Two flagged confirmations (cargo-tauri feature-passthrough syntax in Task 7 Step 1; the exact `generate_handler!` cfg idiom in Task 5 Step 3) point at existing codebase patterns to match, not invented APIs.

**Type consistency:** `SpeechCapabilities` / `TtsVoiceInfo` field names are identical across the Rust types (Task 2), the Kotlin JSON keys (Task 4 Step 3), and the JNI parse (Task 4 Step 5). `select_offline_voice` signature matches between Task 3 and the Plan-2 hand-off note. `AndroidSpeechBridge::query_capabilities` matches between seam (Task 4 Step 1) and impl (Task 4 Step 5).

## Risks

- **`ndk_context::android_context()` may not be populated by the Tauri-mobile runtime.** Task 4 Step 5 uses it; if the on-device smoke (Task 7) shows it returns null/garbage, the fallback is a `#[no_mangle] extern "C"` `nativeInit(JNIEnv, JObject)` exported from Rust and called from `MainActivity.onCreate`, caching the `JavaVM` (via `env.get_java_vm()`) in a `OnceLock`. The Kotlin `PrimerSpeech.init` already caches the Context, so only the VM handle needs this path. This is the single biggest unknown and is exactly why Task 7 is a gate, not a formality.
- **`find_class` on an attached thread can't see app classes** (system classloader). If `find_class("org/theprimer/gui/PrimerSpeech")` fails on-device, resolve `PrimerSpeech` via the cached Context's `getClassLoader().loadClass(...)` instead. Both paths are well-documented JNI-on-Android idioms; Task 7 reveals which the Tauri runtime needs.
- **`cargo-tauri` feature passthrough.** The exact way to pass `--no-default-features --features android-native` to the Gradle-driven cargo build (Task 7 Step 1) must match how the existing qnn APK selects features; confirm against the qnn build invocation before assuming `--` passthrough works.
