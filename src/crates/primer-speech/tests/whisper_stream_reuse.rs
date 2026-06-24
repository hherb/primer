//! Real-audio multi-utterance smoke test for `WhisperStream` cache reuse
//! (closes #166; follow-up to PR #164).
//!
//! # What this guards
//!
//! [`primer_speech::WhisperStt`] reuses a single `whisper_cpp_plus::WhisperStream`
//! across utterances via a single-slot cache: a finalised session returns
//! its stream and the next `open_session` calls `WhisperStream::reset()`
//! instead of paying the ≈500 ms cold-start. `reset()` only clears the
//! input-side per-utterance state (`audio_buf`, `pcmf32_old`,
//! `prompt_tokens`, `n_iter`, `total_samples_processed`); the underlying
//! `WhisperState` (KV cache + GPU compute buffers) is preserved — that is
//! the whole point of the optimisation.
//!
//! The PR #164 unit tests cover the cache *mechanism* (`take`/`put`
//! invariants on `StreamCache<i32>`) but cannot catch the upstream *reuse
//! semantics*: if the preserved `WhisperState` retains decoder logits or
//! attention state from utterance N, that state could bias utterance N+1.
//! The regression would be subtle — prior-turn words bleeding into the
//! next transcript — not a crash. That is exactly the bug class the
//! mechanism tests are blind to.
//!
//! # How the smoke proves independence
//!
//! Rather than hard-coding expected words (brittle against WER), the smoke
//! asserts **reuse-invariance**: it transcribes utterance B twice with
//! identical greedy params —
//!
//!   1. *cold*, in a fresh `WhisperStt` whose cache is empty, so the
//!      `WhisperStream` is freshly constructed; and
//!   2. *reused*, in a single `WhisperStt` where utterance A first
//!      constructs + caches the stream and B then reuses it via `reset()`.
//!
//! Greedy (`best_of = 1`) decoding is deterministic for a fixed model +
//! params + audio, so the two transcripts must match **unless** reuse let
//! A's state bleed into B. Equality of the reused transcript to the cold
//! reference is therefore a strictly stronger, ground-truth-free form of
//! "utterance B does not contain utterance A's terms".
//!
//! # Running it
//!
//! The smoke is `#[ignore]`'d because it needs a real GGML model and two
//! recordings on disk. Provide a 16 kHz mono whisper model and two short
//! 16 kHz mono WAV clips of *different* sentences (e.g. A = "the sky is
//! blue", B = "my favourite animal is a giraffe"):
//!
//! ```text
//! PRIMER_WHISPER_MODEL=/path/to/ggml-small.en.bin \
//! PRIMER_WHISPER_AUDIO_A=/path/to/utterance_a.wav \
//! PRIMER_WHISPER_AUDIO_B=/path/to/utterance_b.wav \
//!   ~/.cargo/bin/cargo test -p primer-speech --features whisper \
//!   --test whisper_stream_reuse -- --ignored --nocapture
//! ```
//!
//! The pure helpers below ([`normalize_transcript`], [`load_wav_mono_16k`],
//! [`chunk_samples`]) carry their own unit tests and run on every
//! `cargo test -p primer-speech` (no model, no `whisper` feature needed),
//! so the audio-plumbing logic is CI-covered even though the end-to-end
//! assertion is owner-run.

use std::path::Path;

/// Sample rate Whisper requires (16 kHz mono). Mirrors the private
/// `SAMPLE_RATE` const inside `WhisperStt`; duplicated here because that
/// const is not part of the crate's public surface. Audio at any other
/// rate is rejected by [`load_wav_mono_16k`] rather than silently
/// resampled — the recording is the test fixture and must already match.
const WHISPER_SAMPLE_RATE: u32 = 16_000;

/// Normalise a transcript for cross-run comparison.
///
/// Lowercases, drops every non-alphanumeric character, and collapses all
/// whitespace runs to a single space (trimming the ends). This makes the
/// comparison robust to incidental punctuation / casing / leading-space
/// differences between the cold and reused decode while preserving the
/// *words*, which are the only thing a `WhisperState` bleed would change.
fn normalize_transcript(s: &str) -> String {
    s.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(|c| c.to_lowercase())
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Load a WAV file as 16 kHz mono `f32` samples in `[-1.0, 1.0]`.
///
/// Returns `Err` (rather than panicking, unlike the upstream
/// `whisper-cpp-plus` template) when the file cannot be opened, is not
/// 16 kHz, uses an unsupported sample format/bit depth or channel count,
/// or contains no samples — so the smoke can surface a clear "check your
/// recording" message instead of an opaque panic. Stereo is down-mixed to
/// mono by averaging the two channels.
fn load_wav_mono_16k(path: &Path) -> Result<Vec<f32>, String> {
    let mut reader =
        hound::WavReader::open(path).map_err(|e| format!("open WAV {}: {e}", path.display()))?;
    let spec = reader.spec();
    if spec.sample_rate != WHISPER_SAMPLE_RATE {
        return Err(format!(
            "{} must be {WHISPER_SAMPLE_RATE} Hz, got {} Hz",
            path.display(),
            spec.sample_rate
        ));
    }

    let interleaved: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
            .collect::<Result<_, _>>()
            .map_err(|e| format!("read 16-bit samples from {}: {e}", path.display()))?,
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| format!("read f32 samples from {}: {e}", path.display()))?,
        (fmt, bits) => {
            return Err(format!(
                "{}: unsupported sample format {fmt:?} at {bits} bits",
                path.display()
            ));
        }
    };

    let mono = match spec.channels {
        1 => interleaved,
        2 => interleaved
            .chunks_exact(2)
            .map(|frame| (frame[0] + frame[1]) / 2.0)
            .collect(),
        n => return Err(format!("{}: unsupported channel count {n}", path.display())),
    };

    if mono.is_empty() {
        return Err(format!("{} contained no samples", path.display()));
    }
    Ok(mono)
}

