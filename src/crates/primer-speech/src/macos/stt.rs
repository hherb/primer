//! SFSpeechRecognizer-backed implementation of `StreamingSpeechToText`.
//!
//! # On-device guarantee
//!
//! `requiresOnDeviceRecognition = true` is set on every request, and the
//! backend refuses to construct if `supportsOnDeviceRecognition` is false —
//! the project's strict-offline-first principle means we must never silently
//! fall back to a network request.
//!
//! # Threading
//!
//! Apple's docs state that the result-handler fires on the recognizer's
//! `queue` property (default: the app's main queue). We replace that with
//! a freshly allocated background `NSOperationQueue` immediately after
//! constructing the recognizer, so the result-handler never needs the main
//! thread to be running its run loop. The handler pushes text into a
//! `std::sync::mpsc::SyncSender`; `TranscriptionSession::push_audio` drains
//! the channel via `try_recv` after appending each PCM buffer.
//!
//! # Session lifetime
//!
//! One `MacosSttSession` per child utterance:
//!
//! 1. `open_session` creates a fresh `SFSpeechAudioBufferRecognitionRequest`
//!    and starts a `recognitionTaskWithRequest_resultHandler` task.
//! 2. `push_audio` builds an `AVAudioPCMBuffer`, calls `appendAudioPCMBuffer`,
//!    and drains the partial-transcript channel.
//! 3. `finalize` calls `endAudio`, then uses `recv_timeout` (not a spin loop)
//!    to wait for the final segment, exiting early when `is_final_seen` is set.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_avf_audio::{AVAudioFormat, AVAudioPCMBuffer};
use objc2_foundation::{NSLocale, NSOperationQueue, NSString};
use objc2_speech::{
    SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionResult, SFSpeechRecognizer,
};
use primer_core::error::{PrimerError, Result};
use primer_core::speech::{Named, StreamingSpeechToText, TranscriptSegment, TranscriptionSession};

/// Backend identifier returned by `Named::name()` and used in logs.
pub const BACKEND_NAME: &str = "macos-native-stt";

/// Sample rate expected by `SFSpeechRecognizer` for PCM input.
/// Apple's own `nativeAudioFormat` typically reports 16 kHz for
/// on-device recognition; using 16 kHz avoids unnecessary resampling.
const STT_SAMPLE_RATE: u32 = 16_000;

/// How long `finalize` waits for the final recognition result.
/// On-device STT can take >300 ms for longer utterances; 2 s gives
/// enough headroom without blocking the voice loop for an unreasonable
/// time when the recognizer is unresponsive.
const FINALIZE_POLL_TIMEOUT: Duration = Duration::from_millis(2_000);

// ═══════════════════════════════════════════════════════════════════════════
// MacosSpeechToText — the backend object
// ═══════════════════════════════════════════════════════════════════════════

/// macOS-native STT backend using `SFSpeechRecognizer`.
///
/// On-device-only by construction: `requiresOnDeviceRecognition = true`
/// is set on every request, and `new()` fails if the recognizer cannot
/// serve the locale on-device.
///
/// Each call to `open_session` opens a fresh recognition task for one
/// child utterance.
pub struct MacosSpeechToText {
    /// The underlying recognizer. Wrapped in `Mutex` so that
    /// `MacosSpeechToText: Send + Sync` — each call to `open_session`
    /// briefly locks to call `recognitionTaskWithRequest_resultHandler`.
    recognizer: Mutex<Retained<SFSpeechRecognizer>>,
}

