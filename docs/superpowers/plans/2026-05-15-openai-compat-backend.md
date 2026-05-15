# OpenAI-Compatible Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an OpenAI-compatible inference and embedding backend so the Primer can talk to oMLX, LM Studio, vLLM, llama.cpp `--server`, and remote OpenAI-format providers (Together, Groq, OpenRouter).

**Architecture:** New `openai_compat` module in `primer-inference` implementing `InferenceBackend` with SSE streaming via `/v1/chat/completions`, plus a sibling in `primer-embedding` implementing `Embedder` via `/v1/embeddings` with native batching. CLI + engine wiring adds `--backend openai-compat` with URL/key/model flags. The same `build_backend` dispatch matrix means classifier/extractor/comprehension automatically get `openai-compat` support.

**Tech Stack:** Rust, reqwest, serde, serde_json, futures, async-trait, tokio — all existing workspace deps. No new external crates.

**Spec:** `docs/superpowers/specs/2026-05-15-openai-compat-backend-design.md`

---

### Task 1: SSE buffer and line parser — tests first

**Files:**
- Create: `src/crates/primer-inference/src/openai_compat.rs`

This task creates the `OpenAiSseBuffer` that buffers raw bytes from an HTTP streaming response and yields `data:` payloads, plus the `parse_openai_compat_chunk` function that turns one JSON payload into a `TokenChunk`. Written test-first.

- [ ] **Step 1: Create the file with module doc, imports, and SSE buffer struct**

Create `src/crates/primer-inference/src/openai_compat.rs`:

```rust
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

/// Delta content from one SSE chunk.
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
/// `TokenChunk`. Returns `Ok(None)` for `[DONE]` or chunks with no
/// content (e.g. the initial role-only delta).
fn parse_openai_compat_chunk(data: &str) -> Result<Option<TokenChunk>> {
    if data == "[DONE]" {
        return Ok(Some(TokenChunk {
            text: String::new(),
            done: true,
        }));
    }
    let chunk: ChatCompletionChunk = serde_json::from_str(data).map_err(|e| {
        PrimerError::Inference(
            format!("OpenAI-compat SSE parse error: {e}; data: {data}").into(),
        )
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
```

- [ ] **Step 2: Write unit tests for the SSE buffer and chunk parser**

Append the test module to the same file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

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
}
```

- [ ] **Step 3: Register the module in lib.rs**

In `src/crates/primer-inference/src/lib.rs`, add:

```rust
pub mod openai_compat;
pub use openai_compat::OpenAiCompatBackend;
```

(The struct doesn't exist yet — add `pub` to the struct definition in step 4. For now the `pub use` line will fail but that's fine; we'll add the struct in task 2.)

- [ ] **Step 4: Run the tests**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-inference openai_compat -- -v`

Expected: All 12 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-inference/src/openai_compat.rs src/crates/primer-inference/src/lib.rs
git commit -m "feat(inference): add OpenAI-compat SSE buffer + chunk parser with tests"
```

---

### Task 2: Error classification and request types

**Files:**
- Modify: `src/crates/primer-inference/src/openai_compat.rs`

- [ ] **Step 1: Write error classification tests**

Add to the `#[cfg(test)] mod tests` block:

```rust
    mod classify_tests {
        use primer_core::error::InferenceError;
        use reqwest::StatusCode;
        use reqwest::header::{HeaderMap, HeaderValue};
        use std::time::Duration;

        use super::super::classify_openai_compat_status;
        use crate::cloud::parse_retry_after;

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
            let e = classify_openai_compat_status(
                StatusCode::UNAUTHORIZED,
                &empty_headers(),
                "",
                "m",
            );
            assert!(matches!(e, InferenceError::Auth));
        }

        #[test]
        fn classifies_403_as_auth() {
            let e = classify_openai_compat_status(
                StatusCode::FORBIDDEN,
                &empty_headers(),
                "",
                "m",
            );
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
                InferenceError::RateLimited { retry_after: Some(d) } => {
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
```

