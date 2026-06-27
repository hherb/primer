//! Supertonic TTS implementation of [`TextToSpeech`] and
//! [`StreamingTextToSpeech`].
//!
//! Wraps the vendored `supertonic-tts` crate (at `src/vendor/supertonic-rs/`,
//! upstream commit `67175af`, patched for `ort 2.0.0-rc.10` compatibility).
//! Four ONNX sessions (duration predictor, text encoder, vector estimator,
//! vocoder) load on construction; the same loaded
//! [`HelperTts`](supertonic_tts::helper::TextToSpeech) is shared across all
//! sessions via `Arc<Mutex<…>>`. The mutex is required because the helper's
//! `call` takes `&mut self` (ort's `Session::run` is `&mut` in rc.10) — but
//! the four sessions themselves load once and are reused for every utterance,
//! so the cold-start cost (≈ tens-of-MB allocations + ORT graph init) is paid
//! once at `SupertonicTts::new`.
//!
//! # Streaming
//!
//! Streaming is achieved by feeding incoming text through a [`PhraseSplitter`]
//! and synthesising one phrase at a time. Each completed phrase invokes the
//! helper's `call(text, lang, style, total_steps, speed, silence_s)` and yields
//! one [`AudioChunk`] + a `PhraseEnd` marker. The 200 ms inter-phrase silence
//! the consumer inserts on `PhraseEnd` lives in
//! [`primer_core::consts::speech::DEFAULT_INTER_PHRASE_SILENCE_MS`]; the
//! `silence_s` argument we pass to `call` is the *intra-phrase* gap for the
//! helper's own internal auto-chunking (it splits inputs over ~300 chars), so
//! we pass `0.0` — phrase-level rhythm is the consumer's job, not the
//! synthesiser's.
//!
//! # Language
//!
//! `VoiceProfile` doesn't carry a language field (Piper voices are
//! locale-specific; the language is implicit in the loaded `.onnx.json`'s
//! phoneme map). Supertonic is multilingual — one model speaks 31 languages —
//! so the language is set at backend-construction via [`Self::with_language`]
//! and reused for every utterance from that instance. The voice loop's
//! `locale_defaults` wires this from `Locale::pack_id()`.
//!
//! # Synthesis parameters
//!
//! `VoiceProfile.rate` maps to upstream `speed` (direct, not inverted — a
//! `rate > 1.0` is faster). `VoiceProfile.pitch` is not exposed by the helper
//! and produces a `tracing::warn!` when non-zero, matching the Piper backend's
//! pitch warning.
//!
//! # Resampling
//!
//! Supertonic emits at 44.1 kHz. The voice loop's shared output pipeline
//! (`backends_common::make_on_audio`) already handles the 44.1 → device-rate
//! resample with the FFT-tail flush sentinel — the backend itself doesn't
//! need to re-derive it.
//!
//! # Build prerequisites
//!
//! Enabling the `supertonic` feature pulls in the vendored `supertonic-tts`
//! crate, which transitively pulls `ort`. ONNX Runtime downloads a prebuilt
//! binary from `cdn.pyke.io` on first build — sandboxed CI environments will
//! fail at this step.
//!
//! Supertonic 3 voices are distributed from `huggingface.co/Supertone/supertonic-3`
//! as a single `~400 MB` asset bundle (four `*.onnx` files +
//! `tts.json` + `unicode_indexer.json`) plus per-voice `voice_styles/*.json`
//! style descriptors. The `tts_supertonic_hello` example shows the loading flow.

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use primer_core::error::{PrimerError, Result};
use primer_core::speech::{
    AudioBuffer, AudioChunk, Named, StreamingTextToSpeech, SynthesisEvent, SynthesisSession,
    TextToSpeech, VoiceProfile,
};
use supertonic_tts::helper::{
    Style, TextToSpeech as HelperTts, load_text_to_speech, load_voice_style,
};

use crate::phrase_split::PhraseSplitter;

/// Backend identifier returned by [`Named::name`].
const BACKEND_NAME: &str = "supertonic";

/// Denoising-step count passed to the helper. Upstream quality knob,
/// recommended range 5..=12; example default is 8. Trading more steps for
/// audible quality is a per-device decision deferred to Stage E's A/B
/// numbers (issue #170); 8 is the conservative middle for now.
const DEFAULT_TOTAL_STEPS: usize = 8;

