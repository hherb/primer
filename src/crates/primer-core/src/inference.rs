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
}

impl Default for GenerationParams {
    fn default() -> Self {
        Self {
            max_tokens: 512,
            temperature: 0.7,
            top_p: 0.9,
            stop_sequences: vec![],
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

/// A single token (or chunk) emitted during streaming generation.
#[derive(Debug, Clone)]
pub struct TokenChunk {
    pub text: String,
    /// True when the model has finished generating.
    pub done: bool,
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
}
