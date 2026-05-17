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
//!   the main thread (which already has its CFRunLoop spinning), then a pool
//!   thread waits on a `dispatch_semaphore`. When the synthesis completes, the
//!   EOS callback signals the semaphore.
//!
//! # NSRunLoop vs GCD
//!
//! `NSRunLoop.runUntilDate` on the main thread drains both the NSRunLoop sources
//! AND the GCD main queue (because the GCD main queue is backed by the main
//! CFRunLoop). This is why 10 ms slice draining works: AVSpeechSynthesizer's
//! internal audio pipeline uses GCD to dispatch PCM buffers back to the main
//! queue, and each `runUntilDate` slice delivers those queued callbacks.

use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use objc2::rc::Retained;
use objc2_avf_audio::{
    AVAudioBuffer, AVAudioPCMBuffer, AVSampleRateKey, AVSpeechSynthesisVoice, AVSpeechSynthesizer,
    AVSpeechUtterance,
};
use objc2_foundation::{NSDate, NSNumber, NSRunLoop, NSString};

use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;
use primer_core::speech::{
    AudioBuffer, AudioChunk, Named, StreamingTextToSpeech, SynthesisSession, TextToSpeech,
    VoiceProfile,
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
// TODO: Replace raw GCD bindings (extern "C" + raw pointer types)
// with the `dispatch2` crate when it stabilises. The crate provides
// safe typed wrappers for dispatch_queue_t / dispatch_semaphore_t /
// dispatch_async_f and would eliminate the _dispatch_main_q
// private-symbol binding. See plan task 5 review notes (commit 37c3f79).
mod dispatch {
    #[allow(non_camel_case_types)]
    pub type dispatch_object_t = *mut std::ffi::c_void;
    #[allow(non_camel_case_types)]
    pub type dispatch_queue_t = dispatch_object_t;
    #[allow(non_camel_case_types)]
    pub type dispatch_semaphore_t = dispatch_object_t;
    #[allow(non_camel_case_types)]
    pub type dispatch_time_t = u64;

    pub const DISPATCH_TIME_NOW: dispatch_time_t = 0;

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
        pub fn dispatch_semaphore_create(value: std::ffi::c_long) -> dispatch_semaphore_t;
        pub fn dispatch_semaphore_signal(dsema: dispatch_semaphore_t) -> std::ffi::c_long;
        pub fn dispatch_semaphore_wait(
            dsema: dispatch_semaphore_t,
            timeout: dispatch_time_t,
        ) -> std::ffi::c_long;
        pub fn dispatch_release(obj: dispatch_object_t);
        pub fn dispatch_time(when: dispatch_time_t, delta: std::ffi::c_longlong)
        -> dispatch_time_t;
    }

    /// Maximum wait expressed as a `dispatch_time_t` value.
    /// 30 seconds × 10^9 ns/s.
    pub const TIMEOUT_NS: i64 = 30 * 1_000_000_000;
}

/// RAII wrapper for a GCD `dispatch_semaphore_t`. Calls `dispatch_release`
/// on `Drop`. Cloneable via `Arc` so both the background thread and the
/// trampoline can hold a strong reference; the last drop releases the
/// underlying semaphore — preventing the use-after-free that would occur
/// if the background thread released on timeout while the trampoline was
/// still queued on the main thread.
struct DispatchSemaphore(dispatch::dispatch_semaphore_t);

// SAFETY: `dispatch_semaphore_t` is thread-safe per Apple docs;
// `dispatch_semaphore_signal` / `dispatch_release` are also thread-safe.
unsafe impl Send for DispatchSemaphore {}
unsafe impl Sync for DispatchSemaphore {}

impl DispatchSemaphore {
    /// Return the raw handle for use with the `dispatch_semaphore_*` FFI.
    fn as_raw(&self) -> dispatch::dispatch_semaphore_t {
        self.0
    }
}

