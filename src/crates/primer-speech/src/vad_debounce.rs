//! Probability → event debounce shared by VAD backends.
//!
//! Any VAD model that emits a per-chunk speech probability can plug into
//! [`VadDebouncer`] to get the threshold + min-silence state machine that
//! produces [`VadEvent::SpeechStart`] / [`VadEvent::SpeechEnd`] transitions.
//! Kept separate from any specific backend so the state machine can be
//! unit-tested without pulling in ONNX Runtime.

use primer_core::speech::VadEvent;

#[derive(Debug, Clone, Copy)]
pub struct VadDebouncer {
    threshold: f32,
    silence_chunks_threshold: u32,
    in_speech: bool,
    silence_chunks: u32,
}

impl VadDebouncer {
    /// `min_silence_chunks` is the count of consecutive sub-threshold chunks
    /// required to leave the speech state. Compute it from desired ms with
    /// `(min_silence_ms * sample_rate) / (chunk_samples * 1000)`, rounded up.
    pub fn new(threshold: f32, min_silence_chunks: u32) -> Self {
        Self {
            threshold,
            silence_chunks_threshold: min_silence_chunks.max(1),
            in_speech: false,
            silence_chunks: 0,
        }
    }

    pub fn reset(&mut self) {
        self.in_speech = false;
        self.silence_chunks = 0;
    }

    /// Feed one chunk's speech probability; get back the transition event.
    pub fn step(&mut self, speech_probability: f32) -> VadEvent {
        let is_speech = speech_probability >= self.threshold;
        match (self.in_speech, is_speech) {
            (false, true) => {
                self.in_speech = true;
                self.silence_chunks = 0;
                VadEvent::SpeechStart
            }
            (true, false) => {
                self.silence_chunks += 1;
                if self.silence_chunks >= self.silence_chunks_threshold {
                    self.in_speech = false;
                    self.silence_chunks = 0;
                    VadEvent::SpeechEnd
                } else {
                    VadEvent::None
                }
            }
            (true, true) => {
                self.silence_chunks = 0;
                VadEvent::None
            }
            (false, false) => VadEvent::None,
        }
    }
}

/// Convert a desired silence duration in ms to a number of chunks at a given
/// sample rate and chunk size. Rounds up so the user's stated minimum is
/// always honoured. Uses integer arithmetic to avoid f32 precision loss at
/// high sample rates or large silence values.
pub fn ms_to_chunks(min_silence_ms: u32, sample_rate: u32, chunk_samples: u32) -> u32 {
    let samples_needed = (min_silence_ms as u64) * (sample_rate as u64);
    let divisor = (chunk_samples as u64) * 1000;
    samples_needed.div_ceil(divisor) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_events(probs: &[f32], threshold: f32, min_silence_chunks: u32) -> Vec<VadEvent> {
        let mut deb = VadDebouncer::new(threshold, min_silence_chunks);
        probs.iter().map(|&p| deb.step(p)).collect()
    }

    #[test]
    fn pure_silence_emits_no_events() {
        let events = collect_events(&[0.1, 0.2, 0.0, 0.3, 0.1], 0.5, 3);
        assert!(events.iter().all(|e| *e == VadEvent::None));
    }

    #[test]
    fn pure_speech_emits_single_start() {
        let events = collect_events(&[0.9, 0.95, 0.8, 0.7], 0.5, 3);
        assert_eq!(events[0], VadEvent::SpeechStart);
        assert!(events[1..].iter().all(|e| *e == VadEvent::None));
    }

    #[test]
    fn brief_dip_does_not_end_speech() {
        // min_silence_chunks = 3 → a single sub-threshold chunk in the
        // middle of speech should not emit SpeechEnd.
        let events = collect_events(&[0.9, 0.9, 0.2, 0.9, 0.9], 0.5, 3);
        assert_eq!(events[0], VadEvent::SpeechStart);
        for e in &events[1..] {
            assert_eq!(*e, VadEvent::None);
        }
    }

    #[test]
    fn sustained_silence_after_speech_emits_end() {
        let events = collect_events(&[0.9, 0.9, 0.1, 0.1, 0.1, 0.1], 0.5, 3);
        assert_eq!(events[0], VadEvent::SpeechStart);
        assert_eq!(events[1], VadEvent::None);
        assert_eq!(events[2], VadEvent::None);
        assert_eq!(events[3], VadEvent::None);
        // 3 consecutive sub-threshold chunks (idx 2..=4) → end at idx 4.
        assert_eq!(events[4], VadEvent::SpeechEnd);
        assert_eq!(events[5], VadEvent::None);
    }

    #[test]
    fn re_entry_into_speech_emits_new_start() {
        let events = collect_events(&[0.9, 0.1, 0.1, 0.1, 0.9], 0.5, 3);
        assert_eq!(events[0], VadEvent::SpeechStart);
        assert_eq!(events[3], VadEvent::SpeechEnd);
        assert_eq!(events[4], VadEvent::SpeechStart);
    }

    #[test]
    fn reset_clears_state() {
        let mut deb = VadDebouncer::new(0.5, 3);
        assert_eq!(deb.step(0.9), VadEvent::SpeechStart);
        deb.reset();
        // After reset, a single low-prob chunk should not trigger an end
        // event (because we're not in speech) and a high-prob chunk should
        // emit a new start.
        assert_eq!(deb.step(0.1), VadEvent::None);
        assert_eq!(deb.step(0.9), VadEvent::SpeechStart);
    }

    #[test]
    fn ms_to_chunks_rounds_up() {
        // 16 kHz, 512-sample chunks → 32 ms per chunk.
        // 300 ms / 32 ms = 9.375 → 10 chunks.
        assert_eq!(ms_to_chunks(300, 16_000, 512), 10);
        // Exact multiple stays exact.
        assert_eq!(ms_to_chunks(320, 16_000, 512), 10);
        // Below one chunk still gives 1.
        assert_eq!(ms_to_chunks(1, 16_000, 512), 1);
    }
}
