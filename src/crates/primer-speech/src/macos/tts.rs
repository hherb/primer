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
//!   `dispatch_async_f` to the main queue submits the synthesis call to run on
//!   the main thread (which already has its CFRunLoop spinning), then the pool
//!   thread drains an `mpsc::channel` via `recv_timeout`. PCM callbacks (which
//!   run on the GCD main queue) feed `SynthesisEvent::Audio` and the zero-frame
//!   EOS sentinel feeds `SynthesisEvent::PhraseEnd`, which terminates the
//!   drain loop.
//!
//! # NSRunLoop vs GCD
//!
//! `NSRunLoop.runUntilDate` on the main thread drains both the NSRunLoop sources
//! AND the GCD main queue (because the GCD main queue is backed by the main
//! CFRunLoop). This is why 10 ms slice draining works: AVSpeechSynthesizer's
//! internal audio pipeline uses GCD to dispatch PCM buffers back to the main
//! queue, and each `runUntilDate` slice delivers those queued callbacks.

use std::ptr::NonNull;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use objc2::rc::Retained;
use objc2_avf_audio::{
    AVAudioBuffer, AVAudioPCMBuffer, AVSampleRateKey, AVSpeechSynthesisVoice, AVSpeechSynthesizer,
    AVSpeechUtterance,
};
use objc2_foundation::{NSDate, NSNumber, NSRunLoop, NSString};

use primer_core::consts::speech::STREAM_DRAIN_POLL_MS;
use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;
use primer_core::speech::{
    AudioBuffer, AudioChunk, Named, StreamingTextToSpeech, SynthesisEvent, SynthesisSession,
    TextToSpeech, VoiceProfile,
};

use crate::PhraseSplitter;

use super::voice::select_voice;

/// Backend identifier returned by `Named::name()` and used in logs.
const BACKEND_NAME: &str = "macos-native-tts";

/// EOS sentinel: AVSpeechSynthesizer signals end-of-utterance with a
/// zero-frame buffer.
const EOS_FRAME_LENGTH: usize = 0;

/// Synthesis timeout. If no EOS sentinel arrives within this window,
/// `synthesize_to_chunks` returns an error.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// NSRunLoop slice for the main-thread poll path. Each `runUntilDate` call
/// blocks for this long, draining any pending GCD main-queue callbacks
/// (including AVSpeechSynthesizer PCM callbacks) before returning.
const RUN_LOOP_SLICE: Duration = Duration::from_millis(10);

/// Raw libdispatch bindings for the background-thread path.
///
/// libdispatch ships in `/usr/lib/system/libdispatch.dylib` and is
/// re-exported by `libSystem.B.dylib`, which every macOS binary links.
/// `dispatch_get_main_queue()` is an inline C function that returns a
/// pointer to `_dispatch_main_q`; we bind the underlying symbol directly.
///
/// After the streaming refactor (issue #114) the only dispatch primitives
/// we still need are `dispatch_async_f` and `_dispatch_main_q` — the
/// background streaming path drains a `mpsc::channel`'s
/// `SynthesisEvent::PhraseEnd` to signal completion instead of a
/// `dispatch_semaphore`. The `dispatch_semaphore_*` family was removed
/// together with the `DispatchSemaphore` RAII wrapper and `SynthCtx`.
///
// TODO: Replace raw GCD bindings (extern "C" + raw pointer types)
// with the `dispatch2` crate when it stabilises. The crate provides
// safe typed wrappers for dispatch_queue_t / dispatch_async_f and would
// eliminate the _dispatch_main_q private-symbol binding. See plan task
// 5 review notes (commit 37c3f79).
mod dispatch {
    #[allow(non_camel_case_types)]
    pub type dispatch_object_t = *mut std::ffi::c_void;
    #[allow(non_camel_case_types)]
    pub type dispatch_queue_t = dispatch_object_t;

    #[link(name = "System")]
    unsafe extern "C" {
        /// The GCD main queue object. `dispatch_get_main_queue()` in C is an
        /// inline function returning `&_dispatch_main_q`; we bind the symbol.
        pub static _dispatch_main_q: std::ffi::c_void;

        pub fn dispatch_async_f(
            queue: dispatch_queue_t,
            context: *mut std::ffi::c_void,
            work: extern "C" fn(*mut std::ffi::c_void),
        );
    }
}

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
// Streaming synthesis helpers — Stage B (issue #114)
// ═══════════════════════════════════════════════════════════════════════════

