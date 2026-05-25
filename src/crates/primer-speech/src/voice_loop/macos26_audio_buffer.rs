//! Pre-resample mic-sample buffer for the macOS 26 audio capture thread.
//!
//! The audio capture thread pops variable-size batches off the cpal mic
//! ringbuf, accumulates them in a buffer, and emits fixed-size chunks
//! (one per call to the resampler) once the buffer has enough samples.
//! Two responsibilities:
//!
//! 1. Re-chunk variable-size mic pops into the resampler's fixed input size.
//! 2. Anti-feedback: while the Primer is speaking (`is_speaking == true`),
//!    discard every captured sample so AVSpeechSynthesizer's own output
//!    can't leak back into the transcriber via the mic.
//!
//! The bug closed by #139 was that the original implementation only
//! short-circuited the **append** step while speaking — any samples that
//! had already accumulated in the buffer **before** the speak transition
//! sat there waiting, then leaked into the post-speak transcription as
//! the first slice of the next chunk. Concretely: 256 samples buffered
//! → SPEAK begins → 100 ms later SPEAK ends → next iteration appends 256
//! fresh samples → resampler sees 256 old + 256 new instead of 512 new,
//! which could fire a phantom `SpeechStart` on stale audio.
//!
//! The fix is to clear the buffer while `is_speaking` is true. We do this
//! on every speaking iteration (not just the `!is_speaking → is_speaking`
//! edge) so the helper stays stateless beyond the buffer itself — same
//! shape as the matching `raw_buf.clear()` in
//! [`super::backends::run_audio_thread`] (the whisper path) and
//! [`super::backends_macos::run_audio_thread_stt`] (the
//! SFSpeechRecognizer path), both of which have always handled this
//! correctly.
//!
//! Pure helper — no cpal, no rubato, no tokio, no Swift. Lets the
//! state-transition logic be unit-tested without an audio backend dep.

/// Inter-iteration buffer for mic samples captured by the audio thread.
///
/// Owns the `Vec<f32>` accumulator; the fixed chunk size (the resampler's
/// preferred input batch size, in samples at the mic's native rate) is
/// frozen at construction.
pub(crate) struct PendingMicBuffer {
    pending: Vec<f32>,
    chunk_samples: usize,
}

impl PendingMicBuffer {
    /// `chunk_samples` is the resampler's preferred input size (already
    /// scaled from the analyzer's preferred output chunk by `mic_rate /
    /// analyzer_rate`). Must be non-zero — a zero-size chunk would loop
    /// forever inside [`Self::feed`].
    pub fn new(chunk_samples: usize) -> Self {
        debug_assert!(chunk_samples > 0, "chunk_samples must be non-zero");
        Self {
            pending: Vec::with_capacity(chunk_samples * 4),
            chunk_samples,
        }
    }

    /// Feed the latest mic-pop batch into the buffer; return any
    /// resampler-ready chunks the new samples have completed.
    ///
    /// While `is_speaking` is `true` the buffer is cleared and an empty
    /// vec is returned — both the freshly-popped samples AND any
    /// previously-buffered ones are dropped (closes #139). When
    /// `is_speaking` is `false`, samples are appended and chunks of
    /// `chunk_samples` are drained off the front in FIFO order until
    /// fewer than `chunk_samples` remain.
    pub fn feed(&mut self, samples: &[f32], is_speaking: bool) -> Vec<Vec<f32>> {
        if is_speaking {
            self.pending.clear();
            return Vec::new();
        }
        self.pending.extend_from_slice(samples);
        let mut out = Vec::new();
        while self.pending.len() >= self.chunk_samples {
            out.push(self.pending.drain(..self.chunk_samples).collect());
        }
        out
    }

    #[cfg(test)]
    fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHUNK: usize = 512;

    #[test]
    fn empty_input_emits_nothing() {
        let mut buf = PendingMicBuffer::new(CHUNK);
        assert!(buf.feed(&[], false).is_empty());
        assert_eq!(buf.pending_len(), 0);
    }

    #[test]
    fn partial_input_buffers_without_emitting() {
        let mut buf = PendingMicBuffer::new(CHUNK);
        let half = vec![0.1_f32; CHUNK / 2];
        assert!(buf.feed(&half, false).is_empty());
        assert_eq!(buf.pending_len(), CHUNK / 2);
    }