- [ ] **Step 2: Add the error classification function and request types**

Add before the `#[cfg(test)]` block in `openai_compat.rs`:

```rust
/// Translate a non-success HTTP response from an OpenAI-compatible
/// server into an `InferenceError`. Pure — no I/O, no `Self`.
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
        404 if body_lower.contains("model") && body_lower.contains("does not exist") => {
            ModelNotFound {
                model: requested_model.to_string(),
            }
        }
        404 if body_lower.contains("model") && body_lower.contains("not") => ModelNotFound {
            model: requested_model.to_string(),
        },
        500..=599 => ServiceUnavailable,
        _ => Other(format!("OpenAI-compat returned {status}: {body}")),
    }
}

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
```

- [ ] **Step 3: Write a serialization test**

Add to the `tests` module:

```rust
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
```

- [ ] **Step 4: Run the tests**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-inference openai_compat -- -v`

Expected: All 22 tests pass (12 from task 1 + 10 new).

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-inference/src/openai_compat.rs
git commit -m "feat(inference): add OpenAI-compat error classification + request types"
```

---

### Task 3: OpenAiCompatBackend struct + InferenceBackend impl

**Files:**
- Modify: `src/crates/primer-inference/src/openai_compat.rs`

- [ ] **Step 1: Add the backend struct and constructor**

Add after the `ChatMessage` struct:

```rust
pub struct OpenAiCompatBackend {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
    retry_settings: primer_core::retry::RetrySettings,
}

impl OpenAiCompatBackend {
    pub fn new(base_url: String, model: String, api_key: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            model,
            api_key,
            retry_settings: primer_core::retry::RetrySettings::default(),
        }
    }
}
```

- [ ] **Step 2: Implement InferenceBackend**

Add:

```rust
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

        let (mut tx, rx) = mpsc::unbounded::<Result<TokenChunk>>();
        let mut bytes_stream = response.bytes_stream();

        tokio::spawn(async move {
            let mut buf = OpenAiSseBuffer::new();
            'outer: loop {
                match bytes_stream.next().await {
                    Some(Ok(bytes)) => {
                        buf.extend(&bytes);
                        while let Some(data) = buf.pop_data() {
                            match parse_openai_compat_chunk(&data) {
                                Ok(Some(chunk)) => {
                                    let done = chunk.done;
                                    if tx.send(Ok(chunk)).await.is_err() {
                                        break 'outer;
                                    }
                                    if done {
                                        break 'outer;
                                    }
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    tracing::warn!(
                                        "Skipping unparseable OpenAI-compat SSE line: {e}"
                                    );
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        let _ = tx
                            .send(Err(PrimerError::Inference(
                                format!("OpenAI-compat byte stream error: {e}").into(),
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
```

- [ ] **Step 3: Verify lib.rs re-export compiles**

The `pub use openai_compat::OpenAiCompatBackend;` added in task 1 step 3 should now resolve. Run:

Run: `cd src && ~/.cargo/bin/cargo build -p primer-inference`

Expected: Compiles cleanly.

- [ ] **Step 4: Run all inference tests**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-inference -- -v`

Expected: All tests pass (existing + new).

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-inference/src/openai_compat.rs
git commit -m "feat(inference): add OpenAiCompatBackend implementing InferenceBackend"
```

---

### Task 4: Engine wiring — BackendParams + build_backend

**Files:**
- Modify: `src/crates/primer-engine/src/wiring.rs`
- Modify: `src/crates/primer-engine/src/lib.rs`

- [ ] **Step 1: Add fields to BackendParams**

In `src/crates/primer-engine/src/wiring.rs`, add two fields to `BackendParams`:

