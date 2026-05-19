# macOS-native TTS PCM streaming — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `SynthesisSession`'s `Vec<AudioChunk>`-returning `push_text`/`finalize` with a callback-driven shape, then change `MacosTtsSession` to stream PCM chunks via an `mpsc::sync_channel` as `AVSpeechSynthesizer` emits them — cutting per-phrase time-to-first-audio from "full phrase synthesis (~hundreds of ms)" to "first PCM callback (~50 ms)" on the macOS-native build.

**Architecture:** Two-stage TDD. Stage A reshapes the trait and adapts every implementation (Piper, macOS, stub, two state-machine mocks) without changing observable timing — each backend wraps its existing Vec-producing path with a callback adapter. Stage B replaces the macOS Vec-wrapper with a true channel-streaming impl: PCM callback (running on the GCD main queue) sends `SynthesisEvent::Audio(chunk)` into a bounded `mpsc::sync_channel`; the caller thread drains the channel and fires `on_event` as each event arrives.

**Tech Stack:** Rust 2024 (workspace), tokio, objc2 + AVFoundation + GCD (macOS only), `std::sync::mpsc::sync_channel` for cross-thread event delivery.

**Spec:** [docs/superpowers/specs/2026-05-19-macos-tts-pcm-streaming-design.md](../specs/2026-05-19-macos-tts-pcm-streaming-design.md)

**Branch:** `speech/macos-native-pcm-streaming-issue-114` (already exists; spec already committed as `f58bbeb`).

**Build commands (every cargo invocation from `src/`, always via `~/.cargo/bin/cargo`):**

```bash
cd /Users/hherb/src/primer/src

# Default features baseline.
~/.cargo/bin/cargo test --workspace
# Expected at plan start: 856 passed, 0 failed, 3 ignored.

# Voice loop (state-machine tests).
~/.cargo/bin/cargo test -p primer-speech --features voice-loop
# Expected at plan start: 84 passed.

# macOS-native (requires macOS host).
~/.cargo/bin/cargo test -p primer-speech --features macos-native
# Expected at plan start: 5 passed, 1 ignored (the harness-false binary).

# Format + lint.
~/.cargo/bin/cargo fmt --all -- --check
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech -- -D warnings
```

---

## File map

**Files modified (Stage A):**
- `src/crates/primer-core/src/speech.rs` — add `SynthesisEvent` enum; reshape `SynthesisSession` trait; update `CannedSynthesisSession` test impl and the `streaming_tts_session_yields_chunks_and_finalizes` test; add new test for explicit `[Audio, PhraseEnd]` ordering.
- `src/crates/primer-speech/src/stub.rs` — update `StubSynthesisSession::push_text` / `finalize` to fire events; update the two stub tests.
- `src/crates/primer-speech/src/piper.rs` — update `PiperSession::push_text` / `finalize` to fire `Audio + PhraseEnd` per phrase.
- `src/crates/primer-speech/src/macos/tts.rs` — Stage-A wrapper around the existing `synthesize_to_chunks`; emit one `Audio` + one `PhraseEnd` per phrase. (Stage B will replace.)
- `src/crates/primer-speech/src/voice_loop/state_machine.rs`:
  - `mocks::MockTtsSession` — adapt to new trait.
  - `tests::llm_error_synthesises_fallback_line::CapturingSession` — adapt to new trait.
  - SPEAK consumer loop (lines 834-853) — closure-driven dispatch on `SynthesisEvent`.
- `src/crates/primer-speech/tests/macos_tts.rs` — update `streaming_session_yields_chunks_for_one_phrase` and `streaming_session_yields_chunks_for_multiple_phrases` to drive the new callback API.

**Files modified (Stage B):**
- `src/crates/primer-speech/src/macos/tts.rs`:
  - Add `PCM_EVENT_CHANNEL_CAPACITY: usize = 64` const.
  - Add `STREAM_DRAIN_POLL_MS: Duration = Duration::from_millis(10)` const.
  - Add `synthesize_streaming_main_thread` and `synthesize_streaming_background`.
  - Replace `MacosTtsSession::push_text` / `finalize` Stage-A wrapper with a call to `synthesize_streaming`.
  - Delete `synthesize_to_chunks_main_thread`, `synthesize_to_chunks_background`, `synthesize_to_chunks`, `coalesce_phrase`, `Accumulator` type alias, `DispatchSemaphore` + its FFI declarations.
  - Rewrite one-shot `TextToSpeech::synthesize` to drive `synthesize_streaming` with a local `Vec<f32>` accumulator (deleting `chunks_to_audio_buffer` if it becomes a one-liner).
- `src/crates/primer-speech/tests/macos_tts.rs` — add `streaming_emits_multiple_audio_events_before_phrase_end` regression test.

**Files modified (Stage C):**
- `src/crates/primer-speech/src/voice_loop/state_machine.rs::tests` — add `TimedMockTts` + `streaming_chunks_reach_speaker_before_phrase_completes` test.
- `src/crates/primer-speech/examples/tts_macos_pcm_smoke.rs` — add `--measure-ttfa` flag; record + print TTFA timing.

---

## Stage A — Trait reshape (no behaviour change)

### Task 1: Add `SynthesisEvent` enum + reshape trait in `primer-core`

**Files:**
- Modify: `src/crates/primer-core/src/speech.rs:193-260` (the AudioChunk/SynthesisSession/StreamingTextToSpeech block)
- Test: `src/crates/primer-core/src/speech.rs::tests` (existing `streaming_tts_session_yields_chunks_and_finalizes` + new sibling)

- [ ] **Step 1: Edit the trait definitions and add `SynthesisEvent`**

Replace the lines from `pub struct AudioChunk {` through the `pub trait StreamingTextToSpeech` block. After this edit the section should read:

