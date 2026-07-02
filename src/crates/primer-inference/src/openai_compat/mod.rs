//! OpenAI-compatible inference backend — calls any server that speaks
//! the OpenAI chat-completions API (oMLX, LM Studio, vLLM, Together,
//! Groq, OpenRouter, llama.cpp `--server`, etc.).

use async_trait::async_trait;
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use primer_core::error::{PrimerError, Result};
use primer_core::inference::*;
use serde::{Deserialize, Serialize};

use crate::cloud::{classify_reqwest_error, parse_retry_after};

// ---------------------------------------------------------------------------
// SSE buffer
// ---------------------------------------------------------------------------

/// Buffers bytes from a streaming HTTP response and yields complete
/// `data:` payloads from the OpenAI SSE format. Ignores comment lines
/// (`:...`), `event:` lines, `id:` lines, and blank lines.
struct OpenAiSseBuffer {
    buf: Vec<u8>,
}

impl OpenAiSseBuffer {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }

    fn extend(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Pop the next `data:` payload, if a complete line is available.
    /// Returns `None` if no complete line ending in `\n` is buffered.
    fn pop_data(&mut self) -> Option<String> {
        loop {
            let nl = self.buf.iter().position(|b| *b == b'\n')?;
            let line_bytes: Vec<u8> = self.buf.drain(..=nl).take(nl).collect();
            let mut line = String::from_utf8_lossy(&line_bytes).into_owned();
            if line.ends_with('\r') {
                line.pop();
            }

            if line.is_empty() {
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("data:") {
                let payload = rest.strip_prefix(' ').unwrap_or(rest);
                return Some(payload.to_string());
            }
            // event:, id:, retry:, or anything else — skip.
        }
    }
}

// ---------------------------------------------------------------------------
// SSE chunk deserialization
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    choices: Vec<ChunkChoice>,
}

