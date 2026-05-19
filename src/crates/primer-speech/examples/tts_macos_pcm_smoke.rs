//! Smoke binary for the AVSpeechSynthesizer PCM-callback path.
//!
//! Answers ONE specific question before any production code lands:
//!
//!     Does AVSpeechSynthesizer.writeUtterance:toBufferCallback: emit
//!     small enough chunks fast enough for the Primer's LISTEN↔SPEAK
//!     transition (target: first chunk < 800 ms, per-chunk < 300 ms —
//!     calibrated to cover both en-US/Samantha ~380 ms and
//!     de-DE/Anna ~640 ms per-call startup)?
//!
//! Apple's docs are silent on chunk size; the answer depends on the voice
//! and possibly the macOS version. This binary measures it on the user's
//! actual machine.
//!
//! Output: one row per callback invocation, plus a one-line verdict line
//! that downstream scripts/CI can grep. Also writes a 16-bit mono WAV so
//! you can sanity-check that the concatenated audio is intelligible.
//!
//! ```text
//! cargo run --example tts_macos_pcm_smoke -p primer-speech \
//!     --features _macos_smoke_check -- \
//!     --voice "com.apple.voice.compact.en-US.Samantha" \
//!     --text "Hello, what would you like to learn about today?" \
//!     --out /tmp/hello_macos.wav
//! ```
//!
//! The `--voice` argument is the voice identifier (find candidates with
//! `say --voice='?'` in a Terminal). Pass an empty string to let
//! AVSpeechSynthesizer pick the system default for the user's locale.
//!
//! On non-macOS hosts the binary is a no-op that prints a one-line
//! message and exits 0 — keeps `cargo build --all-targets` green on
//! Linux/Windows CI.

