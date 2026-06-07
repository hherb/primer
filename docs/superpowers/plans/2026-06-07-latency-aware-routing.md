# Latency-aware Routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a config-driven, OFF-by-default latency term to the `hybrid` inference router so slow local (primary) turns nudge toward the strong secondary leg, with the rolling primary-leg TTFT owned by the `RouterBackend` (correct leg attribution, no dialogue-manager change).

**Architecture:** Pure policy (`update_ema`, `latency_term`, two consts) lives in `primer-core`. The `RouterBackend` (in `primer-inference`) gains an internal rolling-EMA of the primary leg's time-to-first-token, measured by a `TtftTimingStream` wrapper, and folds `latency_term(ema, budget)` into the complexity score it already computes. A `Option<u64>` budget threads from a new CLI flag / GUI field through `BackendParams` to `RouterBackend::new`. With no budget set, `latency_term` returns `0.0` and behavior is byte-identical to today's router.

**Tech Stack:** Rust (edition 2024, toolchain 1.88), `async-trait`, `futures` streams, `tokio`, Tauri 2.x (GUI), vanilla JS (GUI frontend). All cargo commands run from `src/` with `~/.cargo/bin/cargo +1.88`.

**Spec:** [docs/superpowers/specs/2026-06-07-latency-aware-routing-design.md](../specs/2026-06-07-latency-aware-routing-design.md)

**Branch:** `feat/latency-aware-routing` (already created off `main`; spec already committed).

---

## File Structure

- `crates/primer-core/src/consts.rs` — add `W_LATENCY`, `TTFT_EMA_ALPHA` to `mod router` (Task 1).
- `crates/primer-core/src/router.rs` — add pure `update_ema` + `latency_term`; drop stale `recent_primary_ttft_ms` comment (Task 2).
- `crates/primer-inference/src/router.rs` — `RouterBackend` gains the budget field + EMA + `TtftTimingStream` + scoring change + `pub(crate)` test ctor (Task 3). Watch the 500-line guideline; if exceeded, split the timing stream into `router/timing.rs`.
- `crates/primer-engine/src/wiring.rs` — `BackendParams.primary_ttft_budget_ms`; pass to `RouterBackend::new`; fix all struct literals (Task 4).
- `crates/primer-cli/src/main.rs` — `--primary-ttft-budget-ms` flag + into `BackendParams` (Task 5).
- `crates/primer-gui/src/config.rs` — `BackendConfig`/`View`/`Update` field + `Default` + `From` + `into_config` + 7 IPC test payloads (Task 6).
- `crates/primer-gui/src/wiring.rs` — pass the field into `BackendParams` (Task 6).
- `crates/primer-gui/ui/index.html` + `crates/primer-gui/ui/settings.js` — budget input + bind/load/gather/reveal (Task 7).
- `README.md`, `ROADMAP.md`, `CLAUDE.md`, spec §-update (Task 8).

---

## Task 1: Router latency consts

**Files:**
- Modify: `crates/primer-core/src/consts.rs` (the `pub mod router { … }` block, currently ending around line 430)
- Test: same file's existing test module (or inline assertions in Task 2's tests)

- [ ] **Step 1: Add the two consts** to the END of `pub mod router { … }` in `crates/primer-core/src/consts.rs`, just before its closing `}`:

```rust
    /// Score added to a turn's complexity when the primary leg's recent
    /// time-to-first-token EMA exceeds the configured budget, in `hybrid`
    /// mode. A *weight*, not a threshold — it only contributes when a budget is
    /// configured (`--primary-ttft-budget-ms` / the GUI field). Sized so a
    /// slow local leg pushes an otherwise-routine turn (base score below
    /// `ROUTE_SECONDARY_THRESHOLD = 0.5`) over the line on its own. Starting
    /// value; the real budget is owner-calibrated from bench numbers.
    pub const W_LATENCY: f32 = 0.30;

    /// Exponential-moving-average smoothing factor for the rolling primary-leg
    /// TTFT. Device-independent (a standard EMA alpha in `0..=1`), NOT a
    /// routing threshold: higher = more weight on the latest sample.
    pub const TTFT_EMA_ALPHA: f32 = 0.3;
```

- [ ] **Step 2: Verify it compiles**

Run: `~/.cargo/bin/cargo +1.88 build -p primer-core`
Expected: builds clean.

- [ ] **Step 3: Commit**

```bash
cd /Users/hherb/src/primer/src
git add crates/primer-core/src/consts.rs
git commit -m "feat(router): W_LATENCY + TTFT_EMA_ALPHA consts (latency-aware routing)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Pure `update_ema` + `latency_term` in primer-core

**Files:**
- Modify: `crates/primer-core/src/router.rs` (add functions after `complexity_score`, ~line 143; drop stale comment ~lines 82-83)
- Test: same file's `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing tests.** Add to `crates/primer-core/src/router.rs`'s `mod tests` (before the closing `}`):

