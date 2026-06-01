//! macOS-native local backend builder (SFSpeechRecognizer + Silero VAD).
//!
//! Builds a [`LocalBackends`] for macOS ≤ 25 where on-device speech
//! recognition is exposed via SFSpeechRecognizer (added to AppKit in
//! macOS 10.15) and AVSpeechSynthesizer. The VAD is still Silero because
//! SFSpeechRecognizer doesn't expose a "speech started / ended" stream
//! and the macOS 26 `SpeechDetector` would break our macOS 13 floor.
//!
//! Sibling of [`super::backends_macos_native_26`] (SpeechAnalyzer +
//! derived VAD); both consume the shared mic/speaker/closure helpers in
//! [`super::backends_common`] so the only thing that differs between the
//! two macOS builders is the STT/VAD pipeline and the audio-thread body.
//!
//! Lives in a sibling module gated only on
//! `target_os = "macos" + cpal + macos-native` so the whisper/piper
//! dependency stack stays out of the macOS-native build.

#![cfg(all(target_os = "macos", feature = "cpal", feature = "macos-native"))]

use std::sync::Arc;

use primer_core::error::{PrimerError, Result};
use primer_core::speech::{
    StreamingSpeechToText, StreamingTextToSpeech, TranscriptionSession, VadEvent,
    VoiceActivityDetector, VoiceProfile,
};

use crate::voice_loop::backends_common::{
    ChannelStt, LocalBackends, MicPipeline, SpeakerPipeline, make_drain_hook, make_on_audio,
};
use crate::voice_loop::{LoopBackends, VAD_EVENT_CHANNEL_CAPACITY};
use crate::{Resampler, SileroVad, SileroVadParams};

