//! Defaults for the session-time-based break-suggestion feature.
//!
//! See [`crate::session_timing`] and the design spec at
//! `docs/superpowers/specs/2026-05-05-session-break-suggestion-design.md`.

/// Minutes between break-suggestion nudges. After this many minutes
/// of session time (or this many minutes since the last suggestion,
/// whichever is more recent), the next pedagogical intent decision
/// returns `SuggestBreak`. Configurable via `PedagogyConfig` and the
/// `--session-break-after-mins` CLI flag. Must be `>= 1` when enabled
/// (a value of 0 disables the gate entirely).
pub const DEFAULT_INTERVAL_MINUTES: u32 = 30;
