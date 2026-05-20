# `macos-native-26`: SpeechAnalyzer-backed STT on macOS 26+

**Date:** 2026-05-20
**Status:** Design — pending implementation plan
**Builds on:** spike PR #131 (merged; A/B latency probe)

## Problem

The macOS-native voice path on macOS 13+ uses `SFSpeechRecognizer` for STT and depends transitively on the vendored Silero VAD (with ONNX runtime download from cdn.pyke.io) when configured via the cpal path, or runs alongside the Whisper.cpp build on the non-macOS-native path. On macOS 26, Apple shipped a fundamentally faster on-device STT stack — `SpeechAnalyzer` + `SpeechTranscriber` + `SpeechDetector` — that we measured against Whisper.cpp in the A/B probe shipped via #131:

| Metric | macOS 26 SpeechAnalyzer | Whisper `ggml-small.en` |
|---|---|---|
| Time-to-first-partial | ~30 ms | ~3.8 s |
| End-of-speech → FINAL | ~800 ms | ~1.8 s |
| Streaming partials | per-word, predictive | one chunk per utterance |

For a Socratic dialogue loop where the child watches the Primer "think", the streaming win is the headline result.

This spec covers the integration of those APIs into `primer-speech` behind a new `macos-native-26` cargo feature, mutually exclusive with the existing `macos-native`. The macOS path is a low-friction *demo* surface for the project — production children's hardware is non-Apple. Apple-platform iOS is a deliberate near-term target (lead-magnet strategy toward dedicated devices); the design preserves iOS portability with minimal effort. Android is orthogonal — a separate future module sharing nothing with this work.

## Goals

1. **Drop Whisper** when the new feature is on. STT goes through `SpeechAnalyzer`.
2. **Drop Silero + ONNX runtime** when the new feature is on. VAD events derive from `SpeechTranscriber` activity, gated by `SpeechDetector` for power-saving.
3. **Keep TTS** on `AVSpeechSynthesizer` — same as `macos-native` today. No new TTS surface in macOS 26 warrants a new path.
4. **Preserve `LoopBackends` shape and `run_loop` signature.** `LoopBackends` already holds only STT + TTS + locale state; `VadEvent`s flow into `run_loop` as a top-level `mpsc::Receiver<VadEvent>` populated by the audio thread each builder sets up. The new builder's audio thread sources its events from the SpeechAnalyzer pipeline instead of Silero, but the channel shape upstream is unchanged.
5. **Apple-platform-portable inside, macOS-named outside.** `cfg` gates use `target_vendor = "apple"` where the API is platform-uniform; macOS-vs-iOS divergence (AVAudioSession, permission plist plumbing) concentrates in one file. Module rename to `apple26/` deferred to whenever iOS host scaffolding actually lands.
6. **Friction-free demo.** Trust Apple's OS-managed model-asset download on first use; no consent UI.

## Non-goals