```rust
    #[test]
    fn update_ema_seeds_from_none() {
        // No prior average → the sample becomes the average verbatim.
        assert_eq!(update_ema(None, 1200.0, 0.3), 1200.0);
    }

    #[test]
    fn update_ema_moves_toward_new_sample() {
        // alpha 0.5 → halfway between prev and sample.
        assert_eq!(update_ema(Some(1000.0), 2000.0, 0.5), 1500.0);
    }

    #[test]
    fn update_ema_alpha_one_takes_latest() {
        assert_eq!(update_ema(Some(1000.0), 2000.0, 1.0), 2000.0);
    }

    #[test]
    fn update_ema_alpha_zero_keeps_prev() {
        assert_eq!(update_ema(Some(1000.0), 2000.0, 0.0), 1000.0);
    }

    #[test]
    fn latency_term_inert_without_budget() {
        use crate::consts::router::W_LATENCY;
        // No budget configured → always 0.0 regardless of how slow local is.
        assert_eq!(latency_term(Some(9999.0), None), 0.0);
        // No recent TTFT yet → 0.0.
        assert_eq!(latency_term(None, Some(500)), 0.0);
        let _ = W_LATENCY;
    }

    #[test]
    fn latency_term_fires_over_budget() {
        use crate::consts::router::W_LATENCY;
        assert_eq!(latency_term(Some(800.0), Some(500)), W_LATENCY);
    }

    #[test]
    fn latency_term_zero_at_or_under_budget() {
        // Boundary: recent == budget is NOT over → 0.0.
        assert_eq!(latency_term(Some(500.0), Some(500)), 0.0);
        assert_eq!(latency_term(Some(100.0), Some(500)), 0.0);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-core update_ema 2>&1 | tail -20`
Expected: FAIL — `cannot find function 'update_ema'` / `latency_term`.

- [ ] **Step 3: Implement the functions.** Add to `crates/primer-core/src/router.rs` immediately after `complexity_score` (the function ending ~line 143):

```rust
/// O(1) rolling exponential moving average of a TTFT sample, in milliseconds.
/// `prev == None` ⇒ the sample seeds the average. `alpha` (the smoothing
/// factor, `0..=1`, `TTFT_EMA_ALPHA` in practice) weights the latest sample.
/// Pure.
pub fn update_ema(prev: Option<f64>, sample_ms: f64, alpha: f32) -> f64 {
    match prev {
        None => sample_ms,
        Some(p) => alpha as f64 * sample_ms + (1.0 - alpha as f64) * p,
    }
}

/// Latency routing contribution to the complexity score. Returns `W_LATENCY`
/// only when BOTH a recent primary-leg TTFT and a budget are present AND the
/// recent TTFT is strictly greater than the budget; otherwise `0.0`. A
/// `budget_ms` of `None` makes latency routing entirely inert (the OFF
/// default). Pure.
pub fn latency_term(recent_ttft_ms: Option<f64>, budget_ms: Option<u64>) -> f32 {
    match (recent_ttft_ms, budget_ms) {
        (Some(recent), Some(budget)) if recent > budget as f64 => {
            crate::consts::router::W_LATENCY
        }
        _ => 0.0,
    }
}
```

- [ ] **Step 4: Drop the stale reservation comment.** In the `RoutingSignals` struct (~lines 82-83), delete the two commented lines:

```rust
    // Reserved extension point (latency-aware switching, deferred):
    // pub recent_primary_ttft_ms: Option<u64>,
```

so the struct ends cleanly after `pub retrieved_passages: usize,`. Update the struct's doc comment's final sentence to note TTFT is router-owned: append to the existing doc comment a line `/// (Latency-aware routing is router-owned — see primer-inference's RouterBackend — so no TTFT field lives here.)`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-core router 2>&1 | tail -20`
Expected: PASS (all new + existing router tests).

- [ ] **Step 6: Commit**

```bash
git add crates/primer-core/src/router.rs
git commit -m "feat(router): pure update_ema + latency_term (latency-aware routing)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `RouterBackend` latency mechanism

**Files:**
- Modify: `crates/primer-inference/src/router.rs` (struct, `new`, `generate_stream`, add `TtftTimingStream`, add `pub(crate)` seed ctor)
- Test: same file's `#[cfg(test)] mod tests`

**Context:** Today `RouterBackend::new(primary, secondary, mode)` and `generate_stream` computes `score = complexity_score(...)` then `order_legs(mode, score)`. We add an internal EMA and a budget, time the primary leg's first chunk, and fold `latency_term` into the score.