fn main() {
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = imp::run() {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("tts_macos_pcm_smoke: macOS only — no-op on this platform.");
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use clap::Parser;
    use hound::{SampleFormat, WavSpec, WavWriter};
    use objc2::rc::Retained;
    use objc2_avf_audio::{
        AVAudioBuffer, AVAudioPCMBuffer, AVSpeechSynthesisVoice, AVSpeechSynthesizer,
        AVSpeechUtterance,
    };
    use objc2_foundation::{NSDate, NSRunLoop, NSString};

    /// Default phrase the smoke binary speaks if `--text` is omitted. Roughly
    /// 1.5–2.0 s of audio at a normal speaking rate — long enough to observe
    /// at least a dozen PCM-callback invocations.
    const DEFAULT_PHRASE: &str = "Hello, what would you like to learn about today?";

    /// 16-bit mono WAV output.
    const WAV_BITS_PER_SAMPLE: u16 = 16;
    const WAV_CHANNELS: u16 = 1;
    /// f32 → i16 scale; AVSpeechSynthesizer's PCM is in the conventional
    /// [-1.0, 1.0] range. Multiplying by i16::MAX (32767) keeps the loudest
    /// possible peak in-range with no asymmetric clipping.
    const I16_SCALE: f32 = i16::MAX as f32;

    /// Pass thresholds. Empirically calibrated on macOS 15.x (May 2026):
    /// `writeUtterance:` carries a fixed startup cost — ~380 ms for
    /// `com.apple.voice.compact.en-US.Samantha`, ~640 ms for
    /// `com.apple.voice.compact.de-DE.Anna` — before any chunk emits,
    /// regardless of phrase length. The synth runs the full utterance
    /// internally and flushes all chunks at once via the NSRunLoop, so
    /// per-phrase synthesis via PhraseSplitter is essential to spread
    /// that startup cost across the Primer's response. Steady-state
    /// chunks are 256 frames / ~11 ms at 22050 Hz — well under any
    /// voice-loop concern. The 800 ms threshold buys headroom over the
    /// slower of the two shipping voices.
    const PASS_FIRST_CHUNK_LATENCY_MS: u64 = 800;
    const PASS_MAX_CHUNK_MS: u64 = 300;

    #[derive(Parser, Debug)]
    #[command(about = "AVSpeechSynthesizer PCM chunk-size smoke binary")]
    struct Args {
        /// Voice identifier (e.g. com.apple.voice.compact.en-US.Samantha,
        /// com.apple.voice.compact.de-DE.Anna). Empty string ⇒ system default
        /// for the user's locale.
        #[arg(long, default_value = "")]
        voice: String,

        /// Phrase to synthesise.
        #[arg(long, default_value = DEFAULT_PHRASE)]
        text: String,

        /// WAV output path.
        #[arg(long, default_value = "/tmp/hello_macos.wav")]
        out: PathBuf,

        /// Print an explicit time-to-first-audio summary block after the
        /// per-callback rows: writeUtterance → first PCM, writeUtterance →
        /// EOS, and the streaming "win" (= PhraseEnd − TTFA, the gap that
        /// the coalesce path used to waste before issue #114). No
        /// assertion; instrumentation only — re-runs after macOS major
        /// releases get a directly comparable verdict line.
        #[arg(long)]
        measure_ttfa: bool,
    }

    /// One entry per PCM-callback invocation. Recorded on the callback
    /// thread; printed after synthesis completes.
    #[derive(Debug, Clone)]
    struct ChunkRecord {
        t_from_start_ms: u64,
        frames: usize,
        sample_rate: u32,
        samples: Vec<f32>,
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse();

        // ── 1. Build the synthesizer + utterance ────────────────────────
        // SAFETY: AVSpeechSynthesizer::new returns a retained instance.
        let synth: Retained<AVSpeechSynthesizer> = unsafe { AVSpeechSynthesizer::new() };
        let ns_text: Retained<NSString> = NSString::from_str(&args.text);
        let utterance: Retained<AVSpeechUtterance> =
            unsafe { AVSpeechUtterance::speechUtteranceWithString(&ns_text) };

        // Resolve a voice. Always set one explicitly — empirically, leaving
        // `voice` nil causes `writeUtterance:toBufferCallback:` to return
        // immediately without invoking the callback at all on macOS 14+.
        // If --voice is omitted we fall back to voiceWithLanguage("en-US").
        let voice: Option<Retained<AVSpeechSynthesisVoice>> = if !args.voice.is_empty() {
            let ns_voice_id: Retained<NSString> = NSString::from_str(&args.voice);
            unsafe { AVSpeechSynthesisVoice::voiceWithIdentifier(&ns_voice_id) }
        } else {
            let ns_lang: Retained<NSString> = NSString::from_str("en-US");
            unsafe { AVSpeechSynthesisVoice::voiceWithLanguage(Some(&ns_lang)) }
        };
        match &voice {
            Some(v) => {
                let lang = unsafe { v.language() };
                let id = unsafe { v.identifier() };
                eprintln!("using voice id={id} lang={lang}");
                unsafe { utterance.setVoice(Some(v)) };
            }
            None => {
                eprintln!(
                    "warning: voice `{}` not found and no en-US fallback — synthesis will likely emit zero chunks",
                    args.voice
                );
            }
        }

        // ── 2. Set up the PCM-callback collector ────────────────────────
        let records: Arc<Mutex<Vec<ChunkRecord>>> = Arc::new(Mutex::new(Vec::new()));
        let records_in_cb = Arc::clone(&records);
        // EOS flag: flipped to true when the callback receives a zero-frame
        // buffer (Apple's end-of-utterance signal). The run-loop drain
        // breaks out when this fires (or on timeout).
        let eos = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let eos_in_cb = Arc::clone(&eos);
        let started = Instant::now();

        // The callback receives an AVAudioBuffer (abstract). The concrete
        // type for AVSpeechSynthesizer output is AVAudioPCMBuffer. We
        // downcast inside the closure and pull mono f32 PCM out.
        //
        // NOTE on the exact downcast spelling: objc2 0.6 exposes both
        // `Retained::downcast` (returns Result<Retained<T>, Retained<U>>)
        // and the bound-checked `AnyObject::downcast_ref`. The cleanest
        // pattern at the moment of writing is the bound-checked one,
        // because we want a `&AVAudioPCMBuffer` (no extra retain) inside a
        // short-lived closure. If the exact method name has shifted in
        // your objc2 version, replace with the equivalent — the structure
        // (read frameLength, format, floatChannelData) does not change.
        let cb = block2::RcBlock::new(move |buf_ptr: std::ptr::NonNull<AVAudioBuffer>| {
            // SAFETY: buf_ptr is non-null and points to a valid AVAudioBuffer
            // owned by the synthesizer for the duration of the callback.
            let buf: &AVAudioBuffer = unsafe { buf_ptr.as_ref() };

            // Downcast to AVAudioPCMBuffer. If for any reason the runtime
            // hands us a non-PCM buffer (shouldn't happen with the speech
            // synthesizer), just skip it — better to record zero chunks
            // than crash mid-smoke.
            let pcm: &AVAudioPCMBuffer = match buf.downcast_ref::<AVAudioPCMBuffer>() {
                Some(p) => p,
                None => return,
            };

            // SAFETY: all of these are property getters on a retained PCM buffer.
            let frame_length = unsafe { pcm.frameLength() } as usize;

            // AVSpeechSynthesizer signals end-of-utterance with a zero-frame
            // buffer. Record it for the verdict (it lets us measure total
            // wallclock) but don't try to read PCM data.
            if frame_length == 0 {
                records_in_cb.lock().unwrap().push(ChunkRecord {
                    t_from_start_ms: started.elapsed().as_millis() as u64,
                    frames: 0,
                    sample_rate: 0,
                    samples: Vec::new(),
                });
                eos_in_cb.store(true, std::sync::atomic::Ordering::SeqCst);
                return;
            }

            let format = unsafe { pcm.format() };
            let sample_rate = unsafe { format.sampleRate() } as u32;

            // `floatChannelData` returns a `**Float` (UnsafeMutablePointer<UnsafeMutablePointer<Float>>);
            // channel 0 is the only one for mono speech output. Treat the
            // resulting `*mut f32` as a slice of `frame_length` samples.
            let data_ptr = unsafe { pcm.floatChannelData() };
            if data_ptr.is_null() {
                return;
            }
            // SAFETY: data_ptr[0] is a valid mono float channel of
            // `frame_length` frames; the synthesizer retains the backing
            // buffer for the lifetime of the callback so the slice read
            // is sound. `floatChannelData` returns
            // `*mut NonNull<f32>` — first deref gives the per-channel
            // NonNull, `.as_ptr()` strips the wrapper.
            let chan0_nn: std::ptr::NonNull<f32> = unsafe { *data_ptr };
            let chan0_ptr: *mut f32 = chan0_nn.as_ptr();
            let slice: &[f32] = unsafe { std::slice::from_raw_parts(chan0_ptr, frame_length) };

            records_in_cb.lock().unwrap().push(ChunkRecord {
                t_from_start_ms: started.elapsed().as_millis() as u64,
                frames: frame_length,
                sample_rate,
                samples: slice.to_vec(),
            });
        });

        // ── 3. Drive synthesis (blocking) ───────────────────────────────
        // SAFETY: writeUtterance:toBufferCallback: drives synthesis on the
        // calling thread and invokes `cb` once per PCM buffer plus once
        // with frame_length=0 at end-of-utterance. The closure is retained
        // by RcBlock for the duration of the call.
        //
        // The generated binding's `AVSpeechSynthesizerBufferCallback`
        // resolves to `*mut Block<...>`; `&*cb` gives a `&Block`, which we
        // cast to the expected raw pointer. `RcBlock` keeps the block
        // alive across the call so this pointer is sound.
        type CbBlock = block2::Block<dyn Fn(std::ptr::NonNull<AVAudioBuffer>)>;
        let block_ref: &CbBlock = &cb;
        let block_ptr: *mut CbBlock = (block_ref as *const CbBlock).cast_mut();
        unsafe { synth.writeUtterance_toBufferCallback(&utterance, block_ptr) };

        // Drive the current thread's NSRunLoop in 100 ms slices until the
        // EOS marker arrives or we hit a sanity-cap timeout. Empirically,
        // `writeUtterance:toBufferCallback:` schedules work asynchronously
        // and returns instantly; the callback only fires once the run loop
        // is driven. In production this means the TTS backend MUST own a
        // thread that drives the run loop for the duration of synthesis
        // (typically via `tokio::task::spawn_blocking` calling
        // `runUntilDate` in a loop until EOS).
        // NSRunLoop's safe methods (no `unsafe` needed in objc2 0.6 — the
        // generated bindings mark currentRunLoop / runUntilDate as safe).
        let run_loop = NSRunLoop::currentRunLoop();
        let drain_deadline = Instant::now() + std::time::Duration::from_secs(30);
        while !eos.load(std::sync::atomic::Ordering::SeqCst) {
            if Instant::now() >= drain_deadline {
                eprintln!("warning: 30s NSRunLoop drain timeout — giving up");
                break;
            }
            let date = NSDate::dateWithTimeIntervalSinceNow(0.1);
            run_loop.runUntilDate(&date);
        }

        let total_wallclock = started.elapsed();

        // ── 4. Report per-chunk metrics ─────────────────────────────────
        let records = records.lock().unwrap().clone();

        println!(
            "{:>6} {:>13} {:>10} {:>10} {:>10}",
            "chunk", "t_from_start", "frames", "chunk_ms", "rate"
        );
        println!("{}", "-".repeat(56));

        let mut audio_chunks = 0usize;
        let mut first_chunk_latency_ms: Option<u64> = None;
        let mut max_chunk_ms: u64 = 0;
        let mut total_frames = 0usize;
        let mut emitted_sample_rate: u32 = 0;
        let mut combined_samples: Vec<f32> = Vec::new();

        for (i, r) in records.iter().enumerate() {
            if r.frames == 0 {
                println!(
                    "{:>6} {:>13} {:>10} {:>10} {:>10}",
                    i, r.t_from_start_ms, "<EOS>", "-", "-"
                );
                continue;
            }
            let chunk_ms = (r.frames as u64 * 1_000) / r.sample_rate as u64;
            if first_chunk_latency_ms.is_none() {
                first_chunk_latency_ms = Some(r.t_from_start_ms);
            }
            if chunk_ms > max_chunk_ms {
                max_chunk_ms = chunk_ms;
            }
            if emitted_sample_rate == 0 {
                emitted_sample_rate = r.sample_rate;
            }
            audio_chunks += 1;
            total_frames += r.frames;
            combined_samples.extend_from_slice(&r.samples);

            println!(
                "{:>6} {:>13} {:>10} {:>10} {:>10}",
                i, r.t_from_start_ms, r.frames, chunk_ms, r.sample_rate
            );
        }
        println!("{}", "-".repeat(56));

        // ── 5. Verdict line (greppable by CI / scripts) ─────────────────
        let first_latency = first_chunk_latency_ms.unwrap_or(u64::MAX);
        println!(
            "FIRST_CHUNK_LATENCY_MS={first_latency}  MAX_CHUNK_MS={max_chunk_ms}  TOTAL_CHUNKS={audio_chunks}  WALLCLOCK_MS={}",
            total_wallclock.as_millis()
        );

        let pass = audio_chunks > 0
            && first_latency < PASS_FIRST_CHUNK_LATENCY_MS
            && max_chunk_ms < PASS_MAX_CHUNK_MS;
        println!(
            "VERDICT={}",
            if pass {
                "PASS — proceed to Task 1"
            } else {
                "FAIL — stop, write findings before Task 1"
            }
        );

        // ── 5b. Optional TTFA summary (issue #114) ──────────────────────
        if args.measure_ttfa {
            let phrase_end_ms = records
                .iter()
                .find(|r| r.frames == 0)
                .map(|r| r.t_from_start_ms);
            match (first_chunk_latency_ms, phrase_end_ms) {
                (Some(ttfa_ms), Some(eos_ms)) => {
                    let streaming_win_ms = eos_ms.saturating_sub(ttfa_ms);
                    println!(
                        "[smoke] TTFA: {ttfa_ms} ms (writeUtterance → first PCM callback) for voice {:?}",
                        args.voice
                    );
                    println!(
                        "[smoke] PhraseEnd: {eos_ms} ms (writeUtterance → EOS) for voice {:?}",
                        args.voice
                    );
                    println!(
                        "[smoke] Streaming win: {eos_ms} - {ttfa_ms} = {streaming_win_ms} ms earlier than coalesce"
                    );
                }
                _ => {
                    println!(
                        "[smoke] TTFA: unavailable (no audio chunks emitted; voice {:?} may be unsupported)",
                        args.voice
                    );
                }
            }
        }

        // ── 6. Write WAV for ear-check ──────────────────────────────────
        if combined_samples.is_empty() {
            eprintln!("no audio captured; skipping WAV write");
            return Ok(());
        }
        let spec = WavSpec {
            channels: WAV_CHANNELS,
            sample_rate: emitted_sample_rate,
            bits_per_sample: WAV_BITS_PER_SAMPLE,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::create(&args.out, spec)?;
        for s in &combined_samples {
            let clamped = s.clamp(-1.0, 1.0);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            writer.write_sample((clamped * I16_SCALE).round() as i16)?;
        }
        writer.finalize()?;
        #[allow(clippy::cast_precision_loss)]
        let audio_secs = total_frames as f32 / emitted_sample_rate as f32;
        println!(
            "wrote {} ({audio_secs:.2}s of audio at {emitted_sample_rate} Hz)",
            args.out.display()
        );
        Ok(())
    }
}
