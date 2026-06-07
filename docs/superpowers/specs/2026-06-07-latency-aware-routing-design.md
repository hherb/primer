# Latency-aware inference routing — design

**Date:** 2026-06-07
**Status:** approved (brainstorming → spec)
**Phase:** 1.3 (the remaining ROADMAP 1.3 sub-feature; follows the inference router shipped in PR #209)
**Predecessor spec:** [2026-06-07-inference-router-design.md](2026-06-07-inference-router-design.md) — this design fills in that spec's §6 "Latency-aware switching — extension point (deferred)".

## Goal

Let the `hybrid` router nudge a turn toward the strong **secondary** (typically
cloud) leg when the **primary** (typically local) leg has been slow lately —
"if my local model is taking too long to produce a first token, lean on the
cloud for this turn." This is the latency half of Phase 1.3; the complexity half
shipped in PR #209.

## The central constraint: no unvalidated magic threshold

The predecessor spec deferred this feature for one reason: a real time-to-first-
token (TTFT) **budget** is device/accelerator-specific and needs the owner-gated
llama.cpp / QNN bench numbers (p50/p95 TTFT per accelerator) that this repo has
not yet collected. Guessing a budget now would commit an unvalidated magic
number as an active default.

**Resolution: ship the full mechanism; make the TTFT budget config-driven and
OFF by default.** With no budget configured, the latency term contributes
exactly `0.0` and router behavior is byte-identical to today's. The owner sets a
real budget (CLI flag or GUI field) once bench numbers exist — no code change,
no recompile. The only constants that ship are a *weight* (`W_LATENCY`) and an
EMA *smoothing factor* (`TTFT_EMA_ALPHA`); neither is a device-specific
threshold, so both are legitimate `consts::router` tunables under the project's
no-magic-numbers rule.

## Architecture decision: the router owns the rolling TTFT, not the DM

The predecessor spec sketched "the dialogue manager tracks a rolling primary-leg
TTFT and passes it in." During design we found that approach has a correctness
flaw: the DM holds only `&dyn InferenceBackend` and cannot tell **which leg** the
router actually served. In `hybrid` mode, complex turns route to the cloud
(fast TTFT); feeding that cloud TTFT into a "primary/local TTFT" estimate would
drag the estimate down and partially defeat the feature exactly when local is
slow.

**Decision: the `RouterBackend` owns the rolling primary-leg TTFT.** It is the
only component that knows which leg it picked. It times the **primary** leg's
returned stream (first non-empty chunk) and folds the sample into an internal
rolling exponential moving average (EMA); secondary-leg streams pass through
untimed. The latency term is computed inside the router (which already computes
the complexity score), so **no dialogue-manager change is needed** for the
latency half — the intent + passage-count threading from PR #209 is untouched.

A consequence: `RoutingSignals` keeps its two fields `{ intent,
retrieved_passages }`. The reserved `recent_primary_ttft_ms` comment from the
predecessor spec is removed — the DM does not measure or pass TTFT.

## Components

### 1. Pure policy (`primer-core`)

`consts::router` gains:

```rust
/// Score added to a turn's complexity when the primary leg's recent TTFT is
/// over the configured budget, in `hybrid` mode. A weight, not a threshold:
/// it only fires when a budget is configured. Starting value.
pub const W_LATENCY: f32 = 0.30;

/// Exponential-moving-average smoothing factor for the rolling primary-leg
/// TTFT. Device-independent (a standard EMA alpha), NOT a routing threshold.
pub const TTFT_EMA_ALPHA: f32 = 0.3;
```

`router.rs` gains two pure, I/O-free functions:

```rust
/// O(1) rolling exponential moving average. `None` prev ⇒ the sample seeds
/// the average. `alpha` is the smoothing factor (0..=1).
pub fn update_ema(prev: Option<f64>, sample_ms: f64, alpha: f32) -> f64;

/// Latency routing contribution. Returns `W_LATENCY` only when BOTH a recent
/// TTFT and a budget are present AND `recent > budget`; otherwise `0.0`.
/// `budget_ms == None` ⇒ latency routing is inert (always `0.0`).
pub fn latency_term(recent_ttft_ms: Option<f64>, budget_ms: Option<u64>) -> f32;
```

`RoutingSignals` is unchanged in shape (`{ intent, retrieved_passages }`); only
the stale reservation comment is dropped.

### 2. Mechanism (`primer-inference::router`)

- `RouterBackend` gains `ttft_budget_ms: Option<u64>` and an internal
  `Arc<Mutex<Option<f64>>>` holding the rolling primary-leg TTFT EMA (ms).
- A `TtftTimingStream` adapter wraps the **primary** leg's `TokenStream`. On the
  first yielded chunk with non-empty text it computes `start.elapsed()` and
  updates the EMA via `update_ema(prev, sample_ms, TTFT_EMA_ALPHA)`. It records
  at most once per stream; secondary-leg streams are returned unwrapped.
- Scoring in `generate_stream`:
  ```rust
  let base = params.routing.as_ref().map(|s| complexity_score(s, prompt)).unwrap_or(0.0);
  let current_ema = *self.ttft_ema.lock();      // Option<f64>
  let score = base + latency_term(current_ema, self.ttft_budget_ms);
  let order = order_legs(self.mode, score);
  ```
  The latency nudge therefore changes behavior **only in `hybrid`** (cloud-
  preferred ignores the score; local-only builds no router).
- A `pub(crate)` test-only constructor seeds the EMA so router decision tests
  need no real sleeps (no flaky timing).

### 3. Wiring + CLI

- `RouterBackend::new(primary, secondary, mode, ttft_budget_ms)`.
- `build_main_backend` threads the budget through from the wiring params.
- CLI flag `--primary-ttft-budget-ms <ms>` (`Option<u64>`, default `None` =
  OFF). Inert unless paired with `--router-mode hybrid`; no hard error when set
  without hybrid (it simply has no effect — documented, not validated).

### 4. GUI mirror

- `BackendConfig.primary_ttft_budget_ms: Option<u64>` with `#[serde(default)]`
  so existing `gui-config.json` files still deserialize.
- `BackendConfigView` (read) carries it; `BackendConfigUpdate` carries it with
  **no** `#[serde(default)]`, so it is mandatory in every `update_settings` IPC
  payload — `settings.js::gather()` sends `null` when blank, and all existing
  IPC test payloads gain the field.
- `wiring.rs::build_with_strategy` passes it to `build_main_backend`.
- Settings → Inference backend: a "Primary TTFT budget (ms)" number input,
  revealed when the routing mode is `hybrid` (mirrors the fallback-reveal
  pattern). Blank = off. No `validate()` rule — a budget without hybrid is
  inert, not an error.

## Data flow

```
hybrid turn N:
  RouterBackend.generate_stream:
    base   = complexity_score(intent, passages, message)      [PR #209, pure]
    latency= latency_term(self.ttft_ema, self.budget)         [new, pure]
    score  = base + latency
    order  = order_legs(hybrid, score)                        [PR #209, pure]
    first  = leg(order.first)
    if first == Primary: wrap returned stream in TtftTimingStream
                         → first non-empty chunk updates self.ttft_ema
    else (Secondary):    return stream unwrapped (untimed)
```

## Testing (TDD; all host-runnable on default `cargo test`)

- **Pure** — `update_ema`: `None` prev seeds; a higher sample moves the average
  up; `alpha = 1.0` ⇒ average equals the latest sample; `alpha = 0.0` ⇒ average
  unchanged. `latency_term`: every Some/None combination; boundary `recent ==
  budget` ⇒ not over ⇒ `0.0`; `budget = None` ⇒ `0.0`; `recent > budget` ⇒
  `W_LATENCY`.
- **Router** — `TtftTimingStream` records exactly once, on the first non-empty
  chunk, primary-only; `budget = None` leaves the served leg identical to PR
  #209; with a **seeded high EMA + a budget below it**, a turn whose base score
  is below threshold **escalates to the secondary** (proves the latency nudge);
  the same turn with `budget = None` stays primary.
- **GUI** — serde round-trip of the new field; every IPC test payload updated.

## Verification

From `src/`, with `+1.88`:

```
cargo +1.88 fmt --all -- --check
cargo +1.88 clippy --workspace --all-targets -- -D warnings
cargo +1.88 test --workspace --no-fail-fast
cargo +1.88 test -p primer-inference --features qnn
cargo +1.88 clippy -p primer-inference --features llamacpp --all-targets -- -D warnings
```

## Out of scope (deferred, unchanged from predecessor)

- **Calibrating the budget.** The owner sets a real `--primary-ttft-budget-ms`
  after collecting llama.cpp / QNN bench numbers. This design only ships the
  mechanism + an OFF default.
- **N-leg chains** (big-local → small-local → cloud). The decorator pattern
  generalizes but needs device-tuning data.
- **Per-subsystem latency routing.** Classifier/extractor/comprehension keep
  `routing: None` ⇒ base score 0.0; latency does not apply to them (consistent
  with the predecessor spec's "per-subsystem routing is a non-goal").

## File-size watch

`primer-inference/src/router.rs` is 308 lines today. Adding `TtftTimingStream` +
its tests may push it over the 500-line guideline; if so, move the timing-stream
adapter to a sibling `router/timing.rs` (or promote `router.rs` to a directory
module) during implementation.
