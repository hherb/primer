//! Inference router decorator — picks between a primary (typically local/small)
//! and a secondary (typically cloud/strong) backend per turn, based on the
//! pure policy in `primer_core::router`, and self-fails-over at the pre-stream
//! boundary.
//!
//! See docs/superpowers/specs/2026-06-07-inference-router-design.md.
//!
//! Trigger policy: identical pre-stream boundary to `FallbackBackend`. The
//! router picks an ordered (first, second) leg pair via
//! `primer_core::router::order_legs`; if `first.generate_stream().await`
//! returns `Ok(stream)`, that stream is returned verbatim (mid-stream errors
//! propagate and the partial turn drops at the dialogue-manager layer — NO
//! mid-stream re-routing). If `first` returns `Err` pre-stream, `second` (when
//! present) is tried.

use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

use primer_core::consts::router::TTFT_EMA_ALPHA;
use primer_core::error::Result;
use primer_core::inference::{GenerationParams, InferenceBackend, Prompt, TokenChunk, TokenStream};
use primer_core::router::{
    Leg, RouterMode, complexity_score, latency_term, order_legs, update_ema,
};

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

#[async_trait]
impl InferenceBackend for RouterBackend {
    /// Returns the **primary's** name verbatim. Load-bearing: the per-backend
    /// context budget (`primer_core::backend::is_small_context_backend`) keys
    /// off `name()`. Mirrors `FallbackBackend`.
    fn name(&self) -> &str {
        self.primary.name()
    }

    async fn is_available(&self) -> bool {
        self.primary.is_available().await || self.secondary.is_available().await
    }

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
                    let s = second.generate_stream(prompt, params).await?;
                    Ok(self.maybe_time(second_leg, s))
                }
                None => Err(e),
            },
        }
    }
}

/// Stream adapter that records the wall-clock time to the first non-empty
/// chunk into a shared TTFT EMA, then passes every item through unchanged.
/// Records at most once.
///
/// **What this measures, precisely:** `start` is stamped in `maybe_time`, i.e.
/// *after* `generate_stream().await` returned `Ok`, and the sample is taken at
/// the first non-empty chunk poll. So it captures prompt-eval + first-decode
/// (the dominant TTFT term for a local backend) but EXCLUDES the pre-stream
/// `await` (connection / request setup) and INCLUDES any consumer-side
/// scheduling delay between polls. It is a self-consistent proxy for the local
/// leg's responsiveness (only the primary leg is ever timed), not strict
/// wall-clock TTFT — keep that in mind when calibrating `--primary-ttft-budget-ms`
/// from a bench harness that may define TTFT differently.
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

#[cfg(test)]
mod tests;
