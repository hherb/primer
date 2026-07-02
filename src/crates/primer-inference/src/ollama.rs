//! Ollama inference backend — calls a local Ollama server.
//!
//! Useful for testing the pedagogical engine against real local models
//! without integrating llama.cpp directly. Requires `ollama serve` running
//! and the chosen model pulled (e.g., `ollama pull llama3.2`).

use async_trait::async_trait;
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use primer_core::error::{PrimerError, Result};
use primer_core::inference::*;
use serde::{Deserialize, Serialize};

use crate::cloud::classify_reqwest_error;

/// Translate a non-success Ollama HTTP response into an
/// `InferenceError`. Pure — no I/O, no `Self`.
///
/// `requested_model` is the model name the user supplied via `--model`,
/// used to populate `ModelNotFound { model }` when Ollama signals a
/// missing model. `body` is the dev-facing payload for `Other`.
pub(crate) fn classify_ollama_status(
    status: reqwest::StatusCode,
    body: &str,
    requested_model: &str,
) -> primer_core::error::InferenceError {
    use primer_core::error::InferenceError::*;
    let body_lower = body.to_lowercase();
    match status.as_u16() {
        404 if body_lower.contains("model") && body_lower.contains("not") => ModelNotFound {
            model: requested_model.to_string(),
        },
        500..=599 => ServiceUnavailable,
        _ => Other(format!("Ollama returned {status}: {body}")),
    }
}

pub struct OllamaBackend {
    client: reqwest::Client,
    base_url: String,
    model: String,
    retry_settings: primer_core::retry::RetrySettings,
    /// Marker pairs whose enclosed reasoning is stripped from the stream.
    pub(crate) reasoning_markers: Vec<primer_core::reasoning::ReasoningMarker>,
}

impl OllamaBackend {
    pub fn new(base_url: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            model,
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
struct ChatResponseMessage {
    content: String,
}

/// A single NDJSON line from Ollama's streaming chat API.
#[derive(Debug, Deserialize)]
struct ChatChunk {
    message: ChatResponseMessage,
    #[serde(default)]
    done: bool,
    /// Why generation stopped, present only on the terminal (`done`) line.
    /// Ollama reports `"length"` when the model hit its predict/context
    /// limit and `"stop"` on a clean finish.
    #[serde(default)]
    done_reason: Option<String>,
}

/// Map Ollama's `done_reason` to a [`FinishReason`]. `"length"` (the
/// model ran out of context / predict budget) becomes
/// [`FinishReason::Length`] so the dialogue manager's recovery loop
/// fires; everything else (including a clean `"stop"` or an absent
/// reason) stays [`FinishReason::Stop`].
fn map_ollama_finish_reason(done_reason: Option<&str>) -> FinishReason {
    match done_reason {
        Some("length") => FinishReason::Length,
        _ => FinishReason::Stop,
    }
}

/// Buffers bytes from a streaming HTTP response and yields complete
/// newline-delimited lines. Holds partial trailing data until a `\n`
/// arrives in a later chunk.
struct NdjsonBuffer {
    buf: Vec<u8>,
}

impl NdjsonBuffer {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }

    fn extend(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    fn pop_line(&mut self) -> Option<String> {
        let nl = self.buf.iter().position(|b| *b == b'\n')?;
        let line: Vec<u8> = self.buf.drain(..=nl).take(nl).collect();
        // `take(nl)` drops the trailing '\n'. Use lossy decoding so a stray
        // non-UTF-8 byte surfaces as U+FFFD rather than silently disappearing
        // — the caller can then log it via the normal parse-error path.
        Some(String::from_utf8_lossy(&line).into_owned())
    }
}

fn parse_ollama_line(line: &str) -> Result<TokenChunk> {
    let chunk: ChatChunk = serde_json::from_str(line).map_err(|e| {
        PrimerError::Inference(format!("Ollama NDJSON parse error: {e}; line: {line}").into())
    })?;
    Ok(TokenChunk {
        text: chunk.message.content,
        done: chunk.done,
        finish_reason: map_ollama_finish_reason(chunk.done_reason.as_deref()),
    })
}

/// Detect Ollama's post-handshake failure shape. After a 2xx handshake,
/// Ollama reports a generation failure (runner crash, OOM, model
/// unloaded) as an NDJSON line `{"error":"..."}`. That shape does not
/// deserialize as a `ChatChunk`, so without this check it was
/// warn-skipped as "unparseable", the connection closed, and the
/// abrupt-EOF flush finished the truncated reply as a clean turn —
/// bypassing the partial-turn-drops-on-error invariant.
fn parse_ollama_error_line(line: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct ErrorLine {
        error: String,
    }
    serde_json::from_str::<ErrorLine>(line)
        .ok()
        .map(|e| e.error)
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
            stream: true,
            think: false,
            options: ChatOptions {
                temperature: params.temperature,
                top_p: params.top_p,
                num_predict: params.max_tokens,
                stop: params.stop_sequences.clone(),
            },
        };

        // Retry the send + status-check phase only. Once we have a 2xx
        // response and start consuming the byte stream, mid-stream errors
        // propagate as before (the partial Primer turn is dropped at a
        // higher layer).
        let response = primer_core::retry::retry_with_backoff(&self.retry_settings, || async {
            let resp = self
                .client
                .post(format!("{}/api/chat", self.base_url))
                .json(&request)
                .send()
                .await
                .map_err(|e| classify_reqwest_error(&e))?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(classify_ollama_status(status, &body, &self.model));
            }
            Ok(resp)
        })
        .await
        .map_err(PrimerError::Inference)?;