- Hindi (`hi-IN`) — `SpeechTranscriber` doesn't support it on macOS 26.5. Locales beyond `en-*` and `de-*` are deferred.
- iOS host application work — Tauri iOS config, App Store distribution, code signing. This spec is `primer-speech`-only. iOS-side scaffolding is a separate future track.
- Android. Different STT framework, no SpeechAnalyzer there; separate `android_native/` module someday.
- Sharing Whisper KV cache across utterances (filed as #133). The macos-native-26 path doesn't use Whisper at all, so the issue is orthogonal.
- Production children's-device path. Not Apple, not part of this design.

## Architecture

### Cargo feature

```toml
# primer-speech/Cargo.toml
macos-native-26 = ["dep:objc2", "dep:objc2-foundation", "dep:objc2-avf-audio", "dep:objc2-speech", "dep:block2"]
```

Same objc2 dep set as `macos-native`. No `dep:silero-vad-rust`, no `dep:ort`, no `dep:whisper-cpp-plus`. Mutually exclusive with `macos-native`:

```rust
// primer-speech/src/lib.rs
#[cfg(all(feature = "macos-native", feature = "macos-native-26"))]
compile_error!(
    "`macos-native` and `macos-native-26` are mutually exclusive — pick one \
     (`macos-native-26` for macOS 26+, `macos-native` for older macOS)"
);
```

Mirrored in `primer-cli/src/main.rs` and `primer-gui/src/lib.rs`.

### Module layout

```
primer-speech/src/macos26/
├── mod.rs           pub use re-exports + module-level rustdoc
├── analyzer.rs      objc2 wrapper around SpeechAnalyzer, SpeechTranscriber, SpeechDetector
├── stt.rs           Macos26Stt + Macos26TranscriptionSession (impl StreamingSpeechToText)
├── vad.rs           DerivedVadStateMachine — pure logic that turns transcriber Result events
│                    into VadEvents; no trait impl, consumed directly by the audio thread
├── locale.rs        primer Locale ↔ BCP47 mapping (en-US, de-DE only)
└── audio_session.rs cfg-split: macOS no-op vs iOS AVAudioSession setup (single file with divergence)
```

`macos26/mod.rs` re-exports `MacosTextToSpeech` from the existing `crate::macos::tts` module — TTS is reused as-is. `macos::permissions` is similarly re-used. The module rustdoc documents the Apple-platform-portability intent (most files use `cfg(target_vendor = "apple")`; rename to `apple26/` is mechanical when iOS scaffolding lands).

### New voice-loop builder

The existing builders return `(LoopBackends, mpsc::Receiver<VadEvent>)` (and similar handles) — `run_loop` takes both. The new builder matches that shape:

```rust
// primer-speech/src/voice_loop/backends.rs
#[cfg(all(target_vendor = "apple", feature = "macos-native-26"))]
pub async fn build_local_backends_macos_native_26(
    /* same params as build_local_backends_macos_native */
) -> Result<(LoopBackends, /* event/handle tuple matching siblings */)> {
    // 1. cpal mic capture (existing path) -> producer side of an AVAudioPCMBuffer queue
    // 2. Resample to 16 kHz, wrap into AVAudioPCMBuffer chunks
    // 3. Construct SpeechAnalyzer with
    //    [SpeechDetector(.medium), SpeechTranscriber(.progressiveTranscription)]
    // 4. Spawn the audio-handler thread (same pattern as the macos-native and
    //    cpal-whisper builders use today). Inside it:
    //    a. own the AsyncSequence<AnalyzerInput> producer
    //    b. consume transcriber.results in a loop
    //    c. for each result: feed it to DerivedVadStateMachine; push any
    //       resulting VadEvent onto the top-level event mpsc; push the text
    //       onto the ChannelStt mpsc
    //    d. on a separate 100 ms tokio interval: tick the state machine and
    //       push any timer-driven SpeechEnd onto the event mpsc
    // 5. Construct LoopBackends::single_locale(
    //       stt: Arc<ChannelStt>,
    //       tts: Arc<MacosTextToSpeech>,   // reused from macos-native
    //       voice, locale,
    //    )
}
```

Same return shape and call sites as `build_local_backends_macos_native`. CLI/GUI runtime selection adds a third `cfg!` arm. **No new trait or struct shows up in `LoopBackends` itself** — the change is entirely inside the builder and its audio thread.

## Derived-VAD state machine

The core challenge: `SpeechTranscriber.results` is an `AsyncSequence<Result>`. `run_loop` expects `VadEvent::SpeechStart` / `SpeechEnd` on the top-level event channel. We need a deterministic translation, implemented as a pure-logic struct that the audio thread feeds events into.

**State machine (in `macos26/vad.rs`):**

```text
state = Idle
on transcriber.Result:
    case Idle, non-empty text:
        emit SpeechStart; last_partial_at = now; state = Speaking
    case Speaking, isFinal:
        emit SpeechEnd; state = Idle
    case Speaking, non-empty partial:
        last_partial_at = now

every EVENT_POLL_INTERVAL_MS (driven by tokio::time::interval in the audio task):
    if state == Speaking and (now - last_partial_at) > SPEECH_END_TIMEOUT_MS:
        emit SpeechEnd; state = Idle
```

**Why two end signals (isFinal *and* timer)?** SpeechTranscriber emits `isFinal` when the model commits an utterance — typically after a sustained pause. If the user trails off and never triggers a final, the timer guards the LATENT_THINK transition. In practice `isFinal` wins; the timer is a safety net.

**Tunable thresholds in `primer_core::consts::speech::macos26`:**

| Const | Default | Rationale |
|---|---|---|
| `SPEECH_START_MIN_TEXT_CHARS` | `1` | Empty/whitespace partials don't count |
| `SPEECH_END_TIMEOUT_MS` | `600` | 2× Silero's 300 ms — partials don't fire during true silence even mid-utterance, so a longer gap is fine |
| `EVENT_POLL_INTERVAL_MS` | `100` | Cadence for the inactivity timer |

**Struct shape (no trait impl — pure logic, consumed directly):**

```rust
pub struct DerivedVadStateMachine {
    state: State,
    last_partial_at: Option<Instant>,
    cfg: DerivedVadConfig,  // SPEECH_END_TIMEOUT_MS etc.
}

impl DerivedVadStateMachine {
    pub fn on_result(&mut self, text: &str, is_final: bool, now: Instant) -> Option<VadEvent> { … }
    pub fn tick(&mut self, now: Instant) -> Option<VadEvent> { … }
    pub fn reset(&mut self) { … }
}
```

The audio thread owns one instance, calls `on_result` per transcriber `Result`, calls `tick` on a 100 ms interval, and pushes any returned `VadEvent` onto the top-level `mpsc::Sender<VadEvent>` it already received from the builder. **No new trait, no `LoopBackends` change.**

### Channel plumbing

```text
[cpal mic stream]
       │ samples
       ▼
[resampler → AVAudioPCMBuffer chunks via objc2]
       │
       ▼
[SpeechAnalyzer.start(inputSequence:)]   ◄── inside the audio thread for this builder
       │ AsyncSequence<Result>
       ▼
[audio thread loop: on_result + tick]    ◄── owns DerivedVadStateMachine
       │                       │
       │ text segments         │ VadEvent
       ▼                       ▼
[ChannelStt mpsc]         [top-level event mpsc]
       │                       │
       └──────► run_loop ◄─────┘
```

### Edge cases handled

- **Predictive partials with negative lag** (observed in the probe) — fine: SpeechStart fires on first non-empty partial, *earlier* than Silero. Net latency improvement.
- **Intra-sentence silences** — partials arrive every ~100–300 ms during continuous speech. The 600 ms timer sits comfortably above natural inter-word pauses.
- **Abandoned utterances** (user trails off, never `isFinal`) — timer catches them.
- **Barge-in** (child resumes while Primer SPEAKs) — first new partial fires SpeechStart, voice_loop cancels TTS just as it does for Silero today.
- **Reset between utterances** — `DerivedVadStateMachine::reset()` zeroes state to `Idle` so a new session starts clean.

## Locale and asset handling

`macos26/locale.rs`:

```rust
pub fn to_bcp47(locale: Locale) -> Result<&'static str> {
    match locale {
        Locale::English => Ok("en-US"),
        Locale::German  => Ok("de-DE"),
        Locale::Hindi   => Err(PrimerError::Speech(
            "Hindi (hi-IN) not yet supported by SpeechTranscriber on macOS 26.5; \
             use --features primer-cli/speech without macos-native-26 for the Whisper path".into()
        )),
    }
}
```

Single source of truth. Validation happens at `Macos26Stt::new(...)`, not mid-conversation. Error message names the workaround.

**Asset download** — silent and OS-managed (per the project's friction-free demo stance). `Macos26Stt::new(locale)`:

1. Check `SpeechTranscriber.supportedLocales` — hard error if locale not present (covers the `hi-IN` case redundantly).
2. Check `SpeechTranscriber.installedLocales` — if present, return immediately.
3. Otherwise call `AssetInventory.assetInstallationRequest(supporting:).downloadAndInstall()` — Apple's OS surface handles UI/progress.

One `tracing::info!` at start of download so `RUST_LOG=info` developers can see it. No consent prompt, no GUI modal, no `disable_auto_download` flag for the macOS-native-26 path. (The strict-offline-first stance still applies to production children's hardware, which is non-Apple; the macOS demo path is explicitly friction-prioritised.)

## Test strategy

### Unit (host-independent, run everywhere)

- `macos26::vad::tests` — pure state-machine tests. Drive `DerivedVadStateMachine` with scripted `(now, on_result(text, is_final))` / `(now, tick())` calls and assert the `Option<VadEvent>` return. ~5 tests:
  - SpeechStart on first non-empty partial after Idle.
  - SpeechEnd on `isFinal`.
  - SpeechEnd on inactivity timer (no partials for `> SPEECH_END_TIMEOUT_MS`).
  - `reset()` returns to Idle (next non-empty partial fires SpeechStart cleanly).
  - Mid-Speaking partials extend the timer (no spurious SpeechEnd).
- `macos26::locale::tests` — round-trip mapping pinned, including the Hindi-rejection error string.

### Integration (gated on macOS + feature)

- `macos26::analyzer::tests::round_trip_smoke` — construct `Macos26Stt`, open a session, feed a known short WAV, drive to completion, assert at least one segment came out. **Not in CI by default**; gated `#[ignore]` like the existing `fastembed` test. Developer runs explicitly:
  ```bash
  cargo test -p primer-speech --features macos-native-26 -- --ignored round_trip_smoke
  ```

### Manual smoke (documented in CLAUDE.md)

```bash
cargo run --features primer-cli/speech,primer-cli/macos-native-26 \
    --bin primer -- --speech --language en --name TestKid --age 8
```

Round-trip a "what colour is the sky" exchange against `--backend stub`. Reference implementation for parameter choices stays the merged spike at `spikes/macos26_speech/` (preset `.progressiveTranscription`, SpeechDetector sensitivity `.medium`).

## Build prerequisites

- Xcode 17+ with macOS 26 SDK (`xcrun --show-sdk-version` reports `26.x`).
- Rust 1.88+ (already pinned at workspace level).
- **No espeak-ng required** when `macos-native-26` is the only speech feature (piper is not pulled).
- **No ONNX runtime download.**
- **No whisper.cpp C++ build.**

Cold build under `macos-native-26` only is minutes faster than under `macos-native`+`whisper` for the same workspace.

## Implementation order

Captured as proposed work items for the writing-plans skill (not commitments):

1. `objc2-speech` binding probe — verify `SpeechAnalyzer` / `SpeechTranscriber` / `SpeechDetector` types are exposed by the published crate; write minimal FFI shims via raw `objc2` if not.
2. `macos26/analyzer.rs` — Rust wrapper around the Speech FFI; mirrors the merged Swift spike's setup (`.progressiveTranscription`, `.medium` SpeechDetector). One async function `run(transcriber_results_sink, vad_events_sink)`.
3. `macos26/vad.rs` — `DerivedVadStateMachine` + unit tests.
4. `macos26/locale.rs` + `macos26/stt.rs` + `macos26/audio_session.rs` (the last cfg-split between macOS no-op and iOS AVAudioSession).
5. `macos26/mod.rs` with TTS / permissions re-exports.
6. `voice_loop::backends::build_local_backends_macos_native_26` glue.
7. CLI/GUI feature wiring + mutual-exclusion `compile_error!`.
8. Integration smoke (`#[ignore]`).
9. CLAUDE.md note documenting the build flags, the A/B latency claim, the Apple-portability intent of the module, and the deferred `apple26/` rename.

## Branch strategy

Per the project owner's request: develop in a dedicated branch (e.g. `claude/macos-native-26`) until tested and working end-to-end. Merge to `main` only after manual smoke confirms a clean round-trip on macOS 26.5.
