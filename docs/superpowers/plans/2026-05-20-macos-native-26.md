# macos-native-26 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** New `macos-native-26` cargo feature on `primer-speech` that swaps in macOS 26's SpeechAnalyzer/SpeechTranscriber/SpeechDetector for STT+VAD via a Swift sidecar (swift-bridge), keeping AVSpeechSynthesizer TTS reused from `macos-native`. Mutually exclusive with `macos-native`.

**Architecture:** Swift sidecar class `Macos26Pipeline` owns the SpeechAnalyzer pipeline; Rust drives it via `next_result().await` in a loop. A `DerivedVadStateMachine` translates transcriber events to `VadEvent::SpeechStart` / `SpeechEnd`. New `build_local_backends_macos_native_26` wires it into the existing `LoopBackends` shape — no trait or struct changes upstream.

**Tech Stack:** Rust 1.88, Swift 6 + macOS 26 SDK, `swift-bridge` 0.1, `tokio` mpsc channels, `cpal` for mic capture (unchanged).

**Spec:** [`docs/superpowers/specs/2026-05-20-macos-native-26-design.md`](../specs/2026-05-20-macos-native-26-design.md)

**Worktree / branch:** `/Users/hherb/src/primer/.claude/worktrees/wizardly-shtern-bb8824/` on branch `claude/macos-native-26`. All `cargo` commands run from `src/` unless noted.

---

## File map

| File | Created/Modified | Responsibility |
|---|---|---|
| `src/crates/primer-speech/Cargo.toml` | Modify | Declare `macos-native-26` feature + swift-bridge deps |
| `src/crates/primer-speech/src/lib.rs` | Modify | `compile_error!` for mutual exclusion; register `macos26` module |
| `src/crates/primer-speech/build.rs` | Create | swift-bridge-build + swiftc invocation, gated on feature |
| `src/crates/primer-speech/swift-sources/Macos26Pipeline.swift` | Create | Swift sidecar — owns SpeechAnalyzer pipeline |
| `src/crates/primer-speech/src/macos26/mod.rs` | Create | Module entry + re-exports |
| `src/crates/primer-speech/src/macos26/bridge.rs` | Create | `#[swift_bridge::bridge]` module |
| `src/crates/primer-speech/src/macos26/analyzer.rs` | Create | Rust-side driver loop over `next_result()` |
| `src/crates/primer-speech/src/macos26/stt.rs` | Create | `Macos26Stt` + `Macos26TranscriptionSession` (impl `StreamingSpeechToText`) |
| `src/crates/primer-speech/src/macos26/vad.rs` | Create | `DerivedVadStateMachine` + unit tests |
| `src/crates/primer-speech/src/macos26/locale.rs` | Create | `Locale` → BCP47 mapping + tests |
| `src/crates/primer-speech/src/macos26/audio_session.rs` | Create | cfg-split stub for iOS AVAudioSession |
| `src/crates/primer-core/src/consts.rs` | Modify | Add `speech::macos26` consts |
| `src/crates/primer-speech/src/voice_loop/backends.rs` | Modify | Add `build_local_backends_macos_native_26` |
| `src/crates/primer-cli/Cargo.toml` | Modify | Propagate `macos-native-26` feature |
| `src/crates/primer-cli/src/main.rs` | Modify | Runtime cfg arm + `compile_error!` |
| `src/crates/primer-gui/Cargo.toml` | Modify | Propagate feature |
| `src/crates/primer-gui/src/lib.rs` | Modify | `compile_error!` |
| `CLAUDE.md` | Modify | Note about build flags + A/B numbers + deferred `apple26/` rename |

---

### Task 1: Cargo feature scaffold + mutual exclusion

**Files:**
- Modify: `src/crates/primer-speech/Cargo.toml`
- Modify: `src/crates/primer-speech/src/lib.rs`
- Create: `src/crates/primer-speech/src/macos26/mod.rs`

- [ ] **Step 1: Add the swift-bridge dep declarations to `primer-speech/Cargo.toml`**

Insert after the existing optional `objc2-*` block (around line 50, follow the format of other optional deps):

```toml
# swift-bridge — generates the FFI between Rust and the Macos26Pipeline Swift sidecar.
# Pulled only by the `macos-native-26` feature. The build-dep is the codegen half.
swift-bridge = { version = "0.1", optional = true }
```