    #[test]
    fn full_chunk_emits_one_and_leaves_buffer_empty() {
        let mut buf = PendingMicBuffer::new(CHUNK);
        let full = vec![0.2_f32; CHUNK];
        let out = buf.feed(&full, false);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), CHUNK);
        assert!(out[0].iter().all(|&s| s == 0.2));
        assert_eq!(buf.pending_len(), 0);
    }

    #[test]
    fn oversized_input_emits_multiple_chunks_with_tail_remaining() {
        let mut buf = PendingMicBuffer::new(CHUNK);
        // 2.5 × chunk worth of samples
        let big = vec![0.3_f32; CHUNK * 5 / 2];
        let out = buf.feed(&big, false);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), CHUNK);
        assert_eq!(out[1].len(), CHUNK);
        assert_eq!(buf.pending_len(), CHUNK / 2);
    }

    #[test]
    fn samples_arrive_in_fifo_order_across_calls() {
        let mut buf = PendingMicBuffer::new(CHUNK);
        // Mark each call with a unique value so we can verify ordering.
        assert!(buf.feed(&vec![1.0_f32; CHUNK / 2], false).is_empty());
        let out = buf.feed(&vec![2.0_f32; CHUNK / 2], false);
        assert_eq!(out.len(), 1);
        // The emitted chunk should be: first half 1.0 (oldest), second half 2.0.
        assert!(out[0][..CHUNK / 2].iter().all(|&s| s == 1.0));
        assert!(out[0][CHUNK / 2..].iter().all(|&s| s == 2.0));
        assert_eq!(buf.pending_len(), 0);
    }

    #[test]
    fn is_speaking_discards_freshly_popped_samples() {
        let mut buf = PendingMicBuffer::new(CHUNK);
        let full = vec![0.4_f32; CHUNK];
        assert!(buf.feed(&full, true).is_empty());
        assert_eq!(buf.pending_len(), 0);
    }

    /// The exact behaviour change closing #139: pre-speak audio left in
    /// the buffer must be discarded when the speaking flag flips, so the
    /// next post-speak chunk contains only fresh post-speak samples.
    ///
    /// Without the fix, the second `feed` call (with `is_speaking=true`)
    /// would be a no-op, the third call would append 256 fresh samples
    /// to the 256 stale ones, and the resampler would receive 256 stale
    /// + 256 fresh — potentially firing a phantom `SpeechStart`.
    #[test]
    fn pre_speak_samples_do_not_leak_past_speak_transition() {
        let mut buf = PendingMicBuffer::new(CHUNK);

        // 1) Pre-speak: half-fill the buffer with marker value 7.0.
        let pre_speak = vec![7.0_f32; CHUNK / 2];
        assert!(buf.feed(&pre_speak, false).is_empty());
        assert_eq!(buf.pending_len(), CHUNK / 2);

        // 2) SPEAK starts: a mic pop arrives while is_speaking is true.
        //    Both the freshly-popped samples AND the previously-buffered
        //    pre-speak samples must be discarded.
        let during_speak = vec![9.0_f32; 100];
        assert!(buf.feed(&during_speak, true).is_empty());
        assert_eq!(
            buf.pending_len(),
            0,
            "pre-speak buffer must be cleared on speak transition (#139)"
        );

        // 3) SPEAK ends: a full chunk of fresh post-speak samples arrives.
        //    Must emit exactly one chunk, all marker value 3.0 — none of
        //    the 7.0 stale samples should survive.
        let post_speak = vec![3.0_f32; CHUNK];
        let out = buf.feed(&post_speak, false);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), CHUNK);
        assert!(
            out[0].iter().all(|&s| s == 3.0),
            "post-speak chunk must contain only post-speak samples"
        );
        assert_eq!(buf.pending_len(), 0);
    }

    /// Sanity check that subsequent SPEAK iterations keep clearing — not
    /// just the first one. (A naive "only clear on edge" implementation
    /// would still pass `pre_speak_samples_do_not_leak_past_speak_transition`
    /// above; this test would fail it only if speech-edge tracking
    /// regressed in a way that re-buffered samples mid-SPEAK.)
    #[test]
    fn repeated_speak_iterations_stay_clear() {
        let mut buf = PendingMicBuffer::new(CHUNK);
        for _ in 0..10 {
            assert!(buf.feed(&vec![5.0_f32; 200], true).is_empty());
            assert_eq!(buf.pending_len(), 0);
        }
    }
}
