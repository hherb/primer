//! The streaming synthesis paths behind [`MacosTextToSpeech`](super::MacosTextToSpeech):
//! the main-thread and background (GCD-bounce) drain loops, and the PCM
//! callback both share. See the parent module docs for the delivery contract.

use std::ptr::NonNull;
use std::time::{Duration, Instant};

use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2_avf_audio::{
    AVAudioBuffer, AVAudioPCMBuffer, AVSpeechSynthesisVoice, AVSpeechSynthesizer, AVSpeechUtterance,
};
use objc2_foundation::{NSDate, NSRunLoop, NSString};

use primer_core::consts::speech::{
    STREAM_DRAIN_POLL_MS, STREAM_DRAIN_TIMEOUT_SECS, STREAM_RUN_LOOP_SLICE_MS,
};
use primer_core::error::{PrimerError, Result};
use primer_core::speech::{AudioChunk, SynthesisEvent};

use crate::macos::stream_drain::drive_streaming_drain;

/// EOS sentinel: AVSpeechSynthesizer signals end-of-utterance with a
/// zero-frame buffer.
const EOS_FRAME_LENGTH: usize = 0;

/// Synthesis timeout. If no `SynthesisEvent::PhraseEnd` arrives within this
/// window, the streaming call returns an error. Sourced from
/// [`STREAM_DRAIN_TIMEOUT_SECS`] so both streaming paths share one tunable.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(STREAM_DRAIN_TIMEOUT_SECS);