/// Lower sanity bound for [`SupertonicTts::with_total_steps`]. `0` steps
/// would mean no denoising at all (degenerate output), so the floor is 1.
const MIN_TOTAL_STEPS: usize = 1;

/// Upper sanity bound for [`SupertonicTts::with_total_steps`]. Well above
/// the recommended 5..=12 quality range so deliberate quality overrides
/// pass through, but capped so a typo can't burn arbitrary compute.
const MAX_TOTAL_STEPS: usize = 32;

/// Default ISO-639-1 language tag. The voice loop overrides this via
/// [`SupertonicTts::with_language`] from `Locale::pack_id()`.
const DEFAULT_LANGUAGE: &str = "en";

/// Inter-internal-chunk silence (seconds) passed to `helper::call`. The
/// helper auto-chunks input text over ~300 chars and inserts this gap
/// between internal chunks. Phrase-splitting upstream usually keeps each
/// `call` invocation well under that limit, so this rarely triggers, but
/// `0.0` ensures we don't accidentally double up on the consumer's own
/// inter-phrase pause when the helper does chunk further.
const HELPER_INTRA_PHRASE_SILENCE_S: f32 = 0.0;

/// Supertonic text-to-speech backend.
///
/// Loads one Supertonic voice (asset directory + voice-style JSON) on
/// construction; the same loaded model is shared across all sessions via
/// `Arc<Mutex<…>>`. Both the one-shot [`TextToSpeech`] and the streaming
/// [`StreamingTextToSpeech`] traits are implemented.
///
/// One voice per backend instance — runtime voice switching is not
/// supported (mirrors `PiperTts`). Construct multiple `SupertonicTts` if
/// you need multiple voices; `open_session(voice)` returns `Err` if
/// `voice.model_id` doesn't match the constructor-time voice.
pub struct SupertonicTts {
    inner: Arc<Mutex<HelperTts>>,
    style: Arc<Style>,
    voice: VoiceProfile,
    language: String,
    sample_rate: u32,
    total_steps: usize,
}

impl SupertonicTts {
    /// Load a Supertonic voice from the asset directory and voice-style file.
    ///
    /// `onnx_dir` must contain `duration_predictor.onnx`, `text_encoder.onnx`,
    /// `vector_estimator.onnx`, `vocoder.onnx`, `tts.json`, and
    /// `unicode_indexer.json` (the standard Supertonic asset layout).
    /// `voice_style_path` points at e.g. `voice_styles/F1.json`.
    ///
    /// `voice.model_id` defaults to `"supertonic-<stem>"` where `<stem>` is
    /// the voice-style file's stem. Override via [`Self::with_voice`].
    pub fn new(onnx_dir: impl AsRef<Path>, voice_style_path: impl AsRef<Path>) -> Result<Self> {
        let onnx_dir = onnx_dir.as_ref();
        let voice_style_path = voice_style_path.as_ref();
        let onnx_dir_str = onnx_dir.to_str().ok_or_else(|| {
            PrimerError::Speech("supertonic onnx_dir must be valid UTF-8".to_string())
        })?;
        let voice_style_str = voice_style_path.to_str().ok_or_else(|| {
            PrimerError::Speech("supertonic voice_style path must be valid UTF-8".to_string())
        })?;
        let inner = load_text_to_speech(onnx_dir_str, /* use_gpu */ false)
            .map_err(|e| PrimerError::Speech(format!("load supertonic models: {e}")))?;
        let sample_rate = u32::try_from(inner.sample_rate).map_err(|_| {
            PrimerError::Speech(format!(
                "supertonic sample_rate out of range: {}",
                inner.sample_rate
            ))
        })?;
        let style = load_voice_style(&[voice_style_str.to_string()], /* verbose */ false)
            .map_err(|e| PrimerError::Speech(format!("load supertonic voice style: {e}")))?;
        let model_id = derive_model_id(voice_style_path);
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
            style: Arc::new(style),
            voice: VoiceProfile {
                model_id,
                ..VoiceProfile::default()
            },
            language: DEFAULT_LANGUAGE.to_string(),
            sample_rate,
            total_steps: DEFAULT_TOTAL_STEPS,
        })
    }

    /// Override the default `VoiceProfile` for sessions opened via this
    /// backend. The loaded model's `model_id` is preserved — passing a
    /// `VoiceProfile` with a different `model_id` is a no-op for that
    /// field (matches Piper's `with_voice` semantics, same footgun guard).
    pub fn with_voice(mut self, voice: VoiceProfile) -> Self {
        self.voice = merge_voice(&self.voice.model_id, voice);
        self
    }

    /// Set the synthesis language (ISO-639-1; default `"en"`). The voice
    /// loop wires this from `Locale::pack_id()` so the backend speaks the
    /// session's locale.
    pub fn with_language(mut self, lang: impl Into<String>) -> Self {
        self.language = lang.into();
        self
    }

    /// Override the denoising step count (default
    /// [`DEFAULT_TOTAL_STEPS`] = 8). Upstream treats this as a free quality
    /// knob — more steps trade compute for fidelity — with a *recommended*
    /// range of 5..=12; the vendored helper neither validates nor rejects
    /// values outside it, so a developer may legitimately push past 12 for
    /// quality. The value is clamped to
    /// [`MIN_TOTAL_STEPS`]..=[`MAX_TOTAL_STEPS`] purely as a sanity guard:
    /// `0` would mean no denoising at all (degenerate output) and an
    /// unbounded upper end could waste arbitrary compute on a typo. Values
    /// inside the guard but outside the recommended range are honoured as
    /// deliberate overrides, not silently snapped to 5..=12.
    pub fn with_total_steps(mut self, steps: usize) -> Self {
        self.total_steps = clamp_total_steps(steps);
        self
    }

    fn validate_voice(&self, requested: &VoiceProfile) -> Result<()> {
        check_voice_match(&self.voice.model_id, &requested.model_id)
    }
}

