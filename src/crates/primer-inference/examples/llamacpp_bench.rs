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

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;
use primer_core::inference::{GenerationParams, InferenceBackend};
use primer_inference::LlamaCppBackend;
use primer_inference::bench::{
    BENCH_MAX_TOKENS, BenchReport, BenchTargets, DEFAULT_BENCH_SYSTEM_PROMPT,
    DEFAULT_DURATION_SECS, DEFAULT_PROMPTS_PATH, evaluate, format_report, load_bench_prompts,
    measure_prompt, peak_temp_celsius, spawn_thermal_sampler, thermal_csv,
};
use primer_inference::llamacpp::engine::RealLlamaEngine;
use primer_inference::llamacpp::params::resolve_gpu_layers;

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
    let (stop_tx, sampler) = spawn_thermal_sampler(started);

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
