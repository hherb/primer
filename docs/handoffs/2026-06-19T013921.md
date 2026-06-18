# Primer — Next Session Brief

**Audience:** future Claude Code session continuing work on this repo.
**Last updated:** 2026-06-19 — work is on feature branch `android-voice-loop-plan2-tasks2-9`, pushed and open as **PR #251** (7 commits ahead of `main` at `22d8487`; CI running at handoff). This session **shipped all host-testable Plan 2 tasks of the Android-native voice POC — Tasks 2–6 + 9** (the carried "Tasks 2–6 + 9 are host-mergeable now" item from the previous brief). Plan 2 Task 1 (the `nativeInit` bridge bootstrap) had already merged via PR #250 (`22d8487` on `main`); this session picked up the next host-mergeable batch exactly as the prior brief directed.

**Context at session start:** the prior brief's `android-voice-loop-plan2` branch was already fully squash-merged into `main` (PR #250) — its tree was byte-identical to `main`, so there was nothing left to push there. The next concrete step it named was "implement Plan 2 Tasks 2–6 + 9 by TDD" from a desktop; that is what this session did, on a fresh branch off `main`.

## What we shipped this session

All on branch `android-voice-loop-plan2-tasks2-9` (PR #251, open):

- **`07c249a` — Task 2: event types + extended bridge trait.** `android/events.rs` (`SpeechEvent` serde `kind`-tagged enum — the exact JSON `pollSpeechEvent()` will emit), extended `AndroidSpeechBridge` (`start_listening`/`stop_listening`/`poll_event`/`speak`/`cancel_speech`) + a scriptable `MockBridge` reused by the Task 5/6 tests. `JniSpeechBridge` gets `not yet implemented (Task 7)` stubs for the five new methods so the aarch64 guard stays green.
- **`65adbcf` — Task 3: derived VAD.** Pure `android/vad.rs::AndroidDerivedVad` (recognizer `SpeechEvent` → `VadEvent` edges; a `Final` from idle returns `SpeechStart` and stashes a pending `SpeechEnd` via `take_pending_end`). New `consts::speech::android` block (`SPEECH_START_MIN_TEXT_CHARS`, `POLL_TIMEOUT`).
- **`87e5bbc` — Task 4: un-gate `ChannelStt`.** Moved verbatim from cpal-gated `voice_loop::backends_common` into a cpal-free `voice_loop::channel_stt` (re-exported from both old and new paths so the macOS/whisper builders are unchanged). Added `ChannelStt::from_receiver`.
- **`7ff63fa` — Task 5: `AndroidTts`.** `android/tts.rs` — a `StreamingTextToSpeech` whose `push_text` calls `bridge.speak` (blocks until `onDone`) and emits **no** `SynthesisEvent::Audio` (the OS plays itself, D3).
- **`2f20cc8` — Task 6: STT consumer + builder.** `android/stt.rs` pure `process_event` (host-tested) + `run_recognizer_loop` async driver; `voice_loop/backends_android_native.rs::build_android_voice_backends` returning the cpal-free `AndroidVoiceBackends { backends, event_rx, stop }`.
- **`84ce51d` — Task 9: enum variants + voice_loop build fix.** `SttBackend::AndroidNative` / `TtsBackend::AndroidNative` (kebab-case serde, pinned by tests). **Load-bearing fix:** the `voice_loop` module was gated solely on the cpal-pulling `voice-loop` feature, so the Task 6 builder was *silently cfg'd out* on a `--features android-native` build; widened the gate to `any(feature = "voice-loop", feature = "android-native")` — the cpal-dependent submodules stay individually cpal-gated, so android only switches on the cpal-free parts.
- **`2faaf02` — fmt + ROADMAP.** rustfmt over the new modules, dropped a duplicated inner `#![cfg]`, and updated ROADMAP sub-project 6 (Tasks 1–6 + 9 landed; 7/8/10 remain). **README untouched** — nothing user-facing shipped (the POC isn't usable until Task 10).
- **`1334e05` — classloader fix (device-verified).** `nativeInit` caches the `PrimerSpeech` `jclass` (its own `jclass` arg) as a `GlobalRef`; the bridge resolves the class from that cache instead of `JNIEnv::find_class`, fixing the on-device `ClassNotFoundException` (Plan 1 risk #2). Verified on the RedMagic: `speech_capabilities` returns real JSON (`on_device_recognition_available:true`, offline en-US voice present). See the on-device section below.

## What's next (concrete acceptance criteria)

### 1. ⭐ Land PR #251, then start the device-only tasks
- **Acceptance:** confirm PR #251 CI is green (`cargo test (default features)`, `cargo check (non-default features)`, the two android cross-compile guards — `cargo build (aarch64-linux-android)` and the `primer-gui --features android-native` guard) and merge it. After that, **all of Plan 2's host work is done**; only Tasks 7, 8, 10 remain, and they need the RedMagic.

### 2. Device-only Plan 2 tasks (need the RedMagic 11 Pro)
- **Task 7** — real JNI bridge methods (replace the `not yet implemented (Task 7)` stubs in `jni_bridge.rs` with `call_static_method` invocations mirroring `query_capabilities`) + the Kotlin recognizer/TTS plugin in `PrimerSpeech.kt` (persistent `SpeechRecognizer` via `createOnDeviceSpeechRecognizer`, `RecognitionListener` enqueuing `SpeechEvent` JSON matching Task 2's serde shape, `TextToSpeech` + `UtteranceProgressListener` blocking `speak` on a `CountDownLatch`). The event-JSON contract is the cross-language type seam — keep Kotlin's strings identical to `SpeechEvent`'s `#[serde(rename_all="snake_case")]` (`partial`/`final`/`end_of_speech`/`stt_error`/`tts_done`/`tts_error`).
- **Task 8** — GUI android voice commands (`start_voice_mode_android`/`stop_voice_mode_android`/`cancel_voice_response_android` calling `run_loop` with a no-op `on_committed_audio` + `None` drain + `None` is_speaking, per D1) + runtime `RECORD_AUDIO` permission + the `ui/voice.js` android branch.
- **Acceptance (Task 10, the POC gate):** on the RedMagic, in the APK, **in airplane mode**, one full voice turn — speak English → on-device STT transcribes → `QnnBackend` answers → `TextToSpeech` speaks it → loop returns to LISTEN. Simultaneously the Phase 2 and Phase 3 exit demo. **Task 10 Step 1 also re-proves Plan 2 Task 1** (rebuild the APK; `speech_capabilities` must return real JSON, not panic — proves the `nativeInit` JavaVM cache works under Tauri-mobile). If it still fails, apply the `find_class` classloader fallback (Task 7 Step 2 note in the plan).

### Carried / owner-or-hardware-gated (unchanged)
- Pedagogy/answer-quality tuning on the 4B NPU model (owner-in-the-loop — the standing top open question); on-device #224 length-recovery spot-check; latency-routing calibration (`--primary-ttft-budget-ms` around the measured p95 ≈ 2.6 s); #223 GENIE enum; #170 Supertonic Stages E/F; #201 llamacpp BOS; #192/#166 human-at-mic smokes; #157 Termux ONNX validation; #135 glib bump on Tauri 3.

## On-device this session — Task 1 fully re-proven AND the classloader blocker fixed + verified

Built the debug APK (`--no-default-features --features android-native`), installed on the RedMagic (912607710061), launched, and probed. **Both the `nativeInit` JavaVM cache and the new classloader fix are now device-verified; `speech_capabilities` returns real JSON.**

- **✅ `nativeInit` links + runs.** The `Java_org_theprimer_gui_PrimerSpeech_nativeInit` symbol is present and exported in the packaged `lib/arm64-v8a/libprimer_gui.so` (`llvm-nm -D`); the app launches without `UnsatisfiedLinkError`. The #1 carried device unknown ("symbol resolution / load order") is **cleared**.
- **✅ Task 1's JavaVM cache works.** The first APK got `query_capabilities` **past** `java_vm()` + `attach_current_thread` to `find_class` — exactly the path `ndk_context` failed.
- **✅ Plan 1 risk #2 (the `find_class` classloader problem) is FIXED (commit `1334e05`).** The first APK crashed with `java.lang.ClassNotFoundException: "org.theprimer.gui.PrimerSpeech"` (system/bootstrap classloader on a native attached thread — `DexPathList[…nativeLibraryDirectories=[/system/lib64,…]]`). **Fix shipped:** `nativeInit` receives the `PrimerSpeech` class as its `jclass` arg (it's a static method on it) on a real Java thread with the app classloader, and caches a `GlobalRef` to it (`VmCache<GlobalRef>` in `vm.rs`); the bridge materialises a local `JClass` from that cache instead of calling `find_class`. **Verified on the rebuilt APK:** `speech_capabilities` returns real JSON — `on_device_recognition_available: true` with an **offline installed en-US voice** present (`en-us-x-tpf-local`, `network_required:false`, `not_installed:false`). App stays alive, no new crash.

**Device-debug recipe that worked (logcat is dead on this ROM):** symbol check `unzip APK lib/arm64-v8a/*.so` + `llvm-nm -D`; crash inspection `adb shell dumpsys dropbox --print` (look for `data_app_native_crash` / `data_app_crash`). **WebView CDP DOES work** for invoking Tauri commands if you suppress the `Origin` header (Chromium 403s a non-allowlisted `Origin`, but accepts a request with no Origin): `adb forward tcp:9222 localabstract:webview_devtools_remote_<pid>`, then `websocket.create_connection(ws_url, suppress_origin=True)` → `Runtime.evaluate("window.__TAURI__.core.invoke('<cmd>')", awaitPromise=True)`. Helper script left at `/tmp/cdp_invoke.py`. (`withGlobalTauri` is on, so `window.__TAURI__.core.invoke` is available.)

**Immediate next step:** Tasks 7/8/10 proper. Task 1 is done and the JNI class-resolution pattern is proven, so Task 7's five real bridge methods (`start_listening`/`stop_listening`/`poll_event`/`speak`/`cancel_speech`) can reuse the same cached-class `primer_speech_class(env)` helper in `jni_bridge.rs` — no per-method `find_class`. Then the Kotlin recognizer/TTS plugin + GUI commands (Task 8) + the airplane-mode acceptance turn (Task 10).

## Open decisions / risks

- **The `voice_loop` gate widening (`84ce51d`) is the one non-obvious design call.** Before it, `--features android-native` compiled `primer-speech` *without* `voice_loop`, so `build_android_voice_backends` and the shared `run_loop` the GUI android command (Task 8) will drive were silently absent. The fix is correct and the cpal submodules stay gated — but anyone touching `voice_loop`'s feature gates must keep both `voice-loop` and `android-native` compiling. The GUI guard (`primer-gui --features android-native`, aarch64) now exercises this.
- **The `SpeechEvent` serde shape is a cross-language contract.** Task 7's Kotlin must emit JSON byte-matching the Rust enum's snake_case tags. A mismatch surfaces only on-device (Task 10), not in CI. The `round_trips_through_serialize` test pins the Rust side.
- **~~`nativeInit` symbol resolution / load order~~ — RESOLVED this session.** The symbol links + runs on-device, the JavaVM cache works, and the follow-on `find_class` classloader problem is fixed (`1334e05`) and verified end-to-end via a real `speech_capabilities` return. The whole Plan 1→Task 1 JNI bootstrap is now device-proven; Task 7's bridge methods inherit the working pattern.
- **Branch not yet merged.** Everything this session is on `android-voice-loop-plan2-tasks2-9` (PR #251, open). Confirm #251 merged before building on it.

## Patterns to reuse, not reinvent

- **The android module mirrors `primer-inference::qnn`:** pure logic + a `MockBridge`-driven host test path; the real JNI is `#[cfg(target_os = "android")]` and device-verified (no host test). New pure logic gets TDD; new JNI/Kotlin gets exact code + device verification.
- **`run_loop` is untouched by Android.** It already takes `on_committed_audio`, `wait_for_speaker_drain: Option`, `is_speaking: Option` — Android passes a no-op + `None` + `None`. Do not modify the state machine for Android.
- **`ChannelStt` is the STT seam** (now in `voice_loop::channel_stt`, cpal-free): a `StreamingSpeechToText` whose `finalize()` yields the next channel-delivered transcript. The android recognizer consumer (`run_recognizer_loop`) feeds it via `process_event` + a derived-VAD `event_rx`, same shape as macos26's `run_consumer_loop`.
- **Android device facts (carried, still true):** `~/Library/Android/sdk/platform-tools/adb -s 912607710061`; **logcat is dead on this ROM** — read app-internal files via `run-as org.theprimer.gui cat files/...`; APK build `NDK_HOME=/opt/homebrew/share/android-ndk ~/.cargo/bin/cargo-tauri android build --apk --debug --target aarch64 -- --no-default-features --features android-native`; commits touching `.github/workflows` need `gh auth refresh -s workflow`.
- **NDK cross-compile env (for the GUI guard locally):** put `/opt/homebrew/share/android-ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin` on `PATH` and set `CC_aarch64_linux_android`/`AR_aarch64_linux_android`/`CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER` to the `aarch64-linux-android24-clang` / `llvm-ar` there (the bare `primer-speech` android build needs none of this; only `primer-gui` pulls a cc-rs dep that does).

## Exact commands needed to resume

```bash
cd /Users/hherb/src/primer && git checkout android-voice-loop-plan2-tasks2-9 && git log --oneline -8   # 2faaf02 at HEAD; main at 22d8487

# === PR #251 is open; confirm green + merge ===
gh pr checks 251
gh pr merge 251 --squash   # once green

# === Host gate (what CI runs; all green at handoff) ===
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo fmt --all -- --check
~/.cargo/bin/cargo clippy -p primer-speech --features android-native --all-targets
~/.cargo/bin/cargo test -p primer-speech --features android-native          # 97 lib tests
~/.cargo/bin/cargo test --workspace                                          # full default suite
~/.cargo/bin/cargo test -p primer-speech --features silero,whisper,piper,cpal   # ChannelStt move guard
~/.cargo/bin/cargo build -p primer-speech --features android-native --target aarch64-linux-android

# === GUI android-native cross-compile (drift guard; needs NDK env) ===
NDK_BIN=/opt/homebrew/share/android-ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin
PATH="$NDK_BIN:$PATH" \
  CC_aarch64_linux_android="$NDK_BIN/aarch64-linux-android24-clang" \
  AR_aarch64_linux_android="$NDK_BIN/llvm-ar" \
  CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$NDK_BIN/aarch64-linux-android24-clang" \
  ~/.cargo/bin/cargo build --target aarch64-linux-android --no-default-features -p primer-gui --features android-native

# === Then start Plan 2 device tasks 7, 8, 10 (need the RedMagic; full code in the plan doc) ===
# docs/superpowers/plans/2026-06-18-android-native-voice-loop.md
```

## Reporting back

- State plainly, by acceptance criterion, what compiles/tests and what is device-unverified.
- The owner chose the Android voice POC; continue Plan 2 — the remaining work (Tasks 7, 8, 10) is device-only on the RedMagic.
- All host-mergeable Plan 2 work is now done (Tasks 1–6 + 9). Open PR #251 must be merged before the device tasks build on it.
- The GUI is a full app, not a scaffold — trust the code over any stale "scaffold" phrasing.
