//! [`Embedder`] implementation for OpenAI-compatible `/v1/embeddings`.
//!
//! Works with oMLX, LM Studio, vLLM, and remote providers (Together,
//! OpenRouter) that expose the standard OpenAI embeddings endpoint.
//! Unlike [`OllamaEmbedder`], this backend sends the full batch in a
//! single request — the OpenAI format supports array `input` natively.
//!
//! [`Embedder`]: primer_core::embedder::Embedder
//! [`OllamaEmbedder`]: crate::ollama::OllamaEmbedder

use async_trait::async_trait;
use primer_core::embedder::Embedder;
use primer_core::error::{PrimerError, Result};
use primer_core::speech::Named;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const DEFAULT_OPENAI_COMPAT_URL: &str = "http://localhost:8000";

pub struct OpenAiCompatEmbedder {
    client: reqwest::Client,
    base_url: String,
    model_name: String,
    api_key: Option<String>,
    dim: usize,
}

impl OpenAiCompatEmbedder {
    pub async fn new(base_url: &str, model_name: &str, api_key: Option<String>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| {
                PrimerError::Inference(
                    format!("openai-compat embedder client build failed: {e}").into(),
                )
            })?;

        let probe_vec = embed_batch(
            &client,
            base_url,
            model_name,
            api_key.as_deref(),
            &["primer"],
        )
        .await?;
        let dim = probe_vec
            .first()
            .ok_or_else(|| {
                PrimerError::Inference("openai-compat embedder probe returned no vectors".into())
            })?
            .len();

        Ok(Self {
            client,
            base_url: base_url.to_string(),
            model_name: model_name.to_string(),
            api_key,
            dim,
        })
    }
}

impl Named for OpenAiCompatEmbedder {
    fn name(&self) -> &str {
        &self.model_name
    }
}

#[async_trait]
impl Embedder for OpenAiCompatEmbedder {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        let vecs = embed_batch(
            &self.client,
            &self.base_url,
            &self.model_name,
            self.api_key.as_deref(),
            texts,
        )
        .await?;
        for (i, v) in vecs.iter().enumerate() {
            if v.len() != self.dim {
                return Err(PrimerError::Inference(
                    format!(
                        "openai-compat returned vector[{i}] of len {} but probe declared dim {}",
                        v.len(),
                        self.dim
                    )
                    .into(),
                ));
            }
        }
        Ok(vecs)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        &self.model_name
    }
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct EmbeddingRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
    index: usize,
}

// ---------------------------------------------------------------------------
// Shared batch helper
// ---------------------------------------------------------------------------

async fn embed_batch(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
    texts: &[&str],
) -> Result<Vec<Vec<f32>>> {
    let request = EmbeddingRequest {
        model: model.to_string(),
        input: texts.iter().map(|t| t.to_string()).collect(),
    };
    let mut req_builder = client
        .post(format!("{base_url}/v1/embeddings"))
        .json(&request);
    if let Some(key) = api_key {
        req_builder = req_builder.header("authorization", format!("Bearer {key}"));
    }
    let resp = req_builder.send().await.map_err(|e| {
        PrimerError::Inference(format!("openai-compat embedding request failed: {e}").into())
    })?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(PrimerError::Inference(
            format!("openai-compat embedding returned {status}: {body}").into(),
        ));
    }
    let body: EmbeddingResponse = resp.json().await.map_err(|e| {
        PrimerError::Inference(format!("openai-compat embedding response parse error: {e}").into())
    })?;

    let mut sorted = body.data;
    sorted.sort_by_key(|d| d.index);
    Ok(sorted.into_iter().map(|d| d.embedding).collect())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_request_serialization() {
        let req = EmbeddingRequest {
            model: "test-embed".to_string(),
            input: vec!["hello".to_string(), "world".to_string()],
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"model\":\"test-embed\""));
        assert!(json.contains("\"input\":[\"hello\",\"world\"]"));
    }

    #[test]
    fn embedding_response_deserialization() {
        let json = r#"{
            "data": [
                {"embedding": [0.1, 0.2], "index": 1},
                {"embedding": [0.3, 0.4], "index": 0}
            ]
        }"#;
        let resp: EmbeddingResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.data[0].index, 1);
        assert_eq!(resp.data[1].index, 0);
    }

    #[test]
    fn embedding_response_sorted_by_index() {
        let json = r#"{
            "data": [
                {"embedding": [0.1, 0.2], "index": 1},
                {"embedding": [0.3, 0.4], "index": 0}
            ]
        }"#;
        let resp: EmbeddingResponse = serde_json::from_str(json).unwrap();
        let mut sorted = resp.data;
        sorted.sort_by_key(|d| d.index);
        let vecs: Vec<Vec<f32>> = sorted.into_iter().map(|d| d.embedding).collect();
        assert_eq!(vecs[0], vec![0.3, 0.4]);
        assert_eq!(vecs[1], vec![0.1, 0.2]);
    }
}