#[derive(Debug, Deserialize)]
struct ChunkChoice {
    delta: ChunkDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChunkDelta {
    #[serde(default)]
    content: Option<String>,
}

/// Map an OpenAI-compatible `finish_reason` to a [`FinishReason`].
/// `"length"` (the model hit `max_tokens` / the context window) becomes
/// [`FinishReason::Length`] so the dialogue manager's recovery loop
/// fires; every other reason (including a clean `"stop"`) stays
/// [`FinishReason::Stop`].
fn map_openai_finish_reason(finish_reason: &str) -> FinishReason {
    match finish_reason {
        "length" => FinishReason::Length,
        _ => FinishReason::Stop,
    }
}

/// Parse one `data:` payload from the OpenAI SSE stream into a
/// `TokenChunk`. Returns `Ok(None)` for chunks with no content
/// (e.g. the initial role-only delta or empty choices).
fn parse_openai_compat_chunk(data: &str) -> Result<Option<TokenChunk>> {
    if data == "[DONE]" {
        return Ok(Some(TokenChunk {
            text: String::new(),
            done: true,
            ..Default::default()
        }));
    }
    let chunk: ChatCompletionChunk = serde_json::from_str(data).map_err(|e| {
        PrimerError::Inference(format!("OpenAI-compat SSE parse error: {e}; data: {data}").into())
    })?;
    let choice = match chunk.choices.first() {
        Some(c) => c,
        None => return Ok(None),
    };
    let text = choice.delta.content.clone().unwrap_or_default();
    if text.is_empty() && choice.finish_reason.is_none() {
        return Ok(None);
    }
    Ok(Some(TokenChunk {
        text,
        done: choice.finish_reason.is_some(),
        finish_reason: choice
            .finish_reason
            .as_deref()
            .map(map_openai_finish_reason)
            .unwrap_or_default(),
    }))
}

/// Detect a mid-stream error payload. After a 2xx handshake, many
/// OpenAI-compatible servers (OpenRouter, vLLM, proxies) report failure
/// as a `data:` event of the form `{"error": {"message": "...", ...}}`
/// (or `{"error": "..."}`). That shape does not deserialize as a
/// `ChatCompletionChunk`, so without this check it was warn-skipped as
/// "unparseable" and the truncated reply finished as a clean turn —
/// bypassing the partial-turn-drops-on-error invariant.
fn parse_openai_compat_error_data(data: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct ErrorPayload {
        error: ErrorBody,
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ErrorBody {
        Object { message: String },
        Text(String),
    }
    serde_json::from_str::<ErrorPayload>(data)
        .ok()
        .map(|p| match p.error {
            ErrorBody::Object { message } => message,
            ErrorBody::Text(t) => t,
        })
}

// ---------------------------------------------------------------------------
// Error classification
// ---------------------------------------------------------------------------

/// Translate a non-success HTTP response from an OpenAI-compatible
/// server into an `InferenceError`. Pure — no I/O, no `Self`.
///
/// 404 is classified as `ModelNotFound` only when the body matches one
/// of the canonical model-not-found phrasings emitted by the major
/// OpenAI-compat servers we target (OpenAI, oMLX, LM Studio, vLLM,
/// llama.cpp `--server`, Together, Groq, OpenRouter). A 404 with a
/// generic body (e.g. a misconfigured reverse proxy returning a plain
/// "Not Found") falls through to `Other` so the user sees a real error
/// rather than a misleading `ollama pull <model>` suggestion.
pub(crate) fn classify_openai_compat_status(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: &str,
    requested_model: &str,
) -> primer_core::error::InferenceError {
    use primer_core::error::InferenceError::*;
    let body_lower = body.to_lowercase();
    match status.as_u16() {
        401 | 403 => Auth,
        429 => RateLimited {
            retry_after: parse_retry_after(headers),
        },
        404 if is_model_not_found_body(&body_lower) => ModelNotFound {
            model: requested_model.to_string(),
        },
        500..=599 => ServiceUnavailable,
        _ => Other(format!("OpenAI-compat returned {status}: {body}")),
    }
}

/// Whether a 404 response body looks like a model-not-found error from
/// an OpenAI-compatible server. Match against a closed list of canonical
/// phrasings instead of the previous loose `contains("model") &&
/// contains("not")` rule, which would match generic 404 bodies like
/// "did you mean to add the model_not_found handler?".
fn is_model_not_found_body(body_lower: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "does not exist",  // OpenAI canonical
        "model not found", // vLLM, llama.cpp, Together
        "unknown model",   // Groq, some LM Studio versions
        "no such model",   // oMLX, OpenRouter
    ];
    PATTERNS.iter().any(|p| body_lower.contains(p))
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    temperature: f32,
    top_p: f32,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stop: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

pub struct OpenAiCompatBackend {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
    retry_settings: primer_core::retry::RetrySettings,
    /// Marker pairs whose enclosed reasoning is stripped from the stream.
    pub(crate) reasoning_markers: Vec<primer_core::reasoning::ReasoningMarker>,
}

impl OpenAiCompatBackend {
    pub fn new(base_url: String, model: String, api_key: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            model,
            api_key,
            retry_settings: primer_core::retry::RetrySettings::default(),
            reasoning_markers: primer_core::reasoning::default_markers(),
        }
    }

    /// Append custom `(open, close)` reasoning-marker pairs to the built-in
    /// defaults. Builder style; returns `Self`.
    pub fn with_extra_markers(mut self, extra: Vec<(String, String)>) -> Self {
        self.reasoning_markers.extend(
            extra
                .into_iter()
                .map(|(o, c)| primer_core::reasoning::ReasoningMarker::new(o, c)),
        );
        self
    }
}

#[async_trait]
impl InferenceBackend for OpenAiCompatBackend {
    fn name(&self) -> &str {
        "openai-compat"
    }

