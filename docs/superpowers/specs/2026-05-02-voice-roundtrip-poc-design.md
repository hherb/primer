# Voice round-trip POC — `--speech` mode

**Status:** approved design, ready for implementation plan
**Phase:** 2 (step 4 of the speech pipeline — closes the voice loop)
**Author:** brainstorming session, 2026-05-02
**Source briefs:** `docs/primer_TTS_next_step.md`, `primer_next_session.md`
**Supersedes step 4 of the TTS brief** (which intentionally deferred the spec)

## Goal

A `--speech` mode on `primer-cli` that runs the Socratic dialogue end-to-end as a voice conversation on the user's Mac: built-in mic → silero VAD → whisper STT → existing `DialogueManager` → Piper TTS → built-in speaker. Demonstrates that every speech trait already declared by the codebase actually composes into a real conversation, without modifying the existing text REPL or pedagogy code.

The pipeline is **fully streaming**: the child hears the first sentence of the Primer's reply before the LLM has finished generating. Mid-utterance pauses by the child do **not** trigger the Primer to start speaking — the loop is built so the Primer never barges in, even though it's "thinking with its mouth shut" the entire time the child might still be talking.

## Scope

### In scope

- New `--speech` flag on `primer-cli`, behind a new `speech` Cargo feature on the binary crate (so the default workspace build stays light — no cpal in the dep tree).
- New flags: `--whisper-model <PATH>`, `--voice-onnx <PATH>`, `--voice-config <PATH>`, `--voice <ID>` (default `en_GB-alba-medium`), `--mic-silence-ms <MS>` (default `600`).
- New `cpal` feature on `primer-speech` adding `cpal_io::MicCapture` and `cpal_io::SpeakerSink` (thin wrappers around cpal default in / default out, with `rubato` resampling).
- New `primer-cli/src/speech_loop.rs` orchestrator with a four-state machine (`LISTEN → LATENT_THINK → SPEAK → LISTEN`, plus `EXIT`).
- Toolchain bump to **Rust 1.87+** via `rust-toolchain.toml` so the existing `silero` feature compiles (`silero-vad-rust 6.2.1` calls `u32::is_multiple_of`, stable since 1.87).
- Quit by Ctrl+C **or** any of `goodbye` / `bye primer` / `stop primer` (case-insensitive substring match on the transcript).
- Terminal echoes `[child] <transcript>` and `[primer] <reply>` on stdout while audio plays. `--verbose` adds `[vad]`, `[stt]`, `[tts]` lines on stderr.
- Mock-driven integration tests for the state machine covering the commit, abort-on-resumed-speech, and natural-completion paths.

### Out of scope (deliberately deferred — see "Future work")

- Free-form barge-in. No-barge-in is a [pedagogical decision](../../primer_next_session.md), not a POC shortcut. An emergency-stop gesture is future work.
- Wake-word activation. Strict offline-first rules out Picovoice; `openWakeWord` Python or a custom small ONNX would be the alternatives. Mic is opened only when `--speech` is running.
- `--mic` / `--speaker` device-selection flags and `--list-devices`. cpal default in / default out only.
- Auto-download of whisper / piper assets. Missing files produce a clear error pointing at HuggingFace.
- Multi-voice runtime switching (one `PiperTts` per process, like `tts_hello`).
- Speculative-commit `DialogueManager` API (the cleaner fix for the two-consecutive-child-turns artefact described below).
- Cancellation token plumbed through `DialogueManager` / `CloudBackend`. `JoinHandle::abort()` is sufficient for the POC.
- Response-length calibration to attention span (linked from the no-barge-in pedagogy memory).

## Key decisions

### Toolchain bump unblocks silero

