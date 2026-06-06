//! QNN-specific benchmark acceptance targets (Phase 1.2 step 1.2.6).
//!
//! The generic timing/aggregation/thermal/run machinery lives in
//! [`crate::bench`]; this module holds only the Qualcomm device's hard
//! acceptance thresholds and a constructor that packages them as
//! [`BenchTargets`]. The QNN benchmark always gates (unlike the llama.cpp
//! probe, which is measurement-first).

use std::time::Duration;

use crate::bench::BenchTargets;

/// Sustained-decode floor (Phase 1.2 "Done definition": ≥ 15 tok/s on
/// Qwen3-4B W4A16).
pub const TARGET_MIN_DECODE_TOKENS_PER_SEC: f64 = 15.0;
/// Time-to-first-token ceiling (Phase 1.2: TTFT < 3 s).
pub const TARGET_MAX_TTFT: Duration = Duration::from_secs(3);
/// Peak-temperature ceiling across the run (Phase 1.2: peak ≤ 70 °C).
pub const TARGET_MAX_PEAK_TEMP_CELSIUS: f64 = 70.0;

/// The QNN acceptance targets as an all-gated [`BenchTargets`].
pub fn qnn_targets() -> BenchTargets {
    BenchTargets {
        min_decode_tokens_per_sec: Some(TARGET_MIN_DECODE_TOKENS_PER_SEC),
        max_ttft: Some(TARGET_MAX_TTFT),
        max_peak_temp_celsius: Some(TARGET_MAX_PEAK_TEMP_CELSIUS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qnn_targets_are_fully_gated() {
        let t = qnn_targets();
        assert!(!t.is_measurement_only());
        assert_eq!(t.min_decode_tokens_per_sec, Some(15.0));
        assert_eq!(t.max_ttft, Some(Duration::from_secs(3)));
        assert_eq!(t.max_peak_temp_celsius, Some(70.0));
    }
}
