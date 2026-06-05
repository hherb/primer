//! Fallback inference backend — an opt-in decorator that serves a primary
//! backend and falls back to a secondary one when the primary fails
//! *before any token streams* (the pre-stream boundary).
//!
//! Trigger policy (see docs/superpowers/specs/2026-06-05-local-cloud-fallback-design.md):
//! - `generate_stream` tries the primary. If `primary.generate_stream().await`
//!   returns `Ok(stream)`, that stream is returned verbatim — a later (mid-stream)
//!   error propagates and the partial turn drops at the dialogue-manager layer,
//!   exactly as without a fallback. **No mid-stream fallback / re-generation.**
//! - If the primary returns `Err` (pre-stream failure), the secondary is tried.
//!
//! Startup/construction fallback (primary fails to *build*) is handled one layer
//! up in `primer-engine::wiring::build_main_backend`, not here.

use async_trait::async_trait;
use std::sync::Arc;

use primer_core::error::Result;
use primer_core::inference::{GenerationParams, InferenceBackend, Prompt, TokenStream};

/// Decorator wrapping a primary and a secondary backend.
pub struct FallbackBackend {
    primary: Arc<dyn InferenceBackend>,
    secondary: Arc<dyn InferenceBackend>,
}

impl FallbackBackend {
    /// Construct a fallback wrapper. `primary` is tried first on every
    /// request; `secondary` serves only when the primary fails pre-stream.
    pub fn new(primary: Arc<dyn InferenceBackend>, secondary: Arc<dyn InferenceBackend>) -> Self {
        Self { primary, secondary }
    }
}

#[async_trait]
impl InferenceBackend for FallbackBackend {
    /// Returns the **primary's** name verbatim. Load-bearing: the per-backend
    /// context budget (`primer_core::backend::is_small_context_backend`,
    /// prefix-anchored on `"qnn:"`) keys off `name()`. The prompt window is
    /// sized once per turn for the common case (primary); a cloud secondary
    /// handles a smaller window fine.
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
        match self.primary.generate_stream(prompt, params).await {
            Ok(stream) => Ok(stream),
            Err(e) => {
                tracing::warn!(
                    target: "primer::fallback",
                    primary = self.primary.name(),
                    secondary = self.secondary.name(),
                    error = %e,
                    "primary backend failed pre-stream; falling back to secondary"
                );
                self.secondary.generate_stream(prompt, params).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{StreamExt, stream};
    use primer_core::error::PrimerError;
    use primer_core::inference::TokenChunk;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// What a `MockBackend` does when `generate_stream` is called.
    #[derive(Clone)]
    enum Behavior {
        /// Pre-stream OK: emit one `done` chunk with this text.
        Ok(String),
        /// Pre-stream error (the `.await` itself returns `Err`).
        PreStreamErr,
        /// Pre-stream OK, but the first stream item is an `Err` (mid-stream).
        MidStreamErr,
    }

    struct MockBackend {
        name: String,
        calls: Arc<AtomicUsize>,
        behavior: Behavior,
    }

    impl MockBackend {
        /// Returns the backend plus a shared call-counter for assertions.
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
                Behavior::PreStreamErr => {
                    Err(PrimerError::Inference("primary pre-stream down".into()))
                }
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

    /// Drive a stream to completion, accumulating text or surfacing the error.
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
    async fn primary_ok_uses_primary_and_never_calls_secondary() {
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::Ok("hi".into()));
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("SHOULD-NOT".into()));
        let fb = FallbackBackend::new(primary, secondary);
        let out = drive(
            fb.generate_stream(&prompt(), &GenerationParams::default())
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(out, "hi");
        assert_eq!(pcalls.load(Ordering::SeqCst), 1);
        assert_eq!(
            scalls.load(Ordering::SeqCst),
            0,
            "secondary must not be called"
        );
    }

    #[tokio::test]
    async fn primary_pre_stream_err_falls_back_to_secondary() {
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::PreStreamErr);
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("from-cloud".into()));
        let fb = FallbackBackend::new(primary, secondary);
        let out = drive(
            fb.generate_stream(&prompt(), &GenerationParams::default())
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(out, "from-cloud");
        assert_eq!(pcalls.load(Ordering::SeqCst), 1);
        assert_eq!(
            scalls.load(Ordering::SeqCst),
            1,
            "secondary must serve the turn"
        );
    }

    #[tokio::test]
    async fn mid_stream_err_propagates_and_secondary_not_called() {
        let (primary, pcalls) = MockBackend::new("llamacpp:m", Behavior::MidStreamErr);
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::Ok("SHOULD-NOT".into()));
        let fb = FallbackBackend::new(primary, secondary);
        // generate_stream().await is Ok (pre-stream succeeded); the error appears
        // mid-stream while driving — it must propagate, NOT fall back.
        let stream = fb
            .generate_stream(&prompt(), &GenerationParams::default())
            .await
            .unwrap();
        let result = drive(stream).await;
        assert!(result.is_err(), "mid-stream error must propagate");
        assert_eq!(pcalls.load(Ordering::SeqCst), 1);
        assert_eq!(scalls.load(Ordering::SeqCst), 0, "no mid-stream fallback");
    }

    #[tokio::test]
    async fn both_pre_stream_err_propagates_secondary_error() {
        let (primary, _) = MockBackend::new("llamacpp:m", Behavior::PreStreamErr);
        let (secondary, scalls) = MockBackend::new("cloud", Behavior::PreStreamErr);
        let fb = FallbackBackend::new(primary, secondary);
        let result = fb
            .generate_stream(&prompt(), &GenerationParams::default())
            .await;
        assert!(result.is_err(), "both legs down ⇒ error");
        assert_eq!(scalls.load(Ordering::SeqCst), 1, "secondary was attempted");
    }

    #[tokio::test]
    async fn name_returns_primary_name() {
        let (primary, _) = MockBackend::new("qnn:Qwen3-4B", Behavior::Ok("x".into()));
        let (secondary, _) = MockBackend::new("cloud", Behavior::Ok("y".into()));
        let fb = FallbackBackend::new(primary, secondary);
        assert_eq!(fb.name(), "qnn:Qwen3-4B");
    }

    #[tokio::test]
    async fn is_available_true_if_either_leg_available() {
        // primary unavailable (PreStreamErr ⇒ is_available false), secondary ok
        let (primary, _) = MockBackend::new("llamacpp:m", Behavior::PreStreamErr);
        let (secondary, _) = MockBackend::new("cloud", Behavior::Ok("z".into()));
        let fb = FallbackBackend::new(primary, secondary);
        assert!(fb.is_available().await);
    }
}
