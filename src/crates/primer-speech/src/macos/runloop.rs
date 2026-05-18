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

/// RAII guard that flips a shared stop flag on `Drop`. Designed to be
/// held by the worker thread that pairs with [`run_main_loop_until`]:
/// when the guard drops (worker returns normally, returns an `Err`, or
/// panics) the flag is set, [`run_main_loop_until`] exits within one
/// [`RUN_LOOP_SLICE`], and the OS main thread is unblocked.
///
/// Without this, a panic in the worker between "decide to exit" and
/// "explicitly signal the flag" would leave the OS main thread spinning
/// forever. Drop ordering guarantees the flag flips on every exit path.
pub struct StopGuard(Arc<AtomicBool>);

impl StopGuard {
    /// Construct a new guard that will flip `flag` when dropped.
    pub fn new(flag: Arc<AtomicBool>) -> Self {
        Self(flag)
    }
}

impl Drop for StopGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Drive the OS main CFRunLoop on the current thread until
/// `stop_flag` is `true`. Worst-case shutdown latency, measured from
/// the moment the flag flips to the moment this function returns, is
/// one [`RUN_LOOP_SLICE`] (10 ms). It says nothing about how long the
/// worker that owns the flag takes to *decide* to flip it — callers
/// must arrange for that separately (typically via [`StopGuard`] on
/// the worker thread, so the flag flips on every worker exit path).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_guard_sets_flag_on_drop() {
        let flag = Arc::new(AtomicBool::new(false));
        {
            let _guard = StopGuard::new(Arc::clone(&flag));
            assert!(
                !flag.load(Ordering::Acquire),
                "flag must remain unset while guard is alive"
            );
        }
        assert!(
            flag.load(Ordering::Acquire),
            "flag must be set after guard drops"
        );
    }

    #[test]
    fn stop_guard_sets_flag_on_panic_unwind() {
        let flag = Arc::new(AtomicBool::new(false));
        let flag_for_closure = Arc::clone(&flag);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = StopGuard::new(flag_for_closure);
            panic!("worker panic");
        }));
        assert!(result.is_err(), "closure must have panicked");
        assert!(
            flag.load(Ordering::Acquire),
            "flag must be set when guard drops during unwind"
        );
    }

    #[test]
    fn stop_guard_does_not_clear_an_already_set_flag() {
        let flag = Arc::new(AtomicBool::new(true));
        {
            let _guard = StopGuard::new(Arc::clone(&flag));
        }
        assert!(flag.load(Ordering::Acquire), "set flag must remain set");
    }
}
