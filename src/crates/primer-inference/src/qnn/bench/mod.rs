//! Benchmark + thermal harness support for the QNN backend (Phase 1.2
//! step 1.2.6).
//!
//! This module holds the **pure, host-testable** core of the benchmark:
//! prompt-corpus loading ([`prompts`]), timing aggregation and pass/fail
//! evaluation ([`metrics`]), and thermal-reading parsing/CSV ([`thermal`]).
//! The device-only orchestration — constructing a [`super::QnnBackend`],
//! driving `generate_stream`, reading `/sys/class/thermal` on a timer —
//! lives in the `examples/qnn_bench.rs` binary, which is the actual device
//! test. Everything that can be computed without a Qualcomm NPU is a pure
//! function here so it is covered by `cargo test -p primer-inference
//! --features qnn` on any host.
//!
//! The acceptance targets (≥ 15 tok/s sustained decode, < 3 s TTFT, ≤ 70 °C
//! peak) live as `TARGET_*` constants in [`metrics`]; run-shape defaults
//! (sample cadence, default duration, corpus path) live here.

use std::time::Duration;

pub mod metrics;
pub mod prompts;
pub mod thermal;

pub use metrics::{
    BenchReport, BenchTargets, PromptMeasurement, Verdict, evaluate, percentile_duration,
};
pub use prompts::{
    BenchPrompt, BenchPromptError, DEFAULT_BENCH_SYSTEM_PROMPT, load_bench_prompts,
    parse_bench_prompts,
};
pub use thermal::{ThermalSample, parse_thermal_millidegrees, peak_temp_celsius, thermal_csv};

/// Default wall-clock duration the benchmark loops prompts for, in seconds.
/// 15 minutes is long enough to drive the SoC into a steady thermal state
/// (the Phase 1.2 plan's `--duration-secs 900`).
pub const DEFAULT_DURATION_SECS: u64 = 900;

/// How often the background sampler reads the thermal zones. 2 s matches
/// the Phase 1.2 plan; finer cadence floods the CSV without revealing more
/// about a slow thermal-rise curve.
pub const THERMAL_SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

/// Base sysfs directory holding the thermal zones on Linux/Android. The
/// example scans this with `read_dir` (no glob dependency) for entries
/// named [`THERMAL_ZONE_PREFIX`]`*`.
pub const THERMAL_SYSFS_DIR: &str = "/sys/class/thermal";
/// Prefix of a thermal-zone subdirectory under [`THERMAL_SYSFS_DIR`].
pub const THERMAL_ZONE_PREFIX: &str = "thermal_zone";
/// Filename inside each zone directory holding the millidegree reading.
pub const THERMAL_TEMP_FILE: &str = "temp";

/// Default corpus path, relative to the workspace `src/` directory the
/// example is launched from.
pub const DEFAULT_PROMPTS_PATH: &str = "../data/bench/socratic_prompts.jsonl";

/// Token cap per benchmark prompt. Long enough to measure a stable
/// steady-state decode rate, short enough to fit many prompts into the
/// run window. The other generation knobs (temperature, top-p) come from
/// [`primer_core::inference::GenerationParams::default`] so they aren't
/// re-declared here.
pub const BENCH_MAX_TOKENS: u32 = 128;
