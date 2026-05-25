//! macOS-native local backend builders for the voice loop.
//!
//! Two sibling builders:
//! - [`build_local_backends_macos_native`] — Silero VAD + `MacosSpeechToText`
//!   (SFSpeechRecognizer, on-device) + `MacosTextToSpeech` (AVSpeechSynthesizer).
//! - [`build_local_backends_macos_native_26`] — macOS 26's SpeechAnalyzer for
//!   STT + derived VAD + `MacosTextToSpeech` for TTS.
//!
//! Shared types (`LocalBackends`, `ChannelStt`) come from
//! [`super::backends_common`] so neither builder inherits a whisper/piper
//! dependency. Each builder's `pub` item carries the narrowest feature
//! gate that actually reflects its body — `silero + cpal + macos-native`
//! for the SFSpeechRecognizer path, `cpal + macos-native-26` for the
//! SpeechAnalyzer path. Lives in a sibling module gated only on
//! `cpal + any(macos-native, macos-native-26)` so the file itself
//! compiles without whisper/piper deps. Closes #137.

#![cfg(all(
    target_os = "macos",
    feature = "cpal",
    any(feature = "macos-native", feature = "macos-native-26")
))]

use std::sync::Arc;

use primer_core::error::{PrimerError, Result};
use primer_core::speech::{StreamingTextToSpeech, VadEvent, VoiceProfile};

use crate::voice_loop::backends_common::{ChannelStt, LocalBackends};
use crate::voice_loop::{LoopBackends, VAD_EVENT_CHANNEL_CAPACITY};
use crate::{MicCapture, Resampler, SpeakerSink};

// Imports for the macos-native (SFSpeechRecognizer) builder only.
#[cfg(feature = "macos-native")]
use crate::{SileroVad, SileroVadParams};
#[cfg(feature = "macos-native")]
use primer_core::speech::{StreamingSpeechToText, TranscriptionSession, VoiceActivityDetector};

