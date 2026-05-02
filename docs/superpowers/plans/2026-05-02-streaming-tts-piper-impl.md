# Streaming TTS + Piper backend — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `StreamingTextToSpeech` trait to `primer-core` and a `PiperTts` backend in `primer-speech` so the Primer can begin emitting audio before the LLM has finished generating a response. Closes the dual-trait UFCS papercut from PR #3 by extracting a `Named` super-trait shared by every speech trait.

**Architecture:** New super-trait `Named` rehomes `name()` once per backend across the speech-trait family. New `StreamingTextToSpeech` + `SynthesisSession` + `AudioChunk` mirror the streaming-STT lifecycle from PR #3. Real "streaming" comes from a pure `phrase_split` helper module (same shape as `vad_debounce`) that chunks text on `. ! ?` and feeds each phrase to `piper-rs`'s synchronous `Piper::create`. One voice per `PiperTts` instance — runtime voice switching is deferred. PR is backend-only; CLI integration lands in the unified-speech-REPL slice.

**Tech Stack:** Rust 2024, `async_trait`, `tokio` (`spawn_blocking` for the async `TextToSpeech` impl), `piper-rs 0.1.x` (opt-in via `piper` feature), `serde_json` (parse Piper voice config), `hound` (dev-dep, WAV writer for the example binary), `clap` (example binary args).

**Spec:** [docs/superpowers/specs/2026-05-02-streaming-tts-piper-design.md](../specs/2026-05-02-streaming-tts-piper-design.md)
**Source brief:** [docs/primer_TTS_next_step.md](../../primer_TTS_next_step.md)

---

## File structure

### Created

| Path | Responsibility |
|---|---|
| `src/crates/primer-speech/src/phrase_split.rs` | Pure `PhraseSplitter` helper. Same shape as `vad_debounce`. |
| `src/crates/primer-speech/src/piper.rs` | `PiperTts` backend, behind `piper` feature. |
| `src/crates/primer-speech/src/piper_config.rs` | Pure helper: parse Piper voice JSON for `audio.sample_rate`. Behind `piper` feature so the `serde_json` dep stays optional. |
| `src/crates/primer-speech/examples/tts_hello.rs` | Smoke binary: synthesise the brief's phrase, write WAV. |

### Modified

| Path | Change |
|---|---|
| `src/crates/primer-core/src/speech.rs` | New `Named` super-trait; `: Named` on `VoiceActivityDetector` / `SpeechToText` / `StreamingSpeechToText` / `TextToSpeech`; drop inline `name()` requirement on each; `Named` impls on `CannedVad` + `CannedStreamStt` test mocks; add `AudioChunk`, `SynthesisSession`, `StreamingTextToSpeech`; add `streaming_tts_session_yields_chunks_and_finalizes` test mock + assertion. |
| `src/crates/primer-speech/Cargo.toml` | `[dependencies]`: `piper-rs = { version = "0.1", default-features = false, optional = true }`, `serde_json = { workspace = true, optional = true }`. `[dev-dependencies]`: `hound = "3"`, `clap = { workspace = true }`. `[features]`: `piper = ["dep:piper-rs", "dep:serde_json"]`. `[[example]] name = "tts_hello", required-features = ["piper"]`. |
| `src/crates/primer-speech/src/lib.rs` | `pub mod phrase_split; pub use phrase_split::PhraseSplitter;` and `#[cfg(feature = "piper")] pub mod piper_config; #[cfg(feature = "piper")] pub mod piper; #[cfg(feature = "piper")] pub use piper::PiperTts;` |
| `src/crates/primer-speech/src/stub.rs` | Move `name()` from `SpeechToText`/`TextToSpeech` impls into `Named` impls; new `impl StreamingTextToSpeech for StubTts` that uses `PhraseSplitter` and emits zero-sample chunks. |
| `src/crates/primer-speech/src/silero.rs` | Move `name()` from `VoiceActivityDetector` impl into a `Named` impl. |
| `src/crates/primer-speech/src/whisper.rs` | Replace the two `name()` impls (one on `SpeechToText`, one on `StreamingSpeechToText`) with a single `Named` impl. Drops the `BACKEND_NAME` const usage from each trait impl into the one `Named` impl. |
| `docs/primer_TTS_next_step.md` | Refresh per its own "When you're done" instructions (or delete if step 4 needs no carry-over notes). |
| `CLAUDE.md` | Document the `Named` super-trait, `StreamingTextToSpeech`, the `piper` feature, and the `tts_hello` example. |
| `ROADMAP.md` | If a Phase 2 line item exists for "Piper TTS / streaming voice", tick it. (Verify at impl time — only modify if the line is already there.) |

---

## Conventions

- All `cargo` commands run from `src/` (workspace root, not repo root).
- Working branch: `feature/streaming-tts-piper`, already created off `main`. The spec doc is already committed at `b19513e`.
- Test count baseline (counted at start of impl, after the spec commit): **208 unconditional tests** workspace-wide. Final target: **222 unconditional + 1 ignored real-model smoke**. Each task that adds tests notes the running total in its commit message.
- Commit after every task. Subjects use the existing repo convention (`feat:`, `test:`, `refactor:`, `docs:`). PR title at the end: `feat(speech): streaming TTS trait + Piper impl`.
- Tests live in `#[cfg(test)] mod tests { ... }` inside the same file as the code under test, mirroring the existing pattern.
- TDD discipline per task: write failing test → run → verify FAIL → implement → run → verify PASS → commit.
- After every task, sanity-check the default workspace build: `cargo build --workspace`. The default build must pull no `piper-rs` / `hound` / `serde_json` (the latter is only needed by `primer-speech` under the feature; other crates already use it).
- **No magic numbers** (named consts in `consts.rs`-shaped modules or at the top of the file with doc comments). **No `unwrap()` in non-test code** — wrap upstream errors via `PrimerError::Speech(format!("...: {e}"))`.

---

## Phase 1 — `Named` super-trait refactor

The refactor is mechanical but touches multiple files. Doing it first means subsequent phases can use `: Named` on the new trait declaration. There is no behaviour change; the canary test asserts trait dispatch through the new super-trait works.

### Task 1: Add `Named` super-trait + canary test

**Files:**
- Modify: `src/crates/primer-core/src/speech.rs`

- [ ] **Step 1: Write the failing test**

Append at the end of the existing `#[cfg(test)] mod tests` in `src/crates/primer-core/src/speech.rs`, right after `streaming_stt_session_yields_segments_and_finalizes`:

```rust
    /// Canary that the `Named` super-trait is the single source of `name()`
    /// across every speech trait. If this fails to compile, the refactor
    /// regressed and a backend is again declaring `name()` on a leaf trait.
    #[test]
    fn named_super_trait_resolves_via_each_speech_trait() {
        // VAD: CannedVad
        let vad: Box<dyn VoiceActivityDetector> = Box::new(CannedVad::new(vec![]));
        assert_eq!(Named::name(&*vad), "canned");

        // Streaming STT: CannedStreamStt
        let stt: Box<dyn StreamingSpeechToText> = Box::new(CannedStreamStt);
        assert_eq!(Named::name(&*stt), "canned-stream-stt");
    }
```

