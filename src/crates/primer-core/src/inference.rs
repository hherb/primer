//! Inference backend trait — the contract for LLM text generation.
//!
//! Implementations may target:
//! - llama.cpp (CPU / Vulkan GPU) via llama-cpp-rs
//! - Qualcomm QNN SDK (Snapdragon NPU)
//! - Rockchip RKNN-LLM (RK1828 NPU)
//! - Cloud API (Anthropic Claude, etc.)
//!
//! The pedagogical engine calls `generate()` without knowing or caring
//! which backend is active. Backend selection is a runtime configuration
//! concern, not an application logic concern.

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

use crate::conversation::{Speaker, Turn};
use crate::error::Result;

/// Parameters controlling text generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationParams {
    /// Maximum tokens to generate.
    pub max_tokens: u32,
    /// Sampling temperature (0.0 = deterministic, 1.0+ = creative).
    pub temperature: f32,
    /// Top-p (nucleus) sampling threshold.
    pub top_p: f32,
    /// Stop sequences — generation halts when any of these appear.
    pub stop_sequences: Vec<String>,
    /// Optional per-turn routing signals (Phase 1.3 inference router). Set by
    /// the dialogue manager; read ONLY by `RouterBackend`. Every other backend
    /// ignores it. `None` ⇒ no routing context (router scores it 0.0).
    pub routing: Option<crate::router::RoutingSignals>,
    /// Optional cap on this call's pre-stream retry attempts. Set by
    /// `FallbackBackend` / `RouterBackend` on a leg that has a failover
    /// behind it (see [`GenerationParams::for_failover_leg`]); HTTP
    /// backends apply it via [`crate::retry::RetrySettings::capped`].
    /// `None` ⇒ the backend's own retry settings.
    #[serde(default)]
    pub max_retry_attempts: Option<u32>,
}

impl GenerationParams {
    /// A copy for a decorated leg with another leg behind it: retries are
    /// capped at [`crate::consts::retry::FAILOVER_LEG_MAX_ATTEMPTS`] (or a
    /// tighter cap already set), so failover happens fast (issue #312).
    pub fn for_failover_leg(&self) -> Self {
        let cap = crate::consts::retry::FAILOVER_LEG_MAX_ATTEMPTS;
        Self {
            max_retry_attempts: Some(self.max_retry_attempts.map_or(cap, |c| c.min(cap))),
            ..self.clone()
        }
    }
}

impl Default for GenerationParams {
    fn default() -> Self {
        Self {
            max_tokens: 512,
            temperature: 0.7,
            top_p: 0.9,
            stop_sequences: vec![],
            routing: None,
            max_retry_attempts: None,
        }
    }
}

/// A message in the conversation history, following the chat-completion
/// convention of role + content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

/// A prompt: system instruction + conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub system: String,
    pub messages: Vec<Message>,
}

/// Why a streaming generation ended. Meaningful only on the terminal
/// chunk (`done == true`); every non-terminal chunk carries the
/// default `Stop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FinishReason {
    /// The model finished cleanly.
    #[default]
    Stop,
    /// Generation was cut off because the context window filled. The
    /// reply already streamed in full via the token callback but may
    /// stop mid-thought — the dialogue manager reacts by notifying the
    /// child and retrying with a smaller prompt.
    Length,
}

/// A single token (or chunk) emitted during streaming generation.
#[derive(Debug, Clone, Default)]
pub struct TokenChunk {
    pub text: String,
    /// True when the model has finished generating.
    pub done: bool,
    /// Why generation ended. Only meaningful when `done == true`.
    pub finish_reason: FinishReason,
}

/// A boxed, pinned stream of token chunks — the return type of streaming generation.
pub type TokenStream = Pin<Box<dyn Stream<Item = Result<TokenChunk>> + Send>>;

/// The inference backend trait.
///
/// All LLM backends implement this. The pedagogical engine holds a
/// `Box<dyn InferenceBackend>` and is agnostic to the underlying engine.
#[async_trait]
pub trait InferenceBackend: Send + Sync {
    /// Human-readable name of this backend (e.g., "llama.cpp-vulkan", "claude-sonnet").
    fn name(&self) -> &str;

    /// Returns true if the backend is ready to accept requests.
    async fn is_available(&self) -> bool;