```rust
pub struct BackendParams {
    pub api_key: Option<String>,
    pub ollama_url: String,
    pub openai_compat_url: String,
    pub openai_compat_api_key: Option<String>,
    pub classifier_backend: Option<String>,
    pub classifier_model: Option<String>,
    pub extractor_backend: Option<String>,
    pub extractor_model: Option<String>,
    pub comprehension_backend: Option<String>,
    pub comprehension_model: Option<String>,
}
```

- [ ] **Step 2: Add the `openai-compat` arm to `build_backend`**

In the `match backend_name` block, add before the `other =>` arm:

```rust
        "openai-compat" => Ok(Arc::new(
            primer_inference::openai_compat::OpenAiCompatBackend::new(
                params.openai_compat_url.clone(),
                model,
                params.openai_compat_api_key.clone(),
            ),
        )),
```

- [ ] **Step 3: Fix all existing BackendParams construction sites in tests**

Every test that constructs `BackendParams` needs the two new fields. Search for all occurrences in `wiring.rs` and add:

```rust
            openai_compat_url: "http://localhost:8000".into(),
            openai_compat_api_key: None,
```

There are three test helper functions that construct `BackendParams`:
1. `classifier_construction_tests::params_with` (~line 378)
2. `extractor_construction_tests::params` (~line 527)
3. `comprehension_construction_tests::params` (~line 629)

Add the two new fields to each.

- [ ] **Step 4: Build and run tests**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-engine -- -v`

Expected: All existing tests pass; the new `"openai-compat"` arm is exercised implicitly by the unknown-backend test (which tests `"nonexistent"` — our new arm doesn't interfere).

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-engine/src/wiring.rs
git commit -m "feat(engine): wire openai-compat backend into build_backend dispatch"
```

---

### Task 5: CLI flags for openai-compat backend

**Files:**
- Modify: `src/crates/primer-cli/src/main.rs`

- [ ] **Step 1: Add CLI flags to the `Cli` struct**

After the `ollama_url` field, add:

```rust
    /// OpenAI-compatible server URL (used when --backend openai-compat).
    /// Works with oMLX, LM Studio, vLLM, llama.cpp --server, etc.
    #[arg(long, default_value = "http://localhost:8000", env = "OPENAI_COMPAT_URL")]
    openai_compat_url: String,

    /// API key for OpenAI-compatible servers that require auth
    /// (Together, Groq, OpenRouter). Local servers like oMLX/LM Studio
    /// typically don't need one.
    #[arg(long, env = "OPENAI_COMPAT_API_KEY")]
    openai_compat_api_key: Option<String>,
```

- [ ] **Step 2: Update the model resolution block**

In the `main_model` match, add a new arm before the wildcard:

```rust
        "openai-compat" => cli.model.clone().unwrap_or_else(|| {
            eprintln!("Error: --model required for openai-compat backend.");
            std::process::exit(1);
        }),
```

- [ ] **Step 3: Thread the new fields into BackendParams**

In the `BackendParams` construction site, add:

```rust
        openai_compat_url: cli.openai_compat_url.clone(),
        openai_compat_api_key: cli.openai_compat_api_key.clone(),
```

- [ ] **Step 4: Add the banner line for openai-compat**

In the `match cli.backend.as_str()` block that prints the startup banner, add before `other =>`:

```rust
        "openai-compat" => eprintln!(
            "Using openai-compat backend at {} with model {main_model}.",
            cli.openai_compat_url
        ),
```

- [ ] **Step 5: Update the error message for unknown backends**

Change the `other =>` arm's error message to include `'openai-compat'`:

```rust
        other => {
            eprintln!("Unknown backend: {other}. Use 'stub', 'cloud', 'ollama', or 'openai-compat'.");
            std::process::exit(1);
        }
```

- [ ] **Step 6: Build and verify**

Run: `cd src && ~/.cargo/bin/cargo build --bin primer`

Expected: Compiles cleanly.

Run: `cd src && ~/.cargo/bin/cargo run --bin primer -- --help 2>&1 | grep -A1 openai-compat`