Add to the `[build-dependencies]` table at the bottom (create the table if it doesn't exist):

```toml
[build-dependencies]
swift-bridge-build = { version = "0.1", optional = true }
```

Add the new feature line in the `[features]` block, immediately under the existing `macos-native = [...]` line:

```toml
# macOS 26+ STT/VAD via SpeechAnalyzer. Swift sidecar bridged via swift-bridge.
# Mutually exclusive with macos-native — pick one.
macos-native-26 = [
    "dep:swift-bridge",
    "swift-bridge-build",
    "dep:objc2-foundation",
    "dep:objc2-avf-audio",
]
```

(The `"swift-bridge-build"` entry without `dep:` activates the build-dep — Cargo's syntax for opting build-deps into a feature.)

- [ ] **Step 2: Add the mutual-exclusion compile_error and the module gate to `primer-speech/src/lib.rs`**

Append near the top of the file, after the existing `#![...]` attributes and before the first `pub mod` line:

```rust
#[cfg(all(feature = "macos-native", feature = "macos-native-26"))]
compile_error!(
    "`macos-native` and `macos-native-26` are mutually exclusive — pick one \
     (`macos-native-26` for macOS 26+, `macos-native` for older macOS)"
);
```

And in the same file, alongside the existing `#[cfg(...)] pub mod macos;` line, add:

```rust
#[cfg(all(target_vendor = "apple", feature = "macos-native-26"))]
pub mod macos26;
```

- [ ] **Step 3: Create the empty module skeleton**

Create `src/crates/primer-speech/src/macos26/mod.rs` with the following contents:

```rust
//! macOS 26 / iOS 26 SpeechAnalyzer-backed STT + VAD.
//!
//! See [`docs/superpowers/specs/2026-05-20-macos-native-26-design.md`].
//!
//! Cfg gates in this module use `target_vendor = "apple"` rather than
//! `target_os = "macos"` because the underlying APIs are identical
//! across all Apple platforms (iOS 26+, iPadOS 26+, visionOS 26+, tvOS 26+,
//! macOS 26+). Files that genuinely diverge between macOS and iOS
//! concentrate that divergence in [`audio_session`].
//!
//! Module rename to `apple26/` is a mechanical follow-up once an iOS
//! host application actually exists in the repo — see the design doc
//! "Goals" section.

// Sub-modules land in subsequent plan tasks.
```

- [ ] **Step 4: Verify the build still works without the new feature**

Run from `src/`:
```bash
~/.cargo/bin/cargo build -p primer-speech
```
Expected: clean build (no warnings, no errors). The new feature is opt-in; nothing else changed.

- [ ] **Step 5: Verify the mutual-exclusion message fires**

```bash
~/.cargo/bin/cargo build -p primer-speech --features macos-native,macos-native-26 2>&1 | grep -i 'mutually exclusive'
```
Expected: prints the compile_error message verbatim.

- [ ] **Step 6: Commit**

```bash
git add src/crates/primer-speech/Cargo.toml \
        src/crates/primer-speech/src/lib.rs \
        src/crates/primer-speech/src/macos26/mod.rs
git commit -m "speech(macos26): scaffold macos-native-26 feature + mutual-exclusion gate"
```

---

### Task 2: macos26 tunable consts in primer-core

**Files:**
- Modify: `src/crates/primer-core/src/consts.rs`

- [ ] **Step 1: Locate the consts.rs `speech` module**

Run from `src/`:
```bash
grep -n 'pub mod speech\|pub mod silero\|pub mod whisper' crates/primer-core/src/consts.rs | head -5
```
This tells you where the speech-related const modules live so the new `macos26` sub-module slots in alongside them.

- [ ] **Step 2: Add the `speech::macos26` const module**

Append to `crates/primer-core/src/consts.rs` (inside the existing `pub mod speech { ... }` block if there is one, or as a sibling module if speech is itself top-level):

```rust
/// Tunable thresholds for the macos-native-26 derived-VAD state machine.
/// See `crates/primer-speech/src/macos26/vad.rs` and the design doc at
/// `docs/superpowers/specs/2026-05-20-macos-native-26-design.md`.
pub mod macos26 {
    use std::time::Duration;

    /// Empty or whitespace-only transcriber partials don't fire SpeechStart;
    /// at least this many non-whitespace characters must be present.
    pub const SPEECH_START_MIN_TEXT_CHARS: usize = 1;

    /// Inactivity threshold after which the state machine emits SpeechEnd
    /// even if the transcriber never sent `isFinal`. Set to 2× Silero's
    /// 300 ms default because SpeechTranscriber partials don't fire during
    /// true silence even mid-utterance, so a longer gap is safe.
    pub const SPEECH_END_TIMEOUT: Duration = Duration::from_millis(600);

    /// Cadence at which the audio task ticks the state machine to check
    /// for inactivity-driven SpeechEnd. Anything under `SPEECH_END_TIMEOUT`
    /// keeps the worst-case detection latency under 2× this value.
    pub const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
}
```

- [ ] **Step 3: Verify primer-core still builds and tests pass**

```bash
~/.cargo/bin/cargo test -p primer-core --lib
```
Expected: existing tests pass; no new tests in this task.

- [ ] **Step 4: Commit**

```bash
git add src/crates/primer-core/src/consts.rs
git commit -m "core(consts): macos26 derived-VAD thresholds"
```

---

### Task 3: DerivedVadStateMachine (TDD)

**Files:**
- Create: `src/crates/primer-speech/src/macos26/vad.rs`
- Modify: `src/crates/primer-speech/src/macos26/mod.rs`

- [ ] **Step 1: Register the new module**

Append to `crates/primer-speech/src/macos26/mod.rs`:

```rust
pub mod vad;
```

- [ ] **Step 2: Write the failing tests first**

Create `crates/primer-speech/src/macos26/vad.rs` with **tests only** at first — no implementation yet:

```rust
//! Pure-logic state machine that translates SpeechTranscriber `Result`
//! events into the `VadEvent`s that `voice_loop::run_loop` consumes.
//! No FFI, no I/O, no async — easy to unit-test.

#![cfg(all(target_vendor = "apple", feature = "macos-native-26"))]

use std::time::Instant;

use primer_core::consts::speech::macos26::{
    EVENT_POLL_INTERVAL, SPEECH_END_TIMEOUT, SPEECH_START_MIN_TEXT_CHARS,
};
use primer_core::speech::VadEvent;

/// Internal state of the derived VAD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Speaking,
}

/// Pure-logic state machine driven by the audio task's `on_result` calls
/// and a periodic `tick`. Returns `Some(VadEvent)` when a transition fires;
/// the caller is responsible for pushing it onto the top-level event mpsc.
pub struct DerivedVadStateMachine {
    state: State,
    last_partial_at: Option<Instant>,
}

impl DerivedVadStateMachine {
    pub fn new() -> Self {
        Self { state: State::Idle, last_partial_at: None }
    }

    /// Reset to `Idle` between utterances.
    pub fn reset(&mut self) {
        self.state = State::Idle;
        self.last_partial_at = None;
    }

    /// Handle a transcriber Result. Returns a VadEvent if the result
    /// triggers a state transition, otherwise None.
    pub fn on_result(
        &mut self,
        text: &str,
        is_final: bool,
        now: Instant,
    ) -> Option<VadEvent> {
        let _ = (text, is_final, now);
        unimplemented!("Step 4")
    }

    /// Periodic tick from the audio task. Returns a VadEvent if the
    /// inactivity timer fires, otherwise None.
    pub fn tick(&mut self, now: Instant) -> Option<VadEvent> {
        let _ = now;
        unimplemented!("Step 4")
    }
}

impl Default for DerivedVadStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(ms: u64) -> Instant {
        // Use a fixed base + offset so tests are deterministic without
        // calling Instant::now() in production code paths.
        thread_local! {
            static BASE: Instant = Instant::now();
        }
        BASE.with(|b| *b + Duration::from_millis(ms))
    }

    #[test]
    fn first_non_empty_partial_emits_speech_start() {
        let mut sm = DerivedVadStateMachine::new();
        assert_eq!(sm.on_result("hello", false, at(100)), Some(VadEvent::SpeechStart));
    }

    #[test]
    fn empty_partial_does_not_emit_speech_start() {
        let mut sm = DerivedVadStateMachine::new();
        assert_eq!(sm.on_result("", false, at(100)), None);
        assert_eq!(sm.on_result("   ", false, at(150)), None);
    }

    #[test]
    fn is_final_emits_speech_end() {
        let mut sm = DerivedVadStateMachine::new();
        assert_eq!(sm.on_result("hello", false, at(100)), Some(VadEvent::SpeechStart));
        assert_eq!(sm.on_result("hello world.", true, at(2000)), Some(VadEvent::SpeechEnd));
    }

    #[test]
    fn inactivity_timer_emits_speech_end() {
        let mut sm = DerivedVadStateMachine::new();
        assert_eq!(sm.on_result("hello", false, at(100)), Some(VadEvent::SpeechStart));
        // Tick before the timeout — no event.
        assert_eq!(sm.tick(at(100 + 300)), None);
        // Tick after the timeout — SpeechEnd.
        let timeout_ms = SPEECH_END_TIMEOUT.as_millis() as u64;
        assert_eq!(sm.tick(at(100 + timeout_ms + 50)), Some(VadEvent::SpeechEnd));
    }

    #[test]
    fn mid_speaking_partials_extend_timer() {
        let mut sm = DerivedVadStateMachine::new();
        let timeout_ms = SPEECH_END_TIMEOUT.as_millis() as u64;
        assert_eq!(sm.on_result("hello", false, at(0)), Some(VadEvent::SpeechStart));
        // Partial just before the timer would fire — extends last_partial_at.
        assert_eq!(sm.on_result("hello world", false, at(timeout_ms - 100)), None);
        // Original deadline has now passed but the new partial reset the timer.
        assert_eq!(sm.tick(at(timeout_ms + 50)), None);
        // True timeout from the new partial.
        assert_eq!(sm.tick(at(timeout_ms - 100 + timeout_ms + 50)), Some(VadEvent::SpeechEnd));
    }

    #[test]
    fn reset_returns_to_idle() {
        let mut sm = DerivedVadStateMachine::new();
        sm.on_result("hello", false, at(0));
        sm.reset();
        // After reset, next non-empty partial fires SpeechStart cleanly.
        assert_eq!(sm.on_result("hi", false, at(2000)), Some(VadEvent::SpeechStart));
    }

    #[test]
    fn min_text_chars_threshold_respected() {
        // The const lives in primer-core::consts::speech::macos26.
        // Pin its value so a future bump explicitly re-runs these tests.
        assert_eq!(SPEECH_START_MIN_TEXT_CHARS, 1);
        // Sanity: POLL interval is under timeout — otherwise the timer
        // logic would have terrible worst-case latency.
        assert!(EVENT_POLL_INTERVAL < SPEECH_END_TIMEOUT);
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
~/.cargo/bin/cargo test -p primer-speech --features macos-native-26 --lib macos26::vad
```
Expected: All `on_result` / `tick` tests panic with `unimplemented!("Step 4")`. The pin-the-const test passes.

- [ ] **Step 4: Implement the state machine**

Replace the two `unimplemented!()` bodies in `vad.rs`:

```rust
pub fn on_result(
    &mut self,
    text: &str,
    is_final: bool,
    now: Instant,
) -> Option<VadEvent> {
    let non_empty = text.trim().chars().count() >= SPEECH_START_MIN_TEXT_CHARS;
    match self.state {
        State::Idle => {
            if non_empty {
                self.state = State::Speaking;
                self.last_partial_at = Some(now);
                Some(VadEvent::SpeechStart)
            } else {
                None
            }
        }
        State::Speaking => {
            if is_final {
                self.state = State::Idle;
                self.last_partial_at = None;
                Some(VadEvent::SpeechEnd)
            } else {
                if non_empty {
                    self.last_partial_at = Some(now);
                }
                None
            }
        }
    }
}

pub fn tick(&mut self, now: Instant) -> Option<VadEvent> {
    if self.state != State::Speaking {
        return None;
    }
    let Some(last) = self.last_partial_at else {
        return None;
    };
    if now.duration_since(last) > SPEECH_END_TIMEOUT {
        self.state = State::Idle;
        self.last_partial_at = None;
        Some(VadEvent::SpeechEnd)
    } else {
        None
    }
}
```

- [ ] **Step 5: Run the tests again to verify they pass**

```bash
~/.cargo/bin/cargo test -p primer-speech --features macos-native-26 --lib macos26::vad
```
Expected: 7 tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/crates/primer-speech/src/macos26/{mod.rs,vad.rs}
git commit -m "speech(macos26): derived-VAD state machine + unit tests"
```

---

### Task 4: Locale ↔ BCP47 mapping (TDD)

**Files:**
- Create: `src/crates/primer-speech/src/macos26/locale.rs`
- Modify: `src/crates/primer-speech/src/macos26/mod.rs`

- [ ] **Step 1: Register the module**

Append to `crates/primer-speech/src/macos26/mod.rs`:

```rust
pub mod locale;
```

- [ ] **Step 2: Write failing tests**

Create `crates/primer-speech/src/macos26/locale.rs`:

```rust
//! Locale ↔ BCP47 mapping for the `macos-native-26` STT path.
//! Single source of truth; `Macos26Stt::new` calls `to_bcp47` once
//! at construction so a wrong locale is a startup-time error, not a
//! mid-conversation one.

#![cfg(all(target_vendor = "apple", feature = "macos-native-26"))]

use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;

/// Translate a primer `Locale` into the BCP47 string `SpeechTranscriber`
/// expects. Errors loudly on locales SpeechTranscriber doesn't support
/// (Hindi on macOS 26.5).
pub fn to_bcp47(locale: Locale) -> Result<&'static str> {
    let _ = locale;
    unimplemented!("Step 4")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_maps_to_en_us() {
        assert_eq!(to_bcp47(Locale::English).unwrap(), "en-US");
    }

    #[test]
    fn german_maps_to_de_de() {
        assert_eq!(to_bcp47(Locale::German).unwrap(), "de-DE");
    }

    #[test]
    fn hindi_errors_with_helpful_message() {
        let err = to_bcp47(Locale::Hindi).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("Hindi") && msg.contains("hi-IN") && msg.contains("Whisper"),
            "error message should name Hindi, hi-IN, and Whisper as the workaround; got: {msg}"
        );
    }
}
```

- [ ] **Step 3: Run tests, see failure**

```bash
~/.cargo/bin/cargo test -p primer-speech --features macos-native-26 --lib macos26::locale
```
Expected: 3 tests fail (panic on `unimplemented!`).

- [ ] **Step 4: Implement `to_bcp47`**

Replace the unimplemented body in `locale.rs`:

```rust
pub fn to_bcp47(locale: Locale) -> Result<&'static str> {
    match locale {
        Locale::English => Ok("en-US"),
        Locale::German => Ok("de-DE"),
        Locale::Hindi => Err(PrimerError::Speech(
            "Hindi (hi-IN) not yet supported by SpeechTranscriber on macOS 26.5; \
             use --features primer-cli/speech without macos-native-26 for the Whisper path"
                .into(),
        )),
    }
}
```

- [ ] **Step 5: Run tests, see passes**

```bash
~/.cargo/bin/cargo test -p primer-speech --features macos-native-26 --lib macos26::locale
```
Expected: 3 tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/crates/primer-speech/src/macos26/{mod.rs,locale.rs}
git commit -m "speech(macos26): Locale ↔ BCP47 mapping + tests"
```

