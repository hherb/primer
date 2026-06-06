# llama.cpp Benchmark Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a measurement-first llama.cpp throughput benchmark (`examples/llamacpp_bench.rs`) by extracting the QNN bench's generic timing/aggregation/thermal/run-loop machinery into a shared, always-compiled `primer-inference::bench` module.

**Architecture:** Move the backend-agnostic core (metrics, prompts, thermal, plus a new `run` module holding `measure_prompt` + `format_report`) to a crate-root `bench/` module, host-tested on the default `cargo test`. `BenchTargets` becomes all-`Option` fields so `evaluate` serves both QNN's hard gate (all-`Some`) and llama.cpp's measurement-first probe (all-`None` default, opt-in gating via CLI flags). The QNN bench shrinks to a thin `qnn/bench.rs` holding only its acceptance-target constants.

**Tech Stack:** Rust (edition 2024, toolchain 1.88), `primer-core` traits, `futures`/`tokio` streaming, `clap` (examples, dev-dep), `llama-cpp-2` (behind the non-default `llamacpp` feature).

**All commands run from `/Users/hherb/src/primer/src` using `~/.cargo/bin/cargo +1.88`.**

---

## File Structure

- **Create** `crates/primer-inference/src/bench/mod.rs` — shared run-shape consts + re-exports.
- **Create** `crates/primer-inference/src/bench/metrics.rs` — `PromptMeasurement`, `BenchReport`, `percentile_duration`, `Verdict`, `evaluate`, `Option`-field `BenchTargets`.
- **Create** `crates/primer-inference/src/bench/prompts.rs` — copy of the QNN prompt loader (verbatim).
- **Create** `crates/primer-inference/src/bench/thermal.rs` — copy of the QNN thermal helpers (one doctest path edit).
- **Create** `crates/primer-inference/src/bench/run.rs` — `measure_prompt` + `format_report` (new; extracted from the QNN example).
- **Modify** `crates/primer-inference/src/lib.rs` — add `pub mod bench;`.
- **Create** `crates/primer-inference/src/qnn/bench.rs` — QNN acceptance targets only (replaces the `qnn/bench/` dir).
- **Delete** `crates/primer-inference/src/qnn/bench/{mod,metrics,prompts,thermal}.rs`.
- **Modify** `crates/primer-inference/examples/qnn_bench.rs` — import from `bench`, use shared `measure_prompt`/`format_report`/`qnn_targets`.
- **Create** `crates/primer-inference/examples/llamacpp_bench.rs` — the new probe.
- **Modify** `crates/primer-inference/Cargo.toml` — add the `[[example]]` entry.
- **Modify** `crates/primer-inference/tests/bench_corpus.rs` — drop the `qnn` cfg gate, import from `bench`.
- **Modify** `.github/workflows/ci.yml` — add a `--features llamacpp` clippy drift guard.
- **Modify** `CLAUDE.md`, `README.md`, `ROADMAP.md` — reflect the shipped harness.

---

## Task 1: Shared `bench` module — metrics, prompts, thermal, mod

**Files:**
- Create: `crates/primer-inference/src/bench/metrics.rs`
- Create: `crates/primer-inference/src/bench/prompts.rs`
- Create: `crates/primer-inference/src/bench/thermal.rs`
- Create: `crates/primer-inference/src/bench/mod.rs`
- Modify: `crates/primer-inference/src/lib.rs`

- [ ] **Step 1: Copy the two verbatim submodules**

```bash
cd /Users/hherb/src/primer/src
mkdir -p crates/primer-inference/src/bench
cp crates/primer-inference/src/qnn/bench/prompts.rs crates/primer-inference/src/bench/prompts.rs
cp crates/primer-inference/src/qnn/bench/thermal.rs crates/primer-inference/src/bench/thermal.rs
```

- [ ] **Step 2: Fix the thermal doctest path in the copy**

The copied `thermal.rs` carries a doctest importing the old QNN path. Because the new `bench` module is always-compiled, this doctest now runs on the default `cargo test`, so the path must be correct.

In `crates/primer-inference/src/bench/thermal.rs`, change the doctest import line:

```rust
/// # use primer_inference::qnn::bench::thermal::parse_thermal_millidegrees;
```
to:
```rust
/// # use primer_inference::bench::thermal::parse_thermal_millidegrees;
```

(`prompts.rs` has no doctest and needs no edit.)

- [ ] **Step 3: Write `bench/metrics.rs` (Option-field `BenchTargets` + Option-aware `evaluate`)**

Write `crates/primer-inference/src/bench/metrics.rs` with this complete content:

```rust
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
        assert_eq!(percentile_duration(&s, 50.0), Some(Duration::from_millis(30)));
        assert_eq!(percentile_duration(&s, 95.0), Some(Duration::from_millis(50)));
        assert_eq!(percentile_duration(&s, 0.0), Some(Duration::from_millis(10)));
        assert_eq!(percentile_duration(&s, 100.0), Some(Duration::from_millis(50)));
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
        assert_eq!(report.degenerate_runs, 2);
        assert_eq!(report.decode_min_tokens_per_sec, 0.0);
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
```