impl MacosSpeechToText {
    /// Create a new STT backend for `bcp47` (e.g. `"en-US"`, `"de-DE"`).
    ///
    /// Returns `Err(PrimerError::Speech)` if:
    /// - `bcp47` is not a supported locale in this codebase (`en-US` /
    ///   `de-DE` are the only mapped locales today),
    /// - `SFSpeechRecognizer::initWithLocale` returns `nil` (locale unknown
    ///   to the OS),
    /// - or `supportsOnDeviceRecognition` is `false` on this device (the
    ///   on-device model for the locale is not installed).
    ///
    /// # Note on consent prompt
    ///
    /// This constructor does NOT trigger the OS speech-recognition consent
    /// prompt — that prompt requires `NSSpeechRecognitionUsageDescription`
    /// in `Info.plist` and causes a `SIGABRT` in test binaries. The prompt
    /// fires when the first recognition task runs. See `permissions.rs` for
    /// the explicit authorization flow.
    pub fn new(bcp47: &str) -> Result<Self> {
        // Validate the locale against the supported set in this codebase.
        // `SFSpeechRecognizer` accepts many more locales than we map here;
        // unsupported ones return an `Other` error rather than silently
        // constructing a backend for an unhandled locale.
        match bcp47 {
            "en-US" | "de-DE" => {} // accepted locales
            other => {
                return Err(PrimerError::Speech(format!(
                    "unsupported BCP-47 locale for macOS STT: {other}"
                )));
            }
        }

        let ns_id = NSString::from_str(bcp47);
        let ns_locale = NSLocale::localeWithLocaleIdentifier(&ns_id);

        // SAFETY: `alloc()` returns a fresh uninitialised instance;
        // `initWithLocale:` takes exclusive ownership via the `alloc`
        // convention and either fully initialises the recognizer or
        // returns `nil`. No aliasing or shared mutation.
        let recognizer: Retained<SFSpeechRecognizer> =
            unsafe { SFSpeechRecognizer::initWithLocale(SFSpeechRecognizer::alloc(), &ns_locale) }
                .ok_or_else(|| {
                    PrimerError::Speech(format!(
                        "SFSpeechRecognizer could not be created for locale {bcp47}"
                    ))
                })?;

        // Enforce on-device-only: fail loudly rather than silently falling
        // back to network, which would violate [[project_strict_offline_first]].
        // SAFETY: `supportsOnDeviceRecognition` is an immutable property read.
        if !unsafe { recognizer.supportsOnDeviceRecognition() } {
            return Err(PrimerError::Speech(format!(
                "on-device speech recognition is not available for locale {bcp47} on this device; \
                 install the language pack via System Settings → Keyboard → Dictation → Languages"
            )));
        }

        // Replace the default main-queue callback queue with a background
        // queue so result-handler callbacks don't require the main thread's
        // run loop to be spinning. `NSOperationQueue::new()` creates a
        // concurrent background queue.
        // `NSOperationQueue::new()` is a safe factory; `setQueue:` has an
        // additional threading-safety caveat documented in the objc2 bindings
        // (must be set before starting a recognition task). We call it
        // immediately after construction, before any task starts.
        let bg_queue = NSOperationQueue::new();
        // SAFETY: `setQueue:` changes the queue on which recognition callbacks
        // fire. Setting it before the first task starts is safe — no data races.
        unsafe { recognizer.setQueue(&bg_queue) };

        Ok(Self {
            recognizer: Mutex::new(recognizer),
        })
    }
}

// SAFETY: `MacosSpeechToText` is `Send + Sync`:
// - `Mutex<Retained<SFSpeechRecognizer>>` provides `Sync` (and `Send`)
//   on top of an otherwise non-Send `Retained` pointer.
// - All ObjC method calls on the recognizer happen inside `Mutex::lock`
//   guards, serialising access.
// `SFSpeechRecognizer` is documented as safe to use from any thread via
// `recognitionTaskWithRequest_resultHandler`; Apple ships it in apps with
// concurrent dispatch contexts.
unsafe impl Send for MacosSpeechToText {}
unsafe impl Sync for MacosSpeechToText {}

impl Named for MacosSpeechToText {
    fn name(&self) -> &str {
        BACKEND_NAME
    }
}

impl StreamingSpeechToText for MacosSpeechToText {
    fn sample_rate(&self) -> u32 {
        STT_SAMPLE_RATE
    }

