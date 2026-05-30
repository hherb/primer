//! Timing metrics and pass/fail evaluation for the QNN benchmark harness.
//!
//! Everything here is pure: the example feeds in per-prompt
//! [`PromptMeasurement`]s (captured around real `generate_stream` calls)
//! plus an optional peak thermal reading, and gets back an aggregate
//! [`BenchReport`] and a [`Verdict`] against the Phase 1.2 acceptance
//! targets. No clock, no device, no I/O — so the aggregation logic is
//! fully unit-tested on host even though the measurements themselves can
//! only be captured on a Qualcomm device.

use std::time::Duration;

/// Sustained-decode floor the benchmark must clear (Phase 1.2 "Done
/// definition": sustained decode ≥ 15 tok/s on Qwen3-4B W4A16).
pub const TARGET_MIN_DECODE_TOKENS_PER_SEC: f64 = 15.0;
/// Time-to-first-token ceiling (Phase 1.2: TTFT < 3 s).
pub const TARGET_MAX_TTFT: Duration = Duration::from_secs(3);
/// Peak-temperature ceiling across the run (Phase 1.2: peak ≤ 70 °C).
pub const TARGET_MAX_PEAK_TEMP_CELSIUS: f64 = 70.0;

/// Percentile selector for the typical-case TTFT figure.
pub const PERCENTILE_P50: f64 = 50.0;
/// Percentile selector for the tail-case TTFT figure used in the verdict.
pub const PERCENTILE_P95: f64 = 95.0;

/// Acceptance thresholds the benchmark checks each run against.
///
/// `Default` reads the `TARGET_*` constants; the example exposes CLI
/// overrides so a developer probing a smaller model can relax them without
/// recompiling.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchTargets {
    /// Minimum acceptable *sustained* decode rate (checked against the
    /// run's slowest prompt, not its mean).
    pub min_decode_tokens_per_sec: f64,
    /// Maximum acceptable TTFT (checked against the p95, not the median).
    pub max_ttft: Duration,
    /// Maximum acceptable peak temperature across the run.
    pub max_peak_temp_celsius: f64,
}

impl Default for BenchTargets {
    fn default() -> Self {
        Self {
            min_decode_tokens_per_sec: TARGET_MIN_DECODE_TOKENS_PER_SEC,
            max_ttft: TARGET_MAX_TTFT,
            max_peak_temp_celsius: TARGET_MAX_PEAK_TEMP_CELSIUS,
        }
    }
}

/// Timing for one prompt run.
///
/// `decode_tokens` is the count of token chunks received *after* the first
/// — Genie invokes its token callback once per decoded token, so for the
/// QNN backend this is an exact decoded-token count, not an estimate. The
/// first token's arrival is attributed to [`Self::ttft`] (prefill +
/// first decode); `decode_duration` covers first-token → done, so
/// [`Self::decode_tokens_per_sec`] measures the steady-state decode rate
/// rather than being diluted by prefill latency.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptMeasurement {
    /// The prompt label this measurement belongs to (for the per-prompt log).
    pub label: String,
    /// Time from issuing `generate_stream` to the first non-empty chunk.
    pub ttft: Duration,
    /// Tokens decoded after the first (chunk count, == token count for QNN).
    pub decode_tokens: usize,
    /// Wall-clock from first token to the `done` chunk.
    pub decode_duration: Duration,
}

impl PromptMeasurement {
    /// Steady-state decode rate in tokens per second. Returns `0.0` when no
    /// decode time elapsed (a degenerate single-token or zero-token run) so
    /// callers never divide by zero.
    pub fn decode_tokens_per_sec(&self) -> f64 {
        let secs = self.decode_duration.as_secs_f64();
        if secs <= 0.0 {
            return 0.0;
        }
        self.decode_tokens as f64 / secs
    }
}

/// Nearest-rank percentile over a slice of durations.
///
/// `p` is a percentage in `[0, 100]` (clamped). Returns `None` for an
/// empty slice. Uses the nearest-rank method (rank = ⌈p/100 · n⌉, 1-based)
/// which needs no interpolation and is stable for the small sample counts a
/// benchmark produces. Does not mutate the input — it sorts a local copy.
pub fn percentile_duration(samples: &[Duration], p: f64) -> Option<Duration> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let p = p.clamp(0.0, 100.0);
    let n = sorted.len();
    // Nearest-rank: smallest 1-based rank R such that R/n >= p/100.
    let rank = (p / 100.0 * n as f64).ceil() as usize;
    let index = rank.saturating_sub(1).min(n - 1);
    Some(sorted[index])
}