- [ ] **Step 2: Run test to verify it fails (compile error — `Named` doesn't exist)**

```bash
cd src && cargo test -p primer-core named_super_trait_resolves_via_each_speech_trait
```

Expected: compile error `cannot find trait Named in this scope`.

- [ ] **Step 3: Add the `Named` super-trait**

In `src/crates/primer-core/src/speech.rs`, add immediately after the `VoiceProfile` block (before the existing `VadEvent` enum, ~line 56):

```rust
/// Common identifier for any speech backend.
///
/// Every speech trait inherits from `Named` so a single struct that
/// implements both the one-shot and streaming variants of STT or TTS
/// only writes its `name()` impl once. Removing this would re-introduce
/// the dual-trait UFCS snag PR #3 had to live with.
pub trait Named {
    fn name(&self) -> &str;
}
```

- [ ] **Step 4: Add `: Named` to existing speech traits and remove their inline `name()`**

Edit four trait declarations in the same file:

```rust
// Around line 87:
pub trait VoiceActivityDetector: Named + Send {
    // Remove: fn name(&self) -> &str;
    fn sample_rate(&self) -> u32;
    // … rest unchanged
}

// Around line 115:
#[async_trait]
pub trait SpeechToText: Named + Send + Sync {
    // Remove: fn name(&self) -> &str;
    async fn transcribe(&self, audio: &AudioBuffer) -> Result<Transcript>;
}

// Around line 168:
pub trait StreamingSpeechToText: Named + Send + Sync {
    // Remove: fn name(&self) -> &str;
    fn sample_rate(&self) -> u32;
    fn open_session(&self) -> Result<Box<dyn TranscriptionSession>>;
}

// Around line 185:
#[async_trait]
pub trait TextToSpeech: Named + Send + Sync {
    // Remove: fn name(&self) -> &str;
    async fn synthesize(&self, text: &str, voice: &VoiceProfile) -> Result<AudioBuffer>;
}
```

- [ ] **Step 5: Add `Named` impls to in-file test mocks**

In the `#[cfg(test)] mod tests` block of `speech.rs`, edit `CannedVad`'s impl (around line 220) — remove the `fn name(&self) -> &str { "canned" }` from inside `impl VoiceActivityDetector for CannedVad`, and add:

```rust
    impl Named for CannedVad {
        fn name(&self) -> &str { "canned" }
    }
```

Same for `CannedStreamStt` (around line 303): remove the `fn name(&self) -> &str { "canned-stream-stt" }` from `impl StreamingSpeechToText for CannedStreamStt`, and add:

```rust
    impl Named for CannedStreamStt {
        fn name(&self) -> &str { "canned-stream-stt" }
    }
```

- [ ] **Step 6: Run primer-core tests**

```bash
cd src && cargo test -p primer-core
```

Expected: all primer-core tests pass, including the new canary. Workspace at large will not yet compile because `silero.rs`, `whisper.rs`, `stub.rs` still declare `name()` on the leaf traits — that's Task 2.

- [ ] **Step 7: Do not commit yet** — wait for Task 2 so the whole refactor lands as one atomic commit.

### Task 2: Migrate every speech-backend `name()` into a `Named` impl

**Files:**
- Modify: `src/crates/primer-speech/src/silero.rs`
- Modify: `src/crates/primer-speech/src/whisper.rs`
- Modify: `src/crates/primer-speech/src/stub.rs`

- [ ] **Step 1: Confirm baseline failure**

```bash
cd src && cargo build --workspace
```

Expected: errors in `silero.rs`, `whisper.rs`, `stub.rs` of the form `the trait bound 'X: Named' is not satisfied`. The errors locate every site we must edit.

- [ ] **Step 2: Add `use primer_core::speech::Named;` where needed**

`silero.rs` already has `use primer_core::speech::{VadFrame, VoiceActivityDetector};` (around line 17) — extend to include `Named`:

```rust
use primer_core::speech::{Named, VadFrame, VoiceActivityDetector};
```

`whisper.rs` already has a multi-line `use primer_core::speech::{ ... }` (around line 21) — add `Named`:

```rust
use primer_core::speech::{
    AudioBuffer, Named, SpeechToText, StreamingSpeechToText, Transcript, TranscriptSegment,
    TranscriptionSession,
};
```

`stub.rs` already has `use primer_core::speech::*;` — `Named` is included by the wildcard. No edit needed.

- [ ] **Step 3: Move `silero.rs::name()` into a `Named` impl**

In `src/crates/primer-speech/src/silero.rs`, replace:

```rust
impl VoiceActivityDetector for SileroVad {
    fn name(&self) -> &str {
        "silero-vad"
    }

    fn sample_rate(&self) -> u32 {
```

with:

```rust
impl Named for SileroVad {
    fn name(&self) -> &str {
        "silero-vad"
    }
}

impl VoiceActivityDetector for SileroVad {
    fn sample_rate(&self) -> u32 {
```

- [ ] **Step 4: Collapse the two `whisper.rs::name()` impls into one `Named` impl**

In `src/crates/primer-speech/src/whisper.rs`, remove the `fn name(&self) -> &str { BACKEND_NAME }` from inside `impl SpeechToText for WhisperStt` (around line 83) and from inside `impl StreamingSpeechToText for WhisperStt` (around line 116). Add a fresh `impl Named for WhisperStt` between the struct definition and the trait impls (around line 80):

```rust
impl Named for WhisperStt {
    fn name(&self) -> &str {
        BACKEND_NAME
    }
}
```

- [ ] **Step 5: Move `stub.rs::name()` calls into `Named` impls**

In `src/crates/primer-speech/src/stub.rs`, change:

```rust
#[async_trait]
impl SpeechToText for StubStt {
    fn name(&self) -> &str {
        "stub-stt"
    }

    async fn transcribe(&self, audio: &AudioBuffer) -> Result<Transcript> {
```

to:

```rust
impl Named for StubStt {
    fn name(&self) -> &str {
        "stub-stt"
    }
}

#[async_trait]
impl SpeechToText for StubStt {
    async fn transcribe(&self, audio: &AudioBuffer) -> Result<Transcript> {
```

And similarly:

```rust
#[async_trait]
impl TextToSpeech for StubTts {
    fn name(&self) -> &str {
        "stub-tts"
    }

    async fn synthesize(&self, text: &str, _voice: &VoiceProfile) -> Result<AudioBuffer> {
```

to:

```rust
impl Named for StubTts {
    fn name(&self) -> &str {
        "stub-tts"
    }
}

#[async_trait]
impl TextToSpeech for StubTts {
    async fn synthesize(&self, text: &str, _voice: &VoiceProfile) -> Result<AudioBuffer> {
```

- [ ] **Step 6: Build and run all tests**

```bash
cd src && cargo build --workspace && cargo test --workspace
```

Expected: clean build, all 208 + 1 (canary) = 209 tests pass.

- [ ] **Step 7: Commit Task 1 + Task 2 together**

```bash
git add -A
git commit -m "$(cat <<'EOF'
refactor(speech): extract Named super-trait

Closes the dual-trait UFCS papercut PR #3 noted: backends that implement
both one-shot and streaming variants of STT or TTS now write a single
Named impl rather than duplicating name() across every trait. All four
speech traits (VoiceActivityDetector, SpeechToText, StreamingSpeechToText,
TextToSpeech) now inherit from Named.

Mechanical edit only — no behaviour change. Canary test asserts dispatch
through the super-trait still resolves on every leaf trait.

Tests: 208 → 209.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2 — `StreamingTextToSpeech` trait + `AudioChunk` + `SynthesisSession`

### Task 3: Add the new trait family with a mock-driven test

**Files:**
- Modify: `src/crates/primer-core/src/speech.rs`

- [ ] **Step 1: Write the failing trait-shape test**

In the `#[cfg(test)] mod tests` block of `speech.rs`, append after the canary test:

```rust
    /// Mock streaming-TTS that emits one canned `AudioChunk` per push.
    struct CannedStreamingTts;

    /// Sample rate used by the canned mock — matches Whisper's so the
    /// number isn't a magic literal.
    const CANNED_TTS_SAMPLE_RATE: u32 = 22_050;
    /// Each push from the canned mock yields this many samples.
    const CANNED_TTS_SAMPLES_PER_CHUNK: usize = 64;

    struct CannedSynthesisSession {
        scripted: std::vec::IntoIter<&'static str>,
        sample_rate: u32,
    }

    impl SynthesisSession for CannedSynthesisSession {
        fn push_text(&mut self, _text: &str) -> Result<Vec<AudioChunk>> {
            match self.scripted.next() {
                Some(_) => Ok(vec![AudioChunk {
                    samples: vec![0.0; CANNED_TTS_SAMPLES_PER_CHUNK],
                    sample_rate: self.sample_rate,
                }]),
                None => Ok(vec![]),
            }
        }
        fn finalize(self: Box<Self>) -> Result<Vec<AudioChunk>> {
            Ok(vec![])
        }
    }

    impl Named for CannedStreamingTts {
        fn name(&self) -> &str {
            "canned-stream-tts"
        }
    }

    impl StreamingTextToSpeech for CannedStreamingTts {
        fn sample_rate(&self) -> u32 {
            CANNED_TTS_SAMPLE_RATE
        }
        fn open_session(&self, _voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>> {
            Ok(Box::new(CannedSynthesisSession {
                scripted: vec!["alpha", "beta"].into_iter(),
                sample_rate: CANNED_TTS_SAMPLE_RATE,
            }))
        }
    }

    #[test]
    fn streaming_tts_session_yields_chunks_and_finalizes() {
        let tts: Box<dyn StreamingTextToSpeech> = Box::new(CannedStreamingTts);
        assert_eq!(Named::name(&*tts), "canned-stream-tts");
        assert_eq!(tts.sample_rate(), CANNED_TTS_SAMPLE_RATE);
        let voice = VoiceProfile::default();
        let mut session = tts.open_session(&voice).unwrap();
        let c0 = session.push_text("hello.").unwrap();
        let c1 = session.push_text(" world.").unwrap();
        let c2 = session.push_text("").unwrap();
        assert_eq!(c0.len(), 1);
        assert_eq!(c0[0].samples.len(), CANNED_TTS_SAMPLES_PER_CHUNK);
        assert_eq!(c0[0].sample_rate, CANNED_TTS_SAMPLE_RATE);
        assert_eq!(c1.len(), 1);
        assert!(c2.is_empty());
        let trailing = session.finalize().unwrap();
        assert!(trailing.is_empty());
    }
```

- [ ] **Step 2: Run, expect compile failure**

```bash
cd src && cargo test -p primer-core streaming_tts_session_yields_chunks_and_finalizes
```

Expected: errors `cannot find type AudioChunk`, `cannot find trait SynthesisSession`, `cannot find trait StreamingTextToSpeech`.

- [ ] **Step 3: Add the new types**

In `src/crates/primer-core/src/speech.rs`, append after the existing `TextToSpeech` trait declaration:

```rust
/// One PCM chunk emitted by a [`SynthesisSession`] during streaming.
///
/// Emitted as soon as the underlying model has enough context to commit
/// audio (typically once a phrase boundary is reached). Concatenate the
/// `samples` of every chunk in order to reconstruct the full utterance.
/// `sample_rate` is carried per-chunk even though every chunk in one
/// session shares one — keeps this type usable by audio sinks that don't
/// hold a reference to the originating backend.
#[derive(Debug, Clone)]
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
    /// Push text; receive any audio chunks that became available as a result.
    fn push_text(&mut self, text: &str) -> Result<Vec<AudioChunk>>;

    /// Drain remaining buffered text and finalize. Consumes the session.
    fn finalize(self: Box<Self>) -> Result<Vec<AudioChunk>>;
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

- [ ] **Step 4: Run the test**

```bash
cd src && cargo test -p primer-core streaming_tts_session_yields_chunks_and_finalizes
```

Expected: PASS. Run the full primer-core suite to confirm no regressions:

```bash
cd src && cargo test -p primer-core
```

- [ ] **Step 5: Run the full workspace build to make sure nothing else broke**

```bash
cd src && cargo build --workspace && cargo test --workspace
```

Expected: 210 tests pass (209 + the new streaming-TTS canary).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(core): StreamingTextToSpeech trait + AudioChunk + SynthesisSession

Mirrors the streaming-STT lifecycle from PR #3. Open one session per
Primer turn, push_text accumulates and emits chunks as soon as the
backend has enough context, finalize drains the trailing buffer.

Tests: 209 → 210 (CannedStreamingTts mock exercises the trait surface
through Box<dyn StreamingTextToSpeech>).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3 — `phrase_split.rs` pure helper

### Task 4: Implement `PhraseSplitter` with the full unit-test suite

**Files:**
- Create: `src/crates/primer-speech/src/phrase_split.rs`
- Modify: `src/crates/primer-speech/src/lib.rs`

- [ ] **Step 1: Wire the module into `lib.rs`**

In `src/crates/primer-speech/src/lib.rs`, add after the existing `pub mod stub;` block:

```rust
pub mod phrase_split;
pub use phrase_split::PhraseSplitter;
```

- [ ] **Step 2: Create the module skeleton with a placeholder impl that fails every test**

Create `src/crates/primer-speech/src/phrase_split.rs`:

```rust
//! Phrase splitter — pure text-segmentation helper for streaming TTS.
//!
//! Walks an accumulated buffer and emits phrases as soon as a terminator
//! (`. ! ?`) is followed by whitespace. Kept separate from any specific
//! backend so the boundary state machine can be unit-tested without
//! pulling in piper-rs / ONNX Runtime.
//!
//! The rule set mirrors what a streaming speech synthesiser actually
//! needs: split where a human reader would pause for breath, and
//! suppress false splits that destroy delivery (decimals, abbreviations,
//! ellipses).

const PHRASE_TERMINATORS: &[char] = &['.', '!', '?'];

/// ASCII-lowercase abbreviations that should NOT be treated as phrase
/// boundaries. Conservative starting list — extend with evidence.
/// Internal periods (`e.g`, `i.e`, `u.s`) handled as the trailing token
/// only; mid-acronym dots like `U.S.A.` would still split. Acceptable
/// for the children's-conversation register Piper sees today.
const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "prof",
    "sr", "jr", "st",
    "vs", "etc", "ie", "eg",
    "us", "uk",
];

