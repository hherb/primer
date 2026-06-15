# Speech and the voice loop

Voice mode is a working feature today: a child speaks, a VAD detects the end of the utterance, an STT backend transcribes it, the dialogue manager produces a response token-stream, a TTS backend synthesises it phrase-by-phrase, and cpal pushes the resulting audio out the speaker — all without leaving the local machine. The state machine that ties these stages together is `LISTEN → LATENT_THINK → SPEAK → LISTEN`, with strict no-barge-in in either direction. The default portable stack is Silero (VAD) + Whisper (STT) + Piper (TTS), but STT and TTS are now **decoupled runtime choices** (see "Decoupled STT and TTS" below), and on Apple platforms native backends are available too.

The state machine itself is the shared [`voice_loop`](../../src/crates/primer-speech/src/voice_loop/) module on `primer-speech` (behind the `voice-loop` cargo feature). Both binaries consume it: `primer-cli` in `--speech` mode (via a `StdoutObserver`) and `primer-gui` via its Voice-mode toggle (via a `TauriEventObserver` emitting `primer://voice/*` events). The earlier CLI-only `speech_loop.rs` is now a thin wrapper that calls into `voice_loop`; the loop logic, the mic/speaker construction, and the no-barge-in invariants all live in the shared module so the two binaries never fork their voice behaviour.

The chapter is necessarily long because the voice loop is the densest invariant-laden subsystem in the codebase. The compute pieces (VAD, STT, TTS) are well-trodden problems, but the glue around them — when the mic is muted, when the audio thread drops samples, how a phrase tail makes it through a streaming resampler, what happens when a 2-line response runs into a 500 ms ringbuf — is where most of the bugs lived during Phase 2 development, and where the comments in the [voice_loop module](../../src/crates/primer-speech/src/voice_loop/) carry hard-won detail. Read this chapter alongside those files when you touch voice code.

This chapter also relies heavily on three vendored crates. That is not aesthetic; it is mechanical. All three are documented below.

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

| Feature | Role | Pulls in | Notes |
|---|---|---|---|
| `silero` | VAD | [silero-vad-rust](../../src/vendor/silero-vad-rust/) (vendored) | The VAD on every non-macOS-26 build, including macOS-native. |
| `whisper` | STT | `whisper-cpp-plus` | The portable streaming STT; required for Hindi. |
| `piper` | TTS | [piper-rs](../../src/vendor/piper-rs/) (vendored) | The portable streaming TTS. |
| `supertonic` | TTS | `ort`-based multilingual model | One multilingual model (32 languages incl. Hindi/Japanese); Piper-class CPU RTF. |
| `cpal` | audio I/O | `cpal` + `ringbuf` + `rubato` | Mic capture + speaker output. |
| `voice-loop` | state machine | (the above building blocks) | The shared `LISTEN → LATENT_THINK → SPEAK` loop both binaries consume. |
| `macos-native` | STT + TTS | `objc2-speech` / AVFoundation | `SFSpeechRecognizer` STT + `AVSpeechSynthesizer` TTS on macOS 13+; en-US / de-DE. Silero stays the VAD. |
| `macos-native-26` | STT + VAD + TTS | Swift sidecar via `swift-bridge` | `SpeechAnalyzer`/`SpeechTranscriber`/`SpeechDetector` on macOS 26+; AVSpeechSynthesizer for TTS. **Mutually exclusive with `macos-native`** (a `compile_error!` enforces the XOR). |

The portable building-block features are individually selectable for development — you can build with `--features silero` alone if you only want to exercise the VAD against a recorded WAV — but the `--speech` flag on `primer-cli` requires the full portable stack. The CLI's `speech` feature pulls Silero, Whisper, Piper, cpal, and the `voice-loop` state machine in transitively:

```bash
~/.cargo/bin/cargo build --features primer-cli/speech
```

