# llama.cpp benchmark harness — design

**Date:** 2026-06-06
**Status:** approved (brainstorming → spec)
**Phase:** 1.1 bullet (b) — "Qwen3 7B Q4_K_M on MacBook (Metal) / DGX (CUDA) / RedMagic (Vulkan): tok/s + TTFT."
**Related:** mirrors the shape of the Phase 1.2 step 1.2.6 QNN benchmark (`primer-inference/src/qnn/bench/` + `examples/qnn_bench.rs`) and the macos-native-26-vs-Whisper probe (PR #131).

## Problem

`LlamaCppBackend` (Phase 1.1) has landed but is **benchmark-unverified**: we have no
host- or device-runnable harness that measures its time-to-first-token (TTFT) and
sustained decode rate (tok/s), nor a way to compare the same GGUF across the three
target accelerators (Metal / CUDA / Vulkan). The QNN backend already has exactly such
a harness, but its generic timing/aggregation/thermal logic is locked inside the
`qnn`-feature-gated module and partly inside the example binary, so llama.cpp cannot
reuse it without duplication.

## Goals

1. A runnable `examples/llamacpp_bench.rs` that loads a GGUF, loops a Socratic
   dialogue-continuation corpus for a wall-clock window, and reports p50/p95 TTFT,
   mean/min decode tok/s, and (where sysfs exposes it) peak temperature.
2. **Measurement-first**: a default run is a pure probe (no pass/fail). Acceptance
   gating is **opt-in** via CLI target flags, so the same binary can later serve as a
   CI regression gate without baking in a tok/s floor that doesn't generalize across a
   7B-on-a-MacBook and a 3B-on-a-phone.
3. **Single source of truth**: the generic bench machinery (~750 lines) lives once and
   is shared by both the QNN and llama.cpp benches, gaining default-`cargo test`
   coverage it previously lacked.
4. No new dependencies; no GGUF downloaded by any autonomous run (the example is the
   owner/hardware-gated device test).

## Non-goals

- Running the benchmark on real hardware (owner-gated; this ships the harness, not the
  numbers).
- A universal llama.cpp acceptance floor (deliberately omitted — see goal 2).
- Sharing the thermal-sampler *spawn* glue or arg-parsing between the two examples
  (example-local I/O orchestration; only the pure/testable core is shared).

## Design

### Component 1 — new crate-root `bench/` module (always compiled)

`primer-inference/src/bench/`, declared `pub mod bench;` at the crate root
(unconditionally — mirrors how the `LlamaEngine` seam + pure helpers are always
compiled while only `RealLlamaEngine` is feature-gated). Contents, moved verbatim from
`qnn/bench/` except where noted:

- **`metrics.rs`** — `PromptMeasurement`, `BenchReport`, `percentile_duration`,
  `Verdict`, `evaluate`, `BenchTargets`.
  - **The one behavioural change:** `BenchTargets` fields become optional:
    ```rust
    pub struct BenchTargets {
        pub min_decode_tokens_per_sec: Option<f64>,
        pub max_ttft: Option<Duration>,
        pub max_peak_temp_celsius: Option<f64>,
    }
    ```
    `evaluate` treats a `None` target as a vacuous pass for that criterion; a `Some`
    target gates as before. Thermal stays doubly-vacuous (pass when the target is `None`
    OR no sample was captured). `BenchTargets::none()` returns all-`None`;
    `is_measurement_only()` is true iff every field is `None`. `Default` = `none()`.
  - This single mechanism serves both consumers: QNN passes all-`Some` (hard gate),
    llama.cpp passes whatever CLI flags were supplied (measurement-only when none).
- **`prompts.rs`** — verbatim (already fully generic).
- **`thermal.rs`** — verbatim (Linux/Android sysfs; macOS → empty → vacuous pass).
- **`run.rs`** — *new, extracted from the QNN example so both examples share it:*
  - `async fn measure_prompt(backend: &dyn InferenceBackend, prompt: &BenchPrompt, params: &GenerationParams) -> Result<PromptMeasurement>` — the TTFT (issue → first
    non-empty chunk) + decode-window (first → last token) timing loop, generic over the
    trait, now unit-testable with a timed mock backend on the default test path.
  - `fn format_report(title: &str, report: &BenchReport, targets: &BenchTargets, verdict: &Verdict) -> String` — pure string builder (the example `println!`s it). When
    `targets.is_measurement_only()` it renders the numbers plus a "measurement only (no
    acceptance gate)" line instead of PASS/FAIL. Pure → testable without stdout capture.
- **`mod.rs`** — generic run-shape consts (`DEFAULT_DURATION_SECS`,
  `THERMAL_SAMPLE_INTERVAL`, `THERMAL_SYSFS_DIR`, `THERMAL_ZONE_PREFIX`,
  `THERMAL_TEMP_FILE`, `DEFAULT_PROMPTS_PATH`, `BENCH_MAX_TOKENS`,
  `DEFAULT_BENCH_SYSTEM_PROMPT`) + re-exports of the four submodules' public items.

### Component 2 — QNN refactor (contained; verified via `--features qnn`)

