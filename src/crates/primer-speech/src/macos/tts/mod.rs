//! AVSpeechSynthesizer one-shot and streaming TTS backend.
//!
//! # Delivery contract
//!
//! `AVSpeechSynthesizer.writeUtterance:toBufferCallback:` delivers all PCM
//! callbacks on the **GCD main queue** (Apple doc: "The method calls the buffer
//! callback asynchronously on the main thread"). This is non-negotiable — no
//! delegate queue, no custom executor.
//!
//! # Architecture
//!
//! Two execution paths depending on the calling context:
//!
//! **Main-thread path** (test binary; GUI running on main thread):
//!   `writeUtterance:` is called directly. The async loop then drives the
//!   current-thread NSRunLoop in 10 ms slices, yielding to the tokio executor
//!   between slices. This ensures the main queue is drained between polls.
//!
//! **Background-thread path** (GUI background worker; CLI `spawn_blocking`):
//!   `DispatchQueue::main().exec_async(...)` submits the synthesis call to
//!   run on the main thread (which already has its CFRunLoop spinning), then
//!   the pool thread drains an `mpsc::channel` via `recv_timeout`. PCM
//!   callbacks (which run on the GCD main queue) feed `SynthesisEvent::Audio`
//!   and the zero-frame EOS sentinel feeds `SynthesisEvent::PhraseEnd`,
//!   which terminates the drain loop.
//!
//! # NSRunLoop vs GCD
//!
//! `NSRunLoop.runUntilDate` on the main thread drains both the NSRunLoop sources
//! AND the GCD main queue (because the GCD main queue is backed by the main
//! CFRunLoop). This is why 10 ms slice draining works: AVSpeechSynthesizer's
//! internal audio pipeline uses GCD to dispatch PCM buffers back to the main
//! queue, and each `runUntilDate` slice delivers those queued callbacks.

use async_trait::async_trait;
use objc2::rc::Retained;
use objc2_avf_audio::{AVSampleRateKey, AVSpeechSynthesisVoice};
use objc2_foundation::{NSNumber, NSString};

use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;
use primer_core::speech::{
    AudioBuffer, Named, StreamingTextToSpeech, SynthesisEvent, SynthesisSession, TextToSpeech,
    VoiceProfile,
};

use crate::PhraseSplitter;

use super::voice::select_voice;

/// Backend identifier returned by `Named::name()` and used in logs.
const BACKEND_NAME: &str = "macos-native-tts";

mod synthesis;

use synthesis::synthesize_streaming;

/// Query the native PCM sample rate that `AVSpeechSynthesizer` will use for
/// `voice`. The rate is encoded in the dictionary returned by
/// `AVSpeechSynthesisVoice.audioFileSettings` under the key `AVSampleRateKey`.
///
/// Compact (Default) voices deliver 22 050 Hz; Enhanced / Premium neural
/// voices deliver 24 000 Hz on macOS 13+. The value is available at
/// construction time, before any synthesis call, so we read it once here
/// rather than trusting either a hardcoded constant or the first PCM callback.
///
/// Falls back to 22 050 Hz if the key is missing or the value cannot be
/// interpreted — wrong rate here is an 8.8% pitch/speed error; wrong rate in
/// the PCM callback's `sample_rate` field (always correct, from the
/// `AVAudioFormat` the synthesiser delivers) would be far worse, so we keep
/// the fallback path and emit a warning.
fn voice_native_sample_rate(voice: &AVSpeechSynthesisVoice) -> u32 {
    // SAFETY: `audioFileSettings` is documented as "not atomic" (same caveat
    // as `language()`/`identifier()` in voice.rs) — we call it from a single
    // thread with no concurrent mutation of the voice object.
    let settings = unsafe { voice.audioFileSettings() };

    // SAFETY: the static `AVSampleRateKey` is initialised by the AVFoundation
    // framework before any user code runs; `Option::unwrap` is safe here.
    let key: &NSString = unsafe { AVSampleRateKey }.expect("AVSampleRateKey must not be nil");

    // NSDictionary<NSString, AnyObject>::objectForKey returns
    // Option<Retained<AnyObject>>. We downcast to NSNumber to read the float.
    let Some(obj) = settings.objectForKey(key) else {
        tracing::warn!(
            target: "primer::speech::macos",
            "AVSpeechSynthesisVoice.audioFileSettings missing AVSampleRateKey; \
             falling back to 22050 Hz (Enhanced voices deliver 24000 Hz — the \
             resampler will be constructed at 22050 Hz and audio may be slightly \
             slow/low-pitched)"
        );
        return 22_050;
    };

    let Some(number) = obj.downcast_ref::<NSNumber>() else {
        tracing::warn!(
            target: "primer::speech::macos",
            "AVSampleRateKey value is not NSNumber; falling back to 22050 Hz"
        );
        return 22_050;
    };

    let rate = number.as_f64() as u32;
    if rate == 0 {
        tracing::warn!(
            target: "primer::speech::macos",
            "AVSampleRateKey returned 0; falling back to 22050 Hz"
        );
        return 22_050;
    }
    rate
}