        let markers = self.reasoning_markers.clone();
        let (mut tx, rx) = mpsc::unbounded::<Result<TokenChunk>>();
        let mut bytes_stream = response.bytes_stream();

        // Fire-and-forget: no JoinHandle. If the consumer drops `rx`, the
        // first `tx.send` will fail and the task exits naturally. If the
        // upstream hangs without closing the connection, the task lives
        // until the process ends — acceptable for the CLI today; revisit
        // with a cancellation token when this backend is held long-lived.
        tokio::spawn(async move {
            use primer_core::reasoning::ReasoningFilter;
            let mut buf = NdjsonBuffer::new();
            let mut filter = ReasoningFilter::new(markers);
            let mut had_visible = false;
            'outer: loop {
                match bytes_stream.next().await {
                    Some(Ok(bytes)) => {
                        buf.extend(&bytes);
                        while let Some(line) = buf.pop_line() {
                            if line.trim().is_empty() {
                                continue;
                            }
                            let chunk = match parse_ollama_line(&line) {
                                Ok(c) => c,
                                Err(e) => {
                                    // A mid-stream `{"error":"..."}` payload is a
                                    // real generation failure, not line noise:
                                    // surface it so the partial turn drops at the
                                    // dialogue-manager layer instead of the reply
                                    // finishing as a clean truncated turn.
                                    if let Some(msg) = parse_ollama_error_line(&line) {
                                        let _ = tx
                                            .send(Err(PrimerError::Inference(
                                                format!("Ollama mid-stream error: {msg}").into(),
                                            )))
                                            .await;
                                        break 'outer;
                                    }
                                    tracing::warn!("Skipping unparseable Ollama line: {e}");
                                    continue;
                                }
                            };
                            match crate::reasoning_stream::process_filtered_chunk(
                                &mut filter,
                                chunk,
                                &mut had_visible,
                                "ollama",
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
                                format!("Ollama byte stream error: {e}").into(),
                            )))
                            .await;
                        break 'outer;
                    }
                    None => {
                        // Upstream closed without a `done` marker (abrupt EOF). A
                        // normal stream sends a done chunk, hits the `Final` arm,
                        // and breaks before reaching here — so reaching `None`
                        // means we must still flush. Feed a synthetic done chunk
                        // through the shared helper so a held-back visible tail
                        // still reaches the child and a stream that ended
                        // mid-reasoning surfaces `ReasoningWithoutAnswer` instead
                        // of silently producing nothing.
                        if let crate::reasoning_stream::FilterAction::Final(r) =
                            crate::reasoning_stream::process_filtered_chunk(
                                &mut filter,
                                TokenChunk {
                                    text: String::new(),
                                    done: true,
                                    ..Default::default()
                                },
                                &mut had_visible,
                                "ollama",
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