- Delete `qnn/bench/{metrics,prompts,thermal}.rs`; replace the `qnn/bench/` directory
  with a single `qnn/bench.rs` holding only the QNN-specific acceptance-target consts
  (`TARGET_MIN_DECODE_TOKENS_PER_SEC = 15.0`, `TARGET_MAX_TTFT = 3 s`,
  `TARGET_MAX_PEAK_TEMP_CELSIUS = 70.0`, plus the `PERCENTILE_P50/P95` selectors if still
  referenced) and a `qnn_targets() -> BenchTargets` constructor (`Some(15.0)` / `Some(3s)`
  / `Some(70°C)`) with a unit test pinning the values.
- `qnn/mod.rs`: `pub mod bench;` now resolves to the file; remove any re-exports of the
  moved generic items.
- `examples/qnn_bench.rs`: import generic items from `primer_inference::bench::*` and
  targets from `primer_inference::qnn::bench::qnn_targets`; delete its local
  `measure_prompt` / `print_report` in favour of `bench::run::measure_prompt` /
  `bench::run::format_report`. The thermal-sampler spawn glue stays example-local.

### Component 3 — new `examples/llamacpp_bench.rs`

`required-features = ["llamacpp"]` (GPU variants `llamacpp-metal/-cuda/-vulkan` activate
the base feature transitively, matching the CLI feature-chaining, so a
`--features llamacpp-metal` build compiles the example). Structure mirrors `qnn_bench.rs`:

- clap `Args`: `--model <gguf>` (required), `--gpu-layers <i32>` (optional),
  `--n-ctx <u32>` (optional), `--prompts <path>` (default `DEFAULT_PROMPTS_PATH`),
  `--duration-secs` (default `DEFAULT_DURATION_SECS`), `--max-tokens` (default
  `BENCH_MAX_TOKENS`), `--thermal-out <path>` (optional), and the opt-in gate flags
  `--min-decode-tps`/`--max-ttft-ms`/`--max-peak-temp-c` (each `Option`).
- Construction: `RealLlamaEngine::new(&model, resolve_gpu_layers(args.gpu_layers), args.n_ctx)?`
  → `Arc::new(...)` → `LlamaCppBackend::new(arc)`. Reuses `llamacpp::params::resolve_gpu_layers`
  so the no-flag default tracks the compiled GPU feature.
- Loop: same thermal-sampler-spawn + corpus-cycle-until-window as `qnn_bench.rs`, calling
  `bench::run::measure_prompt(&backend, bp, &params)`.
- Verdict: build `BenchTargets` from the optional flags (absent field → `None`). Print
  `format_report("llama.cpp benchmark", ...)`. If `targets.is_measurement_only()`, exit
  `SUCCESS` (pure probe); else `evaluate` and exit non-zero on any failed gate.

### Component 4 — corpus + guard

Reuse `data/bench/socratic_prompts.jsonl` verbatim. Move `tests/bench_corpus.rs` off the
`#![cfg(feature = "qnn")]` gate (it now guards a shared, always-compiled corpus) and
import from `primer_inference::bench` — so the 30-prompt floor + unique-label checks run
in the **default** `cargo test`, not only the QNN job.

### Component 5 — CI drift guard

Align with the existing `.github/workflows/ci.yml` granularity for `llamacpp`. Do **not**
add a heavy per-push full C++ build; if a feature-combo `clippy`/`check` job already
exercises `--features llamacpp`, ensure it carries `--all-targets` so the new example is
drift-checked. Decided at implementation time after reading the workflow.

## Testing (TDD)

New/changed logic gets tests first:

1. `evaluate` Option-semantics: `None` target → vacuous pass; `Some` → gates;
   thermal doubly-vacuous. (shared `metrics.rs`)
2. `BenchTargets::none()` / `is_measurement_only()`. (shared `metrics.rs`)
3. `measure_prompt` against a timed mock `InferenceBackend` emitting N chunks at known
   intervals: asserts TTFT ≈ first-chunk delay, `decode_tokens == N-1`, and a degenerate
   single-chunk stream → `decode_tokens == 0`. (shared `run.rs`)
4. `format_report` for the gated case (contains PASS/FAIL) and the measurement-only case
   (contains the "measurement only" line, no FAIL). (shared `run.rs`)
5. `qnn_targets()` value pin. (`qnn/bench.rs`)
6. Existing moved tests (percentile, report aggregation, prompt parsing, thermal parsing)
   ride along verbatim and must stay green.

## Acceptance criteria

- `cargo test --workspace` green; the shared `bench` tests (incl. `measure_prompt`,
  `format_report`, corpus guard) run on the **default** path.
- `cargo test -p primer-inference --features qnn` green; `cargo build --example qnn_bench
  --features qnn` compiles unchanged behaviourally.
- `cargo build --example llamacpp_bench --features llamacpp` compiles (host CPU build is
  sufficient for compile verification; a real GGUF run is owner-gated).
- `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --check`
  clean. Feature-gated clippy (`--features qnn`, `--features llamacpp`) clean.
- No new dependency added to any `Cargo.toml`.

## Risks / open points

- **Blast radius into shipped QNN code** (the `BenchTargets` Option redesign + the
  example's `measure_prompt`/`print_report` removal). Mitigation: QNN is feature-gated and
  not in default CI, so the `--features qnn` test + clippy + example-build are run
  explicitly as part of the acceptance gate.
- **llama.cpp C++ compile cost in CI** — avoided by not adding a per-push full build;
  drift-guarded at `check`/`clippy` granularity only.
- The real GGUF has never been run by an autonomous session; this harness is verified by
  compile + the shared-core unit tests, not by a live decode. The owner runs the device
  numbers (Phase 1.1 bullet b stays open until then).