Expected: Shows the `--openai-compat-url` and `--openai-compat-api-key` flags.

- [ ] **Step 7: Commit**

```bash
git add src/crates/primer-cli/src/main.rs
git commit -m "feat(cli): add --backend openai-compat with URL and API key flags"
```

---

### Task 6: OpenAI-compat embedder — tests first

**Files:**
- Create: `src/crates/primer-embedding/src/openai_compat.rs`
- Modify: `src/crates/primer-embedding/src/lib.rs`
- Modify: `src/crates/primer-embedding/Cargo.toml`

- [ ] **Step 1: Add the `openai-compat` feature to Cargo.toml**

In `src/crates/primer-embedding/Cargo.toml`, add to the `[features]` section:

```toml
openai-compat = ["dep:reqwest", "dep:serde", "dep:serde_json"]
```

- [ ] **Step 2: Create the embedder module**

Create `src/crates/primer-embedding/src/openai_compat.rs`:

```rust
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
    pub async fn new(
        base_url: &str,
        model_name: &str,
        api_key: Option<String>,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| {
                PrimerError::Inference(
                    format!("openai-compat embedder client build failed: {e}").into(),
                )
            })?;

        let probe_vec = embed_batch(&client, base_url, model_name, api_key.as_deref(), &["primer"])
            .await?;
        let dim = probe_vec
            .first()
            .ok_or_else(|| {
                PrimerError::Inference(
                    "openai-compat embedder probe returned no vectors".into(),
                )
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
        let vecs =
            embed_batch(&self.client, &self.base_url, &self.model_name, self.api_key.as_deref(), texts)
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
        PrimerError::Inference(
            format!("openai-compat embedding response parse error: {e}").into(),
        )
    })?;

    let mut sorted = body.data;
    sorted.sort_by_key(|d| d.index);
    Ok(sorted.into_iter().map(|d| d.embedding).collect())
}

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
```

- [ ] **Step 3: Register the module in lib.rs**

In `src/crates/primer-embedding/src/lib.rs`, add:

```rust
#[cfg(feature = "openai-compat")]
pub mod openai_compat;
#[cfg(feature = "openai-compat")]
pub use openai_compat::{DEFAULT_OPENAI_COMPAT_URL, OpenAiCompatEmbedder};
```

- [ ] **Step 4: Run the tests**

Run: `cd src && ~/.cargo/bin/cargo test -p primer-embedding --features openai-compat -- openai_compat -v`

Expected: All 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/crates/primer-embedding/src/openai_compat.rs src/crates/primer-embedding/src/lib.rs src/crates/primer-embedding/Cargo.toml
git commit -m "feat(embedding): add OpenAI-compat embedder with native batching"
```

---

### Task 7: Engine + CLI wiring for the embedder

**Files:**
- Modify: `src/crates/primer-engine/src/wiring.rs`
- Modify: `src/crates/primer-engine/src/lib.rs`
- Modify: `src/crates/primer-engine/Cargo.toml`
- Modify: `src/crates/primer-cli/Cargo.toml`
- Modify: `src/crates/primer-cli/src/main.rs`

- [ ] **Step 1: Add `openai-compat-embedding` feature to primer-engine**

In `src/crates/primer-engine/Cargo.toml`, add to `[features]`:

```toml
openai-compat-embedding = ["primer-embedding/openai-compat"]
```

- [ ] **Step 2: Add `build_openai_compat_embedder` to wiring.rs**

Add after `build_ollama_embedder`:

```rust
#[cfg(feature = "openai-compat-embedding")]
pub async fn build_openai_compat_embedder(
    url: Option<&str>,
    model: Option<&str>,
    api_key: Option<String>,
) -> std::result::Result<Option<Arc<dyn primer_core::embedder::Embedder>>, String> {
    use primer_embedding::{DEFAULT_OPENAI_COMPAT_URL, OpenAiCompatEmbedder};
    let url = url.unwrap_or(DEFAULT_OPENAI_COMPAT_URL);
    let model = model.ok_or_else(|| {
        "--embedder-openai-compat-model is required when --embedder-backend is openai-compat"
            .to_string()
    })?;
    match OpenAiCompatEmbedder::new(url, model, api_key).await {
        Ok(b) => {
            eprintln!("Embedder: openai-compat {model} at {url}");
            Ok(Some(Arc::new(b) as _))
        }
        Err(e) => {
            eprintln!(
                "openai-compat embedder init failed ({e}); falling back to BM25-only retrieval."
            );
            Ok(None)
        }
    }
}

