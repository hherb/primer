//! Cloud inference backend — calls the Anthropic Claude API.
//!
//! This is the high-quality path, used when the device has WiFi connectivity.
//! The pedagogical engine constructs the prompt locally; only the generation
//! is offloaded to the cloud. The child's conversation history is sent
//! per-request and not stored by the API (per Anthropic's data policy).

use async_trait::async_trait;
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use primer_core::error::{PrimerError, Result};
use primer_core::inference::*;
use serde::{Deserialize, Serialize};

/// Translate a non-success Anthropic HTTP response into an
/// `InferenceError`. Pure — no I/O, no `Self` reference.
///
/// `body` is the response body text used only as the dev-facing payload
/// of `InferenceError::Other`; users never see it (the i18n render layer
/// substitutes a generic message for `Other`).
pub(crate) fn classify_anthropic_status(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: &str,
) -> primer_core::error::InferenceError {
    use primer_core::error::InferenceError::*;
    match status.as_u16() {
        401 | 403 => Auth,
        429 => RateLimited {
            retry_after: parse_retry_after(headers),
        },
        500..=599 => ServiceUnavailable,
        _ => Other(format!("Anthropic returned {status}: {body}")),
    }
}

/// Parse a `Retry-After` header in integer-seconds form. The
/// HTTP-date form is silently dropped (returns `None`) — Anthropic
/// uses the integer-seconds form in practice and parsing the date
/// form would cost a `chrono`/`httpdate` dep for a rare case.
pub(crate) fn parse_retry_after(
    headers: &reqwest::header::HeaderMap,
) -> Option<std::time::Duration> {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
}

/// Translate the `error.type` from a mid-stream Anthropic SSE `error`
/// event into an `InferenceError`. Pure — no I/O, no `Self` reference.
///
/// Mid-stream errors fire after a 2xx handshake, so the retry helper
/// has already exited; this classification exists purely so the
/// user-facing i18n layer surfaces the right message instead of the
/// generic `Other` fallback. Mapped from Anthropic's documented error
/// taxonomy: `overloaded_error`/`api_error` → ServiceUnavailable,
/// `rate_limit_error` → RateLimited (no Retry-After in JSON form),
/// `authentication_error`/`permission_error` → Auth.
///
/// `message` is dev-facing; users never see it (the i18n render layer
/// substitutes a generic message for `Other`).
pub(crate) fn classify_anthropic_error_event(
    error_type: &str,
    message: &str,
) -> primer_core::error::InferenceError {
    use primer_core::error::InferenceError::*;
    match error_type {
        "overloaded_error" | "api_error" => ServiceUnavailable,
        "rate_limit_error" => RateLimited { retry_after: None },
        "authentication_error" | "permission_error" => Auth,
        _ => Other(format!("Anthropic {error_type}: {message}")),
    }
}

/// Translate a transport-level `reqwest::Error` (no HTTP response yet)
/// into an `InferenceError`. Connect / timeout / request-build failures
/// map to `NetworkUnavailable`; everything else falls through to
/// `Other`. Used by both backend modules — kept in this file because
/// it has no Anthropic-specific knowledge but is small enough that
/// a dedicated shared module is overkill.
pub(crate) fn classify_reqwest_error(e: &reqwest::Error) -> primer_core::error::InferenceError {
    if e.is_connect() || e.is_timeout() || e.is_request() {
        primer_core::error::InferenceError::NetworkUnavailable
    } else {
        primer_core::error::InferenceError::Other(format!("reqwest: {e}"))
    }
}

pub struct CloudBackend {
    client: reqwest::Client,
    api_endpoint: String,
    api_key: String,
    model: String,
    retry_settings: primer_core::retry::RetrySettings,
}

impl CloudBackend {
    pub fn new(api_endpoint: String, api_key: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_endpoint,
            api_key,
            model,
            retry_settings: primer_core::retry::RetrySettings::default(),
        }
    }
}