`silero-vad-rust 6.2.1` uses `u32::is_multiple_of` (`unsigned_is_multiple_of` tracking issue #128101), stabilised in Rust 1.87 (April 2025). The workspace currently runs on rustc 1.86. A two-line `rust-toolchain.toml` pinning `1.87+` fixes the build without vendoring or patching upstream. This is the cleanest fix and the one the brief implicitly authorised ("expect to chase a different silero pin or upstream fix").

### Full streaming, not utterance-at-a-time

`DialogueManager::respond_to_streaming(input, FnMut(&str))` already exists. Each `&str` chunk is fed into a synthesis worker thread that owns a `PiperTts` `SynthesisSession`. Each emitted `AudioChunk` flows through a holding buffer into the speaker cpal output stream. Latency from "child stops speaking" to "first PCM sample plays" is dominated by whisper finalize + first LLM phrase + first piper phrase — single seconds, not "wait for full reply" multi-seconds.

### State machine: LATENT_THINK overlaps the LLM with continued listening

The naive LISTEN → THINK → SPEAK loop has a UX failure: a child pausing for 600 ms mid-thought trips VAD `SpeechEnd`, the Primer starts thinking, the child resumes, and the Primer barges in. That is exactly the asymmetry the no-barge-in pedagogy is meant to prevent.

The fix: **the mic stays open across LISTEN and LATENT_THINK.** It only closes at the *commit boundary* — the moment the first audio sample is queued for playback.

```
                ┌──── VAD SpeechStart cancels LLM ─────┐
                ▼                                       │
LISTEN ── VAD SpeechEnd ──▶ LATENT_THINK ── first AudioChunk ──▶ SPEAK ── ringbuf empty ──▶ LISTEN
  │       (mic stays open)   (mic stays open)          (mic CLOSES,         (mic reopens)
  │       LLM starts          VAD still watching        speaker plays)
  │       speculatively       LLM streaming text →
  │                           TTS synthesising →
  │                           samples held in
  │                           pre-speaker buffer)
  ▼
EXIT (Ctrl+C / "goodbye")
```

Two invariants:

1. **The Primer never speaks over the child.** During LATENT_THINK, if VAD fires `SpeechStart`, the in-flight LLM `JoinHandle` is `abort()`'d, the synthesis worker is dropped, the holding buffer is discarded. No audio reaches the speaker. The same whisper streaming session keeps accumulating, so the child's continuation stitches onto their first utterance with no transcript loss.
2. **The child never speaks over the Primer.** Once the first `AudioChunk` reaches the speaker ringbuf, the mic closes for the duration of SPEAK. "Learning to listen" is part of the educational role.

### Pre-speaker holding buffer is the keystone

Synthesis cannot push directly to the cpal speaker ringbuf, because if VAD fires while the first phrase is mid-synthesis, audio would already be playing. So `AudioChunk`s emitted by the synthesis worker land in `tokio::sync::mpsc::unbounded_channel<Vec<f32>>` (the "holding buffer"). The main task `select!`s over `(audio_chunk_rx.recv(), event_rx.recv(), &mut llm_task)`. Three outcomes: commit (first audio chunk wins), abort (`SpeechStart` wins), or natural completion with no audio (LLM task ends first — short whitespace-only reply). Once committed, all subsequent samples flow main-task → speaker ringbuf without further gating.

### Two-consecutive-child-turns artefact — accepted

`DialogueManager::respond_to_streaming` records the child turn before the LLM call. If LATENT_THINK aborts, the child's first utterance is persisted as a turn, then their continuation becomes a separate child turn. The next LLM call sees both — semantically fine, transcript slightly ugly. The clean fix is a speculative-commit DM API (don't `add_child_turn` until the caller signals commit), filed as future work.

### `min_silence_ms` lifted to 600 ms for `--speech`

silero's default of 300 ms is too aggressive given the cancel-on-resume safety net. 600 ms reduces false trips (and the LLM cost they incur) without hurting perceived response time. The cancel mechanism is the safety net, not the primary signal.

### cpal as a `primer-speech` feature, not a workspace dep

cpal is heavyweight (CoreAudio / ALSA / WASAPI bindings) and a default workspace build shouldn't pull it. New `cpal` feature on `primer-speech`; `primer-cli`'s `speech` feature pulls `primer-speech/{silero,whisper,piper,cpal}`.

### State machine lives in `primer-cli`, not `primer-speech`

It's CLI orchestration, not a reusable backend. If a future hardware integration test wants to drive the loop headlessly, we extract it then.

## Architecture

### Crate boundaries

| Module                                   | Crate           | Behind feature                                                  |
|------------------------------------------|-----------------|-----------------------------------------------------------------|
| `cpal_io::MicCapture`, `SpeakerSink`     | `primer-speech` | new `cpal` feature                                              |
| `cpal_io::Resampler` (rubato adapter)    | `primer-speech` | `cpal` feature                                                  |
| `speech_loop` (state machine)            | `primer-cli`    | new `speech` feature → pulls `primer-speech/{silero,whisper,piper,cpal}` |
| `--speech` and friends                   | `primer-cli`    | `speech` feature gates them in clap                             |

### Concurrency model

