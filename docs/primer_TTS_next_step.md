# Primer — TTS Next-Step Brief

**Audience:** future Claude Code session continuing the Phase 2 speech pipeline.
**Last updated:** 2026-05-01 (after PR #3: VAD + streaming STT traits and impls).

This document scopes the **third slice** of the Phase 2 speech pipeline: a TTS trait extension and a Piper backend. PRs #1–#3 introduced the VAD and STT halves of the pipeline; this slice adds voice synthesis so the Primer can speak its responses.

## First moves when you start

1. Read [CLAUDE.md](../CLAUDE.md) — repo conventions and gotchas. **Workspace root is `src/`, not the repo root** — every cargo command runs from `src/`.
2. Read this document end-to-end. Then read [primer_next_session.md](../primer_next_session.md) only if you need broader context — for TTS work it should not be necessary.
3. Read the three speech-pipeline source files added in PR #3 to anchor the existing patterns:
   - `src/crates/primer-core/src/speech.rs` — trait definitions for `VoiceActivityDetector`, `SpeechToText`, `StreamingSpeechToText`, `TranscriptionSession`, and the existing `TextToSpeech`. **Add the new streaming TTS trait here, alongside its synchronous sibling.**
   - `src/crates/primer-speech/src/silero.rs` — reference implementation pattern (feature-gated, named constants, thin wrapper around the upstream crate, debouncer reuse).
   - `src/crates/primer-speech/src/whisper.rs` — reference for a backend that implements **both** the one-shot and the streaming trait variants.
4. From `src/`: `cargo build --workspace && cargo test --workspace`. Should be green: 136 tests after PR #3 merges.

## What's already on the branch (don't redo)

- `VoiceActivityDetector` trait (`primer-core::speech`) and `SileroVad` impl (`primer-speech::silero`, behind `silero` feature). Bundled ONNX weights via `silero-vad-rust 6.2`. `ort` pinned to `=2.0.0-rc.10` to avoid an upstream ndarray drift.
- `VadDebouncer` and `ms_to_chunks` (`primer-speech::vad_debounce`) — reusable, pure, fully unit-tested probability-to-event state machine. Use it from the TTS layer if streaming TTS needs an analogous debounce; reuse rather than duplicate.
- `StreamingSpeechToText` + `TranscriptionSession` traits (`primer-core::speech`) — session-based, `push_audio` may emit segments, `finalize(self: Box<Self>)` consumes the session and drains the trailing buffer. Naming and lifecycle conventions to mirror.
- `WhisperStt` impl (`primer-speech::whisper`, behind `whisper` feature) — implements both `SpeechToText` (one-shot, `spawn_blocking`) and `StreamingSpeechToText` (via `WhisperStream`). Single shared `Arc<WhisperContext>` serves many sessions.
- The existing `TextToSpeech` trait already exists in `primer-core::speech` with one-shot `synthesize(text, voice) -> AudioBuffer`. **Don't replace it — extend it.**

## Goal of this slice

Add a streaming-TTS trait + Piper implementation so the Primer can begin emitting audio before the LLM has finished generating a response. End-to-end target: <150 ms from first LLM token to first PCM sample on a Pi-class device (per the latency budget in the PR #3 description). Children's-voice quality and selection is in scope for the implementation but voice training is **not**.

## Suggested trait shape

Mirror the streaming-STT pattern. Open one synthesis session per Primer turn; push text, drain audio.

```rust
// In primer-core::speech, after the TextToSpeech trait.

/// One PCM chunk emitted by a [`SynthesisSession`] during streaming.
///
/// Emitted as soon as the underlying model has enough context to commit
/// audio (typically once a phrase boundary is reached). Concatenate the
/// `samples` of every chunk in order to reconstruct the full utterance.
pub struct AudioChunk {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

/// A single streaming-synthesis session.
///
/// Created by [`StreamingTextToSpeech::open_session`]. Push partial text
/// from the LLM via [`Self::push_text`]; each push may emit zero or more
/// chunks as soon as the synthesiser has enough context. Call
/// [`Self::finalize`] when the LLM stream has ended to drain the trailing
/// buffer. `Send` but not `Sync`: each Primer turn owns its own session.
pub trait SynthesisSession: Send {
    fn push_text(&mut self, text: &str) -> Result<Vec<AudioChunk>>;
    fn finalize(self: Box<Self>) -> Result<Vec<AudioChunk>>;
}

pub trait StreamingTextToSpeech: Send + Sync {
    fn name(&self) -> &str;
    fn sample_rate(&self) -> u32;
    fn open_session(&self, voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>>;
}
```

Notes on the shape:
- **Phrase-streaming, not phoneme-streaming.** Piper, like Whisper-class STT, naturally synthesises sentence-sized chunks. Don't promise per-character output the model can't deliver.
- **`open_session` takes the `VoiceProfile`**, not the constructor. A single backend should be able to serve different voices over its lifetime without re-loading the engine. Whether Piper actually requires per-voice model loads is an implementation detail — the trait shouldn't bake that in.
- **`AudioChunk` carries its own `sample_rate`** even though every chunk in a session shares one. Keeps the type usable in non-session contexts (e.g., a future hand-off to an audio sink that doesn't know which backend produced the chunk).

Decide with Horst before implementing: does the streaming trait deserve a `Named` super-trait, or is the duplicated `name()` method OK? `WhisperStt` impls both `SpeechToText::name` and `StreamingSpeechToText::name` today — it's an ergonomic snag for direct calls (forces UFCS) but invisible through trait objects. PR #3's review noted this; defer the decision to TTS so you fix or don't-fix the whole speech-trait family at once.

## Suggested implementation

Use `piper-rs` (the `thewh1teagle/piper-rs` crate). Behind a `piper` Cargo feature on `primer-speech`, mirroring `silero` and `whisper`:

```toml
piper-rs = { version = "...", default-features = false, optional = true }

[features]
piper = ["dep:piper-rs"]
```

Let the user supply the model path on construction (Piper voices are `*.onnx` + `*.json` pairs distributed via the rhasspy/piper-voices HF repo). Provide a `PiperTts::new(model_path)` constructor with a `with_voice(VoiceProfile)` builder, mirroring `WhisperStt::new` / `with_language`.

For step 3's deliverable, **a synchronous `TextToSpeech` impl is enough** — wrap Piper's synthesise call, return the `AudioBuffer`. Streaming is the harder piece because Piper's native API may not expose phrase-boundary callbacks; if it doesn't, the simplest "streaming" impl is to chunk the input text on sentence boundaries (`. ! ?`) before handing each chunk to the synchronous synthesiser. A pure `split_into_phrases(text: &str) -> Vec<&str>` helper goes alongside `vad_debounce` so it can be unit-tested without the feature. **Be deliberate** — phrase-splitting that mishandles abbreviations or numbers will produce robotic delivery, and an over-eager split adds latency.

If Piper's Rust binding doesn't support streaming at all in version N, it's acceptable to land the synchronous impl + the `StreamingTextToSpeech` trait + a "buffer until finalize, then synthesise" stub impl, with a TODO. Don't fake streaming — say it isn't there.

## Voice selection (separate concern)

Piper's child-voice options are limited; Phase 2.5 will likely mean training one. For step 3 just pick 2–3 candidate adult voices that sound friendly to children (suggested starting points from the Piper voice list: `en_US-amy-medium`, `en_GB-jenny_dioco-medium`, `en_GB-alba-medium` — listen to samples first). Wire them up so `cargo run --bin primer ... --voice <id>` switches between them. Defer fine-tuning to a later session.

## Required principles (don't relax these)

The repo's working principles, repeated here so you stay honest:

1. **No magic numbers.** Sample rate, default model path, phrase-split punctuation, sentence-boundary lookahead — every numeric or string literal that shapes behaviour gets a named const with a doc comment. Examples landed in PR #3 you can mimic: `const SAMPLE_RATE: u32 = 16_000;` (`silero.rs`, `whisper.rs`); `const DEFAULT_THRESHOLD: f32 = 0.5;` (`silero.rs`).
2. **Prefer pure functions in reusable modules.** If a piece of logic doesn't need the engine to run (phrase-splitting, sample-rate conversion, format clamping), put it in its own module and give it a unit test. The `vad_debounce` module is the template: pure logic, fully tested without the backend, reused by the impl.
3. **Inline documentation and unit tests are mandatory.** Every public item needs a doc comment that says *why*, not just *what*. Every pure helper needs at least one positive and one edge-case test. Trait shapes get a mock-impl test in `primer-core::speech::tests` that exercises the lifecycle through `Box<dyn ...>` (see `streaming_stt_session_yields_segments_and_finalizes` for the template).
4. **No `unwrap` in non-test code.** Wrap upstream errors with `PrimerError::Speech(format!("...: {e}"))`. Existing impls follow this; copy the pattern.
5. **Build-time deps must be opt-in.** Default `cargo build --workspace` must pull no additional deps. Feature-gate the Piper backend the way `silero` and `whisper` are gated.

## Build prerequisites

Piper uses ONNX Runtime (same dep tree story as Silero). Expect `piper-rs` to pull `ort`. Two known hazards from PR #3 to watch for:

- `ort` version-pin compatibility — if `piper-rs` and `silero-vad-rust` disagree on the rc, you may need to pin one to keep `ndarray` consistent across the dep graph. `cargo tree -i ndarray` is your friend.
- ONNX Runtime first-build downloads from `cdn.pyke.io`. Sandboxed CI environments (including the one this branch was developed in) block that host. Document, don't fight.

## Test plan

- `cargo test --workspace` green (no new failures, count goes up by ≥3).
- `cargo build -p primer-speech --features piper` succeeds on a real machine (probably not in CI).
- Integration test (manual, off CI): load a Piper voice, synthesise "Hello, what would you like to learn about today?", play through `cpal` or save to wav, verify it sounds like words and not noise.
- After step 3 lands, the next session connects VAD → STT → DialogueManager → TTS into a live REPL replacement (`primer-cli --speech`). That's step 4.

## Out of scope

- Audio capture and playback (cpal) wiring — that's a separate cross-cutting concern.
- The "single child voice" decision for the Primer's final form — voice selection now is for development testing only.
- NPU offload of the Piper decoder (RKNN/QNN) — Phase 2.5+, after the CPU pipeline closes the loop.
- Any STT or VAD work — those are done; if you're tempted to "while I'm here" them, resist. Land the TTS slice cleanly and merge.

## When you're done

1. Run the full check sequence from `src/`: `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `cargo fmt --check`.
2. Push the branch and open a PR titled `feat(speech): streaming TTS trait + Piper impl` against `main`.
3. In the PR body, link to this doc and call out anything you decided differently from the suggestions here (with reasoning).
4. Update or delete this doc — if step 3 closed everything cleanly, delete it; if you learned things about Piper or the trait shape that would help the next session (step 4: end-to-end voice loop), refresh it.