---

### Task 5: Swift sidecar — Macos26Pipeline.swift

**Files:**
- Create: `src/crates/primer-speech/swift-sources/Macos26Pipeline.swift`

This task adapts the merged spike's Swift code into a class with a swift-bridge-friendly API. No Rust changes yet.

- [ ] **Step 1: Create the Swift sidecar**

Create `src/crates/primer-speech/swift-sources/Macos26Pipeline.swift` with the following content:

```swift
// Swift sidecar for the macos-native-26 feature. Compiled into a static
// library by primer-speech's build.rs and linked statically. Reachable
// from Rust via the swift-bridge module at src/macos26/bridge.rs.
//
// Reference implementation: spikes/macos26_speech/Sources/macos26_speech/main.swift
// — same SpeechAnalyzer setup (.progressiveTranscription preset,
// .medium SpeechDetector sensitivity).

import AVFoundation
import CoreMedia
import Foundation
import Speech

/// Plain value type pushed to Rust per transcriber result. Strings cross
/// the bridge cleanly; AttributedString is reduced to plain text on the
/// Swift side so the Rust side never has to know about it.
public struct ResultEvent {
    public let text: String
    public let isFinal: Bool
    public let rangeStartMs: UInt64
    public let rangeEndMs: UInt64
}

public enum Macos26PipelineError: Error {
    case localeNotSupported(String)
    case noAnalyzerFormat
    case noInstallationRequest
    case streamClosed
}

/// Owns the SpeechAnalyzer + SpeechTranscriber + SpeechDetector trio.
/// Audio is pushed by Rust via `feedAudio`. Results are pulled by Rust
/// via `nextResult`, which awaits the next item on `transcriber.results`.
public final class Macos26Pipeline {
    private let analyzer: SpeechAnalyzer
    private let transcriber: SpeechTranscriber
    private let inputContinuation: AsyncStream<AnalyzerInput>.Continuation
    private let analyzerFormat: AVAudioFormat
    private var resultsIterator: AsyncThrowingStream<SpeechTranscriber.Result, Error>.AsyncIterator?

    public init(localeBcp47: String) async throws {
        let locale = Locale(identifier: localeBcp47)

        let supported = await SpeechTranscriber.supportedLocales
        guard supported.contains(where: { $0.identifier(.bcp47) == localeBcp47 }) else {
            throw Macos26PipelineError.localeNotSupported(localeBcp47)
        }

        let installed = await SpeechTranscriber.installedLocales
        let transcriber = SpeechTranscriber(
            locale: locale,
            preset: .progressiveTranscription
        )
        if !installed.contains(where: { $0.identifier(.bcp47) == localeBcp47 }) {
            // Apple's OS-managed download. No UI; the friction-free
            // demo policy is intentional (see spec, "Asset download").
            guard let req = try await AssetInventory.assetInstallationRequest(
                supporting: [transcriber]
            ) else {
                throw Macos26PipelineError.noInstallationRequest
            }
            try await req.downloadAndInstall()
        }

        let detector = SpeechDetector(
            detectionOptions: .init(sensitivityLevel: .medium),
            reportResults: false
        )
        let analyzer = SpeechAnalyzer(modules: [detector, transcriber])
        guard let fmt = await SpeechAnalyzer.bestAvailableAudioFormat(
            compatibleWith: [transcriber]
        ) else {
            throw Macos26PipelineError.noAnalyzerFormat
        }

        let (inputStream, inputContinuation) = AsyncStream<AnalyzerInput>.makeStream()
        try await analyzer.start(inputSequence: inputStream)

        self.analyzer = analyzer
        self.transcriber = transcriber
        self.inputContinuation = inputContinuation
        self.analyzerFormat = fmt
        // Wrap transcriber.results in an AsyncThrowingStream so we have
        // a concrete iterator type we can drive from nextResult().
        self.resultsIterator = AsyncThrowingStream<SpeechTranscriber.Result, Error> { cont in
            let task = Task {
                do {
                    for try await r in transcriber.results {
                        cont.yield(r)
                    }
                    cont.finish()
                } catch {
                    cont.finish(throwing: error)
                }
            }
            cont.onTermination = { _ in task.cancel() }
        }.makeAsyncIterator()
    }

    /// Sample rate the analyzer wants its input PCM at (typically 16 kHz).
    public func analyzerSampleRate() -> Double {
        return analyzerFormat.sampleRate
    }

    /// Push one PCM chunk into the analyzer. `samples` is mono Float32
    /// at the analyzer's preferred rate. Rust resamples upstream.
    public func feedAudio(samples: [Float]) {
        guard let buffer = AVAudioPCMBuffer(
            pcmFormat: analyzerFormat,
            frameCapacity: AVAudioFrameCount(samples.count)
        ) else { return }
        buffer.frameLength = AVAudioFrameCount(samples.count)
        if let channelData = buffer.floatChannelData {
            samples.withUnsafeBufferPointer { src in
                channelData[0].update(from: src.baseAddress!, count: samples.count)
            }
        }
        inputContinuation.yield(AnalyzerInput(buffer: buffer))
    }

    /// Pull the next transcriber result, awaiting if necessary. Returns
    /// nil once the underlying stream completes (analyzer stopped).
    public func nextResult() async throws -> ResultEvent? {
        guard var iter = resultsIterator else { return nil }
        defer { resultsIterator = iter }
        guard let result = try await iter.next() else { return nil }
        let text = String(result.text.characters).trimmingCharacters(in: .whitespacesAndNewlines)
        let startMs = UInt64(max(0, result.range.start.seconds * 1000))
        let endMs = UInt64(max(0, result.range.end.seconds * 1000))
        return ResultEvent(
            text: text,
            isFinal: result.isFinal,
            rangeStartMs: startMs,
            rangeEndMs: endMs
        )
    }

    /// Stop the analyzer and tear down the pipeline.
    public func stop() async {
        inputContinuation.finish()
        try? await analyzer.finalizeAndFinishThroughEndOfInput()
    }
}
```

- [ ] **Step 2: Type-check the Swift file standalone**

```bash
xcrun -sdk macosx swiftc -typecheck \
    src/crates/primer-speech/swift-sources/Macos26Pipeline.swift
```
Expected: no output (silent success). Any errors should be fixed before proceeding to Task 6 — the build.rs depends on this compiling.

- [ ] **Step 3: Commit**

```bash
git add src/crates/primer-speech/swift-sources/Macos26Pipeline.swift
git commit -m "speech(macos26): Swift sidecar Macos26Pipeline (pull-based async API)"
```

---

### Task 6: build.rs — swift-bridge codegen + swiftc

**Files:**
- Create: `src/crates/primer-speech/build.rs`
- Modify: `src/crates/primer-speech/Cargo.toml` (add `build = "build.rs"`)

- [ ] **Step 1: Tell Cargo about the build script**

Edit `crates/primer-speech/Cargo.toml`. In the `[package]` block (the very top), add:

```toml
build = "build.rs"
```

- [ ] **Step 2: Write the build script**

Create `crates/primer-speech/build.rs`:

