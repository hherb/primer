//! CLI adapter for the shared `primer_speech::voice_loop`.
//!
//! Builds [`LoopBackends`] + [`LoopConfig`] from clap flags, instantiates
//! a [`StdoutObserver`] that preserves the existing CLI print formatting,
//! and calls `primer_speech::voice_loop::run_loop_borrowed` (the
//! borrowed-`&mut DialogueManager` variant; the CLI owns the DM directly
//! so no Arc<Mutex<>> is needed here).
//!
//! CLI-specific glue kept here (not lifted to primer-speech):
//!   - `DialogueResponder` — depends on primer-pedagogy::DialogueManager
//!   - `ChannelStt` / `run_audio_thread` — owns cpal mic I/O
//!   - `StdoutObserver` — CLI stdout/stderr formatting

mod audio_thread;
mod dialogue_responder;
pub mod stdout_observer;

use std::path::Path;
use std::sync::Arc;

use primer_core::error::Result;
use primer_core::speech::StreamingTextToSpeech;
#[cfg(feature = "speech")]
use primer_core::speech::VoiceActivityDetector;
use primer_pedagogy::DialogueManager;
use primer_speech::voice_loop::{LoopBackends, run_loop_borrowed};
use primer_speech::voice_loop::state_machine::VAD_EVENT_CHANNEL_CAPACITY;

pub use stdout_observer::StdoutObserver;

/// Configuration passed into [`run`] from `main`. Same shape as the
/// pre-refactor `SpeechLoopConfig` so the call site in main.rs needs
/// no changes beyond the import path.
pub struct SpeechLoopConfig<'a> {
    pub whisper_model: &'a Path,
    pub voice_onnx: &'a Path,
    pub voice_config: &'a Path,
    pub voice_id: &'a str,
    pub mic_silence_ms: u32,
    pub verbose: bool,
    /// Active locale for TTS dispatch. Today's CLI binds this to the
    /// resolved `--language` once and uses the same locale for the
    /// whole session.
    pub locale: primer_core::i18n::Locale,
}