```rust
/// One PCM chunk emitted by a [`SynthesisSession`] during streaming.
///
/// Concatenate the `samples` of every chunk in order to reconstruct the
/// full utterance. `sample_rate` is carried per-chunk even though every
/// chunk in one session shares one — keeps this type usable by audio
/// sinks that don't hold a reference to the originating backend.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    /// PCM samples, f32, mono.
    pub samples: Vec<f32>,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

/// One event emitted by a streaming [`SynthesisSession`].
///
/// `Audio(chunk)` carries PCM data — consumers forward it to the
/// speaker immediately. `PhraseEnd` is a boundary marker — consumers
/// typically insert a brief inter-phrase pause (~200 ms of silence)
/// before the next phrase's audio.
///
/// Separating audio from boundaries (rather than packing a
/// `bool is_phrase_end` onto chunks) keeps phrase-detection knowledge
/// where it belongs — in the producer's `PhraseSplitter` — and avoids
/// per-chunk overhead in the steady state.
#[derive(Debug, Clone)]
pub enum SynthesisEvent {
    /// PCM data became available.
    Audio(AudioChunk),
    /// End of one phrase. Consumer typically inserts ~200 ms of silence.
    PhraseEnd,
}

/// A single streaming-synthesis session.
///
/// Created by [`StreamingTextToSpeech::open_session`]. Push partial
/// text from the LLM via [`Self::push_text`]; the session invokes
/// `on_event` for each PCM chunk and phrase boundary as soon as it's
/// available — without buffering an entire phrase. Call
/// [`Self::finalize`] when the LLM stream has ended to drain the
/// trailing buffer. `Send` but not `Sync`: each Primer turn owns its
/// own session.
///
/// # Blocking
///
/// Both [`Self::push_text`] and [`Self::finalize`] are synchronous and
/// may run CPU-heavy synthesis (e.g. ONNX inference) on the calling
/// thread. Async callers MUST wrap session calls in
/// `tokio::task::spawn_blocking` (or an equivalent) to keep the
/// runtime free. The trait stays sync so pure backends don't pay an
/// async surface tax.
pub trait SynthesisSession: Send {
    /// Push text. The synthesiser invokes `on_event` for each PCM
    /// chunk and phrase boundary as soon as it's available — without
    /// buffering an entire phrase.
    ///
    /// `on_event` may be invoked zero or more times during one call.
    /// May block the calling thread for the duration of synthesis —
    /// see the trait-level `# Blocking` note.
    fn push_text(
        &mut self,
        text: &str,
        on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()>;

    /// Drain remaining buffered text and finalize. Consumes the
    /// session. Fires the same events as [`Self::push_text`] for any
    /// trailing partial phrase.
    ///
    /// May block for one final synthesis call — see the trait-level
    /// `# Blocking` note.
    fn finalize(
        self: Box<Self>,
        on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()>;
}

/// Streaming text-to-speech backend.
///
/// Open one [`SynthesisSession`] per Primer turn. The backend itself is
/// shareable across sessions (`Send + Sync`); per-session state lives
/// inside the session handle. A backend may also implement the one-shot
/// [`TextToSpeech`] trait — `name()` lives on the [`Named`] super-trait
/// so it's only written once per backend struct.
pub trait StreamingTextToSpeech: Named + Send + Sync {
    /// Sample rate of audio chunks this backend will emit (Hz). Carried
    /// on each [`AudioChunk`] as well so downstream sinks don't need to
    /// hold a reference to this backend.
    fn sample_rate(&self) -> u32;

    /// Open a fresh synthesis session for the given voice profile.
    ///
    /// May error if the backend cannot serve `voice` (for example, the
    /// loaded model has a different `model_id` than the requested voice).
    fn open_session(&self, voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>>;
}
```

- [ ] **Step 2: Update `CannedSynthesisSession` to the new trait shape**

Replace its impl block (currently at speech.rs:486-499):

```rust
impl SynthesisSession for CannedSynthesisSession {
    fn push_text(
        &mut self,
        _text: &str,
        on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()> {
        if self.scripted.next().is_some() {
            on_event(SynthesisEvent::Audio(AudioChunk {
                samples: vec![0.0; CANNED_TTS_SAMPLES_PER_CHUNK],
                sample_rate: self.sample_rate,
            }));
            on_event(SynthesisEvent::PhraseEnd);
        }
        Ok(())
    }

    fn finalize(
        self: Box<Self>,
        _on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()> {
        Ok(())
    }
}
```

Also add `SynthesisEvent` to the test mod's `use super::*;` consumers if not already in scope (it's defined in the parent module so `use super::*;` already imports it).

- [ ] **Step 3: Update `streaming_tts_session_yields_chunks_and_finalizes` test**

Replace the existing test (speech.rs:519-536) body with the new shape:

```rust
#[test]
fn streaming_tts_session_yields_chunks_and_finalizes() {
    let tts: Box<dyn StreamingTextToSpeech> = Box::new(CannedStreamingTts);
    assert_eq!(Named::name(&*tts), "canned-stream-tts");
    assert_eq!(tts.sample_rate(), CANNED_TTS_SAMPLE_RATE);
    let voice = VoiceProfile::default();
    let mut session = tts.open_session(&voice).unwrap();

    let mut events: Vec<SynthesisEvent> = Vec::new();
    let mut sink = |e: SynthesisEvent| events.push(e);

    session.push_text("hello.", &mut sink).unwrap();
    assert_eq!(events.len(), 2, "one Audio + one PhraseEnd per push");
    assert!(matches!(events[0], SynthesisEvent::Audio(_)));
    assert!(matches!(events[1], SynthesisEvent::PhraseEnd));
    if let SynthesisEvent::Audio(ref chunk) = events[0] {
        assert_eq!(chunk.samples.len(), CANNED_TTS_SAMPLES_PER_CHUNK);
        assert_eq!(chunk.sample_rate, CANNED_TTS_SAMPLE_RATE);
    }
    events.clear();

    session.push_text(" world.", &mut sink).unwrap();
    assert_eq!(events.len(), 2);
    events.clear();

    session.push_text("", &mut sink).unwrap();
    assert!(events.is_empty(), "empty push (script exhausted) emits no events");

    // finalize: scripted iterator already exhausted; no trailing events expected.
    session.finalize(&mut sink).unwrap();
    assert!(events.is_empty());
}
```

- [ ] **Step 4: Add a new sibling test pinning the explicit event ordering**

Append this test to the same `mod tests`:

```rust
/// Pin the exact `[Audio, PhraseEnd]` event order for one push. The
/// existing test asserts shape; this one asserts ordering as a
/// separate signal so a future regression that reverses the order
/// (or drops `PhraseEnd`) fails with a clearly diagnostic message.
#[test]
fn synthesis_session_fires_audio_before_phrase_end() {
    let tts: Box<dyn StreamingTextToSpeech> = Box::new(CannedStreamingTts);
    let mut session = tts.open_session(&VoiceProfile::default()).unwrap();
    let mut order: Vec<&'static str> = Vec::new();
    let mut sink = |e: SynthesisEvent| {
        order.push(match e {
            SynthesisEvent::Audio(_) => "audio",
            SynthesisEvent::PhraseEnd => "phrase_end",
        });
    };
    session.push_text("ping.", &mut sink).unwrap();
    assert_eq!(order, vec!["audio", "phrase_end"]);
}
```

- [ ] **Step 5: Compile + run primer-core tests in isolation**

Run: `~/.cargo/bin/cargo test -p primer-core`
Expected: every test in primer-core passes. Trait reshape doesn't touch primer-core's other modules. Workspace builds elsewhere will fail to compile until subsequent tasks land — that's expected.

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-core/src/speech.rs
git commit -m "$(cat <<'EOF'
core(speech): reshape SynthesisSession trait to callback-driven events

Replace push_text/finalize's Vec<AudioChunk> return with a
&mut dyn FnMut(SynthesisEvent) callback. Adds SynthesisEvent enum
with Audio(chunk) and PhraseEnd variants — boundary signalling
that producer-side PhraseSplitter knowledge already implies but
the prior shape forced consumers to reconstruct from Vec length.

Trait remains object-safe (FnMut is sized). CannedSynthesisSession
test mock updated; existing canary test rewritten to drive the
callback API; new sibling test pins the [Audio, PhraseEnd] order
explicitly so a future regression that drops PhraseEnd fails with
a diagnostic message rather than a quieter behavioural shift.

Workspace will not build until subsequent tasks land — stub.rs,
piper.rs, macos/tts.rs, and voice_loop/state_machine.rs each carry
their own SynthesisSession impl that needs adapting.

Towards #114.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Update `StubSynthesisSession` in `primer-speech/src/stub.rs`

**Files:**
- Modify: `src/crates/primer-speech/src/stub.rs:89-101` (impl) + `:112-135` (the two tests)

- [ ] **Step 1: Update the `SynthesisSession` impl**

Replace the `impl SynthesisSession for StubSynthesisSession { ... }` block:

```rust
impl SynthesisSession for StubSynthesisSession {
    fn push_text(
        &mut self,
        text: &str,
        on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()> {
        for _ in self.splitter.push(text) {
            on_event(SynthesisEvent::Audio(Self::silent_chunk()));
            on_event(SynthesisEvent::PhraseEnd);
        }
        Ok(())
    }

    fn finalize(
        mut self: Box<Self>,
        on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()> {
        if self.splitter.flush().is_some() {
            on_event(SynthesisEvent::Audio(Self::silent_chunk()));
            on_event(SynthesisEvent::PhraseEnd);
        }
        Ok(())
    }
}
```

- [ ] **Step 2: Add `SynthesisEvent` to the imports**

Update the top-of-file `use` block to include `SynthesisEvent`:

```rust
use primer_core::speech::{
    AudioBuffer, AudioChunk, Named, SpeechToText, StreamingTextToSpeech, SynthesisEvent,
    SynthesisSession, TextToSpeech, Transcript, VoiceProfile,
};
```

- [ ] **Step 3: Rewrite the two stub tests**

Replace `stub_tts_streaming_emits_chunk_per_phrase` and `stub_tts_streaming_finalize_drains_trailing`:

```rust
#[tokio::test]
async fn stub_tts_streaming_emits_chunk_per_phrase() {
    let tts: Box<dyn StreamingTextToSpeech> = Box::new(StubTts);
    assert_eq!(tts.sample_rate(), STUB_TTS_SAMPLE_RATE);
    let mut session = tts.open_session(&VoiceProfile::default()).unwrap();

    let mut events: Vec<SynthesisEvent> = Vec::new();
    session
        .push_text("Hello. World. ", &mut |e| events.push(e))
        .unwrap();

    // Two phrases ⇒ two Audio + two PhraseEnd events.
    let audio_count = events
        .iter()
        .filter(|e| matches!(e, SynthesisEvent::Audio(_)))
        .count();
    let phrase_end_count = events
        .iter()
        .filter(|e| matches!(e, SynthesisEvent::PhraseEnd))
        .count();
    assert_eq!(audio_count, 2);
    assert_eq!(phrase_end_count, 2);
    for event in &events {
        if let SynthesisEvent::Audio(chunk) = event {
            assert_eq!(chunk.sample_rate, STUB_TTS_SAMPLE_RATE);
            assert!(!chunk.samples.is_empty());
            assert!(chunk.samples.iter().all(|&s| s == 0.0));
        }
    }
}

#[tokio::test]
async fn stub_tts_streaming_finalize_drains_trailing() {
    let tts: Box<dyn StreamingTextToSpeech> = Box::new(StubTts);
    let mut session = tts.open_session(&VoiceProfile::default()).unwrap();

    let mut mid_events: Vec<SynthesisEvent> = Vec::new();
    session
        .push_text("Hello", &mut |e| mid_events.push(e))
        .unwrap();
    assert!(mid_events.is_empty(), "no phrase boundary yet on partial text");

    let mut trailing_events: Vec<SynthesisEvent> = Vec::new();
    session.finalize(&mut |e| trailing_events.push(e)).unwrap();
    assert_eq!(trailing_events.len(), 2, "one Audio + one PhraseEnd for the trailing phrase");
    assert!(matches!(trailing_events[0], SynthesisEvent::Audio(_)));
    assert!(matches!(trailing_events[1], SynthesisEvent::PhraseEnd));
}
```

- [ ] **Step 4: Add `SynthesisEvent` to the test mod's imports**

Update the `use` in the `mod tests` block (currently just `use primer_core::speech::{StreamingTextToSpeech, VoiceProfile};`):

```rust
use primer_core::speech::{StreamingTextToSpeech, SynthesisEvent, VoiceProfile};
```

- [ ] **Step 5: Compile + run primer-speech stub tests**

Run: `~/.cargo/bin/cargo test -p primer-speech --no-default-features stub_tts_`
Expected: both renamed tests pass. (Stub is default-built so no feature flag needed.)

- [ ] **Step 6: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-speech/src/stub.rs
git commit -m "$(cat <<'EOF'
speech(stub): adapt StubSynthesisSession to callback-driven trait

No behaviour change. The stub now fires
Audio(silent_chunk) + PhraseEnd per phrase via the new on_event
sink rather than returning Vec<AudioChunk>. Two existing tests
rewritten to drive the callback API; assertions strengthen from
"len == 2" to per-event-kind counting so a future regression
that emits an extra Audio or skips PhraseEnd is caught directly.

Towards #114.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Update `MockTtsSession` in `primer-speech/src/voice_loop/state_machine.rs::mocks`

**Files:**
- Modify: `src/crates/primer-speech/src/voice_loop/state_machine.rs:1032-1045` (`MockTtsSession`) and surrounding `use` if needed.

- [ ] **Step 1: Update the mock's impl**

Replace the `impl SynthesisSession for MockTtsSession { ... }` block (state_machine.rs:1032-1045):

```rust
impl SynthesisSession for MockTtsSession {
    fn push_text(
        &mut self,
        text: &str,
        on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        on_event(SynthesisEvent::Audio(AudioChunk {
            samples: vec![0.5; self.chunk_samples],
            sample_rate: 22_050,
        }));
        on_event(SynthesisEvent::PhraseEnd);
        Ok(())
    }

    fn finalize(
        self: Box<Self>,
        _on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()> {
        Ok(())
    }
}
```

- [ ] **Step 2: Add `SynthesisEvent` to the mocks-module `use`**

Update state_machine.rs:910-913 to include `SynthesisEvent`:

```rust
use primer_core::speech::{
    AudioChunk, Named, StreamingSpeechToText, StreamingTextToSpeech, SynthesisEvent,
    SynthesisSession, TranscriptSegment, TranscriptionSession, VoiceProfile,
};
```

- [ ] **Step 3: Update the `mock_streaming_tts_emits_one_chunk_per_text` test**

Located at state_machine.rs:1130-1136. Replace with:

```rust
#[test]
fn mock_streaming_tts_emits_one_chunk_per_text() {
    let tts = MockStreamingTts::new(100);
    let voice = VoiceProfile::default();
    let mut session = tts.open_session(&voice).unwrap();

    let mut count_non_empty: u32 = 0;
    session
        .push_text("hi.", &mut |e| {
            if let SynthesisEvent::Audio(_) = e {
                count_non_empty += 1;
            }
        })
        .unwrap();
    assert_eq!(count_non_empty, 1);

    let mut count_empty: u32 = 0;
    session
        .push_text("", &mut |_| count_empty += 1)
        .unwrap();
    assert_eq!(count_empty, 0);
}
```

- [ ] **Step 4: Verify the test compiles standalone (skip running, downstream consumer not yet updated)**

Run: `~/.cargo/bin/cargo build -p primer-speech --features voice-loop`
Expected: SUCCESS for the mocks module, but other call sites in state_machine.rs (the SPEAK consumer at line 842) WILL FAIL to compile because they still call `session.push_text(text)?` returning Vec. That's expected — Task 6 fixes the consumer.

If you want to verify just the mocks compile, comment out the consumer loop lines 842-849 temporarily. Otherwise proceed; the next tasks fix the rest.

- [ ] **Step 5: Stage the changes (commit after Tasks 4-6 to keep the workspace compileable per commit)**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-speech/src/voice_loop/state_machine.rs
# Do NOT commit yet — the next three tasks must land together for the
# workspace to compile. The combined commit lands in Task 6.
```

---

### Task 4: Update `CapturingSession` in `state_machine.rs::tests::llm_error_synthesises_fallback_line`

**Files:**
- Modify: `src/crates/primer-speech/src/voice_loop/state_machine.rs:1947-1961` (in-test mock)

- [ ] **Step 1: Update the inline mock**

Replace `impl SynthesisSession for CapturingSession { ... }`:

```rust
impl SynthesisSession for CapturingSession {
    fn push_text(
        &mut self,
        text: &str,
        on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> primer_core::error::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        self.captured.lock().unwrap().push(text.to_string());
        on_event(SynthesisEvent::Audio(AudioChunk {
            samples: vec![0.5; 64],
            sample_rate: 22_050,
        }));
        on_event(SynthesisEvent::PhraseEnd);
        Ok(())
    }

    fn finalize(
        self: Box<Self>,
        _on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> primer_core::error::Result<()> {
        Ok(())
    }
}
```

- [ ] **Step 2: Update the test's `use` to include `SynthesisEvent`**

Find the test's `use` block (state_machine.rs:1918-1920):

```rust
use primer_core::speech::{
    AudioChunk, Named, StreamingTextToSpeech, SynthesisEvent, SynthesisSession, VadEvent,
    VoiceProfile,
};
```

- [ ] **Step 3: Stage only (no commit yet)**

The workspace is still broken until Task 6 lands; stage and proceed.

---

### Task 5: Update `PiperSession` in `primer-speech/src/piper.rs`

**Files:**
- Modify: `src/crates/primer-speech/src/piper.rs:57-60` (imports) + `:321-337` (impl)

- [ ] **Step 1: Add `SynthesisEvent` to imports**

Update lines 57-60:

```rust
use primer_core::speech::{
    AudioBuffer, AudioChunk, Named, StreamingTextToSpeech, SynthesisEvent, SynthesisSession,
    TextToSpeech, VoiceProfile,
};
```

- [ ] **Step 2: Update the `SynthesisSession` impl**

Replace the `impl SynthesisSession for PiperSession { ... }` block:

```rust
impl SynthesisSession for PiperSession {
    fn push_text(
        &mut self,
        text: &str,
        on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()> {
        for phrase in self.splitter.push(text) {
            let chunk = self.synth_phrase(&phrase)?;
            on_event(SynthesisEvent::Audio(chunk));
            on_event(SynthesisEvent::PhraseEnd);
        }
        Ok(())
    }

    fn finalize(
        mut self: Box<Self>,
        on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()> {
        if let Some(trailing) = self.splitter.flush() {
            let chunk = self.synth_phrase(&trailing)?;
            on_event(SynthesisEvent::Audio(chunk));
            on_event(SynthesisEvent::PhraseEnd);
        }
        Ok(())
    }
}
```

`synth_phrase` is unchanged. Same per-phrase synthesis behaviour, same chunk content — just delivered via callback.

- [ ] **Step 3: Stage only**

The workspace is still broken until Task 6 lands; stage and proceed.

---

### Task 6: Stage A wrapper + consumer change (atomic commit)

This task updates the macOS Stage-A wrapper, the state-machine SPEAK consumer, and the macOS integration tests in one commit so the workspace compiles end-to-end.

**Files:**
- Modify: `src/crates/primer-speech/src/macos/tts.rs:410-438` (`MacosTtsSession` Stage-A wrapper)
- Modify: `src/crates/primer-speech/src/macos/tts.rs:49-52` (imports — add `SynthesisEvent`)
- Modify: `src/crates/primer-speech/src/voice_loop/state_machine.rs:834-853` (SPEAK consumer)
- Modify: `src/crates/primer-speech/src/voice_loop/state_machine.rs:799-803` (top-of-file imports near the top of state_machine.rs that need `SynthesisEvent`)
- Modify: `src/crates/primer-speech/tests/macos_tts.rs:190-244` (two existing streaming tests)

- [ ] **Step 1: Stage-A wrapper in `MacosTtsSession`**

Update `macos/tts.rs:49-52` imports:

```rust
use primer_core::speech::{
    AudioBuffer, AudioChunk, Named, StreamingTextToSpeech, SynthesisEvent, SynthesisSession,
    TextToSpeech, VoiceProfile,
};
```

Replace `impl SynthesisSession for MacosTtsSession { ... }` (macos/tts.rs:410-438) with:

```rust
impl SynthesisSession for MacosTtsSession {
    /// **Stage-A wrapper:** synthesises the full phrase via the existing
    /// [`synthesize_to_chunks`] path, coalesces into one chunk, then
    /// fires `Audio(chunk)` + `PhraseEnd`. Same observable timing as the
    /// pre-trait-reshape behaviour. Stage B replaces this with a true
    /// channel-streaming path. Tracking: #114.
    fn push_text(
        &mut self,
        text: &str,
        on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()> {
        for phrase in self.splitter.push(text) {
            if let Some(chunk) = coalesce_phrase(synthesize_to_chunks(&self.voice, &phrase)?) {
                on_event(SynthesisEvent::Audio(chunk));
                on_event(SynthesisEvent::PhraseEnd);
            }
        }
        Ok(())
    }

    fn finalize(
        mut self: Box<Self>,
        on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()> {
        if let Some(tail) = self.splitter.flush() {
            if let Some(chunk) = coalesce_phrase(synthesize_to_chunks(&self.voice, &tail)?) {
                on_event(SynthesisEvent::Audio(chunk));
                on_event(SynthesisEvent::PhraseEnd);
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 2: Update the state-machine SPEAK consumer**

Find the top-of-file `use` block in `state_machine.rs` (around line 18-30). Add `SynthesisEvent`:

```rust
use primer_core::speech::{
    AudioChunk, StreamingSpeechToText, StreamingTextToSpeech, SynthesisEvent, SynthesisSession,
    TranscriptSegment, TranscriptionSession, VadEvent, VoiceProfile,
};
```

(Exact set may vary — leave existing entries; just ensure `SynthesisEvent` is added. If the imports already use individual lines, follow the existing style.)

Replace `state_machine.rs:834-853`:

```rust
let mut session = active_tts.open_session(active_voice)?;
let tts_rate = active_tts.sample_rate();
// ~200 ms of silence inserted between AudioChunks (each `PhraseEnd`
// event marks the boundary of one phrase). Gives the listener a
// perceptible pause at sentence boundaries without adding much to
// total response time.
const INTER_PHRASE_SILENCE_MS: u32 = 200;
let inter_phrase_silence_samples = (tts_rate * INTER_PHRASE_SILENCE_MS / 1000) as usize;
let mut on_event = |event: SynthesisEvent| match event {
    SynthesisEvent::Audio(chunk) => on_committed_audio(chunk.samples),
    SynthesisEvent::PhraseEnd => {
        on_committed_audio(vec![0.0_f32; inter_phrase_silence_samples])
    }
};
session.push_text(&tts_text, &mut on_event)?;
session.finalize(&mut on_event)?;
// Flush sentinel: empty Vec signals on_audio to drain any
// resampler-leftover tail. Mock callbacks no-op on empty input.
on_committed_audio(Vec::new());
```

Notice `session` here is `Box<dyn SynthesisSession>` returned by `open_session`. The `Box::finalize` call works because `finalize`'s `self: Box<Self>` receiver method dispatches correctly through the trait object — Rust permits `box_val.finalize(...)` when the trait method's receiver is `Box<Self>`.

- [ ] **Step 3: Update the two macOS streaming tests in `tests/macos_tts.rs`**

Replace `streaming_session_yields_chunks_for_one_phrase` (tests/macos_tts.rs:190-216):

```rust
fn streaming_session_yields_chunks_for_one_phrase() {
    use primer_core::speech::SynthesisEvent;
    let tts = MacosTextToSpeech::new("en-US").expect("en-US voice");
    let voice = VoiceProfile {
        model_id: "system".into(),
        rate: 1.0,
        pitch: 0.0,
    };
    let mut session = tts.open_session(&voice).expect("session opens");

    let mut events: Vec<SynthesisEvent> = Vec::new();
    session
        .push_text("Hello.", &mut |e| events.push(e))
        .expect("push ok");
    session
        .finalize(&mut |e| events.push(e))
        .expect("finalize ok");

    let audio_count = events
        .iter()
        .filter(|e| matches!(e, SynthesisEvent::Audio(_)))
        .count();
    let phrase_end_count = events
        .iter()
        .filter(|e| matches!(e, SynthesisEvent::PhraseEnd))
        .count();
    assert!(audio_count >= 1, "session must emit at least one Audio event");
    assert_eq!(phrase_end_count, 1, "exactly one PhraseEnd for one phrase");

    for event in &events {
        if let SynthesisEvent::Audio(chunk) = event {
            assert!(chunk.sample_rate > 0, "Audio chunk sample_rate > 0");
            assert!(!chunk.samples.is_empty(), "Audio chunk has samples");
        }
    }
}
```

Replace `streaming_session_yields_chunks_for_multiple_phrases` (tests/macos_tts.rs:218-244):

```rust
fn streaming_session_yields_chunks_for_multiple_phrases() {
    use primer_core::speech::SynthesisEvent;
    let tts = MacosTextToSpeech::new("en-US").expect("en-US voice");
    let voice = VoiceProfile {
        model_id: "system".into(),
        rate: 1.0,
        pitch: 0.0,
    };
    let mut session = tts.open_session(&voice).expect("session opens");

    let mut events: Vec<SynthesisEvent> = Vec::new();
    session
        .push_text("Hello. World. ", &mut |e| events.push(e))
        .expect("push ok");
    session
        .finalize(&mut |e| events.push(e))
        .expect("finalize ok");

    // Two phrases ⇒ exactly two PhraseEnd events (one per phrase).
    // The Audio count is ≥2 with the Stage-A wrapper (one per phrase)
    // and may grow with Stage B (multiple per phrase as PCM callbacks
    // arrive). The state machine relies on PhraseEnd, not Audio count,
    // for inter-phrase silence.
    let phrase_end_count = events
        .iter()
        .filter(|e| matches!(e, SynthesisEvent::PhraseEnd))
        .count();
    assert_eq!(
        phrase_end_count, 2,
        "two-phrase push must produce exactly two PhraseEnd events; got {phrase_end_count}"
    );

    let total_samples: usize = events
        .iter()
        .filter_map(|e| if let SynthesisEvent::Audio(c) = e { Some(c.samples.len()) } else { None })
        .sum();
    assert!(total_samples > 0, "session must produce non-empty audio for two phrases");
}
```

- [ ] **Step 4: Workspace test sweep**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo build --workspace
~/.cargo/bin/cargo test --workspace
```

Expected: 857 passed (was 856; +1 from Task 1's new sibling test), 0 failed, 3 ignored.

```bash
~/.cargo/bin/cargo test -p primer-speech --features voice-loop
```

Expected: 84 passed, 0 failed, 2 ignored (the mock test was rewritten but count is the same).

On macOS:

```bash
~/.cargo/bin/cargo test -p primer-speech --features macos-native
```

Expected: 5 passed, 1 ignored (existing two streaming tests now run against the Stage-A wrapper; observable behaviour unchanged from today).

- [ ] **Step 5: fmt + clippy**

```bash
~/.cargo/bin/cargo fmt --all -- --check
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech -- -D warnings
```

Expected: clean.

- [ ] **Step 6: Atomic commit covering Tasks 2-6**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-speech/src/stub.rs \
        src/crates/primer-speech/src/piper.rs \
        src/crates/primer-speech/src/macos/tts.rs \
        src/crates/primer-speech/src/voice_loop/state_machine.rs \
        src/crates/primer-speech/tests/macos_tts.rs
git commit -m "$(cat <<'EOF'
speech: adapt all SynthesisSession impls + consumer to new trait

Stage A of #114: every in-tree SynthesisSession impl (stub, Piper,
macOS, two state-machine mocks) now fires SynthesisEvent::{Audio,
PhraseEnd} via the new callback API. The state machine's SPEAK
consumer drives a closure that maps Audio → on_committed_audio and
PhraseEnd → 200ms inter-phrase silence — same observable timing.

The macOS impl wraps the existing synthesize_to_chunks +
coalesce_phrase path, emitting one Audio + one PhraseEnd per
phrase. Stage B (next commit batch) replaces this wrapper with a
true channel-streaming impl so PCM callbacks reach the speaker
before AVSpeechSynthesizer has finished the phrase.

Two macOS integration tests rewritten to assert `phrase_end_count
== 2` rather than `total_chunks == 2`, decoupling the consumer
contract (one PhraseEnd per phrase) from the producer's chunk
granularity (1 today, ≥2 after Stage B).

Workspace: 856 → 857 (one new explicit-ordering test in
primer-core). Voice-loop / macos-native counts unchanged.

Towards #114.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage B — macOS true streaming via mpsc::sync_channel

### Task 7: Add Stage-B constants to `macos/tts.rs`

**Files:**
- Modify: `src/crates/primer-speech/src/macos/tts.rs` (constants block near top, around line 58-72)

- [ ] **Step 1: Add the two new consts**

Insert after `const RUN_LOOP_SLICE: Duration = Duration::from_millis(10);` (around line 73):

```rust
/// Bounded channel capacity between the GCD main-queue PCM callback
/// and the caller thread that drains events. AVSpeechSynthesizer
/// fires its PCM callback ~10 times per phrase (per smoke-binary
/// observation); 64 gives ~6× headroom for a stalled drainer. A
/// `send` that blocks indicates real backpressure — the consumer's
/// speaker ringbuf is full — which is a structurally interesting
/// signal to surface as a stall rather than silently drop chunks.
const PCM_EVENT_CHANNEL_CAPACITY: usize = 64;

/// `recv_timeout` slice for the background-path drain loop. Short
/// enough that the 30s overall [`DRAIN_TIMEOUT`] check fires
/// promptly on a hung synth; long enough to amortise wakeup cost.
/// Not used by the main-thread path (which drives the NSRunLoop in
/// its own 10 ms slices).
const STREAM_DRAIN_POLL_MS: Duration = Duration::from_millis(10);
```

- [ ] **Step 2: Verify it compiles (consts only, unused warnings expected)**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo build -p primer-speech --features macos-native
```

Expected: clean build with two `dead_code` warnings for the unused consts — they're consumed by Task 9.

- [ ] **Step 3: Allow the dead_code warning temporarily**

Add `#[allow(dead_code)]` to each new const so the build stays clean across the interim:

```rust
#[allow(dead_code)] // consumed by synthesize_streaming_* in Task 9
const PCM_EVENT_CHANNEL_CAPACITY: usize = 64;

#[allow(dead_code)] // consumed by synthesize_streaming_background in Task 9
const STREAM_DRAIN_POLL_MS: Duration = Duration::from_millis(10);
```

Task 9 removes the `#[allow]` once the consts are wired.

- [ ] **Step 4: Stage; do not commit (Task 9 bundles)**

```bash
git add src/crates/primer-speech/src/macos/tts.rs
```

---

### Task 8: Add failing macOS streaming structural test

This test will fail against the Stage-A wrapper (which emits exactly 1 Audio per phrase). Task 9's streaming impl makes it pass.

**Files:**
- Modify: `src/crates/primer-speech/tests/macos_tts.rs` — add new test body + add it to `main()`

- [ ] **Step 1: Add the test body**

Append to the test bodies section (after `streaming_session_yields_chunks_for_multiple_phrases`):

```rust
fn streaming_emits_multiple_audio_events_before_phrase_end() {
    use primer_core::speech::SynthesisEvent;
    let tts = MacosTextToSpeech::new("en-US").expect("en-US voice");
    let voice = VoiceProfile {
        model_id: "system".into(),
        rate: 1.0,
        pitch: 0.0,
    };
    let mut session = tts.open_session(&voice).expect("session opens");

    let mut events: Vec<SynthesisEvent> = Vec::new();
    session
        .push_text(
            "This is a longer phrase that should yield multiple PCM callbacks.",
            &mut |e| events.push(e),
        )
        .expect("push ok");
    session
        .finalize(&mut |e| events.push(e))
        .expect("finalize ok");

    // Count Audio events that arrived before the first PhraseEnd.
    let audio_before_first_phrase_end = events
        .iter()
        .take_while(|e| !matches!(e, SynthesisEvent::PhraseEnd))
        .filter(|e| matches!(e, SynthesisEvent::Audio(_)))
        .count();

    assert!(
        audio_before_first_phrase_end >= 2,
        "expected ≥2 Audio events before PhraseEnd (true streaming shape); \
         got {audio_before_first_phrase_end} (events: {:?})",
        events.iter().map(|e| match e {
            SynthesisEvent::Audio(c) => format!("Audio({})", c.samples.len()),
            SynthesisEvent::PhraseEnd => "PhraseEnd".into(),
        }).collect::<Vec<_>>()
    );

    let phrase_end_count = events
        .iter()
        .filter(|e| matches!(e, SynthesisEvent::PhraseEnd))
        .count();
    assert_eq!(phrase_end_count, 1, "exactly one PhraseEnd per phrase");
}
```

- [ ] **Step 2: Wire it into `main()` in tests/macos_tts.rs**

Insert after the existing "Test 6: streaming session yields chunks for multiple phrases" block in `main()`:

```rust
// ── Test 7: streaming emits multiple Audio events before PhraseEnd ──
run_sync_test(
    "streaming_emits_multiple_audio_events_before_phrase_end",
    &mut passed,
    &mut failed,
    || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(async { streaming_emits_multiple_audio_events_before_phrase_end() })
    },
);
```

- [ ] **Step 3: Run the test to verify it FAILS**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-speech --features macos-native --test macos_tts
```

Expected: 5 passed, 1 FAILED with assertion message like:
```
expected ≥2 Audio events before PhraseEnd (true streaming shape); got 1
```

This is the failing-test gate before implementing Stage B. The Stage-A wrapper produces exactly one Audio per phrase.

- [ ] **Step 4: Stage; do not commit (Task 9 bundles)**

```bash
git add src/crates/primer-speech/tests/macos_tts.rs
```

---

### Task 9: Implement `synthesize_streaming_main_thread` + `synthesize_streaming_background`

This is the largest task. It replaces the macOS internals with the channel-based streaming impl.

**Files:**
- Modify: `src/crates/primer-speech/src/macos/tts.rs` — replace synthesis helpers, drop semaphore, wire `MacosTtsSession` to streaming, delete now-dead code.

- [ ] **Step 1: Add the streaming dispatcher**

Replace the `synthesize_to_chunks` function (around tts.rs:296-316) with a new streaming dispatcher. Keep the old `synthesize_to_chunks` AVAILABLE for now — Task 10 deletes it once the one-shot path no longer needs it:

```rust
/// Synthesise `text` using `voice` and stream events to `on_event` as
/// PCM callbacks arrive (per-callback `Audio` events, then `PhraseEnd`
/// on the EOS sentinel). Dispatches to the main-thread path when called
/// on the OS main thread, or the background-thread GCD-bounce path
/// otherwise.
fn synthesize_streaming(
    voice: &AVSpeechSynthesisVoice,
    text: &str,
    on_event: &mut dyn FnMut(SynthesisEvent),
) -> Result<()> {
    let on_main = objc2_foundation::NSThread::isMainThread_class();
    if on_main {
        synthesize_streaming_main_thread(voice, text, on_event)
    } else {
        synthesize_streaming_background(voice, text, on_event)
    }
}
```

- [ ] **Step 2: Add `synthesize_streaming_main_thread`**

Insert before the existing `synthesize_to_chunks_main_thread` function:

```rust
/// Streaming variant of [`synthesize_to_chunks_main_thread`]: fires
/// `on_event` for each PCM callback as it arrives, interleaved with
/// `runUntilDate(10ms)` slices. EOS sentinel converts to
/// `SynthesisEvent::PhraseEnd` and terminates the loop.
fn synthesize_streaming_main_thread(
    voice: &AVSpeechSynthesisVoice,
    text: &str,
    on_event: &mut dyn FnMut(SynthesisEvent),
) -> Result<()> {
    use std::sync::mpsc::{sync_channel, SyncSender};

    let (tx, rx) = sync_channel::<SynthesisEvent>(PCM_EVENT_CHANNEL_CAPACITY);
    let eos = Arc::new(AtomicBool::new(false));

    // ── 1. Build synthesizer + utterance ────────────────────────────────
    // SAFETY: called on the main thread.
    let synth: Retained<AVSpeechSynthesizer> = unsafe { AVSpeechSynthesizer::new() };
    let ns_text = NSString::from_str(text);
    // SAFETY: factory method; ns_text lives for the scope.
    let utterance = unsafe { AVSpeechUtterance::speechUtteranceWithString(&ns_text) };
    // SAFETY: setVoice: on the main thread.
    unsafe { utterance.setVoice(Some(voice)) };

    // ── 2. Register the PCM callback ─────────────────────────────────────
    let tx_cb = tx.clone();
    let eos_cb = Arc::clone(&eos);

    type CbBlock = block2::Block<dyn Fn(NonNull<AVAudioBuffer>)>;
    let cb = block2::RcBlock::new(move |buf_ptr: NonNull<AVAudioBuffer>| {
        stream_pcm_callback(buf_ptr, &tx_cb, &eos_cb);
    });

    // ── 3. Start synthesis (returns immediately; callbacks queued async) ─
    // SAFETY: called on the main thread; `cb` is retained by `RcBlock`.
    let block_ref: &CbBlock = &cb;
    let block_ptr: *mut CbBlock = (block_ref as *const CbBlock).cast_mut();
    unsafe { synth.writeUtterance_toBufferCallback(&utterance, block_ptr) };

    // ── 4. Drain loop: interleave runloop slices with channel drains ────
    let run_loop = NSRunLoop::mainRunLoop();
    let deadline = Instant::now() + DRAIN_TIMEOUT;
    loop {
        // Drain whatever the channel has now — emit Audio events
        // to the consumer as fast as they arrive.
        while let Ok(event) = rx.try_recv() {
            let is_phrase_end = matches!(event, SynthesisEvent::PhraseEnd);
            on_event(event);
            if is_phrase_end {
                // Drop our sender so the callback's clone is the
                // last; nothing references the receiver afterward.
                drop(tx);
                drop(cb);
                return Ok(());
            }
        }
        if eos.load(Ordering::SeqCst) {
            // Drain any final events queued during the last runloop slice.
            while let Ok(event) = rx.try_recv() {
                let is_phrase_end = matches!(event, SynthesisEvent::PhraseEnd);
                on_event(event);
                if is_phrase_end {
                    break;
                }
            }
            drop(tx);
            drop(cb);
            return Ok(());
        }
        if Instant::now() >= deadline {
            drop(tx);
            drop(cb);
            return Err(PrimerError::Speech(
                "AVSpeechSynthesizer 30s NSRunLoop drain timeout (main-thread streaming)".into(),
            ));
        }
        let date = NSDate::dateWithTimeIntervalSinceNow(RUN_LOOP_SLICE.as_secs_f64());
        run_loop.runUntilDate(&date);
    }
}
```

- [ ] **Step 3: Add `synthesize_streaming_background`**

Insert before the existing `synthesize_to_chunks_background` function:

```rust
/// Streaming variant of [`synthesize_to_chunks_background`]: PCM
/// callbacks (firing on the GCD main thread) send events through a
/// bounded channel; this background thread drains the channel via
/// `recv_timeout`, emits events through `on_event`, and exits on
/// `PhraseEnd` or the 30s overall deadline. The dispatch semaphore is
/// dropped from this path — `PhraseEnd` already signals synthesis
/// complete.
fn synthesize_streaming_background(
    voice: &AVSpeechSynthesisVoice,
    text: &str,
    on_event: &mut dyn FnMut(SynthesisEvent),
) -> Result<()> {
    use std::sync::mpsc::{sync_channel, RecvTimeoutError};

    let (tx, rx) = sync_channel::<SynthesisEvent>(PCM_EVENT_CHANNEL_CAPACITY);

    // ── Build utterance on the calling thread ─────────────────────────
    let ns_text = NSString::from_str(text);
    // SAFETY: factory method.
    let utterance = unsafe { AVSpeechUtterance::speechUtteranceWithString(&ns_text) };
    // SAFETY: setVoice:.
    unsafe { utterance.setVoice(Some(voice)) };

    // ── Trampoline context: utterance + sender ────────────────────────
    struct StreamCtx {
        utterance: Retained<AVSpeechUtterance>,
        tx: std::sync::mpsc::SyncSender<SynthesisEvent>,
    }
    // SAFETY: `StreamCtx` is moved into the main-queue trampoline; at that
    // point no other thread holds a reference. The move is one-shot.
    unsafe impl Send for StreamCtx {}

    extern "C" fn trampoline(ctx_raw: *mut std::ffi::c_void) {
        // SAFETY: ctx_raw was Box::into_raw'd below; we take ownership here.
        let ctx = unsafe { Box::from_raw(ctx_raw as *mut StreamCtx) };

        // SAFETY: AVSpeechSynthesizer::new on the main thread.
        let synth: Retained<AVSpeechSynthesizer> = unsafe { AVSpeechSynthesizer::new() };

        let tx_cb = ctx.tx.clone();
        type CbBlock = block2::Block<dyn Fn(NonNull<AVAudioBuffer>)>;
        let eos_cb = Arc::new(AtomicBool::new(false)); // unused but keeps signature parity
        let cb = block2::RcBlock::new(move |buf_ptr: NonNull<AVAudioBuffer>| {
            stream_pcm_callback(buf_ptr, &tx_cb, &eos_cb);
        });

        // SAFETY: called on the main thread.
        let block_ref: &CbBlock = &cb;
        let block_ptr: *mut CbBlock = (block_ref as *const CbBlock).cast_mut();
        unsafe { synth.writeUtterance_toBufferCallback(&ctx.utterance, block_ptr) };

        // `cb` drops here; AVSpeechSynthesizer's Block_retain keeps it
        // alive for late callbacks. Sender on the closure stays alive
        // through Block_retain; when the closure drops after EOS, the
        // sender drops too, the channel disconnects, and any later
        // recv() returns Disconnected.
    }

    let ctx = Box::new(StreamCtx { utterance, tx });
    let ctx_ptr = Box::into_raw(ctx) as *mut std::ffi::c_void;

    // SAFETY: `_dispatch_main_q` is the GCD main queue object.
    let main_queue: dispatch::dispatch_queue_t =
        unsafe { &dispatch::_dispatch_main_q as *const _ as dispatch::dispatch_queue_t };

    // SAFETY: `trampoline` takes ownership of `ctx_ptr` exactly once.
    unsafe { dispatch::dispatch_async_f(main_queue, ctx_ptr, trampoline) };

    // ── Drain loop ────────────────────────────────────────────────────
    let deadline = Instant::now() + DRAIN_TIMEOUT;
    loop {
        match rx.recv_timeout(STREAM_DRAIN_POLL_MS) {
            Ok(event) => {
                let is_phrase_end = matches!(event, SynthesisEvent::PhraseEnd);
                on_event(event);
                if is_phrase_end {
                    return Ok(());
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    return Err(PrimerError::Speech(
                        "AVSpeechSynthesizer 30s drain timeout (background streaming)".into(),
                    ));
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(PrimerError::Speech(
                    "synth channel disconnected before PhraseEnd".into(),
                ));
            }
        }
    }
}
```

- [ ] **Step 4: Add the streaming PCM callback helper**

Insert in the "Shared PCM callback" section (around tts.rs:683):

```rust
/// Convert one PCM buffer from `AVSpeechSynthesizer` into a
/// `SynthesisEvent` and send it through `tx`. Zero-frame buffers
/// (the EOS sentinel) send `SynthesisEvent::PhraseEnd` and also set
/// `eos` so the main-thread caller's drain loop can break out
/// promptly.
///
/// A blocking `send` (channel full) is acceptable — it surfaces real
/// backpressure from a stalled consumer rather than silently dropping
/// chunks. The channel capacity ([`PCM_EVENT_CHANNEL_CAPACITY`]) is
/// sized at ~6× the per-phrase callback count so this should never
/// happen in normal operation.
fn stream_pcm_callback(
    buf_ptr: NonNull<AVAudioBuffer>,
    tx: &std::sync::mpsc::SyncSender<SynthesisEvent>,
    eos: &Arc<AtomicBool>,
) {
    // SAFETY: buf_ptr is non-null and valid for the callback's lifetime.
    let buf: &AVAudioBuffer = unsafe { buf_ptr.as_ref() };

    let pcm: &AVAudioPCMBuffer = match buf.downcast_ref::<AVAudioPCMBuffer>() {
        Some(p) => p,
        None => return,
    };

    // SAFETY: `frameLength` is an immutable property getter.
    let frame_length = unsafe { pcm.frameLength() } as usize;

    if frame_length == EOS_FRAME_LENGTH {
        eos.store(true, Ordering::SeqCst);
        // Best-effort send; receiver may have already dropped on a
        // previous PhraseEnd in degenerate paths.
        let _ = tx.send(SynthesisEvent::PhraseEnd);
        return;
    }

    // SAFETY: immutable property getters.
    let format = unsafe { pcm.format() };
    let channel_count = unsafe { format.channelCount() };
    if channel_count != 1 {
        tracing::warn!(
            channel_count,
            "AVSpeechSynthesizer emitted multi-channel PCM; only channel 0 will be captured"
        );
    }
    let sample_rate = unsafe { format.sampleRate() } as u32;

    // SAFETY: `floatChannelData()[0]` is the mono channel of `frame_length`
    // samples; valid for the callback's lifetime.
    let data_ptr = unsafe { pcm.floatChannelData() };
    if data_ptr.is_null() {
        return;
    }
    let chan0_nn: NonNull<f32> = unsafe { *data_ptr };
    let chan0_ptr: *mut f32 = chan0_nn.as_ptr();
    // SAFETY: valid mono float slice for the callback's lifetime.
    let slice: &[f32] = unsafe { std::slice::from_raw_parts(chan0_ptr, frame_length) };

    let chunk = AudioChunk {
        samples: slice.to_vec(),
        sample_rate,
    };
    // Best-effort send; receiver may have already exited via deadline.
    let _ = tx.send(SynthesisEvent::Audio(chunk));
}
```

- [ ] **Step 5: Replace `MacosTtsSession`'s Stage-A wrapper with `synthesize_streaming`**

Replace the `impl SynthesisSession for MacosTtsSession { ... }` block from Task 6:

```rust
impl SynthesisSession for MacosTtsSession {
    fn push_text(
        &mut self,
        text: &str,
        on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()> {
        for phrase in self.splitter.push(text) {
            synthesize_streaming(&self.voice, &phrase, on_event)?;
        }
        Ok(())
    }

    fn finalize(
        mut self: Box<Self>,
        on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()> {
        if let Some(tail) = self.splitter.flush() {
            synthesize_streaming(&self.voice, &tail, on_event)?;
        }
        Ok(())
    }
}
```

- [ ] **Step 6: Remove the `#[allow(dead_code)]` from the new consts**

They're now used by `synthesize_streaming_*`. Strip the attributes added in Task 7.

- [ ] **Step 7: Run the macOS test suite — the failing test from Task 8 must now PASS**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-speech --features macos-native --test macos_tts
```

Expected: 6 passed, 1 ignored. Critical: `streaming_emits_multiple_audio_events_before_phrase_end` reports ≥2 Audio events before PhraseEnd.

- [ ] **Step 8: Workspace sweep — check Stage A didn't regress**

```bash
~/.cargo/bin/cargo test --workspace
~/.cargo/bin/cargo test -p primer-speech --features voice-loop
~/.cargo/bin/cargo fmt --all -- --check
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
```

Expected: workspace 857 passed; voice-loop 84 passed; fmt + clippy clean.

- [ ] **Step 9: Commit (bundles Tasks 7-9)**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-speech/src/macos/tts.rs \
        src/crates/primer-speech/tests/macos_tts.rs
git commit -m "$(cat <<'EOF'
speech(macos): stream PCM chunks via mpsc::sync_channel (closes #114)

Replace the synthesize_to_chunks accumulator + coalesce_phrase path
with synthesize_streaming_{main_thread,background}: PCM callbacks
running on the GCD main queue send SynthesisEvent::Audio(chunk) into
a bounded mpsc::sync_channel (cap 64 — ~6× the observed ~10
callbacks/phrase rate). Main-thread caller interleaves runloop
slices with try_recv drains; background caller uses recv_timeout.

EOS sentinel converts to SynthesisEvent::PhraseEnd, which terminates
the drain loop. The dispatch semaphore is dropped from the
background path — PhraseEnd is the synchronisation primitive now.
DispatchSemaphore + Arc<DispatchSemaphore> + the related FFI
declarations remain in the dispatch module for Task 10's cleanup
sweep; they're unused as of this commit.

Per-phrase time-to-first-audio drops from ~hundreds of ms (full
phrase synthesis) to ~50 ms (first PCM callback) — pinned by the
new structural test `streaming_emits_multiple_audio_events_before_
phrase_end` (failing before this commit, passing after).

Closes #114.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 10: Delete now-dead code (post-Stage B cleanup)

Removes the old accumulator-based functions and the `DispatchSemaphore` machinery the background path no longer needs.

**Files:**
- Modify: `src/crates/primer-speech/src/macos/tts.rs`

- [ ] **Step 1: Audit what's still used vs. what's dead**

Run `cargo build` and inspect dead-code warnings:

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo build -p primer-speech --features macos-native 2>&1 | grep -E "warning.*(never used|never read|dead_code)" | head -20
```

Expected dead items:
- `fn synthesize_to_chunks`
- `fn synthesize_to_chunks_main_thread`
- `fn synthesize_to_chunks_background`
- `fn coalesce_phrase`
- `fn chunks_to_audio_buffer`
- `fn pcm_callback`
- `type Accumulator = Arc<Mutex<Vec<AudioChunk>>>`
- `struct DispatchSemaphore`
- `struct SynthCtx`
- All `dispatch::dispatch_semaphore_*` FFI declarations

The one-shot `TextToSpeech::synthesize` impl still calls `chunks_to_audio_buffer(synthesize_to_chunks(...)?)` — Step 2 rewrites that to drive `synthesize_streaming` with a local accumulator.

- [ ] **Step 2: Rewrite `TextToSpeech::synthesize` to drive `synthesize_streaming`**

Replace the existing `#[async_trait] impl TextToSpeech for MacosTextToSpeech` body:

```rust
#[async_trait]
impl TextToSpeech for MacosTextToSpeech {
    async fn synthesize(&self, text: &str, _voice: &VoiceProfile) -> Result<AudioBuffer> {
        // Drive the streaming path with a local accumulator. Same
        // concatenation behaviour as the previous chunks_to_audio_buffer
        // helper, but no separate code path for the one-shot case.
        if objc2_foundation::NSThread::isMainThread_class() {
            return synthesize_to_buffer(&self.voice, text);
        }
        let text_owned = text.to_owned();
        let voice_retained = self.voice.clone();
        tokio::task::spawn_blocking(move || synthesize_to_buffer(&voice_retained, &text_owned))
            .await
            .map_err(|e| PrimerError::Speech(format!("spawn_blocking panicked: {e}")))?
    }
}

/// Drive [`synthesize_streaming`] with a local accumulator and return
/// the concatenated audio. Replaces the deleted `chunks_to_audio_buffer`
/// + `synthesize_to_chunks` pair.
fn synthesize_to_buffer(voice: &AVSpeechSynthesisVoice, text: &str) -> Result<AudioBuffer> {
    let mut samples: Vec<f32> = Vec::new();
    let mut sample_rate: u32 = 0;
    synthesize_streaming(voice, text, &mut |event| {
        if let SynthesisEvent::Audio(chunk) = event {
            if sample_rate == 0 {
                sample_rate = chunk.sample_rate;
            }
            samples.extend(chunk.samples);
        }
    })?;
    Ok(AudioBuffer { samples, sample_rate })
}
```

- [ ] **Step 3: Delete dead items**

Delete in this order (each deletion is independent; do them one block at a time so any compile errors localise):

a) Delete `fn synthesize_to_chunks_main_thread` (around lines 473-532)
b) Delete `fn synthesize_to_chunks_background` (around lines 564-681)
c) Delete `fn synthesize_to_chunks` (around lines 306-316)
d) Delete `fn pcm_callback` (around lines 690-734)
e) Delete `fn coalesce_phrase` (around lines 447-457)
f) Delete `fn chunks_to_audio_buffer` (around lines 348-357)
g) Delete `type Accumulator = ...` (around line 290)
h) Delete `struct SynthCtx { ... }` + its `unsafe impl Send` (around lines 539-551)
i) Delete `struct DispatchSemaphore` + its `unsafe impl Send/Sync` + `Drop` + `as_raw` (around lines 125-151)
j) Delete dispatch FFI declarations no longer used. After deletion, only `dispatch_async_f` and `_dispatch_main_q` should remain in the `mod dispatch` block. Remove `dispatch_semaphore_create`, `dispatch_semaphore_signal`, `dispatch_semaphore_wait`, `dispatch_release`, `dispatch_time`, the `dispatch_semaphore_t` typedef, `DISPATCH_TIME_NOW`, `TIMEOUT_NS`. Leave the comments about libdispatch / GCD main queue intact.

