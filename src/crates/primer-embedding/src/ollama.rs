//! [`Embedder`] implementation that calls Ollama's `/api/embeddings`.
//!
//! Useful as a developer-prototyping path — a laptop running Ollama
//! locally can serve `nomic-embed-text` (or other embedding models)
//! without bringing in fastembed-rs's ort dep tree. **Not the
//! recommended runtime backend**: it requires Ollama to be running
//! in the background, network availability to localhost (or wherever
//! the user pointed `--ollama-url`), and pays a per-call HTTP round
//! trip. Production use is `FastEmbedBackend`.
//!
//! [`Embedder`]: primer_core::embedder::Embedder

use async_trait::async_trait;
use primer_core::embedder::Embedder;
use primer_core::error::{PrimerError, Result};
use primer_core::speech::Named;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Default Ollama endpoint. Matches the rest of the codebase's
/// `--ollama-url` flag.
pub const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";

/// Default Ollama embedding model. Nomic-embed-text is small (137M
/// params), 768-dim, and decent quality.
pub const DEFAULT_OLLAMA_MODEL: &str = "nomic-embed-text";

/// Ollama-backed [`Embedder`].
pub struct OllamaEmbedder {
    client: reqwest::Client,
    base_url: String,
    model_name: String,
    /// Cached dimensionality discovered from the first successful embed.
    /// Ollama's `/api/embeddings` doesn't expose model metadata up front,
    /// so we infer dim by doing a one-shot probe at construction.
    dim: usize,
}

impl OllamaEmbedder {
    /// Construct, immediately probing the configured model to discover
    /// its vector dimensionality. Errors if the probe call fails — we
    /// want a clean upfront failure rather than letting the first real
    /// retrieval call surprise the user.
    pub async fn new() -> Result<Self> {
        Self::with_endpoint(DEFAULT_OLLAMA_URL, DEFAULT_OLLAMA_MODEL).await
    }

    /// Construct against an explicit endpoint and model name.
    pub async fn with_endpoint(base_url: &str, model_name: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| {
                PrimerError::Inference(format!("ollama client build failed: {e}").into())
            })?;

        // Probe with a one-token input to discover dim.
        let probe_vec = embed_one(&client, base_url, model_name, "primer").await?;
        Ok(Self {
            client,
            base_url: base_url.to_string(),
            model_name: model_name.to_string(),
            dim: probe_vec.len(),
        })
    }
}

impl Named for OllamaEmbedder {
    fn name(&self) -> &str {
        &self.model_name
    }
}

#[async_trait]
impl Embedder for OllamaEmbedder {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        // Ollama's `/api/embeddings` is one-text-per-call. Issue them
        // sequentially: the conversation hot path embeds 1–2 turns per
        // call, and KB ingestion is offline. Parallelising via
        // futures::stream::buffered is a tweak we can add later if a
        // batch path appears in profiling.
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            let v = embed_one(&self.client, &self.base_url, &self.model_name, t).await?;
            if v.len() != self.dim {
                return Err(PrimerError::Inference(
                    format!(
                        "ollama returned vector of len {} but probe declared dim {}",
                        v.len(),
                        self.dim
                    )
                    .into(),
                ));
            }
            out.push(v);
        }
        Ok(out)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        &self.model_name
    }
}

#[derive(Serialize)]
struct EmbeddingsRequest<'a> {
    model: &'a str,
    prompt: &'a str,
}

#[derive(Deserialize)]
struct EmbeddingsResponse {
    embedding: Vec<f32>,
}

async fn embed_one(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    prompt: &str,
) -> Result<Vec<f32>> {
    let url = format!("{}/api/embeddings", base_url.trim_end_matches('/'));
    let req = EmbeddingsRequest { model, prompt };
    let resp = client
        .post(&url)
        .json(&req)
        .send()
        .await
        .map_err(|e| PrimerError::Inference(format!("ollama POST {url}: {e}").into()))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(PrimerError::Inference(
            format!("ollama returned {status}: {body}").into(),
        ));
    }
    let parsed: EmbeddingsResponse = resp
        .json()
        .await
        .map_err(|e| PrimerError::Inference(format!("ollama parse: {e}").into()))?;
    if parsed.embedding.is_empty() {
        return Err(PrimerError::Inference(
            "ollama returned empty embedding vector".into(),
        ));
    }
    Ok(parsed.embedding)
}