impl Drop for DispatchSemaphore {
    fn drop(&mut self) {
        // SAFETY: We hold exclusive ownership through `self.0`; `Drop` runs
        // exactly once per instance, never on a null/dangling pointer.
        unsafe { dispatch::dispatch_release(self.0) };
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
        tracing::debug!(
            target: "primer::speech::macos",
            bcp47,
            native_sample_rate,
            "MacosTextToSpeech: resolved voice native sample rate"
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

/// Shared accumulator type used in both synthesis paths.
type Accumulator = Arc<Mutex<Vec<AudioChunk>>>;

// ═══════════════════════════════════════════════════════════════════════════
// Core synthesis helper — shared by one-shot and streaming paths
// ═══════════════════════════════════════════════════════════════════════════

/// Synthesize `text` using `voice` and return the individual PCM chunks
/// exactly as AVSpeechSynthesizer delivered them (one chunk per callback
/// invocation, excluding the zero-frame EOS sentinel).
///
/// Dispatches to the main-thread path when called on the OS main thread,
/// or the background-thread GCD-bounce path otherwise.
///
/// The one-shot `synthesize` method concatenates the chunks into an
/// `AudioBuffer`; the streaming `push_text` / `finalize` methods return
/// the chunks directly.
fn synthesize_to_chunks(voice: &AVSpeechSynthesisVoice, text: &str) -> Result<Vec<AudioChunk>> {
    // `+[NSThread isMainThread]` is a thread-safe class method — safe from
    // any thread.
    let on_main = objc2_foundation::NSThread::isMainThread_class();

    if on_main {
        synthesize_to_chunks_main_thread(text, voice)
    } else {
        synthesize_to_chunks_background(text, voice)
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
        // synchronously without GCD bouncing). synthesize_to_chunks detects
        // main-thread and takes the main-thread path.
        if objc2_foundation::NSThread::isMainThread_class() {
            let chunks = synthesize_to_chunks(&self.voice, text)?;
            return Ok(chunks_to_audio_buffer(chunks));
        }
        // Off main thread (typical for tokio worker callers): spawn_blocking so
        // the synchronous synthesize_to_chunks doesn't stall the runtime. Inside
        // the blocking task, synthesize_to_chunks will detect not-main-thread and
        // take the GCD-bounce path.
        let text_owned = text.to_owned();
        let voice_retained = self.voice.clone();
        tokio::task::spawn_blocking(move || {
            let chunks = synthesize_to_chunks(&voice_retained, &text_owned)?;
            Ok::<_, PrimerError>(chunks_to_audio_buffer(chunks))
        })
        .await
        .map_err(|e| PrimerError::Speech(format!("spawn_blocking panicked: {e}")))?
    }
}

/// Concatenate a `Vec<AudioChunk>` into a single `AudioBuffer`.
/// Returns an empty buffer (sample_rate = 0) if `chunks` is empty.
fn chunks_to_audio_buffer(chunks: Vec<AudioChunk>) -> AudioBuffer {
    let sample_rate = chunks.first().map(|c| c.sample_rate).unwrap_or(0);
    let samples: Vec<f32> = chunks.into_iter().flat_map(|c| c.samples).collect();
    AudioBuffer {
        samples,
        sample_rate,
    }
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
        // voice must not prevent session creation.
        let _ = synthesize_to_chunks(&session.voice, " ");

        Ok(Box::new(session))
    }
}

/// Per-turn synthesis session. `!Sync` by construction (owns its own
/// `AVSpeechSynthesisVoice` retained pointer and `PhraseSplitter` state).
/// Each session drives the NSRunLoop / GCD semaphore machinery independently.
struct MacosTtsSession {
    voice: Retained<AVSpeechSynthesisVoice>,
    splitter: PhraseSplitter,
}

// SAFETY: `MacosTtsSession` owns `voice` exclusively after construction
// (no shared references from `MacosTextToSpeech`). All synthesis runs on
// the thread that owns the session. `Send` is required by `SynthesisSession`.
unsafe impl Send for MacosTtsSession {}

impl SynthesisSession for MacosTtsSession {
    fn push_text(&mut self, text: &str) -> Result<Vec<AudioChunk>> {
        let phrases = self.splitter.push(text);
        let mut chunks = Vec::new();
        for phrase in phrases {
            let phrase_chunks = synthesize_to_chunks(&self.voice, &phrase)?;
            chunks.extend(phrase_chunks);
        }
        Ok(chunks)
    }

    fn finalize(mut self: Box<Self>) -> Result<Vec<AudioChunk>> {
        let mut chunks = Vec::new();
        if let Some(tail) = self.splitter.flush() {
            let tail_chunks = synthesize_to_chunks(&self.voice, &tail)?;
            chunks.extend(tail_chunks);
        }
        Ok(chunks)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Main-thread path
// ═══════════════════════════════════════════════════════════════════════════

/// Synthesis from the OS main thread (synchronous). Returns one
/// `AudioChunk` per PCM callback that AVSpeechSynthesizer delivered.
///
/// Calls `writeUtterance:toBufferCallback:` on the current (main) thread,
/// then drives the main NSRunLoop in 10 ms slices — each slice delivers any
/// pending GCD main-queue callbacks (including AVSpeechSynthesizer PCM
/// buffers) and returns.
///
/// This is intentionally synchronous so that non-Send ObjC types never cross
/// an `.await` boundary in the calling `async fn synthesize`.
fn synthesize_to_chunks_main_thread(
    text: &str,
    voice: &AVSpeechSynthesisVoice,
) -> Result<Vec<AudioChunk>> {
    let accumulator: Accumulator = Arc::new(Mutex::new(Vec::new()));
    let eos = Arc::new(AtomicBool::new(false));

    // ── 1. Build synthesizer + utterance ────────────────────────────────
    // SAFETY: called on the main thread.
    let synth: Retained<AVSpeechSynthesizer> = unsafe { AVSpeechSynthesizer::new() };
    let ns_text = NSString::from_str(text);
    // SAFETY: factory method; ns_text lives for the scope.
    let utterance = unsafe { AVSpeechUtterance::speechUtteranceWithString(&ns_text) };
    // SAFETY: setVoice: on the main thread.
    unsafe { utterance.setVoice(Some(voice)) };

    // ── 2. Register the PCM callback ─────────────────────────────────────
    let accum_cb = Arc::clone(&accumulator);
    let eos_cb = Arc::clone(&eos);

    type CbBlock = block2::Block<dyn Fn(NonNull<AVAudioBuffer>)>;
    let cb = block2::RcBlock::new(move |buf_ptr: NonNull<AVAudioBuffer>| {
        pcm_callback(buf_ptr, &accum_cb, &eos_cb);
    });

    // ── 3. Start synthesis (returns immediately; callbacks queued async) ─
    // SAFETY: called on the main thread; `cb` is retained by `RcBlock`.
    let block_ref: &CbBlock = &cb;
    let block_ptr: *mut CbBlock = (block_ref as *const CbBlock).cast_mut();
    unsafe { synth.writeUtterance_toBufferCallback(&utterance, block_ptr) };

    // ── 4. Drain the main NSRunLoop until EOS ────────────────────────────
    // `runUntilDate:` on the main run loop drains both NSRunLoop sources and
    // the GCD main queue (they share the same CFRunLoop). Each 10 ms slice
    // delivers any queued PCM callbacks.
    let run_loop = NSRunLoop::mainRunLoop();
    let deadline = Instant::now() + DRAIN_TIMEOUT;
    loop {
        if eos.load(Ordering::SeqCst) {
            break;
        }
        if Instant::now() >= deadline {
            return Err(PrimerError::Speech(
                "AVSpeechSynthesizer 30s NSRunLoop drain timeout (main-thread path)".into(),
            ));
        }
        let date = NSDate::dateWithTimeIntervalSinceNow(RUN_LOOP_SLICE.as_secs_f64());
        run_loop.runUntilDate(&date);
    }

    // `cb` (RcBlock) goes out of scope here, but synthesis callbacks
    // may still fire afterward. This is safe ONLY because
    // AVSpeechSynthesizer issues its own Block_retain when assigning
    // the callback — Rust's Drop of RcBlock decrements the refcount
    // by 1, leaving the ObjC retain in place. Do not "fix" this by
    // keeping `cb` alive longer; that would double-retain and leak.
    drop(cb);
    let chunks = accumulator.lock().unwrap().clone();
    Ok(chunks)
}

// ═══════════════════════════════════════════════════════════════════════════
// Background-thread path
// ═══════════════════════════════════════════════════════════════════════════

/// Context bundle passed through `dispatch_async_f`.
struct SynthCtx {
    utterance: Retained<AVSpeechUtterance>,
    /// Shared semaphore — both the trampoline and the waiting background
    /// thread hold an `Arc` clone. The last drop calls `dispatch_release`.
    sema: Arc<DispatchSemaphore>,
    accum: Accumulator,
    eos: Arc<AtomicBool>,
}

// SAFETY: `SynthCtx` is moved into the main-queue trampoline; at that point
// no other thread holds a reference to it. The move is one-shot.
unsafe impl Send for SynthCtx {}

/// Synthesis from a background thread (pool thread, GUI async task).
/// Returns one `AudioChunk` per PCM callback that AVSpeechSynthesizer
/// delivered.
///
/// Submits `writeUtterance:toBufferCallback:` to the GCD main queue via
/// `dispatch_async_f`. The main queue runs on the main thread, whose
/// CFRunLoop is already spinning (CLI tool, GUI app). A `dispatch_semaphore`
/// blocks the calling pool thread until the EOS callback fires.
fn synthesize_to_chunks_background(
    text: &str,
    voice: &AVSpeechSynthesisVoice,
) -> Result<Vec<AudioChunk>> {
    // ── 1. Create the EOS semaphore ──────────────────────────────────────
    // SAFETY: `dispatch_semaphore_create(0)` returns a valid semaphore.
    let raw_sema = unsafe { dispatch::dispatch_semaphore_create(0) };
    if raw_sema.is_null() {
        return Err(PrimerError::Speech(
            "dispatch_semaphore_create returned null".into(),
        ));
    }
    // Wrap in Arc so both this thread and the trampoline share ownership.
    // The last Arc to drop calls `dispatch_release` via the RAII wrapper,
    // eliminating the use-after-free that occurred when this thread released
    // on timeout while the trampoline was still queued on the main thread.
    let sema = Arc::new(DispatchSemaphore(raw_sema));

    // ── 2. Build utterance on the calling thread ─────────────────────────
    let ns_text = NSString::from_str(text);
    // SAFETY: factory method.
    let utterance = unsafe { AVSpeechUtterance::speechUtteranceWithString(&ns_text) };
    // SAFETY: setVoice:.
    unsafe { utterance.setVoice(Some(voice)) };

    let accumulator: Accumulator = Arc::new(Mutex::new(Vec::new()));
    let eos = Arc::new(AtomicBool::new(false));

    // ── 3. Build and submit the synthesis trampoline ─────────────────────
    extern "C" fn trampoline(ctx_raw: *mut std::ffi::c_void) {
        // SAFETY: ctx_raw was Box::into_raw'd below; we take ownership here.
        // When `ctx` drops at end of scope, the `Arc<DispatchSemaphore>`
        // inside is decremented. If this is the last Arc (i.e. the background
        // thread already timed out and dropped its Arc), `dispatch_release`
        // runs here — never on an already-freed semaphore.
        let ctx = unsafe { Box::from_raw(ctx_raw as *mut SynthCtx) };

        // SAFETY: AVSpeechSynthesizer::new on the main thread.
        let synth: Retained<AVSpeechSynthesizer> = unsafe { AVSpeechSynthesizer::new() };

        let accum_cb = Arc::clone(&ctx.accum);
        let eos_cb = Arc::clone(&ctx.eos);
        // Clone the Arc so the closure captures its own strong reference.
        // The closure may outlive `ctx`'s drop (ObjC retains the block),
        // but the Arc ensures the semaphore stays alive until the closure drops.
        let sema_cb = Arc::clone(&ctx.sema);

        type CbBlock = block2::Block<dyn Fn(NonNull<AVAudioBuffer>)>;
        let cb = block2::RcBlock::new(move |buf_ptr: NonNull<AVAudioBuffer>| {
            // Check EOS first so we signal the semaphore exactly once.
            let is_eos = {
                let buf: &AVAudioBuffer = unsafe { buf_ptr.as_ref() };
                let pcm: &AVAudioPCMBuffer = match buf.downcast_ref::<AVAudioPCMBuffer>() {
                    Some(p) => p,
                    None => return,
                };
                let frame_length = unsafe { pcm.frameLength() } as usize;
                frame_length == EOS_FRAME_LENGTH
            };

            pcm_callback(buf_ptr, &accum_cb, &eos_cb);

            if is_eos {
                // SAFETY: `sema_cb` (Arc) keeps the semaphore alive for at
                // least as long as this closure lives — safe to signal.
                unsafe { dispatch::dispatch_semaphore_signal(sema_cb.as_raw()) };
            }
        });

        // SAFETY: called on the main thread.
        let block_ref: &CbBlock = &cb;
        let block_ptr: *mut CbBlock = (block_ref as *const CbBlock).cast_mut();
        unsafe { synth.writeUtterance_toBufferCallback(&ctx.utterance, block_ptr) };

        // `cb` (RcBlock) goes out of scope here, but synthesis callbacks
        // may still fire afterward. This is safe ONLY because
        // AVSpeechSynthesizer issues its own Block_retain when assigning
        // the callback — Rust's Drop of RcBlock decrements the refcount
        // by 1, leaving the ObjC retain in place. Do not "fix" this by
        // keeping `cb` alive longer; that would double-retain and leak.
    }

    let ctx = Box::new(SynthCtx {
        utterance,
        sema: Arc::clone(&sema),
        accum: Arc::clone(&accumulator),
        eos: Arc::clone(&eos),
    });
    let ctx_ptr = Box::into_raw(ctx) as *mut std::ffi::c_void;

    // SAFETY: `_dispatch_main_q` is the GCD main queue object.
    let main_queue: dispatch::dispatch_queue_t =
        unsafe { &dispatch::_dispatch_main_q as *const _ as dispatch::dispatch_queue_t };

    // SAFETY: `trampoline` takes ownership of `ctx_ptr` exactly once.
    unsafe { dispatch::dispatch_async_f(main_queue, ctx_ptr, trampoline) };

    // ── 4. Wait for EOS semaphore ────────────────────────────────────────
    // SAFETY: `sema` Arc keeps the semaphore valid throughout the wait.
    // `dispatch_time` computes an absolute deadline.
    let deadline =
        unsafe { dispatch::dispatch_time(dispatch::DISPATCH_TIME_NOW, dispatch::TIMEOUT_NS) };
    let wait_result = unsafe { dispatch::dispatch_semaphore_wait(sema.as_raw(), deadline) };
    // Drop our Arc. If the trampoline has already run and dropped its Arc,
    // this is the last reference and `dispatch_release` runs here.
    // If the trampoline is still queued, its Arc keeps the semaphore alive
    // until the trampoline runs — no UAF.
    drop(sema);

    if wait_result != 0 {
        return Err(PrimerError::Speech(
            "AVSpeechSynthesizer 30s dispatch semaphore timeout (background-thread path)".into(),
        ));
    }

    let chunks = accumulator.lock().unwrap().clone();
    Ok(chunks)
}

// ═══════════════════════════════════════════════════════════════════════════
// Shared PCM callback
// ═══════════════════════════════════════════════════════════════════════════

/// Collect one PCM buffer from `AVSpeechSynthesizer` into `accum` as a
/// new `AudioChunk`. Sets `eos` when a zero-frame buffer arrives (the EOS
/// sentinel).
fn pcm_callback(buf_ptr: NonNull<AVAudioBuffer>, accum: &Accumulator, eos: &Arc<AtomicBool>) {
    // SAFETY: buf_ptr is non-null and valid for the callback's lifetime.
    let buf: &AVAudioBuffer = unsafe { buf_ptr.as_ref() };

    let pcm: &AVAudioPCMBuffer = match buf.downcast_ref::<AVAudioPCMBuffer>() {
        Some(p) => p,
        None => return,
    };

    // SAFETY: `frameLength` is an immutable property getter.
    let frame_length = unsafe { pcm.frameLength() } as usize;

    if frame_length == EOS_FRAME_LENGTH {
        eos.store(true, Ordering::SeqCst);
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

    let mut guard = accum.lock().unwrap();
    guard.push(AudioChunk {
        samples: slice.to_vec(),
        sample_rate,
    });
}