/// Anthropic Messages API request body (simplified).
///
/// Notably absent: `top_p`. Newer Anthropic models (`claude-sonnet-4-6` and
/// later) return a 400 when both `temperature` and `top_p` are specified.
/// We pick `temperature` as the canonical knob; `top_p` from
/// `GenerationParams` is silently dropped on this backend.
#[derive(Debug, Serialize)]
struct ApiRequest {
    model: String,
    max_tokens: u32,
    system: String,
    messages: Vec<ApiMessage>,
    temperature: f32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stop_sequences: Vec<String>,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct ApiMessage {
    role: String,
    content: String,
}

/// One Server-Sent-Event from the Anthropic streaming API.
#[derive(Debug)]
struct SseEvent {
    event: String,
    data: String,
}

/// Incremental SSE parser. Feed bytes via `extend()`, pull complete events
/// via `next_event()`. Holds partial data across chunk boundaries.
///
/// The Anthropic stream sends events separated by blank lines, where each
/// event is a sequence of `event: <name>` and `data: <json>` lines.
struct SseBuffer {
    /// Bytes received but not yet split into lines.
    bytes: Vec<u8>,
    /// Lines already split off from `bytes` but not yet assembled into an event.
    lines: std::collections::VecDeque<String>,
}

impl SseBuffer {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            lines: std::collections::VecDeque::new(),
        }
    }

    fn extend(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
        while let Some(nl) = self.bytes.iter().position(|b| *b == b'\n') {
            let line_bytes: Vec<u8> = self.bytes.drain(..=nl).take(nl).collect();
            // Trim a trailing '\r' for CRLF tolerance.
            let mut line = String::from_utf8_lossy(&line_bytes).into_owned();
            if line.ends_with('\r') {
                line.pop();
            }
            self.lines.push_back(line);
        }
    }

    fn next_event(&mut self) -> Option<SseEvent> {
        // Walk pending lines, building an event until a blank-line terminator
        // is found. Lines starting with `:` are SSE comments (heartbeats).
        loop {
            // Scan ahead for a blank line. If none, no complete event yet.
            let blank_idx = self.lines.iter().position(|l| l.is_empty())?;

            // Collect lines [0, blank_idx) into one event, drop the blank.
            let mut event_name: Option<String> = None;
            let mut data = String::new();
            for _ in 0..blank_idx {
                let line = self.lines.pop_front().expect("checked above");
                if line.starts_with(':') {
                    continue; // SSE comment / heartbeat.
                }
                if let Some(rest) = line.strip_prefix("event:") {
                    event_name = Some(rest.trim().to_string());
                } else if let Some(rest) = line.strip_prefix("data:") {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    // Per SSE spec, exactly ONE optional leading space is
                    // stripped after `data:`. Additional whitespace is part
                    // of the payload.
                    data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                }
                // Any other field (id:, retry:) — ignore.
            }
            // Drop the blank terminator.
            self.lines.pop_front();

            if let Some(event) = event_name {
                return Some(SseEvent { event, data });
            }
            // No event line in that block — probably a pure-comment heartbeat
            // followed by a blank. Loop and try the next event.
        }
    }
}

#[derive(Debug, Deserialize)]
struct ContentBlockDelta {
    delta: TextDelta,
}

#[derive(Debug, Deserialize)]
struct TextDelta {
    #[serde(default)]
    text: String,
}

/// The `message_delta` event's payload. Anthropic reports the
/// terminal `stop_reason` here, in a separate event from the
/// `message_stop` that flags the end of the stream.
#[derive(Debug, Deserialize)]
struct MessageDelta {
    delta: MessageDeltaBody,
}