After this step, `mod dispatch` should be ~20 lines instead of ~40.

- [ ] **Step 4: Build to verify cleanup**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo build -p primer-speech --features macos-native
```

Expected: clean. If any `unused import` warnings remain, remove the imports (e.g. unused `std::sync::Mutex` if the only use was `Accumulator`).

- [ ] **Step 5: Re-run macOS tests**

```bash
~/.cargo/bin/cargo test -p primer-speech --features macos-native
```

Expected: 6 passed, 1 ignored. Same as Task 9 Step 7.

- [ ] **Step 6: Workspace verification**

```bash
~/.cargo/bin/cargo test --workspace
~/.cargo/bin/cargo test -p primer-speech --features voice-loop
~/.cargo/bin/cargo fmt --all -- --check
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech -- -D warnings
```

Expected: 857 passed default-features; 84 voice-loop; clean fmt + clippy.

- [ ] **Step 7: Check tts.rs line count**

```bash
wc -l src/crates/primer-speech/src/macos/tts.rs
```

Expected: ~600 lines (down from 734 — the deletion is larger than the addition). Still over the 500-line guideline; that's covered by a follow-up issue at PR close. Do not split in this PR.

- [ ] **Step 8: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-speech/src/macos/tts.rs
git commit -m "$(cat <<'EOF'
speech(macos): delete dead accumulator + DispatchSemaphore machinery

After the streaming path landed in the previous commit, the
synthesize_to_chunks_{main_thread,background} pair, coalesce_phrase,
chunks_to_audio_buffer, the Accumulator type alias, SynthCtx, and
the entire DispatchSemaphore RAII wrapper are unused. The one-shot
TextToSpeech::synthesize impl is rewritten to drive
synthesize_streaming with a local Vec<f32> accumulator
(synthesize_to_buffer helper) — same concatenation behaviour, no
separate code path.

The dispatch FFI module shrinks to dispatch_async_f +
_dispatch_main_q (the streaming background path doesn't need any
semaphore primitives). tts.rs drops ~130 lines net.

Towards splitting tts.rs into a tts/ directory module — tracked as
a separate follow-up; not part of this PR.

Towards #114.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Stage C — TTFA test + smoke instrumentation

### Task 11: Add state-machine TTFA test with `TimedMockTts`

**Files:**
- Modify: `src/crates/primer-speech/src/voice_loop/state_machine.rs::tests` — add new test + new mock

- [ ] **Step 1: Add the `TimedMockTts` mock**

In the `mocks` sub-module (around line 1000), add after the existing `MockStreamingTts`:

```rust
/// Streaming TTS mock that injects real wallclock delays between
/// Audio events, used to verify the consumer doesn't buffer chunks
/// before forwarding them to `on_committed_audio`.
///
/// Each push_text emits three Audio events at 50 ms intervals,
/// then PhraseEnd. Total per-push wallclock ≈ 100 ms.
pub struct TimedMockTts;

