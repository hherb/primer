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
    /// Build a new embedder against `base_url` using `model_name`.
    ///
    /// **Performs one HTTP round-trip at construction** to probe the
    /// model's output dim. With the 30s client timeout, a slow remote
    /// provider can stall startup by up to half a minute. This matches
    /// the `OllamaEmbedder` behaviour and is acceptable because: the
    /// CLI surface advertises the embedder construction explicitly
    /// (`eprintln!("Embedder: openai-compat …")`), and the dim is
    /// required up-front by the rest of the pipeline (knowledge-base
    /// schema records it; session storage validates against it). A
    /// lazy-dim variant would let cold starts unblock but at the cost
    /// of pushing the failure point past first-use — defer until
    /// startup latency is profiled.
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
        validate_embed_response(&vecs, texts.len(), self.dim)?;
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
// Response validation
// ---------------------------------------------------------------------------

/// Verify a batch response is well-shaped: vector count matches the input
/// batch size, and every vector has the dim the probe established.
///
/// A server returning fewer (or more) vectors than inputs would silently
/// mis-align with the caller's positional persistence (`save_turn_embedding`
/// writes one vector per source text by position), so a mismatch must
/// hard-error rather than degrade.
fn validate_embed_response(
    vecs: &[Vec<f32>],
    expected_count: usize,
    expected_dim: usize,
) -> Result<()> {
    if vecs.len() != expected_count {
        return Err(PrimerError::Inference(
            format!(
                "openai-compat returned {} vectors for {} inputs",
                vecs.len(),
                expected_count
            )
            .into(),
        ));
    }
    for (i, v) in vecs.iter().enumerate() {
        if v.len() != expected_dim {
            return Err(PrimerError::Inference(
                format!(
                    "openai-compat returned vector[{i}] of len {} but probe declared dim {expected_dim}",
                    v.len(),
                )
                .into(),
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared batch helper
// ---------------------------------------------------------------------------

/// Send one batch to `/v1/embeddings` and return the vectors in input order.
///
/// **No retry on purpose:** embedding is fire-and-forget background work
/// after each turn. A failure surfaces as one turn without a vector,
/// which degrades long-term retrieval for that single turn to BM25-only;
/// the conversation flow is unaffected. Adding retry would slow down
/// detection of a misconfigured server without buying anything for the
/// happy path. Compare to the inference backend, which retries because
/// a failure there blocks the child's turn entirely.
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
    fn validate_embed_response_accepts_well_shaped_batch() {
        let vecs = vec![vec![0.1, 0.2, 0.3], vec![0.4, 0.5, 0.6]];
        assert!(validate_embed_response(&vecs, 2, 3).is_ok());
    }

    #[test]
    fn validate_embed_response_rejects_short_batch() {
        // Server returned 1 vector for a 3-text request — silent mis-alignment
        // is the bug we're guarding against.
        let vecs = vec![vec![0.1, 0.2, 0.3]];
        let err = validate_embed_response(&vecs, 3, 3)
            .err()
            .expect("must err");
        let s = format!("{err}");
        assert!(s.contains("1 vectors"), "got: {s}");
        assert!(s.contains("3 inputs"), "got: {s}");
    }

    #[test]
    fn validate_embed_response_rejects_long_batch() {
        let vecs = vec![vec![0.1; 3], vec![0.2; 3], vec![0.3; 3]];
        let err = validate_embed_response(&vecs, 2, 3)
            .err()
            .expect("must err");
        let s = format!("{err}");
        assert!(s.contains("3 vectors"), "got: {s}");
        assert!(s.contains("2 inputs"), "got: {s}");
    }

    #[test]
    fn validate_embed_response_rejects_wrong_dim() {
        let vecs = vec![vec![0.1, 0.2, 0.3], vec![0.4, 0.5]];
        let err = validate_embed_response(&vecs, 2, 3)
            .err()
            .expect("must err");
        let s = format!("{err}");
        assert!(s.contains("vector[1]"), "got: {s}");
        assert!(s.contains("len 2"), "got: {s}");
        assert!(s.contains("dim 3"), "got: {s}");
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