- [ ] **Step 1: Write the failing tests.** Add to `crates/primer-inference/src/router.rs`'s `mod tests`. These use the existing `MockBackend`, `prompt()`, `params_with`, `drive` helpers already in the file:

```rust
    #[tokio::test]
    async fn budget_none_is_byte_identical_to_today() {
        // A routine turn (low base score) with NO budget stays on primary even
        // if we seed a huge EMA — latency routing is inert without a budget.
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
        let r = RouterBackend::with_ttft_ema_for_test(
            primary,
            secondary,
            RouterMode::Hybrid,
            None,          // no budget
            Some(99_999.0) // pretend local is extremely slow
        );
        let out = drive(
            r.generate_stream(&prompt(), &params_with(PedagogicalIntent::Encouragement, 0))
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(out, "LOCAL");
        assert_eq!(pcalls.load(Ordering::SeqCst), 1);
        assert_eq!(scalls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn slow_local_escalates_routine_turn_to_secondary() {
        // Same routine turn, but WITH a budget below the seeded EMA → the
        // latency term pushes the score over threshold → secondary.
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
        let r = RouterBackend::with_ttft_ema_for_test(
            primary,
            secondary,
            RouterMode::Hybrid,
            Some(500),     // 500 ms budget
            Some(2_000.0), // local has been averaging 2 s TTFT
        );
        let out = drive(
            r.generate_stream(&prompt(), &params_with(PedagogicalIntent::Encouragement, 0))
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(out, "CLOUD");
        assert_eq!(scalls.load(Ordering::SeqCst), 1);
        assert_eq!(pcalls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn primary_leg_records_ttft_into_ema() {
        // After a primary-served turn, the EMA transitions from None to Some.
        let (primary, _) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
        let (secondary, _) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
        let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
        assert!(r.ttft_ema_for_test().is_none(), "EMA starts empty");
        // Routine turn → primary → stream wrapped → first chunk records TTFT.
        let stream = r
            .generate_stream(&prompt(), &params_with(PedagogicalIntent::Encouragement, 0))
            .await
            .unwrap();
        let _ = drive(stream).await.unwrap();
        assert!(
            r.ttft_ema_for_test().is_some(),
            "primary leg's first chunk recorded a TTFT sample"
        );
    }

    #[tokio::test]
    async fn secondary_leg_does_not_record_ttft() {
        // A cloud-preferred turn runs the secondary; its TTFT must NOT pollute
        // the primary EMA.
        let (primary, _) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
        let (secondary, _) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
        let r = RouterBackend::new(primary, secondary, RouterMode::CloudPreferred);
        let stream = r
            .generate_stream(&prompt(), &params_with(PedagogicalIntent::Encouragement, 0))
            .await
            .unwrap();
        let _ = drive(stream).await.unwrap();
        assert!(
            r.ttft_ema_for_test().is_none(),
            "secondary leg must not record into the primary EMA"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-inference router 2>&1 | tail -25`
Expected: FAIL — `with_ttft_ema_for_test` / `ttft_ema_for_test` not found.

- [ ] **Step 3: Implement.** Edit `crates/primer-inference/src/router.rs`:

(a) Update imports (top of file) to add the pure helpers + `Mutex` + stream pieces:

```rust
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

use primer_core::consts::router::TTFT_EMA_ALPHA;
use primer_core::error::Result;
use primer_core::inference::{GenerationParams, InferenceBackend, Prompt, TokenChunk, TokenStream};
use primer_core::router::{Leg, RouterMode, complexity_score, latency_term, order_legs, update_ema};
```

(b) Extend the struct + constructors:

