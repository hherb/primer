//! Main-thread CFRunLoop driver for binaries that don't otherwise
//! spin one.
//!
//! `synthesize_to_chunks_background` (in [`super::tts`]) submits
//! `AVSpeechSynthesizer.writeUtterance:toBufferCallback:` to the GCD
//! main queue via `dispatch_async_f` and waits on a `dispatch_semaphore`.
//! That only completes if *someone* is draining the main CFRunLoop on
//! the OS main thread. Tauri's main thread already does this, so the
//! GUI is fine. A plain `tokio::runtime::Builder::block_on(main)` CLI
//! does NOT — tokio's mio/kqueue reactor never services the GCD main
//! queue — so every background-path TTS call deadlocks for 30 s.
//!
//! The fix is to invert threading: run the tokio runtime on a worker
//! thread, and call [`run_main_loop_until`] from the OS main thread.
//! Worker signals completion via the shared `stop_flag`; main returns
//! within one [`RUN_LOOP_SLICE`] of the flag flipping.
//!
//! Pure ObjC2 plumbing — no Send/Sync concerns. Safe to drop into any
//! binary that has `primer-speech` as a dep with the `macos-native`
//! feature on.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use objc2_foundation::{NSDate, NSRunLoop, NSThread};

/// How often the run loop checks the stop flag. Matches the same
/// cadence the in-synthesis drain in
/// [`super::tts::synthesize_to_chunks_main_thread`] uses — 10 ms is
/// fast enough that AVSpeechSynthesizer PCM buffers stream smoothly
/// and slow enough that a process with no GCD activity isn't burning
/// CPU.
const RUN_LOOP_SLICE: Duration = Duration::from_millis(10);

/// Drive the OS main CFRunLoop on the current thread until
/// `stop_flag` is `true`. Worst-case shutdown latency is one
/// [`RUN_LOOP_SLICE`] (10 ms).
///
/// # Panics
/// Panics if called from any thread other than the OS main thread.
/// The CFRunLoop / `NSRunLoop::mainRunLoop` invariant is enforced by
/// AppKit/Foundation; calling from a worker thread would silently
/// spin a *different* run loop and the GCD main queue would still not
/// be drained.
pub fn run_main_loop_until(stop_flag: Arc<AtomicBool>) {
    assert!(
        NSThread::isMainThread_class(),
        "run_main_loop_until must be called on the OS main thread"
    );

    let run_loop = NSRunLoop::mainRunLoop();

    while !stop_flag.load(Ordering::Acquire) {
        let date = NSDate::dateWithTimeIntervalSinceNow(RUN_LOOP_SLICE.as_secs_f64());
        run_loop.runUntilDate(&date);
    }
}
