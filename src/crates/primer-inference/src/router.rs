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
use primer_core::router::{Leg, RouterMode, complexity_score, latency_term, order_legs, update_ema};

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
mod tests {
    use super::*;
    use futures::{StreamExt, stream};
    use primer_core::conversation::PedagogicalIntent;
    use primer_core::error::PrimerError;
    use primer_core::inference::TokenChunk;
    use primer_core::router::RoutingSignals;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone)]
    enum Behavior {
        Ok(String),
        PreStreamErr,
        MidStreamErr,
    }

    struct MockBackend {
        name: String,
        calls: Arc<AtomicUsize>,
        behavior: Behavior,
    }

    impl MockBackend {
        fn new(name: &str, behavior: Behavior) -> (Arc<Self>, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            let b = Arc::new(Self {
                name: name.to_string(),
                calls: calls.clone(),
                behavior,
            });
            (b, calls)
        }
    }

    #[async_trait]
    impl InferenceBackend for MockBackend {
        fn name(&self) -> &str {
            &self.name
        }
        async fn is_available(&self) -> bool {
            !matches!(self.behavior, Behavior::PreStreamErr)
        }
        async fn generate_stream(
            &self,
            _prompt: &Prompt,
            _params: &GenerationParams,
        ) -> Result<TokenStream> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match &self.behavior {
                Behavior::Ok(text) => {
                    let chunk = TokenChunk {
                        text: text.clone(),
                        done: true,
                    };
                    Ok(Box::pin(stream::once(async move { Ok(chunk) })))
                }
                Behavior::PreStreamErr => Err(PrimerError::Inference("primary down".into())),
                Behavior::MidStreamErr => Ok(Box::pin(stream::once(async {
                    Err(PrimerError::Inference("mid-stream boom".into()))
                }))),
            }
        }
    }

    fn prompt() -> Prompt {
        Prompt {
            system: String::new(),
            messages: vec![],
        }
    }

    fn params_with(intent: PedagogicalIntent, passages: usize) -> GenerationParams {
        GenerationParams {
            routing: Some(RoutingSignals {
                intent,
                retrieved_passages: passages,
            }),
            ..GenerationParams::default()
        }
    }

    async fn drive(mut s: TokenStream) -> Result<String> {
        let mut out = String::new();
        while let Some(item) = s.next().await {
            let chunk = item?;
            out.push_str(&chunk.text);
            if chunk.done {
                break;
            }
        }
        Ok(out)
    }

    #[tokio::test]
    async fn hybrid_high_score_routes_to_secondary() {
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
        let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
        let out = drive(
            r.generate_stream(&prompt(), &params_with(PedagogicalIntent::Scaffolding, 3))
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
    async fn hybrid_low_score_routes_to_primary() {
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
        let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
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
    async fn cloud_preferred_always_tries_secondary_first() {
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
        let r = RouterBackend::new(primary, secondary, RouterMode::CloudPreferred);
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
    async fn pre_stream_failure_falls_over_to_other_leg() {
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::PreStreamErr);
        let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
        let out = drive(
            r.generate_stream(&prompt(), &params_with(PedagogicalIntent::Scaffolding, 3))
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(out, "LOCAL");
        assert_eq!(
            scalls.load(Ordering::SeqCst),
            1,
            "secondary attempted first"
        );
        assert_eq!(
            pcalls.load(Ordering::SeqCst),
            1,
            "primary served the fallover"
        );
    }

    #[tokio::test]
    async fn mid_stream_error_propagates_without_reroute() {
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::MidStreamErr);
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
        let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
        let stream = r
            .generate_stream(&prompt(), &params_with(PedagogicalIntent::Encouragement, 0))
            .await
            .unwrap();
        assert!(drive(stream).await.is_err());
        assert_eq!(pcalls.load(Ordering::SeqCst), 1);
        assert_eq!(scalls.load(Ordering::SeqCst), 0, "no mid-stream reroute");
    }

    #[tokio::test]
    async fn name_returns_primary_name() {
        let (primary, _) = MockBackend::new("qnn:Qwen3-4B", Behavior::Ok("x".into()));
        let (secondary, _) = MockBackend::new("cloud", Behavior::Ok("y".into()));
        let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
        assert_eq!(r.name(), "qnn:Qwen3-4B");
    }

    #[tokio::test]
    async fn missing_routing_signals_scores_zero_and_uses_primary_in_hybrid() {
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
        let r = RouterBackend::new(primary, secondary, RouterMode::Hybrid);
        let out = drive(
            r.generate_stream(&prompt(), &GenerationParams::default())
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
    async fn latency_nudge_escalates_borderline_turn() {
        // Borderline base score (ComprehensionCheck = 0.25, 0 passages, no
        // message) is BELOW threshold on its own, but a slow local leg over
        // budget adds W_LATENCY (0.30) → 0.55 ≥ 0.5 → routes to the secondary.
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
        let r = RouterBackend::with_ttft_ema_for_test(
            primary,
            secondary,
            RouterMode::Hybrid,
            Some(500),     // 500 ms budget
            Some(2_000.0), // local has been averaging 2 s TTFT (over budget)
        );
        let out = drive(
            r.generate_stream(&prompt(), &params_with(PedagogicalIntent::ComprehensionCheck, 0))
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
    async fn borderline_turn_stays_local_without_budget() {
        // Same borderline turn, but NO budget → latency inert → 0.25 < 0.5 →
        // stays local. Proves the nudge needs a configured budget.
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
        let r = RouterBackend::with_ttft_ema_for_test(
            primary,
            secondary,
            RouterMode::Hybrid,
            None,          // no budget
            Some(2_000.0), // slow local, but irrelevant without a budget
        );
        let out = drive(
            r.generate_stream(&prompt(), &params_with(PedagogicalIntent::ComprehensionCheck, 0))
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
    async fn slow_local_keeps_trivial_turn_local() {
        // Self-healing property: a TRIVIAL turn (Encouragement = 0.0 base) over
        // budget gets 0.0 + 0.30 = 0.30 < 0.5 → STAYS LOCAL. Latency is a
        // nudge, not a circuit-breaker, so routine turns keep exercising the
        // local leg and its TTFT EMA can recover.
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("LOCAL".into()));
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("CLOUD".into()));
        let r = RouterBackend::with_ttft_ema_for_test(
            primary,
            secondary,
            RouterMode::Hybrid,
            Some(500),
            Some(2_000.0),
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
}
