//! Timing metrics and pass/fail evaluation for the benchmark harness.
//!
//! Pure: callers feed in per-prompt [`PromptMeasurement`]s plus an optional
//! peak thermal reading and get back an aggregate [`BenchReport`] and a
//! [`Verdict`] against caller-supplied [`BenchTargets`]. No clock, no device,
//! no I/O — so the aggregation logic is fully unit-tested on host. Shared by
//! every inference backend's benchmark; backend-specific acceptance targets
//! live next to that backend (e.g. `qnn::bench::qnn_targets`).

use std::time::{Duration, Instant};

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
        decode_tokens_per_sec(self.decode_tokens, self.decode_duration)
    }
}

/// Steady-state decode rate in tokens per second. Returns `0.0` when no decode
/// time elapsed (a degenerate single-token or zero-token run) so callers never
/// divide by zero. The single source of truth for the rate formula shared by
/// [`PromptMeasurement`] and [`StreamTiming`].
pub fn decode_tokens_per_sec(decode_tokens: usize, decode_duration: Duration) -> f64 {
    let secs = decode_duration.as_secs_f64();
    if secs <= 0.0 {
        return 0.0;
    }
    decode_tokens as f64 / secs
}

/// Label-less timing produced by [`StreamTimer`]: the same three quantities
/// [`PromptMeasurement`] reports, without a prompt label. Used wherever a
/// `generate_stream` call is timed in-flight (e.g. the QNN per-turn metrics
/// log) rather than driven by the benchmark loop.
///
/// **`decode_tokens` counts non-empty *chunks*, not tokens.** For the QNN
/// backend a chunk is one token (its per-token C-ABI callback fires once per
/// generated token), so `decode_tokens_per_sec` is a true tokens/sec there. A
/// backend that batches multiple tokens into a chunk would report a chunk rate
/// instead — interpret the figure per backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamTiming {
    /// Time from issuing `generate_stream` to the first non-empty chunk.
    pub ttft: Duration,
    /// Non-empty chunks received after the first (one token per chunk on the
    /// QNN backend; a chunk rather than a token in general — see the type doc).
    pub decode_tokens: usize,
    /// Wall-clock from first token to the last token.
    pub decode_duration: Duration,
}

impl StreamTiming {
    /// Steady-state decode rate (tokens after the first / decode window).
    pub fn decode_tokens_per_sec(&self) -> f64 {
        decode_tokens_per_sec(self.decode_tokens, self.decode_duration)
    }
}

/// Incremental timer for one `generate_stream` call.
///
/// Mirrors the per-prompt timing the benchmark loop does, but as a small
/// caller-driven state machine so it can also observe a stream *in flight*
/// (the consumer drives the stream; the producer can't). It reads no clock
/// itself — every instant is supplied by the caller — which keeps it pure and
/// host-testable with synthetic instants. The first non-empty chunk is
/// attributed to [`StreamTiming::ttft`] (prefill + first decode); subsequent
/// non-empty chunks count toward `decode_tokens`, and the decode window spans
/// first-token → last-token so the rate is not diluted by prefill latency.
#[derive(Debug)]
pub struct StreamTimer {
    issued: Instant,
    ttft: Option<Duration>,
    first_token_at: Option<Instant>,
    tokens_after_first: usize,
    last_token_at: Instant,
}

impl StreamTimer {
    /// Begin timing at the instant the `generate_stream` call was issued.
    pub fn start(issued: Instant) -> Self {
        Self {
            issued,
            ttft: None,
            first_token_at: None,
            tokens_after_first: 0,
            last_token_at: issued,
        }
    }

    /// Observe one chunk. `nonempty` is whether the chunk carried any text;
    /// `now` is the instant it was received. Empty chunks (e.g. the final
    /// `done` sentinel the stub and some backends emit) do not count toward
    /// decode tokens and do not move the decode clock.
    pub fn observe(&mut self, nonempty: bool, now: Instant) {
        if !nonempty {
            return;
        }
        if self.ttft.is_none() {
            self.ttft = Some(now.duration_since(self.issued));
            self.first_token_at = Some(now);
        } else {
            self.tokens_after_first += 1;
        }
        self.last_token_at = now;
    }

    /// Finish timing into a [`StreamTiming`]. When no non-empty chunk ever
    /// arrived, TTFT falls back to `issued → fallback_now` (matching
    /// `measure_prompt`'s degenerate-stream behaviour) and the decode window
    /// is zero.
    pub fn finish(self, fallback_now: Instant) -> StreamTiming {
        let ttft = self
            .ttft
            .unwrap_or_else(|| fallback_now.duration_since(self.issued));
        let decode_duration = match self.first_token_at {
            Some(first) => self.last_token_at.duration_since(first),
            None => Duration::ZERO,
        };
        StreamTiming {
            ttft,
            decode_tokens: self.tokens_after_first,
            decode_duration,
        }
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
            // Infallible: `ttfts` is non-empty (guarded above), and
            // `percentile_duration` only returns `None` on an empty slice.
            ttft_p50: percentile_duration(&ttfts, PERCENTILE_P50)
                .expect("ttfts is non-empty (measurements guarded above)"),
            ttft_p95: percentile_duration(&ttfts, PERCENTILE_P95)
                .expect("ttfts is non-empty (measurements guarded above)"),
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
mod tests;
