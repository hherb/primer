//! AVSpeechSynthesizer one-shot TTS backend.
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
    AVAudioBuffer, AVAudioPCMBuffer, AVSpeechSynthesisVoice, AVSpeechSynthesizer, AVSpeechUtterance,
};
use objc2_foundation::{NSDate, NSRunLoop, NSString};

use primer_core::error::{PrimerError, Result};
use primer_core::i18n::Locale;
use primer_core::speech::{AudioBuffer, Named, TextToSpeech, VoiceProfile};

use super::voice::select_voice;

/// Backend identifier returned by `Named::name()` and used in logs.
const BACKEND_NAME: &str = "macos-native-tts";

/// EOS sentinel: AVSpeechSynthesizer signals end-of-utterance with a
/// zero-frame buffer.
const EOS_FRAME_LENGTH: usize = 0;

/// Synthesis timeout. If no EOS sentinel arrives within this window,
/// `synthesize` returns an error.
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

/// macOS-native TTS backend using `AVSpeechSynthesizer`.
///
/// Each instance is bound to a single locale and holds the best available
/// system voice for that locale. Cheap to construct; the AVSpeechSynthesizer
/// itself is allocated fresh per call to avoid `Send`/`Sync` complications.
pub struct MacosTextToSpeech {
    locale: Locale,
    voice: Retained<AVSpeechSynthesisVoice>,
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

