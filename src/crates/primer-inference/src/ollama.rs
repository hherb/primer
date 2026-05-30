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
    })
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
            let mut total_visible: usize = 0;
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
                                    tracing::warn!("Skipping unparseable Ollama line: {e}");
                                    continue;
                                }
                            };
                            match crate::reasoning_stream::process_filtered_chunk(
                                &mut filter,
                                chunk,
                                &mut total_visible,
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
                        let _ = tx
                            .send(Err(PrimerError::Inference(
                                format!("Ollama byte stream error: {e}").into(),
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
    fn ndjson_buffer_yields_complete_line() {
        let mut buf = NdjsonBuffer::new();
        buf.extend(b"{\"a\":1}\n");
        assert_eq!(buf.pop_line().as_deref(), Some("{\"a\":1}"));
        assert_eq!(buf.pop_line(), None);
    }

    #[test]
    fn ndjson_buffer_holds_partial_line_until_newline() {
        let mut buf = NdjsonBuffer::new();
        buf.extend(b"{\"a\":");
        assert_eq!(buf.pop_line(), None);
        buf.extend(b"1}\n");
        assert_eq!(buf.pop_line().as_deref(), Some("{\"a\":1}"));
        assert_eq!(buf.pop_line(), None);
    }

    #[test]
    fn ndjson_buffer_handles_two_lines_in_one_chunk() {
        let mut buf = NdjsonBuffer::new();
        buf.extend(b"line1\nline2\n");
        assert_eq!(buf.pop_line().as_deref(), Some("line1"));
        assert_eq!(buf.pop_line().as_deref(), Some("line2"));
        assert_eq!(buf.pop_line(), None);
    }

    #[test]
    fn ndjson_buffer_handles_line_split_across_three_chunks() {
        let mut buf = NdjsonBuffer::new();
        buf.extend(b"li");
        assert_eq!(buf.pop_line(), None);
        buf.extend(b"ne");
        assert_eq!(buf.pop_line(), None);
        buf.extend(b"1\n");
        assert_eq!(buf.pop_line().as_deref(), Some("line1"));
        assert_eq!(buf.pop_line(), None);
    }

    #[test]
    fn parse_ollama_line_emits_chunk_for_content() {
        let chunk = parse_ollama_line(r#"{"message":{"content":"Hello"},"done":false}"#).unwrap();
        assert_eq!(chunk.text, "Hello");
        assert!(!chunk.done);
    }

    #[test]
    fn parse_ollama_line_emits_done_chunk() {
        let chunk = parse_ollama_line(r#"{"message":{"content":""},"done":true}"#).unwrap();
        assert_eq!(chunk.text, "");
        assert!(chunk.done);
    }

    #[test]
    fn parse_ollama_line_returns_err_on_garbage() {
        assert!(parse_ollama_line("not json").is_err());
    }

    #[test]
    fn ndjson_buffer_surfaces_invalid_utf8_lossily() {
        // A broken UTF-8 byte (0xff) should NOT silently disappear. The line
        // must be surfaced (lossy-decoded with U+FFFD) so the caller can log
        // and skip it via the normal parse-error path.
        let mut buf = NdjsonBuffer::new();
        buf.extend(&[b'a', 0xff, b'b', b'\n']);
        let line = buf
            .pop_line()
            .expect("invalid UTF-8 line should still be surfaced");
        assert!(line.contains('a'));
        assert!(line.contains('b'));
        assert!(line.contains('\u{FFFD}'));
        assert_eq!(buf.pop_line(), None);
    }

    mod classify_ollama_tests {
        use super::*;
        use primer_core::error::InferenceError;
        use reqwest::StatusCode;

        #[test]
        fn classifies_404_with_model_not_found_body() {
            let body = r#"{"error":"model 'llama3.2' not found, try pulling it first"}"#;
            let e = classify_ollama_status(StatusCode::NOT_FOUND, body, "llama3.2");
            match e {
                InferenceError::ModelNotFound { model } => assert_eq!(model, "llama3.2"),
                other => panic!("expected ModelNotFound, got {other:?}"),
            }
        }

        #[test]
        fn classifies_404_without_model_body_as_other() {
            let e = classify_ollama_status(StatusCode::NOT_FOUND, "page not found", "llama3.2");
            assert!(matches!(e, InferenceError::Other(_)));
        }

        #[test]
        fn classifies_500_as_service_unavailable() {
            let e = classify_ollama_status(StatusCode::INTERNAL_SERVER_ERROR, "", "llama3.2");
            assert!(matches!(e, InferenceError::ServiceUnavailable));
        }

        #[test]
        fn classifies_503_as_service_unavailable() {
            let e = classify_ollama_status(StatusCode::SERVICE_UNAVAILABLE, "", "llama3.2");
            assert!(matches!(e, InferenceError::ServiceUnavailable));
        }

        #[test]
        fn classifies_other_4xx_as_other() {
            let e = classify_ollama_status(StatusCode::BAD_REQUEST, "bad payload", "llama3.2");
            match e {
                InferenceError::Other(s) => assert!(s.contains("400") && s.contains("bad payload")),
                other => panic!("expected Other, got {other:?}"),
            }
        }
    }

    mod reasoning_wiring {
        use super::*;

        /// Drive a sequence of NDJSON lines through the REAL shared filter
        /// helper, collecting visible text and whether an error chunk was
        /// emitted.
        fn drive(backend: &OllamaBackend, lines: &[&str]) -> (String, bool) {
            use crate::reasoning_stream::{FilterAction, process_filtered_chunk};
            use primer_core::reasoning::ReasoningFilter;
            let mut filter = ReasoningFilter::new(backend.reasoning_markers.clone());
            let mut total = 0usize;
            let mut visible = String::new();
            let mut err = false;
            for line in lines {
                let chunk = parse_ollama_line(line).unwrap();
                match process_filtered_chunk(&mut filter, chunk, &mut total, "ollama") {
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

        #[test]
        fn strips_think_block_end_to_end() {
            let b = OllamaBackend::new("http://x".into(), "m".into());
            let lines = [
                r#"{"message":{"content":"<think>plan</think>"},"done":false}"#,
                r#"{"message":{"content":"Hi there"},"done":false}"#,
                r#"{"message":{"content":""},"done":true}"#,
            ];
            let (visible, err) = drive(&b, &lines);
            assert_eq!(visible, "Hi there");
            assert!(!err);
        }

        #[test]
        fn only_reasoning_yields_error() {
            let b = OllamaBackend::new("http://x".into(), "m".into());
            let lines = [
                r#"{"message":{"content":"<think>only thinking"},"done":false}"#,
                r#"{"message":{"content":""},"done":true}"#,
            ];
            let (visible, err) = drive(&b, &lines);
            assert_eq!(visible, "");
            assert!(err);
        }

        #[test]
        fn custom_marker_extends_defaults() {
            let b = OllamaBackend::new("http://x".into(), "m".into())
                .with_extra_markers(vec![("[[r]]".into(), "[[/r]]".into())]);
            let lines = [
                r#"{"message":{"content":"a[[r]]hidden[[/r]]b"},"done":false}"#,
                r#"{"message":{"content":""},"done":true}"#,
            ];
            let (visible, err) = drive(&b, &lines);
            assert_eq!(visible, "ab");
            assert!(!err);
        }
    }
}
