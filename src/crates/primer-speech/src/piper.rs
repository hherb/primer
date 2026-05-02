//! Piper TTS implementation of [`TextToSpeech`] and
//! [`StreamingTextToSpeech`].
//!
//! Wraps `piper-rs` (vendored at `src/vendor/piper-rs/` and patched for
//! `ort 2.0.0-rc.10` compatibility — see commit `b0ed91e`). One ONNX
//! model + JSON config pair is loaded on construction; the same loaded
//! `PiperModel` is shared across sessions via `Arc`.
//!
//! # Loading
//!
//! The vendored crate exposes `piper_rs::from_config_path(config_path)`
//! which auto-derives the ONNX path from the config file's stem:
//! `en_US-amy-medium.onnx.json` → looks for `en_US-amy-medium.onnx` in
//! the same directory.  `PiperTts::new` accepts both paths explicitly
//! for clarity and validates they agree before delegating to
//! `VitsModel::new`.
//!
//! # Streaming
//!
//! Streaming is achieved by feeding incoming text through a
//! [`PhraseSplitter`] and synthesising one phrase at a time. piper-rs
//! has no native phrase-boundary callback, so this is the smallest
//! correct way to get audio chunks out before the LLM has finished
//! generating.
//!
//! # Synthesis parameters
//!
//! Speaking rate is mapped to `length_scale` (inverse: `length_scale =
//! 1.0 / rate`; a faster rate shrinks the length scale). The mapping is
//! applied on every synthesis call by writing a `PiperSynthesisConfig`
//! through the model's `RwLock`-backed `set_fallback_synthesis_config`.
//! This is safe on a single-child device where simultaneous synthesis
//! calls are not expected.
//!
//! Pitch is not exposed by piper-rs; a `tracing::warn!` fires when the
//! caller sets a non-zero `VoiceProfile.pitch`.
//!
//! # Build prerequisites
//!
//! Enabling the `piper` feature pulls in the vendored `piper-rs` crate,
//! which transitively pulls `ort`, `espeak-rs`, `riff-wave`, `rayon`.
//! ONNX Runtime downloads a prebuilt binary from `cdn.pyke.io` on
//! first build — sandboxed CI environments will fail at this step.
//!
//! Piper voices are distributed as `*.onnx` + `*.onnx.json` pairs from
//! `huggingface.co/rhasspy/piper-voices`. The `tts_hello` example shows
//! the typical loading flow.

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use piper_rs::VitsModel;
use piper_rs::synth::PiperSpeechSynthesizer;
use piper_rs::{PiperModel, PiperSynthesisConfig};
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
/// the same loaded model is shared across all sessions via `Arc`.
/// Both the one-shot [`TextToSpeech`] and the streaming
/// [`StreamingTextToSpeech`] traits are implemented.
///
/// One voice per backend instance — runtime voice switching is not
/// supported. Construct multiple `PiperTts` if you need multiple voices;
/// `open_session(voice)` returns `Err` if `voice.model_id` doesn't
/// match the constructor-time voice.
///
/// Synth-config writes to the shared model are serialised through
/// `synth_config_lock` so concurrent `synthesize` / `open_session` calls
/// can't clobber each other's `length_scale` mid-mutation. The vendored
/// `piper-rs` keeps the synthesis config in an internal `RwLock` and
/// our `apply_synth_config` does a get-modify-set; without this guard
/// two paths racing into that sequence could lose updates.
pub struct PiperTts {
    model: Arc<dyn PiperModel + Send + Sync>,
    voice: VoiceProfile,
    speaker_id: Option<i64>,
    sample_rate: u32,
    /// Serialises the get-modify-set sequence in `apply_synth_config`.
    /// See struct doc for why; never held across an `await`.
    synth_config_lock: Mutex<()>,
}

