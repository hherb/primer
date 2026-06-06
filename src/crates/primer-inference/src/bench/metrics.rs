//! Timing metrics and pass/fail evaluation for the benchmark harness.
//!
//! Pure: callers feed in per-prompt [`PromptMeasurement`]s plus an optional
//! peak thermal reading and get back an aggregate [`BenchReport`] and a
//! [`Verdict`] against caller-supplied [`BenchTargets`]. No clock, no device,
//! no I/O — so the aggregation logic is fully unit-tested on host. Shared by
//! every inference backend's benchmark; backend-specific acceptance targets
//! live next to that backend (e.g. `qnn::bench::qnn_targets`).

use std::time::Duration;

/// Percentile selector for the typical-case TTFT figure.
pub const PERCENTILE_P50: f64 = 50.0;
/// Percentile selector for the tail-case TTFT figure used in the verdict.
pub const PERCENTILE_P95: f64 = 95.0;

/// Acceptance thresholds a run is checked against.
///
/// Every field is optional: `None` means "do not gate this criterion" (a
/// vacuous pass). All-`None` ([`Self::none`]) is a pure measurement run with
/// no pass/fail. A backend with hard acceptance targets (e.g. QNN) supplies
/// all-`Some`; the llama.cpp probe supplies only the fields the developer
/// passed on the CLI.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchTargets {
    /// Minimum acceptable *sustained* decode rate (checked against the run's
    /// slowest scored prompt). `None` → not gated.
    pub min_decode_tokens_per_sec: Option<f64>,
    /// Maximum acceptable TTFT (checked against the p95). `None` → not gated.
    pub max_ttft: Option<Duration>,
    /// Maximum acceptable peak temperature. `None` → not gated.
    pub max_peak_temp_celsius: Option<f64>,
}

impl BenchTargets {
    /// A measurement-only target set: no criterion is gated.
    pub fn none() -> Self {
        Self {
            min_decode_tokens_per_sec: None,
            max_ttft: None,
            max_peak_temp_celsius: None,
        }
    }

    /// True when no criterion is gated — the run is a pure measurement and the
    /// caller should report numbers without a PASS/FAIL verdict.
    pub fn is_measurement_only(&self) -> bool {
        self.min_decode_tokens_per_sec.is_none()
            && self.max_ttft.is_none()
            && self.max_peak_temp_celsius.is_none()
    }
}

impl Default for BenchTargets {
    fn default() -> Self {
        Self::none()
    }
}

/// Timing for one prompt run.
///
/// `decode_tokens` is the count of token chunks received *after* the first.
/// The first chunk's arrival is attributed to [`Self::ttft`] (prefill + first
/// decode); `decode_duration` covers first-token → last-token, so
/// [`Self::decode_tokens_per_sec`] measures the steady-state decode rate
/// rather than being diluted by prefill latency.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptMeasurement {
    /// The prompt label this measurement belongs to (for the per-prompt log).
    pub label: String,
    /// Time from issuing `generate_stream` to the first non-empty chunk.
    pub ttft: Duration,
    /// Chunks received after the first.
    pub decode_tokens: usize,
    /// Wall-clock from first token to the last token.
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
/// `p` is a percentage in `[0, 100]` (clamped). Returns `None` for an empty
/// slice. Uses the nearest-rank method (rank = ⌈p/100 · n⌉, 1-based), which
/// needs no interpolation and is stable for small sample counts. Does not
/// mutate the input — it sorts a local copy.
pub fn percentile_duration(samples: &[Duration], p: f64) -> Option<Duration> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let p = p.clamp(0.0, 100.0);
    let n = sorted.len();
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
    /// after the first (rate `0.0` — an early stop, not sustained throughput).
    pub degenerate_runs: usize,
    /// Median (p50) time-to-first-token.
    pub ttft_p50: Duration,
    /// Tail (p95) time-to-first-token — the figure the verdict gates on.
    pub ttft_p95: Duration,
    /// Mean decode rate across the non-degenerate runs.
    pub decode_mean_tokens_per_sec: f64,
    /// Slowest non-degenerate single-run decode rate — the "sustained" figure
    /// the verdict gates on. Falls back to `0.0` when *every* run was
    /// degenerate (nothing decoded → the decode gate fails).
    pub decode_min_tokens_per_sec: f64,
    /// Peak temperature across the run, or `None` if no thermal samples were
    /// captured (e.g. a host dry-run with no sysfs thermal nodes).
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

        // Exclude degenerate runs (rate 0.0 — decoded ≤ 1 token) from both
        // aggregates; the excluded count is reported separately.
        let scored_rates: Vec<f64> = measurements
            .iter()
            .map(PromptMeasurement::decode_tokens_per_sec)
            .filter(|&r| r > 0.0)
            .collect();
        let degenerate_runs = measurements.len() - scored_rates.len();

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
    /// Sustained decode rate met the floor (or the floor was not gated).
    pub decode_pass: bool,
    /// p95 TTFT stayed strictly under the ceiling (or it was not gated).
    pub ttft_pass: bool,
    /// Peak temperature stayed under the ceiling (vacuously true when the
    /// target is unset OR no thermal data was captured).
    pub thermal_pass: bool,
}