/// Synthesise `text` using `voice` and stream events to `on_event` as
/// PCM callbacks arrive (per-callback `Audio` events, then `PhraseEnd`
/// on the EOS sentinel). Dispatches to the main-thread path when called
/// on the OS main thread, or the background-thread GCD-bounce path
/// otherwise.
///
/// Per-phrase time-to-first-audio is ~50 ms (first PCM callback),
/// down from ~hundreds of ms under the pre-#114 full-phrase coalesce
/// path. Closes #114.
fn synthesize_streaming(
    voice: &AVSpeechSynthesisVoice,
    text: &str,
    on_event: &mut dyn FnMut(SynthesisEvent),
) -> Result<()> {
    let on_main = objc2_foundation::NSThread::isMainThread_class();
    if on_main {
        synthesize_streaming_main_thread(voice, text, on_event)
    } else {
        synthesize_streaming_background(voice, text, on_event)
    }
}

/// Main-thread streaming path: fires `on_event` for each PCM callback
/// as it arrives, interleaved with [`RUN_LOOP_SLICE`]-wide
/// `runUntilDate` slices. The zero-frame EOS sentinel converts to
/// `SynthesisEvent::PhraseEnd` and terminates the loop.
///
/// The receiver lives on this thread; the PCM-callback closure (which
/// runs on the GCD main queue, i.e. the same thread) sends events
/// through an **unbounded** `mpsc::channel`. A bounded channel cannot
/// work here: when full, `send` blocks the callback, which is itself
/// being driven by `runUntilDate` on the same thread — the consumer
/// can never re-enter the drain loop to make room. See
/// [`STREAM_DRAIN_POLL_MS`] doc for the wider invariant.
fn synthesize_streaming_main_thread(
    voice: &AVSpeechSynthesisVoice,
    text: &str,
    on_event: &mut dyn FnMut(SynthesisEvent),
) -> Result<()> {
    use std::sync::mpsc::channel;

    let (tx, rx) = channel::<SynthesisEvent>();

    // ── 1. Build synthesizer + utterance ────────────────────────────────
    // SAFETY: called on the main thread.
    let synth: Retained<AVSpeechSynthesizer> = unsafe { AVSpeechSynthesizer::new() };
    let ns_text = NSString::from_str(text);
    // SAFETY: factory method; ns_text lives for the scope.
    let utterance = unsafe { AVSpeechUtterance::speechUtteranceWithString(&ns_text) };
    // SAFETY: setVoice: on the main thread.
    unsafe { utterance.setVoice(Some(voice)) };

    // ── 2. Register the PCM callback ─────────────────────────────────────
    let tx_cb = tx.clone();

    type CbBlock = block2::Block<dyn Fn(NonNull<AVAudioBuffer>)>;
    let cb = block2::RcBlock::new(move |buf_ptr: NonNull<AVAudioBuffer>| {
        stream_pcm_callback(buf_ptr, &tx_cb);
    });

    // ── 3. Start synthesis (returns immediately; callbacks queued async) ─
    // SAFETY: called on the main thread; `cb` is retained by `RcBlock`
    // and additionally by AVSpeechSynthesizer's Block_retain.
    let block_ref: &CbBlock = &cb;
    let block_ptr: *mut CbBlock = (block_ref as *const CbBlock).cast_mut();
    unsafe { synth.writeUtterance_toBufferCallback(&utterance, block_ptr) };

    // ── 4. Drain loop: interleave runloop slices with channel drains ────
    let run_loop = NSRunLoop::mainRunLoop();
    let deadline = Instant::now() + DRAIN_TIMEOUT;
    loop {
        // Drain whatever the channel has now. PhraseEnd terminates.
        while let Ok(event) = rx.try_recv() {
            let is_phrase_end = matches!(event, SynthesisEvent::PhraseEnd);
            on_event(event);
            if is_phrase_end {
                // Drop the local sender so a stale Block_retain'd callback
                // closure dropping later isn't the last sender alive.
                // `cb` (RcBlock) goes out of scope here; ObjC's own
                // Block_retain keeps it alive for any late callbacks.
                drop(tx);
                drop(cb);
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            drop(tx);
            drop(cb);
            return Err(PrimerError::Speech(
                "AVSpeechSynthesizer 30s NSRunLoop drain timeout (main-thread streaming)".into(),
            ));
        }
        let date = NSDate::dateWithTimeIntervalSinceNow(RUN_LOOP_SLICE.as_secs_f64());
        run_loop.runUntilDate(&date);
    }
}

/// Background-thread streaming path: PCM callbacks (firing on the GCD
/// main queue) send events through an unbounded `mpsc::channel`; this
/// background thread drains the channel via `recv_timeout`
/// ([`STREAM_DRAIN_POLL_MS`]), emits events through `on_event`, and
/// exits on `PhraseEnd` or the 30 s overall deadline.
///
/// `SynthesisEvent::PhraseEnd` arriving on the channel IS the
/// synchronisation primitive (no separate `dispatch_semaphore`). The
/// trampoline still owns the utterance and the channel sender so the
/// closure stays alive for late callbacks (via Block_retain).
///
/// The channel is unbounded for the same reason as the main-thread
/// path: PCM callbacks run on the GCD main queue, which must not block.
/// See [`STREAM_DRAIN_POLL_MS`] for the wider invariant.
fn synthesize_streaming_background(
    voice: &AVSpeechSynthesisVoice,
    text: &str,
    on_event: &mut dyn FnMut(SynthesisEvent),
) -> Result<()> {
    use std::sync::mpsc::{RecvTimeoutError, channel};

    let (tx, rx) = channel::<SynthesisEvent>();

    // ── Build utterance on the calling thread ─────────────────────────
    let ns_text = NSString::from_str(text);
    // SAFETY: factory method.
    let utterance = unsafe { AVSpeechUtterance::speechUtteranceWithString(&ns_text) };
    // SAFETY: setVoice:.
    unsafe { utterance.setVoice(Some(voice)) };

    // ── Trampoline context: utterance + sender ────────────────────────
    struct StreamCtx {
        utterance: Retained<AVSpeechUtterance>,
        tx: std::sync::mpsc::Sender<SynthesisEvent>,
    }
    // SAFETY: `StreamCtx` is moved into the main-queue trampoline; at that
    // point no other thread holds a reference. The move is one-shot.
    unsafe impl Send for StreamCtx {}

    extern "C" fn trampoline(ctx_raw: *mut std::ffi::c_void) {
        // SAFETY: ctx_raw was Box::into_raw'd below; we take ownership here.
        let ctx = unsafe { Box::from_raw(ctx_raw as *mut StreamCtx) };

        // SAFETY: AVSpeechSynthesizer::new on the main thread.
        let synth: Retained<AVSpeechSynthesizer> = unsafe { AVSpeechSynthesizer::new() };

        let tx_cb = ctx.tx.clone();
        type CbBlock = block2::Block<dyn Fn(NonNull<AVAudioBuffer>)>;
        let cb = block2::RcBlock::new(move |buf_ptr: NonNull<AVAudioBuffer>| {
            stream_pcm_callback(buf_ptr, &tx_cb);
        });

        // SAFETY: called on the main thread.
        let block_ref: &CbBlock = &cb;
        let block_ptr: *mut CbBlock = (block_ref as *const CbBlock).cast_mut();
        unsafe { synth.writeUtterance_toBufferCallback(&ctx.utterance, block_ptr) };

        // `cb` drops here; AVSpeechSynthesizer's Block_retain keeps it
        // alive for any further callbacks. The closure's clone of `tx`
        // stays alive through Block_retain too; when the closure drops
        // after EOS, the sender drops and any later `recv` would return
        // `Disconnected` (which the caller below ignores after seeing
        // PhraseEnd).
    }

    let ctx = Box::new(StreamCtx { utterance, tx });
    let ctx_ptr = Box::into_raw(ctx) as *mut std::ffi::c_void;

    // SAFETY: `_dispatch_main_q` is the GCD main queue object.
    let main_queue: dispatch::dispatch_queue_t =
        unsafe { &dispatch::_dispatch_main_q as *const _ as dispatch::dispatch_queue_t };

    // SAFETY: `trampoline` takes ownership of `ctx_ptr` exactly once.
    unsafe { dispatch::dispatch_async_f(main_queue, ctx_ptr, trampoline) };

    // ── Drain loop ────────────────────────────────────────────────────
    let poll = Duration::from_millis(STREAM_DRAIN_POLL_MS);
    let deadline = Instant::now() + DRAIN_TIMEOUT;
    loop {
        match rx.recv_timeout(poll) {
            Ok(event) => {
                let is_phrase_end = matches!(event, SynthesisEvent::PhraseEnd);
                on_event(event);
                if is_phrase_end {
                    return Ok(());
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    return Err(PrimerError::Speech(
                        "AVSpeechSynthesizer 30s drain timeout (background streaming)".into(),
                    ));
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(PrimerError::Speech(
                    "synth channel disconnected before PhraseEnd".into(),
                ));
            }
        }
    }
}

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
            return synthesize_to_buffer(&self.voice, text);
        }
        // Off main thread (typical for tokio worker callers): spawn_blocking so
        // the synchronous `synthesize_streaming` doesn't stall the runtime.
        // Inside the blocking task `synthesize_streaming` will detect
        // not-main-thread and take the GCD-bounce path.
        let text_owned = text.to_owned();
        let voice_retained = self.voice.clone();
        tokio::task::spawn_blocking(move || synthesize_to_buffer(&voice_retained, &text_owned))
            .await
            .map_err(|e| PrimerError::Speech(format!("spawn_blocking panicked: {e}")))?
    }
}