Five concurrent actors. Three are tokio tasks / std threads we spawn; two are cpal-managed audio callbacks. No shared mutable state — everything coordinates via channels.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                          MAIN STATE-MACHINE TASK (tokio)                    │
│   LISTEN / LATENT_THINK / SPEAK loop. Owns command_tx, event_rx,            │
│   text_tx, audio_chunk_rx, speaker_ringbuf_producer.                         │
└──┬──────────────▲────────────────▲─────────────────────────────┬────────────┘
   │ command_tx   │ event_rx       │ audio_chunk_rx              │ text_tx
   │ (Finalize,   │ (VadEvent)     │ (Vec<f32>)                  │ (String)
   │  Stop)       │                │                             │
   ▼              │                │                             ▼
┌───────────────────────────┐    ┌─────────────────────────────────────────┐
│   AUDIO CAPTURE TASK      │    │  SYNTHESIS WORKER (std::thread)         │
│   (tokio task)            │    │  Owns PiperTts SynthesisSession.        │
│   Owns:                   │    │  Reads text from text_rx.               │
│   - ringbuf consumer      │    │  Calls push_text → AudioChunks.         │
│   - rubato resampler      │    │  Sends chunk.samples on audio_chunk_tx. │
│   - SileroVad             │    │  On rx close: finalize(), drain, exit.  │
│   - WhisperStt session    │    └─────────────────────────────────────────┘
└──▲────────────────────────┘
   │ raw f32 samples (ringbuf SPSC, lock-free)
   │
┌──┴────────────────────────┐    ┌─────────────────────────────────────────┐
│   CPAL INPUT THREAD       │    │   CPAL OUTPUT THREAD                    │
│   (cpal-managed)          │    │   (cpal-managed)                        │
│   Callback: device→f32→   │    │   Callback: pull from speaker ringbuf,  │
│   ringbuf producer.       │    │   write to device. Underrun → silence.  │
└───────────────────────────┘    └──▲──────────────────────────────────────┘
                                    │ samples drained from holding buffer
                                    │ on commit; written by main task.
                                    │ (ringbuf SPSC, lock-free)