/// Audio capture thread body — generic over the STT backend.
///
/// Identical in shape to the whisper-specific `run_audio_thread` in
/// [`super::backends`] except it accepts any
/// `Arc<dyn StreamingSpeechToText + Send + Sync>` instead of a concrete
/// `Arc<WhisperStt>`. Supplied here with `MacosSpeechToText`.
#[allow(clippy::too_many_arguments)]
fn run_audio_thread_stt(
    mut mic_cons: ringbuf::HeapCons<f32>,
    mut input_resampler: Option<Resampler>,
    in_chunk_samples: usize,
    vad_chunk_samples: usize,
    vad: &mut SileroVad,
    stt: Arc<dyn StreamingSpeechToText + Send + Sync>,
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
        // everything the mic captures and discard any partial STT session.
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
                    tracing::warn!("stt push_audio: {e}");
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
                        match stt.open_session() {
                            Ok(s) => active_session = Some(s),
                            Err(e) => tracing::warn!("stt open_session: {e}"),
                        }
                    }
                    if event_tx.blocking_send(VadEvent::SpeechStart).is_err() {
                        return Ok(());
                    }
                }
                VadEvent::SpeechEnd => {
                    // TODO(macos-native): MacosSttSession::finalize() blocks the audio
                    // capture thread for up to 2 s waiting for SFSpeechRecognizer's final
                    // segment (FINALIZE_POLL_TIMEOUT in stt.rs). During this window the
                    // mic ringbuf keeps filling (5 s of headroom) but new VAD events are
                    // not processed. For fast back-and-forth speech this is a latency
                    // cliff. The fix is to move finalize() to a short-lived helper thread
                    // and signal completion via a channel, letting the audio loop keep
                    // pumping. Deferred to a follow-up PR — see plan task 8 review.
                    let segments = match active_session.take() {
                        Some(s) => match s.finalize() {
                            Ok(segs) => segs,
                            Err(e) => {
                                tracing::warn!("stt finalize: {e}");
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

/// Construct every local backend the voice loop needs using macOS-native
/// speech backends: cpal mic + speaker, silero VAD, `MacosSpeechToText`
/// (SFSpeechRecognizer, on-device), audio capture thread, plus the
/// `on_audio` closure and drain hook. The TTS backend (`tts`) and
/// `voice` profile are injected by the caller, so STT (native) and TTS
/// are decoupled — the builder no longer constructs `MacosTextToSpeech`
/// itself.
///
/// Same `LocalBackends` shape as `build_local_backends`; only the STT
/// `Arc` and VAD differ. Silero VAD and the cpal mic/speaker construction
/// are unchanged. Callers choose which builder to invoke based on
/// compile-time feature flags and, optionally, a runtime config selector.
///
/// `mic_silence_ms` configures the Silero VAD's `min_silence_ms` parameter
/// (how long after speech ends before firing `SpeechEnd`).
///
/// `verbose` only controls a couple of stderr lines about mic/speaker
/// open rates; flip to false from the GUI.
pub async fn build_local_backends_macos_native(
    tts: Arc<dyn StreamingTextToSpeech>,
    voice: VoiceProfile,
    locale: primer_core::i18n::Locale,
    mic_silence_ms: u32,
    verbose: bool,
) -> Result<LocalBackends> {
    use crate::macos::MacosSpeechToText;

    let bcp47 = locale.bcp47();

    // ── Build VAD ────────────────────────────────────────────────
    let vad_params = SileroVadParams {
        min_silence_ms: mic_silence_ms,
        ..SileroVadParams::default()
    };
    let mut audio_vad = SileroVad::new(vad_params)?;

    // ── Build STT (macOS native, on-device-only by construction) ─
    let stt: Arc<dyn StreamingSpeechToText + Send + Sync> =
        Arc::new(MacosSpeechToText::new(bcp47)?);

    // ── TTS sample rate (TTS injected by the caller) ─────────────
    let tts_sample_rate = tts.sample_rate();

    // ── Open mic + input resampler (target: Silero/STT at 16 kHz) ─
    let vad_rate = audio_vad.sample_rate();
    let vad_chunk = audio_vad.chunk_samples();
    let MicPipeline {
        mic,
        mic_cons,
        mut input_resampler,
        in_chunk_samples,
    } = MicPipeline::start(vad_rate, vad_chunk, verbose)?;

    // ── Open speaker + output resampler ──────────────────────────
    let pipeline = SpeakerPipeline::start(tts_sample_rate, verbose)?;

    // ── Channels ────────────────────────────────────────────────
    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<VadEvent>(VAD_EVENT_CHANNEL_CAPACITY);
    let (transcript_tx, transcript_rx) = std::sync::mpsc::channel::<String>();
    let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let is_speaking = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // ── Spawn audio capture thread ──────────────────────────────
    let stt_for_thread = Arc::clone(&stt);
    let stop_flag_thread = Arc::clone(&stop_flag);
    let is_speaking_thread = Arc::clone(&is_speaking);
    let audio_thread = std::thread::Builder::new()
        .name("primer-speech-audio-macos".into())
        .spawn(move || {
            run_audio_thread_stt(
                mic_cons,
                input_resampler.take(),
                in_chunk_samples,
                vad_chunk,
                &mut audio_vad,
                stt_for_thread,
                event_tx,
                transcript_tx,
                stop_flag_thread,
                is_speaking_thread,
            )
        })
        .map_err(|e| PrimerError::Speech(format!("spawn audio thread: {e}")))?;

    // ── Build LoopBackends (single-locale; ChannelStt adapter reads
    //    transcripts from the audio thread). ──────────────────────
    // The `voice` profile is supplied by the caller. For the
    // macOS-native path there is no piper model-id; MacosTextToSpeech
    // ignores the VoiceProfile and selects the voice from its own locale.
    let backends = LoopBackends::single_locale(
        Arc::new(ChannelStt {
            rx: Arc::new(std::sync::Mutex::new(transcript_rx)),
        }),
        Arc::clone(&tts),
        voice,
        locale,
    );
    backends.ensure_active_locale_coverage()?;

    // ── on_audio closure + drain hook ───────────────────────────
    let on_audio = make_on_audio(
        Arc::clone(&pipeline.spk_prod),
        Arc::clone(&pipeline.spk_errored),
        Arc::clone(&pipeline.output_resampler),
        pipeline.output_chunk_in,
        pipeline.need_output_resample,
    );
    let drain_hook = make_drain_hook(
        Arc::clone(&pipeline.spk_prod),
        Arc::clone(&pipeline.spk_errored),
    );

    Ok(LocalBackends {
        backends: Some(backends),
        event_rx: Some(event_rx),
        on_audio: Some(on_audio),
        drain_hook: Some(drain_hook),
        is_speaking,
        audio_thread: Some(audio_thread),
        stop_flag,
        _mic: mic,
        _spk: pipeline.spk,
    })
}