/// macOS-native TTS backend using `AVSpeechSynthesizer`.
///
/// Each instance is bound to a single locale and holds the best available
/// system voice for that locale. Cheap to construct; the AVSpeechSynthesizer
/// itself is allocated fresh per call to avoid `Send`/`Sync` complications.
pub struct MacosTextToSpeech {
    locale: Locale,
    voice: Retained<AVSpeechSynthesisVoice>,
    /// The native PCM sample rate for the selected voice, queried from
    /// `AVSpeechSynthesisVoice.audioFileSettings` at construction time.
    /// Compact (Default) voices deliver 22 050 Hz; Enhanced/Premium neural
    /// voices deliver 24 000 Hz. Used by `StreamingTextToSpeech::sample_rate()`
    /// so the output resampler in `build_local_backends_macos_native` is
    /// constructed at the correct rate.
    native_sample_rate: u32,
}

impl MacosTextToSpeech {
    /// Create a new backend for `bcp47` (e.g. `"en-US"`, `"de-DE"`).
    ///
    /// Returns `Err` if the BCP-47 tag is not supported or no system voice
    /// is installed for the locale.
    pub fn new(bcp47: &str) -> Result<Self> {
        let locale = match bcp47 {
            "en-US" => Locale::English,
            "de-DE" => Locale::German,
            other => {
                return Err(PrimerError::Speech(format!(
                    "unsupported BCP-47 locale for macOS TTS: {other}"
                )));
            }
        };

        let selection = select_voice(&locale).ok_or_else(|| {
            PrimerError::Speech(format!("no system voice available for locale {bcp47}"))
        })?;

        let native_sample_rate = voice_native_sample_rate(&selection.voice);
        // INFO-level on purpose: voice quality (Default / Premium / Enhanced)
        // is the single biggest lever for the macOS-native TTS user experience.
        // Default = robotic Compact voice; Enhanced/Premium = neural. Surfacing
        // the selected voice's identifier + quality at startup makes it obvious
        // when the user is hearing a Compact voice and should install a neural
        // one via System Settings → Accessibility → Spoken Content → System
        // Voice → Manage Voices.
        tracing::info!(
            target: "primer::speech::macos",
            bcp47,
            voice = %selection.identifier,
            quality = ?selection.quality,
            native_sample_rate,
            "MacosTextToSpeech: selected system voice"
        );

        Ok(Self {
            locale,
            voice: selection.voice,
            native_sample_rate,
        })
    }

    /// The locale this backend was constructed for.
    pub fn locale(&self) -> Locale {
        self.locale
    }
}

impl Named for MacosTextToSpeech {
    fn name(&self) -> &str {
        BACKEND_NAME
    }
}

// SAFETY: `MacosTextToSpeech` is `Send + Sync`:
//   - `locale: Locale` is `Copy`.
//   - `voice: Retained<AVSpeechSynthesisVoice>` is a read-only value object.
// All ObjC synthesis work happens inside `synthesize`, not on shared state.
unsafe impl Send for MacosTextToSpeech {}
unsafe impl Sync for MacosTextToSpeech {}

// ═══════════════════════════════════════════════════════════════════════════
// One-shot TextToSpeech impl
// ═══════════════════════════════════════════════════════════════════════════

#[async_trait]
impl TextToSpeech for MacosTextToSpeech {
    async fn synthesize(&self, text: &str, _voice: &VoiceProfile) -> Result<AudioBuffer> {
        // Main-thread fast path: skip spawn_blocking (it would force a worker
        // hop and lose the optimisation where main-thread callers can synthesise
        // synchronously without GCD bouncing). `synthesize_streaming` detects
        // main-thread and takes the main-thread path.
        if objc2_foundation::NSThread::isMainThread_class() {
            return synthesize_to_buffer(&self.voice, text, self.native_sample_rate);
        }
        // Off main thread (typical for tokio worker callers): spawn_blocking so
        // the synchronous `synthesize_streaming` doesn't stall the runtime.
        // Inside the blocking task `synthesize_streaming` will detect
        // not-main-thread and take the GCD-bounce path.
        let text_owned = text.to_owned();
        let voice_retained = self.voice.clone();
        let native_sample_rate = self.native_sample_rate;
        tokio::task::spawn_blocking(move || {
            synthesize_to_buffer(&voice_retained, &text_owned, native_sample_rate)
        })
        .await
        .map_err(|e| PrimerError::Speech(format!("spawn_blocking panicked: {e}")))?
    }
}

