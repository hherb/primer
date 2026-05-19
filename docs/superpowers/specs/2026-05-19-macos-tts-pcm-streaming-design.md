# macOS-native TTS: stream PCM chunks to speaker as `AVSpeechSynthesizer` emits them

**Date:** 2026-05-19
**Closes:** #114
**Status:** Design — pending implementation plan

## Problem

`MacosTtsSession::push_text` ([src/crates/primer-speech/src/macos/tts.rs:419](../../../src/crates/primer-speech/src/macos/tts.rs#L419)) synthesises one phrase to completion before returning any audio:

```rust
fn push_text(&mut self, text: &str) -> Result<Vec<AudioChunk>> {
    let phrases = self.splitter.push(text);
    let mut chunks = Vec::with_capacity(phrases.len());
    for phrase in phrases {
        if let Some(c) = coalesce_phrase(synthesize_to_chunks(&self.voice, &phrase)?) {
            chunks.push(c);
        }
    }
    Ok(chunks)
}
```

`AVSpeechSynthesizer.writeUtterance:toBufferCallback:` fires its PCM callback ~10 times per phrase. Today we accumulate all callbacks via `coalesce_phrase`, return the coalesced chunk, and only THEN start pushing samples to cpal. **Time-to-first-audible-byte (TTFA) per phrase ≈ time-to-full-phrase-synthesis.** Observably worse than Piper for every phrase except the trailing audio of the last phrase.

Piper has the same one-chunk-per-phrase shape but synthesises in <50 ms/phrase, so the coalesce cost is invisible. AVF takes hundreds of ms per phrase; the wait is audibly perceptible at the start of each Primer turn.

## Goal

Stream PCM callbacks directly to the speaker as they arrive rather than coalescing then returning. Reduce per-phrase TTFA on macOS-native from "full phrase synthesis" to "first PCM callback" (~50 ms instead of hundreds of ms).

Piper remains one-chunk-per-phrase — no behaviour change for that backend.

## Non-goals

- Per-inference-step streaming for Piper. Piper's phrase synth is <50 ms; streaming gives marginal benefit and would add test churn.
- Changing the inter-phrase silence model (still 200 ms inserted by the consumer).
- Touching the `is_speaking` echo-suppression flag semantics.
- Splitting `tts.rs` into submodules. The file is already over the 500-line guideline; the streaming change is net roughly line-neutral. A follow-up `tts/` directory module split is the right shape but conflates "change behaviour" with "reorganise" if bundled here.

## Architecture

### Trait reshape (primer-core)

The `SynthesisSession` trait inverts from "synthesize then return a Vec" to "fire a callback per event."

```rust
// primer-core/src/speech.rs

/// One event emitted by a streaming synthesis session.
#[derive(Debug, Clone)]
pub enum SynthesisEvent {
    /// PCM data became available. Consumer forwards to the speaker immediately.
    Audio(AudioChunk),
    /// End of one phrase. Consumer typically inserts a brief pause
    /// (~200 ms of silence) before the next phrase's audio arrives.
    PhraseEnd,
}

pub trait SynthesisSession: Send {
    /// Push text. Synthesiser invokes `on_event` for each PCM chunk
    /// and phrase boundary as soon as it's available — without
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

    /// Drain remaining buffered text and finalize. Consumes the session.
    /// Fires the same events as `push_text` for any trailing partial phrase.
    fn finalize(
        self: Box<Self>,
        on_event: &mut dyn FnMut(SynthesisEvent),
    ) -> Result<()>;
}
```

**Why a single new shape, not additive:**
- Only 5 in-tree implementations need updating (Piper, MacosTtsSession, StubSynthesisSession, MockTtsSession ×2, CapturingSession in state_machine tests). Clean break is cheap.
- `Vec<AudioChunk>` allocations disappear from the hot path.
- The trait stops claiming "synthesize then return" and becomes what it actually is: a push-pull event stream.

**Why `PhraseEnd` as an enum variant, not a flag on chunks:**
- Producer knows about phrase boundaries (its PhraseSplitter has the state); consumer doesn't.
- A `bool is_phrase_end` on chunks would force the producer to detect "last PCM chunk of this phrase" inside the callback, which it can't always know until the EOS sentinel arrives.
- The enum carries the boundary signal cheaply with no per-chunk overhead.

**Object safety preserved:** `&mut dyn FnMut(SynthesisEvent)` is sized, so the trait remains object-safe (`Box<dyn SynthesisSession>` continues to work).

### macOS streaming implementation

The current paths build an `Arc<Mutex<Vec<AudioChunk>>>` accumulator, run synthesis, wait for EOS, then return the Vec. The new paths replace the accumulator with a **`mpsc::sync_channel<SynthesisEvent>`** that crosses the GCD-main → caller-thread boundary.

Two paths still exist (matching the current main-thread vs. background-thread dispatch):

#### Main-thread path (CLI on macOS with `current_thread` runtime; test binary)

`synthesize_streaming_main_thread(voice, text, on_event)`:

1. Create `mpsc::sync_channel(PCM_EVENT_CHANNEL_CAPACITY)` and an `eos: AtomicBool`.
2. Build the AVSpeechSynthesizer + utterance + PCM-callback block (unchanged from today).
3. PCM callback (running on GCD main, which is the current thread here):
   - If `frameLength == 0` (EOS sentinel): set `eos`, send `SynthesisEvent::PhraseEnd` into the channel.
   - Else: convert PCM buffer to `AudioChunk` (same logic as today's `pcm_callback`), send `SynthesisEvent::Audio(chunk)` into the channel.
4. Call `synth.writeUtterance:toBufferCallback:` — returns immediately, callbacks queue async on GCD main.
5. Drain loop:
   ```rust
   loop {
       // Drain whatever the channel has now.
       while let Ok(event) = rx.try_recv() {
           on_event(event);
       }
       if eos.load(SeqCst) { break; }
       if Instant::now() >= deadline { return Err(timeout); }
       let date = NSDate::dateWithTimeIntervalSinceNow(RUN_LOOP_SLICE.as_secs_f64());
       run_loop.runUntilDate(&date);
   }
   // Final drain — anything queued during the last runloop slice.
   while let Ok(event) = rx.try_recv() {
       on_event(event);
   }
   ```
6. The callback closure goes out of scope; AVSpeechSynthesizer's internal Block_retain keeps it alive for any late callbacks. Same lifetime story as today.

`on_event` runs on the OS main thread (same thread as `runUntilDate`). Fine — `on_committed_audio` is `Send` but doesn't require being called off-main.

#### Background-thread path (CLI on Linux/Tauri GUI worker)

`synthesize_streaming_background(voice, text, on_event)`:

1. Create `mpsc::sync_channel(PCM_EVENT_CHANNEL_CAPACITY)`.
2. Build utterance on the calling thread (same as today).
3. Build the trampoline + PCM callback that sends into the channel (PCM-buf → `Audio(chunk)`; EOS → `PhraseEnd`).
4. `dispatch_async_f` the trampoline onto the GCD main queue (same as today).
5. Background thread (calling thread) does:
   ```rust
   let deadline = Instant::now() + DRAIN_TIMEOUT;
   loop {
       match rx.recv_timeout(STREAM_DRAIN_POLL_MS) {
           Ok(event @ SynthesisEvent::Audio(_)) => on_event(event),
           Ok(event @ SynthesisEvent::PhraseEnd) => {
               on_event(event);
               return Ok(());
           }
           Err(RecvTimeoutError::Timeout) => {
               if Instant::now() >= deadline {
                   return Err(timeout_err);
               }
           }
           Err(RecvTimeoutError::Disconnected) => {
               return Err(channel_closed_err);
           }
       }
   }
   ```
6. **The dispatch semaphore is dropped from this path** — `PhraseEnd` already signals "synthesis complete." This is a real simplification: `DispatchSemaphore`, `SynthCtx.sema`, and the semaphore-related FFI calls all disappear from the background path. The `Arc<DispatchSemaphore>` UAF-prevention machinery becomes unnecessary because the channel itself is the synchronisation primitive.

### Sample-rate handling

Unchanged. `MacosTextToSpeech::sample_rate()` still returns `native_sample_rate` queried at construction time. Each `AudioChunk` still carries its own `sample_rate` field (from `AVAudioFormat.sampleRate()` in the PCM callback). The output resampler in `build_local_backends_macos_native` still reads `tts.sample_rate()` at builder time.

### Consumer change (state_machine.rs)

Today (state_machine.rs:836-849):
```rust
const INTER_PHRASE_SILENCE_MS: u32 = 200;
let inter_phrase_silence_samples =
    (tts_rate * INTER_PHRASE_SILENCE_MS / 1000) as usize;
for chunk in session.push_text(&tts_text)? {
    on_committed_audio(chunk.samples);
    on_committed_audio(vec![0.0_f32; inter_phrase_silence_samples]);
}
for chunk in session.finalize()? {
    on_committed_audio(chunk.samples);
    on_committed_audio(vec![0.0_f32; inter_phrase_silence_samples]);
}
on_committed_audio(Vec::new());
```

New:
```rust
let inter_phrase_silence_samples =
    (tts_rate * INTER_PHRASE_SILENCE_MS / 1000) as usize;
let mut on_event = |event: SynthesisEvent| match event {
    SynthesisEvent::Audio(chunk) => on_committed_audio(chunk.samples),
    SynthesisEvent::PhraseEnd => {
        on_committed_audio(vec![0.0_f32; inter_phrase_silence_samples])
    }
};
session.push_text(&tts_text, &mut on_event)?;
session.finalize(&mut on_event)?;
on_committed_audio(Vec::new()); // flush sentinel — unchanged
```

Same observable behaviour for Piper. For macOS, the closure fires inside `synthesize_streaming` as each PCM callback arrives.

## Data flow

```
LLM stream → DialogueResponder → accumulated text
                                        ↓
                            strip_markdown_for_tts
                                        ↓
                  session.push_text(text, &mut on_event)
                                        ↓
              ┌─────────────────────────┴─────────────────────────┐
              │                                                     │
       MacosTtsSession                                       PiperSession
              │                                                     │
   synthesize_streaming(voice, text, on_event)            for phrase in PhraseSplitter.push(text):
              │                                              synth_phrase(phrase) → AudioChunk
   ┌──────────┴───────────┐                                  on_event(Audio(chunk))
   │                      │                                  on_event(PhraseEnd)
 main-thread path    background path
   │                      │
 sync_channel +        sync_channel +
 NSRunLoop drain      recv_timeout loop
   │                      │
 on_event fires       on_event fires
 from main thread     from caller thread
              │
              ↓
       on_committed_audio(samples) → output resampler → speaker ringbuf → cpal
                                        ↓
                                  on_event(PhraseEnd)
                                        ↓
       on_committed_audio(silence 200ms) → speaker ringbuf
```

## Error handling

- 30s synthesis timeout still surfaces as `PrimerError::Speech("...30s drain timeout (main-thread path|background-thread path)")`. Wording preserved.
- A `RecvTimeoutError::Disconnected` (background path) means the PCM-callback closure was dropped before EOS — surface as a new `PrimerError::Speech("synth channel disconnected before EOS")`. Should not happen in practice (block is retained by AVF), but defensive surface beats a silent stall.
- `on_event` panics propagate up through `push_text` / `finalize` as today — caller's responsibility.

## Magic numbers (per project convention)

New consts go to `primer-speech/src/macos/tts.rs` (macOS-specific; not shared with Piper or other crates):

```rust
/// Bounded channel between the GCD main-queue PCM callback and the
/// caller thread that drains events. AVSpeechSynthesizer fires its PCM
/// callback ~10 times per phrase; 64 gives 6× headroom for a stalled
/// drainer. A `send` that blocks indicates real backpressure.
const PCM_EVENT_CHANNEL_CAPACITY: usize = 64;

/// Background-path `recv_timeout` slice. Short enough that the 30s
/// overall deadline check fires promptly on a hung synth; long enough
/// to amortise wakeup cost.
const STREAM_DRAIN_POLL_MS: Duration = Duration::from_millis(10);
```

`DRAIN_TIMEOUT`, `RUN_LOOP_SLICE`, `EOS_FRAME_LENGTH` are unchanged.

## Test plan

### 1. Trait-level structural test (primer-core)

Update `primer-core/src/speech.rs::tests::synthesis_session_trait_dispatches` to drive the new callback API. Stub backend's `push_text` impl emits `Audio(chunk)` then `PhraseEnd`. Assertions:
- `on_event` invoked exactly 2 times for one push (one Audio + one PhraseEnd).
- Event sequence is `[Audio(_), PhraseEnd]` in order.
- Same shape for `finalize`.

### 2. macOS PCM-streaming structural test (`tests/macos_tts.rs`)

New `streaming_emits_audio_events_before_phrase_end`. Runs in the existing `harness = false` main-thread test binary. Records the `on_event` call sequence:

```rust
#[derive(Debug, PartialEq)]
enum EventKind { Audio, PhraseEnd }
let mut events: Vec<EventKind> = Vec::new();
let mut on_event = |e: SynthesisEvent| events.push(match e {
    SynthesisEvent::Audio(_) => EventKind::Audio,
    SynthesisEvent::PhraseEnd => EventKind::PhraseEnd,
});
let mut session = tts.open_session(&voice).unwrap();
session.push_text("This is a short test phrase.", &mut on_event).unwrap();
Box::new(session).finalize(&mut on_event).unwrap();

let audio_before_first_phrase_end = events
    .iter()
    .take_while(|e| **e != EventKind::PhraseEnd)
    .filter(|e| **e == EventKind::Audio)
    .count();
assert!(
    audio_before_first_phrase_end >= 2,
    "expected ≥2 Audio events before PhraseEnd (streaming shape); got {events:?}"
);
let phrase_end_count = events.iter().filter(|e| **e == EventKind::PhraseEnd).count();
assert_eq!(phrase_end_count, 1, "exactly one PhraseEnd per phrase");
```

A coalescing impl (today's shape) would produce `[Audio, PhraseEnd]` and fail the ≥2 assertion. This is what pins the streaming guarantee structurally.

### 3. State-machine TTFA test (`voice_loop/state_machine.rs::tests`)

New `streaming_chunks_reach_speaker_before_phrase_completes`. New `TimedMockTts`:

```rust
struct TimedMockTts;
impl SynthesisSession for TimedMockTtsSession {
    fn push_text(&mut self, _: &str, on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
        on_event(SynthesisEvent::Audio(AudioChunk { samples: vec![0.1; 64], sample_rate: 16_000 }));
        std::thread::sleep(Duration::from_millis(50));
        on_event(SynthesisEvent::Audio(AudioChunk { samples: vec![0.2; 64], sample_rate: 16_000 }));
        std::thread::sleep(Duration::from_millis(50));
        on_event(SynthesisEvent::Audio(AudioChunk { samples: vec![0.3; 64], sample_rate: 16_000 }));
        on_event(SynthesisEvent::PhraseEnd);
        Ok(())
    }
    ...
}
```

`on_committed_audio` records `(Instant::now(), samples.first().copied())` for each non-empty call. Assertion: the timestamp of the **first** committed sample (~0.1) precedes the timestamp of the **third** committed sample (~0.3) by ≥80 ms — proves the consumer doesn't buffer chunks before forwarding.

`push_text` is synchronous (not `async`), so plain `std::thread::sleep` is correct here — no `block_in_place` needed (which would also panic on a `current_thread` runtime). The 80 ms threshold has 20 ms slack vs the 100 ms total sleep budget — large enough to absorb CI noise. Test adds ~100 ms to the workspace test wallclock.

### 4. Manual TTFA smoke (`examples/tts_macos_pcm_smoke.rs`)

Extend the existing example with a `--measure-ttfa` flag. With the flag, the example records `Instant::now()` immediately after `writeUtterance:` returns and again on the first `Audio` event, prints:

```
[smoke] TTFA: 47 ms (writeUtterance → first PCM callback) for voice "com.apple.speech.synthesis.voice.daniel.premium"
[smoke] PhraseEnd: 312 ms (writeUtterance → EOS) for voice "..."
[smoke] Streaming win: 312 - 47 = 265 ms earlier than coalesce
```

No assertion; instrumentation only. Re-run after macOS major releases per the existing convention.

### 5. Existing mock updates (compile fix; ≤1-line per site)

- `MockTtsSession::push_text` (state_machine.rs:1033-1041): emit `Audio(chunk)` then `PhraseEnd` instead of returning `Vec<_>`. Identical observable behaviour to today.
- `MockTtsSession::finalize` (state_machine.rs:1042): same.
- `CapturingSession::push_text` / `finalize` (state_machine.rs:1948-1962): same.
- `StubSynthesisSession::push_text` / `finalize` (speech.rs:487-498): same.

### 6. Echo-suppression invariant (`is_speaking` flag)

The existing `is_speaking` flag is set BEFORE `session.push_text` is called and cleared AFTER the drain hook returns. With streaming, the flag is set BEFORE the first audio arrives at the speaker (same as today) and cleared AFTER the drain hook reports the ringbuf is empty (same as today). No new test needed; the existing `is_speaking_*` tests still pass unchanged.

### 7. Test counts

- Default-features `cargo test --workspace`: 856 → 857 (one new trait-level test in `primer-core::speech::tests`; the existing `synthesis_session_trait_dispatches` is updated, not duplicated, and we add one regression sibling that asserts the `[Audio, PhraseEnd]` event ordering explicitly).
- `cargo test -p primer-speech --features voice-loop`: 84 → 85 (the new TTFA test).
- `cargo test -p primer-speech --features macos-native` (the `tests/macos_tts.rs` `harness = false` binary): +1 (the structural `streaming_emits_audio_events_before_phrase_end`). Existing count there is small; this PR's lift is one test.
- Other feature combinations (`primer-cli/speech`, `primer-cli/speech,macos-native`, `primer-gui/speech`): counts unchanged — those crates have no new tests, but the trait reshape forces a compile-fix sweep through their mock impls.

## Build sequence

1. Add `SynthesisEvent` enum + reshape trait in `primer-core/src/speech.rs`. Update `StubSynthesisSession` impl. Tests fail to compile in other crates.
2. Update `MockTtsSession`, `CapturingSession`, all stubs in state_machine.rs tests.
3. Update Piper backend (`PiperSession::push_text` / `finalize` to fire callbacks instead of returning Vec).
4. Update state_machine.rs SPEAK phase consumer.
5. Rewrite `MacosTtsSession::push_text` / `finalize` to call new `synthesize_streaming_main_thread` / `_background`. Delete `synthesize_to_chunks_*` + `coalesce_phrase` + `Accumulator` type. The one-shot `TextToSpeech::synthesize` impl is rewritten to drive `synthesize_streaming` with a local `Vec<f32>` accumulator (replaces `chunks_to_audio_buffer`). If this proves complicated, keep `chunks_to_audio_buffer` as a private helper that callers pre-fill via the streaming path.
6. Drop `DispatchSemaphore` + `SynthCtx.sema` from the background path (no longer needed). Keep the GCD FFI module — `dispatch_async_f` is still used.
7. Add the three new tests (trait-level, state-machine TTFA, macOS structural).
8. Extend `examples/tts_macos_pcm_smoke.rs` with TTFA instrumentation.
9. Verify on all feature combinations: default, `primer-cli/speech`, `primer-cli/speech,primer-cli/macos-native`, `primer-gui/speech`, `primer-speech/voice-loop`.

## Risks and open questions

- **PCM-callback re-entrancy:** if AVSpeechSynthesizer fires a second PCM callback while `on_event` is still running from the first, the `mpsc::sync_channel` serialises them naturally. No re-entrancy concern.
- **Channel send blocking:** if the consumer thread stalls (e.g. speaker ringbuf full), the PCM callback would block on `send`. AVF doesn't document what happens if a callback blocks on the main thread — possibly stalls the entire main runloop. Cap of 64 should never fill under normal operation; if it does, that's a real backpressure signal and surfacing it as a stall is acceptable. Alternative: use `try_send` and drop chunks on overflow with a `tracing::warn!`. Decision: prefer blocking-on-send for now; revisit if production observation shows stalls.
- **The dispatch semaphore removal might surface a latent bug** in the GCD main-queue trampoline scheduling that the semaphore was masking. The channel-based path has different timing characteristics. Mitigation: keep both code paths reviewable in the same PR diff and verify with the manual smoke before merge.
- **The state-machine TTFA test uses real `std::thread::sleep`** — 100 ms total per test run. Adds ~100 ms to the workspace test wallclock. Acceptable; the test is critical for pinning the streaming guarantee.
- **`tts.rs` file size after this change** is roughly net-zero (channel boilerplate ≈ accumulator boilerplate) but still over the 500-line guideline. Open a follow-up issue for a `tts/` directory module split at PR close.

## Out of scope

- Per-inference-step Piper streaming.
- A `tts/` directory module split.
- Changing the 200 ms inter-phrase silence value or making it locale-tunable.
- A `try_send` overflow policy for the PCM-event channel.
- Removing `chunks_to_audio_buffer` if the one-shot `TextToSpeech::synthesize` impl can be re-derived purely from the streaming path (planned; if it gets complicated, defer).