```

### Channel inventory

| Channel                     | Producer            | Consumer              | Type                                             | Why this type                                                        |
|-----------------------------|---------------------|-----------------------|--------------------------------------------------|----------------------------------------------------------------------|
| mic samples                 | cpal input callback | audio capture task    | `ringbuf::HeapRb<f32>`                           | cpal callback cannot block / allocate — must be lock-free SPSC.      |
| VAD events                  | audio capture task  | main task             | `tokio::sync::mpsc::unbounded_channel<VadEvent>` | Low rate; main needs `recv().await` for `select!`.                   |
| text chunks                 | main task           | synthesis worker      | `std::sync::mpsc<String>`                        | Worker is std thread; sync receiver. Drop sender = signal finalize.  |
| audio chunks (post-synth)   | synthesis worker    | main task             | `tokio::sync::mpsc::unbounded_channel<Vec<f32>>` | Main awaits in `select!`; sync producer side via `unbounded_send`.   |
| speaker samples             | main task           | cpal output callback  | `ringbuf::HeapRb<f32>`                           | Same constraint as mic ringbuf — output callback cannot block.       |
| capture-task commands       | main task           | audio capture task    | `tokio::sync::mpsc<CaptureCommand>`              | `Finalize { reply_tx: oneshot<Transcript> }`, `Stop`.                |

### Resampling

cpal mic on Mac: typically 48 kHz (sometimes 44.1 kHz), often stereo i16 or f32 → silero/whisper need 16 kHz mono f32. `rubato::FftFixedIn` (or polyphase decimator on integer ratios) handles input.

Piper output for `en_GB-alba-medium`: 22.05 kHz (read from voice config at construction; not hardcoded). cpal output stays at device default (typically 48 kHz). `rubato` handles output.

Configuring cpal directly to 16 kHz / 22.05 kHz is possible on macOS but unreliable across devices — resampling once on each side is the boring, portable choice.

### Cancellation semantics

Main task in LATENT_THINK:

```rust
tokio::select! {
    Some(VadEvent::SpeechStart) = event_rx.recv() => {
        llm_task.abort();
        drop(text_tx);          // signals synthesis worker to drain & exit
        drain(&mut audio_chunk_rx);  // discard any chunks that already arrived
        // back to LISTEN; whisper session still accumulating
    }
    Some(samples) = audio_chunk_rx.recv() => {
        // commit
        speaker_stream = open_speaker_stream(...);
        speaker_ringbuf_producer.push_slice(&samples);
        capture_command_tx.send(CaptureCommand::Finalize { reply_tx }).await?;
        let transcript = reply_tx.await?;
        // drain remaining chunks into speaker ringbuf as they arrive,
        // then transition to SPEAK
    }
    res = &mut llm_task => {
        // LLM finished with no audio yet (very short reply, only whitespace,
        // PhraseSplitter never tripped). Drain finalize() output of synthesis
        // worker; if any audio, treat as commit; else log and return to LISTEN.
    }
}
```

Cancellation by `JoinHandle::abort()` resolves to `JoinError::is_cancelled() == true` — main treats it silently at default log level, prints a `[vad] aborted` line under `--verbose`.

## CLI surface

| Flag                       | Type      | Default              | Notes                                                                                       |
|----------------------------|-----------|----------------------|---------------------------------------------------------------------------------------------|
| `--speech`                 | `bool`    | `false`              | Routes through the voice loop instead of the text REPL. Voice mode skips the spoken greeting. |
| `--whisper-model <PATH>`   | `PathBuf` | required if `--speech` | GGML/GGUF whisper file. Missing → error pointing at `huggingface.co/ggerganov/whisper.cpp`. |
| `--voice-onnx <PATH>`      | `PathBuf` | required if `--speech` | Piper voice `.onnx`. Missing → error pointing at `huggingface.co/rhasspy/piper-voices`.     |
| `--voice-config <PATH>`    | `PathBuf` | required if `--speech` | Matching `.onnx.json` sidecar.                                                              |
| `--voice <ID>`             | `String`  | `en_GB-alba-medium`  | `VoiceProfile.model_id` for `open_session`'s mismatch check. Doesn't auto-resolve paths.    |
| `--mic-silence-ms <MS>`    | `u32`     | `600`                | Override silero `min_silence_ms` for `--speech` (default 300 too aggressive given cancel-on-resume). |

`--speech` requires all three model paths via clap's `requires_all`. All three paths are existence-checked at startup before any backend loads. `--voice` vs `Path::file_stem(--voice-onnx)` mismatch logs a `tracing::warn!` (Piper's `open_session` rejects on actual `model_id` mismatch).

Every existing flag (`--backend`, `--model`, `--name`, `--age`, `--knowledge-db`, `--session-db`, `--resume`, `--no-persist`, `--api-key`, `--classifier-*`, `--verbose`) works unchanged. Persistence still writes `~/.primer/<slug>.db`.

Quit phrases live in a `QUIT_PHRASES: &[&str]` constant — `["goodbye", "bye primer", "stop primer"]`. Case-insensitive substring match on the transcript.

## Error handling

**Startup errors (fail loud, exit before opening any audio):**
- Missing model files → "<asset> not found at `<path>`. Download from <hf-url>."
- Whisper / Piper load failure → propagate underlying error prefixed with the failed path.
- cpal default device unavailable → surface verbatim ("no default input device" / "no default output device"). On macOS this surfaces missing mic permission for the terminal.
- Silero load failure → "Silero VAD failed to initialise. ONNX Runtime needs network on first build (downloads from cdn.pyke.io). Re-run after a successful `cargo build`."

**Mid-loop errors (recover and continue):**
- VAD `process_chunk` error → `tracing::warn!`, drop the chunk, continue.
- Whisper push/finalize error → `tracing::warn!`, treat utterance as empty, back to LISTEN.
- LLM error (cloud 429, ollama down, network drop) → `tracing::error!`, synthesise `"Sorry, I had trouble there. Could you ask again?"` via Piper, return to LISTEN.
- Piper synthesis error mid-stream → log, abandon SPEAK, return to LISTEN. Speaker stream drains on its own (cpal underrun = silence, no audible click).
- cpal stream error callback → log, transition to EXIT. A broken audio device is unrecoverable without re-init.

Cancellation (`JoinError::is_cancelled()`) is **not** an error — silent at default level, `[vad] aborted` under `--verbose`.

**No new `PrimerError` variants.** Audio I/O wraps cpal/rubato/ringbuf errors into `PrimerError::Speech` at the boundary. LLM errors keep `PrimerError::Inference`.

## Testing

**Required (no audio hardware, run on every CI):**
- `vad_debounce`, `phrase_split`: existing tests stay green.
- `cpal_io::Resampler` adapter unit tests (rubato is upstream-tested; we test the adapter — input format → mono f32 16 kHz, ratio math, odd channel counts).
- State-machine integration tests in `primer-cli` driven by mock backends:
  - `MockVad` emitting a scripted `VadEvent` sequence.
  - `MockStreamingStt` emitting a scripted transcript on finalize.
  - `StubBackend` (existing) for the LLM.
  - `MockStreamingTts` emitting a fixed `AudioChunk` per text chunk.
  - **Test 1 — happy path:** scripted SpeechEnd → LLM called with expected transcript → audio chunks reach speaker ringbuf → state returns to LISTEN.
  - **Test 2 — cancel on resumed speech:** SpeechEnd, then SpeechStart before any audio chunk → assert `JoinHandle::abort()` triggered, no samples reach speaker ringbuf, second utterance concatenated to transcript on next finalize.
  - **Test 3 — commit on first chunk:** synthesis fires before second SpeechStart → assert samples reach speaker ringbuf, mic closed, state in SPEAK.
  - **Test 4 — natural completion no audio:** LLM stream ends with whitespace-only output → loop back to LISTEN, no error.

**Hardware-tagged (manual / opt-in):**
- `#[ignore]`-annotated test in `primer-speech` running the full Piper round-trip against actual cpal output. `cargo test --features cpal -- --ignored` runs it. Never in CI.