```rust
//! Build script for `primer-speech`.
//!
//! Only does anything when the `macos-native-26` feature is on. In that
//! case it:
//!   1. Invokes `swift-bridge-build` to generate the C header + Swift
//!      glue from src/macos26/bridge.rs.
//!   2. Invokes `swiftc` to compile the Swift sidecar + generated glue
//!      into a static library.
//!   3. Emits cargo:rustc-link-* directives so the final Rust binary
//!      pulls in the .a and the Swift runtime.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=swift-sources");
    println!("cargo:rerun-if-changed=src/macos26/bridge.rs");

    #[cfg(feature = "macos-native-26")]
    macos_native_26::build();
}

#[cfg(feature = "macos-native-26")]
mod macos_native_26 {
    use std::path::PathBuf;
    use std::process::Command;

    const SWIFT_LIB_NAME: &str = "Macos26Pipeline";

    pub fn build() {
        let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let bridges = vec![manifest_dir.join("src/macos26/bridge.rs")];

        // 1. swift-bridge codegen.
        let generated = out_dir.join("generated");
        std::fs::create_dir_all(&generated).expect("create generated dir");
        swift_bridge_build::parse_bridges(bridges)
            .write_all_concatenated(&generated, SWIFT_LIB_NAME);

        // 2. swiftc — compile the sidecar + generated glue into a static lib.
        let swift_sources = manifest_dir.join("swift-sources");
        let bridge_header = generated.join("SwiftBridgeCore.h");  // for reference
        let _ = bridge_header;

        let lib_path = out_dir.join(format!("lib{}.a", SWIFT_LIB_NAME));
        let mut cmd = Command::new("swiftc");
        cmd.arg("-emit-library")
            .arg("-static")
            .arg("-emit-module")
            .arg("-module-name").arg(SWIFT_LIB_NAME)
            .arg("-target").arg(swift_target_triple())
            .arg("-sdk").arg(macos_sdk_path())
            .arg("-O")
            .arg("-parse-as-library")
            .arg(swift_sources.join("Macos26Pipeline.swift"))
            // swift-bridge generated Swift sources:
            .args(walk_swift_files(&generated))
            .arg("-o").arg(&lib_path);
        let status = cmd.status().expect("invoke swiftc");
        assert!(status.success(), "swiftc failed");

        // 3. Link directives.
        println!("cargo:rustc-link-search=native={}", out_dir.display());
        println!("cargo:rustc-link-lib=static={}", SWIFT_LIB_NAME);
        // Swift runtime libraries — required when linking a Swift staticlib.
        println!("cargo:rustc-link-search=native={}", swift_runtime_dir());
        for fw in ["Foundation", "AVFoundation", "CoreMedia", "Speech"] {
            println!("cargo:rustc-link-lib=framework={fw}");
        }
        // Tell rustc to use the Swift runtime's dylibs at link time.
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", swift_runtime_dir());
        println!("cargo:rustc-link-arg=-L{}", swift_runtime_dir());
        println!("cargo:rustc-link-arg=-lswiftCore");
    }

    fn swift_target_triple() -> String {
        // Match the rustc target. e.g. aarch64-apple-darwin → arm64-apple-macos26.0
        let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
            "aarch64" => "arm64",
            "x86_64" => "x86_64",
            other => panic!("unsupported arch: {other}"),
        };
        format!("{arch}-apple-macos26.0")
    }

    fn macos_sdk_path() -> String {
        let out = Command::new("xcrun")
            .args(["--show-sdk-path", "--sdk", "macosx"])
            .output()
            .expect("invoke xcrun");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn swift_runtime_dir() -> String {
        // /Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx
        let xcode = Command::new("xcode-select")
            .arg("-p")
            .output()
            .expect("invoke xcode-select");
        let xcode_path = String::from_utf8(xcode.stdout).unwrap().trim().to_string();
        format!("{xcode_path}/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx")
    }

    fn walk_swift_files(dir: &PathBuf) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .expect("read generated dir")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("swift"))
            .collect()
    }
}
```

- [ ] **Step 3: Try to build — expect a failure pointing at the missing bridge.rs**

```bash
~/.cargo/bin/cargo build -p primer-speech --features macos-native-26 2>&1 | tail -10
```
Expected: build fails because `swift_bridge_build::parse_bridges` can't find `src/macos26/bridge.rs` yet. This validates that the build.rs is at least being invoked. Task 7 lands `bridge.rs`.

- [ ] **Step 4: Commit (intentionally with the broken state, fixed by Task 7)**

```bash
git add src/crates/primer-speech/{Cargo.toml,build.rs}
git commit -m "speech(macos26): build.rs scaffolding (swift-bridge + swiftc)"
```

---

### Task 7: macos26/bridge.rs — swift-bridge module

**Files:**
- Create: `src/crates/primer-speech/src/macos26/bridge.rs`
- Modify: `src/crates/primer-speech/src/macos26/mod.rs`

- [ ] **Step 1: Register the module**

Append to `crates/primer-speech/src/macos26/mod.rs`:

```rust
pub mod bridge;
```

- [ ] **Step 2: Write the bridge declaration**

Create `crates/primer-speech/src/macos26/bridge.rs`:

```rust
//! swift-bridge declaration of the Swift sidecar `Macos26Pipeline`.
//! Compiled by build.rs alongside the Swift sources.

#![cfg(all(target_vendor = "apple", feature = "macos-native-26"))]

#[swift_bridge::bridge]
pub(crate) mod ffi {
    /// Mirror of Swift's `ResultEvent` struct.
    struct ResultEvent {
        text: String,
        is_final: bool,
        range_start_ms: u64,
        range_end_ms: u64,
    }

    extern "Swift" {
        type Macos26Pipeline;

        #[swift_bridge(init, associated_to = Macos26Pipeline)]
        async fn new(locale_bcp47: String) -> Result<Macos26Pipeline, String>;

        fn analyzerSampleRate(&self) -> f64;
        fn feedAudio(&mut self, samples: Vec<f32>);
        async fn nextResult(&mut self) -> Result<Option<ResultEvent>, String>;
        async fn stop(&mut self);
    }
}

pub(crate) use ffi::{Macos26Pipeline, ResultEvent};
```

(`swift-bridge` translates Swift's throwing async functions into Rust `Result<T, String>` returns; the bridge's macro generates the glue.)

- [ ] **Step 3: Build — expect success**

```bash
~/.cargo/bin/cargo build -p primer-speech --features macos-native-26 2>&1 | tail -20
```
Expected: build succeeds. The swift-bridge codegen produces a `.h` and `.swift`, swiftc compiles them along with `swift-sources/Macos26Pipeline.swift`, and rustc links the resulting `.a`. If swiftc complains about the Swift sidecar not matching the bridge signatures (parameter names, throws, etc.), reconcile by editing the Swift file's method signatures — the bridge declaration is the source of truth for the FFI shape.

- [ ] **Step 4: Commit**

```bash
git add src/crates/primer-speech/src/macos26/{mod.rs,bridge.rs}
git commit -m "speech(macos26): swift-bridge module wiring Macos26Pipeline"
```

---

### Task 8: macos26/analyzer.rs — Rust driver

**Files:**
- Create: `src/crates/primer-speech/src/macos26/analyzer.rs`
- Modify: `src/crates/primer-speech/src/macos26/mod.rs`

This task wires the bridged Swift pipeline to the derived-VAD state machine and the two outbound mpsc channels. No new public surface yet — that lands in Task 9.

- [ ] **Step 1: Register the module**

Append to `crates/primer-speech/src/macos26/mod.rs`:

```rust
pub mod analyzer;
```

- [ ] **Step 2: Write the analyzer driver**

Create `crates/primer-speech/src/macos26/analyzer.rs`:

```rust
//! Rust-side driver for the Swift sidecar. Drives `Macos26Pipeline`'s
//! `nextResult().await` in a loop, runs each result through the
//! `DerivedVadStateMachine`, and forwards text + VAD events on two
//! `mpsc::Sender`s supplied by the builder.

#![cfg(all(target_vendor = "apple", feature = "macos-native-26"))]

use std::time::Instant;

use primer_core::consts::speech::macos26::EVENT_POLL_INTERVAL;
use primer_core::error::{PrimerError, Result};
use primer_core::speech::{TranscriptSegment, VadEvent};
use tokio::sync::mpsc;

use crate::macos26::bridge::{Macos26Pipeline, ResultEvent};
use crate::macos26::vad::DerivedVadStateMachine;

/// Messages emitted by [`run_consumer_loop`] onto the text channel.
/// `ChannelStt` consumes these via the existing adapter in voice_loop.
pub struct TextMessage {
    pub segment: TranscriptSegment,
    pub is_final: bool,
}

/// Run the consumer loop until the pipeline stops (returns `Ok(None)`)
/// or errors. The audio thread should `spawn` this on its tokio runtime
/// after constructing the pipeline. Audio feeding happens elsewhere via
/// `pipeline.feedAudio(...)`.
pub async fn run_consumer_loop(
    mut pipeline: swift_bridge::PointerToSwiftType<Macos26Pipeline>,
    text_tx: mpsc::Sender<TextMessage>,
    event_tx: mpsc::Sender<VadEvent>,
) -> Result<()> {
    let mut sm = DerivedVadStateMachine::new();
    let mut tick = tokio::time::interval(EVENT_POLL_INTERVAL);

    loop {
        tokio::select! {
            // Pull the next result from Swift. nextResult() returns None
            // when the underlying transcriber stream completes.
            result = pipeline.nextResult() => {
                match result {
                    Ok(Some(event)) => {
                        let now = Instant::now();
                        let vad_event = sm.on_result(&event.text, event.is_final, now);
                        if let Some(ev) = vad_event {
                            // Channel full means run_loop is wedged; log + drop.
                            if event_tx.try_send(ev).is_err() {
                                tracing::warn!("vad event channel full; dropped {ev:?}");
                            }
                        }
                        let msg = TextMessage {
                            segment: TranscriptSegment {
                                text: event.text,
                                start_ms: event.range_start_ms,
                                end_ms: event.range_end_ms,
                            },
                            is_final: event.is_final,
                        };
                        if text_tx.send(msg).await.is_err() {
                            // Downstream gone — exit cleanly.
                            return Ok(());
                        }
                    }
                    Ok(None) => return Ok(()),  // stream completed
                    Err(e) => return Err(PrimerError::Speech(
                        format!("Macos26Pipeline.nextResult failed: {e}")
                    )),
                }
            }
            // Periodic tick for the inactivity timer.
            _ = tick.tick() => {
                if let Some(ev) = sm.tick(Instant::now()) {
                    if event_tx.try_send(ev).is_err() {
                        tracing::warn!("vad event channel full; dropped {ev:?}");
                    }
                }
            }
        }
    }
}
```