/// Audio capture thread body — generic over the STT backend.
///
/// Identical in shape to the whisper-specific `run_audio_thread` in
/// [`super::backends`] except it accepts any
/// `Arc<dyn StreamingSpeechToText + Send + Sync>` instead of a concrete
/// `Arc<WhisperStt>`. Used by [`build_local_backends_macos_native`]
/// which supplies a `MacosSpeechToText`.
#[cfg(feature = "macos-native")]
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
/// (SFSpeechRecognizer, on-device), `MacosTextToSpeech` (AVSpeechSynthesizer),
/// audio capture thread, plus the `on_audio` closure and drain hook.
///
/// Same `LocalBackends` shape as `build_local_backends`; only the STT/TTS
/// `Arc`s differ. Silero VAD and the cpal mic/speaker construction are
/// unchanged. Callers choose which builder to invoke based on compile-time
/// feature flags and, optionally, a runtime config selector.
///
/// `mic_silence_ms` configures the Silero VAD's `min_silence_ms` parameter
/// (how long after speech ends before firing `SpeechEnd`).
///
/// `verbose` only controls a couple of stderr lines about mic/speaker
/// open rates; flip to false from the GUI.
///
/// **Note on DRY:** the audio-thread construction below is a verbatim copy
/// of the tail of `build_local_backends` (from "Open mic" onward). A
/// follow-up refactor PR will extract the shared tail into a private
/// helper that all builders call. For this commit, DRY is sacrificed for
/// surgical risk reduction.
#[cfg(feature = "macos-native")]
pub async fn build_local_backends_macos_native(
    locale: primer_core::i18n::Locale,
    mic_silence_ms: u32,
    verbose: bool,
) -> Result<LocalBackends> {
    use crate::macos::{MacosSpeechToText, MacosTextToSpeech};

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

    // ── Build TTS (macOS native, locale-resolved voice) ──────────
    let tts: Arc<dyn StreamingTextToSpeech> = Arc::new(MacosTextToSpeech::new(bcp47)?);
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

    // ── Input resampler: mic_rate → 16 kHz for VAD/STT ──────────
    let vad_rate = audio_vad.sample_rate();
    let vad_chunk = audio_vad.chunk_samples();
    let in_chunk_samples: usize = (vad_chunk as u64 * mic_rate as u64 / vad_rate as u64) as usize;
    let mut input_resampler: Option<Resampler> = if mic_rate != vad_rate {
        Some(Resampler::new(mic_rate, vad_rate, in_chunk_samples)?)
    } else {
        None
    };

    // ── Output resampler: tts_rate → spk_rate (lazy) ────────────
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
    // For the macOS-native path there is no piper model-id; the
    // VoiceProfile.model_id field is set to the bcp47 tag as a
    // human-readable identifier. MacosTextToSpeech ignores the
    // VoiceProfile and selects the voice from its own locale.
    let voice = VoiceProfile {
        model_id: bcp47.to_string(),
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
    let drain_hook: crate::voice_loop::DrainHook = Box::new(move || {
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

/// Build a [`LocalBackends`] using macOS 26's SpeechAnalyzer for STT and
/// derived VAD events, with AVSpeechSynthesizer for TTS (reused from the
/// `macos-native` path's `MacosTextToSpeech`). Sibling of
/// [`build_local_backends_macos_native`] — same signature, same return
/// type; the audio thread's STT/VAD pipeline is what differs.
///
/// Audio flow: cpal mic → std::thread (resample) → tokio mpsc → Swift
/// SpeechAnalyzer → DerivedVadStateMachine → VadEvent + TextMessage →
/// bridge task → std::sync::mpsc<String> → ChannelStt → voice loop.
#[cfg(feature = "macos-native-26")]
pub async fn build_local_backends_macos_native_26(
    locale: primer_core::i18n::Locale,
    mic_silence_ms: u32,
    verbose: bool,
) -> Result<LocalBackends> {
    use crate::macos::MacosTextToSpeech;
    use crate::macos26::analyzer::{TextMessage, run_consumer_loop};
    use crate::macos26::audio_session;
    use crate::macos26::bridge::ffi as macos26_ffi;
    use crate::macos26::locale::to_bcp47;

    // mic_silence_ms is a Silero-VAD knob in the sibling; the macos26
    // path uses transcript-derived VAD instead. The parameter is kept
    // for signature parity; we silence the unused-variable warning.
    let _ = mic_silence_ms;

    let bcp47 = to_bcp47(locale)?.to_string();
    audio_session::configure_for_capture()?;

    tracing::info!(
        target: "primer::speech::macos26",
        "constructing SpeechAnalyzer pipeline for locale {bcp47}"
    );
    // `create` is a free function in the bridge ffi module, not an
    // associated function on Macos26Pipeline. Panics on failure (see
    // bridge.rs comment — swift-bridge 0.1.x can't express Result here).
    let pipeline = macos26_ffi::create(bcp47.clone()).await;

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

    // ── TTS (reused AVSpeechSynthesizer from macos-native) ──────
    let tts: Arc<dyn StreamingTextToSpeech> = Arc::new(MacosTextToSpeech::new(&bcp47)?);
    let tts_sample_rate = tts.sample_rate();

    // ── Input resampler: mic_rate → analyzer's preferred rate ──
    // The analyzer's preferred rate is queried from the Swift side
    // (SpeechAnalyzer.bestAvailableAudioFormat) rather than hardcoded:
    // SpeechTranscriber's preferred format is not necessarily 16 kHz,
    // and writing samples at the wrong rate into a buffer claiming the
    // analyzer's format silently produces inaudible garbage.
    let analyzer_rate_f64 = pipeline.analyzer_sample_rate();
    let analyzer_rate: u32 = analyzer_rate_f64.round() as u32;
    tracing::info!(
        target: "primer::speech::macos26",
        "analyzer sample rate: {} Hz (mic: {} Hz)",
        analyzer_rate, mic_rate
    );
    let analyzer_chunk: usize = 512;
    let in_chunk_samples: usize =
        (analyzer_chunk as u64 * mic_rate as u64 / analyzer_rate as u64) as usize;
    let mut input_resampler: Option<Resampler> = if mic_rate != analyzer_rate {
        Some(Resampler::new(mic_rate, analyzer_rate, in_chunk_samples)?)
    } else {
        None
    };

    // ── Output resampler: tts_rate → spk_rate (lazy) ────────────
    let need_output_resample = tts_sample_rate != spk_rate;
    let output_chunk_in: usize = 1024;
    let output_resampler = Arc::new(std::sync::Mutex::new(if need_output_resample {
        Some(Resampler::new(tts_sample_rate, spk_rate, output_chunk_in)?)
    } else {
        None
    }));

    // ── Channels ────────────────────────────────────────────────
    // tokio mpsc: audio thread → analyzer consumer task
    let (audio_tx, audio_rx) = tokio::sync::mpsc::channel::<Vec<f32>>(16);
    // tokio mpsc: consumer task → bridge task (TextMessage)
    let (text_msg_tx, mut text_msg_rx) = tokio::sync::mpsc::channel::<TextMessage>(64);
    // tokio mpsc: consumer task → voice loop (VadEvent)
    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<VadEvent>(VAD_EVENT_CHANNEL_CAPACITY);
    // std mpsc: bridge task → ChannelStt (final transcripts as String)
    let (transcript_tx, transcript_rx) = std::sync::mpsc::channel::<String>();
    let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let is_speaking = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // ── Consumer task: owns Swift pipeline, pulls next_result loop ──
    // Wrap in an async block so the task's outcome is logged on
    // completion. The JoinHandle is intentionally dropped (the task
    // outlives this function), but unlike a bare `tokio::spawn` of
    // `run_consumer_loop` directly, a non-`Ok(())` return now surfaces
    // as a `tracing::warn!` rather than vanishing silently — important
    // for diagnosing a voice-mode session that appears alive but goes
    // quiet because Swift's pipeline errored.
    tokio::spawn(async move {
        match run_consumer_loop(pipeline, audio_rx, text_msg_tx, event_tx).await {
            Ok(()) => {
                tracing::info!(
                    target: "primer::speech::macos26",
                    "consumer loop exited cleanly"
                );
            }
            Err(e) => {
                tracing::warn!(
                    target: "primer::speech::macos26",
                    "consumer loop exited with error: {e}"
                );
            }
        }
    });

    // ── Bridge: TextMessage (tokio mpsc) → String (std mpsc) ────
    // Drops volatile partials; only forwards is_final messages to
    // ChannelStt. Partial transcripts are only useful for live-display
    // in a future GUI; the CLI voice loop only needs final utterances.
    tokio::spawn(async move {
        while let Some(msg) = text_msg_rx.recv().await {
            if msg.is_final {
                if transcript_tx.send(msg.segment.text).is_err() {
                    // ChannelStt dropped; stop forwarding.
                    break;
                }
            } else {
                tracing::trace!(
                    target: "primer::speech::macos26",
                    "volatile partial dropped at ChannelStt bridge: {:?}",
                    msg.segment.text
                );
            }
        }
    });

    // ── Audio thread: mic → resample → audio_tx ─────────────────
    let stop_flag_thread = Arc::clone(&stop_flag);
    let is_speaking_thread = Arc::clone(&is_speaking);
    let audio_thread = std::thread::Builder::new()
        .name("primer-speech-audio-macos26".into())
        .spawn(move || -> Result<()> {
            use ringbuf::traits::Consumer;
            let mut pending: Vec<f32> = Vec::with_capacity(in_chunk_samples * 4);
            let mut tmp = [0f32; 1024];
            let mut mic_cons = mic_cons;
            while !stop_flag_thread.load(std::sync::atomic::Ordering::SeqCst) {
                let popped = mic_cons.pop_slice(&mut tmp);
                if popped == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                }
                // Anti-feedback: discard mic samples while the Primer is speaking.
                if is_speaking_thread.load(std::sync::atomic::Ordering::SeqCst) {
                    continue;
                }
                pending.extend_from_slice(&tmp[..popped]);
                while pending.len() >= in_chunk_samples {
                    let chunk: Vec<f32> = pending.drain(..in_chunk_samples).collect();
                    let resampled = match input_resampler.as_mut() {
                        Some(r) => r
                            .process(&chunk)
                            .map_err(|e| PrimerError::Speech(format!("resample: {e}")))?,
                        None => chunk,
                    };
                    // try_send: if the consumer is wedged we drop rather than block.
                    let _ = audio_tx.try_send(resampled);
                }
            }
            Ok(())
        })
        .map_err(|e| PrimerError::Speech(format!("spawn audio thread: {e}")))?;

    // ── Build LoopBackends ───────────────────────────────────────
    // VoiceProfile.model_id is set to the bcp47 tag as a human-readable
    // identifier. MacosTextToSpeech ignores VoiceProfile and selects the
    // voice from its own locale — same as the sibling.
    // Move `bcp47` into VoiceProfile.model_id — last use, no further
    // borrow needed (TTS construction above and the create() call
    // already cloned what they needed).
    let voice = VoiceProfile {
        model_id: bcp47,
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
    let spk_prod_for_drain_26 = Arc::clone(&spk_prod);
    let spk_errored_for_drain_26 = Arc::clone(&spk_errored);
    let drain_hook: crate::voice_loop::DrainHook = Box::new(move || {
        let prod = Arc::clone(&spk_prod_for_drain_26);
        let errored = Arc::clone(&spk_errored_for_drain_26);
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
