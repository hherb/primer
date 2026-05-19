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
use primer_core::speech::{StreamingTextToSpeech, SynthesisEvent, VoiceProfile};
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
    let mut collect = |e: SynthesisEvent| {
        if let SynthesisEvent::Audio(chunk) = e {
            samples.extend(chunk.samples);
        }
    };
    session.push_text(SMOKE_PHRASE, &mut collect)?;
    session.finalize(&mut collect)?;

    let spec = WavSpec {
        channels: WAV_CHANNELS,
        sample_rate,
        bits_per_sample: WAV_BITS_PER_SAMPLE,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(&args.out, spec)?;
    for s in &samples {
        let clamped = s.clamp(-1.0, 1.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        writer.write_sample((clamped * I16_SCALE).round() as i16)?;
    }
    writer.finalize()?;

    let elapsed = started.elapsed();
    #[allow(clippy::cast_precision_loss)]
    let audio_secs = samples.len() as f32 / sample_rate as f32;
    println!(
        "wrote {} samples ({audio_secs:.2}s of audio) at {sample_rate} Hz to {} in {elapsed:?}",
        samples.len(),
        args.out.display()
    );
    Ok(())
}