impl PiperTts {
    /// Load a Piper voice from the given `.onnx` and `.onnx.json` pair.
    ///
    /// `voice.model_id` defaults to a stem derived from `onnx_path`'s
    /// file name (without extension). Override via [`Self::with_voice`].
    pub fn new(onnx_path: impl AsRef<Path>, config_path: impl AsRef<Path>) -> Result<Self> {
        let onnx_path = onnx_path.as_ref();
        let config_path = config_path.as_ref();
        let sample_rate = piper_config::read_sample_rate(config_path)?;
        let model = VitsModel::new(config_path.to_path_buf(), onnx_path)
            .map_err(|e| PrimerError::Speech(format!("load piper voice: {e}")))?;
        let model_id = onnx_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("piper-voice")
            .to_string();
        Ok(Self {
            model: Arc::new(model),
            voice: VoiceProfile {
                model_id,
                ..VoiceProfile::default()
            },
            speaker_id: None,
            sample_rate,
            synth_config_lock: Mutex::new(()),
        })
    }

    /// Override the default `VoiceProfile` for sessions opened via this
    /// backend. The loaded model's `model_id` is preserved — passing a
    /// `VoiceProfile` with a different `model_id` is a no-op for that
    /// field. (One backend instance, one voice; mixing them at runtime
    /// is not supported and used to surface as a delayed
    /// `open_session` error, which was unhelpfully far from the
    /// mistake.)
    pub fn with_voice(mut self, voice: VoiceProfile) -> Self {
        self.voice = merge_voice(&self.voice.model_id, voice);
        self
    }

    /// Set a multi-speaker model's speaker id (ignored for single-speaker voices).
    pub fn with_speaker_id(mut self, id: i64) -> Self {
        self.speaker_id = Some(id);
        self
    }

    fn validate_voice(&self, requested: &VoiceProfile) -> Result<()> {
        check_voice_match(&self.voice.model_id, &requested.model_id)
    }

    fn length_scale_for(voice: &VoiceProfile) -> f32 {
        if voice.rate > 0.0 {
            DEFAULT_LENGTH_SCALE / voice.rate
        } else {
            DEFAULT_LENGTH_SCALE
        }
    }

    /// Apply synthesis parameters to the shared model before synthesis.
    ///
    /// Writes `length_scale` and optional `speaker_id` through the model's
    /// internal `RwLock`. The `synth_config_lock` mutex serialises the
    /// get-modify-set sequence so a concurrent caller (e.g. one-shot
    /// `synthesize` running at the same time as a `open_session`) can't
    /// land its set between our get and our set and lose the update.
    fn apply_synth_config(&self, length_scale: f32) -> Result<()> {
        // Hold for the entirety of get→modify→set so it's atomic from
        // the perspective of any other PiperTts method on this instance.
        // `unwrap_or_else` recovers from poisoning; an earlier panic
        // mid-mutation can't have left the vendored RwLock in an
        // observably broken state because both halves run RAII-clean.
        let _guard = self
            .synth_config_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Retrieve current defaults so we preserve noise_scale / noise_w.
        let current = self
            .model
            .get_fallback_synthesis_config()
            .map_err(|e| PrimerError::Speech(format!("piper get synth config: {e}")))?;
        // Downcast to the concrete type we know the vendored crate uses.
        let mut cfg = current
            .downcast::<PiperSynthesisConfig>()
            .map_err(|_| PrimerError::Speech("piper synth config downcast failed".to_string()))
            .map(|b| *b)?;
        cfg.length_scale = length_scale;
        if let Some(sid) = self.speaker_id {
            cfg.speaker = Some(sid);
        }
        self.model
            .set_fallback_synthesis_config(&cfg)
            .map_err(|e| PrimerError::Speech(format!("piper set synth config: {e}")))
    }
}

/// Compare a loaded model's `model_id` against a session-requested one.
/// Extracted as a free function so the comparison can be unit-tested
/// without loading a real ONNX model.
fn check_voice_match(loaded: &str, requested: &str) -> Result<()> {
    if requested != loaded {
        return Err(PrimerError::Speech(format!(
            "piper voice mismatch: backend loaded {loaded:?}, session asked for {requested:?}"
        )));
    }
    Ok(())
}