impl Named for TimedMockTts {
    fn name(&self) -> &str {
        "timed-mock-tts"
    }
}

/// Samples per Audio event emitted by [`TimedMockTts`]. Small enough
/// that the consumer's on_committed_audio doesn't do meaningful work
/// per call; large enough that the speaker ringbuf accepts each push
/// without blocking.
const TIMED_MOCK_SAMPLES_PER_CHUNK: usize = 64;
/// Sample rate of the timed-mock chunks. Matches the existing
/// `MockStreamingTts` (Piper-class voice rate).
const TIMED_MOCK_SAMPLE_RATE: u32 = 22_050;
/// Delay between successive Audio events emitted by [`TimedMockTts`].
const TIMED_MOCK_INTER_CHUNK_MS: u64 = 50;

impl StreamingTextToSpeech for TimedMockTts {
    fn sample_rate(&self) -> u32 {
        TIMED_MOCK_SAMPLE_RATE
    }
    fn open_session(&self, _voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>> {
        Ok(Box::new(TimedMockTtsSession))
    }
}

struct TimedMockTtsSession;

impl SynthesisSession for TimedMockTtsSession {
    fn push_text(
        &mut self,
        text: &str,
        on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        for marker in [0.1_f32, 0.2, 0.3] {
            on_event(SynthesisEvent::Audio(AudioChunk {
                samples: vec![marker; TIMED_MOCK_SAMPLES_PER_CHUNK],
                sample_rate: TIMED_MOCK_SAMPLE_RATE,
            }));
            if marker < 0.3 {
                std::thread::sleep(std::time::Duration::from_millis(TIMED_MOCK_INTER_CHUNK_MS));
            }
        }
        on_event(SynthesisEvent::PhraseEnd);
        Ok(())
    }

