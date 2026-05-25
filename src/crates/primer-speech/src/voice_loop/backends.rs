//! Local backend builder for the voice loop (whisper + piper).
//!
//! Constructs the cpal mic + speaker, silero VAD, whisper streaming STT,
//! piper TTS, the audio capture thread (which owns the VAD + whisper
//! streaming session), and the `on_audio` closure + drain hook the loop
//! consumes.
//!
//! Lifted from `primer-cli/src/speech_loop/mod.rs::run` in PR 4 of the
//! GUI voice-mode work so the CLI and GUI share one builder.
//!
//! Shared types (`LocalBackends`, `ChannelStt`) live in
//! [`super::backends_common`] so the macOS-native builders in
//! [`super::backends_macos`] can use them without inheriting the
//! whisper/piper dependency this module pulls in.
//!
//! Lifecycle: callers receive a [`LocalBackends`] struct that holds the
//! audio resources (mic stream, speaker stream, audio thread handle).
//! They must keep it alive until after `run_loop`/`run_loop_borrowed`
//! returns and then call [`LocalBackends::shutdown`] to drain the audio
//! thread cleanly.

#![cfg(all(
    feature = "silero",
    feature = "whisper",
    feature = "piper",
    feature = "cpal"
))]

use std::path::Path;
use std::sync::Arc;

use primer_core::error::{PrimerError, Result};
use primer_core::speech::{
    StreamingSpeechToText, StreamingTextToSpeech, TranscriptionSession, VadEvent,
    VoiceActivityDetector, VoiceProfile,
};

use crate::voice_loop::backends_common::{ChannelStt, LocalBackends};
use crate::voice_loop::{DrainHook, LoopBackends, VAD_EVENT_CHANNEL_CAPACITY};
use crate::{MicCapture, PiperTts, Resampler, SileroVad, SileroVadParams, SpeakerSink, WhisperStt};

/// Body of the audio capture thread.
///
/// Pulls mic samples from `mic_cons`, resamples to the VAD rate when
/// needed, runs the VAD on fixed-size chunks, and drives the per-
/// utterance whisper session: open on `SpeechStart`, push every chunk
/// while in speech, finalize on `SpeechEnd` and SEND the transcript on
/// `transcript_tx` BEFORE forwarding the `SpeechEnd` event on `event_tx`.
/// That ordering guarantees `ChannelSttSession::finalize` (called by the
/// voice loop after seeing the event) finds a transcript.
///
/// Polling cadence: a 5 ms sleep between empty mic-buffer reads. cpal's
/// callback fires every few ms, so this stays well within real-time
/// tolerances.
#[allow(clippy::too_many_arguments)]
fn run_audio_thread(
    mut mic_cons: ringbuf::HeapCons<f32>,
    mut input_resampler: Option<Resampler>,
    in_chunk_samples: usize,
    vad_chunk_samples: usize,
    vad: &mut SileroVad,
    whisper: Arc<WhisperStt>,
    event_tx: tokio::sync::mpsc::Sender<VadEvent>,
    transcript_tx: std::sync::mpsc::Sender<String>,
    stop_flag: Arc<std::sync::atomic::AtomicBool>,
    is_speaking: Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    use ringbuf::traits::Consumer as _;

    let mut raw_buf: Vec<f32> = Vec::with_capacity(in_chunk_samples * 2);
    let mut vad_in_buf: Vec<f32> = Vec::with_capacity(vad_chunk_samples * 2);
    let mut vad_chunk: Vec<f32> = Vec::with_capacity(vad_chunk_samples);
    let mut active_session: Option<Box<dyn TranscriptionSession>> = None;

    loop {
        if stop_flag.load(std::sync::atomic::Ordering::SeqCst) {
            if let Some(s) = active_session.take() {
                let _ = s.finalize();
            }
            return Ok(());
        }

        // Anti-feedback gate: while the voice loop is in SPEAK, drop
        // everything the mic captures and discard any partial whisper
        // session.
        if is_speaking.load(std::sync::atomic::Ordering::SeqCst) {
            while mic_cons.try_pop().is_some() {}
            if let Some(s) = active_session.take() {
                let _ = s.finalize();
            }
            raw_buf.clear();
            vad_in_buf.clear();
            vad.reset();
            std::thread::sleep(std::time::Duration::from_millis(20));
            continue;
        }

        // Drain available samples from the mic ringbuf.
        let mut produced_any = false;
        while let Some(s) = mic_cons.try_pop() {
            raw_buf.push(s);
            produced_any = true;
            if raw_buf.len() >= 8192 {
                break;
            }
        }
        if !produced_any {
            std::thread::sleep(std::time::Duration::from_millis(5));
            continue;
        }

        // Convert raw_buf into VAD-rate chunks.
        if let Some(resampler) = input_resampler.as_mut() {
            let usable = (raw_buf.len() / in_chunk_samples) * in_chunk_samples;
            let mut consumed = 0;
            while consumed + in_chunk_samples <= usable {
                let block = &raw_buf[consumed..consumed + in_chunk_samples];
                match resampler.process(block) {
                    Ok(out) => vad_in_buf.extend(out),
                    Err(e) => tracing::warn!("input resampler error: {e}"),
                }
                consumed += in_chunk_samples;
            }
            raw_buf.drain(..consumed);
        } else {
            vad_in_buf.append(&mut raw_buf);
        }

        // Process VAD chunks while we have enough samples.
        while vad_in_buf.len() >= vad_chunk_samples {
            vad_chunk.clear();
            vad_chunk.extend(vad_in_buf.drain(..vad_chunk_samples));
            if let Some(session) = active_session.as_mut() {
                if let Err(e) = session.push_audio(&vad_chunk) {
                    tracing::warn!("whisper push_audio: {e}");
                }
            }
            let frame = match vad.process_chunk(&vad_chunk) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("vad process_chunk: {e}");
                    continue;
                }
            };
            match frame.event {
                VadEvent::SpeechStart => {
                    if active_session.is_none() {
                        match whisper.open_session() {
                            Ok(s) => active_session = Some(s),
                            Err(e) => tracing::warn!("whisper open_session: {e}"),
                        }
                    }
                    if event_tx.blocking_send(VadEvent::SpeechStart).is_err() {
                        return Ok(());
                    }
                }
                VadEvent::SpeechEnd => {
                    let segments = match active_session.take() {
                        Some(s) => match s.finalize() {
                            Ok(segs) => segs,
                            Err(e) => {
                                tracing::warn!("whisper finalize: {e}");
                                Vec::new()
                            }
                        },
                        None => Vec::new(),
                    };
                    let text: String = segments
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect::<Vec<_>>()
                        .join("")
                        .trim()
                        .to_string();
                    if transcript_tx.send(text).is_err() {
                        return Ok(());
                    }
                    if event_tx.blocking_send(VadEvent::SpeechEnd).is_err() {
                        return Ok(());
                    }
                }
                VadEvent::None => {}
            }
        }
    }
}