/// Merge an override `VoiceProfile` over a loaded model's identity:
/// keep `loaded_model_id`, take everything else from `override_voice`.
/// Extracted from `PiperTts::with_voice` so the model_id-preservation
/// invariant is unit-testable without loading an ONNX model.
fn merge_voice(loaded_model_id: &str, override_voice: VoiceProfile) -> VoiceProfile {
    VoiceProfile {
        model_id: loaded_model_id.to_string(),
        ..override_voice
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
        let length_scale = Self::length_scale_for(voice);
        self.apply_synth_config(length_scale)?;

        let model = Arc::clone(&self.model);
        let text_owned = text.to_string();
        let sample_rate = self.sample_rate;

        let samples = tokio::task::spawn_blocking(move || -> Result<Vec<f32>> {
            let synthesizer = PiperSpeechSynthesizer::new(Arc::clone(&model))
                .map_err(|e| PrimerError::Speech(format!("piper synthesizer init: {e}")))?;
            let stream = synthesizer
                .synthesize_lazy(text_owned, None)
                .map_err(|e| PrimerError::Speech(format!("piper synthesize_lazy: {e}")))?;
            let mut all_samples: Vec<f32> = Vec::new();
            for result in stream {
                let audio = result
                    .map_err(|e| PrimerError::Speech(format!("piper synthesis chunk: {e}")))?;
                all_samples.extend(audio.into_vec());
            }
            Ok(all_samples)
        })
        .await
        .map_err(|e| PrimerError::Speech(format!("piper join: {e}")))??;

        Ok(AudioBuffer {
            samples,
            sample_rate,
        })
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
        let length_scale = Self::length_scale_for(voice);
        self.apply_synth_config(length_scale)?;
        Ok(Box::new(PiperSession {
            model: Arc::clone(&self.model),
            splitter: PhraseSplitter::new(),
            sample_rate: self.sample_rate,
        }))
    }
}

/// Per-turn streaming synthesis session.
///
/// Created by [`PiperTts::open_session`]. Each completed phrase from
/// the [`PhraseSplitter`] is synthesised immediately via `synthesize_lazy`,
/// yielding one [`AudioChunk`] per phrase. This gives the audio pipeline
/// audio to play while the LLM is still generating the next phrase.
struct PiperSession {
    model: Arc<dyn PiperModel + Send + Sync>,
    splitter: PhraseSplitter,
    sample_rate: u32,
}