/// Drive [`synthesize_streaming`] with a local accumulator and return
/// the concatenated audio as a single [`AudioBuffer`]. Replaces the
/// Stage-A `synthesize_to_chunks` + `chunks_to_audio_buffer` pair so
/// the one-shot path and streaming path share one synthesis code path.
fn synthesize_to_buffer(voice: &AVSpeechSynthesisVoice, text: &str) -> Result<AudioBuffer> {
    let mut samples: Vec<f32> = Vec::new();
    let mut sample_rate: u32 = 0;
    synthesize_streaming(voice, text, &mut |event| {
        if let SynthesisEvent::Audio(chunk) = event {
            if sample_rate == 0 {
                sample_rate = chunk.sample_rate;
            }
            samples.extend(chunk.samples);
        }
    })?;
    Ok(AudioBuffer {
        samples,
        sample_rate,
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

// ═══════════════════════════════════════════════════════════════════════════
// Shared PCM callback (streaming)
// ═══════════════════════════════════════════════════════════════════════════

/// Convert one PCM buffer from `AVSpeechSynthesizer` into a
/// `SynthesisEvent` and send it through `tx`. Zero-frame buffers (the
/// EOS sentinel) send `SynthesisEvent::PhraseEnd`. Used by both
/// streaming paths (main-thread + background).
///
/// `send` is non-blocking on the unbounded `mpsc::channel` used by
/// both streaming paths — required so the GCD main queue, which calls
/// this function, never blocks. Errors from `send` (receiver dropped)
/// are intentionally swallowed: the caller already exited (deadline /
/// early PhraseEnd) and no consumer remains.
fn stream_pcm_callback(
    buf_ptr: NonNull<AVAudioBuffer>,
    tx: &std::sync::mpsc::Sender<SynthesisEvent>,
) {
    // SAFETY: buf_ptr is non-null and valid for the callback's lifetime.
    let buf: &AVAudioBuffer = unsafe { buf_ptr.as_ref() };

    let pcm: &AVAudioPCMBuffer = match buf.downcast_ref::<AVAudioPCMBuffer>() {
        Some(p) => p,
        None => return,
    };

    // SAFETY: `frameLength` is an immutable property getter.
    let frame_length = unsafe { pcm.frameLength() } as usize;

    if frame_length == EOS_FRAME_LENGTH {
        let _ = tx.send(SynthesisEvent::PhraseEnd);
        return;
    }

    // SAFETY: immutable property getters.
    let format = unsafe { pcm.format() };
    let channel_count = unsafe { format.channelCount() };
    if channel_count != 1 {
        tracing::warn!(
            channel_count,
            "AVSpeechSynthesizer emitted multi-channel PCM; only channel 0 will be captured"
        );
    }
    let sample_rate = unsafe { format.sampleRate() } as u32;

    // SAFETY: `floatChannelData()[0]` is the mono channel of `frame_length`
    // samples; valid for the callback's lifetime.
    let data_ptr = unsafe { pcm.floatChannelData() };
    if data_ptr.is_null() {
        return;
    }
    let chan0_nn: NonNull<f32> = unsafe { *data_ptr };
    let chan0_ptr: *mut f32 = chan0_nn.as_ptr();
    // SAFETY: valid mono float slice for the callback's lifetime.
    let slice: &[f32] = unsafe { std::slice::from_raw_parts(chan0_ptr, frame_length) };

    let chunk = AudioChunk {
        samples: slice.to_vec(),
        sample_rate,
    };
    let _ = tx.send(SynthesisEvent::Audio(chunk));
}
