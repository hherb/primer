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
use std::sync::Arc;

use primer_core::error::Result;
use primer_core::inference::{GenerationParams, InferenceBackend, Prompt, TokenStream};
use primer_core::router::{Leg, RouterMode, complexity_score, order_legs};

/// Decorator wrapping a primary and a secondary backend with a routing mode.
pub struct RouterBackend {
    primary: Arc<dyn InferenceBackend>,
    secondary: Arc<dyn InferenceBackend>,
    mode: RouterMode,
}

impl RouterBackend {
    /// Construct a router. `primary` is the `--backend` leg; `secondary` is the
    /// `--fallback-backend` leg. `mode` selects the policy.
    pub fn new(
        primary: Arc<dyn InferenceBackend>,
        secondary: Arc<dyn InferenceBackend>,
        mode: RouterMode,
    ) -> Self {
        Self {
            primary,
            secondary,
            mode,
        }
    }

    fn leg(&self, leg: Leg) -> &Arc<dyn InferenceBackend> {
        match leg {
            Leg::Primary => &self.primary,
            Leg::Secondary => &self.secondary,
        }
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
        let score = params
            .routing
            .as_ref()
            .map(|s| complexity_score(s, prompt))
            .unwrap_or(0.0);
        let order = order_legs(self.mode, score);
        let first = self.leg(order.first);

        match first.generate_stream(prompt, params).await {
            Ok(stream) => Ok(stream),
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
                    second.generate_stream(prompt, params).await
                }
                None => Err(e),
            },
        }
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
}