(Note on the `swift_bridge::PointerToSwiftType` type name: swift-bridge 0.1.x exposes opaque Swift instances as a wrapper type. The exact name may be `swift_bridge::SwiftPtr` or similar depending on the published API — adjust to the actual symbol when this task runs. The `cargo build` in Step 3 will surface the right name.)

- [ ] **Step 3: Build to surface the exact swift-bridge type wrapper**

```bash
~/.cargo/bin/cargo build -p primer-speech --features macos-native-26 2>&1 | tail -20
```
Expected: either clean build, or a type-name mismatch error pointing at the actual swift-bridge opaque-type wrapper. Adjust the function signature accordingly and rebuild.

- [ ] **Step 4: Commit**

```bash
git add src/crates/primer-speech/src/macos26/{mod.rs,analyzer.rs}
git commit -m "speech(macos26): consumer loop driving Macos26Pipeline.nextResult()"
```

---

### Task 9: macos26/stt.rs — Macos26Stt + session

**Files:**
- Create: `src/crates/primer-speech/src/macos26/stt.rs`
- Modify: `src/crates/primer-speech/src/macos26/mod.rs`

This wraps the analyzer in the `StreamingSpeechToText` trait, mirroring the existing `MacosSpeechToText` shape.

- [ ] **Step 1: Register the module**

Append to `crates/primer-speech/src/macos26/mod.rs`:

```rust
pub mod stt;
```

- [ ] **Step 2: Write the STT struct + impl**

Create `crates/primer-speech/src/macos26/stt.rs`:

```rust
//! `Macos26Stt` implements `StreamingSpeechToText`. Each session owns one
//! `Macos26Pipeline` (a fresh SpeechAnalyzer instance). Construction is
//! async because Swift's `Macos26Pipeline.new(locale:)` is async — it
//! awaits the asset-install step.

#![cfg(all(target_vendor = "apple", feature = "macos-native-26"))]

use std::sync::Arc;

use async_trait::async_trait;
use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;
use primer_core::speech::{
    Named, StreamingSpeechToText, TranscriptSegment, TranscriptionSession,
};

use crate::macos26::bridge::Macos26Pipeline;
use crate::macos26::locale::to_bcp47;

const BACKEND_NAME: &str = "macos-26-speech-analyzer";

/// Streaming STT backend backed by macOS 26's `SpeechAnalyzer`.
/// Locale is fixed at construction time. Each `open_session` returns a
/// fresh pipeline because SpeechAnalyzer state isn't reset-able.
pub struct Macos26Stt {
    locale: Locale,
}

impl Macos26Stt {
    pub async fn new(locale: Locale) -> Result<Self> {
        // Validate the locale eagerly — `to_bcp47` errors loudly on
        // unsupported locales (today: Hindi).
        let _bcp47 = to_bcp47(locale)?;
        Ok(Self { locale })
    }

    pub fn locale(&self) -> Locale {
        self.locale
    }
}

impl Named for Macos26Stt {
    fn name(&self) -> &str {
        BACKEND_NAME
    }
}

impl StreamingSpeechToText for Macos26Stt {
    fn sample_rate(&self) -> u32 {
        // SpeechTranscriber's preferred format is 16 kHz Int16 mono, but
        // we feed it Float32 16 kHz — the Swift sidecar converts.
        16_000
    }

    fn open_session(&self) -> Result<Box<dyn TranscriptionSession>> {
        // The session's pipeline is constructed lazily on the first push
        // because Macos26Pipeline.new is async and this method is sync
        // (matches the existing WhisperStt trait shape).
        let bcp47 = to_bcp47(self.locale)?;
        Ok(Box::new(Macos26TranscriptionSession::new(bcp47.to_string())))
    }
}

/// One streaming session. Holds the pipeline and an in-process queue of
/// segments produced via the consumer loop. See note: this is a thinner
/// shim than the Whisper one because all the heavy lifting happens in
/// `voice_loop::backends::build_local_backends_macos_native_26`; the
/// trait fit is here for symmetry with the rest of the codebase.
pub struct Macos26TranscriptionSession {
    bcp47: String,
    pipeline: Option<Arc<Macos26Pipeline>>,
}

impl Macos26TranscriptionSession {
    fn new(bcp47: String) -> Self {
        Self { bcp47, pipeline: None }
    }
}

impl TranscriptionSession for Macos26TranscriptionSession {
    fn push_audio(&mut self, _samples: &[f32]) -> Result<Vec<TranscriptSegment>> {
        // In this integration, audio is fed via the analyzer task that
        // build_local_backends_macos_native_26 spawns — NOT through the
        // session's push_audio. This method is a no-op here; the trait
        // exists to satisfy the dispatch shape elsewhere in voice_loop.
        // Document the divergence loudly:
        Err(PrimerError::Speech(
            "Macos26TranscriptionSession::push_audio: audio flows through \
             the analyzer task, not the session trait. Use \
             build_local_backends_macos_native_26 instead.".into()
        ))
    }

    fn finalize(self: Box<Self>) -> Result<Vec<TranscriptSegment>> {
        // Mirror push_audio: no-op, error rather than silently lying.
        let _ = self.bcp47;
        let _ = self.pipeline;
        Err(PrimerError::Speech(
            "Macos26TranscriptionSession::finalize: see push_audio.".into()
        ))
    }
}
```

(Note: the session struct here is intentionally a shim. The "real" path for this backend is `build_local_backends_macos_native_26` which owns the pipeline directly. The shim exists so the trait surface stays uniform across backends. If a future refactor lifts SpeechAnalyzer into the session struct, replace this.)

- [ ] **Step 3: Build**

```bash
~/.cargo/bin/cargo build -p primer-speech --features macos-native-26 2>&1 | tail -10
```
Expected: clean build. If `Arc<Macos26Pipeline>` doesn't compile because swift-bridge opaque types aren't Send+Sync, drop the `Arc<>` field — the production path doesn't use it.

- [ ] **Step 4: Commit**

```bash
git add src/crates/primer-speech/src/macos26/{mod.rs,stt.rs}
git commit -m "speech(macos26): Macos26Stt + session trait shims"
```

---

### Task 10: macos26/audio_session.rs (iOS stub)

**Files:**
- Create: `src/crates/primer-speech/src/macos26/audio_session.rs`
- Modify: `src/crates/primer-speech/src/macos26/mod.rs`

This file exists to localise the macOS-vs-iOS divergence in one place. The iOS branch is a placeholder; concrete impl lands when an iOS host actually exists.

- [ ] **Step 1: Register the module**

Append to `crates/primer-speech/src/macos26/mod.rs`:

```rust
pub mod audio_session;
```

- [ ] **Step 2: Write the cfg-split**

Create `crates/primer-speech/src/macos26/audio_session.rs`:

```rust
//! Platform-specific audio session setup. The macOS branch is a no-op;
//! macOS has no AVAudioSession. The iOS branch is a placeholder today;
//! it'll need a real impl when iOS host scaffolding lands. Concentrating
//! the divergence here keeps `analyzer.rs` and `stt.rs` Apple-portable.

#![cfg(all(target_vendor = "apple", feature = "macos-native-26"))]

use primer_core::error::Result;

#[cfg(target_os = "macos")]
pub fn configure_for_capture() -> Result<()> {
    // No AVAudioSession on macOS — cpal owns the device.
    Ok(())
}

#[cfg(target_os = "ios")]
pub fn configure_for_capture() -> Result<()> {
    // Placeholder: real iOS impl needs to set the AVAudioSession
    // category to .playAndRecord (.measurement mode), activate it,
    // handle interruption notifications. See the iOS scaffolding work
    // tracked separately. Until then, refuse loudly so a developer
    // doesn't ship an unconfigured iOS build.
    Err(primer_core::error::PrimerError::Speech(
        "macos26::audio_session: iOS session configuration is not yet \
         implemented. Add the AVAudioSession setup before shipping an \
         iOS build.".into()
    ))
}
```

