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
fn stream_timer_times_multi_chunk_stream() {
    // Four non-empty chunks at 0/100/200/300ms after an issue at t0+50ms:
    // ttft = 50ms (first chunk), decode window = 300ms over 3 later tokens.
    let issued = Instant::now();
    let mut timer = StreamTimer::start(issued);
    timer.observe(true, issued + Duration::from_millis(50));
    timer.observe(true, issued + Duration::from_millis(150));
    timer.observe(true, issued + Duration::from_millis(250));
    timer.observe(true, issued + Duration::from_millis(350));
    let timing = timer.finish(issued + Duration::from_millis(400));
    assert_eq!(timing.ttft, Duration::from_millis(50));
    assert_eq!(timing.decode_tokens, 3);
    assert_eq!(timing.decode_duration, Duration::from_millis(300));
    assert!((timing.decode_tokens_per_sec() - 10.0).abs() < EPS);
}

#[test]
fn stream_timer_ignores_empty_chunks() {
    // A trailing empty done-sentinel must not count as a token or move
    // the decode clock past the last real token.
    let issued = Instant::now();
    let mut timer = StreamTimer::start(issued);
    timer.observe(true, issued + Duration::from_millis(20));
    timer.observe(true, issued + Duration::from_millis(120));
    timer.observe(false, issued + Duration::from_millis(500)); // empty done
    let timing = timer.finish(issued + Duration::from_millis(500));
    assert_eq!(timing.ttft, Duration::from_millis(20));
    assert_eq!(timing.decode_tokens, 1);
    assert_eq!(timing.decode_duration, Duration::from_millis(100));
}

#[test]
fn stream_timer_degenerate_stream_falls_back_to_elapsed_ttft() {
    // No non-empty chunk ever arrives: ttft falls back to issued →
    // fallback_now, decode window is zero (a degenerate run).
    let issued = Instant::now();
    let mut timer = StreamTimer::start(issued);
    timer.observe(false, issued + Duration::from_millis(10));
    let timing = timer.finish(issued + Duration::from_millis(80));
    assert_eq!(timing.ttft, Duration::from_millis(80));
    assert_eq!(timing.decode_tokens, 0);
    assert_eq!(timing.decode_duration, Duration::ZERO);
    assert_eq!(timing.decode_tokens_per_sec(), 0.0);
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
