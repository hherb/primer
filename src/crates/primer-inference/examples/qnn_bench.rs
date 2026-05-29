//! QNN backend throughput + thermal benchmark (Phase 1.2 step 1.2.6).
//!
//! This is the **device test** for the Qualcomm NPU backend: it constructs
//! a [`QnnBackend`] against a real Genie bundle, loops a corpus of
//! Socratic dialogue-continuation prompts for a fixed wall-clock window,
//! and measures time-to-first-token and steady-state decode rate per
//! prompt while a background task samples `/sys/class/thermal` every two
//! seconds. At the end it prints p50/p95 TTFT, mean/min decode tok/s, and
//! peak temperature, then evaluates pass/fail against the Phase 1.2
//! acceptance targets (≥ 15 tok/s sustained decode, < 3 s TTFT, ≤ 70 °C).
//!
//! All the maths lives in pure, host-tested functions under
//! [`primer_inference::qnn::bench`]; this file is just I/O orchestration
//! (clap, the backend round-trip, the sysfs reads, the CSV write).
//!
//! ```text
//! # On the RedMagic 11 Pro via Termux:
//! cd ~/primer/src
//! ~/.cargo/bin/cargo run --release --example qnn_bench --features qnn -- \
//!     --bundle-dir ~/primer-bundles/qwen3-4b \
//!     --duration-secs 900 \
//!     --thermal-out ~/storage/shared/primer-thermal.csv
//! ```
//!
//! Exits non-zero if the run fails any acceptance target, so it can gate a
//! device-side script.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::Parser;
use futures::StreamExt;
use primer_core::inference::{GenerationParams, InferenceBackend};
use primer_inference::QnnBackend;
use primer_inference::qnn::bench::{
    BENCH_MAX_TOKENS, BenchReport, BenchTargets, DEFAULT_BENCH_SYSTEM_PROMPT,
    DEFAULT_DURATION_SECS, DEFAULT_PROMPTS_PATH, PromptMeasurement, THERMAL_SAMPLE_INTERVAL,
    THERMAL_SYSFS_DIR, THERMAL_TEMP_FILE, THERMAL_ZONE_PREFIX, ThermalSample, evaluate,
    load_bench_prompts, parse_thermal_millidegrees, peak_temp_celsius, thermal_csv,
};
use tokio::sync::oneshot;

/// Conventional QAIRT lib subdirectory under the bundle's parent, matching
/// the AI Hub apps asset layout (mirrors the CLI/GUI default).
const QAIRT_LIB_SUBPATH: &str = "qairt/lib/aarch64-android";

#[derive(Parser, Debug)]
#[command(about = "QNN backend throughput + thermal benchmark")]
struct Args {
    /// Path to the Genie `genie_bundle` directory (contains
    /// `genie_config.json` and, ideally, `primer-meta.json`).
    #[arg(long)]
    bundle_dir: PathBuf,

    /// Directory holding `libGenie.so`. Defaults to
    /// `<bundle-dir>/../qairt/lib/aarch64-android/`.
    #[arg(long)]
    qairt_lib_dir: Option<PathBuf>,

    /// Path to the JSONL prompt corpus.
    #[arg(long, default_value = DEFAULT_PROMPTS_PATH)]
    prompts: PathBuf,

    /// Wall-clock window to loop prompts for, in seconds.
    #[arg(long, default_value_t = DEFAULT_DURATION_SECS)]
    duration_secs: u64,

    /// Optional path to write the thermal-sample CSV.
    #[arg(long)]
    thermal_out: Option<PathBuf>,

    /// Override the sustained-decode floor (tok/s).
    #[arg(long)]
    min_decode_tps: Option<f64>,

    /// Override the TTFT ceiling (milliseconds).
    #[arg(long)]
    max_ttft_ms: Option<u64>,

    /// Override the peak-temperature ceiling (°C).
    #[arg(long)]
    max_peak_temp_c: Option<f64>,
}

impl Args {
    /// Resolve the QAIRT lib dir, defaulting to the conventional layout.
    fn resolved_qairt_lib_dir(&self) -> PathBuf {
        self.qairt_lib_dir.clone().unwrap_or_else(|| {
            self.bundle_dir
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(QAIRT_LIB_SUBPATH)
        })
    }