- [ ] **Step 4: Write `bench/mod.rs` (consts + re-exports; `run` declared but written in Task 2)**

Write `crates/primer-inference/src/bench/mod.rs` with this complete content:

```rust
//! Backend-agnostic benchmark harness: prompt-corpus loading, timing
//! aggregation + pass/fail evaluation, thermal sampling, and the shared
//! run-loop helpers the device examples use.
//!
//! Everything here is pure / generic over `InferenceBackend`, so it is
//! exercised by the default `cargo test` even though the actual device runs
//! live in feature-gated example binaries. Backend-specific acceptance
//! targets live next to their backend (e.g. `qnn::bench::qnn_targets`); this
//! module is target-neutral.

use std::time::Duration;

pub mod metrics;
pub mod prompts;
pub mod run;
pub mod thermal;

pub use metrics::{
    BenchReport, BenchTargets, PERCENTILE_P50, PERCENTILE_P95, PromptMeasurement, Verdict, evaluate,
    percentile_duration,
};
pub use prompts::{
    BenchPrompt, BenchPromptError, DEFAULT_BENCH_SYSTEM_PROMPT, load_bench_prompts,
    parse_bench_prompts,
};
pub use run::{format_report, measure_prompt};
pub use thermal::{
    ThermalSample, parse_thermal_millidegrees, peak_temp_celsius, read_thermal_zones, thermal_csv,
};

/// Default wall-clock duration the benchmark loops prompts for, in seconds.
pub const DEFAULT_DURATION_SECS: u64 = 900;

/// How often the background sampler reads the thermal zones.
pub const THERMAL_SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

/// Base sysfs directory holding the thermal zones on Linux/Android.
pub const THERMAL_SYSFS_DIR: &str = "/sys/class/thermal";
/// Prefix of a thermal-zone subdirectory under [`THERMAL_SYSFS_DIR`].
pub const THERMAL_ZONE_PREFIX: &str = "thermal_zone";
/// Filename inside each zone directory holding the millidegree reading.
pub const THERMAL_TEMP_FILE: &str = "temp";

/// Default corpus path, relative to the workspace `src/` directory.
pub const DEFAULT_PROMPTS_PATH: &str = "../data/bench/socratic_prompts.jsonl";

/// Token cap per benchmark prompt.
pub const BENCH_MAX_TOKENS: u32 = 128;
```

- [ ] **Step 5: Create a temporary stub `bench/run.rs` so Task 1 compiles**

Task 2 fills `run.rs` in properly (TDD). For Task 1 to compile (`mod.rs` declares `pub mod run;` and re-exports `format_report`/`measure_prompt`), write a minimal placeholder now — it will be overwritten in Task 2:

```rust
//! Placeholder — filled in Task 2.
```

This placeholder has no `format_report`/`measure_prompt`, so **remove the `pub use run::{...}` line from `mod.rs` for Task 1 only**, then restore it in Task 2. Concretely: in `bench/mod.rs` Step 4, comment the re-export:

```rust
// pub use run::{format_report, measure_prompt}; // restored in Task 2
```

- [ ] **Step 6: Register the module in `lib.rs`**

In `crates/primer-inference/src/lib.rs`, add `pub mod bench;` immediately before `pub mod cloud;`:

```rust
pub mod bench;
pub mod cloud;
```

- [ ] **Step 7: Run the shared-module tests (default features)**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-inference --lib bench::`
Expected: PASS — the new `bench::metrics` and `bench::thermal` tests (incl. the thermal doctest) run on the default build.

Also run the thermal doctest explicitly:
Run: `~/.cargo/bin/cargo +1.88 test -p primer-inference --doc`
Expected: PASS — `bench::thermal::parse_thermal_millidegrees` doctest passes.

- [ ] **Step 8: Commit**

```bash
cd /Users/hherb/src/primer/src
git add crates/primer-inference/src/bench/ crates/primer-inference/src/lib.rs
git commit -m "feat(inference): shared bench core (metrics/prompts/thermal), Option-field BenchTargets"
```

---

## Task 2: Shared `bench::run` — `measure_prompt` + `format_report` (TDD)

**Files:**
- Modify: `crates/primer-inference/src/bench/run.rs` (replace the Task 1 placeholder)
- Modify: `crates/primer-inference/src/bench/mod.rs` (restore the `run` re-export)

- [ ] **Step 1: Write the failing tests first**

Replace `crates/primer-inference/src/bench/run.rs` with the function stubs + tests (implementation bodies come in Step 3):

```rust
//! Run-loop helpers shared by every backend's benchmark example.
//!
//! [`measure_prompt`] drives one `generate_stream` call and times it;
//! [`format_report`] renders an aggregate report as text. Both are generic
//! over [`InferenceBackend`] / plain data so the device examples
//! (`examples/qnn_bench.rs`, `examples/llamacpp_bench.rs`) carry only argument
//! parsing + backend construction, and the timing/format logic is unit-tested
//! here on the default `cargo test`.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use futures::StreamExt;
use primer_core::error::Result;
use primer_core::inference::{GenerationParams, InferenceBackend};