/// Derive a `model_id` from the voice-style path's file stem. Falls back
/// to `"supertonic-voice"` if the path has no usable stem, so the field
/// is always populated. Extracted so the derivation can be unit-tested
/// without loading ONNX models.
fn derive_model_id(voice_style_path: &Path) -> String {
    voice_style_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| format!("supertonic-{s}"))
        .unwrap_or_else(|| "supertonic-voice".to_string())
}

/// Compare a loaded model's `model_id` against a session-requested one.
/// Extracted as a free function so the comparison can be unit-tested
/// without loading the four ONNX sessions.
fn check_voice_match(loaded: &str, requested: &str) -> Result<()> {
    if requested != loaded {
        return Err(PrimerError::Speech(format!(
            "supertonic voice mismatch: backend loaded {loaded:?}, session asked for {requested:?}"
        )));
    }
    Ok(())
}

/// Merge an override `VoiceProfile` over a loaded model's identity:
/// keep `loaded_model_id`, take everything else from `override_voice`.
/// Mirrors `piper::merge_voice` so the model_id-preservation invariant
/// is identical across TTS backends.
fn merge_voice(loaded_model_id: &str, override_voice: VoiceProfile) -> VoiceProfile {
    VoiceProfile {
        model_id: loaded_model_id.to_string(),
        ..override_voice
    }
}

/// Map `VoiceProfile.rate` to upstream `speed`. Supertonic's `speed`
/// argument is direct (>1.0 = faster) — unlike Piper's `length_scale`
/// which inverts. Non-finite or non-positive rates fall back to `1.0`
/// rather than dividing into a degenerate value or panicking — a
/// single-child REPL prefers degrading to default pace over crashing on
/// a misconfigured `VoiceProfile`.
fn speed_for(voice: &VoiceProfile) -> f32 {
    if voice.rate.is_finite() && voice.rate > 0.0 {
        voice.rate
    } else {
        1.0
    }
}

/// Clamp a requested denoising-step count to the sanity guard
/// [`MIN_TOTAL_STEPS`]..=[`MAX_TOTAL_STEPS`]. This is *not* a snap to the
/// recommended 5..=12 quality range — values in between are honoured as
/// deliberate overrides; the guard only rules out the degenerate `0` and
/// runaway upper end. Extracted as a free function so the boundary
/// behaviour is unit-testable without loading the four ONNX sessions.
fn clamp_total_steps(steps: usize) -> usize {
    steps.clamp(MIN_TOTAL_STEPS, MAX_TOTAL_STEPS)
}

