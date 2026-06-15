//! [`LlamaCppBackend`] — the [`InferenceBackend`] impl for embedded llama.cpp.
//!
//! The backend holds an `Arc<dyn LlamaEngine>` and owns the streaming
//! bridge: render the prompt on the calling task, then run the engine's
//! blocking decode loop inside `spawn_blocking`, feeding each RAW piece
//! through the shared reasoning filter before forwarding to the consumer.
//! Putting the reasoning strip here (not in the engine) keeps it covered by
//! the default `cargo test` via the mock.

use std::sync::Arc;

use async_trait::async_trait;
use futures::channel::mpsc;
use primer_core::error::Result;
use primer_core::inference::{GenerationParams, InferenceBackend, Prompt, TokenChunk, TokenStream};
use primer_core::reasoning::{ReasoningFilter, ReasoningMarker, default_markers};

use super::engine::LlamaEngine;
use crate::reasoning_stream::{FilterAction, process_filtered_chunk};

pub use primer_core::backend::LLAMACPP_NAME_PREFIX;

/// Embedded llama.cpp inference backend.
pub struct LlamaCppBackend {
    name: String,
    engine: Arc<dyn LlamaEngine>,
    reasoning_markers: Vec<ReasoningMarker>,
}

impl LlamaCppBackend {
    /// Construct from any [`LlamaEngine`]. Uses the built-in reasoning
    /// markers; append custom pairs with [`Self::with_extra_markers`].
    pub fn new(engine: Arc<dyn LlamaEngine>) -> Self {
        let name = format!("{LLAMACPP_NAME_PREFIX}{}", engine.model_id());
        Self {
            name,
            engine,
            reasoning_markers: default_markers(),
        }
    }

    /// Append custom `(open, close)` reasoning-marker pairs. Builder style.
    pub fn with_extra_markers(mut self, extra: Vec<(String, String)>) -> Self {
        self.reasoning_markers
            .extend(extra.into_iter().map(|(o, c)| ReasoningMarker::new(o, c)));
        self
    }
}

#[async_trait]
impl InferenceBackend for LlamaCppBackend {
    fn name(&self) -> &str {
        &self.name
    }

    async fn is_available(&self) -> bool {
        // Construction implies the model loaded successfully.
        true
    }