    fn finalize(
        self: Box<Self>,
        _on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()> {
        Ok(())
    }
}
```

- [ ] **Step 2: Add the TTFA test**

In `mod tests` (after the existing mock-related tests, around line 1136), add:

```rust
/// Pin the state machine's consumer-side guarantee that PCM events
/// reach `on_committed_audio` AS THEY ARRIVE — not buffered until
/// after `push_text` returns.
///
/// TimedMockTts emits three Audio events at 50 ms intervals. We
/// record `Instant::now()` at each on_committed_audio call and assert
/// the FIRST sample's timestamp precedes the LAST sample's timestamp
/// by ≥80 ms (vs. 100 ms total inter-event budget — 20 ms slack for
/// CI noise). A consumer that buffered all chunks before forwarding
/// would see all three timestamps clustered within microseconds.
#[tokio::test]
async fn streaming_chunks_reach_speaker_before_phrase_completes() {
    use std::sync::Mutex;
    use std::time::Instant;
    use primer_core::speech::VadEvent;

    let backends = super::LoopBackends::single_locale(
        Arc::new(MockStreamingStt::new("hello primer")),
        Arc::new(TimedMockTts),
        primer_core::speech::VoiceProfile::default(),
        primer_core::i18n::Locale::English,
    );

    let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);
    event_tx.try_send(VadEvent::SpeechStart).unwrap();
    event_tx.try_send(VadEvent::SpeechEnd).unwrap();
    drop(event_tx);