/// CLI entry point for `--speech` mode.
///
/// Builds the speech backends (cpal mic + speaker, silero VAD, whisper
/// streaming STT, piper TTS), instantiates a [`StdoutObserver`], and
/// calls [`primer_speech::voice_loop::run_loop_borrowed`] (the
/// borrowed-`&mut DialogueManager` variant; the CLI owns the DM
/// directly so no Arc<Mutex<>> is needed here).
#[cfg(feature = "speech")]
pub async fn run(cfg: SpeechLoopConfig<'_>, dialogue: &mut DialogueManager) -> Result<()> {
    use primer_core::speech::VadEvent;
    use primer_speech::{
        MicCapture, PiperTts, Resampler, SileroVad, SileroVadParams, SpeakerSink, WhisperStt,
    };

    // ── Build VAD (real, lives in audio thread) ─────────────────────
    let vad_params = SileroVadParams {
        min_silence_ms: cfg.mic_silence_ms,
        ..SileroVadParams::default()
    };
    let mut audio_vad = SileroVad::new(vad_params)?;

    // ── Build STT (real whisper.cpp; the streaming session lives in
    //    the audio thread; run_loop_borrowed sees a ChannelStt wrapper)
    let whisper = Arc::new(WhisperStt::new(cfg.whisper_model)?);

    // ── Build TTS (real piper) ────────────────────────────────────
    // PIPER_ESPEAKNG_DATA_DIRECTORY is probed and set by `main()` before
    // the tokio runtime starts (see `probe_espeak_ng_data`). Doing it here
    // would be unsound — the runtime worker threads already exist.
    let tts: Arc<dyn StreamingTextToSpeech> =
        Arc::new(PiperTts::new(cfg.voice_onnx, cfg.voice_config)?);
    let tts_sample_rate = tts.sample_rate();

    // ── Open mic ──────────────────────────────────────────────────
    let (mic, mic_cons) = MicCapture::start()?;
    let mic_rate = mic.sample_rate;
    if cfg.verbose {
        eprintln!(
            "[speech] mic opened: {}Hz, {} channels",
            mic_rate, mic.channels
        );
    }

    // ── Open speaker ──────────────────────────────────────────────
    let (spk, spk_prod) = SpeakerSink::start()?;
    let spk_rate = spk.sample_rate;
    // Shared "stream errored" flag — set by cpal's err_callback if the
    // output device disappears mid-session. The on_audio retry loop
    // checks this so it bails instead of spinning forever on a dead
    // stream where push_slice will return 0 indefinitely.
    let spk_errored = spk.errored_flag();
    // Wrap the producer in `Arc<Mutex<>>` so it can be observed both
    // from the on_audio callback (push samples) and from the drain
    // hook (read `occupied_len`). The mutex is uncontended in practice
    // — on_audio runs synchronously from the voice loop, and the drain
    // hook is invoked only after on_audio's flush sentinel returns,
    // so push and observe never overlap. Cost: one uncontended lock
    // acquire per phrase, negligible at TTS rates.
    let spk_prod = Arc::new(std::sync::Mutex::new(spk_prod));
    if cfg.verbose {
        eprintln!(
            "[speech] speaker opened: {}Hz, {} channels",
            spk_rate, spk.channels
        );
    }

    // ── Input resampler: mic_rate → 16 kHz for VAD/whisper ─────────
    // We want each Resampler::process call to yield exactly
    // CHUNK_SAMPLES (512) at 16 kHz. Pick the input chunk size as
    // 512 * mic_rate / 16_000 (e.g. 1536 at 48 kHz).
    let vad_rate = audio_vad.sample_rate();
    let vad_chunk = audio_vad.chunk_samples();
    let in_chunk_samples: usize = (vad_chunk as u64 * mic_rate as u64 / vad_rate as u64) as usize;
    let mut input_resampler: Option<Resampler> = if mic_rate != vad_rate {
        Some(Resampler::new(mic_rate, vad_rate, in_chunk_samples)?)
    } else {
        None
    };

    // ── Output resampler factory: piper_rate → spk_rate, lazy
    //    constructed inside the on_audio callback so chunk sizing is
    //    decided once (per chunk size). For simplicity we resample with a
    //    fixed input chunk size that we pad/truncate the TTS chunk to.
    let need_output_resample = tts_sample_rate != spk_rate;
    let output_chunk_in: usize = 1024;
    let output_resampler = Arc::new(std::sync::Mutex::new(if need_output_resample {
        Some(Resampler::new(tts_sample_rate, spk_rate, output_chunk_in)?)
    } else {
        None
    }));

    // ── Channels ──────────────────────────────────────────────────
    let (event_tx, event_rx) =
        tokio::sync::mpsc::channel::<VadEvent>(VAD_EVENT_CHANNEL_CAPACITY);
    let (transcript_tx, transcript_rx) = std::sync::mpsc::channel::<String>();
    // Cancellation flag — set when the voice loop exits to tell the
    // audio thread to stop.
    let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Speaking gate — set by the voice loop's SPEAK phase, checked by
    // the audio thread. While true, the audio thread drains the mic
    // ringbuf but discards the samples (no VAD, no whisper push). This
    // is the anti-feedback guard.
    let is_speaking = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // ── Spawn audio capture thread ─────────────────────────────────
    // Owns: mic_cons, input_resampler, audio_vad, whisper (Arc),
    //       event_tx, transcript_tx, stop_flag, is_speaking.
    let whisper_for_thread = Arc::clone(&whisper);
    let stop_flag_thread = Arc::clone(&stop_flag);
    let is_speaking_thread = Arc::clone(&is_speaking);
    let audio_thread = std::thread::Builder::new()
        .name("primer-speech-audio".into())
        .spawn(move || {
            audio_thread::run_audio_thread(
                mic_cons,
                input_resampler.take(),
                in_chunk_samples,
                vad_chunk,
                &mut audio_vad,
                whisper_for_thread,
                event_tx,
                transcript_tx,
                stop_flag_thread,
                is_speaking_thread,
            )
        })
        .map_err(|e| primer_core::error::PrimerError::Speech(format!("spawn audio thread: {e}")))?;

    // ── Build LoopBackends with the channel STT wrapper ────────────
    // Single-locale v1: the CLI's resolved locale is the active locale,
    // and that's the only entry in the maps.
    let voice = primer_core::speech::VoiceProfile {
        model_id: cfg.voice_id.to_string(),
        // Slow Piper slightly: length_scale = 1.0 / rate, so
        // rate < 1.0 stretches each phoneme. 0.9 is barely
        // perceptible but easier for younger children to follow.
        rate: 0.9,
        ..primer_core::speech::VoiceProfile::default()
    };
    let backends = LoopBackends::single_locale(
        Arc::new(audio_thread::ChannelStt {
            rx: Arc::new(std::sync::Mutex::new(transcript_rx)),
        }),
        Arc::clone(&tts),
        voice,
        cfg.locale,
    );

    // Pre-flight: a missing voice for `active_locale` would otherwise
    // surface as a `PrimerError::Speech` on the child's first sentence.
    backends.ensure_active_locale_coverage()?;

    // ── on_audio callback: push synth chunks to the speaker ─────────
    // Uses a leftover buffer carried across calls so partial tails of
    // each phrase are PREPENDED to the next call's samples instead of
    // zero-padded. An empty input Vec is the FLUSH sentinel.
    let output_resampler_for_cb = Arc::clone(&output_resampler);
    let mut output_leftover: Vec<f32> = Vec::with_capacity(output_chunk_in);
    let spk_errored_for_cb = Arc::clone(&spk_errored);
    let spk_prod_for_cb = Arc::clone(&spk_prod);
    let on_audio: Box<dyn FnMut(Vec<f32>) + Send> = Box::new(move |samples| {
        let is_flush = samples.is_empty();
        let mut samples = samples;
        if need_output_resample {
            let mut guard = output_resampler_for_cb.lock().unwrap();
            if let Some(resampler) = guard.as_mut() {
                // Prepend leftover from prior call, then process as many
                // full output_chunk_in blocks as possible.
                let mut combined: Vec<f32> =
                    Vec::with_capacity(output_leftover.len() + samples.len());
                combined.append(&mut output_leftover);
                combined.append(&mut samples);
                let usable = (combined.len() / output_chunk_in) * output_chunk_in;
                let mut out_buf: Vec<f32> = Vec::with_capacity(combined.len() * 2);
                let mut i = 0;
                while i + output_chunk_in <= usable {
                    let block = &combined[i..i + output_chunk_in];
                    match resampler.process(block) {
                        Ok(o) => out_buf.extend(o),
                        Err(e) => {
                            tracing::warn!("output resampler error: {e}");
                            return;
                        }
                    }
                    i += output_chunk_in;
                }
                let tail: Vec<f32> = combined[usable..].to_vec();
                if is_flush {
                    // End of turn: zero-pad the leftover to chunk_in
                    // and process. The zero-pad lands on trailing
                    // silence; subsequent silence chunks push out any
                    // remaining FFT-buffered output. process_partial()
                    // alone proved insufficient on FftFixedIn — the
                    // last syllable still got swallowed. Feeding 4
                    // silence chunks (≈186 ms of input silence) reliably
                    // drains the internal latency.
                    if !tail.is_empty() {
                        let mut padded = tail;
                        padded.resize(output_chunk_in, 0.0);
                        if let Ok(o) = resampler.process(&padded) {
                            out_buf.extend(o);
                        }
                    }
                    let silence = vec![0.0_f32; output_chunk_in];
                    for _ in 0..4 {
                        match resampler.process(&silence) {
                            Ok(o) => out_buf.extend(o),
                            Err(_) => break,
                        }
                    }
                    output_leftover = Vec::new();
                } else {
                    output_leftover = tail;
                }
                samples = out_buf;
            }
        }
        // Push to speaker ringbuf, blocking until it accepts every
        // sample.
        if !samples.is_empty() {
            let mut prod = spk_prod_for_cb
                .lock()
                .expect("speaker producer mutex poisoned");
            primer_speech::push_all_with_bail(
                &mut prod,
                &samples,
                &spk_errored_for_cb,
                std::time::Duration::from_millis(5),
            );
        }
    });

    // ── Wire DialogueManager via the Responder adapter ─────────────
    let responder: Box<dyn primer_speech::voice_loop::Responder + '_> =
        Box::new(dialogue_responder::DialogueResponder { dialogue });

    // ── Build the drain hook ───────────────────────────────────────
    let spk_prod_for_drain = Arc::clone(&spk_prod);
    let spk_errored_for_drain = Arc::clone(&spk_errored);
    let drain_hook: primer_speech::voice_loop::DrainHook = Box::new(move || {
        let prod = Arc::clone(&spk_prod_for_drain);
        let errored = Arc::clone(&spk_errored_for_drain);
        Box::pin(async move {
            let join = tokio::task::spawn_blocking(move || {
                let prod_guard = prod.lock().expect("speaker producer mutex poisoned");
                let _ = primer_speech::wait_for_drain(
                    &prod_guard,
                    &errored,
                    std::time::Duration::from_millis(10),
                    3,
                    std::time::Duration::from_millis(80),
                    std::time::Duration::from_secs(5),
                );
            });
            if let Err(e) = join.await {
                tracing::warn!("speaker drain task did not complete: {e:?}");
            }
        })
    });

    // ── Observer ─────────────────────────────────────────────────
    let observer = StdoutObserver::new(cfg.verbose);

    // ── Drive the voice loop ─────────────────────────────────────
    let result = run_loop_borrowed(
        backends,
        event_rx,
        responder,
        on_audio,
        Some(drain_hook),
        cfg.verbose,
        Some(Arc::clone(&is_speaking)),
        observer,
    )
    .await;

    // ── Tell the audio thread to stop and wait for it ─────────────
    stop_flag.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Err(panic) = audio_thread.join() {
        tracing::warn!("audio thread panicked: {panic:?}");
    }

    // Keep cpal streams alive until here (drop now via end of scope).
    drop(mic);
    drop(spk);

    let transcripts = result?;
    if cfg.verbose {
        eprintln!("[speech] session ended after {} turn(s)", transcripts.len());
    }
    Ok(())
}

/// Stub implementation when the `speech` feature is disabled. Returns
/// an error so the binary fails fast if `--speech` is somehow set
/// without the feature.
#[cfg(not(feature = "speech"))]
pub async fn run(
    _cfg: SpeechLoopConfig<'_>,
    _dialogue: &mut DialogueManager,
) -> Result<()> {
    Err(primer_core::error::PrimerError::Speech(
        "primer-cli was built without the `speech` feature".into(),
    ))
}

// ── Tests ─────────────────────────────────────────────────────────────
//
// The state-machine behaviour tests (happy path, cancel on resumed speech,
// quit phrase, is_speaking gate, LLM error fallback) have been lifted to
// `primer-speech::voice_loop::state_machine` in Task 1.4. They exercise
// `run_loop_borrowed` directly and do not need to be duplicated here.
//
// What remains here:
//   - Unit tests for the CLI-specific glue types (SpeechLoopConfig shape,
//     any helpers added in future).
//   - The markdown and quit-phrase helpers have moved to primer-speech's
//     state_machine tests.
//
// The mocks and test impls from the old speech_loop.rs are no longer
// needed: the state-machine tests live in primer-speech and use the
// shared MockStreamingStt / MockStreamingTts from that module.