    async fn generate_stream(
        &self,
        prompt: &Prompt,
        params: &GenerationParams,
    ) -> Result<TokenStream> {
        // Render on the calling task (cheap CPU).
        let rendered = self.engine.render_prompt(prompt)?;
        let engine = Arc::clone(&self.engine);
        let markers = self.reasoning_markers.clone();
        let params = params.clone();
        let (tx, rx) = mpsc::unbounded::<Result<TokenChunk>>();

        tokio::task::spawn_blocking(move || {
            let mut filter = ReasoningFilter::new(markers);
            let mut had_visible = false;

            // Forward each RAW piece through the reasoning filter as a
            // non-final chunk. Returns false (stop) if the consumer dropped.
            let mut on_token = |piece: &str| -> bool {
                match process_filtered_chunk(
                    &mut filter,
                    TokenChunk {
                        text: piece.to_string(),
                        done: false,
                        ..Default::default()
                    },
                    &mut had_visible,
                    "llamacpp",
                ) {
                    FilterAction::Nothing => true,
                    FilterAction::Forward(r) => tx.unbounded_send(r).is_ok(),
                    // A non-final chunk never yields Final; treat defensively
                    // as "keep going".
                    FilterAction::Final(r) => tx.unbounded_send(r).is_ok(),
                }
            };

            let result = engine.infer(&rendered, &params, &mut on_token);

            match result {
                Ok(finish_reason) => {
                    // Flush: feed a synthetic done chunk so a held-back
                    // visible tail still reaches the child and a
                    // reasoning-only stream surfaces ReasoningWithoutAnswer.
                    // The engine's `finish_reason` rides on this terminal
                    // chunk (carried through the reasoning filter) so a
                    // `FinishReason::Length` truncation reaches the dialogue
                    // manager's context-limit recovery instead of looking
                    // like a clean `Stop`.
                    if let FilterAction::Final(r) = process_filtered_chunk(
                        &mut filter,
                        TokenChunk {
                            text: String::new(),
                            done: true,
                            finish_reason,
                        },
                        &mut had_visible,
                        "llamacpp",
                    ) {
                        let _ = tx.unbounded_send(r);
                    }
                }
                Err(e) => {
                    // Decode failure: surface it; do NOT flush the filter
                    // (mirrors the HTTP backends' mid-stream error policy).
                    let _ = tx.unbounded_send(Err(e));
                }
            }
        });

        Ok(Box::pin(rx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use primer_core::error::PrimerError;
    use primer_core::inference::{FinishReason, Prompt};

    /// A scripted engine for host tests. `pieces` are emitted as raw tokens
    /// in order; `fail_after` (if set) makes `infer` return an Err once that
    /// many pieces have been emitted; `finish` is the [`FinishReason`]
    /// reported on natural (non-error) completion.
    struct MockLlamaEngine {
        model_id: String,
        pieces: Vec<String>,
        fail_after: Option<usize>,
        finish: FinishReason,
    }

    impl MockLlamaEngine {
        fn new(model_id: &str, pieces: &[&str]) -> Self {
            Self {
                model_id: model_id.to_string(),
                pieces: pieces.iter().map(|s| s.to_string()).collect(),
                fail_after: None,
                finish: FinishReason::Stop,
            }
        }
        fn failing(model_id: &str, pieces: &[&str], fail_after: usize) -> Self {
            Self {
                model_id: model_id.to_string(),
                pieces: pieces.iter().map(|s| s.to_string()).collect(),
                fail_after: Some(fail_after),
                finish: FinishReason::Stop,
            }
        }
        /// A run that completes by exhausting the token budget — reports
        /// [`FinishReason::Length`] like a context-truncated reply. Builder
        /// style.
        fn truncated(mut self) -> Self {
            self.finish = FinishReason::Length;
            self
        }
    }

    impl LlamaEngine for MockLlamaEngine {
        fn model_id(&self) -> &str {
            &self.model_id
        }
        fn render_prompt(&self, prompt: &Prompt) -> Result<String> {
            Ok(format!(
                "SYS:{}|MSGS:{}",
                prompt.system,
                prompt.messages.len()
            ))
        }
        fn infer(
            &self,
            _rendered: &str,
            _params: &GenerationParams,
            on_token: &mut dyn FnMut(&str) -> bool,
        ) -> Result<FinishReason> {
            for (i, p) in self.pieces.iter().enumerate() {
                if !on_token(p) {
                    return Ok(FinishReason::Stop); // consumer dropped
                }
                if Some(i + 1) == self.fail_after {
                    return Err(PrimerError::Inference("mock decode failure".into()));
                }
            }
            Ok(self.finish)
        }
    }

    fn prompt() -> Prompt {
        Prompt {
            system: "be socratic".into(),
            messages: vec![],
        }
    }

    async fn collect(backend: &LlamaCppBackend) -> (String, bool) {
        let (out, errored, _) = collect_with_reason(backend).await;
        (out, errored)
    }

    /// Like [`collect`] but also returns the [`FinishReason`] of the terminal
    /// (`done`) chunk, defaulting to `Stop` if the stream errored or produced
    /// no terminal chunk.
    async fn collect_with_reason(backend: &LlamaCppBackend) -> (String, bool, FinishReason) {
        let mut stream = backend
            .generate_stream(&prompt(), &GenerationParams::default())
            .await
            .unwrap();
        let mut out = String::new();
        let mut errored = false;
        let mut finish = FinishReason::Stop;
        while let Some(item) = stream.next().await {
            match item {
                Ok(c) => {
                    out.push_str(&c.text);
                    if c.done {
                        finish = c.finish_reason;
                    }
                }
                Err(_) => {
                    errored = true;
                    break;
                }
            }
        }
        (out, errored, finish)
    }

    #[test]
    fn name_is_prefixed_model_id() {
        let backend = LlamaCppBackend::new(Arc::new(MockLlamaEngine::new("Qwen3-7B", &["hi"])));
        assert_eq!(backend.name(), "llamacpp:Qwen3-7B");
    }

    #[tokio::test]
    async fn streams_all_pieces_then_done() {
        let backend = LlamaCppBackend::new(Arc::new(MockLlamaEngine::new(
            "m",
            &["Hello", ", ", "world"],
        )));
        let (out, errored) = collect(&backend).await;
        assert_eq!(out, "Hello, world");
        assert!(!errored);
    }

    #[tokio::test]
    async fn generate_aggregates_stream() {
        let backend = LlamaCppBackend::new(Arc::new(MockLlamaEngine::new("m", &["a", "b", "c"])));
        let text = backend
            .generate(&prompt(), &GenerationParams::default())
            .await
            .unwrap();
        assert_eq!(text, "abc");
    }

    #[tokio::test]
    async fn reasoning_markers_are_stripped() {
        // Raw model output contains a <think> block; the consumer must not
        // see it. This proves the strip integration on the DEFAULT build.
        let backend = LlamaCppBackend::new(Arc::new(MockLlamaEngine::new(
            "m",
            &["<think>plan the answer</think>", "The sky is blue."],
        )));
        let (out, errored) = collect(&backend).await;
        assert_eq!(out, "The sky is blue.");
        assert!(!errored);
    }

    #[tokio::test]
    async fn reasoning_only_yields_error() {
        let backend = LlamaCppBackend::new(Arc::new(MockLlamaEngine::new(
            "m",
            &["<think>only thinking, no answer"],
        )));
        let (out, errored) = collect(&backend).await;
        assert_eq!(out, "");
        assert!(errored);
    }

    #[tokio::test]
    async fn mid_stream_decode_error_propagates() {
        let backend = LlamaCppBackend::new(Arc::new(MockLlamaEngine::failing(
            "m",
            &["partial ", "answer"],
            1,
        )));
        let (out, errored) = collect(&backend).await;
        assert_eq!(out, "partial ");
        assert!(errored);
    }

    #[tokio::test]
    async fn clean_run_emits_stop_finish_reason() {
        // A normal completion (eos / clean stop) carries FinishReason::Stop
        // on the terminal chunk — no spurious context-limit recovery.
        let backend = LlamaCppBackend::new(Arc::new(MockLlamaEngine::new("m", &["all done"])));
        let (out, errored, finish) = collect_with_reason(&backend).await;
        assert_eq!(out, "all done");
        assert!(!errored);
        assert_eq!(finish, FinishReason::Stop);
    }

    #[tokio::test]
    async fn truncated_run_emits_length_finish_reason() {
        // A run that exhausts the token budget reports FinishReason::Length so
        // the dialogue manager's notify-and-retry recovery fires (issue #238).
        let backend = LlamaCppBackend::new(Arc::new(
            MockLlamaEngine::new("m", &["cut off mid"]).truncated(),
        ));
        let (out, errored, finish) = collect_with_reason(&backend).await;
        assert_eq!(out, "cut off mid");
        assert!(!errored);
        assert_eq!(finish, FinishReason::Length);
    }

    #[tokio::test]
    async fn truncated_run_carries_length_through_reasoning_filter() {
        // Length must survive the reasoning strip: a truncated reply that
        // opened with a <think> block still reaches the child with the
        // terminal Length flag intact (the filter carries finish_reason).
        let backend = LlamaCppBackend::new(Arc::new(
            MockLlamaEngine::new("m", &["<think>plan</think>", "partial answer"]).truncated(),
        ));
        let (out, errored, finish) = collect_with_reason(&backend).await;
        assert_eq!(out, "partial answer");
        assert!(!errored);
        assert_eq!(finish, FinishReason::Length);
    }

    #[tokio::test]
    async fn custom_marker_is_stripped() {
        let backend = LlamaCppBackend::new(Arc::new(MockLlamaEngine::new(
            "m",
            &["a[[r]]hidden[[/r]]b"],
        )))
        .with_extra_markers(vec![("[[r]]".to_string(), "[[/r]]".to_string())]);
        let (out, errored) = collect(&backend).await;
        assert_eq!(out, "ab");
        assert!(!errored);
    }
}