#[cfg(not(feature = "openai-compat-embedding"))]
pub async fn build_openai_compat_embedder(
    _url: Option<&str>,
    _model: Option<&str>,
    _api_key: Option<String>,
) -> std::result::Result<Option<Arc<dyn primer_core::embedder::Embedder>>, String> {
    Err(
        "openai-compat embedder requires the `openai-compat-embedding` cargo feature. \
         Build with `cargo run --features primer-cli/openai-compat-embedding -- ...`"
            .to_string(),
    )
}
```

- [ ] **Step 3: Re-export from lib.rs**

In `src/crates/primer-engine/src/lib.rs`, update the `pub use wiring::` line to include `build_openai_compat_embedder`:

```rust
pub use wiring::{
    BackendParams, build_backend, build_classifier, build_comprehension, build_extractor,
    build_fastembed_embedder, build_ollama_embedder, build_openai_compat_embedder,
};
```

- [ ] **Step 4: Add `openai-compat-embedding` feature to primer-cli**

In `src/crates/primer-cli/Cargo.toml`, add to `[features]`:

```toml
openai-compat-embedding = ["primer-embedding/openai-compat", "primer-engine/openai-compat-embedding"]
```

- [ ] **Step 5: Add CLI flags for embedder**

In `src/crates/primer-cli/src/main.rs`, add to the `Cli` struct after `embedder_ollama_url`:

```rust
    /// Override the URL for `--embedder-backend openai-compat`.
    /// Defaults to `--openai-compat-url` if set, otherwise
    /// `http://localhost:8000`.
    #[arg(long, value_name = "URL")]
    embedder_openai_compat_url: Option<String>,

    /// Model name for `--embedder-backend openai-compat` (required).
    #[arg(long, value_name = "NAME")]
    embedder_openai_compat_model: Option<String>,