impl Verdict {
    /// True only when every criterion passed (gated or vacuous).
    pub fn all_pass(&self) -> bool {
        self.decode_pass && self.ttft_pass && self.thermal_pass
    }
}

/// Evaluate a report against the acceptance targets.
///
/// A `None` target is a vacuous pass for that criterion. The decode gate uses
/// the **minimum** (sustained) rate and the TTFT gate uses the **p95** tail.
/// The decode (`≥`) and thermal (`≤`) boundaries are inclusive; the TTFT
/// boundary is **exclusive** (`< target`). Thermal is doubly-vacuous: it
/// passes when the target is unset OR no sample was captured.
pub fn evaluate(report: &BenchReport, targets: &BenchTargets) -> Verdict {
    Verdict {
        decode_pass: match targets.min_decode_tokens_per_sec {
            Some(floor) => report.decode_min_tokens_per_sec >= floor,
            None => true,
        },
        ttft_pass: match targets.max_ttft {
            Some(ceil) => report.ttft_p95 < ceil,
            None => true,
        },
        thermal_pass: match (targets.max_peak_temp_celsius, report.peak_temp_celsius) {
            (Some(ceil), Some(peak)) => peak <= ceil,
            _ => true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// f64 comparison epsilon for rate assertions.
    const EPS: f64 = 1e-9;

    /// A fully-gated target set mirroring a hard-acceptance backend (used to
    /// exercise the gating arms of `evaluate`).
    fn gated() -> BenchTargets {
        BenchTargets {
            min_decode_tokens_per_sec: Some(15.0),
            max_ttft: Some(Duration::from_secs(3)),
            max_peak_temp_celsius: Some(70.0),
        }
    }

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
        assert_eq!(
            percentile_duration(&s, 50.0),
            Some(Duration::from_millis(30))
        );
        assert_eq!(
            percentile_duration(&s, 95.0),
            Some(Duration::from_millis(50))
        );
        assert_eq!(
            percentile_duration(&s, 0.0),
            Some(Duration::from_millis(10))
        );
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
            measurement("a", 1000, 30, 2000),
            measurement("b", 2000, 60, 2000),
            measurement("c", 1500, 20, 2000),
        ];
        let report = BenchReport::from_measurements(&measurements, Some(62.0)).unwrap();
        assert_eq!(report.runs, 3);
        assert_eq!(report.degenerate_runs, 0);
        assert!((report.decode_mean_tokens_per_sec - (55.0 / 3.0)).abs() < EPS);
        assert!((report.decode_min_tokens_per_sec - 10.0).abs() < EPS);
        assert_eq!(report.ttft_p50, Duration::from_millis(1500));
        assert_eq!(report.ttft_p95, Duration::from_millis(2000));
        assert_eq!(report.peak_temp_celsius, Some(62.0));
    }

    #[test]
    fn report_excludes_degenerate_runs_from_min() {
        let measurements = vec![
            measurement("a", 1000, 40, 2000),
            measurement("degenerate", 1000, 0, 0),
            measurement("c", 1000, 36, 2000),
        ];
        let report = BenchReport::from_measurements(&measurements, None).unwrap();
        assert_eq!(report.runs, 3);
        assert_eq!(report.degenerate_runs, 1);
        assert!((report.decode_min_tokens_per_sec - 18.0).abs() < EPS);
        assert!((report.decode_mean_tokens_per_sec - 19.0).abs() < EPS);
        assert!(evaluate(&report, &gated()).decode_pass);
    }

    #[test]
    fn report_all_degenerate_yields_zero_and_fails_decode() {
        let measurements = vec![measurement("a", 1000, 0, 0), measurement("b", 1200, 0, 0)];
        let report = BenchReport::from_measurements(&measurements, Some(50.0)).unwrap();
        assert_eq!(report.runs, 2);
        assert_eq!(report.degenerate_runs, 2);
        assert_eq!(report.decode_min_tokens_per_sec, 0.0);
        assert_eq!(report.decode_mean_tokens_per_sec, 0.0);
        assert!(!evaluate(&report, &gated()).decode_pass);
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
        assert!(evaluate(&report, &gated()).all_pass());
    }

    #[test]
    fn verdict_fails_each_criterion_independently() {
        let slow = BenchReport {
            runs: 1,
            degenerate_runs: 0,
            ttft_p50: Duration::from_millis(500),
            ttft_p95: Duration::from_millis(500),
            decode_mean_tokens_per_sec: 14.0,
            decode_min_tokens_per_sec: 14.0,
            peak_temp_celsius: Some(50.0),
        };
        assert!(!evaluate(&slow, &gated()).decode_pass);

        let laggy = BenchReport {
            decode_min_tokens_per_sec: 20.0,
            ttft_p95: Duration::from_millis(3001),
            ..slow.clone()
        };
        assert!(!evaluate(&laggy, &gated()).ttft_pass);

        let hot = BenchReport {
            decode_min_tokens_per_sec: 20.0,
            ttft_p95: Duration::from_millis(1000),
            peak_temp_celsius: Some(70.1),
            ..slow
        };
        assert!(!evaluate(&hot, &gated()).thermal_pass);
    }

    #[test]
    fn thermal_passes_vacuously_without_samples() {
        let report = BenchReport {
            runs: 1,
            degenerate_runs: 0,
            ttft_p50: Duration::from_millis(500),
            ttft_p95: Duration::from_millis(500),
            decode_mean_tokens_per_sec: 20.0,
            decode_min_tokens_per_sec: 20.0,
            peak_temp_celsius: None,
        };
        // Thermal target set but no sample → vacuous pass.
        assert!(evaluate(&report, &gated()).thermal_pass);
    }

    #[test]
    fn decode_and_thermal_boundaries_are_inclusive() {
        let report = BenchReport {
            runs: 1,
            degenerate_runs: 0,
            ttft_p50: Duration::from_millis(500),
            ttft_p95: Duration::from_millis(500),
            decode_mean_tokens_per_sec: 15.0,
            decode_min_tokens_per_sec: 15.0,
            peak_temp_celsius: Some(70.0),
        };
        let v = evaluate(&report, &gated());
        assert!(v.decode_pass);
        assert!(v.thermal_pass);
    }

    #[test]
    fn ttft_boundary_is_exclusive() {
        let at_ceiling = BenchReport {
            runs: 1,
            degenerate_runs: 0,
            ttft_p50: Duration::from_secs(3),
            ttft_p95: Duration::from_secs(3),
            decode_mean_tokens_per_sec: 20.0,
            decode_min_tokens_per_sec: 20.0,
            peak_temp_celsius: Some(50.0),
        };
        assert!(!evaluate(&at_ceiling, &gated()).ttft_pass);

        let just_under = BenchReport {
            ttft_p95: Duration::from_secs(3) - Duration::from_millis(1),
            ..at_ceiling
        };
        assert!(evaluate(&just_under, &gated()).ttft_pass);
    }

    #[test]
    fn none_is_measurement_only() {
        let t = BenchTargets::none();
        assert!(t.is_measurement_only());
        assert_eq!(BenchTargets::default(), t);
    }

    #[test]
    fn measurement_only_passes_every_criterion_vacuously() {
        // A report that would FAIL a gated run passes when nothing is gated.
        let report = BenchReport {
            runs: 1,
            degenerate_runs: 0,
            ttft_p50: Duration::from_secs(10),
            ttft_p95: Duration::from_secs(10),
            decode_mean_tokens_per_sec: 1.0,
            decode_min_tokens_per_sec: 1.0,
            peak_temp_celsius: Some(99.0),
        };
        assert!(evaluate(&report, &BenchTargets::none()).all_pass());
    }

    #[test]
    fn partial_target_gates_only_that_criterion() {
        let report = BenchReport {
            runs: 1,
            degenerate_runs: 0,
            ttft_p50: Duration::from_secs(10),
            ttft_p95: Duration::from_secs(10),
            decode_mean_tokens_per_sec: 1.0,
            decode_min_tokens_per_sec: 1.0,
            peak_temp_celsius: Some(99.0),
        };
        let targets = BenchTargets {
            min_decode_tokens_per_sec: Some(15.0),
            max_ttft: None,
            max_peak_temp_celsius: None,
        };
        let v = evaluate(&report, &targets);
        assert!(!v.decode_pass);
        assert!(v.ttft_pass);
        assert!(v.thermal_pass);
    }
}
