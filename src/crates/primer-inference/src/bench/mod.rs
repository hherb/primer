//! Backend-agnostic benchmark harness: prompt-corpus loading, timing
//! aggregation + pass/fail evaluation, thermal sampling, and the shared
//! run-loop helpers (`measure_prompt`, `format_report`) in the [`run`]
//! submodule.
//!
//! Everything here is pure / generic over `InferenceBackend`, so it is
//! exercised by the default `cargo test` even though the actual device runs
//! live in feature-gated example binaries. Backend-specific acceptance targets
//! live next to their backend (e.g. `qnn::bench::qnn_targets`); this module is
//! target-neutral.

use std::time::Duration;

pub mod metrics;
pub mod prompts;
pub mod run;
pub mod thermal;

pub use metrics::{
    BenchReport, BenchTargets, PERCENTILE_P50, PERCENTILE_P95, PromptMeasurement, Verdict,
    evaluate, percentile_duration,
};
pub use prompts::{
    BenchPrompt, BenchPromptError, DEFAULT_BENCH_SYSTEM_PROMPT, load_bench_prompts,
    parse_bench_prompts,
};
pub use run::{format_report, measure_prompt, spawn_thermal_sampler};
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
