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

/// Parse one `data:` payload from the OpenAI SSE stream into a
/// `TokenChunk`. Returns `Ok(None)` for chunks with no content
/// (e.g. the initial role-only delta or empty choices).
fn parse_openai_compat_chunk(data: &str) -> Result<Option<TokenChunk>> {
    if data == "[DONE]" {
        return Ok(Some(TokenChunk {
            text: String::new(),
            done: true,
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
    }))
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- SSE buffer ---------------------------------------------------------

    #[test]
    fn sse_buffer_yields_data_payload() {
        let mut buf = OpenAiSseBuffer::new();
        buf.extend(b"data: {\"choices\":[]}\n");
        assert_eq!(buf.pop_data().as_deref(), Some("{\"choices\":[]}"));
        assert_eq!(buf.pop_data(), None);
    }

    #[test]
    fn sse_buffer_holds_partial_line() {
        let mut buf = OpenAiSseBuffer::new();
        buf.extend(b"data: {\"ch");
        assert_eq!(buf.pop_data(), None);
        buf.extend(b"oices\":[]}\n");
        assert_eq!(buf.pop_data().as_deref(), Some("{\"choices\":[]}"));
    }

    #[test]
    fn sse_buffer_skips_comment_lines() {
        let mut buf = OpenAiSseBuffer::new();
        buf.extend(b": keepalive\ndata: hello\n");
        assert_eq!(buf.pop_data().as_deref(), Some("hello"));
    }

    #[test]
    fn sse_buffer_skips_blank_and_event_lines() {
        let mut buf = OpenAiSseBuffer::new();
        buf.extend(b"\nevent: message\nid: 42\ndata: payload\n");
        assert_eq!(buf.pop_data().as_deref(), Some("payload"));
        assert_eq!(buf.pop_data(), None);
    }

    #[test]
    fn sse_buffer_handles_two_data_lines_in_one_chunk() {
        let mut buf = OpenAiSseBuffer::new();
        buf.extend(b"data: first\ndata: second\n");
        assert_eq!(buf.pop_data().as_deref(), Some("first"));
        assert_eq!(buf.pop_data().as_deref(), Some("second"));
        assert_eq!(buf.pop_data(), None);
    }

    #[test]
    fn sse_buffer_strips_only_one_leading_space() {
        let mut buf = OpenAiSseBuffer::new();
        buf.extend(b"data:   spaced\n");
        assert_eq!(buf.pop_data().as_deref(), Some("  spaced"));
    }

    #[test]
    fn sse_buffer_handles_crlf() {
        let mut buf = OpenAiSseBuffer::new();
        buf.extend(b"data: crlf\r\n");
        assert_eq!(buf.pop_data().as_deref(), Some("crlf"));
    }

    // -- Chunk parser -------------------------------------------------------

    #[test]
    fn parse_chunk_extracts_content() {
        let data = r#"{"id":"x","choices":[{"delta":{"content":"Hi"},"finish_reason":null}]}"#;
        let chunk = parse_openai_compat_chunk(data).unwrap().unwrap();
        assert_eq!(chunk.text, "Hi");
        assert!(!chunk.done);
    }

    #[test]
    fn parse_chunk_done_on_finish_reason() {
        let data = r#"{"id":"x","choices":[{"delta":{"content":""},"finish_reason":"stop"}]}"#;
        let chunk = parse_openai_compat_chunk(data).unwrap().unwrap();
        assert!(chunk.done);
    }

    #[test]
    fn parse_chunk_done_marker() {
        let chunk = parse_openai_compat_chunk("[DONE]").unwrap().unwrap();
        assert_eq!(chunk.text, "");
        assert!(chunk.done);
    }

    #[test]
    fn parse_chunk_skips_role_only_delta() {
        let data = r#"{"id":"x","choices":[{"delta":{"role":"assistant"},"finish_reason":null}]}"#;
        assert!(parse_openai_compat_chunk(data).unwrap().is_none());
    }

    #[test]
    fn parse_chunk_skips_empty_choices() {
        let data = r#"{"id":"x","choices":[]}"#;
        assert!(parse_openai_compat_chunk(data).unwrap().is_none());
    }

    #[test]
    fn parse_chunk_returns_err_on_garbage() {
        assert!(parse_openai_compat_chunk("not json").is_err());
    }

    // -- Error classification -----------------------------------------------

    mod classify_tests {
        use primer_core::error::InferenceError;
        use reqwest::StatusCode;
        use reqwest::header::{HeaderMap, HeaderValue};
        use std::time::Duration;

        use super::super::classify_openai_compat_status;

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
            let e =
                classify_openai_compat_status(StatusCode::UNAUTHORIZED, &empty_headers(), "", "m");
            assert!(matches!(e, InferenceError::Auth));
        }

        #[test]
        fn classifies_403_as_auth() {
            let e = classify_openai_compat_status(StatusCode::FORBIDDEN, &empty_headers(), "", "m");
            assert!(matches!(e, InferenceError::Auth));
        }

        #[test]
        fn classifies_429_with_retry_after() {
            let e = classify_openai_compat_status(
                StatusCode::TOO_MANY_REQUESTS,
                &headers_with_retry_after(5),
                "",
                "m",
            );
            match e {
                InferenceError::RateLimited {
                    retry_after: Some(d),
                } => {
                    assert_eq!(d, Duration::from_secs(5))
                }
                other => panic!("expected RateLimited(Some(5s)), got {other:?}"),
            }
        }

        #[test]
        fn classifies_429_without_retry_after() {
            let e = classify_openai_compat_status(
                StatusCode::TOO_MANY_REQUESTS,
                &empty_headers(),
                "",
                "m",
            );
            assert!(matches!(
                e,
                InferenceError::RateLimited { retry_after: None }
            ));
        }

        #[test]
        fn classifies_404_model_not_found() {
            let e = classify_openai_compat_status(
                StatusCode::NOT_FOUND,
                &empty_headers(),
                r#"{"error":{"message":"The model `foo` does not exist"}}"#,
                "foo",
            );
            match e {
                InferenceError::ModelNotFound { model } => assert_eq!(model, "foo"),
                other => panic!("expected ModelNotFound, got {other:?}"),
            }
        }

        #[test]
        fn classifies_404_generic_as_other() {
            let e = classify_openai_compat_status(
                StatusCode::NOT_FOUND,
                &empty_headers(),
                "page not found",
                "m",
            );
            assert!(matches!(e, InferenceError::Other(_)));
        }

        /// A 404 whose body merely mentions both "model" and "not" but
        /// not in a model-not-found context must NOT be classified as
        /// `ModelNotFound`. Guards against the old loose `contains("model")
        /// && contains("not")` rule.
        #[test]
        fn classifies_404_with_unrelated_model_and_not_keywords_as_other() {
            let e = classify_openai_compat_status(
                StatusCode::NOT_FOUND,
                &empty_headers(),
                "did you mean to add the model_not_found handler?",
                "m",
            );
            assert!(
                matches!(e, InferenceError::Other(_)),
                "expected Other, got {e:?}"
            );
        }

        #[test]
        fn classifies_404_model_not_found_vllm_phrasing() {
            let e = classify_openai_compat_status(
                StatusCode::NOT_FOUND,
                &empty_headers(),
                r#"{"error":"model not found: foo"}"#,
                "foo",
            );
            match e {
                InferenceError::ModelNotFound { model } => assert_eq!(model, "foo"),
                other => panic!("expected ModelNotFound, got {other:?}"),
            }
        }

        #[test]
        fn classifies_404_model_not_found_groq_phrasing() {
            let e = classify_openai_compat_status(
                StatusCode::NOT_FOUND,
                &empty_headers(),
                r#"{"error":{"message":"unknown model foo"}}"#,
                "foo",
            );
            assert!(matches!(e, InferenceError::ModelNotFound { .. }));
        }

        #[test]
        fn classifies_404_model_not_found_omlx_phrasing() {
            let e = classify_openai_compat_status(
                StatusCode::NOT_FOUND,
                &empty_headers(),
                "no such model: foo",
                "foo",
            );
            assert!(matches!(e, InferenceError::ModelNotFound { .. }));
        }

        #[test]
        fn classifies_500_as_service_unavailable() {
            let e = classify_openai_compat_status(
                StatusCode::INTERNAL_SERVER_ERROR,
                &empty_headers(),
                "",
                "m",
            );
            assert!(matches!(e, InferenceError::ServiceUnavailable));
        }

        #[test]
        fn classifies_503_as_service_unavailable() {
            let e = classify_openai_compat_status(
                StatusCode::SERVICE_UNAVAILABLE,
                &empty_headers(),
                "",
                "m",
            );
            assert!(matches!(e, InferenceError::ServiceUnavailable));
        }
    }

    // -- Request serialization ----------------------------------------------

    #[test]
    fn request_serialization_includes_all_fields() {
        let req = ChatCompletionRequest {
            model: "test-model".to_string(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: "Be helpful.".to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: "Hello".to_string(),
                },
            ],
            stream: true,
            temperature: 0.7,
            top_p: 0.9,
            max_tokens: 512,
            stop: vec![],
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"model\":\"test-model\""));
        assert!(json.contains("\"stream\":true"));
        assert!(json.contains("\"temperature\":0.7"));
        assert!(json.contains("\"top_p\":0.9"));
        assert!(json.contains("\"max_tokens\":512"));
        assert!(
            !json.contains("\"stop\""),
            "empty stop should be omitted, got: {json}"
        );
    }

    #[test]
    fn request_serialization_includes_stop_when_nonempty() {
        let req = ChatCompletionRequest {
            model: "m".to_string(),
            messages: vec![],
            stream: true,
            temperature: 0.7,
            top_p: 0.9,
            max_tokens: 512,
            stop: vec!["END".to_string()],
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"stop\":[\"END\"]"));
    }

    // -- Reasoning-filter wiring (real shared helper) -----------------------

    mod reasoning_wiring {
        use super::*;
        use crate::reasoning_stream::{FilterAction, process_filtered_chunk};
        use primer_core::reasoning::ReasoningFilter;

        fn drive(backend: &OpenAiCompatBackend, payloads: &[&str]) -> (String, bool) {
            let mut filter = ReasoningFilter::new(backend.reasoning_markers.clone());
            let mut had_visible = false;
            let mut visible = String::new();
            let mut err = false;
            for p in payloads {
                let Some(chunk) = parse_openai_compat_chunk(p).unwrap() else {
                    continue;
                };
                match process_filtered_chunk(&mut filter, chunk, &mut had_visible, "openai-compat")
                {
                    FilterAction::Nothing => {}
                    FilterAction::Forward(r) => visible.push_str(&r.unwrap().text),
                    FilterAction::Final(r) => {
                        match r {
                            Ok(c) => visible.push_str(&c.text),
                            Err(_) => err = true,
                        }
                        break;
                    }
                }
            }
            (visible, err)
        }

        fn delta(content: &str) -> String {
            format!(r#"{{"choices":[{{"delta":{{"content":"{content}"}},"finish_reason":null}}]}}"#)
        }

        fn stop() -> String {
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#.to_string()
        }

        #[test]
        fn strips_think_block_end_to_end() {
            let b = OpenAiCompatBackend::new("http://x".into(), "m".into(), None);
            let payloads = [delta("<think>plan</think>"), delta("Hi there"), stop()];
            let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
            let (visible, err) = drive(&b, &refs);
            assert_eq!(visible, "Hi there");
            assert!(!err);
        }

        #[test]
        fn only_reasoning_yields_error() {
            let b = OpenAiCompatBackend::new("http://x".into(), "m".into(), None);
            let payloads = [delta("<think>only thinking"), stop()];
            let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
            let (visible, err) = drive(&b, &refs);
            assert_eq!(visible, "");
            assert!(err);
        }

        #[test]
        fn custom_marker_extends_defaults() {
            let b = OpenAiCompatBackend::new("http://x".into(), "m".into(), None)
                .with_extra_markers(vec![("[[r]]".into(), "[[/r]]".into())]);
            let payloads = [delta("a[[r]]hidden[[/r]]b"), stop()];
            let refs: Vec<&str> = payloads.iter().map(|s| s.as_str()).collect();
            let (visible, err) = drive(&b, &refs);
            assert_eq!(visible, "ab");
            assert!(!err);
        }
    }
}