/// Construct every local backend the voice loop needs: cpal mic + speaker,
/// silero VAD, whisper STT, piper TTS, audio capture thread, plus the
/// `on_audio` closure and drain hook.
///
/// `mic_silence_ms` configures the Silero VAD's `min_silence_ms` parameter
/// (how long after speech ends before firing `SpeechEnd`).
///
/// **Async signature:** today the function body is sync (each backend
/// loads synchronously). The `async fn` shape future-proofs for any
/// post-load setup that needs the tokio reactor.
///
/// `verbose` only controls a couple of stderr lines about mic/speaker
/// open rates; flip to false from the GUI.
pub async fn build_local_backends(
    piper_onnx: &Path,
    piper_config: &Path,
    whisper_model: &Path,
    voice_id: &str,
    locale: primer_core::i18n::Locale,
    mic_silence_ms: u32,
    verbose: bool,
) -> Result<LocalBackends> {
    // ── Build VAD ────────────────────────────────────────────────
    let vad_params = SileroVadParams {
        min_silence_ms: mic_silence_ms,
        ..SileroVadParams::default()
    };
    let mut audio_vad = SileroVad::new(vad_params)?;

    // ── Build STT (whisper.cpp) ──────────────────────────────────
    // Pass the learner's locale as the transcription language so a
    // multilingual model (e.g. `ggml-small.bin` for `de`) actually
    // transcribes in that language. Without this, Whisper falls back
    // to its `"en"` default, forces non-English audio into approximate
    // English, and the LLM never sees the original utterance — silent
    // failure for every non-English locale. `Locale::pack_id()` returns
    // ISO-639-1 ("en", "de", …) which is exactly the form Whisper
    // accepts; pinned in `whisper::tests::pack_id_is_iso_639_1_for_whisper`.
    let whisper = Arc::new(WhisperStt::new(whisper_model)?.with_language(locale.pack_id()));

    // ── Build TTS (piper) ────────────────────────────────────────
    let tts: Arc<dyn StreamingTextToSpeech> = Arc::new(PiperTts::new(piper_onnx, piper_config)?);
    let tts_sample_rate = tts.sample_rate();

    // ── Open mic ────────────────────────────────────────────────
    let (mic, mic_cons) = MicCapture::start()?;
    let mic_rate = mic.sample_rate;
    if verbose {
        eprintln!(
            "[speech] mic opened: {}Hz, {} channels",
            mic_rate, mic.channels
        );
    }

    // ── Open speaker ────────────────────────────────────────────
    let (spk, spk_prod) = SpeakerSink::start()?;
    let spk_rate = spk.sample_rate;
    let spk_errored = spk.errored_flag();
    let spk_prod = Arc::new(std::sync::Mutex::new(spk_prod));
    if verbose {
        eprintln!(
            "[speech] speaker opened: {}Hz, {} channels",
            spk_rate, spk.channels
        );
    }

    // ── Input resampler: mic_rate → 16 kHz for VAD/whisper ──────
    let vad_rate = audio_vad.sample_rate();
    let vad_chunk = audio_vad.chunk_samples();
    let in_chunk_samples: usize = (vad_chunk as u64 * mic_rate as u64 / vad_rate as u64) as usize;
    let mut input_resampler: Option<Resampler> = if mic_rate != vad_rate {
        Some(Resampler::new(mic_rate, vad_rate, in_chunk_samples)?)
    } else {
        None
    };

    // ── Output resampler: piper_rate → spk_rate (lazy) ──────────
    let need_output_resample = tts_sample_rate != spk_rate;
    let output_chunk_in: usize = 1024;
    let output_resampler = Arc::new(std::sync::Mutex::new(if need_output_resample {
        Some(Resampler::new(tts_sample_rate, spk_rate, output_chunk_in)?)
    } else {
        None
    }));

    // ── Channels ────────────────────────────────────────────────
    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<VadEvent>(VAD_EVENT_CHANNEL_CAPACITY);
    let (transcript_tx, transcript_rx) = std::sync::mpsc::channel::<String>();
    let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let is_speaking = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // ── Spawn audio capture thread ──────────────────────────────
    let whisper_for_thread = Arc::clone(&whisper);
    let stop_flag_thread = Arc::clone(&stop_flag);
    let is_speaking_thread = Arc::clone(&is_speaking);
    let audio_thread = std::thread::Builder::new()
        .name("primer-speech-audio".into())
        .spawn(move || {
            run_audio_thread(
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
        .map_err(|e| PrimerError::Speech(format!("spawn audio thread: {e}")))?;

    // ── Build LoopBackends (single-locale; the ChannelStt adapter
    //    reads transcripts from the audio thread). ────────────────
    let voice = VoiceProfile {
        model_id: voice_id.to_string(),
        rate: 0.9,
        ..VoiceProfile::default()
    };
    let backends = LoopBackends::single_locale(
        Arc::new(ChannelStt {
            rx: Arc::new(std::sync::Mutex::new(transcript_rx)),
        }),
        Arc::clone(&tts),
        voice,
        locale,
    );
    backends.ensure_active_locale_coverage()?;

    // ── on_audio closure: synthesised PCM → speaker ringbuf ─────
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
        if !samples.is_empty() {
            let mut prod = spk_prod_for_cb
                .lock()
                .expect("speaker producer mutex poisoned");
            crate::push_all_with_bail(
                &mut prod,
                &samples,
                &spk_errored_for_cb,
                std::time::Duration::from_millis(5),
            );
        }
    });

    // ── Drain hook ──────────────────────────────────────────────
    let spk_prod_for_drain = Arc::clone(&spk_prod);
    let spk_errored_for_drain = Arc::clone(&spk_errored);
    let drain_hook: DrainHook = Box::new(move || {
        let prod = Arc::clone(&spk_prod_for_drain);
        let errored = Arc::clone(&spk_errored_for_drain);
        Box::pin(async move {
            let join = tokio::task::spawn_blocking(move || {
                let prod_guard = prod.lock().expect("speaker producer mutex poisoned");
                let _ = crate::wait_for_drain(
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

    Ok(LocalBackends {
        backends: Some(backends),
        event_rx: Some(event_rx),
        on_audio: Some(on_audio),
        drain_hook: Some(drain_hook),
        is_speaking,
        audio_thread: Some(audio_thread),
        stop_flag,
        _mic: mic,
        _spk: spk,
    })
}
