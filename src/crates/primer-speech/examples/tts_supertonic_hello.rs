//! Stage A smoke binary for the Supertonic TTS evaluation
//! (branch `claude/evaluate-supertonic-tts-fOKNd`).
//!
//! This deliberately does not implement the `TextToSpeech` /
//! `StreamingTextToSpeech` traits yet — it just exercises the vendored
//! `supertonic-tts` library end-to-end so we can confirm:
//!   * the rc.7 → rc.10 ort migration in `vendor/supertonic-rs` compiles
//!   * the four ONNX sessions load
//!   * a single utterance synthesises and lands as PCM in a WAV file
//!
//! Run:
//! ```text
//! cargo run --example tts_supertonic_hello --features supertonic -- \
//!   --onnx-dir /path/to/supertonic/assets/onnx \
//!   --voice-style /path/to/supertonic/assets/voice_styles/F1.json \
//!   --text "Hello, what would you like to learn about today?" \
//!   --lang en \
//!   --out hello.wav
//! ```
//!
//! Asset layout expected under `--onnx-dir`:
//!   - duration_predictor.onnx
//!   - text_encoder.onnx
//!   - vector_estimator.onnx
//!   - vocoder.onnx
//!   - tts.json (config; sample_rate lives here)
//!   - unicode_indexer.json
//!
//! Source the assets from the Supertonic HuggingFace repo (~400 MB):
//!   git clone https://huggingface.co/Supertone/supertonic
//!
//! Stage B will wrap this into a `SupertonicTts` impl of the two trait
//! families, with a 44.1 → 48 kHz resampler and a per-phrase chunker
//! mirroring the `PhraseSplitter` glue used by the Piper backend.

use std::path::PathBuf;
use std::time::Instant;

use clap::Parser;
use hound::{SampleFormat, WavSpec, WavWriter};
use supertonic_tts::helper::{load_text_to_speech, load_voice_style};

const WAV_BITS_PER_SAMPLE: u16 = 16;
const WAV_CHANNELS: u16 = 1;
/// f32 samples from supertonic are in [-1.0, 1.0]; clamp + scale to i16.
const I16_SCALE: f32 = i16::MAX as f32;

/// Denoising steps. Upstream default is 8 (quality knob, 5..=12).
const DEFAULT_TOTAL_STEPS: usize = 8;
/// Speed factor (>1.0 faster, <1.0 slower). Upstream example uses 1.05.
const DEFAULT_SPEED: f32 = 1.0;
/// Inter-chunk silence inserted between text chunks (s). The library
/// auto-chunks long inputs at ~300 chars; for short phrases this never
/// triggers but the value still has to be passed.
const DEFAULT_INTER_CHUNK_SILENCE_S: f32 = 0.3;

#[derive(Parser, Debug)]
#[command(about = "Supertonic TTS smoke binary (Stage A evaluation)")]
struct Args {
    /// Directory containing the four `*.onnx` files + `tts.json` + `unicode_indexer.json`.
    #[arg(long)]
    onnx_dir: PathBuf,
    /// Voice style JSON (e.g. `voice_styles/F1.json`).
    #[arg(long)]
    voice_style: PathBuf,
    /// Phrase to synthesise.
    #[arg(
        long,
        default_value = "Hello, what would you like to learn about today?"
    )]
    text: String,
    /// ISO-639-1 code; supertonic ships en/de/hi/ja/ko/es/fr + 24 more.
    #[arg(long, default_value = "en")]
    lang: String,
    /// Output WAV path.
    #[arg(long, default_value = "hello.wav")]
    out: PathBuf,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let onnx_dir = args
        .onnx_dir
        .to_str()
        .ok_or("--onnx-dir must be valid UTF-8")?;
    let voice_style_path = args
        .voice_style
        .to_str()
        .ok_or("--voice-style must be valid UTF-8")?;

    println!("Loading TTS components from {} …", onnx_dir);
    let load_start = Instant::now();
    let mut tts = load_text_to_speech(onnx_dir, /* use_gpu */ false)?;
    let sample_rate = tts.sample_rate as u32;
    println!(
        "  loaded in {:.2?} (sample_rate = {} Hz)",
        load_start.elapsed(),
        sample_rate,
    );

    println!("Loading voice style from {} …", voice_style_path);
    let style = load_voice_style(&[voice_style_path.to_string()], /* verbose */ true)?;

    println!("Synthesising: {:?} (lang={})", args.text, args.lang);
    let synth_start = Instant::now();
    let (wav, duration_s) = tts.call(
        &args.text,
        &args.lang,
        &style,
        DEFAULT_TOTAL_STEPS,
        DEFAULT_SPEED,
        DEFAULT_INTER_CHUNK_SILENCE_S,
    )?;
    let synth_elapsed = synth_start.elapsed();
    let rtf = synth_elapsed.as_secs_f32() / duration_s.max(1e-6);
    println!(
        "  {} samples ({:.2}s audio) in {:.2?}  →  RTF = {:.3}",
        wav.len(),
        duration_s,
        synth_elapsed,
        rtf,
    );

    let spec = WavSpec {
        channels: WAV_CHANNELS,
        sample_rate,
        bits_per_sample: WAV_BITS_PER_SAMPLE,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(&args.out, spec)?;
    for sample in &wav {
        let clamped = sample.clamp(-1.0, 1.0);
        writer.write_sample((clamped * I16_SCALE) as i16)?;
    }
    writer.finalize()?;
    println!(
        "Wrote {} ({} Hz, {}-bit mono)",
        args.out.display(),
        sample_rate,
        WAV_BITS_PER_SAMPLE
    );

    Ok(())
}