```

- [ ] **Step 6: Update the import and embedder dispatch**

Update the import line at the top:

```rust
use primer_engine::{
    BackendParams, IN_MEMORY, build_backend, build_classifier, build_comprehension,
    build_extractor, build_fastembed_embedder, build_ollama_embedder,
    build_openai_compat_embedder, create_learner_with_id,
    reconcile_persisted_learner, resolve_session_db_path, should_show_first_run_banner,
    verify_resume_locale_match,
};
```

In the embedder dispatch `match`, add before the `other =>` arm:

```rust
        "openai-compat" => {
            let url = cli
                .embedder_openai_compat_url
                .as_deref()
                .or(Some(cli.openai_compat_url.as_str()));
            match build_openai_compat_embedder(
                url,
                cli.embedder_openai_compat_model.as_deref().or(cli.embedder_model.as_deref()),
                cli.openai_compat_api_key.clone(),
            )
            .await
            {
                Ok(opt) => opt,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }
```

Update the error message in the `other =>` arm:

```rust
        other => {
            eprintln!(
                "Error: unknown --embedder-backend {other:?}; expected one of none, stub, fastembed, ollama, openai-compat"
            );
            std::process::exit(1);
        }
```

- [ ] **Step 7: Build and verify**

Run: `cd src && ~/.cargo/bin/cargo build --bin primer`

Expected: Compiles cleanly.

Run: `cd src && ~/.cargo/bin/cargo run --bin primer -- --help 2>&1 | grep -A1 embedder-openai`

Expected: Shows the `--embedder-openai-compat-url` and `--embedder-openai-compat-model` flags.

- [ ] **Step 8: Commit**

```bash
git add src/crates/primer-engine/src/wiring.rs src/crates/primer-engine/src/lib.rs src/crates/primer-engine/Cargo.toml src/crates/primer-cli/Cargo.toml src/crates/primer-cli/src/main.rs
git commit -m "feat(cli): wire openai-compat embedder into engine + CLI dispatch"
```

---

### Task 8: Full workspace build + test verification

**Files:**
- No new files.

- [ ] **Step 1: Run clippy across the workspace**

Run: `cd src && ~/.cargo/bin/cargo clippy --workspace --all-targets 2>&1`

Expected: No warnings from our code (third-party crate warnings are OK).

- [ ] **Step 2: Run cargo fmt**

Run: `cd src && ~/.cargo/bin/cargo fmt`

Expected: No formatting changes (if there are, stage them).

- [ ] **Step 3: Run all workspace tests**

Run: `cd src && ~/.cargo/bin/cargo test`

Expected: All tests pass.

- [ ] **Step 4: Commit any formatting fixes**

```bash
git add -u
git commit -m "style: cargo fmt"
```

(Only if step 2 produced changes.)

---

### Task 9: CLAUDE.md documentation update

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update the CLI flags section**

In the `## Common commands` section, add a usage example after the ollama line:

```
cargo run --bin primer -- --backend openai-compat --model mlx-community/Qwen3-8B-4bit   # OpenAI-compatible server (oMLX, LM Studio, vLLM, etc.)
```

- [ ] **Step 2: Update the CLI flags paragraph**

In the long CLI flags paragraph, update the `--backend` description to include `openai-compat`:

Add `openai-compat` to the `--backend stub|cloud|ollama|openai-compat` pattern.

Add descriptions for the new flags: `--openai-compat-url <url>` (default `http://localhost:8000`; or env `OPENAI_COMPAT_URL`), `--openai-compat-api-key <key>` (or env `OPENAI_COMPAT_API_KEY`; optional for local servers, required for remote providers), `--embedder-backend` gains `openai-compat`, `--embedder-openai-compat-url <url>` (falls back to `--openai-compat-url`), `--embedder-openai-compat-model <name>` (required when embedder backend is `openai-compat`).

- [ ] **Step 3: Add architecture notes**

Add a bullet in the "Conventions and gotchas" section:

```
- **`OpenAiCompatBackend` speaks the OpenAI chat-completions SSE format (`/v1/chat/completions`).** System prompt is a `"system"` role message (same as Ollama). SSE parsing uses `OpenAiSseBuffer` which yields `data:` payloads and ignores comment/event/id lines. `data: [DONE]` terminates the stream. `finish_reason != null` also terminates. The SSE format is structurally different from Anthropic's (which uses `event:` + `data:` pairs with typed event names like `content_block_delta`); the parsers are deliberately separate. `classify_openai_compat_status` handles error classification (401/403 → Auth, 429 → RateLimited, 404+model → ModelNotFound, 5xx → ServiceUnavailable). Optional `Authorization: Bearer` header for remote providers. Retry wraps pre-stream phase only, same as Cloud and Ollama.
- **`OpenAiCompatEmbedder` sends the full batch in one request** via the standard `/v1/embeddings` endpoint (unlike `OllamaEmbedder` which loops one-at-a-time). Feature-gated behind `openai-compat` in `primer-embedding/Cargo.toml`. Results are sorted by `index` to match input order (the spec allows out-of-order responses). Dim-mismatch is a hard error, same as `OllamaEmbedder`.
```

- [ ] **Step 4: Update the `BackendParams` note**

In any existing mention of `BackendParams`, note it now has `openai_compat_url: String` and `openai_compat_api_key: Option<String>`.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: add OpenAI-compat backend to CLAUDE.md"
```
