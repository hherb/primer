//! cpal-based audio I/O for the speech REPL.
//!
//! See module-level docs further down. This file currently holds the
//! `Resampler` adapter; mic and speaker types follow.

use primer_core::error::{PrimerError, Result};
use rubato::{FftFixedIn, Resampler as RubatoResampler};

/// Sub-chunk count for `FftFixedIn`. 1 = single FFT per call; higher
/// values trade latency for slightly better frequency resolution.
/// 1 is fine for our coarse 48 kHz → 16 kHz / 22 kHz → 48 kHz needs.
const RESAMPLER_SUB_CHUNKS: usize = 1;

/// Adapter around `rubato::FftFixedIn` that resamples mono f32 audio
/// from one sample rate to another.
///
/// Constructed with a fixed input chunk size; callers feed exactly that
/// many samples per `process` call (or pad / split externally). Output
/// chunk size is derived from the rate ratio.
pub struct Resampler {
    inner: FftFixedIn<f32>,
    input_chunk_samples: usize,
}

impl Resampler {
    /// Create a resampler from `input_rate` to `output_rate`, expecting
    /// exactly `input_chunk_samples` mono f32 samples per `process` call.
    pub fn new(input_rate: u32, output_rate: u32, input_chunk_samples: usize) -> Result<Self> {
        let inner = FftFixedIn::<f32>::new(
            input_rate as usize,
            output_rate as usize,
            input_chunk_samples,
            RESAMPLER_SUB_CHUNKS,
            1, // mono
        )
        .map_err(|e| PrimerError::Speech(format!("rubato init: {e}")))?;
        Ok(Self {
            inner,
            input_chunk_samples,
        })
    }

    /// Resample one chunk of mono f32 audio. Input length must equal
    /// the constructor-time `input_chunk_samples`; otherwise errors.
    pub fn process(&mut self, input: &[f32]) -> Result<Vec<f32>> {
        if input.len() != self.input_chunk_samples {
            return Err(PrimerError::Speech(format!(
                "Resampler expected {} samples, got {}",
                self.input_chunk_samples,
                input.len()
            )));
        }
        let out = self
            .inner
            .process(&[input], None)
            .map_err(|e| PrimerError::Speech(format!("rubato process: {e}")))?;
        // mono input → mono output: take the first (only) channel.
        out.into_iter()
            .next()
            .ok_or_else(|| PrimerError::Speech("rubato returned no channels".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity: 48 kHz → 16 kHz resampling produces ~1/3 the sample count.
    /// FFT buffering may cause the first call to return 0 samples; feed two
    /// chunks to verify the rate is correct.
    #[test]
    fn resampler_48k_to_16k_reduces_sample_count_by_about_three() {
        let chunk = 1024;
        let mut r = Resampler::new(48_000, 16_000, chunk).expect("construct");
        let input = vec![0.0f32; chunk];
        // Prime the buffer with a dummy chunk.
        let _primed = r.process(&input).expect("process (prime)");
        // Second chunk; should contain real output.
        let out = r.process(&input).expect("process (main)");
        // Allow ±20% slack for FFT edge effects.
        let expected = chunk / 3;
        let lower = expected * 4 / 5;
        let upper = expected * 6 / 5;
        assert!(
            out.len() >= lower && out.len() <= upper,
            "expected ~{expected} samples, got {}",
            out.len()
        );
    }

    #[test]
    fn resampler_rejects_wrong_input_length() {
        let mut r = Resampler::new(48_000, 16_000, 1024).expect("construct");
        let too_short = vec![0.0f32; 512];
        let err = r.process(&too_short).expect_err("should reject");
        assert!(
            matches!(err, PrimerError::Speech(_)),
            "expected PrimerError::Speech, got {err:?}"
        );
    }
}