        Ok(Self {
            locale,
            voice: selection.voice,
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
type Accumulator = Arc<Mutex<(Vec<f32>, u32)>>;

#[async_trait]
impl TextToSpeech for MacosTextToSpeech {
    async fn synthesize(&self, text: &str, _voice: &VoiceProfile) -> Result<AudioBuffer> {
        // `+[NSThread isMainThread]` is a thread-safe class method — safe from
        // any thread.
        let on_main = objc2_foundation::NSThread::isMainThread_class();

        if on_main {
            // Synchronous call — no non-Send ObjC types cross any `.await`
            // boundary in the outer async fn.  The main-thread path drives the
            // NSRunLoop in a tight loop; on a `current_thread` runtime (the test
            // harness) this is the only task, so brief blocking is harmless.
            synthesize_main_thread_path(text, &self.voice)
        } else {
            let text_owned = text.to_owned();
            let voice_retained = self.voice.clone();
            tokio::task::spawn_blocking(move || {
                synthesize_background_path(&text_owned, voice_retained)
            })
            .await
            .map_err(|e| PrimerError::Speech(format!("spawn_blocking panicked: {e}")))?
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Main-thread path
// ═══════════════════════════════════════════════════════════════════════════

/// Synthesis from the OS main thread (synchronous).
///
/// Calls `writeUtterance:toBufferCallback:` on the current (main) thread,
/// then drives the main NSRunLoop in 10 ms slices — each slice delivers any
/// pending GCD main-queue callbacks (including AVSpeechSynthesizer PCM
/// buffers) and returns.
///
/// This is intentionally synchronous so that non-Send ObjC types never cross
/// an `.await` boundary in the calling `async fn synthesize`.
fn synthesize_main_thread_path(text: &str, voice: &AVSpeechSynthesisVoice) -> Result<AudioBuffer> {
    let accumulator: Accumulator = Arc::new(Mutex::new((Vec::new(), 0u32)));
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

    drop(cb);
    let (samples, sample_rate) = accumulator.lock().unwrap().clone();
    Ok(AudioBuffer {
        samples,
        sample_rate,
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Background-thread path
// ═══════════════════════════════════════════════════════════════════════════

/// Context bundle passed through `dispatch_async_f`.
struct SynthCtx {
    utterance: Retained<AVSpeechUtterance>,
    sema: dispatch::dispatch_semaphore_t,
    accum: Accumulator,
    eos: Arc<AtomicBool>,
}

// SAFETY: `SynthCtx` is moved into the main-queue trampoline; at that point
// no other thread holds a reference to it. The move is one-shot.
unsafe impl Send for SynthCtx {}

/// Synthesis from a background thread (pool thread, GUI async task).
///
/// Submits `writeUtterance:toBufferCallback:` to the GCD main queue via
/// `dispatch_async_f`. The main queue runs on the main thread, whose
/// CFRunLoop is already spinning (CLI tool, GUI app). A `dispatch_semaphore`
/// blocks the calling pool thread until the EOS callback fires.
fn synthesize_background_path(
    text: &str,
    voice: Retained<AVSpeechSynthesisVoice>,
) -> Result<AudioBuffer> {
    // ── 1. Create the EOS semaphore ──────────────────────────────────────
    // SAFETY: `dispatch_semaphore_create(0)` returns a valid semaphore.
    let sema = unsafe { dispatch::dispatch_semaphore_create(0) };
    if sema.is_null() {
        return Err(PrimerError::Speech(
            "dispatch_semaphore_create returned null".into(),
        ));
    }

    // ── 2. Build utterance on the calling thread ─────────────────────────
    let ns_text = NSString::from_str(text);
    // SAFETY: factory method.
    let utterance = unsafe { AVSpeechUtterance::speechUtteranceWithString(&ns_text) };
    // SAFETY: setVoice:.
    unsafe { utterance.setVoice(Some(&voice)) };

    let accumulator: Accumulator = Arc::new(Mutex::new((Vec::new(), 0u32)));
    let eos = Arc::new(AtomicBool::new(false));

    // ── 3. Build and submit the synthesis trampoline ─────────────────────
    extern "C" fn trampoline(ctx_raw: *mut std::ffi::c_void) {
        // SAFETY: ctx_raw was Box::into_raw'd below; we take ownership here.
        let ctx = unsafe { Box::from_raw(ctx_raw as *mut SynthCtx) };

        // SAFETY: AVSpeechSynthesizer::new on the main thread.
        let synth: Retained<AVSpeechSynthesizer> = unsafe { AVSpeechSynthesizer::new() };

        let accum_cb = Arc::clone(&ctx.accum);
        let eos_cb = Arc::clone(&ctx.eos);
        let sema_ptr = ctx.sema;

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
                // SAFETY: sema_ptr is alive until the waiting thread wakes.
                unsafe { dispatch::dispatch_semaphore_signal(sema_ptr) };
            }
        });

        // SAFETY: called on the main thread.
        let block_ref: &CbBlock = &cb;
        let block_ptr: *mut CbBlock = (block_ref as *const CbBlock).cast_mut();
        unsafe { synth.writeUtterance_toBufferCallback(&ctx.utterance, block_ptr) };
        // `cb` is dropped here; synthesis callbacks may still fire afterward,
        // but the EOS one has already signalled the semaphore.
    }

    let ctx = Box::new(SynthCtx {
        utterance,
        sema,
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
    // SAFETY: `sema` is valid; `dispatch_time` computes an absolute deadline.
    let deadline =
        unsafe { dispatch::dispatch_time(dispatch::DISPATCH_TIME_NOW, dispatch::TIMEOUT_NS) };
    let wait_result = unsafe { dispatch::dispatch_semaphore_wait(sema, deadline) };
    // SAFETY: release our reference to the semaphore.
    unsafe { dispatch::dispatch_release(sema) };

    if wait_result != 0 {
        return Err(PrimerError::Speech(
            "AVSpeechSynthesizer 30s dispatch semaphore timeout (background-thread path)".into(),
        ));
    }

    let (samples, sample_rate) = accumulator.lock().unwrap().clone();
    Ok(AudioBuffer {
        samples,
        sample_rate,
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Shared PCM callback
// ═══════════════════════════════════════════════════════════════════════════

/// Collect one PCM buffer from `AVSpeechSynthesizer` into `accum`.
/// Sets `eos` when a zero-frame buffer arrives (the EOS sentinel).
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
    guard.0.extend_from_slice(slice);
    if guard.1 == 0 {
        guard.1 = sample_rate;
    }
}
