//! Pure scheduling helpers for the session-time-based break suggestion.
//!
//! [`should_suggest_break_now`] takes `now` as a parameter so it's
//! deterministic and testable; the dialogue manager calls it from
//! `respond_to_streaming` once per turn to decide whether the next
//! pedagogical intent should be `SuggestBreak`.
//!
//! Constants come from [`crate::consts::break_suggest`].

use chrono::{DateTime, Utc};

/// Carries break-gate state into intent decisions. The carrier exists so
/// `decide_intent_at_with_pack` can take a single parameter rather than
/// two scalars; `disabled()` is the no-op gate that the no-pack/no-time
/// `decide_intent` and `decide_intent_at` wrappers default to so the
/// pre-existing characterization tests pass verbatim.
#[derive(Debug, Clone, Copy)]
pub struct BreakGate {
    /// Minutes between break suggestions. `0` disables the gate.
    pub interval_minutes: u32,
    /// Wallclock timestamp of the last `SuggestBreak` fire in this session;
    /// `None` if none has fired yet.
    pub last_suggested_at: Option<DateTime<Utc>>,
}

impl BreakGate {
    /// The disabled gate — `should_suggest_break_now` always returns false.
    pub fn disabled() -> Self {
        Self {
            interval_minutes: 0,
            last_suggested_at: None,
        }
    }
}

/// Returns true when a `SuggestBreak` intent should fire on the next turn.
///
/// Reference point is `last_suggested_at` if set, otherwise `started_at`.
/// Returns false when:
///   - `interval_minutes == 0` (gate disabled),
///   - elapsed time since the reference point is less than the interval,
///   - the clock went backwards (`now < reference`).
pub fn should_suggest_break_now(
    now: DateTime<Utc>,
    started_at: DateTime<Utc>,
    last_suggested_at: Option<DateTime<Utc>>,
    interval_minutes: u32,
) -> bool {
    if interval_minutes == 0 {
        return false;
    }
    let reference = last_suggested_at.unwrap_or(started_at);
    let elapsed_mins = now.signed_duration_since(reference).num_minutes();
    elapsed_mins >= 0 && (elapsed_mins as u32) >= interval_minutes
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Fixed reference point so tests don't depend on the wallclock.
    fn t(min: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 5, 12, 0, 0).unwrap() + chrono::Duration::minutes(min)
    }

    #[test]
    fn break_gate_disabled_has_zero_interval_and_no_prior() {
        let g = BreakGate::disabled();
        assert_eq!(g.interval_minutes, 0);
        assert!(g.last_suggested_at.is_none());
    }

    #[test]
    fn gate_disabled_never_fires() {
        // interval=0 means disabled — should always be false even past threshold.
        assert!(!should_suggest_break_now(t(120), t(0), None, 0));
    }

    #[test]
    fn pre_threshold_no_prior_does_not_fire() {
        // 5 minutes elapsed, 30 min interval — too early.
        assert!(!should_suggest_break_now(t(5), t(0), None, 30));
    }

    #[test]
    fn exactly_at_threshold_fires() {
        // Boundary: elapsed == interval should fire (>=).
        assert!(should_suggest_break_now(t(30), t(0), None, 30));
    }

    #[test]
    fn post_threshold_no_prior_fires() {
        assert!(should_suggest_break_now(t(35), t(0), None, 30));
    }

    #[test]
    fn post_threshold_with_prior_not_yet_due_does_not_fire() {
        // Started at 0, suggested at 30, now at 45 — only 15 min
        // since last suggestion, interval is 30, so not due.
        assert!(!should_suggest_break_now(t(45), t(0), Some(t(30)), 30));
    }

    #[test]
    fn post_threshold_with_prior_due_again_fires() {
        // Started at 0, suggested at 30, now at 65 — 35 min since
        // last suggestion (>= 30), so due again.
        assert!(should_suggest_break_now(t(65), t(0), Some(t(30)), 30));
    }

    #[test]
    fn clock_going_backwards_does_not_fire() {
        // `now` is before `started_at`. signed_duration is negative;
        // we should return false rather than treating wraparound as overdue.
        assert!(!should_suggest_break_now(t(0), t(60), None, 30));
    }
}