    /// Generate a complete response (non-streaming). Default implementation
    /// collects from the streaming variant.
    async fn generate(&self, prompt: &Prompt, params: &GenerationParams) -> Result<String> {
        use futures::StreamExt;
        let mut stream = self.generate_stream(prompt, params).await?;
        let mut output = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            output.push_str(&chunk.text);
            if chunk.done {
                break;
            }
        }
        Ok(output)
    }

    /// Generate a streaming response. This is the primary interface —
    /// streaming allows the TTS pipeline to begin speaking before
    /// generation is complete, reducing perceived latency.
    async fn generate_stream(
        &self,
        prompt: &Prompt,
        params: &GenerationParams,
    ) -> Result<TokenStream>;

    /// Condense a sequence of conversation turns into a short prose
    /// summary suitable for inclusion in a future system prompt.
    ///
    /// Used by `DialogueManager` to maintain long-term memory across
    /// hours of conversation: turns that fall out of the active context
    /// window are summarized and the summary is persisted on the
    /// `Session`. The default implementation builds a one-shot prompt
    /// and dispatches to `generate`. Stub backends override this to
    /// return a canned string so tests don't need a real model.
    ///
    /// `target_chars` is a soft target for the output length, not a
    /// hard cap. Implementations should pass it through to the model
    /// as guidance.
    async fn summarize(&self, turns: &[Turn], target_chars: usize) -> Result<String> {
        let prompt = build_summarize_prompt(turns, target_chars);
        let params = GenerationParams {
            // Heuristic: ~3 chars per token, plus headroom.
            max_tokens: ((target_chars / 3) as u32).max(256),
            temperature: 0.3,
            top_p: 0.9,
            stop_sequences: vec![],
            routing: None,
            max_retry_attempts: None,
        };
        self.generate(&prompt, &params).await
    }
}

/// Construct a summarization prompt from a slice of turns. The system
/// instruction frames the task; the conversation itself is laid out as
/// alternating user/assistant messages so the model "sees" the original
/// dialogue rather than a flattened transcript.
pub fn build_summarize_prompt(turns: &[Turn], target_chars: usize) -> Prompt {
    let system = format!(
        "You are summarizing a conversation between the Primer (a Socratic AI learning \
         companion) and a child. The summary will be re-shown to a future Primer turn \
         as long-term memory.\n\n\
         Capture, in plain prose:\n\
         - The topics that were explored.\n\
         - Concepts the child clearly grasped, and how they expressed that understanding.\n\
         - Concepts the child struggled with or had misconceptions about.\n\
         - The emotional arc (curious, frustrated, distracted, energised).\n\n\
         Write a single paragraph of about {target_chars} characters. No bullet lists, \
         no headings, no quotation marks. Refer to the child as \"the child\" — the \
         summary will be read by the Primer in a future session. Write the summary in \
         the same language the conversation is in. Begin directly with the summary \
         itself — no preamble like \"Here is a summary\".",
    );
    let messages = turns
        .iter()
        .map(|t| Message {
            role: match t.speaker {
                Speaker::Child => Role::User,
                Speaker::Primer => Role::Assistant,
            },
            content: t.text.clone(),
        })
        .collect();
    Prompt { system, messages }
}

#[cfg(test)]
mod gen_params_tests {
    use super::*;

    #[test]
    fn default_generation_params_have_no_routing() {
        let p = GenerationParams::default();
        assert!(p.routing.is_none());
    }

    #[test]
    fn default_generation_params_have_no_retry_cap() {
        assert!(GenerationParams::default().max_retry_attempts.is_none());
    }

    #[test]
    fn failover_leg_caps_retries_and_keeps_every_other_field() {
        let p = GenerationParams {
            max_tokens: 7,
            stop_sequences: vec!["x".into()],
            ..Default::default()
        };
        let leg = p.for_failover_leg();
        assert_eq!(
            leg.max_retry_attempts,
            Some(crate::consts::retry::FAILOVER_LEG_MAX_ATTEMPTS)
        );
        assert_eq!(leg.max_tokens, 7);
        assert_eq!(leg.stop_sequences, vec!["x".to_string()]);
    }

    #[test]
    fn failover_leg_keeps_a_tighter_existing_cap() {
        let p = GenerationParams {
            max_retry_attempts: Some(0),
            ..Default::default()
        };
        assert_eq!(p.for_failover_leg().max_retry_attempts, Some(0));
    }
}

#[cfg(test)]
mod finish_reason_tests {
    use super::*;

    #[test]
    fn finish_reason_defaults_to_stop() {
        assert_eq!(FinishReason::default(), FinishReason::Stop);
    }

    #[test]
    fn token_chunk_default_is_empty_not_done_stop() {
        let c = TokenChunk::default();
        assert!(c.text.is_empty());
        assert!(!c.done);
        assert_eq!(c.finish_reason, FinishReason::Stop);
    }
}