/// Aggregate report over all prompt runs in a benchmark.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchReport {
    /// Number of prompt runs aggregated.
    pub runs: usize,
    /// Runs excluded from the decode mean/min because they decoded no tokens
    /// after the first (rate `0.0` — an early stop, not sustained
    /// throughput). Surfaced so a run dominated by short completions is
    /// visible rather than silently dragging the verdict to fail.
    pub degenerate_runs: usize,
    /// Median (p50) time-to-first-token.
    pub ttft_p50: Duration,
    /// Tail (p95) time-to-first-token — the figure the verdict gates on.
    pub ttft_p95: Duration,
    /// Mean decode rate across the non-degenerate runs.
    pub decode_mean_tokens_per_sec: f64,
    /// Slowest non-degenerate single-run decode rate — the "sustained"
    /// figure the verdict gates on. Runs that decoded ≤ 1 token (rate `0.0`)
    /// are excluded; see [`Self::degenerate_runs`]. Falls back to `0.0` when
    /// *every* run was degenerate (nothing decoded → the decode gate fails).
    pub decode_min_tokens_per_sec: f64,
    /// Peak temperature across the run, or `None` if no thermal samples
    /// were captured (e.g. a host dry-run with no sysfs thermal nodes).
    pub peak_temp_celsius: Option<f64>,
}

impl BenchReport {
    /// Aggregate per-prompt measurements into a report. Returns `None` when
    /// `measurements` is empty (nothing to report on).
    pub fn from_measurements(
        measurements: &[PromptMeasurement],
        peak_temp_celsius: Option<f64>,
    ) -> Option<Self> {
        if measurements.is_empty() {
            return None;
        }
        let ttfts: Vec<Duration> = measurements.iter().map(|m| m.ttft).collect();

        // Exclude degenerate runs (rate 0.0 — decoded ≤ 1 token, so an early
        // stop rather than a sustained stream) from both aggregates. A single
        // short completion must not be allowed to define "sustained" and fail
        // the whole run; the excluded count is reported separately.
        let scored_rates: Vec<f64> = measurements
            .iter()
            .map(PromptMeasurement::decode_tokens_per_sec)
            .filter(|&r| r > 0.0)
            .collect();
        let degenerate_runs = measurements.len() - scored_rates.len();

        // When every run was degenerate, there is no sustained throughput to
        // report: 0.0 correctly fails the decode gate.
        let (mean, min) = if scored_rates.is_empty() {
            (0.0, 0.0)
        } else {
            let mean = scored_rates.iter().sum::<f64>() / scored_rates.len() as f64;
            let min = scored_rates.iter().copied().fold(f64::INFINITY, f64::min);
            (mean, min)
        };

        Some(Self {
            runs: measurements.len(),
            degenerate_runs,
            // Safe to unwrap: the slice is non-empty (guarded above).
            ttft_p50: percentile_duration(&ttfts, PERCENTILE_P50).unwrap(),
            ttft_p95: percentile_duration(&ttfts, PERCENTILE_P95).unwrap(),
            decode_mean_tokens_per_sec: mean,
            decode_min_tokens_per_sec: min,
            peak_temp_celsius,
        })
    }
}

/// Per-criterion pass/fail outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    /// Sustained decode rate met the floor.
    pub decode_pass: bool,
    /// p95 TTFT stayed strictly under the ceiling (Phase 1.2 target is
    /// `< 3 s`, so exactly 3 s fails).
    pub ttft_pass: bool,
    /// Peak temperature stayed under the ceiling (vacuously true when no
    /// thermal data was captured — a host dry-run can't fail thermal).
    pub thermal_pass: bool,
}

impl Verdict {
    /// True only when every gated criterion passed.
    pub fn all_pass(&self) -> bool {
        self.decode_pass && self.ttft_pass && self.thermal_pass
    }
}