```rust
/// Decorator wrapping a primary and a secondary backend with a routing mode.
pub struct RouterBackend {
    primary: Arc<dyn InferenceBackend>,
    secondary: Arc<dyn InferenceBackend>,
    mode: RouterMode,
    /// Owner-configured TTFT budget in ms. `None` ⇒ latency routing OFF (the
    /// default); `latency_term` is then always `0.0`.
    ttft_budget_ms: Option<u64>,
    /// Rolling exponential moving average of the PRIMARY leg's measured
    /// time-to-first-token, in ms. `None` until the primary leg has served at
    /// least one turn. Shared with each `TtftTimingStream` so a primary-served
    /// turn updates it from the stream-consumption side.
    ttft_ema: Arc<Mutex<Option<f64>>>,
}

impl RouterBackend {
    /// Construct a router. `primary` is the `--backend` leg; `secondary` is the
    /// `--fallback-backend` leg. `mode` selects the policy. Latency routing is
    /// OFF (no budget); use [`RouterBackend::with_ttft_budget`] to enable it.
    pub fn new(
        primary: Arc<dyn InferenceBackend>,
        secondary: Arc<dyn InferenceBackend>,
        mode: RouterMode,
    ) -> Self {
        Self::with_ttft_budget(primary, secondary, mode, None)
    }

    /// Construct a router with an optional TTFT budget (ms). `None` ⇒ latency
    /// routing OFF. The budget only changes behavior in `hybrid` mode.
    pub fn with_ttft_budget(
        primary: Arc<dyn InferenceBackend>,
        secondary: Arc<dyn InferenceBackend>,
        mode: RouterMode,
        ttft_budget_ms: Option<u64>,
    ) -> Self {
        Self {
            primary,
            secondary,
            mode,
            ttft_budget_ms,
            ttft_ema: Arc::new(Mutex::new(None)),
        }
    }

    fn leg(&self, leg: Leg) -> &Arc<dyn InferenceBackend> {
        match leg {
            Leg::Primary => &self.primary,
            Leg::Secondary => &self.secondary,
        }
    }

    /// Current rolling primary-leg TTFT EMA (ms). Lock-poisoning falls back to
    /// `None` (treated as "no data" → latency term inert) rather than panicking.
    fn current_ttft_ema(&self) -> Option<f64> {
        self.ttft_ema.lock().map(|g| *g).unwrap_or(None)
    }

    #[cfg(test)]
    pub(crate) fn with_ttft_ema_for_test(
        primary: Arc<dyn InferenceBackend>,
        secondary: Arc<dyn InferenceBackend>,
        mode: RouterMode,
        ttft_budget_ms: Option<u64>,
        ema: Option<f64>,
    ) -> Self {
        let r = Self::with_ttft_budget(primary, secondary, mode, ttft_budget_ms);
        *r.ttft_ema.lock().unwrap() = ema;
        r
    }

    #[cfg(test)]
    pub(crate) fn ttft_ema_for_test(&self) -> Option<f64> {
        self.current_ttft_ema()
    }
}
```

(c) Replace the `generate_stream` body so it folds in the latency term and wraps the primary leg's stream:

```rust
    async fn generate_stream(
        &self,
        prompt: &Prompt,
        params: &GenerationParams,
    ) -> Result<TokenStream> {
        let base = params
            .routing
            .as_ref()
            .map(|s| complexity_score(s, prompt))
            .unwrap_or(0.0);
        let score = base + latency_term(self.current_ttft_ema(), self.ttft_budget_ms);
        let order = order_legs(self.mode, score);
        let first = self.leg(order.first);

        match first.generate_stream(prompt, params).await {
            Ok(stream) => Ok(self.maybe_time(order.first, stream)),
            Err(e) => match order.second {
                Some(second_leg) => {
                    let second = self.leg(second_leg);
                    tracing::warn!(
                        target: "primer::router",
                        first = first.name(),
                        second = second.name(),
                        error = %e,
                        "routed leg failed pre-stream; falling back to other leg"
                    );
                    // The fallover stream is also timed when it is the primary.
                    let s = second.generate_stream(prompt, params).await?;
                    Ok(self.maybe_time(second_leg, s))
                }
                None => Err(e),
            },
        }
    }
```

(d) Add the `maybe_time` helper inside `impl RouterBackend` (wraps only the primary leg):

```rust
    /// Wrap `stream` in a TTFT timer when it is the PRIMARY leg's stream;
    /// return secondary streams unwrapped (their TTFT must not pollute the
    /// primary EMA).
    fn maybe_time(&self, leg: Leg, stream: TokenStream) -> TokenStream {
        match leg {
            Leg::Primary => Box::pin(TtftTimingStream {
                inner: stream,
                start: Instant::now(),
                ema: self.ttft_ema.clone(),
                recorded: false,
            }),
            Leg::Secondary => stream,
        }
    }
```

(e) Add the `TtftTimingStream` adapter (place it after the `impl InferenceBackend for RouterBackend` block, before `#[cfg(test)]`):

```rust
/// Stream adapter that records the wall-clock time to the first non-empty
/// chunk into a shared TTFT EMA, then passes every item through unchanged.
/// Records at most once.
struct TtftTimingStream {
    inner: TokenStream,
    start: Instant,
    ema: Arc<Mutex<Option<f64>>>,
    recorded: bool,
}

impl Stream for TtftTimingStream {
    type Item = Result<TokenChunk>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let polled = self.inner.as_mut().poll_next(cx);
        if let Poll::Ready(Some(Ok(chunk))) = &polled {
            if !self.recorded && !chunk.text.is_empty() {
                let sample_ms = self.start.elapsed().as_secs_f64() * 1000.0;
                if let Ok(mut guard) = self.ema.lock() {
                    *guard = Some(update_ema(*guard, sample_ms, TTFT_EMA_ALPHA));
                }
                self.recorded = true;
            }
        }
        polled
    }
}
```

