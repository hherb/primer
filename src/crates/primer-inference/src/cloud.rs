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
            }))
        }
        "message_stop" => Ok(Some(TokenChunk {
            text: String::new(),
            done: true,
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
        // Note: `message_delta` carries `stop_reason` which can flag mid-stream
        // refusals or `max_tokens` truncation (i.e. "successful but incomplete"
        // outcomes). We currently treat these as benign — the consumer sees a
        // shorter response. If those need to surface as errors, inspect
        // `stop_reason` here before falling through.
        _ => Ok(None),
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
            'outer: loop {
                match bytes_stream.next().await {
                    Some(Ok(bytes)) => {
                        buf.extend(&bytes);
                        while let Some(ev) = buf.next_event() {
                            match parse_anthropic_event(&ev) {
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
mod tests {
    use super::*;

    #[test]
    fn sse_buffer_yields_complete_event() {
        let mut buf = SseBuffer::new();
        buf.extend(b"event: content_block_delta\ndata: {\"x\":1}\n\n");
        let ev = buf.next_event().unwrap();
        assert_eq!(ev.event, "content_block_delta");
        assert_eq!(ev.data, "{\"x\":1}");
        assert!(buf.next_event().is_none());
    }

    #[test]
    fn sse_buffer_handles_event_split_across_chunks() {
        let mut buf = SseBuffer::new();
        buf.extend(b"event: message_stop\n");
        assert!(buf.next_event().is_none());
        buf.extend(b"data: {}\n\n");
        let ev = buf.next_event().unwrap();
        assert_eq!(ev.event, "message_stop");
        assert_eq!(ev.data, "{}");
    }

    #[test]
    fn sse_buffer_handles_two_events_in_one_chunk() {
        let mut buf = SseBuffer::new();
        buf.extend(
            b"event: content_block_delta\ndata: {\"a\":1}\n\nevent: message_stop\ndata: {}\n\n",
        );
        let e1 = buf.next_event().unwrap();
        assert_eq!(e1.event, "content_block_delta");
        assert_eq!(e1.data, "{\"a\":1}");
        let e2 = buf.next_event().unwrap();
        assert_eq!(e2.event, "message_stop");
        assert_eq!(e2.data, "{}");
        assert!(buf.next_event().is_none());
    }

    #[test]
    fn sse_buffer_ignores_comment_and_blank_lines_outside_event() {
        let mut buf = SseBuffer::new();
        buf.extend(b":keepalive\n\nevent: ping\ndata: {}\n\n");
        // The ":keepalive\n\n" should not produce a phantom event.
        let ev = buf.next_event().unwrap();
        assert_eq!(ev.event, "ping");
        assert!(buf.next_event().is_none());
    }

    #[test]
    fn parse_anthropic_event_yields_text_delta() {
        let ev = SseEvent {
            event: "content_block_delta".to_string(),
            data: r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#
                .to_string(),
        };
        let chunk = parse_anthropic_event(&ev).unwrap().unwrap();
        assert_eq!(chunk.text, "Hi");
        assert!(!chunk.done);
    }

    #[test]
    fn parse_anthropic_event_yields_done_on_message_stop() {
        let ev = SseEvent {
            event: "message_stop".to_string(),
            data: "{}".to_string(),
        };
        let chunk = parse_anthropic_event(&ev).unwrap().unwrap();
        assert_eq!(chunk.text, "");
        assert!(chunk.done);
    }

    #[test]
    fn parse_anthropic_event_skips_ignorable_events() {
        for name in [
            "ping",
            "message_start",
            "content_block_start",
            "content_block_stop",
            "message_delta",
        ] {
            let ev = SseEvent {
                event: name.to_string(),
                data: "{}".to_string(),
            };
            assert!(
                parse_anthropic_event(&ev).unwrap().is_none(),
                "expected {name} to be ignored"
            );
        }
    }

    #[test]
    fn api_request_omits_top_p() {
        // Anthropic's newer models (e.g. claude-sonnet-4-6) reject requests
        // that set both `temperature` and `top_p`. We pick temperature as the
        // canonical knob and skip `top_p` from the wire format entirely.
        let req = ApiRequest {
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 100,
            system: "s".to_string(),
            messages: vec![],
            temperature: 0.7,
            stop_sequences: vec![],
            stream: true,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            !json.contains("top_p"),
            "top_p must not be in the request body, got: {json}"
        );
        assert!(
            json.contains("\"temperature\":0.7"),
            "temperature should still be sent"
        );
    }

    #[test]
    fn sse_buffer_strips_only_one_space_after_data_prefix() {
        // Per the SSE spec, ONE optional leading space is stripped after
        // `data:`. Additional whitespace must be preserved as part of the
        // payload. Anthropic's JSON never relies on this, but if we ever
        // reuse this parser for another source it must not corrupt payloads.
        let mut buf = SseBuffer::new();
        buf.extend(b"event: x\ndata:   spaced\n\n");
        let ev = buf.next_event().unwrap();
        assert_eq!(ev.data, "  spaced");
    }

    #[test]
    fn parse_anthropic_event_overloaded_error_is_typed_service_unavailable() {
        // Mid-stream `overloaded_error` events must surface as the typed
        // ServiceUnavailable variant so the i18n layer renders the right
        // friendly message (otherwise users see the generic Other fallback
        // even though Anthropic told us exactly what went wrong).
        let ev = SseEvent {
            event: "error".to_string(),
            data: r#"{"type":"error","error":{"type":"overloaded_error","message":"slow down"}}"#
                .to_string(),
        };
        let err = parse_anthropic_event(&ev).unwrap_err();
        match err {
            primer_core::error::PrimerError::Inference(
                primer_core::error::InferenceError::ServiceUnavailable,
            ) => {}
            other => panic!("expected typed ServiceUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn parse_anthropic_event_rate_limit_error_is_typed_rate_limited() {
        let ev = SseEvent {
            event: "error".to_string(),
            data: r#"{"type":"error","error":{"type":"rate_limit_error","message":"too many"}}"#
                .to_string(),
        };
        let err = parse_anthropic_event(&ev).unwrap_err();
        match err {
            primer_core::error::PrimerError::Inference(
                primer_core::error::InferenceError::RateLimited { retry_after: None },
            ) => {}
            other => panic!("expected typed RateLimited{{None}}, got {other:?}"),
        }
    }

    #[test]
    fn parse_anthropic_event_authentication_error_is_typed_auth() {
        let ev = SseEvent {
            event: "error".to_string(),
            data: r#"{"type":"error","error":{"type":"authentication_error","message":"bad key"}}"#
                .to_string(),
        };
        let err = parse_anthropic_event(&ev).unwrap_err();
        match err {
            primer_core::error::PrimerError::Inference(
                primer_core::error::InferenceError::Auth,
            ) => {}
            other => panic!("expected typed Auth, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod classify_tests {
    use super::*;
    use primer_core::error::InferenceError;
    use reqwest::StatusCode;
    use reqwest::header::{HeaderMap, HeaderValue};
    use std::time::Duration;

    fn empty_headers() -> HeaderMap {
        HeaderMap::new()
    }

    fn headers_with_retry_after(seconds: u64) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("retry-after", HeaderValue::from(seconds));
        h
    }

    #[test]
    fn classifies_401_as_auth() {
        let e = classify_anthropic_status(StatusCode::UNAUTHORIZED, &empty_headers(), "");
        assert!(matches!(e, InferenceError::Auth));
    }

    #[test]
    fn classifies_403_as_auth() {
        let e = classify_anthropic_status(StatusCode::FORBIDDEN, &empty_headers(), "");
        assert!(matches!(e, InferenceError::Auth));
    }

    #[test]
    fn classifies_429_with_retry_after_header() {
        let e = classify_anthropic_status(
            StatusCode::TOO_MANY_REQUESTS,
            &headers_with_retry_after(7),
            "rate limited",
        );
        match e {
            InferenceError::RateLimited {
                retry_after: Some(d),
            } => assert_eq!(d, Duration::from_secs(7)),
            other => panic!("expected RateLimited(Some(7s)), got {other:?}"),
        }
    }

    #[test]
    fn classifies_429_without_retry_after_header() {
        let e = classify_anthropic_status(StatusCode::TOO_MANY_REQUESTS, &empty_headers(), "");
        assert!(matches!(
            e,
            InferenceError::RateLimited { retry_after: None }
        ));
    }

    #[test]
    fn classifies_500_as_service_unavailable() {
        let e = classify_anthropic_status(StatusCode::INTERNAL_SERVER_ERROR, &empty_headers(), "");
        assert!(matches!(e, InferenceError::ServiceUnavailable));
    }

    #[test]
    fn classifies_503_as_service_unavailable() {
        let e = classify_anthropic_status(StatusCode::SERVICE_UNAVAILABLE, &empty_headers(), "");
        assert!(matches!(e, InferenceError::ServiceUnavailable));
    }

    #[test]
    fn classifies_unknown_4xx_as_other_with_status_in_body() {
        let e = classify_anthropic_status(StatusCode::IM_A_TEAPOT, &empty_headers(), "tea");
        match e {
            InferenceError::Other(s) => {
                assert!(s.contains("418") && s.contains("tea"), "got: {s}")
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn parse_retry_after_handles_integer_seconds() {
        let h = headers_with_retry_after(42);
        assert_eq!(parse_retry_after(&h), Some(Duration::from_secs(42)));
    }

    #[test]
    fn parse_retry_after_handles_missing_header() {
        assert_eq!(parse_retry_after(&empty_headers()), None);
    }

    #[test]
    fn parse_retry_after_drops_http_date_form() {
        // HTTP-date form is silently dropped (returns None) rather than
        // attempting to parse the date — see spec §Retry helper.
        let mut h = HeaderMap::new();
        h.insert(
            "retry-after",
            HeaderValue::from_static("Wed, 21 Oct 2026 07:28:00 GMT"),
        );
        assert_eq!(parse_retry_after(&h), None);
    }

    #[test]
    fn classify_error_event_overloaded_is_service_unavailable() {
        let e = classify_anthropic_error_event("overloaded_error", "slow down");
        assert!(matches!(e, InferenceError::ServiceUnavailable));
    }

    #[test]
    fn classify_error_event_api_error_is_service_unavailable() {
        let e = classify_anthropic_error_event("api_error", "internal blip");
        assert!(matches!(e, InferenceError::ServiceUnavailable));
    }

    #[test]
    fn classify_error_event_rate_limit_is_rate_limited_no_retry_after() {
        // Mid-stream JSON error events have no Retry-After header
        // available; surface RateLimited with retry_after=None.
        let e = classify_anthropic_error_event("rate_limit_error", "too many");
        assert!(matches!(
            e,
            InferenceError::RateLimited { retry_after: None }
        ));
    }

    #[test]
    fn classify_error_event_authentication_is_auth() {
        let e = classify_anthropic_error_event("authentication_error", "bad key");
        assert!(matches!(e, InferenceError::Auth));
    }

    #[test]
    fn classify_error_event_permission_is_auth() {
        let e = classify_anthropic_error_event("permission_error", "no access");
        assert!(matches!(e, InferenceError::Auth));
    }

    #[test]
    fn classify_error_event_unknown_type_falls_through_to_other() {
        let e = classify_anthropic_error_event("invalid_request_error", "bad json");
        match e {
            InferenceError::Other(s) => {
                assert!(
                    s.contains("invalid_request_error") && s.contains("bad json"),
                    "Other should surface both type and message for diagnostics, got: {s}"
                );
            }
            other => panic!("expected Other for unmapped type, got {other:?}"),
        }
    }
}
