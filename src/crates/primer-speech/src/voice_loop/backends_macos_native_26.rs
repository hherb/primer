//! macOS 26 native local backend builder (SpeechAnalyzer + derived VAD).
//!
//! Builds a [`LocalBackends`] for macOS 26+ where the new SpeechAnalyzer
//! API replaces the older SFSpeechRecognizer + Silero VAD pair: a single
//! Swift pipeline pulls partial + final transcripts AND derived-VAD
//! `SpeechStart`/`SpeechEnd` events from one acoustic graph (see the
//! Swift sidecar at `crates/primer-speech/swift-sources/`). TTS reuses
//! `MacosTextToSpeech` (AVSpeechSynthesizer) from the `macos-native`
//! path — AVSpeechSynthesizer is unchanged on macOS 26.
//!
//! Sibling of [`super::backends_macos_native`]; both consume the shared
//! mic/speaker/closure helpers in [`super::backends_common`] so the only
//! thing that differs between the two macOS builders is the STT/VAD
//! pipeline and the audio-thread body. The audio thread here delegates
//! buffering to [`crate::voice_loop::macos26_audio_buffer::PendingMicBuffer`]
//! so the pre-speak / post-speak transition behaviour is unit-tested
//! without an audio-backend dep (closes #139).

#![cfg(all(target_os = "macos", feature = "cpal", feature = "macos-native-26"))]

use std::sync::Arc;

use primer_core::error::{PrimerError, Result};
use primer_core::speech::{StreamingTextToSpeech, VadEvent, VoiceProfile};

use crate::Resampler;
use crate::voice_loop::backends_common::{
    ChannelStt, LocalBackends, MicPipeline, SpeakerPipeline, make_drain_hook, make_on_audio,
};
use crate::voice_loop::{LoopBackends, VAD_EVENT_CHANNEL_CAPACITY};

/// Build a [`LocalBackends`] using macOS 26's SpeechAnalyzer for STT and
/// derived VAD events. The TTS backend (`tts`) and `voice` profile are
/// injected by the caller (production wires AVSpeechSynthesizer via
/// `MacosTextToSpeech`), so STT (native) and TTS are decoupled — this
/// builder no longer constructs `MacosTextToSpeech` itself. Sibling of
/// [`super::backends_macos_native::build_local_backends_macos_native`] —
/// same signature, same return type; the audio thread's STT/VAD pipeline
/// is what differs.
///
/// Audio flow: cpal mic → std::thread (resample) → tokio mpsc → Swift
/// SpeechAnalyzer → DerivedVadStateMachine → VadEvent + TextMessage →
/// bridge task → std::sync::mpsc<String> → ChannelStt → voice loop.
///
/// `_mic_silence_ms` is a Silero-VAD knob used by the sibling
/// [`super::backends_macos_native::build_local_backends_macos_native`];
/// the macos-26 path uses transcript-derived VAD instead. The parameter
/// is kept for signature parity with the sibling builder; the leading
/// underscore tells the compiler the unused-variable is deliberate.
pub async fn build_local_backends_macos_native_26(
    tts: Arc<dyn StreamingTextToSpeech>,
    voice: VoiceProfile,
    locale: primer_core::i18n::Locale,
    _mic_silence_ms: u32,
    verbose: bool,
) -> Result<LocalBackends> {
    use crate::macos26::analyzer::{TextMessage, run_consumer_loop};
    use crate::macos26::audio_session;
    use crate::macos26::bridge::ffi as macos26_ffi;
    use crate::macos26::locale::to_bcp47;

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

    // ── TTS sample rate (TTS injected by the caller) ─────────────
    let tts_sample_rate = tts.sample_rate();

    // ── Open mic + input resampler ───────────────────────────────
    // The analyzer's preferred rate is queried from the Swift side
    // (SpeechAnalyzer.bestAvailableAudioFormat) rather than hardcoded:
    // SpeechTranscriber's preferred format is not necessarily 16 kHz,
    // and writing samples at the wrong rate into a buffer claiming the
    // analyzer's format silently produces inaudible garbage.
    let analyzer_rate_f64 = pipeline.analyzer_sample_rate();
    let analyzer_rate: u32 = analyzer_rate_f64.round() as u32;
    let analyzer_chunk: usize = 512;
    let MicPipeline {
        mic,
        mic_cons,
        mut input_resampler,
        in_chunk_samples,
    } = MicPipeline::start(analyzer_rate, analyzer_chunk, verbose)?;
    tracing::info!(
        target: "primer::speech::macos26",
        "analyzer sample rate: {} Hz (mic: {} Hz)",
        analyzer_rate, mic.sample_rate
    );

    // ── Open speaker + output resampler ──────────────────────────
    let speaker_pipeline = SpeakerPipeline::start(tts_sample_rate, verbose)?;

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
    //
    // Buffering is delegated to `PendingMicBuffer` so the pre-speak /
    // post-speak transition behaviour can be unit-tested without an
    // audio backend dep (closes #139 — pre-speak samples used to leak
    // into the post-speak transcription as a phantom partial). The
    // resampler call and `audio_tx.try_send` stay inline because they
    // depend on the rubato/tokio types this thread owns.
    let stop_flag_thread = Arc::clone(&stop_flag);
    let is_speaking_thread = Arc::clone(&is_speaking);
    let audio_thread = std::thread::Builder::new()
        .name("primer-speech-audio-macos26".into())
        .spawn(move || -> Result<()> {
            run_audio_thread_macos26(
                mic_cons,
                in_chunk_samples,
                &mut input_resampler,
                stop_flag_thread,
                is_speaking_thread,
                audio_tx,
            )
        })
        .map_err(|e| PrimerError::Speech(format!("spawn audio thread: {e}")))?;

    // ── Build LoopBackends ───────────────────────────────────────
    // The `voice` profile is supplied by the caller. MacosTextToSpeech
    // ignores VoiceProfile and selects the voice from its own locale —
    // same as the sibling.
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
        Arc::clone(&speaker_pipeline.spk_prod),
        Arc::clone(&speaker_pipeline.spk_errored),
        Arc::clone(&speaker_pipeline.output_resampler),
        speaker_pipeline.output_chunk_in,
        speaker_pipeline.need_output_resample,
    );
    let drain_hook = make_drain_hook(
        Arc::clone(&speaker_pipeline.spk_prod),
        Arc::clone(&speaker_pipeline.spk_errored),
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
        _spk: speaker_pipeline.spk,
    })
}

