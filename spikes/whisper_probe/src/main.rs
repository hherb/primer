//! Whisper streaming-STT timing probe — A/B counterpart to the
//! `macos26_speech` Swift spike. Same output format:
//!
//!     [wall_ms]  tag  lag=Xms  audio=Yms  seg=A..Bms  "text"
//!
//! `wall_ms` is wall-clock ms since the first audio frame was pulled from
//! the cpal mic ring. `audio` is the cumulative 16 kHz audio fed to the
//! VAD / Whisper pipeline. `seg` is the segment's audio-time range within
//! the current utterance (per `TranscriptSegment.start_ms` / `end_ms`).
//! `lag` is wall-clock arrival minus the global audio time of the
//! segment's end — i.e. how long after the audio for the segment was
//! captured did the segment text appear.
//!
//! Pipeline: cpal mic → resample-to-16 kHz → Silero VAD (512-sample
//! chunks) → Whisper streaming session opened on SpeechStart, finalised
//! on SpeechEnd.
//!
//! Usage:
//!     cargo run --release -- --model <path-to-ggml-or-gguf> [--language en]

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use primer_core::speech::{
    StreamingSpeechToText, TranscriptSegment, TranscriptionSession, VadEvent,
    VoiceActivityDetector,
};
use primer_speech::cpal_io::{MicCapture, Resampler};
use primer_speech::silero::SileroVad;
use primer_speech::whisper::WhisperStt;
use ringbuf::traits::Consumer;

/// Sample rate at which Silero + Whisper consume audio.
const TARGET_RATE: u32 = 16_000;
/// Silero requires exactly 512 samples per chunk at 16 kHz (= 32 ms).
const VAD_FRAME: usize = 512;
/// Audio duration represented by one VAD frame (ms).
const FRAME_MS: u64 = 32;
/// How long to sleep when the mic ring is empty between polls.
const POLL_SLEEP: Duration = Duration::from_millis(5);

#[derive(Parser, Debug)]
#[command(about = "Whisper streaming-STT timing probe (A/B counterpart to macos26_speech)")]
struct Args {
    /// Path to a GGML/GGUF Whisper model file
    /// (e.g. `~/.cache/primer/models/whisper/ggml-small.bin`).
    #[arg(long)]
    model: PathBuf,
    /// Transcription language (ISO 639-1: en, de, ...).
    #[arg(long, default_value = "en")]
    language: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    eprintln!("[probe] loading whisper model: {}", args.model.display());
    let stt = WhisperStt::new(&args.model)
        .context("load whisper model")?
        .with_language(&args.language);
    eprintln!("[probe] language: {}", args.language);

    eprintln!("[probe] opening mic ...");
    let (_mic, mut cons) = MicCapture::start().context("open mic")?;
    let mic_rate = _mic.sample_rate;
    eprintln!(
        "[probe] mic: {} Hz, {} ch -> resampling to {} Hz",
        mic_rate, _mic.channels, TARGET_RATE
    );

    // Size the resampler input chunk so its output is ~one VAD frame.
    let input_chunk: usize = (mic_rate as usize * VAD_FRAME) / TARGET_RATE as usize;
    if input_chunk == 0 {
        bail!("mic sample rate {} too low for {}-sample VAD frames", mic_rate, VAD_FRAME);
    }
    let mut resampler = Resampler::new(mic_rate, TARGET_RATE, input_chunk)
        .context("create resampler")?;
    let mut vad = SileroVad::with_defaults().context("init Silero VAD")?;

    let mut pending_native: Vec<f32> = Vec::with_capacity(input_chunk * 4);
    let mut pending_16k: Vec<f32> = Vec::with_capacity(VAD_FRAME * 4);
    let mut anchor: Option<Instant> = None;
    let mut audio_ms_total: u64 = 0;
    let mut session: Option<Box<dyn TranscriptionSession>> = None;
    let mut speech_start_audio_ms: u64 = 0;

    eprintln!("[probe] listening — press Ctrl+C to stop");
    println!("---");

    let mut tmp = [0f32; 1024];
    loop {
        // Drain whatever the cpal ring has right now.
        loop {
            let n = cons.pop_slice(&mut tmp);
            if n == 0 {
                break;
            }
            pending_native.extend_from_slice(&tmp[..n]);
        }
        if pending_native.len() < input_chunk {
            thread::sleep(POLL_SLEEP);
            continue;
        }

        let chunk: Vec<f32> = pending_native.drain(..input_chunk).collect();
        if anchor.is_none() {
            anchor = Some(Instant::now());
        }
        let resampled = resampler.process(&chunk).context("resample")?;
        pending_16k.extend(resampled);

        while pending_16k.len() >= VAD_FRAME {
            let frame: Vec<f32> = pending_16k.drain(..VAD_FRAME).collect();
            audio_ms_total += FRAME_MS;
            let frame_start_ms = audio_ms_total - FRAME_MS;

            let v = vad.process_chunk(&frame).context("vad step")?;
            match v.event {
                VadEvent::SpeechStart => {
                    print_event("SPEECH_START", &anchor, audio_ms_total);
                    session = Some(stt.open_session().context("open whisper session")?);
                    speech_start_audio_ms = frame_start_ms;
                }
                VadEvent::SpeechEnd => {
                    if let Some(sess) = session.take() {
                        let segs = sess.finalize().context("finalize whisper")?;
                        print_segments(
                            &segs,
                            "FINAL",
                            &anchor,
                            audio_ms_total,
                            speech_start_audio_ms,
                        );
                    }
                    print_event("SPEECH_END", &anchor, audio_ms_total);
                }
                VadEvent::None => {}
            }
            if let Some(sess) = session.as_mut() {
                let segs = sess.push_audio(&frame).context("whisper push")?;
                if !segs.is_empty() {
                    print_segments(
                        &segs,
                        "part ",
                        &anchor,
                        audio_ms_total,
                        speech_start_audio_ms,
                    );
                }
            }
        }
    }
}

fn wall_ms(anchor: &Option<Instant>) -> f64 {
    anchor
        .map(|a| a.elapsed().as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

fn print_event(name: &str, anchor: &Option<Instant>, audio_ms: u64) {
    println!(
        "[{:7.0}ms] {:5}                       audio={:6}ms",
        wall_ms(anchor),
        name,
        audio_ms
    );
}

fn print_segments(
    segs: &[TranscriptSegment],
    tag: &str,
    anchor: &Option<Instant>,
    audio_ms: u64,
    speech_offset_ms: u64,
) {
    let wm = wall_ms(anchor);
    for seg in segs {
        let seg_end_global = speech_offset_ms + seg.end_ms;
        let lag = wm - seg_end_global as f64;
        let text = seg.text.trim();
        println!(
            "[{:7.0}ms] {}  lag={:6.0}ms  audio={:6}ms  seg={:5}..{:<5}ms  {}",
            wm, tag, lag, audio_ms, seg.start_ms, seg.end_ms, text
        );
    }
}