    /// Acceptance targets with any CLI overrides applied.
    fn targets(&self) -> BenchTargets {
        let mut t = BenchTargets::default();
        if let Some(v) = self.min_decode_tps {
            t.min_decode_tokens_per_sec = v;
        }
        if let Some(ms) = self.max_ttft_ms {
            t.max_ttft = Duration::from_millis(ms);
        }
        if let Some(c) = self.max_peak_temp_c {
            t.max_peak_temp_celsius = c;
        }
        t
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(passed) => {
            if passed {
                ExitCode::SUCCESS
            } else {
                eprintln!("benchmark FAILED one or more acceptance targets");
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("benchmark error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Run the benchmark. Returns `Ok(true)` when every acceptance target
/// passed, `Ok(false)` when the run completed but failed a target.
async fn run() -> Result<bool, Box<dyn std::error::Error>> {
    let args = Args::parse();
    let prompts = load_bench_prompts(&args.prompts, DEFAULT_BENCH_SYSTEM_PROMPT)?;
    println!(
        "loaded {} prompts from {}",
        prompts.len(),
        args.prompts.display()
    );

    let qairt_lib_dir = args.resolved_qairt_lib_dir();
    println!(
        "constructing QnnBackend (bundle={}, qairt_lib={})",
        args.bundle_dir.display(),
        qairt_lib_dir.display()
    );
    let backend = QnnBackend::new(args.bundle_dir.clone(), qairt_lib_dir).await?;
    println!("backend ready: {}", backend.name());

    // Start the thermal sampler. It owns its own clock and returns the
    // collected samples when signalled to stop.
    let started = Instant::now();
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let sampler = tokio::spawn(thermal_sampler(started, stop_rx));

    let params = GenerationParams {
        max_tokens: BENCH_MAX_TOKENS,
        ..GenerationParams::default()
    };
    let duration = Duration::from_secs(args.duration_secs);

    let mut measurements: Vec<PromptMeasurement> = Vec::new();
    let mut cycle = 0usize;
    // Loop the corpus until the window elapses; cycling keeps the device
    // under sustained load to surface thermal throttling.
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

    // Stop the sampler and collect its readings.
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
    print_report(&report, &targets);
    Ok(evaluate(&report, &targets).all_pass())
}

/// Measure one prompt: TTFT (issue → first non-empty chunk) and decode
/// rate (first token → done).
async fn measure_prompt(
    backend: &QnnBackend,
    bp: &primer_inference::qnn::bench::BenchPrompt,
    params: &GenerationParams,
) -> Result<PromptMeasurement, Box<dyn std::error::Error>> {
    let issued = Instant::now();
    let mut stream = backend.generate_stream(&bp.prompt, params).await?;

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
    // Decode window: first token → last token. If only one token arrived,
    // the window is zero and `decode_tokens_per_sec` returns 0.0.
    let decode_duration = match first_token_at {
        Some(first) => last_token_at.duration_since(first),
        None => Duration::ZERO,
    };

    Ok(PromptMeasurement {
        label: bp.label.clone(),
        ttft,
        decode_tokens: tokens_after_first,
        decode_duration,
    })
}

/// Background thermal sampler. Ticks every [`THERMAL_SAMPLE_INTERVAL`],
/// reading every `thermal_zone*/temp` under [`THERMAL_SYSFS_DIR`], until
/// the stop signal fires. Returns all collected samples.
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

/// Read every `thermal_zone*/temp` node under `base` into samples stamped
/// with `elapsed_secs`. Silently skips unreadable or non-numeric nodes —
/// a flaky single zone must never abort the benchmark. Returns empty on a
/// host with no sysfs thermal tree (e.g. macOS), which the verdict treats
/// as a vacuous thermal pass.
fn read_thermal_zones(base: &Path, elapsed_secs: f64) -> Vec<ThermalSample> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let zone = name.to_string_lossy();
        if !zone.starts_with(THERMAL_ZONE_PREFIX) {
            continue;
        }
        let temp_path = entry.path().join(THERMAL_TEMP_FILE);
        let Ok(raw) = std::fs::read_to_string(&temp_path) else {
            continue;
        };
        if let Some(temp_celsius) = parse_thermal_millidegrees(&raw) {
            out.push(ThermalSample {
                elapsed_secs,
                zone: zone.into_owned(),
                temp_celsius,
            });
        }
    }
    out
}

/// Print the aggregate report and the pass/fail line per criterion.
fn print_report(report: &BenchReport, targets: &BenchTargets) {
    let verdict = evaluate(report, targets);
    let pf = |ok: bool| if ok { "PASS" } else { "FAIL" };
    println!("\n=== QNN benchmark report ({} runs) ===", report.runs);
    println!(
        "TTFT  p50={:.0}ms  p95={:.0}ms   (target p95 < {:.0}ms)  [{}]",
        report.ttft_p50.as_secs_f64() * 1000.0,
        report.ttft_p95.as_secs_f64() * 1000.0,
        targets.max_ttft.as_secs_f64() * 1000.0,
        pf(verdict.ttft_pass),
    );
    println!(
        "decode mean={:.2} tok/s  min={:.2} tok/s   (target min >= {:.2})  [{}]",
        report.decode_mean_tokens_per_sec,
        report.decode_min_tokens_per_sec,
        targets.min_decode_tokens_per_sec,
        pf(verdict.decode_pass),
    );
    match report.peak_temp_celsius {
        Some(t) => println!(
            "peak temp={:.1}°C   (target <= {:.1}°C)  [{}]",
            t,
            targets.max_peak_temp_celsius,
            pf(verdict.thermal_pass),
        ),
        None => println!("peak temp=n/a (no thermal samples)  [PASS — vacuous]"),
    }
    println!("overall: [{}]", pf(verdict.all_pass()));
}