/// Audio capture thread body for the macOS-26 path.
///
/// Buffers via `PendingMicBuffer` (gating `is_speaking` to prevent
/// pre-speak audio leaking into the post-speak transcription), runs each
/// chunk through the optional resampler, and `try_send`s the resampled
/// chunk to the consumer task that owns the Swift SpeechAnalyzer
/// pipeline. `try_send` (not `blocking_send`) so a wedged consumer drops
/// samples rather than backpressuring the audio thread into glitches.
///
/// Pulled out of the inline `std::thread::Builder::spawn` closure so the
/// builder body stays under the 500-line guideline and the closure type
/// inference doesn't bloat error messages.
fn run_audio_thread_macos26(
    mut mic_cons: ringbuf::HeapCons<f32>,
    in_chunk_samples: usize,
    input_resampler: &mut Option<Resampler>,
    stop_flag: Arc<std::sync::atomic::AtomicBool>,
    is_speaking: Arc<std::sync::atomic::AtomicBool>,
    audio_tx: tokio::sync::mpsc::Sender<Vec<f32>>,
) -> Result<()> {
    use crate::voice_loop::macos26_audio_buffer::PendingMicBuffer;
    use ringbuf::traits::Consumer;

    /// One mic-callback drain in samples. cpal's callback fires every
    /// few ms; 1024 mono f32 ≈ 23 ms at 44.1 kHz / 21 ms at 48 kHz —
    /// well inside cpal's per-callback budget on every host audio
    /// stack we ship on. Sized to fit on the stack so the audio thread
    /// never heap-allocates per drain.
    const AUDIO_DRAIN_BLOCK: usize = 1024;
    const POLL_SLEEP: std::time::Duration = std::time::Duration::from_millis(5);

    let mut buffer = PendingMicBuffer::new(in_chunk_samples);
    let mut tmp = [0f32; AUDIO_DRAIN_BLOCK];
    while !stop_flag.load(std::sync::atomic::Ordering::SeqCst) {
        let popped = mic_cons.pop_slice(&mut tmp);
        if popped == 0 {
            std::thread::sleep(POLL_SLEEP);
            continue;
        }
        let is_speaking_now = is_speaking.load(std::sync::atomic::Ordering::SeqCst);
        let chunks = buffer.feed(&tmp[..popped], is_speaking_now);
        for chunk in chunks {
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
}
