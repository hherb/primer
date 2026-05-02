# Streaming TTS trait + Piper backend

**Status:** approved design, ready for implementation plan
**Phase:** 2 (third slice of the speech pipeline; follows VAD + streaming STT in PR #3)
**Author:** brainstorming session, 2026-05-02
**Source brief:** `docs/primer_TTS_next_step.md`

## Goal

Add a streaming text-to-speech trait + a Piper backend so the Primer can begin emitting audio before the LLM has finished generating a response. End-to-end target (reproducing the brief): <150 ms from first LLM token to first PCM sample on a Pi-class device. The trait shape closes a known papercut from PR #3 — backends that implement both the one-shot and streaming variants of a speech trait having to write `name()` twice — by extracting a `Named` super-trait that all five speech traits inherit from.

## Scope

**In scope**

- New `Named` super-trait in `primer-core::speech`. All existing speech traits (`VoiceActivityDetector`, `SpeechToText`, `StreamingSpeechToText`, `TextToSpeech`) gain a `: Named` bound; the inline `fn name(&self) -> &str` is removed from each of them and rehomed onto a single `impl Named for X` per backend. Mechanical refactor across `silero.rs`, `whisper.rs`, `stub.rs`, and the in-file test mocks.
- New `StreamingTextToSpeech: Named + Send + Sync` trait + `SynthesisSession: Send` + `AudioChunk` value type in `primer-core::speech`. Lifecycle mirrors `StreamingSpeechToText` / `TranscriptionSession`: `open_session(&VoiceProfile) -> Box<dyn SynthesisSession>`, `push_text(&mut self, &str) -> Result<Vec<AudioChunk>>`, `finalize(self: Box<Self>) -> Result<Vec<AudioChunk>>`.
- New pure helper module `primer-speech::phrase_split` — `PhraseSplitter` with `push(&mut self, &str) -> Vec<String>` and `flush(&mut self) -> Option<String>`. Zero backend deps. Same shape as `vad_debounce`.
- New `primer-speech::piper` module behind a `piper` Cargo feature. `PiperTts` impls `Named + TextToSpeech + StreamingTextToSpeech`. Wraps `piper-rs 0.1.x` (`thewh1teagle/piper-rs`).
- `StubTts` extension: `impl Named for StubTts` + `impl StreamingTextToSpeech for StubTts` so the new trait is exercised through dyn dispatch in tests without any backend feature flag.
- Smoke binary `src/crates/primer-speech/examples/tts_hello.rs`: takes `--onnx` / `--config` / `--out`, synthesises the brief's smoke phrase, writes a 16-bit PCM WAV via `hound` (added as a dev-dependency).

**Out of scope (deliberately deferred)**

- Real-time NPU offload of the Piper decoder (RKNN/QNN) — Phase 2.5+, after the CPU pipeline closes the loop.
- Audio capture / playback (cpal) wiring — separate cross-cutting concern.
- The unified `--speech` REPL that wires VAD → STT → DialogueManager → TTS — that's step 4. This slice does **not** modify `primer-cli`.
- Voice training. Step 4 picks a voice at startup; Phase 2.5 is the right time to consider a child-voice fine-tune.
- Runtime voice switching inside one `PiperTts`. The trait shape allows it (`open_session` takes a `VoiceProfile`); this impl errors on a mismatch with its constructor-time voice. A future backend (e.g. a multi-voice cache) can implement runtime switching without changing the trait.
- Multi-speaker selection beyond a single optional `speaker_id` builder. Most candidate voices are single-speaker; multi-speaker selection beyond `with_speaker_id` is future work.
- `pitch` honouring on the Piper backend — `piper-rs` 0.1.x has no pitch knob. Pitch lives on the trait for future backends; on Piper it logs a one-shot `tracing::warn!` on first non-zero value and is otherwise a no-op.

## Key decisions

### `Named` super-trait — extracted

All five speech traits share `: Named`. Each backend writes `impl Named for X { fn name(&self) -> &str }` exactly once even when it implements both the one-shot and the streaming variant. Cost is mechanical edits across `silero.rs`, `whisper.rs`, `stub.rs`, plus two in-file test mocks (`CannedVad`, `CannedStreamStt`). Benefit is a permanent fix to the dual-trait UFCS papercut PR #3 flagged. The brief explicitly authorised "fix or don't-fix the whole speech-trait family at once"; this is the fix half.

### Phrase-splitting is the streaming mechanism

`piper-rs::Piper::create(...)` is one-shot and synchronous; the crate exposes no phrase-boundary callback. Real streaming is achieved by chunking text on `. ! ?` and synthesising each completed phrase. First audio chunk lands ~one phrase after the LLM emits that phrase, not after the whole utterance — this is what the brief's <150 ms latency budget needs.

The phrase-splitter is a pure-Rust helper module mirroring `vad_debounce`: a small struct, named consts, fully unit-tested without any backend dep, then reused by `PiperTts`'s session.

**Boundary rule.** A phrase ends at byte position `i` when **all** of:

1. Char at `i` is in `PHRASE_TERMINATORS = ['.', '!', '?']`.
2. There exists a char at `i+1` (else: not yet a boundary; wait for more text — this is the "lookahead-by-one" rule that prevents premature splits).
3. That next char is whitespace.
4. **Abbreviation guard:** if the terminator is `.` and the word ending immediately before `i` (lowercased, ASCII-alphabetic) is in `ABBREVIATIONS`, no boundary.
5. **Decimal guard** is implied by rule 3 — `3.1` never qualifies because `1` isn't whitespace.
6. **Ellipsis collapse:** a run of `.` characters is treated as one terminator.

```rust
const PHRASE_TERMINATORS: &[char] = &['.', '!', '?'];

/// ASCII-lowercase abbreviations that should NOT be treated as phrase
/// boundaries. Conservative starting list — extend with evidence.
const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "prof",
    "sr", "jr", "st",
    "vs", "etc", "ie", "eg",
    "us", "uk",
];
```

Iteration uses `char_indices()` (not byte indexing) so non-ASCII glyphs in children's names or content can't trigger a UTF-8 codepoint-boundary panic.

### One voice per `PiperTts` instance

`piper-rs::Piper` binds one model file at construction. `PiperTts::new(onnx_path, config_path)` loads one voice. `StreamingTextToSpeech::open_session(&VoiceProfile)` honours the trait shape but errors with `PrimerError::Speech("piper voice mismatch: backend loaded {x}, session asked for {y}")` if `voice.model_id` doesn't match the constructor-time voice's id. Multiple voices = multiple `PiperTts` instances. The trait stays correct for a future multi-voice backend; this impl just doesn't implement runtime switching yet.

This matches the brief's "voice selection is for development testing only" guidance and avoids a speculative LRU-cache design.

### Sample rate is per-voice, read at construction

Piper voice configs each declare their own sample rate (`audio.sample_rate` in the JSON sidecar). `PiperTts` reads it at construction — preferring `piper_rs::Piper`'s accessor if one exists, else parsing the config JSON via `serde_json` — and caches it. `StreamingTextToSpeech::sample_rate(&self)` returns the cached value with no I/O. Each emitted `AudioChunk` carries the same rate so downstream sinks don't need to hold a backend reference.

### `VoiceProfile` knob mapping

- `model_id` — validated; mismatch errors as above.
- `rate` — `length_scale = 1.0 / rate` on `piper.create(...)`. The inversion lives in a single named const so the relationship is explicit. (`rate > 1.0` ⇒ faster ⇒ smaller `length_scale`.)
- `pitch` — ignored on Piper (no upstream knob in 0.1.x). One `tracing::warn!` on the first non-zero value per session; the doc comment notes this is reserved for a future backend.
- `speaker_id` — separate from `VoiceProfile`. `PiperTts` exposes a `with_speaker_id(i64)` builder; default is `None`. Multi-speaker discrimination beyond this is future work.

### Backend-only PR + example binary

This slice does not modify `primer-cli`. The smoke test runs through `cargo run --example tts_hello --features piper`. Step 4 (the unified speech REPL) is where `--voice` lands on the real binary. Half-implemented CLI flags rot; deferring is cheaper than back-filling.

## Module layout

```
src/crates/primer-core/src/speech.rs
  + pub trait Named { fn name(&self) -> &str; }
  + pub trait StreamingTextToSpeech: Named + Send + Sync { … }
  + pub struct AudioChunk { samples: Vec<f32>, sample_rate: u32 }
  + pub trait SynthesisSession: Send { … }
  ~ existing traits gain `: Named` super-trait, drop the inline name()
  ~ existing test mocks (CannedVad, CannedStreamStt) get `Named` impls

src/crates/primer-speech/src/phrase_split.rs        # NEW pure helper
src/crates/primer-speech/src/piper.rs               # NEW, behind `piper` feature
src/crates/primer-speech/src/lib.rs
  + pub mod phrase_split; pub use phrase_split::PhraseSplitter;
  + #[cfg(feature = "piper")] pub mod piper;
  + #[cfg(feature = "piper")] pub use piper::PiperTts;

src/crates/primer-speech/Cargo.toml
  + [dependencies]
  +   piper-rs = { version = "0.1", default-features = false, optional = true }
  + [dev-dependencies]
  +   hound = "3"                                  # example WAV writer only
  + [features]
  +   piper = ["dep:piper-rs"]
  + [[example]]
  +   name = "tts_hello"
  +   required-features = ["piper"]

src/crates/primer-speech/examples/tts_hello.rs      # NEW smoke binary

src/crates/primer-speech/src/stub.rs
  ~ impl Named for StubTts
  + impl StreamingTextToSpeech for StubTts          # zero-sample chunks via PhraseSplitter
```

No changes to `primer-pedagogy`, `primer-cli`, `primer-knowledge`, `primer-storage`, `primer-classifier`, or `primer-inference`. The `Named` super-trait refactor is contained to `primer-core::speech` and `primer-speech`.

## Trait shape (canonical)

```rust
/// Common identifier for any speech backend. Extracted as a super-trait so a
/// single struct that implements both the one-shot and streaming variants of
/// STT or TTS only writes its `name()` once.
pub trait Named {
    fn name(&self) -> &str;
}

pub trait VoiceActivityDetector: Named + Send { /* unchanged otherwise */ }
#[async_trait] pub trait SpeechToText: Named + Send + Sync { /* unchanged */ }
pub trait StreamingSpeechToText: Named + Send + Sync { /* unchanged */ }
#[async_trait] pub trait TextToSpeech: Named + Send + Sync { /* unchanged */ }

/// One PCM chunk emitted by a [`SynthesisSession`] during streaming.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

/// A single streaming-synthesis session.
pub trait SynthesisSession: Send {
    fn push_text(&mut self, text: &str) -> Result<Vec<AudioChunk>>;
    fn finalize(self: Box<Self>) -> Result<Vec<AudioChunk>>;
}

/// Streaming text-to-speech backend. Open one session per Primer turn.
pub trait StreamingTextToSpeech: Named + Send + Sync {
    fn sample_rate(&self) -> u32;
    fn open_session(&self, voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>>;
}
```

## `PiperTts` impl sketch

```rust
const BACKEND_NAME: &str = "piper";
/// Default `length_scale` (piper-rs term for "normal pace"). VoiceProfile.rate
/// inverts: rate > 1.0 ⇒ faster ⇒ smaller length_scale = 1.0 / rate.
const DEFAULT_LENGTH_SCALE: f32 = 1.0;

pub struct PiperTts {
    piper: Arc<piper_rs::Piper>,
    voice: VoiceProfile,
    speaker_id: Option<i64>,
    sample_rate: u32,
}

impl PiperTts {
    pub fn new(onnx_path: impl AsRef<Path>, config_path: impl AsRef<Path>) -> Result<Self>;
    pub fn with_voice(self, voice: VoiceProfile) -> Self;
    pub fn with_speaker_id(self, id: i64) -> Self;
}

impl Named for PiperTts { /* "piper" */ }

#[async_trait]
impl TextToSpeech for PiperTts {
    async fn synthesize(&self, text: &str, voice: &VoiceProfile) -> Result<AudioBuffer> {
        // validate voice.model_id; spawn_blocking → piper.create(...) → AudioBuffer.
    }
}

impl StreamingTextToSpeech for PiperTts {
    fn sample_rate(&self) -> u32 { self.sample_rate }
    fn open_session(&self, voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>> {
        // validate voice.model_id; build PiperSession with PhraseSplitter::new().
    }
}

struct PiperSession {
    piper: Arc<piper_rs::Piper>,
    splitter: PhraseSplitter,
    length_scale: f32,
    speaker_id: Option<i64>,
    sample_rate: u32,
}

impl SynthesisSession for PiperSession {
    fn push_text(&mut self, text: &str) -> Result<Vec<AudioChunk>> {
        // splitter.push(text) → Vec<String>; for each phrase call
        // piper.create(&phrase, false, speaker_id, length_scale, None, None) → (Vec<f32>, u32);
        // build AudioChunk per phrase. Synchronous; runs on caller's thread.
    }
    fn finalize(mut self: Box<Self>) -> Result<Vec<AudioChunk>> {
        // splitter.flush() → Option<String>; synthesise if Some.
    }
}
```

Async vs. sync: `TextToSpeech` keeps its existing `#[async_trait]` shape; the impl wraps `piper.create` in `tokio::task::spawn_blocking` (mirrors `WhisperStt::transcribe`). `StreamingTextToSpeech` is sync (matches `StreamingSpeechToText`); per-phrase `create` runs on the caller's thread because each phrase is short and the streaming-STT pattern is sync.

## Tests

| Where | What | Count | CI? |
|---|---|---|---|
| `primer-core::speech::tests` | `streaming_tts_session_yields_chunks_and_finalizes` (mirrors the streaming-STT mock test) | +1 | yes |
| `primer-core::speech::tests` | `named_super_trait_resolves_via_each_speech_trait` (one assertion per `: Named` trait) | +1 | yes |
| `primer-speech::phrase_split::tests` | 10 cases — empty input; two-sentence happy path; decimal guard; abbreviation guard; ellipsis collapse; mid-token push doesn't eagerly split; flush drains pending; flush returns None on empty/whitespace; non-ASCII content survives without panic; exclamation + question split | +10 | yes |
| `primer-speech::stub::tests` (new mod) | `stub_tts_streaming_emits_chunk_per_phrase`; `stub_tts_streaming_finalize_drains_trailing` | +2 | yes |
| `primer-speech::piper::tests` | `#[cfg(feature = "piper")]` + `#[ignore]`-by-default smoke loading a real model from `$PIPER_TEST_MODEL`; skipped when env unset | +1 (skipped) | no |

Net: **+14 unconditional** tests (208 → 222 baseline, give or take refactor-touched mocks). The Named refactor itself should not change any test counts on its own.

## Build prerequisites

- `cargo build --workspace` (no features) **must remain clean** — `piper-rs` is opt-in via `default = []`. Default workspace build pulls no new deps.
- `cargo build -p primer-speech --features piper` pulls `piper-rs 0.1.x` and its transitives (`ort`, `espeak-rs`, `riff-wave`, `rayon`).
- **`ort` rc compatibility:** if `piper-rs` and `silero-vad-rust 6.2` disagree on the `ort` rc when both `silero` and `piper` features are enabled, `cargo tree -i ndarray` will show two ndarray versions. Resolve by pinning both via the workspace `[workspace.dependencies]` block (the existing `=2.0.0-rc.10` for `ort` is the precedent). Document in the PR if a clean resolution isn't possible.
- ONNX Runtime first-build downloads from `cdn.pyke.io`. Sandboxed CI environments (including the one this branch was developed in) block that host. Document, don't fight.
- `hound` lives in `[dev-dependencies]` and the `tts_hello` example is gated `required-features = ["piper"]`. Default `cargo build --workspace` pulls neither `hound` nor `piper-rs`.

## Smoke-test binary

`src/crates/primer-speech/examples/tts_hello.rs`:

```
cargo run --example tts_hello --features piper -- \
  --onnx /path/to/en_US-amy-medium.onnx \
  --config /path/to/en_US-amy-medium.onnx.json \
  --out hello.wav
```

- Synthesises `"Hello, what would you like to learn about today?"` (the brief's smoke phrase).
- Drives the **streaming** path: opens a session, `push_text(SMOKE_PHRASE)`, `finalize()`, concatenates emitted `AudioChunk`s. Proves the streaming path produces the same audio as `TextToSpeech::synthesize` would.
- Writes 16-bit PCM WAV via `hound` at `PiperTts::sample_rate()`.
- Prints elapsed wall-clock and total sample count to stdout.
- `clap` (workspace dep) for arg parsing in the existing `derive` style.

## Final check sequence (per the source brief)

From `src/`:

- `cargo build --workspace`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets`
- `cargo fmt --check`
- (manually, with model downloaded) `cargo run --example tts_hello --features piper -- …` and listen.

PR title: `feat(speech): streaming TTS trait + Piper impl` (per the brief).

After merge: refresh or delete `docs/primer_TTS_next_step.md` per its own "When you're done" item — refresh if step 4 (the unified VAD→STT→DialogueManager→TTS REPL) would benefit from notes about what we learned, delete if everything closed cleanly.