The Apple-native backends layer on top: build with `--features primer-cli/speech,primer-cli/macos-native` (macOS 13+) or `--features primer-cli/speech,primer-cli/macos-native-26` (macOS 26+). The `macos-native`/`macos-native-26` XOR is enforced at compile time in `primer-speech/src/lib.rs`, `primer-cli/src/main.rs`, and `primer-gui/src/lib.rs`. The GUI mirrors these features as `primer-gui/speech`, `primer-gui/macos-native`, and `primer-gui/macos-native-26`.

Two pure helper modules round out the crate: [vad_debounce.rs](../../src/crates/primer-speech/src/vad_debounce.rs) carries the silence-debounce state machine ("how many consecutive silent chunks before we declare the utterance over?") and [phrase_split.rs](../../src/crates/primer-speech/src/phrase_split.rs) carries the streaming phrase splitter that the Piper (and Supertonic, and AVSpeechSynthesizer) backend uses to chunk LLM output on `. ! ?` boundaries. Both are dependency-free of the rest of the crate so they can be unit-tested without a backend.

## Decoupled STT and TTS

STT and TTS are independent runtime choices (issue #170). STT picks *which voice-loop builder skeleton* runs; TTS picks *which synthesiser is injected into it*; they vary independently. The three voice-loop builders no longer construct their own TTS — each takes an injected `tts: Arc<dyn StreamingTextToSpeech>` plus a `VoiceProfile`.

Two shared helpers in [voice_loop/selectors.rs](../../src/crates/primer-speech/src/voice_loop/selectors.rs) are the single construction path both binaries use:

- `build_tts(TtsBackend, &TtsAssets) -> (Arc<dyn StreamingTextToSpeech>, VoiceProfile)` — feature-gated per arm; an uncompiled choice returns a `PrimerError::Speech` with a "rebuild with `--features X`" hint (the same shape as the qnn backend pattern).
- `build_voice_backends(SttBackend, tts, voice, whisper_model, …)` — cfg-dispatches to the Whisper / macos-native / macos-native-26 builder skeleton.

The `SttBackend { Whisper, MacosNative }` and `TtsBackend { Piper, Supertonic, MacosNative }` enums live in `primer-speech`, gated on the `voice-loop` building blocks. The CLI uses them directly via `--tts piper|supertonic` (with `--supertonic-dir` / `--supertonic-voice-style` required when `--tts supertonic`, enforced by a clap `ArgGroup` plus a runtime `validate_speech_assets` completeness check). The GUI defines its *own* mirror `config::SttBackend` / `config::TtsBackend` enums (because `primer-gui/src/config.rs` is always compiled but `primer-speech` is an optional dep) and offers separate STT and TTS dropdowns in Settings → Speech; the GUI→`primer_speech` enum conversion happens at the `speech`-gated `voice/backends.rs` boundary.

> **Why:** decoupling is what lets Supertonic (the multilingual TTS, the reason Hindi voice works) pair with Whisper STT independently — Hindi has no on-device Apple STT, so the Hindi voice path is Whisper-STT + Supertonic-TTS. macOS-native STT/TTS have no `hi-IN` anyway, so the CLI macos-native build stays compile-time AVSpeech.

> **Gotcha:** the GUI's three backend builders are gated so each file is built by exactly one cargo feature: [backends.rs](../../src/crates/primer-speech/src/voice_loop/backends.rs) (Whisper + injected TTS), [backends_macos_native.rs](../../src/crates/primer-speech/src/voice_loop/backends_macos_native.rs) (SFSpeechRecognizer + Silero + injected TTS), [backends_macos_native_26.rs](../../src/crates/primer-speech/src/voice_loop/backends_macos_native_26.rs) (SpeechAnalyzer + derived VAD + injected TTS). All three re-export at `voice_loop::*` — **call sites must use the public re-export** (`voice_loop::build_local_backends_macos_native`), not the inner module path, so a future internal rename doesn't break callers. Shared mic/speaker/closure helpers live in `voice_loop::backends_common` so the post-mic construction is verified once, not copied three times.

## Apple-native speech backends

Two Apple-native feature stacks exist for macOS (and, eventually, iOS — the modules are internally Apple-portable via `cfg(target_vendor = "apple")`).

- **`macos-native`** ([primer-speech/src/macos/](../../src/crates/primer-speech/src/macos/)) uses `SFSpeechRecognizer` with `requiresOnDeviceRecognition = true` (a hard error if the locale isn't on-device — it never falls back to the network, per [[project_strict_offline_first]]) for STT, and a streaming `AVSpeechSynthesizer` (`writeUtterance:toBufferCallback:` + `PhraseSplitter`) for TTS. Silero stays the VAD (the macOS-26-only `SpeechDetector` would break the macOS 13 floor). Locales: `en-US`, `de-DE`.
- **`macos-native-26`** ([primer-speech/src/macos26/](../../src/crates/primer-speech/src/macos26/)) uses `SpeechAnalyzer` + `SpeechTranscriber` + `SpeechDetector` via a Swift sidecar at `crates/primer-speech/swift-sources/Macos26PipelineImpl.swift`, bridged to Rust through `swift-bridge` and compiled to `libMacos26Pipeline.a` by the crate's `build.rs` (`swiftc`). It reuses `MacosTextToSpeech` for TTS (AVSpeechSynthesizer is unchanged on macOS 13+ and 26+). VAD events are *derived* from transcriber activity by `DerivedVadStateMachine`. Empirically ~100× faster to first partial than Whisper (~30 ms vs ~3.8 s). Locales: `en-US`, `de-DE` (Hindi errors loudly — `SpeechTranscriber` has no `hi-IN` as of macOS 26.5).

Both are the friction-free demo surface, not production children's hardware: asset download is OS-managed (silent), and the `macos-native`/`macos-native-26` XOR is enforced by a `compile_error!`. See the long-form gotchas in [CLAUDE.md](../../CLAUDE.md) for the main-thread / `dispatch2` requirements, the streaming `mpsc<SynthesisEvent>` channel discipline, and the `dispatch2` whole-struct-capture (RFC 2229) trap.

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

## Why ort-sys is vendored at src/vendor/ort-sys/

The third vendored crate is `ort-sys` 2.0.0-rc.10 — a **verbatim** copy of the crates.io release with a single one-arm cfg patch in `src/internal/dirs.rs`: the Linux `cache_dir()` arm is broadened from `#[cfg(target_os = "linux")]` to `#[cfg(any(target_os = "linux", target_os = "android"))]`. rc.10 gates `cache_dir()` on windows/linux/macos with no catch-all, so on an Android host the function is configured out and `build.rs`'s `use ...::cache_dir` fails to resolve (E0432) — the Termux build blocker (issue #157). The Linux XDG logic works unchanged on Android because Termux populates `$XDG_CACHE_HOME` / `$HOME`.

It is wired into [src/Cargo.toml](../../src/Cargo.toml) alongside the other two:

```toml
# src/Cargo.toml
[patch.crates-io]
piper-rs = { path = "vendor/piper-rs" }
silero-vad-rust = { path = "vendor/silero-vad-rust" }
ort-sys = { path = "vendor/ort-sys" }
```

> **Gotcha:** keep the rest byte-identical to rc.10 — the patch is a single cfg attribute so a future rebase onto a newer rc is a trivial re-apply. It is **rc.10-shaped, not main-shaped**: upstream `main` already made `cache_dir()` unconditional, so do not "modernise" the patch while the workspace remains pinned to rc.10. Both the `embedding` (fastembed) and `speech` (silero/whisper/piper) feature stacks pin `ort = "=2.0.0-rc.10"`, which is why this patch is load-bearing for both — drop it only once the workspace can move off the rc.10 pin entirely.

## Whisper STT details: language and the stream cache

Two non-obvious facts about the Whisper backend bite if missed.

**Whisper's transcription language must be set explicitly from the learner's locale.** `WhisperStt::new(path)` defaults `language` to `"en"`. Without `.with_language(locale.pack_id())` in `voice_loop::backends::build_local_backends`, the multilingual `ggml-small.bin` model (used for `de`) is forced into English transcription mode, German audio comes out as approximate English, and the LLM then responds in English on a German-locale session. `Locale::pack_id()` returns ISO-639-1 — exactly the form Whisper accepts. This is pinned by `whisper::tests::pack_id_is_iso_639_1_for_whisper`.

**`WhisperStt` reuses a single `WhisperStream` across utterances via a single-slot `StreamCache`** (issue #133). The first `open_session` constructs a fresh stream (≈500 ms wallclock for KV-cache + GPU compute-buffer allocation on Apple Silicon); every subsequent `open_session` `take()`s the cached stream and calls `WhisperStream::reset()` (which clears the audio buffers and per-utterance state while leaving the KV cache intact — that's the load-bearing point). The re-cache policy is **happy-path-only**: a stream is re-cached on a successful flush, but discarded (with a `tracing::warn!`) on flush error or on `Drop` without a prior `finalize`, because a flush-failed stream may carry decoder state we cannot characterise — paying one cold-start tax is safer than silently biasing the next utterance. The cache is single-slot by design (one VAD-driven session at a time per backend); mutex poisoning is non-fatal (logged, falls back to a fresh construction).

## The voice-loop state machine

The voice loop lives in the shared [voice_loop module](../../src/crates/primer-speech/src/voice_loop/) on `primer-speech` — `run_loop` (Arc-based, for the GUI) and `run_loop_borrowed` (`'r`-lifetime `&mut DialogueManager`, for tests and the CLI which owns the DM directly). `primer-cli`'s `speech_loop` module is a thin consumer of it. `run_loop` returns a `LoopHandle` with a `stop_tx` (CLI Ctrl+C / GUI End-voice-mode) and a `cancel_response_tx` (GUI Stop button + Esc; aborts the in-flight LLM + TTS and returns to LISTEN). Per-binary behaviour differences live entirely in the `LoopObserver` implementation each binary supplies (CLI: `StdoutObserver`; GUI: `TauriEventObserver`).

At runtime it cycles through four logical states:

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

The mute mechanism is an `Arc<AtomicBool>` plumbed from [`run_loop`](../../src/crates/primer-speech/src/voice_loop/state_machine.rs) into the audio capture thread. The audio thread polls the flag every 5 ms; while it reads `true`, it drains and discards mic samples instead of feeding them to the VAD.

Around SPEAK, the flag is set to `true` before the first phrase is pushed to cpal, and cleared only after the speaker ringbuf has fully drained. The drain wait runs through the [`DrainHook`](../../src/crates/primer-speech/src/voice_loop/state_machine.rs) abstraction:

```rust
// src/crates/primer-speech/src/voice_loop/state_machine.rs
pub type DrainHook = /* boxed FnMut returning a future */;
```

In production, the hook is a `tokio::task::spawn_blocking` wrapper around [`primer_speech::wait_for_drain`](../../src/crates/primer-speech/src/cpal_io.rs). `wait_for_drain` polls the speaker ringbuf's `occupied_len()` until it reads zero on three consecutive 10 ms checks (defending against momentary cpal buffer underruns being misread as "drained"), then waits an additional 80 ms grace period for cpal's own internal output buffer to flush, with a 5 s sanity cap that includes the grace window. In tests, `run_loop` accepts `None` as the hook because mock speaker sinks have no real ringbuf to drain.

> **Gotcha:** Calling `std::thread::sleep` directly inside `on_audio` (which runs in the same task as `run_loop`) would block other tasks for up to 5 s and panic outright on a single-threaded tokio runtime. Going through `spawn_blocking` is what keeps the synchronous wait off the tokio worker. If you change the drain mechanism, preserve this property — the on_audio path must never sleep on the runtime thread.

> **Gotcha (macOS-native CLI):** on a `cfg(all(target_os = "macos", feature = "macos-native"))` build, `primer-cli` runs its tokio runtime as `Builder::new_current_thread()` on the OS main thread, because AVSpeechSynthesizer dispatches its PCM callbacks to the GCD main queue and a plain `NSRunLoop::runUntilDate` from a CLI cannot drain it. Background tokio tasks (classifier, extractor, comprehension, embedding) are then cooperatively scheduled on main and stall during synthesis — they catch up at the start of the next turn via `await_pending_post_response`, which is exactly when the dialogue manager already expects them, so user-visible behaviour is unchanged.

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

The LLM produces markdown — `*emphasis*`, `**strong**`, `` `code` `` — and Piper has no idea what to do with asterisks or backticks. They come out as literal "asterisk asterisk" or get phonemised into something garbled. So before pushing to the TTS backend, the voice loop runs the response through [`strip_markdown_for_tts`](../../src/crates/primer-speech/src/voice_loop/state_machine.rs), which removes paired markers and leaves bare unmatched ones alone:

| Input | Stripped |
|---|---|
| `*why*` | `why` |
| `**important**` | `important` |
| `` `code` `` | `code` |
| `a* footnote` | `a* footnote` (bare unmatched stays) |
| `5*3=15` | `5 times 3=15` (digit-flanked `*` → multiplication) |
| `5**2` | `5 times 2` (digit-flanked `**` → exponent) |
| `value *= 5` | `value *= 5` (operator stays) |

The digit-flanking rule reads multiplication and exponent notation aloud naturally — without it, "5*3" would be voiced "five star three" or worse. The visible transcript retains the original markdown unchanged; only the TTS path strips. The complete coverage is in the [unit tests in state_machine.rs](../../src/crates/primer-speech/src/voice_loop/state_machine.rs).

## The dedicated audio-capture thread

The audio capture path runs on its own `std::thread`, **not** a tokio task. It owns the silero VAD instance, the active whisper streaming session, and the mic-side resampler. It emits two kinds of output:

- `VadEvent`s (`SpeechStart`, `SpeechEnd`, `None`) over a `tokio::sync::mpsc` channel that `run_loop` consumes.
- On `SpeechEnd`, the finalised transcript over a separate channel that `run_loop` reads via a [`ChannelStt`](../../src/crates/primer-speech/src/voice_loop/state_machine.rs) wrapper — a thin adapter that implements `StreamingSpeechToText` against the channel so `run_loop` can treat audio capture as a regular trait object.

This split keeps test/production cleanly separated: mocks bypass `cpal` entirely by injecting events and transcripts directly into those channels. The whole audio stack (cpal, ringbufs, rubato resampler, silero, whisper) is dark to unit tests, which is what makes the [voice_loop state-machine tests](../../src/crates/primer-speech/src/voice_loop/state_machine.rs) tractable.

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

At startup, a `probe_espeak_ng_data` function walks three candidate locations and sets the `PIPER_ESPEAKNG_DATA_DIRECTORY` environment variable to the parent of the first complete `espeak-ng-data` directory it finds. There are two byte-identical copies (differing only in logging): the CLI's in [primer-cli/src/main.rs](../../src/crates/primer-cli/src/main.rs) (runs before the tokio runtime starts; logs to stderr) and the GUI's in [primer-gui/src/lib.rs](../../src/crates/primer-gui/src/lib.rs) (runs before the Tauri builder spawns workers; logs via `tracing::info!`). When refactoring, move the shared implementation into `primer-speech` rather than letting the two diverge. The candidate locations:

```
/opt/homebrew/share   # macOS Apple Silicon (brew install espeak-ng)
/usr/local/share      # macOS Intel / generic
/usr/share            # Linux (apt/dnf)
```

It checks for the presence of `espeak-ng-data/phontab` rather than just the directory existence, because `espeak-rs`'s incomplete subset would otherwise satisfy a directory-only probe and leave you with phoneme failures at runtime. If `PIPER_ESPEAKNG_DATA_DIRECTORY` is already set when the probe runs, the probe respects it.

> **Gotcha:** Voice id must be passed explicitly. Pass `--voice <model-id>` matching the file stem of `--voice-onnx` (e.g. `--voice en_GB-alba-medium` for `en_GB-alba-medium.onnx`). Before this fix, `run_loop` hardcoded `VoiceProfile::default()` (`en_US-amy-medium`) as the voice profile passed to Piper's `open_session`, and Piper rejected any non-default voice with a model-id mismatch error. The CLI now also validates that `--voice` matches the `--voice-onnx` filename stem and errors at parse time if not.

> **Gotcha:** Build with rustup, not Homebrew rust. This was already mentioned in [chapter 1](01-getting-started.md), but it bears repeating here because speech is where the bug surfaces hardest. [rust-toolchain.toml](../../src/rust-toolchain.toml) pins Rust 1.87+, but that pin is honoured only by rustup's proxy binaries. If a Homebrew-installed `cargo` shadows on `PATH`, Cargo silently falls back to whatever Homebrew has (often 1.86), and silero will fail to compile with a confusing trait-resolution error. Always invoke as `~/.cargo/bin/cargo` for any speech-feature build.

> **Gotcha:** Voice rate is `0.9` (length_scale ≈ 1.111), not 1.0. Each phrase is followed by 200 ms of injected silence before the next chunk is pushed to cpal. Both numbers were tuned against the target audience: too fast and a 7-year-old loses the thread; too breathless and the pacing feels artificial. Adjust [`VoiceProfile.rate`](../../src/crates/primer-core/src/speech.rs) and the inter-chunk sleep in [voice_loop/state_machine.rs](../../src/crates/primer-speech/src/voice_loop/state_machine.rs) if needed, but treat it as a UX call, not a code call.

## GUI voice-mode asset auto-download

The CLI takes explicit asset paths (`--whisper-model`, `--voice-onnx`, `--voice-config`, `--supertonic-dir`, …) and never downloads anything. The GUI, by contrast, can auto-download voice assets on first use, consent-gated. Asset cache lives under `~/.cache/primer/models/` with per-locale sub-dirs `voice/<locale>/` and `whisper/` for Piper/Whisper, and a **locale-independent** root `supertonic/` (`onnx/` + `voice_styles/`) for Supertonic — Supertonic is one multilingual model serving every locale, so it does not use the per-locale layout.

First-run is consent-gated via the `download_voice_assets` Tauri command + a consent modal driven by [ui/voice.js](../../src/crates/primer-gui/ui/voice.js). The IPC takes only `kinds: Vec<String>` (e.g. `"piper_onnx"`, `"whisper_model"`, the seven `supertonic_*` kinds) — never paths or URLs; the server re-resolves the destination + canonical URL itself, keeping the trust boundary tight (a compromised webview cannot direct the host to write outside the cache or fetch from a non-canonical URL). Downloads are hardened for timeout (default 30 min), resume (`<dest>.partial` + `Range` requests), and oversize (capped at 150 % of the declared size). The Supertonic bundle is modelled as **7 single-file `kind`s** so the existing single-file resume/oversize infra is reused unchanged for its two large files.

> **Gotcha:** `SpeechSettings.disable_auto_download` in `gui-config.json` is now actually enforced for every backend (it was a stored-but-ignored no-op before Supertonic Stage D). When set, a missing asset routes to an informational "add paths in Settings → Speech" banner (`StartVoiceModeError::AutoDownloadDisabled`) with no Download button, honouring [[project_strict_offline_first]].

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

**5. Wire into the voice-loop selectors.** The voice loop selects backends by trait object via the two shared helpers in [voice_loop/selectors.rs](../../src/crates/primer-speech/src/voice_loop/selectors.rs): `build_tts(TtsBackend, …)` for synthesisers and `build_voice_backends(SttBackend, …)` for STT skeletons. Add a feature-gated arm to the relevant enum and helper, returning a `PrimerError::Speech` "rebuild with `--features X`" hint on the uncompiled-feature path (mirror the existing arms). Then thread the new choice through the CLI flag (`--tts` / the STT selection) and the GUI's mirror `config::TtsBackend` / `config::SttBackend` enums. Keep the construction error path graceful — a missing model file should produce a clear error message naming the flag and the file, not a panic.

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