Note: `TokenStream` is `Pin<Box<dyn Stream<...> + Send>>`, so `self.inner.as_mut().poll_next(cx)` works directly.

- [ ] **Step 4: Run tests to verify they pass**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-inference router 2>&1 | tail -25`
Expected: PASS (4 new + 7 existing router tests).

- [ ] **Step 5: File-size check + clippy**

Run: `wc -l crates/primer-inference/src/router.rs && ~/.cargo/bin/cargo +1.88 clippy -p primer-inference --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clippy clean. If `router.rs` is over 500 lines, move `TtftTimingStream` + its `impl Stream` into a new `crates/primer-inference/src/router/timing.rs` with `mod timing; use timing::TtftTimingStream;` (promote `router.rs` → `router/mod.rs`), then re-run.

- [ ] **Step 6: Commit**

```bash
git add crates/primer-inference/src/router.rs
git commit -m "feat(router): router-owned rolling TTFT EMA + latency-aware scoring

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Thread the budget through wiring

**Files:**
- Modify: `crates/primer-engine/src/wiring.rs` — add `BackendParams.primary_ttft_budget_ms`; pass to `RouterBackend::new`; fix all literals.

- [ ] **Step 1: Add the field to `BackendParams`.** In `crates/primer-engine/src/wiring.rs`, after the `router_mode` field (~line 93), add:

```rust
    /// Phase 1.3 latency-aware routing budget (ms). `None` ⇒ latency routing
    /// OFF (the default). When set AND `router_mode == Hybrid`, the
    /// `RouterBackend` nudges a turn toward the secondary when its rolling
    /// primary-leg TTFT EMA exceeds this budget. Consumed only by
    /// [`build_main_backend`]'s router path.
    pub primary_ttft_budget_ms: Option<u64>,
```

- [ ] **Step 2: Pass it to the router.** In `build_router_backend` (~line 337), change the `RouterBackend::new(...)` call to:

```rust
            Ok(Arc::new(RouterBackend::with_ttft_budget(
                primary,
                secondary,
                params.router_mode,
                params.primary_ttft_budget_ms,
            )))
```

- [ ] **Step 3: Build to surface every broken literal**

Run: `~/.cargo/bin/cargo +1.88 build -p primer-engine 2>&1 | grep -A2 "missing field" | head -40`
Expected: errors at the `BackendParams { … }` test-fixture literals (around lines 803, 963, 1079, 1200, and the helper near 1406). Each lists `missing field primary_ttft_budget_ms`.

- [ ] **Step 4: Fix every literal.** For each `BackendParams { … }` literal the build flags in `crates/primer-engine/src/wiring.rs`, add the line directly after its `router_mode:` line:

```rust
            primary_ttft_budget_ms: None,
```

(Match the surrounding indentation. The helper near line 1406 takes individual params; if it threads `router_mode`, also add a `primary_ttft_budget_ms: Option<u64>` parameter there and pass it through — otherwise just add `primary_ttft_budget_ms: None,` to the literal it builds.)

- [ ] **Step 5: Verify the crate builds + tests pass**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-engine 2>&1 | tail -15`
Expected: builds clean; tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/primer-engine/src/wiring.rs
git commit -m "feat(router): thread primary_ttft_budget_ms through BackendParams

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: CLI flag `--primary-ttft-budget-ms`

**Files:**
- Modify: `crates/primer-cli/src/main.rs` — flag decl (~after line 106) + `BackendParams` construction (~line 985).

- [ ] **Step 1: Add the flag.** In `crates/primer-cli/src/main.rs`, after the `router_mode` flag (~line 106), add:

```rust
    /// Phase 1.3 latency-aware routing budget in milliseconds. Absent ⇒ latency
    /// routing OFF (the default). Only takes effect with `--router-mode hybrid`
    /// AND `--fallback-backend` set: when the local (primary) leg's recent
    /// time-to-first-token exceeds this budget, complex-enough turns are nudged
    /// to the secondary. Set the real value from your accelerator's bench
    /// numbers.
    #[arg(long)]
    primary_ttft_budget_ms: Option<u64>,
```

- [ ] **Step 2: Construct it into `BackendParams`.** In the `BackendParams { … }` literal (~line 940-985), after the `router_mode,` line (~985), add:

```rust
        primary_ttft_budget_ms: cli.primary_ttft_budget_ms,
```

- [ ] **Step 3: Verify the CLI builds + `--help` shows the flag**

Run: `~/.cargo/bin/cargo +1.88 run --bin primer -- --help 2>&1 | grep -A1 "primary-ttft-budget"`
Expected: the flag appears in help with its description.

- [ ] **Step 4: Smoke-test that a flagless run is unaffected**