/// NSRunLoop slice for the main-thread poll path. Each `runUntilDate` call
/// blocks for this long, draining any pending GCD main-queue callbacks
/// (including AVSpeechSynthesizer PCM callbacks) before returning. Sourced
/// from [`STREAM_RUN_LOOP_SLICE_MS`].
const RUN_LOOP_SLICE: Duration = Duration::from_millis(STREAM_RUN_LOOP_SLICE_MS);

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
pub(super) fn synthesize_streaming(
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
    //
    // The shared [`drive_streaming_drain`] helper owns the deadline
    // check, the `on_event` dispatch, and the PhraseEnd termination
    // rule. The wait-step closure encodes the main-thread shape:
    // `try_recv` first (so any events queued by GCD callbacks since
    // the last slice are drained immediately), then `runUntilDate`
    // for one slice so the next batch of PCM callbacks gets a chance
    // to dispatch.
    let run_loop = NSRunLoop::mainRunLoop();
    let deadline = Instant::now() + DRAIN_TIMEOUT;
    let result = drive_streaming_drain(on_event, deadline, "main-thread streaming", || {
        if let Ok(event) = rx.try_recv() {
            return Ok(Some(event));
        }
        // SAFETY: AVFoundation requires NSRunLoop calls on the main
        // thread. This branch only runs on the main-thread path
        // (gated by the caller dispatch in `synthesize_streaming`).
        let date = NSDate::dateWithTimeIntervalSinceNow(RUN_LOOP_SLICE.as_secs_f64());
        run_loop.runUntilDate(&date);
        Ok(None)
    });

    // Drop the local sender so a stale Block_retain'd callback closure
    // dropping later isn't the last sender alive. `cb` (RcBlock) drops
    // here too; ObjC's own Block_retain keeps it alive for any late
    // callbacks.
    drop(tx);
    drop(cb);
    result
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

    // ── Dispatch work-item context: utterance + sender ────────────────
    //
    // The closure captures `ctx` by move and runs on the GCD main queue
    // exactly once (FnOnce). At that point no other thread holds a
    // reference to the captured `utterance` / `tx`. The `unsafe impl
    // Send` is required because `Retained<AVSpeechUtterance>` is not
    // `Send` by default (objc2 marks `Retained<T>: Send` only when `T:
    // Send + Sync`, which Apple's ObjC classes generally aren't), but
    // the move semantics make the cross-thread transfer single-use and
    // race-free.
    //
    // The `let owned = ctx;` re-binding INSIDE the closure body is
    // load-bearing: under Rust 2021's disjoint capture (RFC 2229),
    // accessing `ctx.utterance` and `ctx.tx` — OR destructuring `ctx`
    // with `let StreamCtx { utterance, tx } = ctx;` — captures each
    // field individually, and the per-field capture of `utterance:
    // Retained<AVSpeechUtterance>` would inherit only its own
    // (non-`Send`) bound, defeating the struct-level `unsafe impl
    // Send`. An identity-rebind expression like `let owned = ctx;`
    // refers to `ctx` as a whole value, which forces whole-struct
    // capture; the closure then becomes `Send` via the unsafe impl,
    // and the subsequent destructure on the local `owned` is just a
    // routine in-scope binding.
    struct StreamCtx {
        utterance: Retained<AVSpeechUtterance>,
        tx: std::sync::mpsc::Sender<SynthesisEvent>,
    }
    // SAFETY: `StreamCtx` is moved into the main-queue closure; at that
    // point no other thread holds a reference. The move is one-shot.
    unsafe impl Send for StreamCtx {}

    let ctx = StreamCtx { utterance, tx };

    // SAFETY rationale for the closure body: it runs on the GCD main
    // queue (via `DispatchQueue::main().exec_async`), which is the queue
    // AVSpeechSynthesizer requires for both `AVSpeechSynthesizer::new()`
    // and `writeUtterance:toBufferCallback:`. The closure is `FnOnce +
    // Send + 'static` as `exec_async` requires.
    DispatchQueue::main().exec_async(move || {
        // Force whole-struct capture under RFC 2229 disjoint capture
        // (see comment above the `StreamCtx` declaration).
        let owned = ctx;
        let StreamCtx { utterance, tx } = owned;

        // SAFETY: AVSpeechSynthesizer::new on the main thread.
        let synth: Retained<AVSpeechSynthesizer> = unsafe { AVSpeechSynthesizer::new() };

        let tx_cb = tx.clone();
        // Keep the synthesizer alive for the whole synthesis by moving a
        // retained clone into the PCM-callback block. `writeUtterance` is
        // async — it only QUEUES synthesis and returns immediately — so the
        // local `synth` would otherwise be released the instant this
        // work-item closure returns, cancelling synthesis before a single
        // buffer callback fires (observed in the GUI background path:
        // `event_count=0`, no PCM callbacks, 30 s drain timeout). The block
        // is Block_retained by AVSpeechSynthesizer until the utterance
        // completes (EOS), so the clone — and thus the synthesizer — lives
        // exactly until synthesis finishes, then is released on the main
        // queue when AVFoundation drops the block. This is the lifetime the
        // main-thread path gets for free by keeping `synth` in function
        // scope across the drain loop.
        let synth_keepalive = synth.clone();
        type CbBlock = block2::Block<dyn Fn(NonNull<AVAudioBuffer>)>;
        let cb = block2::RcBlock::new(move |buf_ptr: NonNull<AVAudioBuffer>| {
            // `synth_keepalive` is captured solely to extend the
            // synthesizer's lifetime to match the block's; it is not used.
            let _ = &synth_keepalive;
            stream_pcm_callback(buf_ptr, &tx_cb);
        });

        // SAFETY: called on the main thread.
        let block_ref: &CbBlock = &cb;
        let block_ptr: *mut CbBlock = (block_ref as *const CbBlock).cast_mut();
        unsafe { synth.writeUtterance_toBufferCallback(&utterance, block_ptr) };

        // ── Drop order at end of scope ──────────────────────────────────
        // `cb`, the local `synth`, and `utterance` all drop here when the
        // work-item returns. Crucially, the synthesizer is NOT deallocated
        // at this point: `synth_keepalive` (a retained clone captured by
        // `cb`) holds a strong reference, and AVSpeechSynthesizer
        // Block_retains `cb` until the utterance finishes (EOS). So the
        // synthesizer survives precisely until synthesis completes, then is
        // released on the main queue when AVFoundation drops the block.
        // The closure's clone of `tx` rides along inside `cb` the same way;
        // when the block is finally released after EOS the sender drops and
        // any later `recv` returns `Disconnected` (ignored after PhraseEnd).
        //
        // This replaces the prior "synth self-retains while utterances are
        // in flight" assumption, which was FALSE for at least the Enhanced
        // en-US voice on macOS (synth dropped here → synthesis cancelled →
        // zero buffer callbacks). The block-capture keepalive is the fix the
        // earlier SAFETY-CRITICAL note anticipated. Leak analysis: the
        // synth↔block cycle is broken when AVSpeechSynthesizer releases the
        // block at EOS; an utterance that errors without ever emitting EOS
        // would leak one synthesizer, an acceptable bound versus the
        // alternative of no audio at all. Audit tag: grep for
        // `SAFETY-CRITICAL on macOS` when bumping the supported macOS floor.
    });

    // ── Drain loop ────────────────────────────────────────────────────
    //
    // The shared [`drive_streaming_drain`] helper owns the deadline
    // check, the `on_event` dispatch, and the PhraseEnd termination
    // rule. The wait-step closure encodes the background shape:
    // `recv_timeout` for one poll slice, returning `Ok(None)` on a
    // benign timeout so the helper re-checks the deadline and tries
    // again — this is what catches a runaway producer that keeps
    // firing Audio events without ever emitting PhraseEnd.
    let poll = Duration::from_millis(STREAM_DRAIN_POLL_MS);
    let deadline = Instant::now() + DRAIN_TIMEOUT;
    drive_streaming_drain(on_event, deadline, "background streaming", || {
        match rx.recv_timeout(poll) {
            Ok(event) => Ok(Some(event)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(PrimerError::Speech(
                "synth channel disconnected before PhraseEnd".into(),
            )),
        }
    })
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