    fn open_session(&self) -> Result<Box<dyn TranscriptionSession>> {
        // ── 1. Create the recognition request ───────────────────────────
        // SAFETY: `SFSpeechAudioBufferRecognitionRequest::new()` is a safe
        // factory (the superclass `NSObject::new` is always safe).
        let request: Retained<SFSpeechAudioBufferRecognitionRequest> =
            unsafe { SFSpeechAudioBufferRecognitionRequest::new() };

        // Enforce on-device-only per request as well (belt-and-suspenders;
        // the recognizer-level check in `new()` is the primary gate).
        // SAFETY: `setRequiresOnDeviceRecognition:` is a simple property
        // setter on the request object.
        unsafe { request.setRequiresOnDeviceRecognition(true) };

        // Enable partial results so we get incremental updates while the
        // child is still speaking — the voice loop uses these for display.
        // SAFETY: simple property setter.
        unsafe { request.setShouldReportPartialResults(true) };

        // ── 2. Build the cached AVAudioFormat (16 kHz mono) ─────────────
        // Built once here at open_session and stored on the session, so
        // push_audio doesn't re-allocate an identical ObjC object on every
        // call (saves alloc churn on the audio thread for every push).
        //
        // SAFETY: factory initialiser taking no mutable state.
        let format: Retained<AVAudioFormat> = unsafe {
            AVAudioFormat::initStandardFormatWithSampleRate_channels(
                AVAudioFormat::alloc(),
                STT_SAMPLE_RATE as f64,
                1, // mono
            )
        }
        .ok_or_else(|| {
            PrimerError::Speech("AVAudioFormat init failed for 16 kHz mono STT format".into())
        })?;

        // ── 3. Create the mpsc channel ───────────────────────────────────
        // The channel is bounded at 64 segments — each partial result from
        // the recognizer is one send; 64 is more than any single utterance
        // will produce before `push_audio` drains them.
        let (seg_tx, seg_rx) = mpsc::sync_channel::<TranscriptSegment>(64);

        // ── 4. Arc<AtomicBool> to signal is_final without touching the channel ──
        // The result handler sets this to `true` (Release) when `isFinal()` is
        // true. `finalize` checks it (Acquire) after each recv to exit early —
        // zero busy-wait, exits within microseconds of the final result arriving.
        let is_final_seen = Arc::new(AtomicBool::new(false));
        let is_final_seen_handler = Arc::clone(&is_final_seen);

        // ── 5. Build the result-handler closure ──────────────────────────
        // The closure captures `seg_tx` (a `SyncSender` — `Send + Clone`)
        // and `is_final_seen_handler` (an `Arc<AtomicBool>` — `Send + Sync`).
        // It fires on the background `NSOperationQueue` we set during
        // construction, so no main-thread run-loop dependency here.
        //
        // `*mut SFSpeechRecognitionResult` may be null (error path); we
        // check for null before dereferencing.
        let handler_tx = seg_tx;
        let handler: block2::RcBlock<
            dyn Fn(*mut SFSpeechRecognitionResult, *mut objc2_foundation::NSError),
        > = block2::RcBlock::new(
            move |result_ptr: *mut SFSpeechRecognitionResult,
                  _error_ptr: *mut objc2_foundation::NSError| {
                // Null result means an error — nothing to forward.
                if result_ptr.is_null() {
                    return;
                }
                // SAFETY: result_ptr is non-null and valid for the closure's
                // lifetime (the OS retains the result object while calling us).
                let result: &SFSpeechRecognitionResult = unsafe { &*result_ptr };

                // `bestTranscription.formattedString` gives the highest-
                // confidence partial or final transcript string.
                // SAFETY: immutable property getters on a live ObjC object.
                let transcription = unsafe { result.bestTranscription() };
                let formatted = unsafe { transcription.formattedString() };
                let text = formatted.to_string();

                // `is_final` — if true this is the last segment.
                // SAFETY: immutable property getter.
                let is_final = unsafe { result.isFinal() };

                if text.is_empty() {
                    // Even on empty text: if this is final, set the flag so
                    // finalize() doesn't wait out the full timeout.
                    if is_final {
                        is_final_seen_handler.store(true, Ordering::Release);
                    }
                    return;
                }

                let segment = TranscriptSegment {
                    text,
                    // We don't have accurate per-segment timing from
                    // SFSpeechRecognizer without diving into
                    // SFTranscriptionSegment; use 0 as a placeholder.
                    // The voice loop uses only the text content.
                    start_ms: 0,
                    end_ms: 0,
                };

                // Send — distinguish between expected shutdown (Disconnected)
                // and a full buffer (worth a warning, suggests push_audio isn't
                // draining frequently enough or the channel bound needs raising).
                match handler_tx.try_send(segment) {
                    Ok(()) => {}
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        // Receiver dropped — session closed before recognition
                        // finished. Silent: this is the expected shutdown path
                        // when the voice loop cancels mid-utterance.
                    }
                    Err(mpsc::TrySendError::Full(_)) => {
                        tracing::warn!(
                            target: "primer::speech::macos",
                            "STT segment channel full; dropping a partial transcript segment. \
                             Increase the channel bound or call push_audio more frequently."
                        );
                    }
                }

                // Signal finalize() to stop waiting as soon as the final
                // result has been pushed — set AFTER the send so the receiver
                // sees the segment before exiting.
                if is_final {
                    is_final_seen_handler.store(true, Ordering::Release);
                    tracing::debug!(
                        target: "primer::speech::macos_stt",
                        "SFSpeechRecognizer: final result received"
                    );
                }
            },
        );

