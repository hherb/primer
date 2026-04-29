//! Ollama inference backend — calls a local Ollama server.
//!
//! Useful for testing the pedagogical engine against real local models
//! without integrating llama.cpp directly. Requires `ollama serve` running
//! and the chosen model pulled (e.g., `ollama pull llama3.2`).

use async_trait::async_trait;
use futures::stream;
use primer_core::error::{PrimerError, Result};
use primer_core::inference::*;
use serde::{Deserialize, Serialize};

pub struct OllamaBackend {
    client: reqwest::Client,
    base_url: String,
    model: String,
}

impl OllamaBackend {
    pub fn new(base_url: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            model,
        }
    }
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    /// Disabled by default: Socratic responses are short and a reasoning
    /// trace would only burn the num_predict budget before any visible
    /// content is emitted. Older Ollama versions ignore this field.
    think: bool,
    options: ChatOptions,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ChatOptions {
    temperature: f32,
    top_p: f32,
    num_predict: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stop: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    message: ChatResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    content: String,
}

#[async_trait]
impl InferenceBackend for OllamaBackend {
    fn name(&self) -> &str {
        "ollama"
    }

    async fn is_available(&self) -> bool {
        self.client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await
            .is_ok()
    }

    async fn generate_stream(
        &self,
        prompt: &Prompt,
        params: &GenerationParams,
    ) -> Result<TokenStream> {
        // Ollama's chat API takes the system instruction as a leading
        // "system" message rather than a separate top-level field.
        let mut messages = Vec::with_capacity(prompt.messages.len() + 1);
        if !prompt.system.is_empty() {
            messages.push(ChatMessage {
                role: "system".to_string(),
                content: prompt.system.clone(),
            });
        }
        for m in &prompt.messages {
            messages.push(ChatMessage {
                role: match m.role {
                    Role::System => "system".to_string(),
                    Role::User => "user".to_string(),
                    Role::Assistant => "assistant".to_string(),
                },
                content: m.content.clone(),
            });
        }

        let request = ChatRequest {
            model: self.model.clone(),
            messages,
            stream: false,
            think: false,
            options: ChatOptions {
                temperature: params.temperature,
                top_p: params.top_p,
                num_predict: params.max_tokens,
                stop: params.stop_sequences.clone(),
            },
        };

        let response = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&request)
            .send()
            .await
            .map_err(|e| PrimerError::Inference(format!("Ollama request failed: {e}")))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| PrimerError::Inference(format!("Failed to read Ollama response: {e}")))?;

        if !status.is_success() {
            return Err(PrimerError::Inference(format!(
                "Ollama returned {status}: {body}"
            )));
        }

        let chat: ChatResponse = serde_json::from_str(&body).map_err(|e| {
            PrimerError::Inference(format!(
                "Failed to parse Ollama response: {e}\nBody: {body}"
            ))
        })?;

        if chat.message.content.trim().is_empty() {
            return Err(PrimerError::Inference(format!(
                "Ollama returned empty content (model may have exhausted num_predict on a \
                 reasoning trace, or model name may be wrong). Raw response: {body}"
            )));
        }

        let chunk = TokenChunk {
            text: chat.message.content,
            done: true,
        };
        Ok(Box::pin(stream::once(async { Ok(chunk) })))
    }
}