- [ ] **Step 3: Build**

```bash
~/.cargo/bin/cargo build -p primer-speech --features macos-native-26 2>&1 | tail -5
```
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add src/crates/primer-speech/src/macos26/{mod.rs,audio_session.rs}
git commit -m "speech(macos26): audio_session cfg-split (macOS no-op, iOS stub)"
```

---

### Task 11: macos26/mod.rs — re-exports + reused TTS

**Files:**
- Modify: `src/crates/primer-speech/src/macos26/mod.rs`

- [ ] **Step 1: Add re-exports**

At the bottom of `crates/primer-speech/src/macos26/mod.rs`, append:

```rust
// Re-use the macos-native AVSpeechSynthesizer TTS — there's no new TTS
// surface in macOS 26 that warrants a parallel impl. The macos-native
// feature is mutually exclusive with macos-native-26, but the `macos`
// module's TTS submodule compiles standalone (it only needs the objc2
// bindings that macos-native-26 also pulls).
//
// HOWEVER: today's `macos` module is gated by `feature = "macos-native"`,
// so we re-declare just the TTS path under the macos-native-26 feature.
// See `crate::macos::tts` for the implementation.
//
// For this initial integration we INLINE the re-import via a sibling
// path; subsequent refactor may extract a shared submodule. See the
// design doc's "TTS path: reuse" goal.

pub use crate::macos26::stt::Macos26Stt;
pub use crate::macos26::analyzer::TextMessage;
```

- [ ] **Step 2: Verify the TTS reuse path actually compiles**

The simplest way to share `MacosTextToSpeech` cleanly across both features is to widen its cfg-gate in `crates/primer-speech/src/lib.rs`. Find the existing line that registers the `macos` module:

```rust
#[cfg(all(target_os = "macos", feature = "macos-native"))]
pub mod macos;
```

Replace with:

```rust
#[cfg(all(target_os = "macos", any(feature = "macos-native", feature = "macos-native-26")))]
pub mod macos;
```

And inside `crates/primer-speech/src/macos/mod.rs`, find the per-submodule cfg gates (currently all `feature = "macos-native"`) and widen the TTS-only path. Specifically, the `pub mod tts;` line — change its gate (if any) to `any(feature = "macos-native", feature = "macos-native-26")`, leaving the STT modules (`stt`, `voice`) on `macos-native` only:

```rust
// crates/primer-speech/src/macos/mod.rs
#[cfg(feature = "macos-native")]
pub mod stt;          // SFSpeechRecognizer — only for the legacy path

#[cfg(any(feature = "macos-native", feature = "macos-native-26"))]
pub mod tts;          // AVSpeechSynthesizer — reused by macos-native-26

// (locale, permissions, voice — leave as macos-native gated for now;
//  the macos-native-26 path doesn't currently touch them. If a future
//  task needs them, widen those gates the same way.)
```

(If the existing `mod.rs` doesn't have per-submodule cfg gates because the whole `macos` module was previously feature-gated as one unit, add the per-submodule gates while widening the parent gate.)

Then add to `crates/primer-speech/src/macos26/mod.rs`:

```rust
pub use crate::macos::tts::MacosTextToSpeech;
```

- [ ] **Step 3: Build**

```bash
~/.cargo/bin/cargo build -p primer-speech --features macos-native-26 2>&1 | tail -10
```
Expected: clean. The `macos26` module now exports `Macos26Stt` and `MacosTextToSpeech`.

- [ ] **Step 4: Build the legacy path too to make sure widening the gates didn't break it**

```bash
~/.cargo/bin/cargo build -p primer-speech --features macos-native 2>&1 | tail -5
```
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-speech/src/lib.rs \
        src/crates/primer-speech/src/macos/mod.rs \
        src/crates/primer-speech/src/macos26/mod.rs
git commit -m "speech(macos26): re-export Macos26Stt + reuse MacosTextToSpeech"
```

---

### Task 12: `build_local_backends_macos_native_26`

**Files:**
- Modify: `src/crates/primer-speech/src/voice_loop/backends.rs`
- Modify: `src/crates/primer-speech/src/macos26/analyzer.rs`