#[derive(Debug, Deserialize)]
struct MessageDeltaBody {
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ErrorPayload {
    error: ErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ErrorDetail {
    #[serde(rename = "type", default)]
    error_type: String,
    #[serde(default)]
    message: String,
}

/// Translate one Anthropic SSE event into an optional `TokenChunk`.
/// Returns `Ok(None)` for events we can safely ignore.
fn parse_anthropic_event(ev: &SseEvent) -> Result<Option<TokenChunk>> {
    match ev.event.as_str() {
        "content_block_delta" => {
            let parsed: ContentBlockDelta = serde_json::from_str(&ev.data).map_err(|e| {
                PrimerError::Inference(
                    format!(
                        "Anthropic content_block_delta parse error: {e}; data: {}",
                        ev.data
                    )
                    .into(),
                )
            })?;
            Ok(Some(TokenChunk {
                text: parsed.delta.text,
                done: false,
                ..Default::default()
            }))
        }
        "message_stop" => Ok(Some(TokenChunk {
            text: String::new(),
            done: true,
            ..Default::default()
        })),
        "error" => {
            let parsed: ErrorPayload = serde_json::from_str(&ev.data).map_err(|e| {
                PrimerError::Inference(
                    format!("Anthropic error event parse failed: {e}; data: {}", ev.data).into(),
                )
            })?;
            Err(PrimerError::Inference(classify_anthropic_error_event(
                &parsed.error.error_type,
                &parsed.error.message,
            )))
        }
        // ping, message_start, content_block_start, content_block_stop, message_delta.
        // `message_delta` carries `stop_reason`, but it arrives in a *separate*
        // event from the terminal `message_stop` — so the finish reason is
        // extracted by `anthropic_finish_reason_from_event` and threaded onto
        // the terminal chunk by `AnthropicEventTranslator`, not here.
        _ => Ok(None),
    }
}

/// Map Anthropic's `stop_reason` to a [`FinishReason`]. `"max_tokens"`
/// (the reply hit the `max_tokens` cap / context window) becomes
/// [`FinishReason::Length`] so the dialogue manager's recovery loop
/// fires; every other reason (`"end_turn"`, `"stop_sequence"`, …) stays
/// [`FinishReason::Stop`].
fn map_anthropic_stop_reason(stop_reason: &str) -> FinishReason {
    match stop_reason {
        "max_tokens" => FinishReason::Length,
        _ => FinishReason::Stop,
    }
}

/// Extract the finish reason from a `message_delta` event, if it carries
/// a `stop_reason`. Returns `None` for any other event and for a
/// `message_delta` with no `stop_reason` (so it never overwrites a
/// reason already observed).
fn anthropic_finish_reason_from_event(ev: &SseEvent) -> Option<FinishReason> {
    if ev.event != "message_delta" {
        return None;
    }
    let parsed: MessageDelta = serde_json::from_str(&ev.data).ok()?;
    parsed
        .delta
        .stop_reason
        .as_deref()
        .map(map_anthropic_stop_reason)
}

/// Stateful translator from Anthropic SSE events to `TokenChunk`s.
///
/// Anthropic delivers the terminal `stop_reason` (on `message_delta`)
/// and the end-of-stream marker (on `message_stop`) in two separate
/// events. The translator remembers the most recent stop reason and
/// stamps it onto the terminal chunk, so the dialogue manager sees a
/// `done` chunk whose `finish_reason` reflects whether the reply was
/// cut off by the context window.
struct AnthropicEventTranslator {
    pending_finish: FinishReason,
}

impl AnthropicEventTranslator {
    fn new() -> Self {
        Self {
            pending_finish: FinishReason::Stop,
        }
    }

    /// Translate one event into the chunk to forward (if any). The
    /// terminal chunk carries the tracked finish reason.
    fn step(&mut self, ev: &SseEvent) -> Result<Option<TokenChunk>> {
        if let Some(fr) = anthropic_finish_reason_from_event(ev) {
            self.pending_finish = fr;
        }
        let mut chunk = match parse_anthropic_event(ev)? {
            Some(c) => c,
            None => return Ok(None),
        };
        if chunk.done {
            chunk.finish_reason = self.pending_finish;
        }
        Ok(Some(chunk))
    }
}

#[async_trait]
impl InferenceBackend for CloudBackend {
    fn name(&self) -> &str {
        "cloud-anthropic"
    }

    async fn is_available(&self) -> bool {
        // Simple connectivity check — try to reach the API endpoint.
        self.client.head(&self.api_endpoint).send().await.is_ok()
    }

    async fn generate_stream(
        &self,
        prompt: &Prompt,
        params: &GenerationParams,
    ) -> Result<TokenStream> {
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
            stop_sequences: params.stop_sequences.clone(),
            stream: true,
        };

        // Retry the send + status-check phase only. Once we have a 2xx
        // response and start consuming the byte stream, mid-stream errors
        // propagate as before (the partial Primer turn is dropped at a
        // higher layer).
        let response = primer_core::retry::retry_with_backoff(&self.retry_settings, || async {
            let resp = self
                .client
                .post(format!("{}/v1/messages", self.api_endpoint))
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .json(&request)
                .send()
                .await
                .map_err(|e| classify_reqwest_error(&e))?;
            let status = resp.status();
            if !status.is_success() {
                let headers = resp.headers().clone();
                let body = resp.text().await.unwrap_or_default();
                return Err(classify_anthropic_status(status, &headers, &body));
            }
            Ok(resp)
        })
        .await
        .map_err(PrimerError::Inference)?;

        let (mut tx, rx) = mpsc::unbounded::<Result<TokenChunk>>();
        let mut bytes_stream = response.bytes_stream();

        // Fire-and-forget: see the matching note in `ollama.rs`. Same
        // caveat: long-lived backends will eventually want a cancellation
        // token instead of relying on consumer-drop to stop the task.
        tokio::spawn(async move {
            let mut buf = SseBuffer::new();
            let mut translator = AnthropicEventTranslator::new();
            'outer: loop {
                match bytes_stream.next().await {
                    Some(Ok(bytes)) => {
                        buf.extend(&bytes);
                        while let Some(ev) = buf.next_event() {
                            match translator.step(&ev) {
                                Ok(Some(chunk)) => {
                                    let done = chunk.done;
                                    if tx.send(Ok(chunk)).await.is_err() {
                                        break 'outer;
                                    }
                                    if done {
                                        break 'outer;
                                    }
                                }
                                Ok(None) => {} // ignorable event
                                Err(e) => {
                                    let _ = tx.send(Err(e)).await;
                                    break 'outer;
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        let _ = tx
                            .send(Err(PrimerError::Inference(
                                format!("Anthropic byte stream error: {e}").into(),
                            )))
                            .await;
                        break 'outer;
                    }
                    None => break 'outer,
                }
            }
        });

        Ok(Box::pin(rx))
    }
}

#[cfg(test)]
mod classify_tests;
#[cfg(test)]
mod tests;