/// Split `samples` into consecutive slices of at most `chunk_len`, so the
/// smoke pushes audio incrementally the way the voice loop does (it feeds
/// VAD-frame-sized slices, not the whole utterance in one call). The final
/// chunk may be shorter. A `chunk_len` of 0 yields the whole slice as a
/// single chunk (slices cannot be chunked by 0).
fn chunk_samples(samples: &[f32], chunk_len: usize) -> Vec<&[f32]> {
    if chunk_len == 0 {
        return vec![samples];
    }
    samples.chunks(chunk_len).collect()
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    #[test]
    fn normalize_lowercases_strips_punctuation_and_collapses_whitespace() {
        assert_eq!(
            normalize_transcript("  The Sky, is BLUE! "),
            "the sky is blue"
        );
        assert_eq!(normalize_transcript("Hello\n\tworld"), "hello world");
    }

    #[test]
    fn normalize_empty_and_punctuation_only_become_empty() {
        assert_eq!(normalize_transcript(""), "");
        assert_eq!(normalize_transcript("  ,.!?  "), "");
    }

    #[test]
    fn normalize_keeps_alphanumerics_and_is_idempotent() {
        let once = normalize_transcript("Room 101 — it's cold.");
        assert_eq!(once, "room 101 its cold");
        // Re-normalising already-normalised text is a no-op.
        assert_eq!(normalize_transcript(&once), once);
    }

    #[test]
    fn chunk_splits_with_short_final_chunk() {
        let data: Vec<f32> = (0..10).map(|i| i as f32).collect();
        let chunks = chunk_samples(&data, 4);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], &[0.0, 1.0, 2.0, 3.0]);
        assert_eq!(chunks[1], &[4.0, 5.0, 6.0, 7.0]);
        assert_eq!(chunks[2], &[8.0, 9.0]);
    }

    #[test]
    fn chunk_zero_len_yields_whole_slice() {
        let data = [1.0_f32, 2.0, 3.0];
        let chunks = chunk_samples(&data, 0);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], &data[..]);
    }

    #[test]
    fn chunk_concatenation_round_trips() {
        let data: Vec<f32> = (0..23).map(|i| i as f32).collect();
        let rejoined: Vec<f32> = chunk_samples(&data, 5).concat();
        assert_eq!(rejoined, data);
    }

    /// Write a tiny WAV with the given spec, then load it back through
    /// [`load_wav_mono_16k`]. Returns the loader's result so each test can
    /// assert the happy or error path. No whisper model involved.
    fn round_trip(
        spec: hound::WavSpec,
        write: impl FnOnce(&mut hound::WavWriter<std::io::BufWriter<std::fs::File>>),
    ) -> Result<Vec<f32>, String> {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("clip.wav");
        let mut writer = hound::WavWriter::create(&path, spec).expect("create wav");
        write(&mut writer);
        writer.finalize().expect("finalize wav");
        load_wav_mono_16k(&path)
    }

    #[test]
    fn load_mono_16k_i16_round_trips_normalised_samples() {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: WHISPER_SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let samples = round_trip(spec, |w| {
            for v in [0_i16, i16::MAX, i16::MIN / 2] {
                w.write_sample(v).unwrap();
            }
        })
        .expect("load should succeed");
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[0], 0.0);
        assert!((samples[1] - 1.0).abs() < 1e-4, "i16::MAX maps to ~1.0");
        assert!(samples[2] < 0.0, "negative sample stays negative");
    }

    #[test]
    fn load_downmixes_stereo_to_mono() {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: WHISPER_SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        // Two stereo frames; each averages its L/R pair.
        let samples = round_trip(spec, |w| {
            for v in [i16::MAX, 0, 0, i16::MAX] {
                w.write_sample(v).unwrap();
            }
        })
        .expect("load should succeed");
        assert_eq!(samples.len(), 2, "stereo frames collapse to mono samples");
        assert!((samples[0] - 0.5).abs() < 1e-3);
        assert!((samples[1] - 0.5).abs() < 1e-3);
    }

    #[test]
    fn load_rejects_wrong_sample_rate() {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let err = round_trip(spec, |w| w.write_sample(0_i16).unwrap())
            .expect_err("8 kHz must be rejected");
        assert!(
            err.contains("16000 Hz"),
            "error names the required rate: {err}"
        );
    }

    #[test]
    fn load_rejects_empty_clip() {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: WHISPER_SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let err = round_trip(spec, |_| {}).expect_err("empty clip must be rejected");
        assert!(
            err.contains("no samples"),
            "error mentions emptiness: {err}"
        );
    }
}

