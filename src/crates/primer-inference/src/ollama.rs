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

pub struct OllamaBackend {
    client: reqwest::Client,
    base_url: String,
    model: String,
}

impl OllamaBackend {
    pub fn new(base_url: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            model,
        }
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
    let chunk: ChatChunk = serde_json::from_str(line)
        .map_err(|e| PrimerError::Inference(format!("Ollama NDJSON parse error: {e}; line: {line}")))?;
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

        let response = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&request)
            .send()
            .await
            .map_err(|e| PrimerError::Inference(format!("Ollama request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(PrimerError::Inference(format!(
                "Ollama returned {status}: {body}"
            )));
        }

        let (mut tx, rx) = mpsc::unbounded::<Result<TokenChunk>>();
        let mut bytes_stream = response.bytes_stream();

        // Fire-and-forget: no JoinHandle. If the consumer drops `rx`, the
        // first `tx.send` will fail and the task exits naturally. If the
        // upstream hangs without closing the connection, the task lives
        // until the process ends — acceptable for the CLI today; revisit
        // with a cancellation token when this backend is held long-lived.
        tokio::spawn(async move {
            let mut buf = NdjsonBuffer::new();
            'outer: loop {
                match bytes_stream.next().await {
                    Some(Ok(bytes)) => {
                        buf.extend(&bytes);
                        while let Some(line) = buf.pop_line() {
                            if line.trim().is_empty() {
                                continue;
                            }
                            match parse_ollama_line(&line) {
                                Ok(chunk) => {
                                    let done = chunk.done;
                                    if tx.send(Ok(chunk)).await.is_err() {
                                        break 'outer;
                                    }
                                    if done {
                                        break 'outer;
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("Skipping unparseable Ollama line: {e}");
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        let _ = tx
                            .send(Err(PrimerError::Inference(format!(
                                "Ollama byte stream error: {e}"
                            ))))
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
        let line = buf.pop_line().expect("invalid UTF-8 line should still be surfaced");
        assert!(line.contains('a'));
        assert!(line.contains('b'));
        assert!(line.contains('\u{FFFD}'));
        assert_eq!(buf.pop_line(), None);
    }
}
