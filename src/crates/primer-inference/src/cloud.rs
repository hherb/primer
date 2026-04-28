//! Cloud inference backend — calls the Anthropic Claude API.
//!
//! This is the high-quality path, used when the device has WiFi connectivity.
//! The pedagogical engine constructs the prompt locally; only the generation
//! is offloaded to the cloud. The child's conversation history is sent
//! per-request and not stored by the API (per Anthropic's data policy).

use async_trait::async_trait;
use futures::stream;
use primer_core::error::{PrimerError, Result};
use primer_core::inference::*;
use serde::{Deserialize, Serialize};

pub struct CloudBackend {
    client: reqwest::Client,
    api_endpoint: String,
    api_key: String,
    model: String,
}

impl CloudBackend {
    pub fn new(api_endpoint: String, api_key: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_endpoint,
            api_key,
            model,
        }
    }
}

/// Anthropic Messages API request body (simplified).
#[derive(Debug, Serialize)]
struct ApiRequest {
    model: String,
    max_tokens: u32,
    system: String,
    messages: Vec<ApiMessage>,
    temperature: f32,
    top_p: f32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stop_sequences: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ApiMessage {
    role: String,
    content: String,
}

/// Anthropic Messages API response body (simplified, non-streaming).
#[derive(Debug, Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    text: String,
}

#[async_trait]
impl InferenceBackend for CloudBackend {
    fn name(&self) -> &str {
        "cloud-anthropic"
    }

    async fn is_available(&self) -> bool {
        // Simple connectivity check — try to reach the API endpoint.
        self.client
            .head(&self.api_endpoint)
            .send()
            .await
            .is_ok()
    }

    async fn generate_stream(
        &self,
        prompt: &Prompt,
        params: &GenerationParams,
    ) -> Result<TokenStream> {
        // For now, use the non-streaming API and emit the result as a single chunk.
        // TODO: implement SSE streaming for lower time-to-first-token.

        let api_messages: Vec<ApiMessage> = prompt
            .messages
            .iter()
            .map(|m| ApiMessage {
                role: match m.role {
                    Role::System => "user".to_string(), // system is a top-level field
                    Role::User => "user".to_string(),
                    Role::Assistant => "assistant".to_string(),
                },
                content: m.content.clone(),
            })
            .collect();

        let request = ApiRequest {
            model: self.model.clone(),
            max_tokens: params.max_tokens,
            system: prompt.system.clone(),
            messages: api_messages,
            temperature: params.temperature,
            top_p: params.top_p,
            stop_sequences: params.stop_sequences.clone(),
        };

        let response = self
            .client
            .post(&format!("{}/v1/messages", self.api_endpoint))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| PrimerError::Inference(format!("API request failed: {e}")))?;

        let api_response: ApiResponse = response
            .json()
            .await
            .map_err(|e| PrimerError::Inference(format!("Failed to parse API response: {e}")))?;

        let text = api_response
            .content
            .into_iter()
            .map(|b| b.text)
            .collect::<Vec<_>>()
            .join("");

        let chunk = TokenChunk { text, done: true };
        Ok(Box::pin(stream::once(async { Ok(chunk) })))
    }
}