/// Evaluate a report against the acceptance targets.
///
/// The decode gate uses the **minimum** (sustained) rate and the TTFT gate
/// uses the **p95** tail — both deliberately conservative, so a run that
/// passes the verdict passes for the worst representative prompt, not just
/// on average. The decode (`≥`) and thermal (`≤`) boundaries are inclusive;
/// the TTFT boundary is **exclusive** (`< 3 s`), each matching the operator
/// in its Phase 1.2 target. Absent thermal data passes the thermal gate
/// vacuously; on a real device the sampler always produces readings.
pub fn evaluate(report: &BenchReport, targets: &BenchTargets) -> Verdict {
    Verdict {
        decode_pass: report.decode_min_tokens_per_sec >= targets.min_decode_tokens_per_sec,
        ttft_pass: report.ttft_p95 < targets.max_ttft,
        thermal_pass: match report.peak_temp_celsius {
            Some(t) => t <= targets.max_peak_temp_celsius,
            None => true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// f64 comparison epsilon for rate assertions.
    const EPS: f64 = 1e-9;

    fn measurement(label: &str, ttft_ms: u64, tokens: usize, decode_ms: u64) -> PromptMeasurement {
        PromptMeasurement {
            label: label.to_string(),
            ttft: Duration::from_millis(ttft_ms),
            decode_tokens: tokens,
            decode_duration: Duration::from_millis(decode_ms),
        }
    }

    #[test]
    fn decode_rate_is_tokens_over_decode_seconds() {
        // 30 tokens in 2.0 s = 15 tok/s.
        let m = measurement("x", 500, 30, 2000);
        assert!((m.decode_tokens_per_sec() - 15.0).abs() < EPS);
    }

    #[test]
    fn decode_rate_is_zero_when_no_decode_time() {
        let m = measurement("x", 500, 0, 0);
        assert_eq!(m.decode_tokens_per_sec(), 0.0);
    }

    #[test]
    fn percentile_of_empty_is_none() {
        assert_eq!(percentile_duration(&[], PERCENTILE_P50), None);
    }

    #[test]
    fn percentile_nearest_rank() {
        let s: Vec<Duration> = [10, 20, 30, 40, 50]
            .iter()
            .map(|&ms| Duration::from_millis(ms))
            .collect();
        // p50 of 5 items: rank = ceil(0.5*5)=3 → index 2 → 30ms.
        assert_eq!(
            percentile_duration(&s, 50.0),
            Some(Duration::from_millis(30))
        );
        // p95: rank = ceil(0.95*5)=5 → index 4 → 50ms.
        assert_eq!(
            percentile_duration(&s, 95.0),
            Some(Duration::from_millis(50))
        );
        // p0 clamps to the smallest.
        assert_eq!(
            percentile_duration(&s, 0.0),
            Some(Duration::from_millis(10))
        );
        // p100 is the largest.
        assert_eq!(
            percentile_duration(&s, 100.0),
            Some(Duration::from_millis(50))
        );
    }

    #[test]
    fn percentile_does_not_mutate_input() {
        let s = vec![
            Duration::from_millis(30),
            Duration::from_millis(10),
            Duration::from_millis(20),
        ];
        let before = s.clone();
        let _ = percentile_duration(&s, 50.0);
        assert_eq!(s, before);
    }

    #[test]
    fn report_is_none_for_empty_measurements() {
        assert!(BenchReport::from_measurements(&[], Some(50.0)).is_none());
    }

    #[test]
    fn report_aggregates_mean_min_and_percentiles() {
        let measurements = vec![
            measurement("a", 1000, 30, 2000), // 15 tok/s
            measurement("b", 2000, 60, 2000), // 30 tok/s
            measurement("c", 1500, 20, 2000), // 10 tok/s (slowest)
        ];
        let report = BenchReport::from_measurements(&measurements, Some(62.0)).unwrap();
        assert_eq!(report.runs, 3);
        assert_eq!(report.degenerate_runs, 0);
        // Mean of 15, 30, 10 = 18.333…
        assert!((report.decode_mean_tokens_per_sec - (55.0 / 3.0)).abs() < EPS);
        assert!((report.decode_min_tokens_per_sec - 10.0).abs() < EPS);
        // TTFTs sorted: 1000,1500,2000 → p50 rank ceil(1.5)=2 → 1500.
        assert_eq!(report.ttft_p50, Duration::from_millis(1500));
        // p95 rank ceil(2.85)=3 → 2000.
        assert_eq!(report.ttft_p95, Duration::from_millis(2000));
        assert_eq!(report.peak_temp_celsius, Some(62.0));
    }

    #[test]
    fn report_excludes_degenerate_runs_from_min() {
        // The middle run decoded only its first token (0 after-first tokens),
        // so its rate is 0.0 and must not define the sustained minimum.
        let measurements = vec![
            measurement("a", 1000, 40, 2000),      // 20 tok/s
            measurement("degenerate", 1000, 0, 0), // 0 tok/s — excluded
            measurement("c", 1000, 36, 2000),      // 18 tok/s (slowest scored)
        ];
        let report = BenchReport::from_measurements(&measurements, None).unwrap();
        assert_eq!(report.runs, 3);
        assert_eq!(report.degenerate_runs, 1);
        // Min is the slowest *scored* run, not the degenerate 0.0.
        assert!((report.decode_min_tokens_per_sec - 18.0).abs() < EPS);
        // Mean averages only the two scored runs: (20 + 18) / 2 = 19.
        assert!((report.decode_mean_tokens_per_sec - 19.0).abs() < EPS);
        // With the degenerate run excluded, the run clears the floor.
        assert!(evaluate(&report, &BenchTargets::default()).decode_pass);
    }

    #[test]
    fn report_all_degenerate_yields_zero_and_fails_decode() {
        let measurements = vec![measurement("a", 1000, 0, 0), measurement("b", 1200, 0, 0)];
        let report = BenchReport::from_measurements(&measurements, Some(50.0)).unwrap();
        assert_eq!(report.runs, 2);
        assert_eq!(report.degenerate_runs, 2);
        assert_eq!(report.decode_min_tokens_per_sec, 0.0);
        assert_eq!(report.decode_mean_tokens_per_sec, 0.0);
        // Nothing decoded a sustained stream → the decode gate fails.
        assert!(!evaluate(&report, &BenchTargets::default()).decode_pass);
    }

    #[test]
    fn verdict_passes_when_all_criteria_met() {
        let report = BenchReport {
            runs: 5,
            degenerate_runs: 0,
            ttft_p50: Duration::from_millis(1200),
            ttft_p95: Duration::from_millis(2500),
            decode_mean_tokens_per_sec: 22.0,
            decode_min_tokens_per_sec: 16.0,
            peak_temp_celsius: Some(65.0),
        };
        let v = evaluate(&report, &BenchTargets::default());
        assert!(v.decode_pass);
        assert!(v.ttft_pass);
        assert!(v.thermal_pass);
        assert!(v.all_pass());
    }

    #[test]
    fn verdict_fails_each_criterion_independently() {
        let targets = BenchTargets::default();

        // Sustained decode below floor.
        let slow = BenchReport {
            runs: 1,
            degenerate_runs: 0,
            ttft_p50: Duration::from_millis(500),
            ttft_p95: Duration::from_millis(500),
            decode_mean_tokens_per_sec: 14.0,
            decode_min_tokens_per_sec: 14.0,
            peak_temp_celsius: Some(50.0),
        };
        let v = evaluate(&slow, &targets);
        assert!(!v.decode_pass);
        assert!(!v.all_pass());

        // TTFT p95 over ceiling.
        let laggy = BenchReport {
            ttft_p95: Duration::from_millis(3001),
            ..slow.clone()
        };
        let v = evaluate(
            &BenchReport {
                decode_min_tokens_per_sec: 20.0,
                ..laggy
            },
            &targets,
        );
        assert!(!v.ttft_pass);

        // Peak temperature over ceiling.
        let hot = BenchReport {
            decode_min_tokens_per_sec: 20.0,
            ttft_p95: Duration::from_millis(1000),
            peak_temp_celsius: Some(70.1),
            ..slow
        };
        let v = evaluate(&hot, &targets);
        assert!(!v.thermal_pass);
    }

    #[test]
    fn verdict_thermal_passes_vacuously_without_samples() {
        let report = BenchReport {
            runs: 1,
            degenerate_runs: 0,
            ttft_p50: Duration::from_millis(500),
            ttft_p95: Duration::from_millis(500),
            decode_mean_tokens_per_sec: 20.0,
            decode_min_tokens_per_sec: 20.0,
            peak_temp_celsius: None,
        };
        let v = evaluate(&report, &BenchTargets::default());
        assert!(v.thermal_pass);
        assert!(v.all_pass());
    }

    #[test]
    fn decode_and_thermal_boundaries_are_inclusive() {
        // ≥ 15 tok/s and ≤ 70 °C: exactly at target passes.
        let report = BenchReport {
            runs: 1,
            degenerate_runs: 0,
            ttft_p50: Duration::from_millis(500),
            ttft_p95: Duration::from_millis(500),
            decode_mean_tokens_per_sec: TARGET_MIN_DECODE_TOKENS_PER_SEC,
            decode_min_tokens_per_sec: TARGET_MIN_DECODE_TOKENS_PER_SEC,
            peak_temp_celsius: Some(TARGET_MAX_PEAK_TEMP_CELSIUS),
        };
        let v = evaluate(&report, &BenchTargets::default());
        assert!(v.decode_pass);
        assert!(v.thermal_pass);
        assert!(v.all_pass());
    }

    #[test]
    fn ttft_boundary_is_exclusive() {
        // The TTFT target is strict (`< 3 s`): exactly 3 s fails, just under
        // passes — unlike the inclusive decode/thermal gates.
        let targets = BenchTargets::default();
        let at_ceiling = BenchReport {
            runs: 1,
            degenerate_runs: 0,
            ttft_p50: TARGET_MAX_TTFT,
            ttft_p95: TARGET_MAX_TTFT,
            decode_mean_tokens_per_sec: 20.0,
            decode_min_tokens_per_sec: 20.0,
            peak_temp_celsius: Some(50.0),
        };
        assert!(!evaluate(&at_ceiling, &targets).ttft_pass);

        let just_under = BenchReport {
            ttft_p95: TARGET_MAX_TTFT - Duration::from_millis(1),
            ..at_ceiling
        };
        assert!(evaluate(&just_under, &targets).ttft_pass);
    }
}
