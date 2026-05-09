# Speech and the voice loop

Voice mode is a working proof-of-concept today: a child speaks, Silero detects the end of the utterance, Whisper transcribes it, the dialogue manager produces a response token-stream, Piper synthesises it phrase-by-phrase, and cpal pushes the resulting audio out the speaker — all without leaving the local machine. The state machine that ties these stages together is `LISTEN → THINK → SPEAK → LISTEN`, with strict no-barge-in in either direction.

The chapter is necessarily long because the voice loop is the densest invariant-laden subsystem in the codebase. The compute pieces (VAD, STT, TTS) are well-trodden problems, but the glue around them — when the mic is muted, when the audio thread drops samples, how a phrase tail makes it through a streaming resampler, what happens when a 2-line response runs into a 500 ms ringbuf — is where most of the bugs lived during Phase 0.3 development, and where the comments in [speech_loop.rs](../../src/crates/primer-cli/src/speech_loop.rs) carry hard-won detail. Read this chapter alongside that file when you touch voice code.

This chapter also relies heavily on two vendored crates. That is not aesthetic; it is mechanical. Both are documented below.

## The five speech traits and the Named base trait

The five speech traits all live in [primer-core/src/speech.rs](../../src/crates/primer-core/src/speech.rs):

| Trait | Purpose |
|---|---|
| [`VoiceActivityDetector`](../../src/crates/primer-core/src/speech.rs) | Audio frames in, per-chunk speech-state events out (`SpeechStart` / `SpeechEnd` / `None`). |
| [`SpeechToText`](../../src/crates/primer-core/src/speech.rs) | One-shot: full audio buffer in, transcript out. |
| [`StreamingSpeechToText`](../../src/crates/primer-core/src/speech.rs) | Open a [`TranscriptionSession`](../../src/crates/primer-core/src/speech.rs), push samples, drain segments. |
| [`TextToSpeech`](../../src/crates/primer-core/src/speech.rs) | One-shot: text in, audio buffer out. |
| [`StreamingTextToSpeech`](../../src/crates/primer-core/src/speech.rs) | Open a [`SynthesisSession`](../../src/crates/primer-core/src/speech.rs), push partial text, drain audio chunks per phrase. |

Every one of those traits inherits from a single base trait, [`Named`](../../src/crates/primer-core/src/speech.rs):

```rust
// src/crates/primer-core/src/speech.rs
pub trait Named {
    fn name(&self) -> &str;
}

pub trait VoiceActivityDetector: Named + Send { /* ... */ }
pub trait SpeechToText: Named + Send + Sync { /* ... */ }
pub trait StreamingSpeechToText: Named + Send + Sync { /* ... */ }
pub trait TextToSpeech: Named + Send + Sync { /* ... */ }
pub trait StreamingTextToSpeech: Named + Send + Sync { /* ... */ }
```

The reason: a single backend struct often implements both the one-shot variant and the streaming variant of the same family — the Whisper backend is both `SpeechToText` and `StreamingSpeechToText`; Piper is both `TextToSpeech` and `StreamingTextToSpeech`. If `name()` were declared on both leaf traits, an implementor would have to write it twice, and every direct call site would have to disambiguate via UFCS (`<T as SpeechToText>::name(...)`). With `Named` as a super-trait, each backend writes `name()` once and every leaf gets it for free. The unit test [`named_super_trait_resolves_via_each_speech_trait`](../../src/crates/primer-core/src/speech.rs) is the canary that pins this invariant — adding a sixth speech trait should add a sixth assertion there.

## Concrete backends and feature gates

Stub backends always build. They live in [primer-speech/src/stub.rs](../../src/crates/primer-speech/src/stub.rs) and return canned silent buffers — they exist so the compile graph for the default workspace build stays light, and so unit tests can dispatch through trait objects without pulling in any real audio code.

Real backends are gated behind cargo features in [primer-speech/Cargo.toml](../../src/crates/primer-speech/Cargo.toml):