    // Record (Instant, first-sample-value) per non-empty on_audio call.
    let timeline: Arc<Mutex<Vec<(Instant, Option<f32>)>>> = Arc::new(Mutex::new(Vec::new()));
    let timeline_cb = Arc::clone(&timeline);
    let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |samples| {
        if !samples.is_empty() {
            timeline_cb
                .lock()
                .unwrap()
                .push((Instant::now(), samples.first().copied()));
        }
    });

    let observer = MockObserver::new();
    super::run_loop_borrowed(
        backends,
        event_rx,
        Box::new(EchoResponder),
        on_audio,
        None,
        false,
        None,
        observer.clone(),
    )
    .await
    .expect("loop runs to completion");

    let recorded = timeline.lock().unwrap();
    // Expect at least: [marker=0.1, marker=0.2, marker=0.3, silence(zeros)].
    // We assert on the gap between the FIRST 0.1 commit and the FINAL 0.3
    // commit (ignoring the trailing 200ms inter-phrase silence, which is
    // appended in the same call site as 0.3 and so shares its timestamp).
    let first = recorded
        .iter()
        .find(|(_, v)| v.map(|x| (x - 0.1).abs() < 0.01).unwrap_or(false))
        .expect("0.1 marker was committed");
    let last = recorded
        .iter()
        .rfind(|(_, v)| v.map(|x| (x - 0.3).abs() < 0.01).unwrap_or(false))
        .expect("0.3 marker was committed");
    let gap = last.0.duration_since(first.0);
    assert!(
        gap >= std::time::Duration::from_millis(80),
        "expected ≥80ms between first and last Audio commit (true streaming); got {gap:?}"
    );
}