        // ── 6. Start the recognition task ────────────────────────────────
        // Lock the recognizer to call `recognitionTaskWithRequest_resultHandler`.
        // The task runs asynchronously; this call returns immediately.
        //
        // `RcBlock<F>` derefs to `Block<F>` (= `DynBlock<F>`) so `&*handler`
        // gives the `&DynBlock<...>` reference the API expects. ObjC convention
        // for block arguments is `Block_copy` on entry, so the recognizer
        // holds its own strong reference for the task's lifetime; Rust's Drop
        // of RcBlock (decrement by 1) is safe — mirrors the TTS pattern.
        let task = {
            let recognizer = self.recognizer.lock().map_err(|_| {
                PrimerError::Speech("MacosSpeechToText recognizer mutex poisoned".into())
            })?;
            // SAFETY: `recognitionTaskWithRequest_resultHandler:` takes
            // `&SFSpeechRecognitionRequest` (the base class) and
            // `&DynBlock<dyn Fn(...)>`. The request was freshly constructed
            // above; `&*handler` coerces the RcBlock deref to a DynBlock ref.
            // The recognizer internally retains both for the task's lifetime.
            unsafe { recognizer.recognitionTaskWithRequest_resultHandler(&request, &handler) }
        };

        // `handler` (RcBlock) goes out of scope here. This is intentional
        // and safe: `recognitionTaskWithRequest_resultHandler` calls
        // `Block_copy` internally (ObjC convention for block arguments),
        // which increments the refcount. Rust's Drop decrements by 1,
        // leaving the ObjC copy alive for the task's lifetime.
        drop(handler);

        Ok(Box::new(MacosSttSession {
            request,
            _task: task,
            seg_rx,
            is_final_seen,
            format,
        }))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// MacosSttSession — per-utterance session
// ═══════════════════════════════════════════════════════════════════════════

/// One streaming transcription session.
///
/// `Send` but not `Sync` — each utterance owns its session.
struct MacosSttSession {
    /// The recognition request; audio is appended here; `endAudio` is
    /// called on `finalize`.
    request: Retained<SFSpeechAudioBufferRecognitionRequest>,
    /// The recognition task handle. Kept alive (but unused directly) so
    /// the task is not cancelled when we drop it. Prefixed with `_` to
    /// suppress the unused-field warning — we need the owned `Retained`
    /// to keep the task alive; `cancel()` is called implicitly on `Drop`.
    _task: Retained<objc2_speech::SFSpeechRecognitionTask>,
    /// Partial/final transcript segments delivered by the result handler.
    seg_rx: mpsc::Receiver<TranscriptSegment>,
    /// Set to `true` (Release) by the result handler when `isFinal()` fires.
    /// `finalize` reads this (Acquire) after each `recv_timeout` to exit
    /// early — no busy-wait, no arbitrary 300 ms ceiling for slow devices.
    is_final_seen: Arc<AtomicBool>,
    /// Cached 16 kHz mono `AVAudioFormat`, built once at `open_session` and
    /// reused across all `push_audio` calls to avoid per-call ObjC allocation
    /// churn on the audio thread.
    format: Retained<AVAudioFormat>,
}

// SAFETY: `MacosSttSession` owns its fields exclusively after construction:
// - `Retained<SFSpeechAudioBufferRecognitionRequest>` and
//   `Retained<SFSpeechRecognitionTask>` are ObjC ref-counted objects;
//   the `Mutex` in the parent `MacosSpeechToText` is no longer involved —
//   we hold our own `Retained` copies.
// - `mpsc::Receiver<TranscriptSegment>` is `Send`.
// - `Arc<AtomicBool>` is `Send + Sync`.
// - `Retained<AVAudioFormat>` is an immutable-after-construction ObjC object.
// `Send` is required by the `TranscriptionSession` supertrait.
unsafe impl Send for MacosSttSession {}

impl TranscriptionSession for MacosSttSession {
    fn push_audio(&mut self, samples: &[f32]) -> Result<Vec<TranscriptSegment>> {
        if samples.is_empty() {
            return Ok(vec![]);
        }

        // ── 1. Build an AVAudioPCMBuffer containing `samples` ────────────
        let frame_count = samples.len() as u32;

        // Use the cached AVAudioFormat built once in open_session — no per-call
        // ObjC allocation needed.
        //
        // SAFETY: `initWithPCMFormat:frameCapacity:` requires a valid format
        // and a non-zero capacity. Both conditions are met.
        let pcm: Retained<AVAudioPCMBuffer> = unsafe {
            AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
                AVAudioPCMBuffer::alloc(),
                &self.format,
                frame_count,
            )
        }
        .ok_or_else(|| {
            PrimerError::Speech(format!(
                "AVAudioPCMBuffer allocation failed for {frame_count} frames"
            ))
        })?;