use super::metrics::{BenchReport, BenchTargets, PromptMeasurement, Verdict};
use super::prompts::BenchPrompt;

/// Measure one prompt: TTFT (issue → first non-empty chunk) and steady-state
/// decode rate (first token → last token). Generic over the backend. A
/// mid-stream error propagates.
pub async fn measure_prompt(
    backend: &dyn InferenceBackend,
    prompt: &BenchPrompt,
    params: &GenerationParams,
) -> Result<PromptMeasurement> {
    todo!()
}

/// Render an aggregate report as multi-line text. Pure (returns a `String` the
/// caller prints) so it is testable without capturing stdout. When
/// `targets.is_measurement_only()` the verdict lines are replaced by a
/// "measurement only" note.
pub fn format_report(
    title: &str,
    report: &BenchReport,
    targets: &BenchTargets,
    verdict: &Verdict,
) -> String {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::metrics::evaluate;
    use async_trait::async_trait;
    use futures::channel::mpsc;
    use primer_core::inference::{Prompt, TokenChunk, TokenStream};

    /// A backend that emits `chunks` as non-empty token chunks, sleeping
    /// `gap` before each chunk after the first, then a final empty done chunk.
    struct TimedMock {
        chunks: Vec<String>,
        gap: Duration,
    }

    #[async_trait]
    impl InferenceBackend for TimedMock {
        fn name(&self) -> &str {
            "timed-mock"
        }
        async fn is_available(&self) -> bool {
            true
        }
        async fn generate_stream(
            &self,
            _prompt: &Prompt,
            _params: &GenerationParams,
        ) -> Result<TokenStream> {
            let chunks = self.chunks.clone();
            let gap = self.gap;
            let (tx, rx) = mpsc::unbounded::<Result<TokenChunk>>();
            tokio::spawn(async move {
                for (i, c) in chunks.iter().enumerate() {
                    if i > 0 {
                        tokio::time::sleep(gap).await;
                    }
                    let _ = tx.unbounded_send(Ok(TokenChunk {
                        text: c.clone(),
                        done: false,
                    }));
                }
                let _ = tx.unbounded_send(Ok(TokenChunk {
                    text: String::new(),
                    done: true,
                }));
            });
            Ok(Box::pin(rx))
        }
    }

    fn bench_prompt() -> BenchPrompt {
        BenchPrompt {
            label: "probe".to_string(),
            prompt: Prompt {
                system: "be socratic".to_string(),
                messages: vec![],
            },
        }
    }

    #[tokio::test]
    async fn measure_prompt_times_multi_chunk_stream() {
        let mock = TimedMock {
            chunks: vec!["a".into(), "b".into(), "c".into(), "d".into()],
            gap: Duration::from_millis(20),
        };
        let m = measure_prompt(&mock, &bench_prompt(), &GenerationParams::default())
            .await
            .unwrap();
        assert_eq!(m.label, "probe");
        // 4 chunks → 3 after the first.
        assert_eq!(m.decode_tokens, 3);
        // Decode window spans 3 gaps (~60ms); allow loose floor for scheduling.
        assert!(
            m.decode_duration >= Duration::from_millis(40),
            "decode_duration too short: {:?}",
            m.decode_duration
        );
        // First chunk is immediate, so TTFT is well under the decode window.
        assert!(m.ttft < m.decode_duration);
    }

    #[tokio::test]
    async fn measure_prompt_single_chunk_is_degenerate() {
        let mock = TimedMock {
            chunks: vec!["only".into()],
            gap: Duration::from_millis(20),
        };
        let m = measure_prompt(&mock, &bench_prompt(), &GenerationParams::default())
            .await
            .unwrap();
        assert_eq!(m.decode_tokens, 0);
        assert_eq!(m.decode_duration, Duration::ZERO);
        assert_eq!(m.decode_tokens_per_sec(), 0.0);
    }

    fn report() -> BenchReport {
        BenchReport {
            runs: 2,
            degenerate_runs: 0,
            ttft_p50: Duration::from_millis(800),
            ttft_p95: Duration::from_millis(1200),
            decode_mean_tokens_per_sec: 12.0,
            decode_min_tokens_per_sec: 10.0,
            peak_temp_celsius: Some(55.0),
        }
    }

    #[test]
    fn format_report_gated_shows_pass_fail() {
        let targets = BenchTargets {
            min_decode_tokens_per_sec: Some(15.0),
            max_ttft: Some(Duration::from_secs(3)),
            max_peak_temp_celsius: Some(70.0),
        };
        let verdict = evaluate(&report(), &targets);
        let text = format_report("llama.cpp benchmark", &report(), &targets, &verdict);
        assert!(text.contains("llama.cpp benchmark"));
        // decode min 10 < floor 15 → FAIL line present; overall present.
        assert!(text.contains("FAIL"));
        assert!(text.contains("overall:"));
        assert!(!text.contains("measurement only"));
    }

    #[test]
    fn format_report_measurement_only_has_note() {
        let targets = BenchTargets::none();
        let verdict = evaluate(&report(), &targets);
        let text = format_report("llama.cpp benchmark", &report(), &targets, &verdict);
        assert!(text.contains("measurement only"));
        assert!(!text.contains("overall:"));
        assert!(!text.contains("PASS"));
        assert!(!text.contains("FAIL"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-inference --lib bench::run`
Expected: FAIL / panic — `measure_prompt` and `format_report` are `todo!()`.

- [ ] **Step 3: Implement `measure_prompt` and `format_report`**

Replace the two `todo!()` bodies in `crates/primer-inference/src/bench/run.rs`:

```rust
pub async fn measure_prompt(
    backend: &dyn InferenceBackend,
    prompt: &BenchPrompt,
    params: &GenerationParams,
) -> Result<PromptMeasurement> {
    let issued = Instant::now();
    let mut stream = backend.generate_stream(&prompt.prompt, params).await?;

    let mut ttft: Option<Duration> = None;
    let mut first_token_at: Option<Instant> = None;
    let mut tokens_after_first = 0usize;
    let mut last_token_at = issued;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let now = Instant::now();
        if !chunk.text.is_empty() {
            if ttft.is_none() {
                ttft = Some(now.duration_since(issued));
                first_token_at = Some(now);
            } else {
                tokens_after_first += 1;
            }
            last_token_at = now;
        }
        if chunk.done {
            break;
        }
    }

    let ttft = ttft.unwrap_or_else(|| issued.elapsed());
    let decode_duration = match first_token_at {
        Some(first) => last_token_at.duration_since(first),
        None => Duration::ZERO,
    };

    Ok(PromptMeasurement {
        label: prompt.label.clone(),
        ttft,
        decode_tokens: tokens_after_first,
        decode_duration,
    })
}

pub fn format_report(
    title: &str,
    report: &BenchReport,
    targets: &BenchTargets,
    verdict: &Verdict,
) -> String {
    let pf = |ok: bool| if ok { "PASS" } else { "FAIL" };
    let mut out = String::new();

    let _ = writeln!(
        out,
        "=== {title} report ({} runs, {} degenerate) ===",
        report.runs, report.degenerate_runs
    );

    let _ = write!(
        out,
        "TTFT  p50={:.0}ms  p95={:.0}ms",
        report.ttft_p50.as_secs_f64() * 1000.0,
        report.ttft_p95.as_secs_f64() * 1000.0,
    );
    match targets.max_ttft {
        Some(t) => {
            let _ = writeln!(
                out,
                "   (target p95 < {:.0}ms)  [{}]",
                t.as_secs_f64() * 1000.0,
                pf(verdict.ttft_pass)
            );
        }
        None => {
            let _ = writeln!(out);
        }
    }

    let _ = write!(
        out,
        "decode mean={:.2} tok/s  min={:.2} tok/s",
        report.decode_mean_tokens_per_sec, report.decode_min_tokens_per_sec,
    );
    match targets.min_decode_tokens_per_sec {
        Some(t) => {
            let _ = writeln!(out, "   (target min >= {t:.2})  [{}]", pf(verdict.decode_pass));
        }
        None => {
            let _ = writeln!(out);
        }
    }

    match report.peak_temp_celsius {
        Some(peak) => {
            let _ = write!(out, "peak temp={peak:.1}°C");
            match targets.max_peak_temp_celsius {
                Some(t) => {
                    let _ = writeln!(
                        out,
                        "   (target <= {t:.1}°C)  [{}]",
                        pf(verdict.thermal_pass)
                    );
                }
                None => {
                    let _ = writeln!(out);
                }
            }
        }
        None => {
            let _ = writeln!(out, "peak temp=n/a (no thermal samples)");
        }
    }

    if targets.is_measurement_only() {
        let _ = writeln!(out, "measurement only (no acceptance gate)");
    } else {
        let _ = writeln!(out, "overall: [{}]", pf(verdict.all_pass()));
    }
    out
}
```

- [ ] **Step 4: Restore the `run` re-export in `mod.rs`**

In `crates/primer-inference/src/bench/mod.rs`, restore the commented line from Task 1 Step 5 to:

```rust
pub use run::{format_report, measure_prompt};
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-inference --lib bench::run`
Expected: PASS — all four `run` tests green.

- [ ] **Step 6: Commit**

```bash
git add crates/primer-inference/src/bench/run.rs crates/primer-inference/src/bench/mod.rs
git commit -m "feat(inference): shared bench run-loop (measure_prompt + format_report)"
```

---

## Task 3: Rewire the QNN bench onto the shared core

**Files:**
- Delete: `crates/primer-inference/src/qnn/bench/{mod,metrics,prompts,thermal}.rs`
- Create: `crates/primer-inference/src/qnn/bench.rs`
- Modify: `crates/primer-inference/examples/qnn_bench.rs`

- [ ] **Step 1: Replace the `qnn/bench/` directory with a single targets file**

```bash
cd /Users/hherb/src/primer/src
rm -r crates/primer-inference/src/qnn/bench
```

Then write `crates/primer-inference/src/qnn/bench.rs`:

```rust
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
```

(`crates/primer-inference/src/qnn/mod.rs` already has `pub mod bench;`, which now resolves to `bench.rs`. It does not re-export any moved item, so no edit is needed there.)

- [ ] **Step 2: Update the QNN example imports**

In `crates/primer-inference/examples/qnn_bench.rs`, replace the import block (the `use primer_inference::qnn::bench::{...};` group and the `QnnBackend` line) with:

```rust
use primer_inference::QnnBackend;
use primer_inference::bench::{
    BENCH_MAX_TOKENS, BenchReport, BenchTargets, DEFAULT_BENCH_SYSTEM_PROMPT,
    DEFAULT_DURATION_SECS, DEFAULT_PROMPTS_PATH, PromptMeasurement, THERMAL_SAMPLE_INTERVAL,
    THERMAL_SYSFS_DIR, ThermalSample, evaluate, format_report, load_bench_prompts, measure_prompt,
    peak_temp_celsius, read_thermal_zones, thermal_csv,
};
use primer_inference::qnn::bench::qnn_targets;
```

- [ ] **Step 3: Rebase the example's `targets()` onto `qnn_targets()`**

In `crates/primer-inference/examples/qnn_bench.rs`, replace the `fn targets(&self) -> BenchTargets { ... }` body with:

```rust
    fn targets(&self) -> BenchTargets {
        let mut t = qnn_targets();
        if let Some(v) = self.min_decode_tps {
            t.min_decode_tokens_per_sec = Some(v);
        }
        if let Some(ms) = self.max_ttft_ms {
            t.max_ttft = Some(Duration::from_millis(ms));
        }
        if let Some(c) = self.max_peak_temp_c {
            t.max_peak_temp_celsius = Some(c);
        }
        t
    }
```

- [ ] **Step 4: Delete the example-local `measure_prompt` and `print_report`; call the shared helpers**

In `crates/primer-inference/examples/qnn_bench.rs`:
1. Delete the entire `async fn measure_prompt(...) -> Result<PromptMeasurement, ...> { ... }` function (the shared `bench::measure_prompt` replaces it; the existing call site `measure_prompt(&backend, bp, &params).await` now resolves to the import).
2. Delete the entire `fn print_report(report, targets, verdict) { ... }` function.
3. Replace its call site `print_report(&report, &targets, &verdict);` with:

```rust
    println!("{}", format_report("QNN benchmark", &report, &targets, &verdict));
```

- [ ] **Step 5: Verify the QNN feature build, tests, clippy, and example**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-inference --features qnn`
Expected: PASS — incl. `qnn::bench::tests::qnn_targets_are_fully_gated` and the (now shared) `bench::*` tests.

Run: `~/.cargo/bin/cargo +1.88 build --example qnn_bench --features qnn`
Expected: compiles clean.

Run: `~/.cargo/bin/cargo +1.88 clippy -p primer-inference --features qnn --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/primer-inference/src/qnn/ crates/primer-inference/examples/qnn_bench.rs
git commit -m "refactor(inference): QNN bench reuses shared core; qnn/bench is targets-only"
```

---

## Task 4: Move the corpus guard onto the default test path

**Files:**
- Modify: `crates/primer-inference/tests/bench_corpus.rs`

- [ ] **Step 1: Rewrite the corpus guard to use the shared loader, ungated**

Replace `crates/primer-inference/tests/bench_corpus.rs` with this complete content:

```rust
//! Regression guard for the shipped benchmark corpus.
//!
//! The benchmark examples are device/owner-gated, but the prompt corpus they
//! consume is plain data we validate on any host: it must parse, meet the
//! 30-prompt floor, and carry unique labels. The corpus is shared by the QNN
//! and llama.cpp benches, so this guard rides the DEFAULT `cargo test` via the
//! always-compiled [`primer_inference::bench`] loader.

use std::collections::HashSet;
use std::path::PathBuf;

use primer_inference::bench::{DEFAULT_BENCH_SYSTEM_PROMPT, load_bench_prompts};

/// 30 representative dialogue-continuation prompts (Phase 1.2 plan step 1.2.6
/// task 1). The corpus may grow but must not shrink below this floor.
const MIN_CORPUS_PROMPTS: usize = 30;

fn corpus_path() -> PathBuf {
    // CARGO_MANIFEST_DIR = <repo>/src/crates/primer-inference.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../data/bench/socratic_prompts.jsonl")
}

#[test]
fn shipped_corpus_parses_and_meets_floor() {
    let path = corpus_path();
    let prompts = load_bench_prompts(&path, DEFAULT_BENCH_SYSTEM_PROMPT)
        .unwrap_or_else(|e| panic!("shipped corpus at {} failed to load: {e}", path.display()));
    assert!(
        prompts.len() >= MIN_CORPUS_PROMPTS,
        "expected >= {MIN_CORPUS_PROMPTS} prompts, got {}",
        prompts.len()
    );
}

#[test]
fn shipped_corpus_has_unique_labels() {
    let prompts = load_bench_prompts(&corpus_path(), DEFAULT_BENCH_SYSTEM_PROMPT).unwrap();
    let mut seen = HashSet::new();
    for p in &prompts {
        assert!(seen.insert(p.label.clone()), "duplicate label: {}", p.label);
    }
}
```

- [ ] **Step 2: Run the corpus guard on the default build**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-inference --test bench_corpus`
Expected: PASS — both tests run without the `qnn` feature.

- [ ] **Step 3: Commit**

```bash
git add crates/primer-inference/tests/bench_corpus.rs
git commit -m "test(inference): run shipped-corpus guard on the default test path"
```

---

## Task 5: The `llamacpp_bench` example

**Files:**
- Create: `crates/primer-inference/examples/llamacpp_bench.rs`
- Modify: `crates/primer-inference/Cargo.toml`

- [ ] **Step 1: Add the `[[example]]` entry**

In `crates/primer-inference/Cargo.toml`, after the existing `[[example]] name = "qnn_bench"` block, add:

```toml
# llama.cpp throughput benchmark (Phase 1.1 bullet b). Requires the
# `llamacpp` feature so `RealLlamaEngine` is in the dep graph; the GPU
# passthrough features (`llamacpp-metal/-cuda/-vulkan`) activate it
# transitively. Measurement-first — a flagless run prints numbers and exits 0.
[[example]]
name = "llamacpp_bench"
required-features = ["llamacpp"]
```

- [ ] **Step 2: Write the example**

Write `crates/primer-inference/examples/llamacpp_bench.rs` with this complete content:

```rust
//! llama.cpp backend throughput benchmark (Phase 1.1 bullet b).
//!
//! Loads a local GGUF, loops a Socratic dialogue-continuation corpus for a
//! wall-clock window, and reports p50/p95 TTFT + mean/min decode tok/s (and
//! peak temperature where `/sys/class/thermal` exposes it). Measurement-first:
//! a plain run prints numbers and exits 0. Pass any of `--min-decode-tps`,
//! `--max-ttft-ms`, `--max-peak-temp-c` to turn it into a pass/fail gate that
//! exits non-zero on failure (so it can gate a CI/regression script).
//!
//! ```text
//! cd src
//! ~/.cargo/bin/cargo run --release --example llamacpp_bench \
//!     --features llamacpp-metal -- \
//!     --model ~/models/Qwen3-7B-Q4_K_M.gguf \
//!     --duration-secs 120
//! ```
//!
//! Build with `--features llamacpp` for CPU, or `llamacpp-metal` /
//! `llamacpp-cuda` / `llamacpp-vulkan` for GPU offload.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;
use primer_core::inference::{GenerationParams, InferenceBackend};
use primer_inference::LlamaCppBackend;
use primer_inference::bench::{
    BENCH_MAX_TOKENS, BenchReport, BenchTargets, DEFAULT_BENCH_SYSTEM_PROMPT,
    DEFAULT_DURATION_SECS, DEFAULT_PROMPTS_PATH, THERMAL_SAMPLE_INTERVAL, THERMAL_SYSFS_DIR,
    ThermalSample, evaluate, format_report, load_bench_prompts, measure_prompt, peak_temp_celsius,
    thermal_csv, read_thermal_zones,
};
use primer_inference::llamacpp::engine::RealLlamaEngine;
use primer_inference::llamacpp::params::resolve_gpu_layers;
use tokio::sync::oneshot;

#[derive(Parser, Debug)]
#[command(about = "llama.cpp backend throughput benchmark")]
struct Args {
    /// Path to the GGUF model file.
    #[arg(long)]
    model: PathBuf,

    /// Number of layers to offload to GPU. Negative = all (the llama.cpp
    /// convention); omitted = all when a GPU passthrough feature is compiled,
    /// else CPU-only.
    #[arg(long)]
    gpu_layers: Option<i32>,

    /// Context length override (0 = the model's trained length).
    #[arg(long)]
    n_ctx: Option<u32>,

    /// Path to the JSONL prompt corpus.
    #[arg(long, default_value = DEFAULT_PROMPTS_PATH)]
    prompts: PathBuf,

    /// Wall-clock window to loop prompts for, in seconds.
    #[arg(long, default_value_t = DEFAULT_DURATION_SECS)]
    duration_secs: u64,

    /// Max tokens to decode per prompt.
    #[arg(long, default_value_t = BENCH_MAX_TOKENS)]
    max_tokens: u32,

    /// Optional path to write the thermal-sample CSV.
    #[arg(long)]
    thermal_out: Option<PathBuf>,

    /// Opt-in gate: minimum sustained decode rate (tok/s).
    #[arg(long)]
    min_decode_tps: Option<f64>,

    /// Opt-in gate: maximum TTFT (milliseconds).
    #[arg(long)]
    max_ttft_ms: Option<u64>,

    /// Opt-in gate: maximum peak temperature (°C).
    #[arg(long)]
    max_peak_temp_c: Option<f64>,
}

impl Args {
    /// Acceptance targets from the opt-in gate flags. Absent flags stay
    /// `None`, so a flagless run is a pure measurement.
    fn targets(&self) -> BenchTargets {
        BenchTargets {
            min_decode_tokens_per_sec: self.min_decode_tps,
            max_ttft: self.max_ttft_ms.map(Duration::from_millis),
            max_peak_temp_celsius: self.max_peak_temp_c,
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => {
            eprintln!("benchmark FAILED one or more acceptance targets");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("benchmark error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Returns `Ok(true)` on a passing/measurement-only run, `Ok(false)` when a
/// gated target failed.
async fn run() -> Result<bool, Box<dyn std::error::Error>> {
    let args = Args::parse();
    let prompts = load_bench_prompts(&args.prompts, DEFAULT_BENCH_SYSTEM_PROMPT)?;
    println!(
        "loaded {} prompts from {}",
        prompts.len(),
        args.prompts.display()
    );

    let gpu_layers = resolve_gpu_layers(args.gpu_layers);
    println!(
        "loading GGUF {} (gpu_layers={}, n_ctx={:?})",
        args.model.display(),
        gpu_layers,
        args.n_ctx
    );
    let engine = RealLlamaEngine::new(&args.model, gpu_layers, args.n_ctx)?;
    let backend = LlamaCppBackend::new(Arc::new(engine));
    println!("backend ready: {}", backend.name());

    let started = Instant::now();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let sampler = tokio::spawn(thermal_sampler(started, stop_rx));

    let params = GenerationParams {
        max_tokens: args.max_tokens,
        ..GenerationParams::default()
    };
    let duration = Duration::from_secs(args.duration_secs);

    let mut measurements = Vec::new();
    let mut cycle = 0usize;
    'outer: while started.elapsed() < duration {
        cycle += 1;
        for bp in &prompts {
            if started.elapsed() >= duration {
                break 'outer;
            }
            match measure_prompt(&backend, bp, &params).await {
                Ok(m) => {
                    println!(
                        "[cycle {cycle}] {:<16} ttft={:>6.0}ms decode={:>5} tok in {:>6.0}ms = {:>6.2} tok/s",
                        m.label,
                        m.ttft.as_secs_f64() * 1000.0,
                        m.decode_tokens,
                        m.decode_duration.as_secs_f64() * 1000.0,
                        m.decode_tokens_per_sec(),
                    );
                    measurements.push(m);
                }
                Err(e) => eprintln!("[cycle {cycle}] {} errored: {e}", bp.label),
            }
        }
    }

    let _ = stop_tx.send(());
    let samples = sampler.await.unwrap_or_default();
    let peak = peak_temp_celsius(&samples);

    if let Some(out) = &args.thermal_out {
        std::fs::write(out, thermal_csv(&samples))?;
        println!(
            "wrote {} thermal samples to {}",
            samples.len(),
            out.display()
        );
    }

    let report = match BenchReport::from_measurements(&measurements, peak) {
        Some(r) => r,
        None => {
            eprintln!("no successful prompt runs — nothing to report");
            return Ok(false);
        }
    };
    let targets = args.targets();
    let verdict = evaluate(&report, &targets);
    println!(
        "\n{}",
        format_report("llama.cpp benchmark", &report, &targets, &verdict)
    );
    Ok(verdict.all_pass())
}

/// Background thermal sampler. Ticks every [`THERMAL_SAMPLE_INTERVAL`],
/// reading every `thermal_zone*/temp` under [`THERMAL_SYSFS_DIR`], until the
/// stop signal fires. Returns all collected samples (empty on macOS).
async fn thermal_sampler(started: Instant, mut stop: oneshot::Receiver<()>) -> Vec<ThermalSample> {
    let mut samples = Vec::new();
    let mut ticker = tokio::time::interval(THERMAL_SAMPLE_INTERVAL);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let elapsed = started.elapsed().as_secs_f64();
                samples.extend(read_thermal_zones(Path::new(THERMAL_SYSFS_DIR), elapsed));
            }
            _ = &mut stop => break,
        }
    }
    samples
}
```

- [ ] **Step 3: Build the example (compiles llama.cpp; first build is slow)**

Run: `~/.cargo/bin/cargo +1.88 build --example llamacpp_bench --features llamacpp`
Expected: compiles clean (no GGUF needed to compile). This builds llama.cpp's C++ on first run.

Run: `~/.cargo/bin/cargo +1.88 clippy -p primer-inference --features llamacpp --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/primer-inference/Cargo.toml crates/primer-inference/examples/llamacpp_bench.rs
git commit -m "feat(inference): llamacpp_bench example (Phase 1.1 bullet b, measurement-first)"
```

---

## Task 6: CI drift guard for the `llamacpp` feature

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add a `--features llamacpp` clippy step to the `feature-combos` job**

In `.github/workflows/ci.yml`, in the `feature-combos` job, after the `cargo check --features fastembed` step, add:

```yaml
      - name: cargo clippy --features llamacpp
        # Phase 1.1 drift guard. The embedded llama.cpp backend + its
        # benchmark example (`examples/llamacpp_bench.rs`,
        # required-features = llamacpp) are non-default, so they rot
        # silently. `--all-targets` lints the example too. Compiles
        # llama.cpp (C++) on first run; the rust-cache layer above keeps
        # subsequent runs warm.
        run: cargo clippy -p primer-inference --features llamacpp --all-targets -- -D warnings
```

- [ ] **Step 2: Validate the workflow YAML locally (syntax only)**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('ok')"`
(run from the repo root `/Users/hherb/src/primer`)
Expected: prints `ok`.

- [ ] **Step 3: Commit**

```bash
cd /Users/hherb/src/primer
git add .github/workflows/ci.yml
git commit -m "ci: clippy drift guard for the llamacpp feature + example"
```

---

## Task 7: Documentation

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md`
- Modify: `ROADMAP.md`

- [ ] **Step 1: CLAUDE.md — note the shared bench module + llamacpp_bench**

In `CLAUDE.md`, in the `primer-inference` bullet (the long paragraph describing the crate's backends), append a sentence after the QNN benchmark description:

> **Phase 1.1 ships a llama.cpp throughput benchmark** mirroring the QNN harness. The generic timing/aggregation/thermal/run machinery now lives once in the always-compiled `primer-inference::bench` module (`metrics`, `prompts`, `thermal`, `run::{measure_prompt, format_report}`), host-tested on the default `cargo test`; `qnn::bench` shrinks to a targets-only file (`qnn_targets()`). `BenchTargets` fields are all-`Option` — `evaluate` treats `None` as a vacuous pass, so the same machinery serves QNN's hard gate (all-`Some`) and the llama.cpp probe (measurement-first; `examples/llamacpp_bench.rs`, `required-features = ["llamacpp"]`, opt-in gating via `--min-decode-tps`/`--max-ttft-ms`/`--max-peak-temp-c`). The shared corpus guard (`tests/bench_corpus.rs`) now runs on the default test path. Actual llama.cpp tok/s/TTFT numbers remain device-unverified (owner-gated; no GGUF is downloaded by any autonomous run).

- [ ] **Step 2: README.md — reflect Phase 1.1 status**

In `README.md`, find the status section describing llama.cpp (Phase 1.1 / "Local llama.cpp inference has landed"). Append: "A measurement-first throughput benchmark (`cargo run --example llamacpp_bench --features llamacpp -- --model <gguf>`) reports p50/p95 TTFT + mean/min tok/s; device numbers are still pending."

- [ ] **Step 3: ROADMAP.md — mark the bench-harness deliverable**

In `ROADMAP.md`, under Phase 1.1 bullet (b) (benchmarking), add a sub-note that the **harness** has shipped (`examples/llamacpp_bench.rs` + shared `bench` module) and what remains is running it on MacBook (Metal) / DGX (CUDA) / RedMagic (Vulkan) to collect the actual tok/s + TTFT numbers.

- [ ] **Step 4: Commit**

```bash
cd /Users/hherb/src/primer
git add CLAUDE.md README.md ROADMAP.md
git commit -m "docs: shared bench module + llamacpp_bench harness (Phase 1.1 bullet b)"
```

---

## Final verification (run before opening the PR)

- [ ] **Full default workspace verify**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo +1.88 test --workspace --no-fail-fast
```
Expected: fmt clean, clippy clean, all tests pass (incl. the new `bench::*` + corpus guard on the default path).

- [ ] **Feature-gated verify (QNN + llamacpp)**

```bash
~/.cargo/bin/cargo +1.88 test -p primer-inference --features qnn
~/.cargo/bin/cargo +1.88 clippy -p primer-inference --features qnn --all-targets -- -D warnings
~/.cargo/bin/cargo +1.88 build --example qnn_bench --features qnn
~/.cargo/bin/cargo +1.88 clippy -p primer-inference --features llamacpp --all-targets -- -D warnings
~/.cargo/bin/cargo +1.88 build --example llamacpp_bench --features llamacpp
```
Expected: all green.

---

## Self-review notes (author)

- **Spec coverage:** Component 1 → Tasks 1–2; Component 2 (QNN refactor) → Task 3; Component 3 (example) → Task 5; Component 4 (corpus guard) → Task 4; Component 5 (CI) → Task 6; docs → Task 7. All covered.
- **Option-target redesign** is in Task 1 (metrics.rs) and consumed by Tasks 3/5.
- **Type/name consistency:** `measure_prompt`, `format_report`, `qnn_targets`, `BenchTargets` (Option fields), `BenchPrompt`, `BenchReport`, `Verdict`, `ThermalSample` used identically across tasks.
- **Green-at-every-commit:** Task 1 keeps the `run` re-export commented (placeholder) so it compiles before Task 2 fills `run.rs`; the QNN path stays green because Task 1 only *adds* the shared module (qnn/bench still intact) and Task 3 does the swap atomically.
