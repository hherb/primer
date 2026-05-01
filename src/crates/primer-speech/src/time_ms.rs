//! Pure conversion helpers for millisecond timestamps.
//!
//! Speech backends frequently use signed millisecond offsets (whisper.cpp
//! returns `i64`; Silero's iterator returns `f64` seconds). The Primer's
//! domain types use `u64` ms relative to session open. The conversions
//! are trivial but worth isolating: kept here, they can be tested without
//! pulling in any backend feature, and any future backend that needs the
//! same clamp can reuse them.

/// Clamp a signed-ms timestamp to non-negative `u64`. Negative inputs
/// (which whisper.cpp permits in its type but should not return in
/// practice) are coerced to zero rather than triggering UB through a
/// bare `as` cast.
pub fn clamp_signed_ms_to_u64(ms: i64) -> u64 {
    ms.max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_value_passes_through() {
        assert_eq!(clamp_signed_ms_to_u64(0), 0);
        assert_eq!(clamp_signed_ms_to_u64(1), 1);
        assert_eq!(clamp_signed_ms_to_u64(1_234_567), 1_234_567);
    }

    #[test]
    fn negative_value_clamps_to_zero() {
        assert_eq!(clamp_signed_ms_to_u64(-1), 0);
        assert_eq!(clamp_signed_ms_to_u64(i64::MIN), 0);
    }

    #[test]
    fn max_i64_round_trips() {
        // Whisper timestamps never reach this far, but the cast must
        // not panic or wrap negative if it did.
        assert_eq!(clamp_signed_ms_to_u64(i64::MAX), i64::MAX as u64);
    }
}