        // Set the buffer's `frameLength` to `frame_count` so the recognizer
        // knows how many samples are valid.
        // SAFETY: `setFrameLength:` is a simple property setter; value ≤
        // `frameCapacity` (we allocated exactly `frame_count` frames above).
        unsafe { pcm.setFrameLength(frame_count) };

        // Copy samples into the buffer's float channel data.
        // SAFETY:
        // - `floatChannelData()` returns a non-null pointer for f32-format
        //   PCM buffers; our format is standard (= deinterleaved f32), so
        //   this is guaranteed non-null.
        // - `*data_ptr` is the channel-0 pointer to `frame_count` f32 samples.
        // - The `copy_from_slice` write stays within the allocated region
        //   (frame_count samples = frame_capacity × channels × sizeof(f32)).
        let data_ptr = unsafe { pcm.floatChannelData() };
        if data_ptr.is_null() {
            return Err(PrimerError::Speech(
                "AVAudioPCMBuffer floatChannelData returned null for standard mono format".into(),
            ));
        }
        let chan0_nn = unsafe { *data_ptr };
        let chan0_ptr = chan0_nn.as_ptr();
        // SAFETY: valid writable f32 slice of `frame_count` elements within
        // the PCM buffer's allocated storage.
        let dst: &mut [f32] =
            unsafe { std::slice::from_raw_parts_mut(chan0_ptr, frame_count as usize) };
        dst.copy_from_slice(samples);

        // ── 2. Append the PCM buffer to the recognition request ───────────
        // SAFETY: `appendAudioPCMBuffer:` is documented to accept buffers
        // matching the request's native format. We use 16 kHz mono which
        // is always accepted; if the format diverges from the native format
        // the OS will resample internally.
        unsafe { self.request.appendAudioPCMBuffer(&pcm) };

        // ── 3. Drain the segment channel ─────────────────────────────────
        let mut segments = Vec::new();
        while let Ok(seg) = self.seg_rx.try_recv() {
            segments.push(seg);
        }
        Ok(segments)
    }

    fn finalize(self: Box<Self>) -> Result<Vec<TranscriptSegment>> {
        // Signal end-of-audio to the recognizer — it will deliver the final
        // result to the handler block, then mark the task as finished.
        // SAFETY: `endAudio` is safe to call at any point after the request
        // was started; it is idempotent.
        {
            // SAFETY: endAudio is a thread-safe Apple API on the request object.
            unsafe { self.request.endAudio() };
        }

        let mut out = Vec::new();
        let deadline = Instant::now() + FINALIZE_POLL_TIMEOUT;

        loop {
            // Drain any segments that have already arrived.
            while let Ok(seg) = self.seg_rx.try_recv() {
                out.push(seg);
            }

            // Exit early as soon as the handler has flagged is_final — the
            // final segment is already in `out` (we set the flag after the
            // send, so the segment landed before the flag was visible here).
            if self.is_final_seen.load(Ordering::Acquire) {
                // One more drain to catch anything that arrived between the
                // last try_recv pass and reading the flag.
                while let Ok(seg) = self.seg_rx.try_recv() {
                    out.push(seg);
                }
                break;
            }

            // Compute remaining budget; exit if we've hit the deadline.
            let remaining = match deadline.checked_duration_since(Instant::now()) {
                Some(d) if !d.is_zero() => d,
                _ => break, // timeout hit
            };

            // Block until the next segment arrives or the budget expires.
            // This is zero CPU while waiting — no busy-wait spin.
            match self.seg_rx.recv_timeout(remaining) {
                Ok(seg) => out.push(seg),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        Ok(out)
    }
}