**Manual smoke (documented in spec, not automated):**
- `cargo run --features speech --bin primer -- --speech --whisper-model <path> --voice-onnx <path> --voice-config <path>`. Speak a question, hear an answer, say "goodbye", verify session DB contains the turns.

**Coverage target:** every `select!` arm in the LATENT_THINK match must be exercised by at least one mock test. That's the part most likely to ship with a subtle race.

## File-by-file changes

### New files
- `src/rust-toolchain.toml` — pin Rust 1.87+.
- `src/crates/primer-speech/src/cpal_io.rs` — `MicCapture`, `SpeakerSink`, `Resampler` adapter.
- `src/crates/primer-cli/src/speech_loop.rs` — state machine + LATENT_THINK `select!`. `QUIT_PHRASES` constant + match helper live as a small private submodule inside this file. `MockVad`, `MockStreamingStt`, `MockStreamingTts` live inline in a `#[cfg(test)] mod mocks` at the bottom of the same file.

### Modified files
- `src/Cargo.toml` — add `cpal`, `rubato`, `ringbuf` to `[workspace.dependencies]`.
- `src/crates/primer-speech/Cargo.toml` — new `cpal` feature gating `cpal`, `rubato`, `ringbuf` deps.
- `src/crates/primer-speech/src/lib.rs` — `pub mod cpal_io;` behind `cpal` feature.
- `src/crates/primer-cli/Cargo.toml` — new `speech` feature pulling `primer-speech/{silero,whisper,piper,cpal}`.
- `src/crates/primer-cli/src/main.rs` — new flags (gated on `speech` feature), branch into `speech_loop::run` when `--speech` set.

### Deleted files
- None.

## Known limitations & future work

| Limitation | Resolution path |
|------------|------------------|
| Two-consecutive-child-turns artefact on cancel | Speculative-commit `DialogueManager` API. |
| `JoinHandle::abort()` doesn't gracefully cancel the underlying HTTP request to Anthropic | Cancellation token plumbed through `DialogueManager` / `CloudBackend`. |
| Mic device is whatever cpal picks as default | `--mic` / `--speaker` / `--list-devices` flags. |
| User must manually download whisper / piper assets | `primer-models` helper crate with checksums. |
| One voice per process | `PiperTtsRouter { HashMap<model_id, PiperTts> }`. |
| Primer's reply length doesn't adapt to child's attention span | New pedagogy work; signals from `LearnerModel` + classifier. |
| No emergency-stop gesture during SPEAK | Hardware button or held-modifier-key in a future iteration; never free-form barge-in. |
| No wake-word | `openWakeWord` Python or custom small ONNX (Picovoice ruled out by offline-first). |

## Reporting back

When implementation lands, the report should plainly state:
- Which acceptance criteria are met (manual smoke succeeds end-to-end, all four mock tests green, `cargo clippy --workspace --all-targets --all-features` clean).
- Whether any of the future-work items proved unavoidable (e.g. cancellation needed full plumbing after all).
- Whether the toolchain bump caused unexpected fallout in other crates.
- Whether silero on Rust 1.87 actually compiles cleanly or needs a follow-up patch.