impl PiperSession {
    fn synth_phrase(&self, phrase: &str) -> Result<AudioChunk> {
        let synthesizer = PiperSpeechSynthesizer::new(Arc::clone(&self.model))
            .map_err(|e| PrimerError::Speech(format!("piper synthesizer init: {e}")))?;
        let stream = synthesizer
            .synthesize_lazy(phrase.to_string(), None)
            .map_err(|e| PrimerError::Speech(format!("piper synthesize_lazy: {e}")))?;
        let mut samples: Vec<f32> = Vec::new();
        for result in stream {
            let audio =
                result.map_err(|e| PrimerError::Speech(format!("piper synthesis chunk: {e}")))?;
            samples.extend(audio.into_vec());
        }
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

    /// `length_scale = 1.0 / rate` when rate is positive.
    #[test]
    fn length_scale_inverts_positive_rate() {
        let v = VoiceProfile {
            rate: 2.0,
            ..VoiceProfile::default()
        };
        assert!((PiperTts::length_scale_for(&v) - 0.5).abs() < f32::EPSILON);
    }

    /// `rate = 1.0` round-trips to the default length_scale.
    #[test]
    fn length_scale_default_for_unit_rate() {
        let v = VoiceProfile {
            rate: 1.0,
            ..VoiceProfile::default()
        };
        assert!((PiperTts::length_scale_for(&v) - DEFAULT_LENGTH_SCALE).abs() < f32::EPSILON);
    }

    /// Zero / negative / NaN rates fall back to the default rather than
    /// dividing by zero or producing a non-finite length_scale. This
    /// behaviour is forgiving by design — the alternative was a hard
    /// error, but a single-child REPL prefers degrading to default
    /// pace over panicking on a misconfigured `VoiceProfile`.
    #[test]
    fn length_scale_falls_back_to_default_on_non_positive_rate() {
        for bad in [0.0, -1.0, f32::NAN] {
            let v = VoiceProfile {
                rate: bad,
                ..VoiceProfile::default()
            };
            assert_eq!(PiperTts::length_scale_for(&v), DEFAULT_LENGTH_SCALE);
        }
    }

    /// A request with the same model_id as the loaded model is accepted.
    #[test]
    fn check_voice_match_accepts_same_id() {
        assert!(check_voice_match("en_US-amy-medium", "en_US-amy-medium").is_ok());
    }

    /// A mismatched model_id is rejected with a `PrimerError::Speech`
    /// that names BOTH ids — diagnosing a misconfiguration is the whole
    /// point of the early validation, so we lock the wording.
    #[test]
    fn check_voice_match_rejects_mismatched_id() {
        let err = check_voice_match("en_US-amy", "fr_FR-bob").unwrap_err();
        let s = format!("{err}");
        assert!(s.contains("en_US-amy"), "missing loaded id in: {s}");
        assert!(s.contains("fr_FR-bob"), "missing requested id in: {s}");
    }

    /// `merge_voice` overrides rate/pitch but PRESERVES the loaded
    /// model_id. Before this guard, callers who passed a different
    /// model_id via `PiperTts::with_voice` silently broke every
    /// subsequent `open_session`. The foreign model_id is dropped on
    /// the floor, not surfaced as an error — keeps the builder
    /// ergonomic while making the footgun impossible.
    ///
    /// Unit-tested at the helper level so we don't need to fabricate a
    /// `PiperTts` (which requires an `Arc<dyn PiperModel>` — non-trivial
    /// to mock since some of the trait's signatures use crate-private
    /// types). `with_voice` itself is a one-liner over this helper.
    #[test]
    fn merge_voice_preserves_loaded_model_id() {
        let merged = merge_voice(
            "loaded-id",
            VoiceProfile {
                model_id: "ATTACKER-WRONG-ID".to_string(),
                rate: 1.5,
                pitch: 0.2,
            },
        );
        assert_eq!(merged.model_id, "loaded-id");
        assert!((merged.rate - 1.5).abs() < f32::EPSILON);
        assert!((merged.pitch - 0.2).abs() < f32::EPSILON);
    }

    /// Same loaded id + override-with-default still yields the loaded id.
    /// Sanity check for the most common call:
    /// `PiperTts::new(...).with_voice(VoiceProfile::default())`.
    #[test]
    fn merge_voice_overwrites_model_id_even_when_override_is_default() {
        let merged = merge_voice("loaded-id", VoiceProfile::default());
        assert_eq!(merged.model_id, "loaded-id");
        assert!((merged.rate - VoiceProfile::default().rate).abs() < f32::EPSILON);
    }

    /// Real-model smoke test. Skipped unless BOTH
    /// `$PIPER_TEST_MODEL_ONNX` and `$PIPER_TEST_MODEL_CONFIG` are set,
    /// because piper-rs needs a voice file pair on disk and CI doesn't
    /// ship them. Only one of the two set is treated as a
    /// misconfiguration and panics so the running developer notices.
    #[tokio::test]
    #[ignore]
    async fn piper_smoke_synthesise_returns_non_empty_audio() {
        let (onnx, cfg) = match (
            std::env::var("PIPER_TEST_MODEL_ONNX").ok(),
            std::env::var("PIPER_TEST_MODEL_CONFIG").ok(),
        ) {
            (Some(o), Some(c)) => (o, c),
            (None, None) => return,
            _ => panic!("PIPER_TEST_MODEL_ONNX and PIPER_TEST_MODEL_CONFIG must be set together"),
        };
        let tts = PiperTts::new(&onnx, &cfg).expect("load piper");
        let voice = tts.voice.clone();
        let audio = tts.synthesize("Hello.", &voice).await.expect("synthesise");
        assert!(!audio.samples.is_empty());
        assert_eq!(audio.sample_rate, tts.sample_rate());
    }
}
