# Android-Native Voice POC (RedMagic) — Design

**Date:** 2026-06-18
**Status:** approved (owner-paired brainstorming)
**Scope:** First on-device voice round-trip inside the Android APK — OS-native STT + TTS, English only.
**Phase:** delivers the Phase 2 *and* Phase 3 exit demo ("a child can have the conversation entirely by voice" / "turn on the device and converse with no other equipment").

## Context

The Tauri-Android APK already runs full multi-turn Socratic conversations on the Hexagon NPU
(`QnnBackend`, ~25 tok/s, stable across turns — PRs #218/#222/#227). What it cannot yet do is
**talk**: voice mode works on desktop (CLI `--speech` + GUI) via Silero VAD + Whisper STT + Piper
TTS, but on Android the `speech` feature is off entirely — no cpal, whisper.cpp, Piper, or `ort`
compiled in.

This POC extends the working APK to a complete voice turn, on-device and offline, on the RedMagic
11 Pro (NX809J, Android 16 / API 36).

### Why the silicon is not the constraint

The voice loop is strictly **LISTEN → LATENT_THINK → SPEAK** with no barge-in (a deliberate
pedagogical invariant, `[[project_no_barge_in_pedagogy]]`). That makes the heavy voice stages
**temporally disjoint from NPU decode**:

- **LISTEN** — STT runs; NPU idle.
- **LATENT_THINK** — Qwen3-4B on the NPU; voice subsystem idle.
- **SPEAK** — TTS runs; NPU idle (only streaming-TTS lightly overlaps the token tail).

STT and the LLM never run at the same instant, so voice compute on CPU/GPU does not contend with
the NPU for thermal/bandwidth budget. On a Snapdragon 8 Elite Gen 5 with 24 GB RAM this is a
non-event. **The real risk was never compute — it was the software stack on Android.**

### Device recon (read-only adb, 2026-06-18, the actual RedMagic)

`SpeechRecognizer` providers on the device:

```
com.google.android.as/...AiAiSpeechRecognitionService    ← on-device (SODA / Android System Intelligence)
com.google.android.tts/...GoogleTTSRecognitionService     ← network fallback
com.anthropic.claude/.bell.assist.ClaudeRecognitionService
```

`TextToSpeech` engine providers:

```
com.google.android.tts/...GoogleTtsService                ← Google TTS (ships offline en-US voices)
```

`com.google.android.as` (Android System Intelligence) is **installed and enabled**
(`installed=true`, `enabled=0`=default/enabled, `stopped=false`) and advertises an **on-device**
`RecognitionService` — the SODA model behind Live Caption / Recorder / Gboard dictation.
`SpeechRecognizer.createOnDeviceSpeechRecognizer()` binds to it. This is the load-bearing finding:
**on-device offline STT + TTS is genuinely available on this ROM.**

adb cannot return three facts (they need an in-app API call): the literal
`isOnDeviceRecognitionAvailable()` boolean, the on-device-supported locale list, and which
installed TTS voices report `isNetworkConnectionRequired() == false`. These are confirmed by the
Section 3 diagnostic before the full loop is built.

## Chosen approach

**OS-native primary** — Android on-device `SpeechRecognizer` (ASI/SODA) for STT + Android
`TextToSpeech` (offline voice) for TTS. Zero `ort`, zero whisper.cpp, zero Piper, zero espeak-ng,
zero cpal. Architecturally the Android analogue of the existing `macos-native-26` path (the OS owns
the mic + endpointing; VAD is derived from recognizer events; TTS plays itself).

Rejected / deferred:

- **B — Whisper STT + energy VAD + native TTS** (the prior option-2 plan). Reused more desktop code
  but bundles a ~466 MB Whisper model and writes an energy VAD. Now the **fallback** for locales
  ASI on-device doesn't cover (e.g. German), not the primary. Designed here, not built.
- **C — Silero VAD / Piper TTS on Android** (`ort`). The exact `ort`-on-`aarch64-linux-android`
  bet (#157) we are avoiding; if `ort` fails to load on-device the whole loop dies. Out.
- **Pure `jni-rs` from Rust** for the speech APIs (matching the `macos-native` objc2 style).
  Rejected: the Android speech APIs are callback-heavy and main-thread/Looper-bound; raw JNI means
  manual thread-attach, GlobalRef listener objects, and a hand-pumped Looper. A Kotlin plugin
  handles this naturally and is the documented Tauri-mobile native-plugin path.

## Design

### 1. Backend — `StreamingSpeechToText` + `StreamingTextToSpeech` on Android

Add an Android-native speech backend that satisfies the **existing** `primer-core` speech traits,
so the shared `primer_speech::voice_loop` state machine, the no-barge-in gating, and the GUI's
`primer://voice/*` events all work unchanged — exactly how `macos-native` plugs in for Apple.

- **STT** wraps `SpeechRecognizer.createOnDeviceSpeechRecognizer()`. `RecognitionListener`
  partial results → `SpeechStart` + transcript stream; final result / endpoint → `SpeechEnd`. The
  **VAD is derived** from these events (`macos-native-26` pattern) — no separate VAD, no cpal mic.
- **TTS** wraps `TextToSpeech`. At init, enumerate voices for the locale and select one where
  `isNetworkConnectionRequired() == false`; **hard-error if none** (never fall back to a network
  voice — `[[project_strict_offline_first]]`). `UtteranceProgressListener.onDone` returns the loop
  to LISTEN. TTS plays through Android's own audio output — **no cpal speaker, no ringbuf-drain
  machinery** (the most fragile part of the desktop loop is simply absent here).

### 2. Bridge — Kotlin Tauri-mobile plugin

The Java/Kotlin speech plumbing lives in a **Kotlin Tauri-mobile plugin** (the Android analogue of
the `macos-native-26` Swift sidecar). It owns the main-thread/Looper-bound recognizer + synthesizer
lifecycle and exposes commands (`startListening`, `stopListening`, `speak`, `cancel`,
`queryCapabilities`) and events (partial, final, sttError, ttsStart, ttsDone, ttsError). A thin
Rust adapter translates those into the `StreamingSpeechToText` / `StreamingTextToSpeech` trait
shapes consumed by `voice_loop`. **This Rust↔Kotlin bridge is the primary implementation risk; the
plan validates it first** (a minimal "Kotlin command round-trips to Rust and back" smoke before any
speech logic).

### 3. De-risking first deliverable — in-APK capability diagnostic

Before wiring the loop, ship one Tauri command (surfaced in Settings → Diagnostics, mirroring the
existing QNN-metrics opt-in) that calls the real APIs and reports:

- `SpeechRecognizer.isOnDeviceRecognitionAvailable(context)`
- on-device-supported locales (where determinable)
- installed `TextToSpeech` voices and their `isNetworkConnectionRequired()` flag

Read back over adb (`run-as` / logcat). This converts the last three adb-unanswerable unknowns into
hard facts and doubles as the "can our app context reach these Java APIs at all" smoke. Its result
confirms or adjusts Section 1 before the full loop is built. **If `isOnDeviceRecognitionAvailable`
is false or no offline en-US voice exists, the POC stops here and re-scopes to fallback B** rather
than building on a false premise.

### 4. Feature gating & build

- New cargo feature (working name `android-native`) on `primer-speech`, propagated through
  `primer-gui`, that compiles the Android-native backend + the Rust bridge adapter. Off by default;
  the default Android APK feature set stays BM25-only with no speech (mirrors #157).
- The Kotlin plugin is part of the `gen/android` Gradle project (committed, like the rest of the
  Android scaffold).
- Microphone permission (`RECORD_AUDIO`) added to the Android manifest + runtime-permission request
  on first voice use.

### 5. Acceptance

On the RedMagic, in the APK, **in airplane mode** (proving fully offline): tap voice mode, speak an
English question → transcribed on-device by ASI/SODA → answered by the NPU (`QnnBackend`) → spoken
aloud by `TextToSpeech` → loop returns to LISTEN. One complete voice turn, no other equipment, no
network. This is simultaneously the Phase 2 and Phase 3 exit demo.

Regression guards (must stay green): desktop `primer-gui` build/test/fmt/clippy unchanged; the
Android `--features qnn` cross-compile drift-guard unchanged; the new `android-native` feature
cross-compiles for `aarch64-linux-android` in CI (lib-only, no Gradle, mirroring the existing qnn
guard).

## Scope

**In scope:** English-only on-device STT (ASI/SODA) + offline TTS, derived VAD, Kotlin bridge
plugin, capability diagnostic, mic permission, one offline voice turn on-device, the
`android-native` feature + CI drift-guard.

**Out of scope (deferred increments):** German (and the Whisper + energy-VAD fallback that German
likely needs), barge-in either direction, Whisper/Piper/Silero/`ort`/cpal on Android, voice-quality
or latency tuning, the Phase-B "big central ear/mouth" production voice UI
(`[[project_gui_voice_phased_visual]]` — this POC reuses the existing composer-zone voice widget).

## Risks

- **Rust↔Kotlin bridge complexity.** Primary risk. Mitigation: a minimal round-trip smoke before
  any speech logic; the capability diagnostic (Section 3) is the first real exercise of it.
- **On-device STT behaviour quirks.** `SpeechRecognizer` may auto-timeout, cap utterance length, or
  require re-arming per turn. Acceptable for a turn-based Socratic loop (re-arm in LISTEN); tune in
  the plan if it bites.
- **Offline en-US voice not installed.** Possible on a fresh ROM. Detected by the diagnostic;
  resolution is a one-time voice install (a setup step, not a runtime network dependency — within
  `[[project_strict_offline_first]]`).
- **NubiaOS recognizer routing.** The device default `voice_recognition_service` is currently the
  Google TTS (network) service; `createOnDeviceSpeechRecognizer()` explicitly bypasses the default
  and targets the on-device ASI service, so the default setting does not gate us. Confirmed by the
  diagnostic.
- **Permission/AppOps on this ROM.** NubiaOS may handle `RECORD_AUDIO` runtime grants differently;
  verify on first device run.