Run: `echo "quit" | ~/.cargo/bin/cargo +1.88 run --bin primer -- --name T --age 8 2>&1 | tail -3`
Expected: the REPL starts and exits cleanly (stub backend, no router, no budget).

- [ ] **Step 5: Commit**

```bash
git add crates/primer-cli/src/main.rs
git commit -m "feat(cli): --primary-ttft-budget-ms flag (latency-aware routing, OFF by default)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: GUI config mirror

**Files:**
- Modify: `crates/primer-gui/src/config.rs` — `BackendConfig` (~143) + `Default` (~163) + `BackendConfigView` (~686) + `From` (~708) + `BackendConfigUpdate` (~793) + `into_config` (~824) + 7 IPC test payloads.
- Modify: `crates/primer-gui/src/wiring.rs` — pass field into `BackendParams` (~207).

- [ ] **Step 1: Add a serde round-trip test (failing).** In `crates/primer-gui/src/config.rs`'s test module, add:

```rust
    #[test]
    fn backend_config_carries_ttft_budget() {
        let mut cfg = BackendConfig::default();
        assert_eq!(cfg.primary_ttft_budget_ms, None, "OFF by default");
        cfg.primary_ttft_budget_ms = Some(750);
        let json = serde_json::to_string(&cfg).unwrap();
        let back: BackendConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.primary_ttft_budget_ms, Some(750));
        // Old configs without the field still deserialize (serde default).
        let old = r#"{"kind":"stub","model":null,"ollama_url":"u","openai_compat_url":"u","api_key_source":{"kind":"env"},"openai_compat_api_key_source":{"kind":"env"},"qnn_bundle_dir":null,"qnn_qairt_lib_dir":null,"gguf_path":null,"llamacpp_gpu_layers":null,"llamacpp_n_ctx":null,"reasoning_markers":"","fallback_backend":null,"fallback_model":null,"router_mode":"local-only"}"#;
        let parsed: BackendConfig = serde_json::from_str(old).unwrap();
        assert_eq!(parsed.primary_ttft_budget_ms, None);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-gui backend_config_carries_ttft 2>&1 | tail -15`
Expected: FAIL — no field `primary_ttft_budget_ms`.

- [ ] **Step 3: Add the `BackendConfig` field.** After the `router_mode` field (~line 143):

```rust
    /// Phase 1.3 latency-aware routing budget (ms). Mirrors the CLI's
    /// `--primary-ttft-budget-ms`. `None` (default) ⇒ latency routing OFF.
    /// Only takes effect with `router_mode == Hybrid` AND a configured
    /// fallback. `#[serde(default)]` so existing configs load unchanged.
    #[serde(default)]
    pub primary_ttft_budget_ms: Option<u64>,
```

- [ ] **Step 4: Add to `Default` impl.** After `router_mode: …::LocalOnly,` (~line 163):

```rust
            primary_ttft_budget_ms: None,
```

- [ ] **Step 5: Add to `BackendConfigView`.** After its `router_mode: String,` (~line 686):

```rust
    /// Latency-aware routing budget (ms). Passes through verbatim (not a
    /// secret) so the settings form can re-show it.
    pub primary_ttft_budget_ms: Option<u64>,
```

And in `From<&GuiConfig> for GuiConfigView` (~line 708), after `router_mode: c.backend.router_mode.name().to_string(),`:

```rust
                primary_ttft_budget_ms: c.backend.primary_ttft_budget_ms,
```

- [ ] **Step 6: Add to `BackendConfigUpdate`.** After its `router_mode: String,` (~line 793):

```rust
    /// Latency-aware routing budget (ms). Like every other
    /// `BackendConfigUpdate` field, this is **mandatory** in the
    /// `update_settings` payload (the struct has no `#[serde(default)]`), so
    /// `settings.js::gather()` must always send it (`null` when blank). Not a
    /// secret.
    pub primary_ttft_budget_ms: Option<u64>,
```

And in `into_config` (~line 824, after the `router_mode: …` block closes with `},`):

```rust
                primary_ttft_budget_ms: self.backend.primary_ttft_budget_ms,
```

- [ ] **Step 7: Update the 7 IPC test payloads.** Each JSON test payload in `config.rs` that contains `"router_mode": "local-only"` (lines ~1142, 1215, 1302, 1386, 1582, 1631, 1685) must gain a sibling key. Add `"primary_ttft_budget_ms": null,` immediately before each `"router_mode"` line (mind trailing-comma validity — `router_mode` is followed by `}` in some, `,` in others; add the new key BEFORE `router_mode` so it always needs a trailing comma, which is valid).

- [ ] **Step 8: Thread into GUI wiring.** In `crates/primer-gui/src/wiring.rs`, after `router_mode: backend_config.router_mode,` (~line 207):

```rust
        // Phase 1.3 latency-aware routing budget from Settings → Inference
        // backend. `None` ⇒ latency routing OFF. Mirrors the CLI's
        // `--primary-ttft-budget-ms`.
        primary_ttft_budget_ms: backend_config.primary_ttft_budget_ms,