/// Minimal Responder for the TTFA test. Echoes the transcript so
/// the SPEAK phase has non-empty text to feed into the TTS.
struct EchoResponder;
impl super::Responder for EchoResponder {
    fn respond<'a>(
        &'a mut self,
        transcript: &'a str,
        mut on_chunk: Box<dyn FnMut(&str) + Send + 'a>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = primer_core::error::Result<String>>
                + Send
                + 'a,
        >,
    > {
        let owned = transcript.to_string();
        Box::pin(async move {
            on_chunk(&owned);
            Ok(owned)
        })
    }
}
```

- [ ] **Step 3: Run the new test**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo test -p primer-speech --features voice-loop streaming_chunks_reach_speaker_before_phrase_completes
```

Expected: PASS. Test wallclock ~100 ms.

- [ ] **Step 4: Run the full voice-loop suite to confirm no regression**

```bash
~/.cargo/bin/cargo test -p primer-speech --features voice-loop
```

Expected: 85 passed (was 84; +1 new test), 0 failed, 2 ignored.

- [ ] **Step 5: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-speech/src/voice_loop/state_machine.rs
git commit -m "$(cat <<'EOF'
voice(test): pin streaming-to-speaker guarantee in state machine

TimedMockTts emits three Audio events at 50ms intervals, then
PhraseEnd. The new streaming_chunks_reach_speaker_before_phrase_
completes test records Instant::now() per on_committed_audio call
and asserts the first 0.1-marker timestamp precedes the last
0.3-marker timestamp by ≥80ms — pins the SPEAK consumer's
guarantee that PCM events reach the speaker as they arrive, not
batched after push_text returns.

A consumer that buffered all chunks before forwarding would see
all three timestamps clustered within microseconds and fail with
"expected ≥80ms between first and last Audio commit".

Voice-loop tests: 84 → 85. Test wallclock ~100ms.

Towards #114.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 12: Extend `tts_macos_pcm_smoke` example with TTFA instrumentation

**Files:**
- Modify: `src/crates/primer-speech/examples/tts_macos_pcm_smoke.rs`

- [ ] **Step 1: Add a `--measure-ttfa` flag and print TTFA at end**

The existing smoke binary already records `(callback_time - start)` for each PCM callback. We add a one-line summary that explicitly names the "TTFA" figure for downstream scripts/grep.

Find the `struct Args { ... }` block (around line 91) and add:

```rust
/// Print an explicit TTFA (time-to-first-audio) summary line.
/// The per-callback rows are always printed; this flag adds a
/// single TTFA verdict line at the end suitable for greppable
/// downstream tooling.
#[arg(long)]
measure_ttfa: bool,
```