This is the glue function. The sibling function `build_local_backends_macos_native` in the same file is the reference — its actual signature (verified by reading [backends.rs:734](src/crates/primer-speech/src/voice_loop/backends.rs#L734)) is:

```rust
pub async fn build_local_backends_macos_native(
    locale: primer_core::i18n::Locale,
    mic_silence_ms: u32,
    verbose: bool,
) -> Result<LocalBackends> { ... }
```

`LocalBackends` (defined at [backends.rs:282](src/crates/primer-speech/src/voice_loop/backends.rs#L282)) is a struct holding `Option<LoopBackends>`, `Option<mpsc::Receiver<VadEvent>>`, `Option<Box<dyn FnMut(Vec<f32>) + Send>>` (the speaker push callback), `Option<DrainHook>`, `Arc<AtomicBool>` (is_speaking), plus owned `MicCapture` + `SpeakerSink` + the audio thread join handle. The new builder constructs the same shape.

- [ ] **Step 1: First restructure `run_consumer_loop` to take audio in**

The audio thread will feed audio via an mpsc; the consumer task owns the pipeline and pulls results. This avoids needing the pipeline to be Send+Sync across two tasks.

Replace the existing body of `crates/primer-speech/src/macos26/analyzer.rs::run_consumer_loop` from Task 8 with:

```rust
pub async fn run_consumer_loop(
    mut pipeline: crate::macos26::bridge::ffi::Macos26Pipeline,
    mut audio_rx: tokio::sync::mpsc::Receiver<Vec<f32>>,
    text_tx: tokio::sync::mpsc::Sender<TextMessage>,
    event_tx: tokio::sync::mpsc::Sender<VadEvent>,
) -> Result<()> {
    let mut sm = DerivedVadStateMachine::new();
    let mut tick = tokio::time::interval(EVENT_POLL_INTERVAL);

    loop {
        tokio::select! {
            // Audio in from the mic thread.
            samples = audio_rx.recv() => {
                match samples {
                    Some(s) => pipeline.feedAudio(s),
                    None => {
                        // Mic gone — stop the pipeline gracefully.
                        pipeline.stop().await;
                        return Ok(());
                    }
                }
            }
            // Results out from Swift.
            result = pipeline.nextResult() => {
                match result {
                    Ok(Some(event)) => {
                        let now = Instant::now();
                        if let Some(ev) = sm.on_result(&event.text, event.is_final, now) {
                            if event_tx.try_send(ev).is_err() {
                                tracing::warn!("vad event channel full; dropped {ev:?}");
                            }
                        }
                        let msg = TextMessage {
                            segment: TranscriptSegment {
                                text: event.text,
                                start_ms: event.range_start_ms,
                                end_ms: event.range_end_ms,
                            },
                            is_final: event.is_final,
                        };
                        if text_tx.send(msg).await.is_err() {
                            return Ok(());
                        }
                    }
                    Ok(None) => return Ok(()),
                    Err(e) => return Err(PrimerError::Speech(
                        format!("Macos26Pipeline.nextResult failed: {e}")
                    )),
                }
            }
            // Inactivity-timer tick.
            _ = tick.tick() => {
                if let Some(ev) = sm.tick(Instant::now()) {
                    if event_tx.try_send(ev).is_err() {
                        tracing::warn!("vad event channel full; dropped {ev:?}");
                    }
                }
            }
        }
    }
}
```

(If `Macos26Pipeline` as a value type doesn't compile in the signature — swift-bridge opaque types are typically wrapped — change the parameter to whatever wrapper type `swift-bridge 0.1` actually produces. Build error in the next task will show the exact name.)

- [ ] **Step 2: Add the new builder**

Append to `crates/primer-speech/src/voice_loop/backends.rs` immediately after the closing `}` of `build_local_backends_macos_native`:

```rust
/// Build a [`LocalBackends`] using macOS 26's SpeechAnalyzer for STT and
/// derived VAD events, with AVSpeechSynthesizer for TTS (reused from the
/// `macos-native` path's `MacosTextToSpeech`). Sibling of
/// [`build_local_backends_macos_native`] — same signature, same return
/// type; the audio thread's STT/VAD pipeline is what differs.
#[cfg(all(target_os = "macos", feature = "macos-native-26"))]
pub async fn build_local_backends_macos_native_26(
    locale: primer_core::i18n::Locale,
    mic_silence_ms: u32,
    verbose: bool,
) -> Result<LocalBackends> {
    use crate::macos::MacosTextToSpeech;
    use crate::macos26::analyzer::{run_consumer_loop, TextMessage};
    use crate::macos26::audio_session;
    use crate::macos26::bridge::ffi as macos26_ffi;
    use crate::macos26::locale::to_bcp47;

    // mic_silence_ms is a Silero-VAD knob in the sibling; the macos26
    // path uses transcript-derived VAD instead. The parameter is kept
    // for signature parity; we just record it and move on.
    let _ = mic_silence_ms;

    let bcp47 = to_bcp47(locale)?.to_string();
    audio_session::configure_for_capture()?;

    // 1. Build the Swift pipeline (awaits asset install on first run).
    tracing::info!(
        target: "primer::speech::macos26",
        "constructing SpeechAnalyzer pipeline for locale {bcp47}"
    );
    let pipeline = macos26_ffi::Macos26Pipeline::new(bcp47.clone())
        .await
        .map_err(|e| PrimerError::Speech(
            format!("Macos26Pipeline.new({bcp47}) failed: {e}")
        ))?;

    // 2. Open mic + speaker (copy from the sibling at
    //    crates/primer-speech/src/voice_loop/backends.rs L759-L780).
    let (mic, mic_cons) = MicCapture::start()?;
    let mic_rate = mic.sample_rate;
    if verbose {
        eprintln!(
            "[speech] mic opened: {}Hz, {} channels",
            mic_rate, mic.channels
        );
    }
    let (spk, spk_prod) = SpeakerSink::start()?;
    let spk_rate = spk.sample_rate;
    let spk_errored = spk.errored_flag();
    let spk_prod = Arc::new(std::sync::Mutex::new(spk_prod));
    if verbose {
        eprintln!(
            "[speech] speaker opened: {}Hz, {} channels",
            spk_rate, spk.channels
        );
    }

    // 3. Input resampler: mic_rate → 16 kHz for the analyzer.
    let analyzer_rate: u32 = 16_000;
    let vad_chunk: usize = 512;  // not used as a VAD chunk here, just sizes the resampler
    let in_chunk_samples: usize =
        (vad_chunk as u64 * mic_rate as u64 / analyzer_rate as u64) as usize;
    let mut input_resampler: Option<Resampler> = if mic_rate != analyzer_rate {
        Some(Resampler::new(mic_rate, analyzer_rate, in_chunk_samples)?)
    } else {
        None
    };

    // 4. TTS (reused; same construction as the sibling).
    let tts: Arc<dyn StreamingTextToSpeech> = Arc::new(MacosTextToSpeech::new(&bcp47)?);
    let tts_sample_rate = tts.sample_rate();

    // 5. Channels for downstream consumers + audio thread.
    let (text_tx, text_rx) = tokio::sync::mpsc::channel::<TextMessage>(64);
    let (event_tx, event_rx) =
        tokio::sync::mpsc::channel::<VadEvent>(VAD_EVENT_CHANNEL_CAPACITY);
    let (audio_tx, audio_rx) = tokio::sync::mpsc::channel::<Vec<f32>>(8);

    // 6. Spawn the consumer task (pulls results from Swift, runs the
    //    DerivedVadStateMachine, emits text + VadEvents).
    let _consumer_handle = tokio::spawn(run_consumer_loop(
        pipeline, audio_rx, text_tx, event_tx,
    ));

    // 7. Audio thread: pull from mic ring, resample, send to audio_tx.
    //    Same shape as the sibling's audio thread, but feeds an mpsc
    //    instead of driving Silero+SFSpeechRecognizer locally.
    let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let is_speaking = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let audio_thread = {
        let stop_flag = stop_flag.clone();
        let is_speaking = is_speaking.clone();
        std::thread::spawn(move || -> Result<()> {
            let mut pending: Vec<f32> = Vec::with_capacity(in_chunk_samples * 4);
            let mut tmp = [0f32; 1024];
            while !stop_flag.load(std::sync::atomic::Ordering::SeqCst) {
                use ringbuf::traits::Consumer;
                let popped = mic_cons.pop_slice(&mut tmp);
                if popped == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                }
                // Anti-feedback: drop mic samples while the Primer is speaking.
                if is_speaking.load(std::sync::atomic::Ordering::SeqCst) {
                    continue;
                }
                pending.extend_from_slice(&tmp[..popped]);
                while pending.len() >= in_chunk_samples {
                    let chunk: Vec<f32> = pending.drain(..in_chunk_samples).collect();
                    let resampled = match input_resampler.as_mut() {
                        Some(r) => r.process(&chunk)?,
                        None => chunk,
                    };
                    // try_send: if the consumer is wedged we drop rather than block.
                    let _ = audio_tx.try_send(resampled);
                }
            }
            Ok(())
        })
    };

    // 8. Build the LoopBackends + assemble LocalBackends.
    let stt: Arc<dyn StreamingSpeechToText + Send + Sync> =
        Arc::new(ChannelStt::from_text_rx(text_rx));
    let backends = LoopBackends::single_locale(
        stt,
        tts.clone(),
        primer_core::speech::VoiceProfile::default(),  // mirror sibling
        locale,
    );

    // The speaker push callback + drain hook are lifted unchanged from
    // the sibling — they're TTS-side, not STT-side, so the macos26
    // feature inherits the same machinery. Lift the exact closures from
    // crates/primer-speech/src/voice_loop/backends.rs in the sibling's
    // tail (the section after "Build LoopBackends" — search for
    // `on_audio` and `drain_hook` in that file).
    let (on_audio, drain_hook) =
        crate::voice_loop::backends::tts_push_helpers(
            spk_prod.clone(),
            spk_rate,
            tts_sample_rate,
            spk_errored.clone(),
        )?;
    // ^ If `tts_push_helpers` doesn't exist with that exact name, the
    //   sibling builds the closures inline — copy the relevant ~30 lines
    //   verbatim and replace this call with that code.

    Ok(LocalBackends {
        backends: Some(backends),
        event_rx: Some(event_rx),
        on_audio: Some(on_audio),
        drain_hook: Some(drain_hook),
        is_speaking,
        audio_thread: Some(audio_thread),
        stop_flag,
        _mic: mic,
        _spk: spk,
    })
}
```

- [ ] **Step 3: Build**

```bash
~/.cargo/bin/cargo build -p primer-speech --features macos-native-26 2>&1 | tail -20
```
Expected: clean. Likely error categories and how to resolve:

- swift-bridge opaque-type name mismatch (e.g. `Macos26Pipeline` should be `swift_bridge::Wrapped<Macos26Pipeline>` or similar) → adjust the consumer-loop and builder signatures to match the actual generated wrapper.
- `tts_push_helpers` doesn't exist → grep the sibling for the inline closure construction (look for `move |samples: Vec<f32>|` and the speaker-write loop) and inline that ~30-line block here verbatim.
- `MacosTextToSpeech::new(&bcp47)` arity mismatch → check the actual constructor signature in `crates/primer-speech/src/macos/tts.rs`.

- [ ] **Step 4: Commit**

```bash
git add src/crates/primer-speech/src/voice_loop/backends.rs \
        src/crates/primer-speech/src/macos26/analyzer.rs
git commit -m "speech(macos26): build_local_backends_macos_native_26"
```

---

### Task 13: CLI feature propagation + cfg arm

**Files:**
- Modify: `src/crates/primer-cli/Cargo.toml`
- Modify: `src/crates/primer-cli/src/main.rs`

- [ ] **Step 1: Add the feature to `primer-cli/Cargo.toml`**

Locate the existing line:

```toml
macos-native = ["primer-speech/macos-native"]
```

Add immediately below:

```toml
macos-native-26 = ["primer-speech/macos-native-26"]
```

- [ ] **Step 2: Add the mutual-exclusion compile_error to `primer-cli/src/main.rs`**

Near the top of `main.rs`, after the imports:

```rust
#[cfg(all(feature = "macos-native", feature = "macos-native-26"))]
compile_error!(
    "`macos-native` and `macos-native-26` are mutually exclusive — pick one"
);
```

- [ ] **Step 3: Add the third cfg arm in the runtime backend selection**

Find the existing pair of cfg branches that route between `build_local_backends` and `build_local_backends_macos_native`:

```bash
grep -n 'build_local_backends_macos_native\|build_local_backends(' crates/primer-cli/src/main.rs
```

Adapt them into a three-way switch:

```rust
#[cfg(all(target_os = "macos", feature = "primer-speech/macos-native-26"))]
let (backends, event_rx /* etc */) =
    primer_speech::voice_loop::backends::build_local_backends_macos_native_26(cfg.clone()).await?;

#[cfg(all(
    target_os = "macos",
    feature = "primer-speech/macos-native",
    not(feature = "primer-speech/macos-native-26"),
))]
let (backends, event_rx /* etc */) =
    primer_speech::voice_loop::backends::build_local_backends_macos_native(cfg.clone()).await?;

#[cfg(not(any(
    feature = "primer-speech/macos-native",
    feature = "primer-speech/macos-native-26",
)))]
let (backends, event_rx /* etc */) =
    primer_speech::voice_loop::backends::build_local_backends(cfg.clone()).await?;
```

(`feature = "primer-speech/X"` checks the propagated feature in the dep — same pattern as the existing cfg arms. If the existing arms use a different idiom, mirror that idiom exactly.)

- [ ] **Step 4: Build under each combination**

```bash
~/.cargo/bin/cargo build -p primer-cli --features speech 2>&1 | tail -3
~/.cargo/bin/cargo build -p primer-cli --features speech,macos-native 2>&1 | tail -3
~/.cargo/bin/cargo build -p primer-cli --features speech,macos-native-26 2>&1 | tail -3
```
Expected: all three succeed.

```bash
~/.cargo/bin/cargo build -p primer-cli --features speech,macos-native,macos-native-26 2>&1 | grep -i 'mutually exclusive'
```
Expected: prints the compile_error.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-cli/Cargo.toml src/crates/primer-cli/src/main.rs
git commit -m "cli: macos-native-26 feature + runtime cfg arm"
```

---

### Task 14: GUI feature propagation

**Files:**
- Modify: `src/crates/primer-gui/Cargo.toml`
- Modify: `src/crates/primer-gui/src/lib.rs`

- [ ] **Step 1: Add the feature**

In `crates/primer-gui/Cargo.toml`, alongside the existing `macos-native` propagation:

```toml
macos-native-26 = ["primer-speech/macos-native-26"]
```

- [ ] **Step 2: Add compile_error to `primer-gui/src/lib.rs`**

```rust
#[cfg(all(feature = "macos-native", feature = "macos-native-26"))]
compile_error!(
    "`macos-native` and `macos-native-26` are mutually exclusive — pick one"
);
```

- [ ] **Step 3: Add runtime cfg arm in the GUI's voice backend builder**

```bash
grep -n 'build_local_backends_macos_native\|build_local_backends(' crates/primer-gui/src/voice/backends.rs
```

Mirror the three-arm pattern from the CLI (Task 13 Step 3) in whatever GUI file dispatches between the builders.

- [ ] **Step 4: Build under each combination**

```bash
~/.cargo/bin/cargo build -p primer-gui --features speech 2>&1 | tail -3
~/.cargo/bin/cargo build -p primer-gui --features speech,macos-native 2>&1 | tail -3
~/.cargo/bin/cargo build -p primer-gui --features speech,macos-native-26 2>&1 | tail -3
```
Expected: all clean.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-gui/Cargo.toml src/crates/primer-gui/src/lib.rs \
        src/crates/primer-gui/src/voice/backends.rs
git commit -m "gui: macos-native-26 feature + runtime cfg arm"
```

---

### Task 15: Integration smoke test (#[ignore])

**Files:**
- Create or modify: `src/crates/primer-speech/tests/macos26_smoke.rs`

This is a `#[ignore]`'d test that constructs a `Macos26Stt`, opens a session (which constructs the pipeline → triggers asset install on first run), and verifies the pipeline reaches the listening state. Not in CI; developer runs on demand.

- [ ] **Step 1: Write the test**

Create `crates/primer-speech/tests/macos26_smoke.rs`:

```rust
//! End-to-end smoke for the macos-native-26 path. Not in CI; gated
//! `#[ignore]` because it requires macOS 26 + an installed locale model
//! (en-US ships with the OS; de-DE downloads on first run).
//!
//! Run with:
//!   cargo test -p primer-speech --features macos-native-26 \
//!       --test macos26_smoke -- --ignored --nocapture

#![cfg(all(target_os = "macos", feature = "macos-native-26"))]

use primer_core::i18n::Locale;
use primer_speech::macos26::Macos26Stt;

#[tokio::test]
#[ignore = "requires macOS 26 host; not in CI"]
async fn construct_en_us() {
    let stt = Macos26Stt::new(Locale::English).await.expect("en-US Macos26Stt constructs");
    assert_eq!(stt.locale(), Locale::English);
}

#[tokio::test]
#[ignore = "requires macOS 26 host; first run downloads de-DE model (~hundreds of MB)"]
async fn construct_de_de_triggers_download_if_missing() {
    // The actual download happens inside Macos26Pipeline.new (called by
    // the consumer loop), not in Macos26Stt::new. This test just pins
    // the construction-time validation.
    let stt = Macos26Stt::new(Locale::German).await.expect("de-DE Macos26Stt constructs");
    assert_eq!(stt.locale(), Locale::German);
}

#[tokio::test]
#[ignore = "requires macOS 26 host"]
async fn hindi_rejected_with_helpful_error() {
    let err = Macos26Stt::new(Locale::Hindi).await.unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("Hindi"));
    assert!(msg.contains("Whisper"));
}
```

- [ ] **Step 2: Run the (ignored) test explicitly**

```bash
~/.cargo/bin/cargo test -p primer-speech --features macos-native-26 \
    --test macos26_smoke -- --ignored --nocapture
```
Expected: 3 tests pass (assuming the host actually is macOS 26).

- [ ] **Step 3: Verify regular test runs do NOT run these**

```bash
~/.cargo/bin/cargo test -p primer-speech --features macos-native-26 --test macos26_smoke 2>&1 | tail -5
```
Expected: 0 passed, 3 ignored.

- [ ] **Step 4: Commit**

```bash
git add src/crates/primer-speech/tests/macos26_smoke.rs
git commit -m "speech(macos26): ignored integration smoke tests"
```

---

### Task 16: CLAUDE.md note + manual round-trip

**Files:**
- Modify: `CLAUDE.md` (repo root, NOT under `src/`)

- [ ] **Step 1: Do a manual end-to-end round-trip first**

Before writing the docs, verify the feature actually works end-to-end. From the worktree root:

```bash
cd src && ~/.cargo/bin/cargo run \
    --features primer-cli/speech,primer-cli/macos-native-26 \
    --bin primer -- --backend stub --speech --language en --name Smoke --age 8
```

Say something like "what colour is the sky" into the mic. Expect:
- Streaming partials appearing in stdout / verbose log as words land.
- The Primer responds (stub backend gives canned Socratic responses).
- Saying "bye" cleanly exits.

If anything is broken end-to-end, return to the relevant task before writing docs.

- [ ] **Step 2: Add the CLAUDE.md note**

Find the existing macOS-related section in `CLAUDE.md` (search for "macos-native" or "AVSpeechSynthesizer"). Append the following paragraph as its sibling:

```markdown
- **`macos-native-26` is the new SpeechAnalyzer-backed feature** for macOS 26+ (and iOS 26+ once host scaffolding exists). Mutually exclusive with `macos-native`. Build with `~/.cargo/bin/cargo build --features primer-cli/speech,primer-cli/macos-native-26`. Drops `silero` + `whisper` + `ort` from the build entirely; STT and derived VAD events come from a Swift sidecar (`crates/primer-speech/swift-sources/Macos26Pipeline.swift`) bridged via `swift-bridge` and compiled to a static lib by `crates/primer-speech/build.rs`. Build prerequisites grow by one item: `swiftc` on PATH (Xcode-bundled; same as building `spikes/macos26_speech/`). TTS reuses `MacosTextToSpeech` from the existing `macos/tts.rs` (AVSpeechSynthesizer is unchanged across macOS 13+/26+). Locale support: `en-US` and `de-DE`; Hindi errors loudly at construction because `SpeechTranscriber` doesn't support `hi-IN` as of macOS 26.5. Asset download trusts Apple's OS-managed flow silently — the macOS path is the friction-free demo surface, not production children's hardware. Empirical A/B vs Whisper (PR #131): ~100× faster time-to-first-partial (~30 ms vs ~3.8 s), ~2× faster final transcript (~800 ms vs ~1.8 s). Module is internally Apple-portable via `cfg(target_vendor = "apple")`; module rename `macos26/` → `apple26/` is a mechanical follow-up once an iOS host application exists. See [docs/superpowers/specs/2026-05-20-macos-native-26-design.md](docs/superpowers/specs/2026-05-20-macos-native-26-design.md) for the full design.
```

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: CLAUDE.md note on macos-native-26 feature"
```

- [ ] **Step 4: Push the branch**

```bash
git push -u origin claude/macos-native-26
```

- [ ] **Step 5: (Optional) Open a draft PR for review**

```bash
gh pr create --draft --title "speech(macos): macos-native-26 — SpeechAnalyzer STT/VAD" \
    --body-file docs/superpowers/specs/2026-05-20-macos-native-26-design.md
```

(Draft because per the project owner's request the branch sits until manual smoke confirms the end-to-end round-trip on macOS 26.5. Flip to "ready" once that's done.)

---

## Notes on TDD discipline

- **Tasks 2, 3, 4** are pure-logic and TDD-shaped. Tests first, then implementation, both committed in the same task.
- **Task 5 (Swift sidecar)** can't be Rust-TDD'd directly — the smoke test in Task 15 covers it.
- **Tasks 6, 7, 8** are FFI plumbing where the `cargo build` IS the test.
- **Task 9** is shim code with no behaviour that can be unit-tested in isolation.
- **Tasks 12, 13, 14** are wiring; the smoke test in Task 15 is their integration test.

When in doubt, prefer the smaller behaviour-pinning unit test over the larger integration test. The DerivedVadStateMachine tests in Task 3 are the highest-leverage tests in this plan.

## Notes on commits

Each task ends with one commit. Don't batch — 16 small commits make bisecting easy if a manual round-trip surfaces a regression at the end.