/// Streaming phrase splitter.
///
/// Append text via [`Self::push`]; receive any phrases that became
/// complete as a result. Call [`Self::flush`] when the upstream stream
/// has closed to drain whatever remains regardless of terminator.
#[derive(Debug, Default)]
pub struct PhraseSplitter {
    buffer: String,
}

impl PhraseSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `text` and return any phrases that became complete as a
    /// result. Each returned phrase is trimmed.
    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.buffer.push_str(text);
        self.drain_completed()
    }

    /// Drain whatever remains in the buffer, regardless of terminator.
    /// Returns `None` if the buffer is empty or whitespace-only.
    pub fn flush(&mut self) -> Option<String> {
        let trimmed = self.buffer.trim().to_string();
        self.buffer.clear();
        if trimmed.is_empty() { None } else { Some(trimmed) }
    }

    fn drain_completed(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(end) = self.find_boundary() {
            let phrase: String = self.buffer[..end].trim().to_string();
            self.buffer.drain(..end);
            if !phrase.is_empty() {
                out.push(phrase);
            }
        }
        out
    }

    /// Find the byte index *after* the next phrase boundary (and any
    /// trailing whitespace consumed with it), or `None` if the buffer
    /// doesn't yet contain a complete phrase.
    ///
    /// Rules:
    /// 1. `buffer[i]` must be in `PHRASE_TERMINATORS`.
    /// 2. There must exist a char at the position just after any run of
    ///    identical `.` characters starting at `i` (so `...` collapses).
    /// 3. That next char must be whitespace.
    /// 4. If the terminator is a single `.` and the word ending at `i`
    ///    is an abbreviation, no boundary.
    /// 5. Decimal guard is implicit in (3): `3.1` never qualifies because
    ///    `1` isn't whitespace.
    fn find_boundary(&self) -> Option<usize> {
        let mut iter = self.buffer.char_indices().peekable();
        while let Some((i, ch)) = iter.next() {
            if !PHRASE_TERMINATORS.contains(&ch) {
                continue;
            }

            // Collapse a run of `.` (handles `...` and longer).
            let mut term_end = i + ch.len_utf8();
            if ch == '.' {
                while let Some(&(_, next_ch)) = iter.peek() {
                    if next_ch == '.' {
                        term_end += '.'.len_utf8();
                        iter.next();
                    } else {
                        break;
                    }
                }
            }

            // Need a char after the terminator run.
            let next_ch = match self.buffer[term_end..].chars().next() {
                Some(c) => c,
                None => return None,
            };

            if !next_ch.is_whitespace() {
                continue;
            }

            // Abbreviation guard: only relevant for a single-dot terminator.
            let is_single_dot = ch == '.' && term_end == i + 1;
            if is_single_dot && self.is_abbreviation_before(i) {
                continue;
            }

            // Consume trailing whitespace so the next phrase doesn't start
            // with leading space.
            let mut after = term_end;
            for (j, c) in self.buffer[term_end..].char_indices() {
                if c.is_whitespace() {
                    after = term_end + j + c.len_utf8();
                } else {
                    break;
                }
            }
            return Some(after);
        }
        None
    }

    fn is_abbreviation_before(&self, dot_index: usize) -> bool {
        let prefix = &self.buffer[..dot_index];
        let word: String = prefix
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>()
            .to_ascii_lowercase();
        if word.is_empty() {
            return false;
        }
        ABBREVIATIONS.contains(&word.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_nothing() {
        let mut s = PhraseSplitter::new();
        assert!(s.push("").is_empty());
        assert_eq!(s.flush(), None);
    }

    #[test]
    fn simple_two_sentences_emit_two_phrases() {
        let mut s = PhraseSplitter::new();
        let phrases = s.push("Hello. World. ");
        assert_eq!(phrases, vec!["Hello.", "World."]);
        // Buffer should now be empty (trailing whitespace consumed).
        assert_eq!(s.flush(), None);
    }

    #[test]
    fn decimal_does_not_split() {
        let mut s = PhraseSplitter::new();
        // Trailing space confirms the period after "today" is a real boundary.
        let phrases = s.push("It is 3.14 today. ");
        assert_eq!(phrases, vec!["It is 3.14 today."]);
    }

    #[test]
    fn abbreviation_does_not_split() {
        let mut s = PhraseSplitter::new();
        let phrases = s.push("Dr. Smith arrived. ");
        assert_eq!(phrases, vec!["Dr. Smith arrived."]);
    }

    #[test]
    fn multiple_terminators_collapse() {
        let mut s = PhraseSplitter::new();
        let phrases = s.push("Wait... what? ");
        assert_eq!(phrases, vec!["Wait...", "what?"]);
    }

    #[test]
    fn mid_token_push_does_not_eagerly_split() {
        let mut s = PhraseSplitter::new();
        // First push has a terminator but no following whitespace yet — no boundary.
        let p0 = s.push("Hello.");
        assert!(p0.is_empty());
        // Next push starts with whitespace, completing the boundary, and contains its own.
        let p1 = s.push(" World. ");
        assert_eq!(p1, vec!["Hello.", "World."]);
    }

    #[test]
    fn flush_drains_pending_text_without_terminator() {
        let mut s = PhraseSplitter::new();
        assert!(s.push("Hello").is_empty());
        assert_eq!(s.flush(), Some("Hello".to_string()));
    }

    #[test]
    fn flush_returns_none_on_empty_or_whitespace() {
        let mut s = PhraseSplitter::new();
        assert!(s.push("   \n\t").is_empty());
        assert_eq!(s.flush(), None);
    }

    #[test]
    fn non_ascii_in_phrase_does_not_panic() {
        let mut s = PhraseSplitter::new();
        let phrases = s.push("Bonjour Élise. ");
        assert_eq!(phrases, vec!["Bonjour Élise."]);
    }

    #[test]
    fn exclamation_and_question_split() {
        let mut s = PhraseSplitter::new();
        let phrases = s.push("Wow! Really? ");
        assert_eq!(phrases, vec!["Wow!", "Really?"]);
    }
}
```

- [ ] **Step 3: Run the tests**

```bash
cd src && cargo test -p primer-speech phrase_split
```

Expected: all 10 tests pass.

- [ ] **Step 4: Run the full workspace test suite**

```bash
cd src && cargo build --workspace && cargo test --workspace
```

Expected: 220 tests pass (210 + 10).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(speech): PhraseSplitter helper for streaming TTS

Pure text-segmentation module — same shape as vad_debounce. Splits an
incoming text stream on . ! ? followed by whitespace, with abbreviation
guard, decimal guard (implicit), and ellipsis collapse. Iteration uses
char_indices so non-ASCII glyphs in children's names can't trigger a
UTF-8 boundary panic.

Tests: 210 → 220 (10 cases covering empty, two-sentence, decimal,
abbreviation, ellipsis, mid-token push, flush drain, flush empty,
non-ASCII safety, exclamation+question).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4 — `PiperTts` backend (behind `piper` feature)

### Task 5: Wire the `piper` feature, deps, and example slot in Cargo.toml

**Files:**
- Modify: `src/crates/primer-speech/Cargo.toml`

- [ ] **Step 1: Edit the manifest**

Replace the `[dependencies]`, `[features]` sections of `src/crates/primer-speech/Cargo.toml` (preserving every existing line) so the file reads:

```toml
[package]
name = "primer-speech"
description = "Speech-to-text and text-to-speech backends for the Primer"
version.workspace = true
edition.workspace = true

[dependencies]
primer-core.workspace = true
async-trait.workspace = true
tokio.workspace = true
tracing.workspace = true
thiserror.workspace = true

# Silero VAD — bundled ONNX weights, opt-in via the `silero` feature so the
# ort/ndarray dep tree only compiles for users who want VAD.
silero-vad-rust = { version = "6.2", default-features = false, optional = true }
# Pin ort to the rc that silero-vad-rust 6.2 was built against; without
# this, Cargo picks a newer rc whose bundled ndarray version diverges from
# silero-vad-rust's direct ndarray dep and the type `Array2<f32>` no
# longer matches across crates.
ort = { version = "=2.0.0-rc.10", default-features = false, features = ["ndarray"], optional = true }

# Whisper.cpp — opt-in via the `whisper` feature. Builds bundled C++ source
# (cmake + a C++ compiler must be present on the build host).
whisper-cpp-plus = { version = "0.1", default-features = false, optional = true }

# Piper TTS — opt-in via the `piper` feature. Pulls ort + espeak-rs + riff-wave
# transitively. Watch for ort rc disagreement with silero-vad-rust 6.2 when
# both `silero` and `piper` features are enabled.
piper-rs = { version = "0.1", default-features = false, optional = true }
# Used by the piper backend to read `audio.sample_rate` from the voice
# config sidecar JSON. Optional so the default workspace build doesn't
# pull serde_json into primer-speech.
serde_json = { workspace = true, optional = true }

[dev-dependencies]
# WAV writer for the tts_hello example. Dev-only — not part of the
# default build dep tree.
hound = "3"
clap = { workspace = true }

[features]
default = []
silero = ["dep:silero-vad-rust", "dep:ort"]
whisper = ["dep:whisper-cpp-plus"]
piper = ["dep:piper-rs", "dep:serde_json"]

[[example]]
name = "tts_hello"
required-features = ["piper"]
```

- [ ] **Step 2: Confirm the default build still works (no piper-rs pulled)**

```bash
cd src && cargo build --workspace
```

Expected: clean. `cargo metadata` should not show `piper-rs` in the resolved tree:

```bash
cd src && cargo tree --workspace 2>/dev/null | grep -i piper-rs || echo "piper-rs not in default tree"
```

Expected: `piper-rs not in default tree`.

- [ ] **Step 3: Confirm the `piper`-feature build resolves piper-rs (download/compile may take a while on first run)**

```bash
cd src && cargo check -p primer-speech --features piper
```

Expected: succeeds (or fails at `cdn.pyke.io` download in sandboxed CI — this is acceptable per the brief; document and move on if it does).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
build(speech): add piper feature, piper-rs dep, tts_hello example slot

piper-rs is opt-in via a `piper` feature so the default workspace build
remains clean. serde_json is also gated under the same feature (only
the piper backend needs it). hound + clap live in [dev-dependencies]
to support the example binary. The example itself is gated via
required-features so cargo build --examples without --features piper
doesn't try to pull piper-rs.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 6: Add the `piper_config` pure helper (parse `audio.sample_rate` from voice JSON)

**Files:**
- Create: `src/crates/primer-speech/src/piper_config.rs`
- Modify: `src/crates/primer-speech/src/lib.rs`

- [ ] **Step 1: Wire the module into `lib.rs`**

In `src/crates/primer-speech/src/lib.rs`, add:

```rust
#[cfg(feature = "piper")]
pub mod piper_config;
```

(Place it adjacent to where `silero` and `whisper` are wired.)

- [ ] **Step 2: Write the failing test**

Create `src/crates/primer-speech/src/piper_config.rs`:

```rust
//! Read sample-rate from a Piper voice config JSON file.
//!
//! Piper voice configs declare their own `audio.sample_rate` (typically
//! 16_000, 22_050, or 24_000 depending on the voice). This helper is
//! a pure function over a JSON path so it can be unit-tested without
//! constructing a real `piper_rs::Piper`.

use std::fs;
use std::path::Path;

use primer_core::error::{PrimerError, Result};

/// JSON pointer-ish path into a Piper voice config: `audio.sample_rate`.
const SAMPLE_RATE_KEY: &str = "audio.sample_rate";

/// Read `audio.sample_rate` from the voice config JSON at `path`.
///
/// Errors with [`PrimerError::Speech`] if the file is missing, unparseable,
/// or doesn't contain a non-zero integer at `audio.sample_rate`.
pub fn read_sample_rate(path: impl AsRef<Path>) -> Result<u32> {
    let path = path.as_ref();
    let raw = fs::read_to_string(path)
        .map_err(|e| PrimerError::Speech(format!("read piper config {path:?}: {e}")))?;
    let json: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| PrimerError::Speech(format!("parse piper config {path:?}: {e}")))?;
    let rate = json
        .get("audio")
        .and_then(|a| a.get("sample_rate"))
        .and_then(|v| v.as_u64())
        .ok_or_else(|| {
            PrimerError::Speech(format!(
                "piper config {path:?} missing {SAMPLE_RATE_KEY} or wrong type"
            ))
        })?;
    if rate == 0 || rate > u32::MAX as u64 {
        return Err(PrimerError::Speech(format!(
            "piper config {path:?} has implausible sample_rate {rate}"
        )));
    }
    Ok(rate as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp_json(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        f.write_all(content.as_bytes()).expect("write");
        f
    }

    #[test]
    fn reads_sample_rate_from_minimal_config() {
        let f = write_temp_json(r#"{"audio":{"sample_rate":22050}}"#);
        assert_eq!(read_sample_rate(f.path()).unwrap(), 22_050);
    }

    #[test]
    fn errors_when_audio_section_missing() {
        let f = write_temp_json(r#"{"phonemes":{}}"#);
        let err = read_sample_rate(f.path()).unwrap_err();
        assert!(format!("{err}").contains("audio.sample_rate"));
    }

    #[test]
    fn errors_when_sample_rate_zero() {
        let f = write_temp_json(r#"{"audio":{"sample_rate":0}}"#);
        let err = read_sample_rate(f.path()).unwrap_err();
        assert!(format!("{err}").contains("implausible"));
    }

    #[test]
    fn errors_when_file_missing() {
        let err = read_sample_rate("/tmp/__primer_no_such_file__.json").unwrap_err();
        assert!(format!("{err}").contains("read piper config"));
    }
}
```

- [ ] **Step 3: Add `tempfile` as a dev-dependency**

In `src/crates/primer-speech/Cargo.toml`, add to `[dev-dependencies]`:

```toml
tempfile = "3"
```

- [ ] **Step 4: Run, expect all 4 tests pass**

```bash
cd src && cargo test -p primer-speech --features piper piper_config
```

Expected: 4 tests pass.

- [ ] **Step 5: Run the full workspace build (no features)**

```bash
cd src && cargo build --workspace && cargo test --workspace
```

Expected: 220 tests pass (no change in default tree — `piper_config` is feature-gated). Confirm `piper_config` tests are NOT included in the default test run.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(speech): piper_config helper reads audio.sample_rate from voice JSON

Pure function over a JSON path so the sample-rate read is unit-testable
without constructing a real piper_rs::Piper. Errors clearly when the
file is missing, unparseable, or lacks a plausible audio.sample_rate.

Tests: +4 under --features piper (gated; default workspace test count
unchanged at 220).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 7: Add the `piper.rs` backend (`PiperTts` + `PiperSession`)

**Files:**
- Create: `src/crates/primer-speech/src/piper.rs`
- Modify: `src/crates/primer-speech/src/lib.rs`

> **Note on `piper-rs::Piper::create` parameter order.** Based on `examples/wav.rs` of upstream piper-rs 0.1.x, `create(text, false, speaker_id, None, None, None)` is six args returning `(Vec<f32>, u32)`. The 4th–6th `Option`s are presumed to be `length_scale`, `noise_scale`, `noise_w` (standard Piper inference knobs). Verify this at the very start of this task by running `cargo doc --features piper --no-deps -p primer-speech` (or `rustdoc --crate-name piper_rs ...`) once the dep is resolved. If the parameter ordering is different, adjust the `create(...)` call sites below to match — the structural code (PhraseSplitter integration, `Arc<Piper>`, voice-mismatch validation) does not depend on parameter ordering.

- [ ] **Step 1: Wire the module into `lib.rs`**

In `src/crates/primer-speech/src/lib.rs`, add adjacent to the existing speech-feature gates:

```rust
#[cfg(feature = "piper")]
pub mod piper;
#[cfg(feature = "piper")]
pub use piper::PiperTts;
```

- [ ] **Step 2: Create `piper.rs` with the full impl**

Create `src/crates/primer-speech/src/piper.rs`:

```rust
//! Piper TTS implementation of [`TextToSpeech`] and
//! [`StreamingTextToSpeech`].
//!
//! Wraps `piper-rs` (`thewh1teagle/piper-rs`). One ONNX model + JSON
//! config pair is loaded on construction; the same loaded `Piper` is
//! shared across sessions via `Arc`.
//!
//! Streaming is achieved by feeding incoming text through a
//! [`PhraseSplitter`] and synthesising one phrase at a time via
//! `Piper::create`. piper-rs 0.1.x has no native phrase-boundary
//! callback, so this is the smallest correct way to get audio chunks
//! out before the LLM has finished generating.
//!
//! # Build prerequisites
//!
//! Enabling the `piper` feature pulls in `piper-rs`, which transitively
//! pulls `ort`, `espeak-rs`, `riff-wave`, and `rayon`. ONNX Runtime
//! downloads a prebuilt binary from `cdn.pyke.io` on first build —
//! sandboxed CI environments will fail at this step. After the first
//! successful build the binary is cached under the cargo target dir.
//!
//! Piper voices are distributed as `*.onnx` + `*.onnx.json` pairs from
//! `huggingface.co/rhasspy/piper-voices`. The `tts_hello` example shows
//! the typical loading flow.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use primer_core::error::{PrimerError, Result};
use primer_core::speech::{
    AudioBuffer, AudioChunk, Named, StreamingTextToSpeech, SynthesisSession, TextToSpeech,
    VoiceProfile,
};

use crate::phrase_split::PhraseSplitter;
use crate::piper_config;

/// Backend identifier returned by [`Named::name`].
const BACKEND_NAME: &str = "piper";

/// `length_scale` value piper-rs treats as "normal pace". `VoiceProfile.rate`
/// inverts: `length_scale = 1.0 / rate`. A `rate > 1.0` means faster
/// delivery and therefore a *smaller* length_scale.
const DEFAULT_LENGTH_SCALE: f32 = 1.0;

/// Piper text-to-speech backend.
///
/// Loads one Piper voice (`.onnx` + `.onnx.json` pair) on construction;
/// the same loaded `Piper` is shared across all sessions via `Arc`.
/// Both the one-shot [`TextToSpeech`] and the streaming
/// [`StreamingTextToSpeech`] traits are implemented; pick whichever
/// matches the call site.
///
/// One voice per backend instance — runtime voice switching is not
/// supported. Construct multiple `PiperTts` if you need multiple voices;
/// `open_session(voice)` returns `Err` if `voice.model_id` doesn't
/// match the constructor-time voice.
pub struct PiperTts {
    piper: Arc<piper_rs::Piper>,
    voice: VoiceProfile,
    speaker_id: Option<i64>,
    sample_rate: u32,
}

impl PiperTts {
    /// Load a Piper voice from the given `.onnx` and `.onnx.json` pair.
    ///
    /// `voice.model_id` defaults to a stem derived from `onnx_path`'s
    /// file name (without extension). Override via [`Self::with_voice`].
    pub fn new(
        onnx_path: impl AsRef<Path>,
        config_path: impl AsRef<Path>,
    ) -> Result<Self> {
        let onnx_path = onnx_path.as_ref();
        let config_path = config_path.as_ref();
        let sample_rate = piper_config::read_sample_rate(config_path)?;
        let piper = piper_rs::Piper::new(onnx_path, config_path)
            .map_err(|e| PrimerError::Speech(format!("load piper voice: {e}")))?;
        let model_id = onnx_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("piper-voice")
            .to_string();
        Ok(Self {
            piper: Arc::new(piper),
            voice: VoiceProfile {
                model_id,
                ..VoiceProfile::default()
            },
            speaker_id: None,
            sample_rate,
        })
    }

    /// Set the default `VoiceProfile` for sessions opened via this backend.
    pub fn with_voice(mut self, voice: VoiceProfile) -> Self {
        self.voice = voice;
        self
    }

    /// Set a multi-speaker model's speaker id (ignored for single-speaker voices).
    pub fn with_speaker_id(mut self, id: i64) -> Self {
        self.speaker_id = Some(id);
        self
    }

    fn validate_voice(&self, requested: &VoiceProfile) -> Result<()> {
        if requested.model_id != self.voice.model_id {
            return Err(PrimerError::Speech(format!(
                "piper voice mismatch: backend loaded {:?}, session asked for {:?}",
                self.voice.model_id, requested.model_id
            )));
        }
        Ok(())
    }

    fn length_scale_for(&self, voice: &VoiceProfile) -> f32 {
        if voice.rate > 0.0 {
            DEFAULT_LENGTH_SCALE / voice.rate
        } else {
            DEFAULT_LENGTH_SCALE
        }
    }
}

impl Named for PiperTts {
    fn name(&self) -> &str {
        BACKEND_NAME
    }
}

#[async_trait]
impl TextToSpeech for PiperTts {
    async fn synthesize(&self, text: &str, voice: &VoiceProfile) -> Result<AudioBuffer> {
        self.validate_voice(voice)?;
        if voice.pitch != 0.0 {
            tracing::warn!(
                pitch = voice.pitch,
                "piper backend ignores VoiceProfile.pitch (no upstream knob)"
            );
        }
        let piper = self.piper.clone();
        let speaker_id = self.speaker_id;
        let length_scale = self.length_scale_for(voice);
        let text = text.to_string();
        let sample_rate = self.sample_rate;
        let samples = tokio::task::spawn_blocking(move || -> Result<Vec<f32>> {
            let (samples, _rate) = piper
                .create(&text, false, speaker_id, Some(length_scale), None, None)
                .map_err(|e| PrimerError::Speech(format!("piper synthesise: {e}")))?;
            Ok(samples)
        })
        .await
        .map_err(|e| PrimerError::Speech(format!("piper join: {e}")))??;
        Ok(AudioBuffer { samples, sample_rate })
    }
}

impl StreamingTextToSpeech for PiperTts {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn open_session(&self, voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>> {
        self.validate_voice(voice)?;
        if voice.pitch != 0.0 {
            tracing::warn!(
                pitch = voice.pitch,
                "piper backend ignores VoiceProfile.pitch (no upstream knob)"
            );
        }
        Ok(Box::new(PiperSession {
            piper: self.piper.clone(),
            splitter: PhraseSplitter::new(),
            length_scale: self.length_scale_for(voice),
            speaker_id: self.speaker_id,
            sample_rate: self.sample_rate,
        }))
    }
}

struct PiperSession {
    piper: Arc<piper_rs::Piper>,
    splitter: PhraseSplitter,
    length_scale: f32,
    speaker_id: Option<i64>,
    sample_rate: u32,
}

impl PiperSession {
    fn synth_phrase(&self, phrase: &str) -> Result<AudioChunk> {
        let (samples, _rate) = self
            .piper
            .create(phrase, false, self.speaker_id, Some(self.length_scale), None, None)
            .map_err(|e| PrimerError::Speech(format!("piper synthesise: {e}")))?;
        Ok(AudioChunk {
            samples,
            sample_rate: self.sample_rate,
        })
    }
}

impl SynthesisSession for PiperSession {
    fn push_text(&mut self, text: &str) -> Result<Vec<AudioChunk>> {
        let phrases = self.splitter.push(text);
        let mut out = Vec::with_capacity(phrases.len());
        for phrase in phrases {
            out.push(self.synth_phrase(&phrase)?);
        }
        Ok(out)
    }

    fn finalize(mut self: Box<Self>) -> Result<Vec<AudioChunk>> {
        match self.splitter.flush() {
            Some(trailing) => Ok(vec![self.synth_phrase(&trailing)?]),
            None => Ok(vec![]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real-model smoke test. Skipped unless `$PIPER_TEST_MODEL_ONNX` and
    /// `$PIPER_TEST_MODEL_CONFIG` are set, because piper-rs needs an
    /// actual voice file pair on disk and CI doesn't ship them.
    #[tokio::test]
    #[ignore]
    async fn piper_smoke_synthesise_returns_non_empty_audio() {
        let onnx = match std::env::var("PIPER_TEST_MODEL_ONNX") {
            Ok(p) => p,
            Err(_) => return, // env not set; treat as skip even without --ignored harness
        };
        let cfg = std::env::var("PIPER_TEST_MODEL_CONFIG").expect("PIPER_TEST_MODEL_CONFIG");
        let tts = PiperTts::new(&onnx, &cfg).expect("load piper");
        let voice = tts.voice.clone();
        let audio = tts
            .synthesize("Hello.", &voice)
            .await
            .expect("synthesise");
        assert!(!audio.samples.is_empty());
        assert_eq!(audio.sample_rate, tts.sample_rate());
    }
}
```

- [ ] **Step 3: Confirm the feature build passes**

```bash
cd src && cargo build -p primer-speech --features piper
```

Expected: succeeds (or `cdn.pyke.io` blocked in sandbox — acceptable).

- [ ] **Step 4: Confirm the default workspace build remains clean**

```bash
cd src && cargo build --workspace && cargo test --workspace
```

Expected: 220 tests pass — no change in default tree.

- [ ] **Step 5: Confirm the feature test compiles (smoke is `#[ignore]`)**

```bash
cd src && cargo test -p primer-speech --features piper --no-run
```

Expected: compile succeeds. The smoke test is gated so a normal `cargo test --features piper` run will skip it.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(speech): PiperTts backend impl Named + TextToSpeech + StreamingTextToSpeech

Wraps piper-rs 0.1.x. One voice per backend; open_session(voice) errors
on model_id mismatch. Streaming achieved via PhraseSplitter — each
completed phrase is a separate piper.create() call, yielding one
AudioChunk per phrase. Async TextToSpeech wraps piper.create in
spawn_blocking to keep the runtime free.

Voice knobs: rate → length_scale = 1.0/rate (with a const that names
the inversion); pitch warned-once and otherwise ignored (piper-rs 0.1.x
has no pitch knob); speaker_id via builder.

Smoke test gated behind --features piper AND $PIPER_TEST_MODEL_*; skipped
on CI. Default workspace test count unchanged at 220.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5 — `StubTts` streaming extension

### Task 8: Add `impl StreamingTextToSpeech for StubTts`

**Files:**
- Modify: `src/crates/primer-speech/src/stub.rs`

- [ ] **Step 1: Write the failing test**

Append a `#[cfg(test)] mod tests` block at the bottom of `src/crates/primer-speech/src/stub.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use primer_core::speech::{StreamingTextToSpeech, VoiceProfile};

    /// `StubTts` claims this rate as a no-op default since it emits silence.
    /// Mirrors the value used in stub.rs's existing synthesize body.
    const STUB_TTS_SAMPLE_RATE: u32 = 16_000;

    #[tokio::test]
    async fn stub_tts_streaming_emits_chunk_per_phrase() {
        let tts: Box<dyn StreamingTextToSpeech> = Box::new(StubTts);
        assert_eq!(tts.sample_rate(), STUB_TTS_SAMPLE_RATE);
        let mut session = tts.open_session(&VoiceProfile::default()).unwrap();
        let phrases = session.push_text("Hello. World. ").unwrap();
        assert_eq!(phrases.len(), 2);
        for chunk in &phrases {
            assert_eq!(chunk.sample_rate, STUB_TTS_SAMPLE_RATE);
            assert!(!chunk.samples.is_empty());
            assert!(chunk.samples.iter().all(|&s| s == 0.0));
        }
    }

    #[tokio::test]
    async fn stub_tts_streaming_finalize_drains_trailing() {
        let tts: Box<dyn StreamingTextToSpeech> = Box::new(StubTts);
        let mut session = tts.open_session(&VoiceProfile::default()).unwrap();
        let mid = session.push_text("Hello").unwrap();
        assert!(mid.is_empty());
        let trailing = session.finalize().unwrap();
        assert_eq!(trailing.len(), 1);
        assert_eq!(trailing[0].sample_rate, STUB_TTS_SAMPLE_RATE);
    }
}
```

- [ ] **Step 2: Run, expect compile failure**

```bash
cd src && cargo test -p primer-speech stub_tts_streaming_emits_chunk_per_phrase
```

Expected: error `the trait bound 'StubTts: StreamingTextToSpeech' is not satisfied`.

- [ ] **Step 3: Add the impl in `stub.rs`**

In `src/crates/primer-speech/src/stub.rs`, after the existing `impl TextToSpeech for StubTts { ... }` block, append:

```rust
use crate::phrase_split::PhraseSplitter;
use primer_core::speech::{AudioChunk, StreamingTextToSpeech, SynthesisSession};

/// Stub-TTS sample rate; matches the rate the existing one-shot
/// `synthesize` returns.
const STUB_TTS_SAMPLE_RATE: u32 = 16_000;
/// Number of zero samples per emitted stub chunk. One short burst per
/// phrase is enough to exercise the trait surface in tests.
const STUB_TTS_SAMPLES_PER_CHUNK: usize = 1_024;

impl StreamingTextToSpeech for StubTts {
    fn sample_rate(&self) -> u32 {
        STUB_TTS_SAMPLE_RATE
    }

    fn open_session(&self, _voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>> {
        Ok(Box::new(StubSynthesisSession {
            splitter: PhraseSplitter::new(),
        }))
    }
}

struct StubSynthesisSession {
    splitter: PhraseSplitter,
}

impl StubSynthesisSession {
    fn silent_chunk() -> AudioChunk {
        AudioChunk {
            samples: vec![0.0; STUB_TTS_SAMPLES_PER_CHUNK],
            sample_rate: STUB_TTS_SAMPLE_RATE,
        }
    }
}

impl SynthesisSession for StubSynthesisSession {
    fn push_text(&mut self, text: &str) -> Result<Vec<AudioChunk>> {
        let phrases = self.splitter.push(text);
        Ok(phrases.iter().map(|_| Self::silent_chunk()).collect())
    }

    fn finalize(mut self: Box<Self>) -> Result<Vec<AudioChunk>> {
        match self.splitter.flush() {
            Some(_) => Ok(vec![Self::silent_chunk()]),
            None => Ok(vec![]),
        }
    }
}
```

(The existing `use primer_core::speech::*;` at the top of the file should already cover `Result`, `VoiceProfile`, `AudioBuffer`. The explicit `use primer_core::speech::{AudioChunk, StreamingTextToSpeech, SynthesisSession};` is for clarity since it names exactly the new types this section uses.)

- [ ] **Step 4: Run the new tests**

```bash
cd src && cargo test -p primer-speech stub_tts_streaming
```

Expected: 2 tests pass.

- [ ] **Step 5: Run the full workspace tests**

```bash
cd src && cargo test --workspace
```

Expected: 222 tests pass (220 + 2).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(speech): StubTts implements StreamingTextToSpeech

Stub session uses PhraseSplitter and emits a fixed-length zero-sample
AudioChunk per completed phrase. Lets the new trait surface be
exercised through dyn dispatch in tests without any backend feature
being enabled.

Tests: 220 → 222.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 6 — `tts_hello` example binary

### Task 9: Add the smoke binary

**Files:**
- Create: `src/crates/primer-speech/examples/tts_hello.rs`

> **Note:** examples don't get unit tests; the manual run is the test. The example exercises the *streaming* path (push the whole phrase, then finalize, then concatenate emitted chunks) so it doubles as a manual integration check.

- [ ] **Step 1: Create the example**

Create `src/crates/primer-speech/examples/tts_hello.rs`:

```rust
//! Smoke binary for the Piper TTS backend.
//!
//! Synthesises a fixed phrase via the `StreamingTextToSpeech` path,
//! concatenates the emitted chunks, and writes a 16-bit PCM WAV.
//!
//! ```text
//! cargo run --example tts_hello --features piper -- \
//!   --onnx /path/to/en_US-amy-medium.onnx \
//!   --config /path/to/en_US-amy-medium.onnx.json \
//!   --out hello.wav
//! ```

use std::path::PathBuf;
use std::time::Instant;

use clap::Parser;
use hound::{SampleFormat, WavSpec, WavWriter};
use primer_core::speech::{StreamingTextToSpeech, VoiceProfile};
use primer_speech::PiperTts;

/// Phrase the brief uses for the smoke test.
const SMOKE_PHRASE: &str = "Hello, what would you like to learn about today?";

/// PCM bit depth for the WAV output. 16-bit i16 is the universal lingua
/// franca for short voice clips.
const WAV_BITS_PER_SAMPLE: u16 = 16;
/// Mono output — no stereo wiring in the example.
const WAV_CHANNELS: u16 = 1;
/// f32 → i16 conversion scale. f32 samples from piper-rs are in [-1.0, 1.0].
const I16_SCALE: f32 = i16::MAX as f32;

#[derive(Parser, Debug)]
#[command(about = "Piper TTS smoke binary")]
struct Args {
    /// Path to the Piper voice ONNX model (e.g. en_US-amy-medium.onnx).
    #[arg(long)]
    onnx: PathBuf,
    /// Path to the matching voice config JSON (e.g. en_US-amy-medium.onnx.json).
    #[arg(long)]
    config: PathBuf,
    /// Output WAV path.
    #[arg(long, default_value = "hello.wav")]
    out: PathBuf,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let started = Instant::now();

    let tts = PiperTts::new(&args.onnx, &args.config)?;
    let sample_rate = tts.sample_rate();
    let voice = VoiceProfile {
        model_id: args
            .onnx
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("piper-voice")
            .to_string(),
        ..VoiceProfile::default()
    };

    let mut session = tts.open_session(&voice)?;
    let mut samples: Vec<f32> = Vec::new();
    for chunk in session.push_text(SMOKE_PHRASE)? {
        samples.extend(chunk.samples);
    }
    for chunk in session.finalize()? {
        samples.extend(chunk.samples);
    }

    let spec = WavSpec {
        channels: WAV_CHANNELS,
        sample_rate,
        bits_per_sample: WAV_BITS_PER_SAMPLE,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(&args.out, spec)?;
    for s in &samples {
        let clamped = s.clamp(-1.0, 1.0);
        writer.write_sample((clamped * I16_SCALE) as i16)?;
    }
    writer.finalize()?;

    let elapsed = started.elapsed();
    println!(
        "wrote {} samples ({:.2}s of audio) at {sample_rate} Hz to {} in {elapsed:?}",
        samples.len(),
        samples.len() as f32 / sample_rate as f32,
        args.out.display()
    );
    Ok(())
}
```

- [ ] **Step 2: Confirm the example builds under the `piper` feature**

```bash
cd src && cargo build --example tts_hello --features piper
```

Expected: succeeds (or pyke.io blocked — acceptable; document and skip the manual run for now).

- [ ] **Step 3: Confirm the default workspace build is unaffected**

```bash
cd src && cargo build --workspace
```

Expected: clean. The example is gated `required-features = ["piper"]` so it doesn't appear in the default build set.

- [ ] **Step 4: Manual smoke (optional, requires a Piper voice on disk)**

If a voice is available locally, run:

```bash
cd src && cargo run --example tts_hello --features piper -- \
  --onnx /path/to/en_US-amy-medium.onnx \
  --config /path/to/en_US-amy-medium.onnx.json \
  --out /tmp/hello.wav
```

Expected: `wrote N samples (~3.0s of audio) at 22050 Hz to /tmp/hello.wav in <wall-clock>`. Listen via `afplay /tmp/hello.wav` on macOS or `aplay /tmp/hello.wav` on Linux. The output should sound like the smoke phrase, not noise.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(speech): tts_hello smoke binary exercises the streaming Piper path

Manual smoke test for the Piper backend. Drives the streaming path
(push_text → finalize → concatenate) so it doubles as an integration
check that the streaming output equals what one-shot synthesise would
produce. Writes 16-bit PCM WAV via hound.

Build-gated via required-features = ["piper"] so it stays out of the
default workspace build.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 7 — Docs

### Task 10: Refresh `CLAUDE.md` and the source brief

**Files:**
- Modify: `CLAUDE.md`
- Modify (or delete): `docs/primer_TTS_next_step.md`
- Modify (only if a relevant line exists): `ROADMAP.md`

- [ ] **Step 1: Edit `CLAUDE.md`**

In the "Architecture: trait-based hardware abstraction" section, find the bullet that lists `primer-core`'s public traits and update it to reflect the `Named` super-trait + the new streaming TTS trait. Replace the existing line:

```
- **`primer-core`** — the only crate that defines public traits (`InferenceBackend`, `KnowledgeBase`, `SpeechToText`, `TextToSpeech`) plus shared types (`LearnerModel`, `Session`, `Turn`, `PedagogicalIntent`, `EngagementState`, `UnderstandingDepth`, `Prompt`, `Passage`). Everything else depends on this.
```

with:

```
- **`primer-core`** — the only crate that defines public traits (`InferenceBackend`, `KnowledgeBase`, `Named`, `VoiceActivityDetector`, `SpeechToText`, `StreamingSpeechToText`, `TextToSpeech`, `StreamingTextToSpeech`) plus shared types (`LearnerModel`, `Session`, `Turn`, `PedagogicalIntent`, `EngagementState`, `UnderstandingDepth`, `Prompt`, `Passage`, `AudioChunk`). All speech traits inherit from `Named`, so a backend that implements both the one-shot and streaming variant of a trait writes its `name()` impl exactly once. Everything else depends on this.
```

In the same section, find the bullet for `primer-speech` and update it:

```
- **`primer-speech`** — stub-only today; speech is a Phase 2 concern.
```

Replace with:

```
- **`primer-speech`** — Phase 2 speech backends. Stubs always available; real backends behind features: Silero VAD (`silero`), Whisper STT (`whisper`), Piper TTS (`piper`). Pure helper modules `vad_debounce` and `phrase_split` carry the streaming state machines so they can be unit-tested without a backend dep. The Piper backend uses `PhraseSplitter` to chunk LLM output on `. ! ?` and synthesise per-phrase via `piper-rs::Piper::create`, since that crate has no native phrase-boundary callback. One voice per `PiperTts` instance (`open_session(voice)` errors on `model_id` mismatch); multi-voice runtime switching is deferred. Smoke binary `cargo run --example tts_hello --features piper` writes a WAV to validate a voice is wired correctly.
```

In the "Common commands" section, add an entry under the existing `cargo` commands:

```
cargo run --example tts_hello --features piper -- --onnx <path> --config <path> --out hello.wav   # Piper TTS smoke
```

In the "Conventions and gotchas worth knowing" section, add a new bullet:

```
- **`Named` is the single source of `name()` across every speech trait** (`VoiceActivityDetector`, `SpeechToText`, `StreamingSpeechToText`, `TextToSpeech`, `StreamingTextToSpeech`). Adding a new speech trait means inheriting from `Named` so backends never have to write `name()` twice.
```

- [ ] **Step 2: Refresh or delete `docs/primer_TTS_next_step.md`**

The doc's own "When you're done" item says:

> "Update or delete this doc — if step 3 closed everything cleanly, delete it; if you learned things about Piper or the trait shape that would help the next session (step 4: end-to-end voice loop), refresh it."

Step 4 will need to know:
- That `piper-rs::Piper::create` is synchronous and one-shot — `PhraseSplitter` is the streaming mechanism.
- That `PiperTts` is one-voice-per-instance; the unified speech REPL needs to instantiate it once at startup with a chosen voice.
- That `--voice <id>` doesn't exist on `primer-cli` yet — step 4 owns that wiring.

That's load-bearing for step 4. Refresh the doc rather than deleting it. Replace the entire file with a much shorter brief:

```markdown
# Primer — TTS post-step-3 brief

**Audience:** future Claude Code session implementing the unified speech REPL (step 4 of Phase 2).
**Last updated:** <implementation-date>

PR #4 (or whichever PR number this lands as) added `StreamingTextToSpeech`, a Piper backend, the `Named` super-trait, and `PhraseSplitter`. This brief carries forward the small handful of facts step 4 will care about.

## Carry-forward facts

- `piper-rs 0.1.x::Piper::create` is **synchronous and one-shot**. There is no native phrase-boundary callback. Streaming is faked via `PhraseSplitter` chunking on `. ! ?`. If step 4 ever wants finer-grained streaming (per-syllable, etc.) it has to wait for upstream piper-rs to expose hooks or move to a different synthesiser.
- `PiperTts` is **one voice per instance**. Construct it at startup with the chosen voice ONNX + JSON pair. `open_session(voice)` errors on `model_id` mismatch; if step 4 wants runtime voice switching it has to either build a `PiperTtsRouter` (HashMap of model_id → backend) or wait for a future multi-voice impl.
- The Piper backend's `sample_rate()` is read from the voice config at construction. Don't hardcode 22 050 — different voices use different rates.
- `cargo run --example tts_hello --features piper -- --onnx … --config … --out …` is the manual smoke. Reuse it as a copy-paste path-validity check before driving real audio out via cpal.
- ONNX Runtime first-build downloads from `cdn.pyke.io`. Sandboxed CI environments will fail. Document, don't fight.
- `primer-cli` does NOT yet have a `--voice` flag; step 4 owns that.

## What step 4 needs to add

Out of scope for this brief (read the spec for step 4 itself), but at a high level:
- cpal capture → SileroVad → WhisperStt streaming session → DialogueManager → PiperTts streaming session → cpal playback
- A new `--speech` flag on `primer-cli` that routes through the speech path instead of the text REPL
- A way to pick the voice (`--voice` plus a sensible default candidate from `en_US-amy-medium`, `en_GB-jenny_dioco-medium`, `en_GB-alba-medium`)

Delete this brief once step 4 is in.
```

Replace `<implementation-date>` with the actual date when committing.

- [ ] **Step 3: Check `ROADMAP.md` for a Phase 2 line item**

```bash
grep -n -i 'phase 2\|piper\|tts\|speech' ROADMAP.md
```

If a line resembling "Phase 2 / TTS / Piper" exists with a tickable `- [ ]`, change it to `- [x]`. Otherwise skip — don't invent a new line.

- [ ] **Step 4: Final verification**

```bash
cd src && cargo build --workspace && cargo test --workspace && cargo clippy --workspace --all-targets && cargo fmt --check
```

Expected: clean build, 222 unconditional tests pass, no new clippy warnings, fmt clean.

If clippy emits new warnings on the refactored speech traits or the new piper module, fix them in this same task — don't defer. Common ones to expect:
- `clippy::needless_pass_by_value` on `VoiceProfile` arguments — accept the lint or pass by reference if natural
- `clippy::module_inception` if `piper.rs` declares a `mod piper` (it doesn't; the file IS the module)

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
docs: refresh CLAUDE.md and primer_TTS_next_step.md for streaming TTS landing

CLAUDE.md now lists Named/StreamingTextToSpeech/AudioChunk and the
piper feature; gotchas section flags the single-source-of-name() rule.
The TTS next-step brief is rewritten as a short carry-forward for the
unified speech REPL (step 4): records the facts step 4 will care about
(piper-rs is one-shot, one voice per instance, sample rate per voice,
CLI --voice not yet wired) and what's still out of scope.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## PR

### Task 11: Push and open the pull request

- [ ] **Step 1: Push the branch**

```bash
git push -u origin feature/streaming-tts-piper
```

- [ ] **Step 2: Open the PR**

```bash
gh pr create --title "feat(speech): streaming TTS trait + Piper impl" --body "$(cat <<'EOF'
## Summary

- Adds `StreamingTextToSpeech` + `SynthesisSession` + `AudioChunk` to `primer-core` so the Primer can begin emitting audio before the LLM has finished generating a response.
- New `PiperTts` backend (`piper-rs 0.1.x`, behind a `piper` feature). Streaming via `PhraseSplitter` since piper-rs has no native phrase-boundary callback.
- Extracts a `Named` super-trait shared by all five speech traits, closing the dual-trait UFCS papercut PR #3 noted.
- Backend-only PR: `primer-cli` is unchanged. The unified speech REPL is step 4.

Spec: [docs/superpowers/specs/2026-05-02-streaming-tts-piper-design.md](../docs/superpowers/specs/2026-05-02-streaming-tts-piper-design.md)
Plan: [docs/superpowers/plans/2026-05-02-streaming-tts-piper-impl.md](../docs/superpowers/plans/2026-05-02-streaming-tts-piper-impl.md)

## Test plan

- [ ] `cargo test --workspace` — 222 unconditional tests pass (was 208 baseline; +1 Named canary, +1 streaming-TTS canary, +10 PhraseSplitter, +2 StubTts streaming).
- [ ] `cargo build --workspace` — default build pulls no `piper-rs` / `hound` / `serde_json` (latter is shared, unchanged in other crates).
- [ ] `cargo clippy --workspace --all-targets` — clean.
- [ ] `cargo fmt --check` — clean.
- [ ] `cargo build -p primer-speech --features piper` — succeeds on a real machine (may fail in CI sandbox at the cdn.pyke.io ONNX Runtime download — known limitation).
- [ ] `cargo test -p primer-speech --features piper` — 4 piper_config tests pass; piper smoke test is `#[ignore]` and skipped.
- [ ] Manual: `cargo run --example tts_hello --features piper -- --onnx <voice>.onnx --config <voice>.onnx.json --out hello.wav` produces a WAV that sounds like the smoke phrase.
EOF
)"
```

- [ ] **Step 3: Close the loop**

Return the PR URL printed by `gh pr create`. Verify it shows in the PR list:

```bash
gh pr list --head feature/streaming-tts-piper
```

---

## Done criteria

- All 11 tasks above committed with the convention-conforming subjects.
- 222 unconditional workspace tests pass; clippy + fmt clean.
- PR opened against `main` titled `feat(speech): streaming TTS trait + Piper impl`.
- `docs/primer_TTS_next_step.md` either deleted or refreshed as a step-4 carry-forward brief.
- `CLAUDE.md` reflects the `Named` super-trait, the new TTS trait family, and the `piper` feature.