#[cfg(feature = "whisper")]
mod reuse_smoke {
    //! The model-dependent smoke. Gated on the `whisper` feature (so the
    //! file still compiles + runs its pure-helper tests on a default
    //! build) and `#[ignore]`'d (so it never runs without an explicitly
    //! provided model + recordings).

    use super::{chunk_samples, load_wav_mono_16k, normalize_transcript};
    use primer_core::speech::StreamingSpeechToText;
    use primer_speech::WhisperStt;
    use std::path::Path;

    /// Samples per `push_audio` call. 1600 samples = 100 ms at 16 kHz —
    /// representative of the VAD-frame-sized slices the voice loop feeds,
    /// so the smoke exercises the incremental-`process_step` path rather
    /// than a single whole-utterance push. `WhisperStream` buffers
    /// internally, so any positive value is correctness-equivalent; this
    /// one just keeps the streaming cadence realistic.
    const PUSH_CHUNK_SAMPLES: usize = 1_600;

    /// Drive one full utterance through a fresh session: `open_session`
    /// (which either constructs or `reset()`-reuses the cached stream),
    /// incremental `push_audio`, then `finalize` (which returns the stream
    /// to the cache on success). Returns the concatenated segment text.
    fn transcribe_one(stt: &WhisperStt, samples: &[f32]) -> String {
        let mut session = stt.open_session().expect("open transcription session");
        let mut text = String::new();
        for chunk in chunk_samples(samples, PUSH_CHUNK_SAMPLES) {
            for seg in session.push_audio(chunk).expect("push_audio") {
                text.push_str(&seg.text);
            }
        }
        for seg in session.finalize().expect("finalize") {
            text.push_str(&seg.text);
        }
        text
    }

    /// Read a required env var as a path, or print a skip note and return
    /// `None` so the `#[ignore]`'d test no-ops gracefully when run without
    /// the fixtures wired up.
    fn fixture(var: &str) -> Option<std::path::PathBuf> {
        match std::env::var_os(var) {
            Some(v) => Some(std::path::PathBuf::from(v)),
            None => {
                eprintln!("skip: set {var} to run the WhisperStream reuse smoke");
                None
            }
        }
    }

    #[test]
    #[ignore = "needs PRIMER_WHISPER_MODEL + two 16 kHz mono WAV recordings (PRIMER_WHISPER_AUDIO_A/_B)"]
    fn reused_stream_does_not_bleed_prior_utterance() {
        let (Some(model), Some(audio_a), Some(audio_b)) = (
            fixture("PRIMER_WHISPER_MODEL"),
            fixture("PRIMER_WHISPER_AUDIO_A"),
            fixture("PRIMER_WHISPER_AUDIO_B"),
        ) else {
            return;
        };

        let samples_a = load_wav_mono_16k(Path::new(&audio_a)).expect("load utterance A");
        let samples_b = load_wav_mono_16k(Path::new(&audio_b)).expect("load utterance B");

        // Reference: utterance B transcribed COLD in its own backend
        // instance. A separate `WhisperStt` has its own empty cache, so the
        // `WhisperStream` is freshly constructed — never reused.
        let ref_b = {
            let stt = WhisperStt::new(&model).expect("load model (cold reference)");
            transcribe_one(&stt, &samples_b)
        };

        // Reuse path: ONE backend instance. Utterance A constructs + caches
        // the stream; utterance B then reuses it via `reset()`.
        let stt = WhisperStt::new(&model).expect("load model (reuse path)");
        let reuse_a = transcribe_one(&stt, &samples_a); // caches the stream
        let reuse_b = transcribe_one(&stt, &samples_b); // reuses cached stream

        assert!(
            !reuse_a.trim().is_empty(),
            "utterance A produced no transcript — check the recording / model"
        );
        assert!(
            !reuse_b.trim().is_empty(),
            "utterance B produced no transcript — check the recording / model"
        );

        assert_eq!(
            normalize_transcript(&reuse_b),
            normalize_transcript(&ref_b),
            "utterance B's transcript changed when the WhisperStream was reused \
             after utterance A. The preserved WhisperState bled prior-utterance \
             decoder/KV state into the fresh utterance — reset() is NOT \
             correctness-safe for reuse (#166).\n  reused B: {reuse_b:?}\n  cold  B: {ref_b:?}\n  (utterance A was: {reuse_a:?})"
        );
    }
}