```

- [ ] **Step 9: Run tests**

Run: `~/.cargo/bin/cargo +1.88 test -p primer-gui 2>&1 | tail -20`
Expected: PASS, including the new serde test and all IPC payload tests.

- [ ] **Step 10: Commit**

```bash
git add crates/primer-gui/src/config.rs crates/primer-gui/src/wiring.rs
git commit -m "feat(gui): mirror primary_ttft_budget_ms into BackendConfig/View/Update + wiring

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: GUI frontend (input + bind/load/gather/reveal)

**Files:**
- Modify: `crates/primer-gui/ui/index.html` (after the Routing mode `<label>`, ~line 477)
- Modify: `crates/primer-gui/ui/settings.js` (field registry ~70, load ~307, gather ~789, reveal ~535 + listener ~525)

- [ ] **Step 1: Add the HTML field.** In `crates/primer-gui/ui/index.html`, after the Routing-mode `</label>` (~line 477) and before the closing `</div>` (~478), add:

```html
            <label class="field" id="f-backend-ttft-budget-field" hidden>
              <span>Primary TTFT budget (ms)</span>
              <input
                type="number"
                id="f-backend-ttft-budget"
                min="1"
                placeholder="(off)"
              />
              <small class="hint muted"
                >Latency-aware routing (hybrid only). When the local backend's
                recent time-to-first-token exceeds this many milliseconds,
                complex-enough turns are routed to the fallback. Blank =
                off. Set from your device's measured TTFT.</small
              >
            </label>
```

- [ ] **Step 2: Bind the field.** In `crates/primer-gui/ui/settings.js`, in the `dom.fields` registry (~line 70, after `backendRouterMode`), add:

```javascript
    backendTtftBudget: document.getElementById("f-backend-ttft-budget"),
    backendTtftBudgetField: document.getElementById("f-backend-ttft-budget-field"),
```

- [ ] **Step 3: Load the value.** After the `f.backendRouterMode.value = …` line (~307), add:

```javascript
  f.backendTtftBudget.value = view.backend.primary_ttft_budget_ms ?? "";
  applyRouterModeReveal(f.backendRouterMode.value);
```

- [ ] **Step 4: Add a reveal function + listener.** Near `applyFallbackReveal` (~line 535), add:

```javascript
function applyRouterModeReveal(mode) {
  dom.fields.backendTtftBudgetField.hidden = mode !== "hybrid";
}
```

And register a change listener near the fallback listener (~line 525):

```javascript
  dom.fields.backendRouterMode.addEventListener("change", () => {
    applyRouterModeReveal(dom.fields.backendRouterMode.value);
  });
```

- [ ] **Step 5: Gather the value.** In the `gather()` payload (~line 789, after `router_mode: …`), add:

```javascript
      primary_ttft_budget_ms: (() => {
        const v = f.backendTtftBudget.value.trim();
        if (!v) return null;
        const n = Number.parseInt(v, 10);
        return Number.isFinite(n) && n > 0 ? n : null;
      })(),
```

- [ ] **Step 6: Verify the GUI builds** (compiles the Rust side; the JS is static)

Run: `~/.cargo/bin/cargo +1.88 build -p primer-gui 2>&1 | tail -5`
Expected: builds clean.

- [ ] **Step 7: Commit**

