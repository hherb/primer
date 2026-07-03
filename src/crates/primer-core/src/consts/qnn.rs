//! Tunables for the on-device QNN per-turn throughput metrics file
//! (`primer_inference::qnn::metrics`).

/// Maximum size, in bytes, the per-turn metrics JSONL file may reach
/// before it is rotated (the live file is renamed to a single `.1`
/// backup and a fresh file started). Total on-disk footprint is
/// therefore bounded at roughly `2 ×` this value — one live file plus
/// one rotated backup.
///
/// Each record is ~120 bytes, so 1 MiB holds ~8.7k turns; with the
/// single-backup rotation up to ~17k recent turns are retained. That is
/// ample for a dev/eval capture session while guaranteeing a long-lived
/// install can never grow the file without bound (issue #228).
pub const METRICS_FILE_MAX_BYTES: u64 = 1024 * 1024;