| Feature | Pulls in | Lives at |
|---|---|---|
| `silero` | [silero-vad-rust](../../src/vendor/silero-vad-rust/) (vendored) | [silero.rs](../../src/crates/primer-speech/src/silero.rs) |
| `whisper` | `whisper-cpp-plus` | [whisper.rs](../../src/crates/primer-speech/src/whisper.rs) |
| `piper` | [piper-rs](../../src/vendor/piper-rs/) (vendored) | [piper.rs](../../src/crates/primer-speech/src/piper.rs) |
| `cpal` | `cpal` + `ringbuf` + `rubato` | [cpal_io.rs](../../src/crates/primer-speech/src/cpal_io.rs) |

The four features are individually selectable for development — you can build with `--features silero` alone if you only want to exercise the VAD against a recorded WAV — but the `--speech` flag on `primer-cli` requires all four. The CLI's `speech` feature pulls them in transitively:

```bash
~/.cargo/bin/cargo build --features primer-cli/speech
```

Two pure helper modules round out the crate: [vad_debounce.rs](../../src/crates/primer-speech/src/vad_debounce.rs) carries the silence-debounce state machine ("how many consecutive silent chunks before we declare the utterance over?") and [phrase_split.rs](../../src/crates/primer-speech/src/phrase_split.rs) carries the streaming phrase splitter that the Piper backend uses to chunk LLM output on `. ! ?` boundaries. Both are dependency-free of the rest of the crate so they can be unit-tested without a backend.

## Why piper-rs is vendored at src/vendor/piper-rs/