```bash
git add crates/primer-gui/ui/index.html crates/primer-gui/ui/settings.js
git commit -m "feat(gui): Primary TTFT budget input (revealed for hybrid routing)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Docs

**Files:**
- Modify: `CLAUDE.md` (the `RouterBackend` bullet), `README.md`, `ROADMAP.md`, and the spec's predecessor note.

- [ ] **Step 1: Update the `RouterBackend` bullet in `CLAUDE.md`.** Find the `**`RouterBackend` is the Phase 1.3 per-turn inference router**` bullet. Update its "Latency-aware switching is a deferred extension point" closing sentence to:

```
**Latency-aware switching has shipped, config-gated and OFF by default:** `RouterBackend` owns a rolling primary-leg TTFT EMA (measured by an internal `TtftTimingStream` that times ONLY the primary leg's first non-empty chunk — secondary streams pass through untimed, so cloud TTFT never pollutes the local estimate). The pure `primer_core::router::latency_term(recent_ttft_ms, budget_ms)` adds `consts::router::W_LATENCY` to the complexity score only when a budget is set AND the EMA exceeds it; `update_ema` (alpha = `TTFT_EMA_ALPHA`) is the O(1) roll-up. The budget is `--primary-ttft-budget-ms` (CLI) / the GUI "Primary TTFT budget (ms)" field (revealed for hybrid) / `BackendParams.primary_ttft_budget_ms`; `None` ⇒ inert (byte-identical to the no-latency router). No TTFT budget const ships — the owner calibrates it from bench numbers. The nudge only changes behavior in `hybrid` (cloud-preferred ignores the score; local-only builds no router).
```

- [ ] **Step 2: Update `README.md`.** Locate the inference-router / Phase 1.3 status line and note latency-aware routing now ships config-gated (OFF by default). Keep it to one or two sentences consistent with the surrounding prose. (If no router line exists yet in README, add a short bullet under the inference/status section.)

- [ ] **Step 3: Update `ROADMAP.md`.** Under Phase 1.3, change the latency-aware switching item from deferred/pending to "shipped — config-gated, OFF by default (`--primary-ttft-budget-ms`); threshold calibration owner-gated on bench numbers."

- [ ] **Step 4: Update the predecessor spec note.** In `docs/superpowers/specs/2026-06-07-inference-router-design.md` §6 ("Latency-aware switching — extension point (deferred)"), add a one-line banner at the top of that section: `> **Update 2026-06-07:** shipped — see [2026-06-07-latency-aware-routing-design.md](2026-06-07-latency-aware-routing-design.md). Config-gated, OFF by default; router-owned TTFT (not DM-owned as sketched below).`

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md README.md ROADMAP.md docs/superpowers/specs/2026-06-07-inference-router-design.md
git commit -m "docs(router): latency-aware routing shipped (config-gated, OFF by default)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Full verification + PR

- [ ] **Step 1: Run the complete verify loop from `src/`**

```bash
cd /Users/hherb/src/primer/src
~/.cargo/bin/cargo +1.88 fmt --all -- --check
~/.cargo/bin/cargo +1.88 clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo +1.88 test --workspace --no-fail-fast
~/.cargo/bin/cargo +1.88 test -p primer-inference --features qnn
~/.cargo/bin/cargo +1.88 clippy -p primer-inference --features llamacpp --all-targets -- -D warnings
```

Expected: all clean / 0 failures. Fix anything that fails before proceeding.

- [ ] **Step 2: Push + open the PR**

```bash
cd /Users/hherb/src/primer
git push -u origin feat/latency-aware-routing
gh pr create --base main --title "feat(router): Phase 1.3 latency-aware routing (config-gated, OFF by default)" --body "$(cat <<'EOF'
Adds the latency half of Phase 1.3: the `hybrid` router nudges slow-local turns toward the strong secondary leg, with the rolling primary-leg TTFT owned by the `RouterBackend` (correct leg attribution, no dialogue-manager change).

**Key property:** with no `--primary-ttft-budget-ms` set, behavior is byte-identical to today's router — no magic threshold ships active. The owner calibrates the budget from bench numbers.

- Pure `update_ema` + `latency_term` + `W_LATENCY`/`TTFT_EMA_ALPHA` consts in `primer-core`.
- `RouterBackend` rolling TTFT EMA via a primary-only `TtftTimingStream`.
- CLI `--primary-ttft-budget-ms` + GUI "Primary TTFT budget (ms)" field (revealed for hybrid).

Spec: docs/superpowers/specs/2026-06-07-latency-aware-routing-design.md

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 3: Confirm CI**

Run: `gh pr checks` (after a moment)
Expected: `cargo test (default features)` and the other checks go green.

---

## Self-review notes

- **Spec coverage:** consts (Task 1) · pure `update_ema`/`latency_term` + RoutingSignals comment drop (Task 2) · router EMA + `TtftTimingStream` + scoring + test ctor (Task 3) · `BackendParams` threading (Task 4) · CLI flag (Task 5) · GUI Config/View/Update + wiring + 7 payloads (Task 6) · GUI frontend reveal (Task 7) · docs incl. spec §6 update (Task 8) · verification (Task 9). All spec sections map to a task.
- **Type consistency:** `with_ttft_budget`, `with_ttft_ema_for_test`, `ttft_ema_for_test`, `current_ttft_ema`, `maybe_time`, `TtftTimingStream` are defined in Task 3 and referenced only there + Task 4 (`with_ttft_budget`). `primary_ttft_budget_ms` is the single field name used across `BackendParams`, `BackendConfig`/`View`/`Update`, both wirings, the CLI, and the GUI JSON key. `update_ema`/`latency_term` signatures defined in Task 2 are used verbatim in Task 3.
- **No magic numbers:** `W_LATENCY` + `TTFT_EMA_ALPHA` are the only new constants and both live in `consts::router`; no TTFT budget value is hard-coded (it's owner config, default `None`).
