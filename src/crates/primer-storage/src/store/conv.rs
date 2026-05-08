//! Pure type-conversion helpers used across the storage layer.
//!
//! SQLite stores integers as 64-bit signed values; the in-memory model
//! uses narrower unsigned integers (`u8` for `age`, `u32` for counts,
//! `u64` for non-negative seconds). A silent `as` cast wraps mod-2^N,
//! which is the wrong failure mode for a corrupt-DB read path — these
//! helpers return a `Result` instead.

use chrono::{DateTime, Utc};
use primer_core::error::{PrimerError, Result};

pub(super) fn parse_rfc3339(s: &str, field: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| PrimerError::Storage(format!("parse {field} {s:?}: {e}")))
}

/// Narrow a SQLite-stored `i64` to `u8`. SQLite's INTEGER column type
/// is 64-bit signed, but the in-memory model uses `u8` for `age`. A
/// silent `as` cast wraps mod 256 (so 300 → 44, -1 → 255), which is
/// exactly the wrong failure mode for a corrupt-DB read path.
pub(super) fn i64_to_u8(value: i64, field: &str) -> Result<u8> {
    u8::try_from(value).map_err(|_| {
        PrimerError::Storage(format!(
            "{field} value {value} is outside the valid u8 range 0..=255"
        ))
    })
}

/// Narrow a SQLite-stored `i64` to `u32`. Same rationale as
/// `i64_to_u8`.
pub(super) fn i64_to_u32(value: i64, field: &str) -> Result<u32> {
    u32::try_from(value).map_err(|_| {
        PrimerError::Storage(format!(
            "{field} value {value} is outside the valid u32 range 0..=4294967295"
        ))
    })
}

/// Narrow a SQLite-stored `i64` to `u64`. Negatives are rejected
/// rather than clamped — a stored negative seconds value indicates
/// corruption, not "interpret as zero".
pub(super) fn i64_to_u64(value: i64, field: &str) -> Result<u64> {
    u64::try_from(value)
        .map_err(|_| PrimerError::Storage(format!("{field} value {value} must be non-negative")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rfc3339_round_trips_utc() {
        let s = "2026-05-07T12:34:56Z";
        let dt = parse_rfc3339(s, "ts").expect("parse");
        assert_eq!(dt.to_rfc3339(), "2026-05-07T12:34:56+00:00");
    }

    #[test]
    fn parse_rfc3339_normalises_offset_to_utc() {
        let dt = parse_rfc3339("2026-05-07T12:00:00+02:00", "ts").expect("parse");
        // Offset stripped, timestamp converted to UTC.
        assert_eq!(dt.to_rfc3339(), "2026-05-07T10:00:00+00:00");
    }

    #[test]
    fn parse_rfc3339_returns_error_with_field_name() {
        let err = parse_rfc3339("not-a-date", "started_at")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("started_at"),
            "error includes field name: {err}"
        );
        assert!(err.contains("not-a-date"), "error includes input: {err}");
    }

    #[test]
    fn i64_to_u8_accepts_valid() {
        assert_eq!(i64_to_u8(0, "age").unwrap(), 0);
        assert_eq!(i64_to_u8(255, "age").unwrap(), 255);
        assert_eq!(i64_to_u8(8, "age").unwrap(), 8);
    }

    #[test]
    fn i64_to_u8_rejects_negative_and_overflow() {
        assert!(i64_to_u8(-1, "age").is_err());
        assert!(i64_to_u8(256, "age").is_err());
        assert!(i64_to_u8(i64::MAX, "age").is_err());
    }

    #[test]
    fn i64_to_u32_accepts_valid_and_rejects_out_of_range() {
        assert_eq!(i64_to_u32(0, "encounter").unwrap(), 0);
        assert_eq!(i64_to_u32(u32::MAX as i64, "encounter").unwrap(), u32::MAX);
        assert!(i64_to_u32(-1, "encounter").is_err());
        assert!(i64_to_u32(u32::MAX as i64 + 1, "encounter").is_err());
    }

    #[test]
    fn i64_to_u64_rejects_negative_only() {
        assert_eq!(i64_to_u64(0, "secs").unwrap(), 0);
        assert_eq!(i64_to_u64(i64::MAX, "secs").unwrap(), i64::MAX as u64);
        assert!(i64_to_u64(-1, "secs").is_err());
    }
}