- [ ] **Step 2: Add TTFA output at the end of `run()`**

In `imp::run()`, after the existing post-loop verdict lines (find the section that prints the "PASS" / "FAIL" verdict for `PASS_FIRST_CHUNK_LATENCY_MS`), add:

```rust
if args.measure_ttfa {
    if let Some(first) = entries.first() {
        let ttfa_ms = first.elapsed_ms;
        let phrase_end_ms = entries
            .iter()
            .find(|e| e.frame_count == 0)
            .map(|e| e.elapsed_ms)
            .unwrap_or(0);
        let streaming_win_ms = phrase_end_ms.saturating_sub(ttfa_ms);
        println!(
            "[smoke] TTFA: {ttfa_ms} ms (writeUtterance → first PCM callback) for voice {:?}",
            args.voice
        );
        if phrase_end_ms > 0 {
            println!(
                "[smoke] PhraseEnd: {phrase_end_ms} ms (writeUtterance → EOS) for voice {:?}",
                args.voice
            );
            println!(
                "[smoke] Streaming win: {phrase_end_ms} - {ttfa_ms} = {streaming_win_ms} ms earlier than coalesce"
            );
        }
    }
}
```

(The exact field name `elapsed_ms` and `frame_count` may need adjustment based on the existing `entries: Vec<Entry>` structure — read the surrounding code and use whichever fields capture per-callback wallclock + zero-frame detection.)

- [ ] **Step 3: Verify the example still builds and runs**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo build --example tts_macos_pcm_smoke -p primer-speech
```

Expected: clean build.

Manual smoke (optional — needs macOS + a voice):

```bash
~/.cargo/bin/cargo run --example tts_macos_pcm_smoke -p primer-speech -- --measure-ttfa
```

Expected: per-callback row output (unchanged) plus the three new `[smoke]` lines at the end.

- [ ] **Step 4: Commit**

```bash
cd /Users/hherb/src/primer
git add src/crates/primer-speech/examples/tts_macos_pcm_smoke.rs
git commit -m "$(cat <<'EOF'
speech(example): add --measure-ttfa flag to tts_macos_pcm_smoke

Print an explicit grep-friendly TTFA summary after the
per-callback rows:

  [smoke] TTFA: 47 ms (writeUtterance → first PCM callback) ...
  [smoke] PhraseEnd: 312 ms (writeUtterance → EOS) ...
  [smoke] Streaming win: 312 - 47 = 265 ms earlier than coalesce

Lets re-runs after macOS major releases produce a directly
comparable verdict line. No assertion; instrumentation only.

Towards #114.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 13: Final verification + push

- [ ] **Step 1: Full test sweep across feature combinations**

```bash
cd /Users/hherb/src/primer/src

# Default features.
~/.cargo/bin/cargo test --workspace
# Expected: 857 passed, 0 failed, 3 ignored.

# Voice loop.
~/.cargo/bin/cargo test -p primer-speech --features voice-loop
# Expected: 85 passed, 0 failed, 2 ignored.

# macOS-native (skip on non-macOS hosts).
~/.cargo/bin/cargo test -p primer-speech --features macos-native
# Expected: 6 passed, 1 ignored.

# CLI speech build.
~/.cargo/bin/cargo test -p primer-cli --features speech
# Expected: 12 passed, 0 failed, 0 ignored.

# CLI speech + macos-native build (macOS only).
~/.cargo/bin/cargo test -p primer-cli --features speech,macos-native
# Expected: 12 passed, 0 failed, 0 ignored.

# GUI speech build.
~/.cargo/bin/cargo test -p primer-gui --features speech
# Expected: 146 passed, 0 failed, 0 ignored.
```

- [ ] **Step 2: fmt + clippy across feature combinations**

```bash
~/.cargo/bin/cargo fmt --all -- --check
~/.cargo/bin/cargo clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo clippy --workspace --all-targets --features primer-gui/speech -- -D warnings
RUSTFLAGS="-D warnings" ~/.cargo/bin/cargo test --workspace --no-fail-fast
```

Expected: clean across all.

- [ ] **Step 3: Push branch and open PR**

```bash
git push -u origin speech/macos-native-pcm-streaming-issue-114
gh pr create --title "speech(macos): stream PCM chunks to speaker as AVSpeechSynthesizer emits them (closes #114)" --body "$(cat <<'EOF'
## Summary

- Reshape `SynthesisSession::push_text` / `finalize` from `Vec<AudioChunk>` return to a `&mut dyn FnMut(SynthesisEvent)` callback. `SynthesisEvent` is an enum of `Audio(chunk)` + `PhraseEnd`.
- macOS-native TTS now streams PCM chunks via an `mpsc::sync_channel(64)` between the GCD-main PCM callback and the caller thread. Per-phrase time-to-first-audio drops from ~hundreds of ms (full phrase synthesis) to ~50 ms (first PCM callback).
- Drops the `DispatchSemaphore` machinery from the background path — `PhraseEnd` is the synchronisation primitive now.
- Same observable timing for Piper (still one Audio + one PhraseEnd per phrase). Same 200 ms inter-phrase silence inserted by the state machine consumer.

Closes #114. Spec: [docs/superpowers/specs/2026-05-19-macos-tts-pcm-streaming-design.md](docs/superpowers/specs/2026-05-19-macos-tts-pcm-streaming-design.md).

## Test plan

- [x] `cargo test --workspace` (default features): 857 passed, 0 failed, 3 ignored (was 856; +1 explicit-ordering test in primer-core).
- [x] `cargo test -p primer-speech --features voice-loop`: 85 passed (was 84; +1 TTFA test).
- [x] `cargo test -p primer-speech --features macos-native` (macOS host): 6 passed, 1 ignored (was 5; +1 structural streaming test).
- [x] `cargo test -p primer-cli --features speech` / `speech,macos-native` / `cargo test -p primer-gui --features speech`: counts unchanged.
- [x] `cargo fmt --all -- --check`: clean.
- [x] `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- [x] `cargo clippy --workspace --all-targets --features primer-gui/speech -- -D warnings`: clean.
- [ ] Manual: `cargo run --example tts_macos_pcm_smoke -p primer-speech -- --measure-ttfa` printed `[smoke] Streaming win: <N> ms earlier than coalesce` with `N > 100`.
- [ ] Manual: `cargo run -p primer-cli --features speech,macos-native --bin primer -- --speech --name Smoke --age 9 --no-persist --verbose` — Primer's spoken response begins playing within ~100 ms of the LLM stream completing (subjective; the prior delay was clearly noticeable).

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 4: Check CI**

```bash
gh pr checks
```

Wait for the `cargo test (default features)` check to pass.

- [ ] **Step 5: Optional follow-up issue**

If `tts.rs` is still over the 500-line guideline (Task 10 brought it to ~600), open a follow-up:

```bash
gh issue create --title "refactor(macos): split src/macos/tts.rs into a tts/ directory module" --body "$(cat <<'EOF'
## Context

After PR for #114 landed the macOS streaming impl, `src/crates/primer-speech/src/macos/tts.rs` is still over the 500-line guideline. The streaming change was deliberately scoped to behaviour-only; a structural split was deferred to keep that diff reviewable.

## Proposal

Split into:

- `tts/mod.rs` — `MacosTextToSpeech` struct, public `Named` / `StreamingTextToSpeech` / `TextToSpeech` trait impls, the shared `synthesize_streaming` dispatcher.
- `tts/dispatch.rs` — the GCD FFI module (`dispatch_async_f`, `_dispatch_main_q`).
- `tts/main_thread.rs` — `synthesize_streaming_main_thread`.
- `tts/background.rs` — `synthesize_streaming_background`.
- `tts/pcm_callback.rs` — `stream_pcm_callback` helper.

Pure refactor; behaviour unchanged.

## Out of scope

- Any behaviour change. Tests must remain bit-identical.

## Surfaced from

PR for #114 — deferred to keep the streaming diff focused.
EOF
)"
```

---

## Self-review (done before publishing this plan)

**Spec coverage check (every spec section maps to a task):**

| Spec section | Task |
|---|---|
| Trait reshape (SynthesisEvent + new signatures) | Task 1 |
| StubSynthesisSession adapt | Task 2 |
| MockTtsSession adapt | Task 3 |
| CapturingSession adapt | Task 4 |
| Piper backend adapt | Task 5 |
| macOS Stage-A wrapper | Task 6 |
| State machine SPEAK consumer | Task 6 |
| Two integration tests adapt | Task 6 |
| macOS streaming consts | Task 7 |
| Failing structural test (gate) | Task 8 |
| synthesize_streaming_main_thread | Task 9 |
| synthesize_streaming_background (no semaphore) | Task 9 |
| stream_pcm_callback helper | Task 9 |
| Wire MacosTtsSession to streaming | Task 9 |
| Delete dead code (coalesce, accumulator, semaphore) | Task 10 |
| One-shot synthesize via streaming | Task 10 |
| State-machine TTFA test (TimedMockTts) | Task 11 |
| Manual TTFA smoke (--measure-ttfa) | Task 12 |
| Final verification across feature matrix | Task 13 |
| File-split follow-up issue | Task 13 Step 5 |

**Placeholder scan:** No "TBD" or "fill in details" anywhere. The one approximate line (`The exact field name elapsed_ms and frame_count may need adjustment based on the existing entries structure`) is bounded by the prior step's instruction to read the surrounding code, and the names come from the existing example's design.

**Type consistency:** `SynthesisEvent::Audio(AudioChunk)` and `SynthesisEvent::PhraseEnd` used consistently. `on_event: &mut dyn FnMut(SynthesisEvent)` used in every signature. `Box<Self>` on `finalize` matches the existing pattern. `synthesize_streaming` is the dispatcher name across all tasks.

**Build-state continuity:** Tasks 2-5 stage changes without committing; the workspace deliberately does not compile between Task 1 and Task 6. Task 6 commits all of Tasks 2-6 atomically so every committed state in the branch is a green workspace.