impl Named for SupertonicTts {
    fn name(&self) -> &str {
        BACKEND_NAME
    }
}

#[async_trait]
impl TextToSpeech for SupertonicTts {
    async fn synthesize(&self, text: &str, voice: &VoiceProfile) -> Result<AudioBuffer> {
        self.validate_voice(voice)?;
        warn_on_pitch(voice);
        let inner = Arc::clone(&self.inner);
        let style = Arc::clone(&self.style);
        let language = self.language.clone();
        let text_owned = text.to_string();
        let speed = speed_for(voice);
        let total_steps = self.total_steps;
        let sample_rate = self.sample_rate;

        let samples = tokio::task::spawn_blocking(move || -> Result<Vec<f32>> {
            let mut guard = inner.lock().unwrap_or_else(|p| p.into_inner());
            let (wav, _duration_s) = guard
                .call(
                    &text_owned,
                    &language,
                    &style,
                    total_steps,
                    speed,
                    HELPER_INTRA_PHRASE_SILENCE_S,
                )
                .map_err(|e| PrimerError::Speech(format!("supertonic synthesize: {e}")))?;
            Ok(wav)
        })
        .await
        .map_err(|e| PrimerError::Speech(format!("supertonic join: {e}")))??;

        Ok(AudioBuffer {
            samples,
            sample_rate,
        })
    }
}

impl StreamingTextToSpeech for SupertonicTts {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn open_session(&self, voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>> {
        self.validate_voice(voice)?;
        warn_on_pitch(voice);
        Ok(Box::new(SupertonicSession {
            inner: Arc::clone(&self.inner),
            style: Arc::clone(&self.style),
            splitter: PhraseSplitter::new(),
            language: self.language.clone(),
            speed: speed_for(voice),
            total_steps: self.total_steps,
            sample_rate: self.sample_rate,
        }))
    }
}

fn warn_on_pitch(voice: &VoiceProfile) {
    if voice.pitch != 0.0 {
        tracing::warn!(
            pitch = voice.pitch,
            "supertonic backend ignores VoiceProfile.pitch (no upstream knob)"
        );
    }
}

/// Per-turn streaming synthesis session.
///
/// Created by [`SupertonicTts::open_session`]. Each completed phrase from
/// the [`PhraseSplitter`] is synthesised immediately via the shared
/// `HelperTts::call`, yielding one [`AudioChunk`] + `PhraseEnd` marker per
/// phrase. The mutex protecting the inner helper is uncontended in the
/// single-active-session voice-loop invariant; concurrent sessions would
/// serialise on it, which is correct (ORT graphs aren't `Send`-safe to
/// run concurrently against the same session) but undesirable for
/// throughput. For now this matches the voice loop's one-utterance-at-a-time
/// shape.
struct SupertonicSession {
    inner: Arc<Mutex<HelperTts>>,
    style: Arc<Style>,
    splitter: PhraseSplitter,
    language: String,
    speed: f32,
    total_steps: usize,
    sample_rate: u32,
}

impl SupertonicSession {
    fn synth_phrase(&self, phrase: &str) -> Result<AudioChunk> {
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let (wav, _duration_s) = guard
            .call(
                phrase,
                &self.language,
                &self.style,
                self.total_steps,
                self.speed,
                HELPER_INTRA_PHRASE_SILENCE_S,
            )
            .map_err(|e| PrimerError::Speech(format!("supertonic synth phrase: {e}")))?;
        Ok(AudioChunk {
            samples: wav,
            sample_rate: self.sample_rate,
        })
    }
}

impl SynthesisSession for SupertonicSession {
    fn push_text(&mut self, text: &str, on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
        for phrase in self.splitter.push(text) {
            let chunk = self.synth_phrase(&phrase)?;
            on_event(SynthesisEvent::Audio(chunk));
            on_event(SynthesisEvent::PhraseEnd);
        }
        Ok(())
    }

    fn finalize(mut self: Box<Self>, on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
        if let Some(trailing) = self.splitter.flush() {
            let chunk = self.synth_phrase(&trailing)?;
            on_event(SynthesisEvent::Audio(chunk));
            on_event(SynthesisEvent::PhraseEnd);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