The upstream `piper-rs` 0.1.9 release was built against a pre-`rc.10` revision of the [ort](https://github.com/pykeio/ort) ONNX-runtime crate. The rest of the workspace pins `ort = "=2.0.0-rc.10"` (the version that `silero-vad-rust 6.2` was built against), so dropping in upstream `piper-rs` produces a duplicate-`ort` error at link time.

The vendored copy at [src/vendor/piper-rs/](../../src/vendor/piper-rs/) carries a small patch fixing three call sites where the `ort` API surface changed at `rc.10`:

- `Value::from_array` now wants owned arrays.
- `Session::run` now takes `&mut self`.
- `try_extract_tensor` now returns `(&Shape, &[T])`.

The patch is wired up in the workspace [Cargo.toml](../../src/Cargo.toml):

```toml
# src/Cargo.toml
[patch.crates-io]
piper-rs = { path = "vendor/piper-rs" }
silero-vad-rust = { path = "vendor/silero-vad-rust" }
```

The intersection of `ort` versions between `piper-rs` 0.1.x and `silero-vad-rust` 6.2 is empty, so until upstream releases an `rc.10`-compatible version the fork stays. The smoke binary [tts_hello.rs](../../src/crates/primer-speech/examples/tts_hello.rs) covers the basic synth path so a future upstream update can be validated end-to-end with one command.

## Why silero-vad-rust is vendored at src/vendor/silero-vad-rust/

Three patches, all small:

1. **`is_multiple_of(N)` → `% N == 0`.** Upstream uses the unstable `unsigned_is_multiple_of` API. The toolchain pin in [rust-toolchain.toml](../../src/rust-toolchain.toml) is not enough on its own — `is_multiple_of` is still unstable on rustc 1.97-nightly, so the call sites had to be rewritten. Stable rustc cannot compile upstream as-is.
2. **`load-dynamic` removed from the `ort` feature list.** Removing it lets us link ONNX Runtime statically. Combined with the explicit `download-binaries` and `copy-dylibs` features that [primer-speech/Cargo.toml](../../src/crates/primer-speech/Cargo.toml) lists for `ort`, this achieves a fully static `libonnxruntime` link with no `dlopen` failure at runtime.
3. **Crate-level `#![allow(unused_variables, dead_code)]`** to silence two upstream warnings under Primer's `-D warnings` build profile.

The patch ships as the entire vendored crate at [src/vendor/silero-vad-rust/](../../src/vendor/silero-vad-rust/) and is wired into [src/Cargo.toml](../../src/Cargo.toml) alongside `piper-rs` (see snippet in the previous section). Drop the vendor patch as soon as a `silero-vad-rust` release picks up the API surface and stable Rust supports `is_multiple_of`.

## The speech_loop state machine

The voice loop lives in [speech_loop.rs](../../src/crates/primer-cli/src/speech_loop.rs). At runtime it cycles through four logical states:

```
LISTEN  → mic open, VAD watching for SpeechStart
  ↓ (SpeechStart)
LATENT_THINK + LISTEN  → child speaking; whisper streaming open;
                         on SpeechEnd, finalize transcript
  ↓ (SpeechEnd)
THINK   → mic still open (cancel-on-SpeechStart aborts the LLM if
          the child resumes); call dialogue manager; LLM streams reply
  ↓ (response complete)
SPEAK   → mic muted; Piper synthesises phrase by phrase; cpal pushes
          to speakers; await drain hook
  ↓ (drain complete)
LISTEN  → next utterance
```

Two invariants:

- **The Primer never speaks over the child.** During LATENT_THINK and THINK the mic stays open. If the child starts speaking again before the LLM has produced a complete response, the `cancel-on-SpeechStart` path aborts the in-flight inference call cleanly and re-enters LISTEN.
- **The child never speaks over the Primer.** During SPEAK the mic is hard-muted via the `is_speaking` flag (next section) — incoming samples are drained and discarded, the active whisper session is dropped, and local buffers are cleared. The mic does not re-open until the speaker ringbuf is fully drained.

> **Why:** No barge-in is pedagogical, not a POC limitation. Learning to listen — to wait for the answer rather than interrupting — is part of the educational experience the Primer is trying to deliver. Don't "fix" this by adding barge-in support; ask first.

## The is_speaking flag and drain-hook discipline

The mute mechanism is an `Arc<AtomicBool>` plumbed from [`run_loop`](../../src/crates/primer-cli/src/speech_loop.rs) into the audio capture thread. The audio thread polls the flag every 5 ms; while it reads `true`, it drains and discards mic samples instead of feeding them to the VAD.

Around SPEAK, the flag is set to `true` before the first phrase is pushed to cpal, and cleared only after the speaker ringbuf has fully drained. The drain wait runs through the [`DrainHook`](../../src/crates/primer-cli/src/speech_loop.rs) abstraction:

```rust
// src/crates/primer-cli/src/speech_loop.rs
pub type DrainHook = /* boxed FnMut returning a future */;
```

In production, the hook is a `tokio::task::spawn_blocking` wrapper around [`primer_speech::wait_for_drain`](../../src/crates/primer-speech/src/cpal_io.rs). `wait_for_drain` polls the speaker ringbuf's `occupied_len()` until it reads zero on three consecutive 10 ms checks (defending against momentary cpal buffer underruns being misread as "drained"), then waits an additional 80 ms grace period for cpal's own internal output buffer to flush, with a 5 s sanity cap that includes the grace window. In tests, `run_loop` accepts `None` as the hook because mock speaker sinks have no real ringbuf to drain.

> **Gotcha:** Calling `std::thread::sleep` directly inside `on_audio` (which runs in the same task as `run_loop`) would block other tasks for up to 5 s and panic outright on a single-threaded tokio runtime. Going through `spawn_blocking` is what keeps the synchronous wait off the tokio worker. If you change the drain mechanism, preserve this property — the on_audio path must never sleep on the runtime thread.

The speaker producer is wrapped in `Arc<Mutex<HeapProd<f32>>>` so both the `on_audio` push path and the drain hook can access it. The mutex is uncontended in practice — push and observe never overlap by the state-machine design — but is needed because Rust's type system cannot prove that on its own.

## Speaker ringbuf sizing

The speaker SPSC ring buffer is sized for ~5 s of 48 kHz mono audio:

```rust
// src/crates/primer-speech/src/cpal_io.rs
const SPEAKER_RINGBUF_CAPACITY: usize = 240_000;
```

The previous size of 24,000 samples (~500 ms) had a subtle and devastating failure mode: when Piper synthesised a multi-phrase response faster than cpal drained, the ringbuf filled, the producer dropped samples on push failure, and only the first phrase made it through. The child would hear "The Sun is a star." and silence where the rest of the answer should have been. The 240,000-sample capacity comfortably absorbs a multi-second phrase without back-pressure.

> **Gotcha:** Never shrink the speaker ringbuf below 240,000 samples. The previous 24,000-sample buffer dropped phrases — only the first phrase of a multi-phrase response made it through. If you find yourself reducing this for a memory-constrained build target, you need a different fix (e.g. block-on-full instead of drop-on-full), not a smaller buffer.

## Markdown stripping for TTS

The LLM produces markdown — `*emphasis*`, `**strong**`, `` `code` `` — and Piper has no idea what to do with asterisks or backticks. They come out as literal "asterisk asterisk" or get phonemised into something garbled. So before pushing to Piper, the speech loop runs the response through [`strip_markdown_for_tts`](../../src/crates/primer-cli/src/speech_loop.rs), which removes paired markers and leaves bare unmatched ones alone:

| Input | Stripped |
|---|---|
| `*why*` | `why` |
| `**important**` | `important` |
| `` `code` `` | `code` |
| `a* footnote` | `a* footnote` (bare unmatched stays) |
| `5*3=15` | `5 times 3=15` (digit-flanked `*` → multiplication) |
| `5**2` | `5 times 2` (digit-flanked `**` → exponent) |
| `value *= 5` | `value *= 5` (operator stays) |

The digit-flanking rule reads multiplication and exponent notation aloud naturally — without it, "5*3" would be voiced "five star three" or worse. The visible transcript on stdout retains the original markdown unchanged; only the TTS path strips. The complete coverage is in the [unit tests in speech_loop.rs](../../src/crates/primer-cli/src/speech_loop.rs).

## The dedicated audio-capture thread

The audio capture path runs on its own `std::thread`, **not** a tokio task. It owns the silero VAD instance, the active whisper streaming session, and the mic-side resampler. It emits two kinds of output:

- `VadEvent`s (`SpeechStart`, `SpeechEnd`, `None`) over a `tokio::sync::mpsc` channel that `run_loop` consumes.
- On `SpeechEnd`, the finalised transcript over a separate channel that `run_loop` reads via a [`ChannelStt`](../../src/crates/primer-cli/src/speech_loop.rs) wrapper — a thin adapter that implements `StreamingSpeechToText` against the channel so `run_loop` can treat audio capture as a regular trait object.

This split keeps test/production cleanly separated: mocks bypass `cpal` entirely by injecting events and transcripts directly into those channels. The whole audio stack (cpal, ringbufs, rubato resampler, silero, whisper) is dark to unit tests, which is what makes the [speech_loop tests](../../src/crates/primer-cli/src/speech_loop.rs) tractable.

There is also an ordering contract worth flagging: the audio thread MUST send the transcript on the transcript channel **before** emitting `VadEvent::SpeechEnd` on the event channel. `run_loop` relies on that ordering to call `finalize` on a session that already has its final segment queued. Reversing the order would race.

## Resampler tail-handling

Sample-rate conversion runs through `rubato`'s `FftFixedIn`. The mic side resamples device-rate → 16 kHz for the VAD and Whisper; the speaker side resamples voice-config rate → device output rate for cpal. Both share the same hazard: `FftFixedIn` buffers FFT state internally, and any input sample that doesn't fall on a chunk boundary is held back inside the resampler until enough trailing samples arrive to complete the next FFT.

That's a problem at end-of-utterance. Without help, the last syllable or word of each phrase — and often the entire last phrase of a multi-phrase response — gets silently discarded because the resampler is still holding its tail.

Two mechanisms together solve this:

1. **A `leftover: Vec<f32>` buffer carried across `on_audio` calls.** Each call prepends the previous call's leftover before processing, so phrase tails are stitched into the next call rather than zero-padded mid-stream.
2. **An end-of-turn flush sentinel.** When `on_audio` is called with an empty `Vec`, it zero-pads any remaining leftover up to the chunk boundary and then drives **four extra silence chunks** (~186 ms of input silence at the relevant rates) through the resampler to drain its FFT-buffered output. Empirically, fewer chunks were not enough — `FftFixedIn` needs a non-trivial silence tail to flush its lookahead.

If you change the resampler config, validate against the multi-phrase end-of-response case specifically. A test that passes a single phrase will not catch a regression here.

## espeak-ng requirement and probe

Piper uses `espeak-ng` to phonemise text before feeding it into the ONNX voice model. The `espeak-rs` crate ships an embedded subset of the espeak-ng data files, but that subset is **incomplete** — it is missing files like `phontab` that most non-English voices need, and even the English voices fail in subtle ways without the full data set.

The fix is to install `espeak-ng` system-wide and point Piper at the system data directory:

```bash
# macOS (Apple Silicon and Intel)
brew install espeak-ng

# Debian / Ubuntu
sudo apt install espeak-ng-data

# Fedora / RHEL
sudo dnf install espeak-ng-data
```

At startup, [`probe_espeak_ng_data` in main.rs](../../src/crates/primer-cli/src/main.rs) walks three candidate locations and sets the `PIPER_ESPEAKNG_DATA_DIRECTORY` environment variable to the parent of the first complete `espeak-ng-data` directory it finds:

```
/opt/homebrew/share   # macOS Apple Silicon (brew install espeak-ng)
/usr/local/share      # macOS Intel / generic
/usr/share            # Linux (apt/dnf)
```

It checks for the presence of `espeak-ng-data/phontab` rather than just the directory existence, because `espeak-rs`'s incomplete subset would otherwise satisfy a directory-only probe and leave you with phoneme failures at runtime. If `PIPER_ESPEAKNG_DATA_DIRECTORY` is already set when the probe runs, the probe respects it.

> **Gotcha:** Voice id must be passed explicitly. Pass `--voice <model-id>` matching the file stem of `--voice-onnx` (e.g. `--voice en_GB-alba-medium` for `en_GB-alba-medium.onnx`). Before this fix, `run_loop` hardcoded `VoiceProfile::default()` (`en_US-amy-medium`) as the voice profile passed to Piper's `open_session`, and Piper rejected any non-default voice with a model-id mismatch error. The CLI now also validates that `--voice` matches the `--voice-onnx` filename stem and errors at parse time if not.

> **Gotcha:** Build with rustup, not Homebrew rust. This was already mentioned in [chapter 1](01-getting-started.md), but it bears repeating here because speech is where the bug surfaces hardest. [rust-toolchain.toml](../../src/rust-toolchain.toml) pins Rust 1.87+, but that pin is honoured only by rustup's proxy binaries. If a Homebrew-installed `cargo` shadows on `PATH`, Cargo silently falls back to whatever Homebrew has (often 1.86), and silero will fail to compile with a confusing trait-resolution error. Always invoke as `~/.cargo/bin/cargo` for any speech-feature build.

> **Gotcha:** Voice rate is `0.9` (length_scale ≈ 1.111), not 1.0. Each phrase is followed by 200 ms of injected silence before the next chunk is pushed to cpal. Both numbers were tuned against the target audience: too fast and a 7-year-old loses the thread; too breathless and the pacing feels artificial. Adjust [`VoiceProfile.rate`](../../src/crates/primer-core/src/speech.rs) and the inter-chunk sleep in [speech_loop.rs](../../src/crates/primer-cli/src/speech_loop.rs) if needed, but treat it as a UX call, not a code call.

---

### Recipe — Add a speech backend

Worked example: a hypothetical streaming STT backend that wraps a different upstream model (say, a `Vosk`-based one). Seven steps.

**1. Pick the trait.** The five traits map cleanly to the three logical roles:

| Role | Trait | Audio direction |
|---|---|---|
| VAD | [`VoiceActivityDetector`](../../src/crates/primer-core/src/speech.rs) | frames in, events out |
| STT | [`SpeechToText`](../../src/crates/primer-core/src/speech.rs) (one-shot) or [`StreamingSpeechToText`](../../src/crates/primer-core/src/speech.rs) (live) | audio in, text out |
| TTS | [`TextToSpeech`](../../src/crates/primer-core/src/speech.rs) (one-shot) or [`StreamingTextToSpeech`](../../src/crates/primer-core/src/speech.rs) (live) | text in, audio out |

For a live STT backend, implement `StreamingSpeechToText` plus a [`TranscriptionSession`](../../src/crates/primer-core/src/speech.rs). For batch use, the one-shot trait alone is fine.

**2. Implement it.** `Named::name()` is written once via super-trait inheritance:

```rust
// src/crates/primer-speech/src/vosk.rs (sketch)
use primer_core::speech::{
    Named, StreamingSpeechToText, TranscriptionSession, TranscriptSegment,
};
use primer_core::error::Result;

pub struct VoskStt { /* model handle */ }

impl Named for VoskStt {
    fn name(&self) -> &str { "vosk" }
}

impl StreamingSpeechToText for VoskStt {
    fn sample_rate(&self) -> u32 { 16_000 }

    fn open_session(&self) -> Result<Box<dyn TranscriptionSession>> {
        // ... allocate a per-utterance recogniser
        todo!()
    }
}
```

Note that `StreamingSpeechToText` is `Send + Sync` — the backend is shared across sessions. Per-utterance state lives inside the `TranscriptionSession` (which is `Send` but not `Sync`). Don't move per-utterance buffers into the backend struct.

**3. Gate behind a cargo feature.** In [primer-speech/Cargo.toml](../../src/crates/primer-speech/Cargo.toml), add the dep as `optional = true` and a feature that enables it. Naming convention: lowercase backend name, matching the existing pattern (`silero`, `whisper`, `piper`, `cpal`):

```toml
# src/crates/primer-speech/Cargo.toml
[dependencies]
vosk = { version = "0.x", optional = true }

[features]
vosk = ["dep:vosk"]
```

If your backend needs `ort`, list `ort` features explicitly with `default-features = false`, mirroring the pattern in the existing `[dependencies]` block — without `download-binaries` and `copy-dylibs` ONNX Runtime cannot be found at link time, and the failure mode at build time is misleading.

**4. Vendor or patch incompatible deps.** If your upstream uses a different `ort` revision (or any other workspace-wide pinned dep), drop a vendored copy under [src/vendor/<crate>/](../../src/vendor/) and add the patch to [src/Cargo.toml](../../src/Cargo.toml):

```toml
# src/Cargo.toml
[patch.crates-io]
my-upstream-crate = { path = "vendor/my-upstream-crate" }
```

Keep the vendored copy minimal — preserve upstream `LICENSE`, `README.md`, and original `Cargo.toml.orig` (rename the original `Cargo.toml` to `Cargo.toml.orig` before editing). The patch should be the smallest possible change to make the API compatible. Note in this chapter and in the vendored crate's `BUILDING.md` what the patch does and when it can be dropped — see [vendor/piper-rs/BUILDING.md](../../src/vendor/piper-rs/BUILDING.md) for the precedent.

**5. Wire into LoopBackends.** The voice loop selects backends by trait object in [speech_loop.rs](../../src/crates/primer-cli/src/speech_loop.rs). Add a feature-gated factory branch in [main.rs](../../src/crates/primer-cli/src/main.rs) that constructs your backend from CLI flags and passes it through to `LoopBackends`. Keep the construction error path graceful — a missing model file should produce a clear error message naming the flag and the file, not a panic.

**6. Add a smoke example.** Following the precedent of [tts_hello.rs](../../src/crates/primer-speech/examples/tts_hello.rs), add an example at `src/crates/primer-speech/examples/<name>.rs` that exercises the backend without the full REPL — for an STT backend, that means reading a fixed WAV file and printing the transcript. Mark it `required-features` in [Cargo.toml](../../src/crates/primer-speech/Cargo.toml) so it only builds when the relevant feature is on:

```toml
# src/crates/primer-speech/Cargo.toml
[[example]]
name = "vosk_hello"
required-features = ["vosk"]
```

The smoke example is what lets a contributor (and a future you, after a vendor-patch dependency bump) validate the backend in isolation in seconds.

**7. Add system-dep installation notes** to [chapter 8](08-testing-and-debugging.md) if your backend needs a system package. Piper's espeak-ng dependency lives there for exactly this reason; whisper.cpp's cmake/C++-toolchain requirement also belongs in chapter 8. If your backend is pure-Rust with no system deps, no chapter-8 entry is needed.

When you're done: build with `--features primer-speech/<your-feature>`, run the smoke example, and verify it produces sensible output. Then build with `--features primer-cli/speech` — confirming that your new backend doesn't break the existing voice loop is the load-bearing integration test.