/// Drive [`synthesize_streaming`] with a local accumulator and return
/// the concatenated audio as a single [`AudioBuffer`]. Replaces the
/// Stage-A `synthesize_to_chunks` + `chunks_to_audio_buffer` pair so
/// the one-shot path and streaming path share one synthesis code path.
///
/// `sample_rate` is captured from the first `Audio` event. If no `Audio`
/// event arrives (empty input, silent-only synthesis), `fallback_sample_rate`
/// — the voice's `native_sample_rate` queried at backend construction — is
/// used instead. The previous behaviour returned `sample_rate: 0`, which was
/// a divide-by-zero footgun for downstream sinks/resamplers; the fallback
/// keeps the buffer's sample_rate field consistent with what a non-empty
/// synthesis from the same voice would have produced.
fn synthesize_to_buffer(
    voice: &AVSpeechSynthesisVoice,
    text: &str,
    fallback_sample_rate: u32,
) -> Result<AudioBuffer> {
    let mut samples: Vec<f32> = Vec::new();
    let mut sample_rate: Option<u32> = None;
    synthesize_streaming(voice, text, &mut |event| {
        if let SynthesisEvent::Audio(chunk) = event {
            sample_rate.get_or_insert(chunk.sample_rate);
            samples.extend(chunk.samples);
        }
    })?;
    Ok(AudioBuffer {
        samples,
        sample_rate: sample_rate.unwrap_or(fallback_sample_rate),
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Streaming TextToSpeech impl
// ═══════════════════════════════════════════════════════════════════════════

impl StreamingTextToSpeech for MacosTextToSpeech {
    /// Returns the voice's actual native PCM sample rate, queried from
    /// `AVSpeechSynthesisVoice.audioFileSettings` at construction time.
    /// Compact (Default) voices deliver 22 050 Hz; Enhanced/Premium neural
    /// voices deliver 24 000 Hz on macOS 13+.
    ///
    /// Each `AudioChunk` also carries its own `sample_rate` field (always
    /// correct, from the `AVAudioFormat` in the PCM callback), but this
    /// trait method is the value that `build_local_backends_macos_native`
    /// reads at builder time to construct the output resampler. Returning
    /// the wrong rate here caused audio to play ~8.8% too slowly and one
    /// semitone too low when an Enhanced voice was active.
    fn sample_rate(&self) -> u32 {
        self.native_sample_rate
    }

    fn open_session(&self, _voice: &VoiceProfile) -> Result<Box<dyn SynthesisSession>> {
        let voice = self.voice.clone();

        let session = MacosTtsSession {
            voice,
            splitter: PhraseSplitter::new(),
        };

        // Pre-warm: synthesize a silent space to absorb the ~380-640 ms
        // per-voice cold-start cost so the first real `push_text` call
        // returns audio promptly. Failure is silently ignored — a broken
        // voice must not prevent session creation. Events are
        // discarded; we only need the AVSpeechSynthesizer initialisation
        // side-effects.
        let _ = synthesize_streaming(&session.voice, " ", &mut |_event| {});

        Ok(Box::new(session))
    }
}

/// Per-turn synthesis session. `!Sync` by construction (owns its own
/// `AVSpeechSynthesisVoice` retained pointer and `PhraseSplitter` state).
/// Each session drives one `synthesize_streaming` invocation per phrase
/// (which dispatches to the main-thread or background-thread path
/// based on the calling context).
struct MacosTtsSession {
    voice: Retained<AVSpeechSynthesisVoice>,
    splitter: PhraseSplitter,
}

// SAFETY: `MacosTtsSession` owns `voice` exclusively after construction
// (no shared references from `MacosTextToSpeech`). All synthesis runs on
// the thread that owns the session. `Send` is required by `SynthesisSession`.
unsafe impl Send for MacosTtsSession {}

impl SynthesisSession for MacosTtsSession {
    /// Stream PCM events for each phrase via [`synthesize_streaming`]:
    /// each PCM callback from AVSpeechSynthesizer flows through an
    /// unbounded `mpsc::channel` and fires `Audio(chunk)` on the
    /// consumer; the zero-frame EOS sentinel fires `PhraseEnd`.
    /// Per-phrase TTFA is ~50 ms (first PCM callback) instead of
    /// ~hundreds of ms (full phrase coalesce). Closes #114.
    fn push_text(&mut self, text: &str, on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
        for phrase in self.splitter.push(text) {
            synthesize_streaming(&self.voice, &phrase, on_event)?;
        }
        Ok(())
    }

    fn finalize(mut self: Box<Self>, on_event: &mut dyn FnMut(SynthesisEvent)) -> Result<()> {
        if let Some(tail) = self.splitter.flush() {
            synthesize_streaming(&self.voice, &tail, on_event)?;
        }
        Ok(())
    }
}