    async fn is_available(&self) -> bool {
        self.client
            .get(format!("{}/v1/models", self.base_url))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    async fn generate_stream(
        &self,
        prompt: &Prompt,
        params: &GenerationParams,
    ) -> Result<TokenStream> {
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

        let request = ChatCompletionRequest {
            model: self.model.clone(),
            messages,
            stream: true,
            temperature: params.temperature,
            top_p: params.top_p,
            max_tokens: params.max_tokens,
            stop: params.stop_sequences.clone(),
        };

        let api_key = self.api_key.clone();
        let url = format!("{}/v1/chat/completions", self.base_url);
        let model = self.model.clone();

        let response = primer_core::retry::retry_with_backoff(&self.retry_settings, || {
            let client = &self.client;
            let request = &request;
            let api_key = &api_key;
            let url = &url;
            let model = &model;
            async move {
                let mut req_builder = client
                    .post(url.as_str())
                    .header("content-type", "application/json")
                    .header("accept", "text/event-stream")
                    .json(request);
                if let Some(key) = api_key {
                    req_builder = req_builder.header("authorization", format!("Bearer {key}"));
                }
                let resp = req_builder
                    .send()
                    .await
                    .map_err(|e| classify_reqwest_error(&e))?;
                let status = resp.status();
                if !status.is_success() {
                    let headers = resp.headers().clone();
                    let body = resp.text().await.unwrap_or_default();
                    return Err(classify_openai_compat_status(
                        status, &headers, &body, model,
                    ));
                }
                Ok(resp)
            }
        })
        .await
        .map_err(PrimerError::Inference)?;

        let markers = self.reasoning_markers.clone();
        let (mut tx, rx) = mpsc::unbounded::<Result<TokenChunk>>();
        let mut bytes_stream = response.bytes_stream();

        tokio::spawn(async move {
            use primer_core::reasoning::ReasoningFilter;
            let mut buf = OpenAiSseBuffer::new();
            let mut filter = ReasoningFilter::new(markers);
            let mut had_visible = false;
            'outer: loop {
                match bytes_stream.next().await {
                    Some(Ok(bytes)) => {
                        buf.extend(&bytes);
                        while let Some(data) = buf.pop_data() {
                            let chunk = match parse_openai_compat_chunk(&data) {
                                Ok(Some(c)) => c,
                                Ok(None) => continue,
                                Err(e) => {
                                    // A mid-stream `{"error": ...}` payload is a
                                    // real generation failure, not line noise:
                                    // surface it so the partial turn drops at the
                                    // dialogue-manager layer instead of the reply
                                    // finishing as a clean truncated turn.
                                    if let Some(msg) = parse_openai_compat_error_data(&data) {
                                        let _ = tx
                                            .send(Err(PrimerError::Inference(
                                                format!("OpenAI-compat mid-stream error: {msg}")
                                                    .into(),
                                            )))
                                            .await;
                                        break 'outer;
                                    }
                                    tracing::warn!(
                                        "Skipping unparseable OpenAI-compat SSE line: {e}"
                                    );
                                    continue;
                                }
                            };
                            match crate::reasoning_stream::process_filtered_chunk(
                                &mut filter,
                                chunk,
                                &mut had_visible,
                                "openai-compat",
                            ) {
                                crate::reasoning_stream::FilterAction::Nothing => {}
                                crate::reasoning_stream::FilterAction::Forward(r) => {
                                    if tx.send(r).await.is_err() {
                                        break 'outer;
                                    }
                                }
                                crate::reasoning_stream::FilterAction::Final(r) => {
                                    let _ = tx.send(r).await;
                                    break 'outer;
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        // Mid-stream transport error: surface it and stop. We do
                        // NOT flush the filter here — an error already drops the
                        // partial turn at the dialogue-manager layer, and flushing
                        // an unterminated `Inside` block would risk leaking
                        // reasoning we deliberately suppressed.
                        let _ = tx
                            .send(Err(PrimerError::Inference(
                                format!("OpenAI-compat byte stream error: {e}").into(),
                            )))
                            .await;
                        break 'outer;
                    }
                    None => {
                        // Upstream closed without a `[DONE]` / finish_reason
                        // marker (abrupt EOF). A normal stream emits a done chunk,
                        // hits the `Final` arm, and breaks before reaching here —
                        // so reaching `None` means we must still flush. Feed a
                        // synthetic done chunk through the shared helper so a
                        // held-back visible tail still reaches the child and a
                        // stream that ended mid-reasoning surfaces
                        // `ReasoningWithoutAnswer` instead of silently producing
                        // nothing.
                        if let crate::reasoning_stream::FilterAction::Final(r) =
                            crate::reasoning_stream::process_filtered_chunk(
                                &mut filter,
                                TokenChunk {
                                    text: String::new(),
                                    done: true,
                                    ..Default::default()
                                },
                                &mut had_visible,
                                "openai-compat",
                            )
                        {
                            let _ = tx.send(r).await;
                        }
                        break 'outer;
                    }
                }
            }
        });

        Ok(Box::pin(rx))
    }
}

#[cfg(test)]
mod tests;
